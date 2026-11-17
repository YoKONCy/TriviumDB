#!/usr/bin/env python3
"""TDB 版本迁移的唯一用户入口；调用独立、协议固定的原生迁移执行器。"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

PROTOCOL = 1
TOOL = "tdb-migrate-2"


def main():
    parser = argparse.ArgumentParser(description="离线迁移到新目录 (Offline migration into a new directory)")
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-dtype", choices=("f32", "f16", "u64"), help="显式确认旧空检查点的来源类型 (Explicit dtype for a legacy empty checkpoint)")
    parser.add_argument("--destination", required=True, type=Path)
    operation = parser.add_mutually_exclusive_group()
    operation.add_argument("--check", action="store_true", help="检查来源并在副本验证转换，源文件零写 (Check conversion on a copy without source writes)")
    operation.add_argument("--execute", action="store_true", help="转换并发布新目标目录 (Convert and publish a new destination directory)")
    parser.add_argument("--tool", type=Path, help="指定已构建的迁移工具 (Path to the built migration tool)")
    parser.add_argument("--storage", choices=("rom", "mmap"), default="mmap")
    parser.add_argument("--memory-mib", type=int, default=256)
    parser.add_argument("--max-source-mib", type=int, default=1024)
    parser.add_argument("--max-work-mib", type=int, default=8192)
    args = parser.parse_args()
    suffix = ".exe" if sys.platform == "win32" else ""
    tool = args.tool
    if tool is None:
        installed = shutil.which("triviumdb-migrate" + suffix)
        if installed:
            tool = Path(installed)
        else:
            root = Path(__file__).resolve().parent.parent
            for profile in ("release", "debug"):
                candidate = root / "target" / "migration" / profile / ("triviumdb-migrate" + suffix)
                if candidate.is_file():
                    tool = candidate
                    break
    if tool is None or not tool.is_file():
        parser.error("找不到迁移工具；先构建 crates/triviumdb-migrate (Migration tool not found; build crates/triviumdb-migrate first)")
    tool = tool.resolve()
    try:
        identity = subprocess.run([str(tool), "--protocol"], capture_output=True, check=True)
        identity = json.loads(identity.stdout)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        parser.error(f"无法验证迁移工具协议 (Cannot verify migration tool protocol): {error}")
    if not isinstance(identity, dict):
        parser.error("迁移工具协议响应无效 (Invalid migration tool protocol response)")
    if identity.get("protocol") != PROTOCOL or identity.get("tool") != TOOL:
        parser.error("迁移工具协议不匹配 (Migration tool protocol mismatch)")
    command = [str(tool), "--source", str(args.source), "--destination", str(args.destination),
               "--execute" if args.execute else "--check", "--storage", args.storage,
               "--memory-mib", str(args.memory_mib), "--max-source-mib", str(args.max_source_mib),
               "--max-work-mib", str(args.max_work_mib)]
    if args.source_dtype:
        command.extend(["--source-dtype", args.source_dtype])
    # 子进程持有全部租约；中断后等待进程结束，再向宿主返回。
    process = subprocess.Popen(command)
    try:
        return process.wait()
    except KeyboardInterrupt:
        process.terminate()
        process.wait()
        return 130


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    sys.exit(main())
