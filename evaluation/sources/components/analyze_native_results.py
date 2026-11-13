"""解压后独立核验原TDB生产稳定性证据，复算三次构建与十五个固定查询位置。"""

import argparse
import hashlib
import json
import math
import statistics
import struct
import sys
from pathlib import Path, PurePosixPath

import numpy as np

from analyze_results import QueryBootstrap, digest, read_json, require, value_digest, write_csv


CONFIG_SHA = "fbfe981ffadc9fd5583efae0320a45cf53c4cc0b665ba346f0bf028f3d39dcf8"
BASE_SHA = "5511265695bee911b49615d62a47c78c5018c81ad8e0152deb5db119ad474226"
POSITIONS = [(88, 1), (88, 8), (88, 16), (88, 32), (384, 1)]
GIB = 1024 ** 3


def same_number(actual, expected, message):
    require(math.isfinite(float(actual)) and math.isclose(float(actual), float(expected), rel_tol=1e-11, abs_tol=1e-12), message)


def describe(values):
    values = list(map(float, values))
    require(len(values) >= 2 and all(math.isfinite(v) for v in values), "构建重复统计样本不足或非有限")
    mean = statistics.mean(values)
    sd = statistics.stdev(values)
    return dict(samples=values, count=len(values), mean=mean, sample_sd=sd, cv=sd / mean if mean else None)


