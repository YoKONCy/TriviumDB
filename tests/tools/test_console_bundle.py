"""验收真实资源包中的完整内容、来源与可复现性。"""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]


class ConsoleBundleTests(unittest.TestCase):
    def test_bundle_contains_exact_frontend_and_host_contract(self):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root) / "console.zip"
            command = [sys.executable, str(ROOT / "scripts/package_console.py"), "--output", str(output)]
            subprocess.run(command, check=True, capture_output=True)
            first = output.read_bytes()
            subprocess.run(command, check=True, capture_output=True)
            self.assertEqual(output.read_bytes(), first)
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(set(archive.namelist()), {"index.html", "console-contract.json", "manifest.json", "docs/embedded-console.md", "LICENSE"})
                html = archive.read("index.html")
                self.assertEqual(html, (ROOT / "crates/triviumdb-server/web/index.html").read_bytes())
                self.assertEqual(archive.read("console-contract.json"), (ROOT / "src/database/console_contract.json").read_bytes())
                self.assertEqual(archive.read("docs/embedded-console.md"), (ROOT / "docs/embedded-console.md").read_bytes())
                self.assertEqual(archive.read("LICENSE"), (ROOT / "LICENSE").read_bytes())
                manifest = json.loads(archive.read("manifest.json"))
                self.assertEqual(manifest["component"], "triviumdb-console")
                self.assertEqual(manifest["packageVersion"], json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"])
                self.assertEqual(manifest["htmlSha256"], hashlib.sha256(html).hexdigest())
                self.assertEqual(manifest["protocolVersion"], 1)
                self.assertTrue(manifest["requiresFrontendTransportAdapter"])
                self.assertIn("https://cdn.jsdelivr.net/npm/gsap@3.13.0/dist/gsap.min.js", manifest["externalAssetUrls"])


if __name__ == "__main__":
    unittest.main()
