"""将权威 JSON 按数据集及职责导出为可直接离线加载的数据分片。"""

import json
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "results/benchmark-snapshot.json"
DIRECTORY = ROOT / "dashboard/data"
ENTRY = ROOT / "dashboard/index.html"


def write_part(relative, expression, value):
    """分片只改变存储封装，数值和身份字段完整保留。"""
    payload = json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False)
    payload = payload.replace("\u2028", "\\u2028").replace("\u2029", "\\u2029")
    path = DIRECTORY / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".js.tmp")
    temporary.write_text("// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。\n" + expression + " = " + payload + ";\n", encoding="utf-8", newline="\n")
    temporary.replace(path)


def split_annotations(value, pool, lookup):
    """同一数据集的重复备注与证据对象共享存储，装配时恢复原始内容。"""
    if isinstance(value, list):
        return [split_annotations(item, pool, lookup) for item in value]
    if not isinstance(value, dict):
        return value
    if set(value) == {"$annotation"}:
        raise ValueError("源记录与注释引用保留字段冲突")
    result = {}
    for key, item in value.items():
        if key in ("notes", "evidence") and isinstance(item, dict):
            identity = json.dumps(item, ensure_ascii=False, sort_keys=True, allow_nan=False)
            if identity not in lookup:
                lookup[identity] = len(pool)
                pool.append(item)
            result[key] = {"$annotation": lookup[identity]}
        else:
            result[key] = split_annotations(item, pool, lookup)
    return result


def main():
    snapshot = json.loads(SOURCE.read_text(encoding="utf-8"))
    catalog = {
        "meta": snapshot["meta"], "datasets": snapshot["datasets"],
        "configOrder": [item["id"] for item in snapshot["configs"]],
        "curveOrder": [item["id"] for item in snapshot["curves"]],
    }
    write_part("catalog.js", "window.QuiverBenchmarkParts", {"catalog": catalog, "configurations": {}, "observations": {}, "annotations": {}})
    scripts = ["catalog.js"]
    for dataset in snapshot["datasets"]:
        identity = dataset["id"]
        if not re.fullmatch(r"D\d{2}", identity):
            raise ValueError(f"数据集身份无法作为分片名 {identity}")
        pool, lookup = [], {}
        configs = split_annotations([item for item in snapshot["configs"] if item["dataset"] == identity], pool, lookup)
        dataset_curves = [item for item in snapshot["curves"] if item["dataset"] == identity]
        curve_parts = []
        for config in configs:
            config_id = config["id"]
            if not re.fullmatch(r"[A-Za-z0-9_.-]+", config_id):
                raise ValueError(f"配置身份无法作为分片名 {config_id}")
            curves = split_annotations([item for item in dataset_curves if item["config"] == config_id], pool, lookup)
            relative = f"observations/{identity}/{config_id}.js"
            write_part(relative, f'window.QuiverBenchmarkParts.observations["{config_id}"]', curves)
            curve_parts.append(relative)
        for folder, name, values in [("annotations", "annotations", pool), ("configurations", "configurations", configs)]:
            relative = f"{folder}/{identity}.js"
            write_part(relative, f'window.QuiverBenchmarkParts.{name}["{identity}"]', values)
            scripts.append(relative)
        scripts.extend(curve_parts)
        obsolete = DIRECTORY / "observations" / f"{identity}.js"
        if obsolete.is_file() and obsolete.resolve().is_relative_to(DIRECTORY.resolve()):
            obsolete.unlink()
    write_part("reference-notes.js", "window.QuiverBenchmarkParts.notes", snapshot["notes"])
    write_part("historical-bq.js", "window.QuiverBenchmarkParts.historicalBQ", snapshot["historicalBQ"])
    scripts.extend(["reference-notes.js", "historical-bq.js", "assemble.js"])
    tag_list = "\n".join(f'    <script defer src="data/{name}"></script>' for name in scripts)
    block = "<!-- 数据分片开始 -->\n" + tag_list + "\n    <!-- 数据分片结束 -->"
    html = ENTRY.read_text(encoding="utf-8")
    if "<!-- 数据分片开始 -->" in html:
        html, count = re.subn(r"<!-- 数据分片开始 -->.*?<!-- 数据分片结束 -->", lambda _: block, html, count=1, flags=re.S)
    else:
        html, count = re.subn(r'<script defer src="data/benchmark.js"></script>', lambda _: block, html, count=1)
    if count != 1:
        raise ValueError("数据分片加载位置无法确定")
    ENTRY.write_text(html, encoding="utf-8", newline="\n")
    print(f"看板数据已拆分，共 {len(scripts)} 个文件、{snapshot['meta']['points']} 条观测")


if __name__ == "__main__":
    main()
