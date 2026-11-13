"""离线复算本轮受控组件实验，保留完整身份、缺项和统计口径。"""

import argparse
import csv
import hashlib
import json
import math
import sys
from collections import Counter
from pathlib import Path, PurePosixPath

import numpy as np


SEED = 20261108
REPLICATES = 4000
T95_DF4 = 2.7764451051977987
PADDING = np.uint32(4294967295)


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def digest(path):
    hasher = hashlib.sha256()
    with Path(path).open("rb") as handle:
        while block := handle.read(8 * 1024 * 1024):
            hasher.update(block)
    return hasher.hexdigest()


def value_digest(value):
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def checked_array(path, dtype, shape):
    size = int(np.prod(shape)) * np.dtype(dtype).itemsize
    require(Path(path).stat().st_size == size, "数组长度不匹配：" + str(path))
    return np.fromfile(path, dtype=dtype).reshape(shape)


def write_csv(path, rows, fallback_fields):
    fields = list(dict.fromkeys([*fallback_fields, *(key for row in rows for key in row)]))
    with Path(path).open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({key: json.dumps(value, ensure_ascii=False) if isinstance(value, (dict, list)) else value
                             for key, value in row.items()})


class QueryBootstrap:
    """对查询或源块重采样，重复轮次保留在同一查询簇内。"""

    def __init__(self, groups, unit):
        self.groups, self.inverse = np.unique(groups, return_inverse=True)
        self.unit = unit
        self.counts = np.bincount(self.inverse).astype(np.float64)
        rng = np.random.default_rng(SEED)
        selected = rng.integers(0, len(self.groups), size=(REPLICATES, len(self.groups)))
        self.weights = np.zeros((REPLICATES, len(self.groups)), dtype=np.int32)
        for row, indices in enumerate(selected):
            self.weights[row] = np.bincount(indices, minlength=len(self.groups))
        self.denominators = self.weights @ self.counts

    def mean_interval(self, values):
        totals = np.bincount(self.inverse, weights=values, minlength=len(self.groups))
        samples = (self.weights @ totals) / self.denominators
        return np.quantile(samples, [0.025, 0.975]).tolist()

    def percentile_interval(self, rounds_by_query, probabilities=(0.95, 0.99)):
        # 将簇重复次数视为每个原始延迟样本的整数权重，避免展开巨大的重采样矩阵。
        values = rounds_by_query.reshape(-1)
        point_groups = np.tile(self.inverse, rounds_by_query.shape[0])
        order = np.argsort(values, kind="stable")
        sorted_values = values[order]
        sorted_groups = point_groups[order]
        estimates = np.empty((REPLICATES, len(probabilities)), dtype=np.float64)
        for start in range(0, REPLICATES, 64):
            stop = min(start + 64, REPLICATES)
            cumulative = np.cumsum(self.weights[start:stop, sorted_groups], axis=1, dtype=np.int64)
            total = cumulative[:, -1]
            for column, probability in enumerate(probabilities):
                position = probability * (total - 1)
                lower = np.floor(position).astype(np.int64)
                upper = np.ceil(position).astype(np.int64)
                lo_index = np.argmax(cumulative > lower[:, None], axis=1)
                hi_index = np.argmax(cumulative > upper[:, None], axis=1)
                fraction = position - lower
                estimates[start:stop, column] = sorted_values[lo_index] * (1-fraction) + sorted_values[hi_index] * fraction
        return [np.quantile(estimates[:, index], [0.025, 0.975]).tolist()
                for index in range(len(probabilities))]


