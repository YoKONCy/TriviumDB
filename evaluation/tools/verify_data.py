"""核对发布文件的身份、完整测量表与离线看板的数据副本。"""

import csv
import hashlib
import json
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
manifest = json.loads((ROOT / "provenance/manifest.json").read_text(encoding="utf-8"))
for record in manifest["files"]:
    path = ROOT / record["path"]
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != record["sha256"]:
        raise ValueError(f"发布文件身份不一致 {record['path']}")

snapshot = json.loads((ROOT / "results/benchmark-snapshot.json").read_text(encoding="utf-8"))
configs = {record["id"]: record for record in snapshot["configs"]}
def read_part(relative, expression):
    text = (ROOT / "dashboard/data" / relative).read_text(encoding="utf-8")
    match = re.search(re.escape(expression) + r" = (.*);\s*\Z", text, re.S)
    if match is None:
        raise ValueError(f"数据分片格式无效 {relative}")
    return json.loads(match.group(1))


def restore_annotations(value, pool):
    if isinstance(value, list):
        return [restore_annotations(item, pool) for item in value]
    if not isinstance(value, dict):
        return value
    if set(value) == {"$annotation"}:
        index = value["$annotation"]
        if type(index) is not int or index < 0 or index >= len(pool):
            raise ValueError("数据注释引用无效")
        return pool[index]
    return {key: restore_annotations(item, pool) for key, item in value.items()}


catalog = read_part("catalog.js", "window.QuiverBenchmarkParts")["catalog"]
split_configs = {}
split_curves = {}
for dataset in catalog["datasets"]:
    identity = dataset["id"]
    pool = read_part(f"annotations/{identity}.js", f'window.QuiverBenchmarkParts.annotations["{identity}"]')
    group = read_part(f"configurations/{identity}.js", f'window.QuiverBenchmarkParts.configurations["{identity}"]')
    for config in restore_annotations(group, pool):
        split_configs[config["id"]] = config
        group = read_part(f'observations/{identity}/{config["id"]}.js', f'window.QuiverBenchmarkParts.observations["{config["id"]}"]')
        for curve in restore_annotations(group, pool):
            split_curves[curve["id"]] = curve
dashboard_data = {
    "meta": catalog["meta"], "datasets": catalog["datasets"],
    "configs": [split_configs[identity] for identity in catalog["configOrder"]],
    "curves": [split_curves[identity] for identity in catalog["curveOrder"]],
    "historicalBQ": read_part("historical-bq.js", "window.QuiverBenchmarkParts.historicalBQ"),
    "notes": read_part("reference-notes.js", "window.QuiverBenchmarkParts.notes"),
}
if dashboard_data != snapshot:
    raise ValueError("离线看板与公开数据快照不一致")

with (ROOT / "results/all-operating-points.csv").open(encoding="utf-8-sig", newline="") as stream:
    table = list(csv.DictReader(stream))
rows = {(row["curve_id"], row["threads"]): row for row in table}
if len(rows) != len(table):
    raise ValueError("完整测量表存在重复观测身份")

count = 0
for curve in snapshot["curves"]:
    config = configs[curve["config"]]
    for thread, point in curve["points"].items():
        row = rows[(curve["id"], thread)]
        for source, column in [("recall", "recall_at_10"), ("qps", "qps"), ("p50", "p50_ms"), ("p95", "p95_ms"), ("p99", "p99_ms")]:
            value = point.get(source)
            if value is None:
                if row[column] != "":
                    raise ValueError(f"未测量指标被填入数值 {curve['id']} {column}")
            elif float(row[column]) != value:
                raise ValueError(f"导出指标与原快照不一致 {curve['id']} {column}")
        if json.loads(row["qps_rounds"]) != point.get("samples"):
            raise ValueError(f"五轮吞吐数组不一致 {curve['id']}")
        if row["measurement_domain"] != (curve.get("domain") or ""):
            raise ValueError(f"计时域不一致 {curve['id']}")
        for value, column in [(config.get("hot"), "navigation_gib"),
                              (config.get("cold"), "rerank_gib"),
                              (config.get("build"), "build_s"),
                              (config.get("install"), "sm_install_s"),
                              (point.get("resident"), "query_rss_gib")]:
            if (value is None and row[column] != "") or (value is not None and float(row[column]) != value):
                raise ValueError(f"组件或计时记录不一致 {curve['id']} {column}")
        for metric in ("p95", "p99"):
            interval = point.get(metric + "ci") or [None, None]
            for value, side in zip(interval, ("low", "high")):
                column = metric + "_ci_" + side + "_ms"
                if (value is None and row[column] != "") or (value is not None and float(row[column]) != value):
                    raise ValueError(f"延迟区间不一致 {curve['id']} {column}")
        count += 1
if count != len(table) or count != manifest["observations"]:
    raise ValueError("公开测量覆盖数量不一致")
print(f"公开资料核验完成，共 {count} 个观测、{manifest['methods']} 种方法 (Verified publication data)")