class Analysis:
    def __init__(self, evidence, expected_config_sha, archive=None):
        self.evidence = evidence.resolve(strict=True)
        self.manifest = read_json(self.evidence / "export-manifest.json")
        self.internal = read_json(self.evidence / "export-receipt.json")
        self.expected_config_sha = expected_config_sha
        self.archive = archive
        self.by_source = {}
        self.hashes = {}
        self.stamps = {}
        self.builds = []
        self.points = []
        self.raw_requests = 0

    def member(self, name):
        parsed = PurePosixPath(name)
        require(not parsed.is_absolute() and ".." not in parsed.parts and ":" not in name, "归档路径越界")
        path = self.evidence.joinpath(*parsed.parts)
        require(not path.is_symlink() and path.resolve().is_relative_to(self.evidence), "归档成员使用符号链接或越界")
        return path

    def checked(self, path, expected):
        path = Path(path)
        relative = path.relative_to(self.evidence).as_posix()
        require(relative in self.hashes and self.hashes[relative] == expected, "文件不属于已核归档或SHA不匹配：" + relative)
        return path

    def resolve(self, original, expected=None):
        candidates = self.by_source.get(str(original), [])
        for candidate in candidates:
            if expected is None or self.hashes[candidate.relative_to(self.evidence).as_posix()] == expected:
                return candidate
        raise ValueError("无法定位身份匹配的原始文件：" + str(original))

    def verify_export(self):
        require(self.manifest.get("status") == "passed" and self.internal.get("status") == "passed", "归档未完整成功")
        binding = self.internal["manifest"]
        require(binding["file"] == "export-manifest.json" and (self.evidence / binding["file"]).stat().st_size == binding["bytes"]
                and digest(self.evidence / binding["file"]) == binding["sha256"], "内部回执未绑定完整归档清单")
        require(self.manifest["config_sha256"] == self.internal["config_sha256"] == self.expected_config_sha, "生产配置身份不符")
        require(value_digest(self.manifest["files"]) == self.manifest["inventory_sha256"] == self.internal["inventory_sha256"], "归档成员清单身份链不同")
        require(self.manifest.get("production_tdb_qps") is True and self.manifest.get("controlled_component_engine") is False,
                "归档类型不是原TDB生产测量")
        for name, identity in self.manifest["files"].items():
            path = self.member(name)
            require(path.is_file() and path.stat().st_size == identity["bytes"], "归档成员长度不符：" + name)
            actual = digest(path)
            require(actual == identity["sha256"], "归档成员SHA不符：" + name)
            self.hashes[name] = actual
            value = path.stat()
            self.stamps[name] = (value.st_size, value.st_mtime_ns, value.st_ctime_ns)
            self.by_source.setdefault(identity["source_path"], []).append(path)
        require(len(self.hashes) == self.internal["payload_files"], "有效归档成员数量不符")
        if self.archive:
            outer = read_json(self.archive.with_name(self.archive.name + ".receipt.json"))
            require(outer.get("status") == "passed" and self.archive.stat().st_size == outer["zip"]["bytes"]
                    and digest(self.archive) == outer["zip"]["sha256"], "外层ZIP回执不符")
            require(outer["config_sha256"] == self.expected_config_sha and outer["manifest"] == binding
                    and outer["inventory_sha256"] == self.manifest["inventory_sha256"]
                    and digest(self.evidence / "export-receipt.json") == outer["internal_receipt"]["sha256"],
                    "外层回执未绑定解压后的内部证据")
        self.config_path = self.checked(self.evidence / "native-config.json", self.expected_config_sha)
        self.config = read_json(self.config_path)
        require((self.config["n"], self.config["dim"], self.config["build_repeats"], self.config["rounds"]) == (1000000, 768, 3, 5), "生产规模或轮数变化")
        require([(p["ef"], p["threads"]) for p in self.config["positions"]] == POSITIONS, "固定查询位置变化")
        require(self.config["warmup_batches"] >= 3 and self.config["warmup_seconds"] >= 1
                and self.config["throughput_seconds"] >= 5 and self.config["cpus"] == list(range(64, 96)), "计量时长或CPU协议变化")
        self.verify_sources_and_inputs()

    def verify_sources_and_inputs(self):
        config = self.config
        self.resolve(config["binary"], config["binary_sha256"])
        for name, sha in config["source_sha256"].items():
            self.checked(self.evidence / "source" / name, sha)
        identity = read_json(self.resolve(config["source_identity"]["path"], config["source_identity"]["sha256"]))
        require(identity["source_files_matched"] == 121 and not identity["missing"] and not identity["mismatched"], "crate身份不完整")
        for name, sha in identity["repo_files_sha256"].items():
            self.checked(self.evidence / "frozen-tdb/repo" / name, sha)
        for name, value in identity["runner"].items():
            if isinstance(value, dict):
                self.checked(self.evidence / "frozen-tdb/runner" / name, value["sha256"])
        inputs = {item["file"]: item for item in config["inputs"]}
        require(inputs["base.f32"]["sha256"] == BASE_SHA and inputs["base.f32"]["bytes"] == 3072000000, "基础1M向量身份变化")
        require(any(item["file"] == "base.f32" and item["sha256"] == BASE_SHA for item in self.manifest["omitted_large_inputs"]), "缺少原向量省略及重建记录")
        for name in ("queries.f32", "truth10.i32"):
            path = self.evidence / "inputs/D03-1M" / name
            self.checked(path, inputs[name]["sha256"])
            require(path.stat().st_size == inputs[name]["bytes"], "可复算输入长度不符")
        query_path = self.evidence / "inputs/D03-1M/queries.f32"
        require(query_path.stat().st_size == 744 * 768 * 4, "原744查询长度不符")
        self.truth = np.fromfile(self.evidence / "inputs/D03-1M/truth10.i32", dtype="<i4").reshape(744, 10)
        require((self.truth >= 0).all() and (self.truth < 1000000).all()
                and all(len(set(row)) == 10 for row in self.truth), "原始Top10真值非法")
        ids_path = self.evidence / "inputs/D03-1M/query-ids.json"
        self.checked(ids_path, "215bfd861c3bc657a1a65761240ba3d443a52d127ef3eb467046d91361675ac6")
        ids = np.asarray(read_json(ids_path))
        require(ids.shape == (744,) and ids.dtype.kind in "iu" and len(np.unique(ids)) == 744, "查询编号未形成744个唯一查询簇")
        self.bootstrap = QueryBootstrap(ids, "原744条查询；各查询的五轮延迟保持在同一簇内")
        prerequisite = read_json(self.resolve(config["prerequisite"]["path"], config["prerequisite"]["sha256"]))
        require(prerequisite.get("status") == "passed", "受控组件先决队列未成功")
        self.checked(self.evidence / "context/component-plan.json", prerequisite["plan_sha256"])
        self.checked(self.evidence / "context/component-results-manifest.json", prerequisite["manifest_sha256"])

    def job_directory(self, original):
        prefix = self.config["run"].rstrip("/") + "/"
        require(str(original).startswith(prefix), "任务目录不属于生产RUN")
        return self.member("native/" + str(original)[len(prefix):])

    def verify_receipts(self):
        complete_path = self.evidence / "native/queue-complete.json"
        self.checked(complete_path, self.manifest["queue_complete_sha256"])
        complete = read_json(complete_path)
        require(complete.get("status") == "passed" and complete["config_sha256"] == self.expected_config_sha, "生产队列完成身份不符")
        self.checked(self.evidence / "native/queue-results-manifest.json", complete["manifest_sha256"])
        self.checked(self.evidence / "native/native-summary.json", complete["summary_sha256"])
        self.jobs = read_json(self.evidence / "native/queue-results-manifest.json")
        expected = {f"build-r{r}" for r in range(3)} | {f"r{r}-ef{ef}-t{t}" for r in range(3) for ef, t in POSITIONS}
        require(set(self.jobs) == expected, "未覆盖3次构建与15个固定查询位置")
        for key, binding in self.jobs.items():
            directory = self.job_directory(binding["directory"])
            self.checked(directory / "receipt.json", binding["receipt_sha256"])
            receipt = read_json(directory / "receipt.json")
            require(receipt == binding["receipt"] and receipt.get("status") == "passed" and receipt["job_id"] == key, "任务回执不符")
            require(receipt["config_sha256"] == self.expected_config_sha and receipt["binary_sha256"] == self.config["binary_sha256"]
                    and receipt["source_identity"] == self.config["source_identity"] and receipt["inputs"] == self.config["inputs"], "原始任务身份链不同")
            for name, identity in receipt["files"].items():
                path = directory.joinpath(*PurePosixPath(name).parts)
                require(path.resolve().is_relative_to(directory), "任务文件路径越界")
                self.checked(path, identity["sha256"])
                require(path.stat().st_size == identity["bytes"], "任务文件长度不符")
        self.repair = read_json(self.evidence / "native/repair-admission-v2.json")
        require(self.repair.get("status") == "passed" and self.repair["config_sha256"] == self.expected_config_sha
                and self.repair["original_source_sha256"] == self.config["source_sha256"]["native_stability.py"], "独立监督修复未绑定原测量身份")
        self.checked(self.evidence / "source/native_stability_v2.py", self.repair["wrapper_sha256"])
        gate_path = self.evidence / "context/functional-state-writer-v2/admission.json"
        self.checked(gate_path, self.repair["writer_gate_sha256"])
        require(read_json(gate_path) == self.repair["writer_gate"] == dict(status="passed", writers=4, writes=1000,
                reads=1000, temporary_files_remaining=0), "独立监督修复核心回归不完整")
        require(any(name.startswith("native/") and "incident" in name and sha == self.repair["incident_sha256"]
                    for name, sha in self.hashes.items()), "写入竞争的原事故证据未归档")
        require(set(self.repair["completed_receipts"]) == {"build-r0", "r0-ef88-t1", "r0-ef88-t8", "r0-ef88-t16"}, "修复前完整检查点集合不同")
        for key, old in self.repair["completed_receipts"].items():
            require(self.jobs[key]["receipt_sha256"] == old["sha256"], "修复前完成结果被覆盖或重测")
            self.resolve(old["path"], old["sha256"])

    def labels(self, value):
        labels = np.asarray(value["labels"])
        bits = np.asarray(value["score_bits"])
        require(labels.shape == bits.shape == (744, 10) and labels.dtype.kind in "iu" and bits.dtype.kind in "iu", "原始标签或分数位形状错误")
        require((labels >= 0).all() and (labels < 1000000).all() and (bits >= 0).all() and (bits <= 0xffffffff).all(), "原始标签或分数位越界")
        require(all(len(set(row)) == 10 for row in labels) and np.isfinite(bits.astype("<u4").view("<f4")).all(), "重复标签或非有限分数")
        recall_by_query = (labels[:, :, None] == self.truth[:, None, :]).any(axis=2).mean(axis=1)
        return labels, bits, recall_by_query

    def response(self, directory, name, ef, threads, kind, seconds, reference):
        request = read_json(directory / (name + ".request.json"))
        expected = dict(qfile=self.config["data"] + "/queries.f32", threads=threads, ef=ef, budget=64, mode="l2", kind=kind, seconds=seconds)
        require(request == expected, "原始查询请求改变：" + name)
        raw = read_json(directory / (name + ".response.jsonl"))
        supervised = read_json(directory / (name + ".json"))
        require(raw["request"] == request and all(supervised.get(k) == v for k, v in raw.items()), "原始响应与监督JSON不同")
        labels, bits, recall = self.labels(raw)
        require(np.array_equal(labels, reference[0]) and np.array_equal(bits, reference[1]), "同图各轮标签或精确分数位不同")
        for field in ("supervised_interval_ns", "supervised_monotonic_interval_ns"):
            interval = supervised[field]
            require(len(interval) == 2 and all(isinstance(v, int) for v in interval), "监督时钟区间非法")
            if field == "supervised_monotonic_interval_ns":
                require(interval[1] >= interval[0], "监督单调时钟发生倒退")
        cpus = list(range(64, 64 + threads))
        require(supervised["affinity"] and all(value == cpus for value in supervised["affinity"].values()), "实际查询线程CPU集合不符")
        memory = supervised["process_family_memory"]
        calculated = sum(p["rss_bytes"] + p["hugetlb_bytes"] for p in memory["processes"])
        require(calculated == memory["rss_plus_hugetlb_bytes"] and calculated <= 56 * GIB, "原始RSS加hugetlb分项或保护线不符")
        if kind == "throughput":
            executed, elapsed = raw["queries_executed"], raw["seconds"]
            require(isinstance(executed, int) and executed >= 744 and executed % 744 == 0
                    and math.isfinite(elapsed) and elapsed >= seconds, "完整批次数或原生吞吐时钟非法")
            measured = (supervised["supervised_monotonic_interval_ns"][1] - supervised["supervised_monotonic_interval_ns"][0]) / 1e9
            require(elapsed <= measured + .002, "原生吞吐时钟超出实际单调时钟区间")
        self.raw_requests += 1
        return raw, supervised, recall

    def analyze_build(self, repeat):
        binding = self.jobs[f"build-r{repeat}"]
        directory = self.job_directory(binding["directory"])
        result = read_json(directory / "result.json")
        config = read_json(directory / "config.json")
        require(result["count"] == 1000000 and result["config"] == config and (config["n"], config["dim"], config["threads"], config["route"], config["encoding"], config["prune"])
                == (1000000, 768, 8, "l2", "sm", "lazy"), "构建参数不符")
        with (directory / "index.bin").open("rb") as handle:
            magic, version, dim, n, m, degree, efc, alpha, entry, level, dirty, reserved, chunks = struct.unpack("<4s6If5I", handle.read(52))
        require((magic, version, dim, n, m, degree, efc, chunks) == (b"QUIV", 1, 768, 1000000, 32, 64, 128, 12)
                and math.isclose(alpha, 1.2, abs_tol=1e-6) and entry < n and level <= n, "原生产图头部不符")
        _, _, recall = self.labels(result)
        audit = read_json(directory / "independent-audit.json")
        graph_sha = binding["receipt"]["files"]["index.bin"]["sha256"]
        require(audit["index_sha256"] == graph_sha, "构建审计图SHA不同")
        same_number(audit["recall_at_10"], float(recall.mean()), "构建诊断召回复算不同")
        supervision = read_json(directory / "supervision.json")
        require(supervision["returncode"] == 0 and supervision["monitor_error"] is None
                and supervision["peak_rss_plus_hugetlb_bytes"] <= 56 * GIB, "构建监督记录未成功")
        require(all(supervision["memory_events_after"].get(k, 0) == supervision["memory_events_before"].get(k, 0)
                    for k in ("oom", "oom_kill", "oom_group_kill")), "构建期间新增OOM事件")
        row = dict(measurement_system="TDB生产", kind="build", repeat=repeat, graph_sha256=graph_sha,
                   build_diagnostic_ef128_recall=float(recall.mean()),
                   peak_rss_plus_hugetlb_bytes=supervision["peak_rss_plus_hugetlb_bytes"], sessions=[])
        for name in ("input_load_s", "ingest_s", "build_s", "save_s", "install_s"):
            require(math.isfinite(result[name]) and result[name] >= 0, "构建时间非法")
            row[name] = result[name]
        self.builds.append(row)

    def audit_resource_record(self, directory, repaired_session=False):
        supervision = read_json(directory / "supervision.json")
        peak = 0
        path = directory / "resource-samples.jsonl"
        for line in path.read_text(encoding="utf-8").splitlines() if path.exists() else []:
            memory = json.loads(line)["memory"]
            total = sum(p["rss_bytes"] + p["hugetlb_bytes"] for p in memory["processes"])
            require(total == memory["rss_plus_hugetlb_bytes"] and total <= 56 * GIB, "监控原始RSS与hugetlb分项或保护线不符")
            peak = max(peak, total)
        require(peak == supervision["peak_rss_plus_hugetlb_bytes"], "监控原始峰值与监督汇总不同")
        require(all(supervision["memory_events_after"].get(k, 0) == supervision["memory_events_before"].get(k, 0)
                    for k in ("oom", "oom_kill", "oom_group_kill")), "监控期间存在新增OOM事件")
        require(repaired_session or (supervision["returncode"] == 0 and supervision["monitor_error"] is None), "非已绑定历史修复的监督记录异常")
        return dict(directory=str(directory.relative_to(self.evidence)), peak_rss_plus_hugetlb_bytes=peak,
                    returncode=supervision["returncode"], monitor_error=supervision["monitor_error"],
                    historical_writer_incident_bound=repaired_session)

    def analyze_point(self, repeat, ef, threads):
        binding = self.jobs[f"r{repeat}-ef{ef}-t{threads}"]
        directory = self.job_directory(binding["directory"])
        receipt = binding["receipt"]
        parent = self.jobs[f"build-r{repeat}"]
        require(receipt["parent_build"] == dict(path=parent["directory"] + "/receipt.json", sha256=parent["receipt_sha256"],
                index_sha256=parent["receipt"]["files"]["index.bin"]["sha256"]), "查询父图身份链错误")
        result = read_json(directory / "result.json")
        gate = read_json(directory / "gate.response.jsonl")
        labels, bits, recalls = self.labels(gate)
        self.response(directory, "gate", ef, threads, "diagnostic", 0., (labels, bits))
        require(np.array_equal(np.load(directory / "labels.npy", allow_pickle=False), labels)
                and np.array_equal(np.load(directory / "score_bits.npy", allow_pickle=False), bits), "导出原始数组与原生JSONL不同")
        if threads != 1:
            reference = self.job_directory(self.jobs[f"r{repeat}-ef{ef}-t1"]["directory"])
            require(np.array_equal(labels, np.load(reference / "labels.npy", allow_pickle=False))
                    and np.array_equal(bits, np.load(reference / "score_bits.npy", allow_pickle=False)), "同图线程标签不一致")
        latency = np.load(directory / "latency_ns.npy", allow_pickle=False)
        require(latency.shape == (5, 744) and latency.dtype.kind in "iu" and (latency > 0).all(), "五轮原生延迟数组非法")
        qps = []
        peak = 0
        for r in range(5):
            warm_seconds = 0.
            for warm in range(self.config["warmup_batches"]):
                raw, _, _ = self.response(directory, f"warmup-r{r}-b{warm}", ef, threads, "throughput",
                                          self.config["warmup_seconds"] / self.config["warmup_batches"], (labels, bits))
                warm_seconds += raw["seconds"]
            require(warm_seconds >= 1, "预热累计不足一秒")
            timed, wrapper, _ = self.response(directory, f"throughput-r{r}", ef, threads, "throughput", float(self.config["throughput_seconds"]), (labels, bits))
            sample = timed["queries_executed"] / timed["seconds"]
            qps.append(sample)
            stored = result["throughput"][r]
            require(stored["queries_executed"] == timed["queries_executed"] and stored["request_interval"] == wrapper["supervised_interval_ns"]
                    and stored["process_family_memory"] == wrapper["process_family_memory"], "结果表吞吐计数、时钟或内存与原始证据不同")
            same_number(stored["seconds"], timed["seconds"], "吞吐时钟不同")
            same_number(stored["qps"], sample, "吞吐QPS不同")
            same_number(result["qps_samples"][r], sample, "五轮QPS样本不同")
            peak = max(peak, wrapper["process_family_memory"]["rss_plus_hugetlb_bytes"])
            tail, _, _ = self.response(directory, f"latency-r{r}", ef, threads, "latency", 0., (labels, bits))
            require(np.array_equal(np.asarray(tail["latency_ns"]), latency[r]), "原始JSONL与五轮逐查询尾延迟不同")
        recall = float(recalls.mean())
        same_number(result["recall_at_10"], recall, "结果表召回与标签复算不同")
        same_number(result["qps_mean"], statistics.mean(qps), "五轮QPS均值不同")
        same_number(result["qps_round_sample_sd"], statistics.stdev(qps), "五轮QPS标准差不同")
        tails = np.quantile(latency, [.95, .99]) / 1e6
        same_number(result["p95_ms"], tails[0], "P95复算不同")
        same_number(result["p99_ms"], tails[1], "P99复算不同")
        ready_path = self.resolve(result["service_ready_path"], result["service_ready_sha256"])
        ready = read_json(ready_path)
        require(read_json(ready_path.parent / "ready.response.jsonl") == ready and ready.get("ready") is True
                and ready["vector_mmap_bytes"] == 3072000000, "服务加载原始响应或映射大小不同")
        expected_index = parent["directory"] + "/index.bin"
        require(ready["config"]["index"] == expected_index and ready["config"]["route"] == "l2", "加载服务属于不同父图或评分路径")
        session = dict(path=str(ready_path.relative_to(self.evidence)), index_load_s=ready["index_load_s"], install_s=ready["install_s"])
        if session not in self.builds[repeat]["sessions"]:
            self.builds[repeat]["sessions"].append(session)
        components = ready["routed_memory"]
        require(components["dimension"] == 768 and components["native_squared_l2"] is False
                and components["logical_rerank_fp32_bytes"] == 3072000000, "加载服务的逻辑组件规模或评分路径不同")
        static_bytes = sum(components[key] for key in ("sm_owned_bytes", "shared_graph_bytes", "raw_bq_signature_bytes",
                                                       "external_ids_bytes", "slot_mapping_bytes", "exact_norm_scales_bytes"))
        ci = np.asarray(self.bootstrap.percentile_interval(latency), dtype=np.float64) / 1e6
        row = dict(measurement_system="TDB生产", kind="query", repeat=repeat, ef=ef, threads=threads,
                   graph_sha256=parent["receipt"]["files"]["index.bin"]["sha256"], recall_at_10=recall,
                   recall_query_cluster_ci95=self.bootstrap.mean_interval(recalls), qps_samples=qps,
                   qps_mean=statistics.mean(qps), qps_round_sample_sd=statistics.stdev(qps),
                   qps_round_cv=statistics.stdev(qps) / statistics.mean(qps), p95_ms=float(tails[0]), p99_ms=float(tails[1]),
                   p95_query_cluster_ci95_ms=ci[0].tolist(), p99_query_cluster_ci95_ms=ci[1].tolist(),
                   peak_sampled_rss_plus_hugetlb_bytes=peak, static_component_subtotal_bytes=static_bytes,
                   visited_bytes=components["visited_bytes_per_thread"] * threads, logical_fp32_bytes=components["logical_rerank_fp32_bytes"],
                   routed_memory=components, service_ready=session["path"], receipt_sha256=binding["receipt_sha256"])
        self.points.append(row)

    def execute(self):
        self.verify_export()
        self.verify_receipts()
        for repeat in range(3):
            self.analyze_build(repeat)
            for ef, threads in POSITIONS:
                self.analyze_point(repeat, ef, threads)
            self.builds[repeat]["first_service_index_load_s"] = self.builds[repeat]["sessions"][0]["index_load_s"]
            self.builds[repeat]["first_service_install_s"] = self.builds[repeat]["sessions"][0]["install_s"]
        across = []
        for ef, threads in POSITIONS:
            rows = [row for row in self.points if row["ef"] == ef and row["threads"] == threads]
            across.append(dict(measurement_system="TDB生产", kind="across_builds", ef=ef, threads=threads,
                               qps=describe(row["qps_mean"] for row in rows), recall=describe(row["recall_at_10"] for row in rows)))
        build_statistics = {name: describe(row[name] for row in self.builds)
                            for name in ("build_s", "install_s", "save_s", "input_load_s", "ingest_s",
                                         "first_service_index_load_s", "first_service_install_s")}
        original = read_json(self.evidence / "native/native-summary.json")
        require(original.get("status") == "passed" and original["config_sha256"] == self.expected_config_sha
                and len(original["builds"]) == 3 and len(original["points"]) == 5, "生产汇总覆盖与独立结果不同")
        for row, stored in zip(across, original["points"]):
            require((row["ef"], row["threads"]) == (stored["ef"], stored["threads"]), "生产汇总位置顺序不同")
            same_number(stored["qps_graph_mean"], row["qps"]["mean"], "生产跨构建QPS均值不同")
            same_number(stored["qps_across_graph_sample_sd"], row["qps"]["sample_sd"], "生产跨构建QPS标准差不同")
            same_number(stored["recall_mean"], row["recall"]["mean"], "生产跨构建召回均值不同")
            same_number(stored["recall_sample_sd"], row["recall"]["sample_sd"], "生产跨构建召回标准差不同")
        unique_index_count = len({row["graph_sha256"] for row in self.builds})
        require(original["unique_graph_count"] == unique_index_count, "原汇总的索引文件SHA计数不同")
        repaired_directories = set()
        for key in self.repair["completed_receipts"]:
            if key.startswith("build-"):
                continue
            result = read_json(self.job_directory(self.jobs[key]["directory"]) / "result.json")
            repaired_directories.add(self.resolve(result["service_ready_path"], result["service_ready_sha256"]).parent)
        resource_audits = []
        for repeat in range(3):
            directory = self.job_directory(self.jobs[f"build-r{repeat}"]["directory"])
            resource_audits.append(self.audit_resource_record(directory))
            for session in self.builds[repeat]["sessions"]:
                directory = (self.evidence / session["path"]).parent
                resource_audits.append(self.audit_resource_record(directory, directory in repaired_directories))
        for name, original in self.stamps.items():
            value = self.member(name).stat()
            require((value.st_size, value.st_mtime_ns, value.st_ctime_ns) == original, "分析期间原始归档证据变化")
        return dict(status="passed", measurement_system="TDB生产", controlled_component_engine=False,
                    config_sha256=self.expected_config_sha, archive_verified=bool(self.archive), n=1000000, dimension=768,
                    queries=744, build_replicates=3, query_positions=15, rounds_per_position=5,
                    unique_index_sha_count=unique_index_count, resource_audits=resource_audits,
                    raw_request_response_pairs_checked=self.raw_requests, bootstrap=dict(seed=20261108, repetitions=4000, unit="原查询簇"),
                    supervision_repair=self.repair,
                    builds=self.builds, points=self.points, across_builds=across, build_statistics=build_statistics,
                    sample_definition="每次独立构建的五轮QPS均值形成一个构建级观测，跨构建样本数为3；查询五轮作为同一查询簇处理")