class Analysis:
    def __init__(self, plan_path, run, output):
        self.plan_path = plan_path.resolve()
        self.run = run.resolve()
        self.output = output.resolve()
        self.plan = read_json(self.plan_path)
        self.plan_sha = digest(self.plan_path)
        self.jobs = {job["id"]: job for job in self.plan["jobs"]}
        self.inputs = {entry["id"]: entry for entry in self.plan["inputs"]}
        self.records = []
        self.verified = {}
        self.query_scores = {}
        self.bootstrap = {}
        self.status = read_json(self.run / "queue-state.json") if (self.run / "queue-state.json").exists() else {}
        self.alerts = [read_json(self.run / name) for name in ("alert.json", "controller-alert.json")
                       if (self.run / name).exists()]
        self.ordinary_errors = read_json(self.run / "ordinary-errors.json") if (self.run / "ordinary-errors.json").exists() else []
        self.manifest = read_json(self.run / "queue-results-manifest.json") if (self.run / "queue-results-manifest.json").exists() else {}
        completion = self.run / "queue-complete.json"
        if completion.exists():
            receipt = read_json(completion)
            require(receipt["plan_sha256"] == self.plan_sha, "队列完成回执与计划不一致")
            require(receipt["manifest_sha256"] == digest(self.run / "queue-results-manifest.json"), "队列结果清单身份不一致")
        # 完整检查点暂停时可能尚未生成总清单，逐任务回执仍可独立验证。
        for job in self.plan["jobs"]:
            if job["id"] in self.manifest:
                continue
            receipts = list((self.run / "jobs" / job["id"]).glob("attempt-*/receipt.json"))
            if len(receipts) == 1:
                self.manifest[job["id"]] = dict(receipt_path=str(receipts[0]), receipt_sha256=digest(receipts[0]),
                                                binding_origin="本地完整检查点回执")

    def mapped_paths(self, value):
        text = str(value).replace("\\", "/")
        candidates = [Path(value), self.run / text, self.plan_path.parent / text]
        for key in ("run", "run_directory", "remote_run"):
            prefix = str(self.plan.get(key, "")).replace("\\", "/").rstrip("/")
            if prefix and text.startswith(prefix + "/"):
                candidates.append(self.run / text[len(prefix)+1:])
        if "/jobs/" in text:
            candidates.append(self.run / "jobs" / text.split("/jobs/", 1)[1])
        return list(dict.fromkeys(candidates))

    def resolve_hashed(self, value, expected):
        for path in self.mapped_paths(value):
            if path.is_file() and digest(path) == expected:
                return path
        raise ValueError("找不到身份匹配的归档文件：" + str(value))

    def input_file(self, dataset, name, identity, required=True):
        expected = identity["files"].get(name)
        if expected is None:
            require(not required, "输入回执未绑定文件：" + dataset + "/" + name)
            return None
        candidates = [self.run / "inputs" / dataset / name, self.run / "data" / dataset / name,
                      self.plan_path.parent / "data" / dataset / name, self.run.parent / "data" / dataset / name]
        source = PurePosixPath(self.inputs[dataset]["path"]) / name
        candidates.extend(self.mapped_paths(str(source)))
        for path in dict.fromkeys(candidates):
            if path.is_file() and path.stat().st_size == expected["bytes"] and digest(path) == expected["sha256"]:
                return path
        require(not required, "缺少身份匹配的冻结输入：" + dataset + "/" + name)
        return None

    def metadata(self, job):
        graph_job = self.jobs.get(job.get("graph_job", ""), job if job["kind"] == "build" else {})
        entry = self.inputs[job["dataset"]]
        return dict(job_id=job["id"], kind=job["kind"], dataset=job["dataset"],
                    coverage_status="pending", n=entry["n"], dim=entry["dim"],
                    graph_job=job.get("graph_job", job["id"] if job["kind"] == "build" else ""),
                    graph_encoding=graph_job.get("encoding"), discovery=graph_job.get("discovery"),
                    prune=graph_job.get("prune"), direction=graph_job.get("direction"),
                    alpha=graph_job.get("alpha"), seed=graph_job.get("seed"),
                    query_encoding=job.get("encoding") if job["kind"] == "query" else None,
                    score=job.get("score"), ef=job.get("ef"), threads=job.get("threads", 8),
                    plan_sha256=self.plan_sha, engine_source_sha256=self.plan["source_sha256"]["engine.cpp"],
                    binary_sha256=self.plan["binary_sha256"],
                    measurement_scope="共同受控C++引擎组件实验")

    def verify_receipt(self, job, row):
        binding = self.manifest[job["id"]]
        receipt_path = self.resolve_hashed(binding["receipt_path"], binding["receipt_sha256"])
        receipt = read_json(receipt_path)
        require(receipt["status"] == "passed" and receipt["job_sha256"] == value_digest(job), "任务回执状态或配置不一致")
        require(receipt["binary_sha256"] == self.plan["binary_sha256"], "任务二进制不一致")
        require(receipt["engine_source_sha256"] == self.plan["source_sha256"]["engine.cpp"], "任务源码不一致")
        if receipt["plan_sha256"] != self.plan_sha:
            adoption = self.plan.get("adopted_receipts", {}).get(job["id"], {})
            require(adoption.get("sha256") == binding["receipt_sha256"], "旧计划回执未被显式采纳")
        directory = receipt_path.parent
        for name, expected in receipt["files"].items():
            path = directory / name
            require(path.resolve().is_relative_to(directory.resolve()), "回执文件路径越出现场目录")
            require(path.stat().st_size == expected["bytes"] and digest(path) == expected["sha256"], "原始文件身份变化：" + str(path))
        identity = receipt["input_identity"]
        expected_input = self.inputs[job["dataset"]]
        require(identity["n"] == expected_input["n"] and identity["dim"] == expected_input["dim"], "输入规模或维度不匹配")
        for item in expected_input["files"]:
            require(identity["files"].get(item["file"]) == dict(bytes=item["bytes"], sha256=item["sha256"]), "冻结输入身份不匹配")
        parent = receipt.get("parent_graph")
        if parent:
            require(job["graph_job"] in self.verified, "父图原始证据尚未验证")
            parent_record = self.verified[job["graph_job"]]
            require(parent["sha256"] == parent_record["graph_sha256"], "父图身份不匹配")
            require(identity["files"]["base.f32"] == parent_record["input_identity"]["files"]["base.f32"], "查询与父图基础向量不同")
        row.update(receipt_sha256=binding["receipt_sha256"], receipt_path=str(receipt_path),
                   receipt_binding_origin=binding.get("binding_origin", "冻结队列结果清单"),
                   original_plan_sha256=receipt["plan_sha256"], input_identity_sha256=value_digest(identity),
                   input_base_sha256=identity["files"]["base.f32"]["sha256"],
                   input_queries_sha256=identity["files"].get("queries.f32", {}).get("sha256"),
                   input_truth_sha256=identity["files"].get("truth32.u32", {}).get("sha256"),
                   input_identity=identity, parent_graph_sha256=parent["sha256"] if parent else None)
        return directory, receipt

    def analyze_build(self, job, row, directory, receipt):
        result = read_json(directory / "build.json")
        require(result["status"] == "passed", "构建原始状态未通过")
        for key in ("encoding", "discovery", "prune", "direction", "seed"):
            require(result[key] == job[key], "构建参数不一致：" + key)
        require(math.isclose(result["alpha"], job["alpha"], rel_tol=0, abs_tol=1e-6), "构建alpha不一致")
        raw = np.fromfile(directory / "graph.u32", dtype="<u4")
        require(len(raw) > 0 and int(raw[0]) == row["n"], "图节点数不匹配")
        n = row["n"]
        if len(raw) == 1 + n * 65 and np.all(raw[1:].reshape(n, 65)[:, 0] == 64):
            rows = raw[1:].reshape(n, 65)[:, 1:]
            require(np.all(rows < n), "图编号越界")
            require(not np.any(rows == np.arange(n)[:, None]), "图存在自环")
            require(not np.any(np.diff(np.sort(rows, axis=1), axis=1) == 0), "图存在重复边")
            edges, minimum, maximum = n * 64, 64, 64
        else:
            position, degrees = 1, []
            for source in range(n):
                require(position < len(raw), "图行缺失")
                degree = int(raw[position]); position += 1
                ids = raw[position:position+degree]; position += degree
                require(degree <= 64 and len(ids) == degree, "图出度或长度不合法")
                require(np.all(ids < n) and source not in ids and len(np.unique(ids)) == degree, "图邻接编号不合法")
                degrees.append(degree)
            require(position == len(raw), "图存在未解释尾部")
            edges, minimum, maximum = sum(degrees), min(degrees), max(degrees)
        origins = checked_array(directory / "edge-origin.u8", "u1", (edges,))
        stages = checked_array(directory / "edge-stage.u8", "u1", (edges,))
        require(np.all(origins < 3) and np.all(stages < 3), "边来源或阶段标签不合法")
        origin_counts = np.bincount(origins, minlength=3)
        stage_counts = np.bincount(stages, minlength=3)
        historical = checked_array(directory / "selector-counts.u64", "<u8", (3, 4))
        origin_names = ("diversity", "backfill", "unpruned_reverse")
        stage_names = ("forward", "reverse", "final")
        require(result["edges"] == edges, "边数汇总不一致")
        for name, count in zip(origin_names, origin_counts):
            require(result["final_edge_origins"][name] == int(count), "最终边来源汇总不一致")
            row["final_edge_"+name] = int(count)
            row["final_edge_"+name+"_percent"] = 100*int(count)/edges if edges else None
        history = {}
        for index, name in enumerate(stage_names):
            require(result["final_edge_stages"][name] == int(stage_counts[index]), "最终边阶段汇总不一致")
            row["final_stage_"+name+"_edges"] = int(stage_counts[index])
            values = dict(zip(("calls", "candidates", "diversity", "backfill"), map(int, historical[index])))
            selected = values["diversity"] + values["backfill"]
            values["selected_outputs"] = selected
            values["backfill_output_percent"] = 100*values["backfill"]/selected if selected else None
            history[name] = values
        for key in ("input_load_seconds", "encoding_seconds", "graph_seconds", "build_seconds", "save_seconds"):
            require(math.isfinite(result[key]) and result[key] >= 0, "构建计时含非法值")
            row[key] = result[key]
        require(result["build_seconds"] + 1e-6 >= result["graph_seconds"] + result["encoding_seconds"], "构建总计时与组成不一致")
        row.update(graph_sha256=receipt["files"]["graph.u32"]["sha256"], graph_edges=edges,
                   graph_minimum_degree=minimum, graph_maximum_degree=maximum,
                   reachable_from_zero_reported=result.get("reachable_from_zero"),
                   selector_history=history, row_stride_bytes=result.get("row_stride_bytes"),
                   encoded_rows_bytes=result.get("row_bytes"), graph_capacity_bytes=result.get("shared_graph_capacity_bytes"),
                   origin_denominator="最终图边数", selector_denominator="各阶段历次选择输出总数，允许同一边多次计入",
                   coverage_status="verified")

    def bootstrap_for(self, dataset, count, identity):
        if dataset in self.bootstrap:
            return self.bootstrap[dataset]
        ids_path = self.input_file(dataset, "query-ids.json", identity)
        ids = read_json(ids_path)
        require(len(ids) == count and len(set(ids)) == count, "冻结查询编号长度或唯一性不符")
        group_path = self.input_file(dataset, "source-query-groups.json", identity, required=False)
        if group_path:
            groups = np.asarray(read_json(group_path))
            require(groups.ndim == 1 and len(groups) == count, "源块查询分组长度不匹配")
            unit = "源查询块"
        else:
            require(dataset != "D03-fresh", "新增查询缺少冻结源块分组")
            groups = np.arange(count)
            unit = "查询"
        bootstrap = QueryBootstrap(groups, unit)
        self.bootstrap[dataset] = bootstrap
        return bootstrap

    def analyze_query(self, job, row, directory, receipt):
        result = read_json(directory / "query.json")
        require(result["status"] == "passed" and result.get("controlled_engine") is True, "查询状态或引擎范围不符")
        require(result.get("production_tdb_qps") is False, "受控结果不得标为生产TDB吞吐")
        count = result["evaluation_queries"]
        require(result["n"] == row["n"] and result["dim"] == row["dim"], "查询规模或维度不一致")
        require(result["threads"] == job["threads"] and result["ef"] == job["ef"], "线程或搜索宽度不一致")
        require(result["actual_threads"] == job["threads"], "实际查询线程数不一致")
        require(result["encoding"] == job["encoding"] and result["score"] == job["score"], "查询编码或评分不一致")
        require(result["rounds"] == 5 and result["latency_rounds"] == 5 and result["latency_unit"] == "seconds", "查询轮数或时钟单位不一致")
        identity = receipt["input_identity"]
        truth_path = self.input_file(job["dataset"], "truth32.u32", identity)
        truth = checked_array(truth_path, "<u4", (count, 32))[:, :10]
        labels = checked_array(directory / "labels.u32", "<u4", (count, 10))
        require(np.all(labels < row["n"]) and np.all(truth < row["n"]), "标签或真值编号越界")
        require(not np.any(np.diff(np.sort(labels, axis=1), axis=1) == 0), "最终标签重复")
        require(not np.any(np.diff(np.sort(truth, axis=1), axis=1) == 0), "真值标签重复")
        for name in ("round-labels.u32", "latency-labels.u32"):
            values = checked_array(directory / name, "<u4", (5, count, 10))
            require(np.all(values == labels[None]), "同图多轮标签不一致：" + name)
        latency = checked_array(directory / "latency.f64", "<f8", (5, count))
        require(np.all(np.isfinite(latency)) and np.all(latency >= 0), "原生延迟包含非法值")
        stats = checked_array(directory / "stats.f64", "<f8", (count, 8))
        require(np.all(np.isfinite(stats)) and np.all(stats[:, 0] == np.arange(count)), "原始工作量统计不合法")
        words = (row["n"]+63)//64
        visited = checked_array(directory / "visited.u64", "<u8", (count, words))
        pool = checked_array(directory / "pool.u32", "<u4", (count, job["ef"]))
        budget = min(64, job["ef"])
        rerank = checked_array(directory / "rerank.u32", "<u4", (count, budget))
        require(result["budget"] == budget, "原始精排预算不一致")
        for values in (pool, rerank):
            require(np.all((values < row["n"]) | (values == PADDING)), "候选集合编号越界")
        row_ids = np.arange(count)[:, None]
        visited_hits = ((visited[row_ids, truth//64] >> (truth.astype(np.uint64) % 64)) & 1).sum(axis=1)
        beam_hits = np.any(pool[:, :, None] == truth[:, None, :], axis=1).sum(axis=1)
        rerank_hits = np.any(rerank[:, :, None] == truth[:, None, :], axis=1).sum(axis=1)
        final_hits = np.any(labels[:, :, None] == truth[:, None, :], axis=1).sum(axis=1)
        for index in range(count):
            p = pool[index][pool[index] != PADDING]
            f = rerank[index][rerank[index] != PADDING]
            require(len(np.unique(p)) == len(p) and len(np.unique(f)) == len(f), "候选或精排编号重复")
            require(set(labels[index]).issubset(f) and set(f).issubset(p), "最终标签与精排、候选池包含关系不成立")
            require(np.all((visited[index, p//64] >> (p.astype(np.uint64) % 64)) & 1), "候选池含未访问节点")
            scored = int(np.bitwise_count(visited[index]).sum()) if hasattr(np, "bitwise_count") else sum(int(v).bit_count() for v in visited[index])
            require(stats[index, 2] == scored and stats[index, 3] == len(f), "访问或精排工作量与原集合不一致")
        observed = np.column_stack((visited_hits, beam_hits, rerank_hits, final_hits))
        require(np.all(stats[:, 4:8] == observed), "阶段覆盖与原集合不一致")
        require(np.all(visited_hits >= beam_hits) and np.all(beam_hits >= rerank_hits), "阶段真近邻覆盖不单调")
        scores = final_hits.astype(np.float64)/10
        recall = float(scores.mean())
        require(math.isclose(recall, result["recall"], rel_tol=0, abs_tol=1e-10), "标签召回与汇总不一致")
        for name, expected in (("mean_expanded", stats[:, 1].mean()), ("mean_scores", stats[:, 2].mean()),
                               ("mean_fp32", stats[:, 3].mean()), ("visited_recall", visited_hits.mean()/10),
                               ("beam_recall", beam_hits.mean()/10), ("rerank_recall", rerank_hits.mean()/10)):
            require(math.isclose(result[name], float(expected), rel_tol=1e-10, abs_tol=1e-10), "查询工作量或覆盖汇总不一致：" + name)
        require(np.all(final_hits == rerank_hits), "精排集合与最终命中存在差异，需要核实精确评分或同距规则")
        bootstrap = self.bootstrap_for(job["dataset"], count, identity)
        recall_ci = bootstrap.mean_interval(scores)
        points = np.quantile(latency, [0.95, 0.99])
        intervals = bootstrap.percentile_interval(latency)
        clocks = read_json(directory / "throughput-rounds.json")
        require(len(clocks) == 5 and len(result["qps_rounds"]) == 5, "五轮原始吞吐计时不完整")
        qps = []
        for index, clock in enumerate(clocks):
            require(clock["loops"] >= 1 and clock["queries"] == clock["loops"] * count and clock["wall_s"] > 0, "轮次请求数或墙钟不一致")
            require(clock["wall_s"] + 1e-6 >= job.get("min_seconds", 1), "轮次未达到冻结计时长度")
            if "warmup_minimum_batches" in result:
                require(clock["warmup_batches"] >= result["warmup_minimum_batches"], "独立轮次预热批次不足")
                require(clock["warmup_queries"] == clock["warmup_batches"] * count, "预热完整查询数不一致")
                require(clock["warmup_wall_s"] + 1e-6 >= result["warmup_minimum_seconds"], "独立轮次预热时间不足")
            actual = clock["queries"]/clock["wall_s"]
            require(math.isclose(actual, clock["qps"], rel_tol=1e-10) and math.isclose(actual, result["qps_rounds"][index], rel_tol=1e-10), "原时钟与QPS汇总不一致")
            qps.append(actual)
        mean = float(np.mean(qps)); sd = float(np.std(qps, ddof=1)); half = T95_DF4 * sd/math.sqrt(5)
        row.update(coverage_status="verified", queries=count, rounds=5,
                   query_cluster_unit=bootstrap.unit, query_clusters=len(bootstrap.groups),
                   recall_percent=100*recall, recall_ci95_low_percent=100*recall_ci[0], recall_ci95_high_percent=100*recall_ci[1],
                   qps_mean=mean, qps_round_sd=sd, qps_round_cv_percent=100*sd/mean,
                   qps_t95_low=max(0, mean-half), qps_t95_high=mean+half, qps_rounds=qps,
                   throughput_round_clocks=clocks,
                   p95_ms=1000*float(points[0]), p95_ci95_low_ms=1000*intervals[0][0], p95_ci95_high_ms=1000*intervals[0][1],
                   p99_ms=1000*float(points[1]), p99_ci95_low_ms=1000*intervals[1][0], p99_ci95_high_ms=1000*intervals[1][1],
                   visited_recall_percent=10*float(visited_hits.mean()), beam_recall_percent=10*float(beam_hits.mean()),
                   rerank_recall_percent=10*float(rerank_hits.mean()),
                   traversal_loss_pp=100-10*float(visited_hits.mean()),
                   beam_retention_loss_pp=10*float((visited_hits-beam_hits).mean()),
                   rerank_selection_loss_pp=10*float((beam_hits-rerank_hits).mean()),
                   mean_expanded=float(stats[:, 1].mean()), mean_scores=float(stats[:, 2].mean()), mean_fp32=float(stats[:, 3].mean()),
                   input_load_seconds=result.get("input_load_seconds"), graph_load_seconds=result.get("graph_load_seconds"),
                   encoding_seconds=result.get("encoding_seconds"), row_stride_bytes=result.get("row_stride_bytes"),
                   encoded_rows_bytes=result.get("encoded_rows_bytes"), graph_capacity_bytes=result.get("graph_capacity_bytes"),
                   raw_fp32_bytes=result.get("raw_fp32_bytes"), visited_bytes_per_thread=result.get("visited_bytes_per_thread"),
                   throughput_boundary=result.get("throughput_boundary"), latency_boundary=result.get("latency_boundary"),
                   raw_labels_sha256=receipt["files"]["labels.u32"]["sha256"],
                   raw_latency_sha256=receipt["files"]["latency.f64"]["sha256"],
                   input_query_ids_sha256=identity["files"]["query-ids.json"]["sha256"],
                   input_query_groups_sha256=identity["files"].get("source-query-groups.json", {}).get("sha256"))
        self.query_scores[job["id"]] = scores

    def missing(self, job, row):
        status = self.status.get("jobs", {}).get(job["id"], "pending")
        if status == "running" and self.alerts:
            incident = self.alerts[-1]
            kind = incident.get("kind", incident.get("error_type", ""))
            status = "resource_stopped" if kind == "ResourceIncident" else "controller_stopped"
            row["failure_kind"] = kind
            row["note"] = incident.get("error", incident.get("message", "监督器停止"))
        elif status == "completed":
            status = "missing_archive"
            row["note"] = "状态为完成但完整结果清单缺少回执，需补收归档"
        elif status == "failed":
            errors = [e for e in self.ordinary_errors if e.get("job") == job["id"]]
            row["note"] = errors[-1].get("error", "保留的普通运行错误") if errors else "任务失败，原始回执缺失"
            row["failure_kind"] = "execution_error"
        else:
            row["note"] = "尚无完整可验证回执"
        row["coverage_status"] = status

    def analyze(self):
        for job in self.plan["jobs"]:
            row = self.metadata(job)
            try:
                if job["id"] not in self.manifest:
                    self.missing(job, row)
                else:
                    directory, receipt = self.verify_receipt(job, row)
                    if job["kind"] == "build":
                        self.analyze_build(job, row, directory, receipt)
                    else:
                        self.analyze_query(job, row, directory, receipt)
                    self.verified[job["id"]] = row
            except Exception as exc:
                row["coverage_status"] = "verification_failed"
                row["failure_kind"] = type(exc).__name__
                row["note"] = str(exc)
            self.records.append(row)
            print("离线核对：" + job["id"] + "，状态" + row["coverage_status"], flush=True)
        paired = self.paired()
        self.output.mkdir(parents=True, exist_ok=True)
        write_csv(self.output / "results.csv", self.records, ["dataset", "job_id", "kind", "coverage_status"])
        write_csv(self.output / "paired.csv", paired, ["dataset", "candidate_job", "reference_job", "coverage_status"])
        (self.output / "汇总.md").write_text(self.report(paired), encoding="utf-8")
        return 2 if any(r["coverage_status"] in ("verification_failed", "missing_archive") for r in self.records) else 0

    def paired(self):
        references = {}
        for job in self.plan["jobs"]:
            if job["kind"] != "query":
                continue
            graph = self.jobs[job["graph_job"]]
            reference = graph["encoding"] == "sm" and graph["discovery"] == "hamming" and graph["prune"] == "dbq" and graph["direction"] == "legacy"
            reference = reference and math.isclose(graph["alpha"], 1.2, abs_tol=1e-6) and job["encoding"] == "sm" and job["score"] == "l2"
            key = (job["dataset"], job["ef"], job["threads"])
            if reference and key not in references:
                references[key] = job["id"]
        output = []
        for job in self.plan["jobs"]:
            if job["kind"] != "query":
                continue
            key = (job["dataset"], job["ef"], job["threads"])
            reference = references.get(key)
            if job["id"] == reference:
                continue
            row = dict(dataset=job["dataset"], candidate_job=job["id"], reference_job=reference,
                       ef=job["ef"], threads=job["threads"], coverage_status="reference_missing",
                       reference_rule="冻结计划中首个生产约定SM/Hamming/DBQ/legacy1.2图及L2查询，不按评估性能挑选")
            if reference in self.query_scores and job["id"] in self.query_scores:
                delta = self.query_scores[job["id"]] - self.query_scores[reference]
                bootstrap = self.bootstrap[job["dataset"]]
                interval = bootstrap.mean_interval(delta)
                candidate, base = self.verified[job["id"]], self.verified[reference]
                row.update(coverage_status="verified", recall_delta_pp=100*float(delta.mean()),
                           paired_ci95_low_pp=100*interval[0], paired_ci95_high_pp=100*interval[1],
                           zero_in_interval=bool(interval[0] <= 0 <= interval[1]),
                           mean_qps_ratio=candidate["qps_mean"]/base["qps_mean"],
                           candidate_graph_sha256=candidate["parent_graph_sha256"],
                           reference_graph_sha256=base["parent_graph_sha256"],
                           input_queries_sha256=candidate["input_queries_sha256"],
                           input_truth_sha256=candidate["input_truth_sha256"],
                           candidate_receipt_sha256=candidate["receipt_sha256"], reference_receipt_sha256=base["receipt_sha256"],
                           engine_source_sha256=candidate["engine_source_sha256"], binary_sha256=candidate["binary_sha256"],
                           query_cluster_unit=bootstrap.unit, query_clusters=len(bootstrap.groups))
            elif reference and job["id"] not in self.query_scores:
                row["coverage_status"] = "candidate_unverified"
            output.append(row)
        return output

    def report(self, paired):
        def number(value, digits=2):
            return "—" if value is None else f"{value:,.{digits}f}"

        lines = ["# QuIVer 评审补充实验离线汇总", "",
                 "**本报告统计共同受控 C++ 引擎的组件实验。其 QPS、构图时间和内存字段对应这一实验对象。** 原生 TDB 主评测与跨图稳定性由独立结果汇总。", "",
                 f"冻结计划 SHA256 为 `{self.plan_sha}`。固定随机种子 {SEED}，bootstrap {REPLICATES:,} 次。", "",
                 "## 覆盖情况", "", "| 数据集 | 类型 | 计划位置 | 已验证 | 其他状态 |", "| --- | --- | --- | --- | --- |"]
        for dataset in self.inputs:
            for kind in ("build", "query"):
                rows = [r for r in self.records if r["dataset"] == dataset and r["kind"] == kind]
                if rows:
                    counts = Counter(r["coverage_status"] for r in rows)
                    others = "，".join(f"{k} {v}" for k, v in counts.items() if k != "verified") or "—"
                    lines.append(f"| {dataset} | {kind} | {len(rows)} | {counts['verified']} | {others} |")
        lines += ["", "资源保护、运行错误、尚未执行和归档不完整分别保留状态，数值留空。它们不作为算法性能负结果。", "",
                  "## 构图与边来源", "", "build_seconds 沿用引擎的编码加构图计时，input_load、encoding、graph 和 save 在 CSV 中分列。最终边来源按最后一次决定保留该边的操作归属，分母为最终边数。历史 selector 各阶段按历次选择输出计数，允许同一边反复出现，其分母另列。", ""]
        for dataset in self.inputs:
            rows = [r for r in self.records if r["dataset"] == dataset and r["kind"] == "build" and r["coverage_status"] == "verified"]
            if not rows:
                continue
            lines += [f"### {dataset}", "", "| 任务 | 编码 | 发现 | 剪枝 | 方向 | α | 编码秒 | 图秒 | 合计秒 | 最终回填 % | 直接反向 % |", "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
            for r in rows:
                lines.append("| " + " | ".join([r["job_id"], r["graph_encoding"], r["discovery"], r["prune"], r["direction"], str(r["alpha"]), number(r["encoding_seconds"]), number(r["graph_seconds"]), number(r["build_seconds"]), number(r["final_edge_backfill_percent"]), number(r["final_edge_unpruned_reverse_percent"])]) + " |")
            lines.append("")
        lines += ["## 查询质量、吞吐与尾延迟", "", "Recall 使用查询簇重采样。旧查询每条为一簇，D03-fresh 按冻结源块分组重采样，并保留同一查询五轮延迟的相关性。P95/P99 来自全部原生服务时钟，单位为毫秒。QPS 是五轮原始请求数除以墙钟后的均值，其 t 区间描述固定图、固定查询下的轮间变化。完整区间与原时钟保留在 CSV。", ""]
        for dataset in self.inputs:
            rows = [r for r in self.records if r["dataset"] == dataset and r["kind"] == "query" and r["coverage_status"] == "verified"]
            if not rows:
                continue
            lines += [f"### {dataset}", "", "| 任务 | 图 | 评分 | ef | 线程 | Recall %及95%区间 | QPS ± SD | P95 ms | P99 ms |", "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
            for r in rows:
                ci = f"{number(r['recall_percent'])} [{number(r['recall_ci95_low_percent'])}, {number(r['recall_ci95_high_percent'])}]"
                qps = f"{number(r['qps_mean'], 0)} ± {number(r['qps_round_sd'], 1)}"
                lines.append("| " + " | ".join([r["job_id"], r["graph_job"], r["score"], str(r["ef"]), str(r["threads"]), ci, qps, number(r["p95_ms"], 3), number(r["p99_ms"], 3)]) + " |")
            lines.append("")
        lines += ["## 预设配对召回差", "", "基准为冻结计划中首个 SM 编码、Hamming 发现、DBQ 剪枝、legacy 方向、α=1.2 的图及 L2 查询。只配对同数据集、同 ef、同线程位置；未按评估结果选择基准或最强候选。QPS 比值仅为点估计。", "",
                  "| 数据集 | 候选任务 | 基准任务 | 召回差 pp | 配对95%区间 pp | QPS比值 |", "| --- | --- | --- | --- | --- | --- |"]
        for r in paired:
            if r["coverage_status"] == "verified":
                lines.append(f"| {r['dataset']} | {r['candidate_job']} | {r['reference_job']} | {number(r['recall_delta_pp'], 3)} | [{number(r['paired_ci95_low_pp'], 3)}, {number(r['paired_ci95_high_pp'], 3)}] | {number(r['mean_qps_ratio'], 3)} |")
        lines += ["", "## 未验证位置与资料状态", ""]
        for incident in self.alerts:
            kind = incident.get("kind", incident.get("error_type", "监督器事件"))
            message = incident.get("error", incident.get("message", "查看保留的原始告警"))
            lines.append(f"- 监督器事件 {kind}。{message}。")
        missing = [r for r in self.records if r["coverage_status"] != "verified"]
        if missing:
            for r in missing:
                lines.append(f"- {r['job_id']}，{r['coverage_status']}。{r.get('note', '等待完整证据')}。")
        else:
            lines.append("全部计划位置的归档、标签、原始时钟及阶段覆盖均已通过本次离线复算。")
        unavailable_pairs = [r for r in paired if r["coverage_status"] != "verified"]
        if unavailable_pairs:
            lines += ["", f"另有 {len(unavailable_pairs)} 个配对位置因基准或候选未验证而保留缺项，逐项见 paired.csv。"]
        lines += ["", "## 文件与口径", "",
                  f"- [完整结果与身份](<{(self.output / 'results.csv').as_posix()}>)。",
                  f"- [全部预设配对](<{(self.output / 'paired.csv').as_posix()}>)。",
                  "- CSV 保留源 engine、binary、输入、图、回执和原始标签/时钟的 SHA，原始阶段计数与吞吐轮次不折叠丢弃。",
                  "- 大型FP32输入身份沿用冻结计划和完成回执，真值、查询编号及源块分组在本地重新核对SHA。",
                  "- bootstrap 区间条件于当前图与固定实验协议，不提供跨构建方差。",
                  "- 本报告没有改写既有权威主表，也没有将组件引擎结果换称完整 TDB 生产结果。", ""]
        return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    return Analysis(args.plan, args.run, args.out).analyze()


if __name__ == "__main__":
    sys.exit(main())
