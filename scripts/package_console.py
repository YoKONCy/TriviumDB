#!/usr/bin/env python3
"""从唯一维护的 HTML 与宿主协议生成可分发资源包，不改写前端内容。"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def package(output: Path) -> None:
    html = (ROOT / "crates/triviumdb-server/web/index.html").read_bytes()
    contract = (ROOT / "src/database/console_contract.json").read_bytes()
    version = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M)[1]
    manifest = {
        "component": "triviumdb-console",
        "packageVersion": version,
        "protocolVersion": json.loads(contract)["protocolVersion"],
        "htmlSha256": hashlib.sha256(html).hexdigest(),
        "requiresFrontendTransportAdapter": True,
        "externalAssetUrls": sorted(set(re.findall(r'https://cdn\.jsdelivr\.net/[^\s\'"<>]+', html.decode("utf-8")))),
    }
    files = {
        "index.html": html,
        "console-contract.json": contract,
        "manifest.json": json.dumps(manifest, ensure_ascii=False, indent=2).encode("utf-8"),
        "docs/embedded-console.md": (ROOT / "docs/embedded-console.md").read_bytes(),
        "LICENSE": (ROOT / "LICENSE").read_bytes(),
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    # 临时文件与最终制品同目录；失败时不留下半份 ZIP，不改写任何源码资源。
    with tempfile.TemporaryDirectory(prefix="console-package-", dir=output.parent) as temporary:
        staged = Path(temporary) / "console.zip"
        with zipfile.ZipFile(staged, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, content in files.items():
                entry = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = 0o100644 << 16
                archive.writestr(entry, content)
        staged.replace(output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    package(arguments.output.resolve())
    print(f"控制台资源包已生成 (Console resource bundle created): {arguments.output}")


if __name__ == "__main__":
    main()