def save(summary, output):
    output.mkdir(parents=True, exist_ok=True)
    require(not any(output.iterdir()), "输出目录已有文件，请使用独立目录以保留旧分析结果")
    (output / "native-summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    csv_rows = [*summary["builds"], *summary["points"]]
    for name, values in summary["build_statistics"].items():
        csv_rows.append(dict(measurement_system="TDB生产", kind="build_time_across_builds", metric=name,
                             build_samples=3, mean_seconds=values["mean"], sample_sd_seconds=values["sample_sd"], cv=values["cv"]))
    for row in summary["across_builds"]:
        csv_rows.append(dict(measurement_system="TDB生产", kind=row["kind"], ef=row["ef"], threads=row["threads"],
                             build_samples=3, qps_mean=row["qps"]["mean"], qps_across_build_sample_sd=row["qps"]["sample_sd"],
                             qps_across_build_cv=row["qps"]["cv"], recall_at_10=row["recall"]["mean"],
                             recall_across_build_sample_sd=row["recall"]["sample_sd"], recall_across_build_cv=row["recall"]["cv"]))
    write_csv(output / "native-summary.csv", csv_rows, ["measurement_system", "kind", "repeat", "ef", "threads"])
    lines = ["# 原TDB生产稳定性复算", "", "三次独立一百万向量构建，每图五个固定查询位置，每位置五轮完整744查询。",
             "受控组件引擎数值位于独立组件汇总，本表仅列真实TDB生产路径。", "",
             f"不同索引文件SHA数为{summary['unique_index_sha_count']}。", "",
             "## 构建", "", "| 构建 | 索引文件SHA前12位 | 构图秒 | SM安装秒 | 保存秒 |", "|---|---|---:|---:|---:|"]
    for row in summary["builds"]:
        lines.append(f"| {row['repeat']} | {row['graph_sha256'][:12]} | {row['build_s']:.3f} | {row['install_s']:.3f} | {row['save_s']:.3f} |")
    names = dict(build_s="构图", install_s="构建态SM安装", save_s="保存", input_load_s="输入读取", ingest_s="导入",
                 first_service_index_load_s="首次服务图加载", first_service_install_s="首次服务SM安装")
    lines.extend(["", "| 时间项目 | 三次均值秒 | 样本SD秒 | CV |", "|---|---:|---:|---:|"])
    for name, values in summary["build_statistics"].items():
        lines.append(f"| {names[name]} | {values['mean']:.3f} | {values['sample_sd']:.3f} | {values['cv']:.2%} |")
    lines.extend(["", "## 各构建查询结果", "", "| 构建 | ef | 线程 | Recall@10 | QPS均值 | 轮间CV | P95毫秒 | P99毫秒 |",
                  "|---|---:|---:|---:|---:|---:|---:|---:|"])
    for row in summary["points"]:
        lines.append(f"| {row['repeat']} | {row['ef']} | {row['threads']} | {row['recall_at_10']:.4%} | {row['qps_mean']:.1f} | {row['qps_round_cv']:.2%} | {row['p95_ms']:.4f} | {row['p99_ms']:.4f} |")
    lines.extend(["", "## 跨构建汇总", "", "| ef | 线程 | QPS均值 | 构建级SD | 构建级CV | Recall均值 | Recall SD |",
                  "|---|---:|---:|---:|---:|---:|---:|"])
    for row in summary["across_builds"]:
        lines.append(f"| {row['ef']} | {row['threads']} | {row['qps']['mean']:.1f} | {row['qps']['sample_sd']:.1f} | {row['qps']['cv']:.2%} | {row['recall']['mean']:.4%} | {row['recall']['sample_sd']:.6f} |")
    lines.extend(["", summary["sample_definition"], "尾延迟查询簇95%区间与完整分项保存在JSON和CSV中，bootstrap 4000次，种子20261108。", ""])
    (output / "native-summary.md").write_text("\n".join(lines), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--config-sha256", default=CONFIG_SHA)
    args = parser.parse_args()
    summary = Analysis(args.evidence, args.config_sha256, args.archive).execute()
    save(summary, args.out)
    print("生产原始证据独立核验完成，3次构建与15个查询位置已复算。", flush=True)


if __name__ == "__main__":
    main()
