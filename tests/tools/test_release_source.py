"""真实 Git 历史与独立 GitHub 响应样例验证发布来源；仅隔离网络边界。"""

from copy import deepcopy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("release_source", Path(__file__).resolve().parents[2] / "scripts/validate_release_source.py")
release_source = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release_source)


class EvidenceAPI:
    def __init__(self, pulls, runs):
        self.pulls = pulls
        self.runs = runs

    def pages(self, endpoint, key=None):
        return deepcopy(self.pulls if key is None else self.runs)


class ReleaseSourceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="tdb-release-source-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git("init", "--initial-branch=master")
        self.git("config", "user.name", "Release source test")
        self.git("config", "user.email", "release-test@example.invalid")
        self.git("commit", "--allow-empty", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("checkout", "-b", "dev")
        self.git("commit", "--allow-empty", "-m", "candidate")
        self.candidate = self.git("rev-parse", "HEAD")
        self.git("checkout", "master")
        self.git("merge", "--no-ff", "dev", "-m", "release")
        self.revision = self.git("rev-parse", "HEAD")
        self.git("tag", "v1.2.3")
        self.git("update-ref", "refs/remotes/origin/master", self.revision)
        self.pull = {
            "number": 7, "merged_at": "2026-01-01T00:00:00Z", "merge_commit_sha": self.revision,
            "head": {"ref": "dev", "sha": self.candidate, "repo": {"full_name": "owner/project"}},
            "base": {"ref": "master", "repo": {"full_name": "owner/project"}},
        }
        self.run = {"id": 10, "head_sha": self.revision, "head_branch": "master", "event": "push",
                    "path": ".github/workflows/ci.yml", "status": "completed", "conclusion": "success"}

    def git(self, *arguments):
        return subprocess.run(["git", *arguments], cwd=self.root, check=True, capture_output=True, text=True).stdout.strip()

    def validate(self, pulls=None, runs=None, tag="v1.2.3"):
        return release_source.validate(self.root, tag, "owner/project", EvidenceAPI(
            [self.pull] if pulls is None else pulls, [self.run] if runs is None else runs))

    def test_release_merge_with_exact_ci_is_accepted(self):
        self.assertEqual(self.validate(), {"tag": "v1.2.3", "revision": self.revision,
                                          "channel": "stable", "release_pr": 7, "ci_run": 10})

    def test_historical_release_can_be_republished_after_master_advances(self):
        self.git("commit", "--allow-empty", "-m", "later release")
        self.git("update-ref", "refs/remotes/origin/master", self.git("rev-parse", "HEAD"))
        self.git("checkout", "--detach", self.revision)
        self.assertEqual(self.validate()["revision"], self.revision)

    def test_ordinary_master_commit_is_rejected(self):
        self.git("checkout", "--detach", self.base)
        self.git("tag", "v1.2.2")
        with self.assertRaises(release_source.ReleaseSourceError):
            self.validate(tag="v1.2.2")

    def test_merged_feature_ancestor_is_rejected(self):
        self.git("checkout", "--detach", self.candidate)
        self.git("tag", "v1.2.4")
        with self.assertRaises(release_source.ReleaseSourceError):
            self.validate(tag="v1.2.4")

    def test_missing_or_incorrect_release_pr_is_rejected(self):
        variants = [[], [dict(self.pull, merged_at=None)], [dict(self.pull, merge_commit_sha=self.base)]]
        for side, field, value in [("head", "ref", "feature"), ("base", "ref", "dev"),
                                    ("head", "sha", self.base), ("head", "repo", {"full_name": "fork/project"}),
                                    ("base", "repo", {"full_name": "fork/project"}), ("head", "repo", None)]:
            pull = deepcopy(self.pull)
            pull[side][field] = value
            variants.append([pull])
        for pulls in variants:
            with self.subTest(pulls=pulls), self.assertRaises(release_source.ReleaseSourceError):
                self.validate(pulls=pulls)

    def test_ci_must_cover_exact_master_revision_and_succeed(self):
        for field, value in [("head_sha", self.candidate), ("head_branch", "dev"), ("event", "pull_request"),
                             ("path", ".github/workflows/server-release.yml"), ("status", "in_progress"),
                             ("conclusion", "failure"), ("conclusion", "cancelled"), ("conclusion", "skipped")]:
            with self.subTest(field=field, value=value), self.assertRaises(release_source.ReleaseSourceError):
                self.validate(runs=[dict(self.run, **{field: value})])
        with self.assertRaises(release_source.ReleaseSourceError):
            self.validate(runs=[])

    def test_latest_failed_ci_is_not_hidden_by_earlier_success(self):
        with self.assertRaises(release_source.ReleaseSourceError):
            self.validate(runs=[self.run, dict(self.run, id=11, conclusion="failure")])

    def test_wrong_checkout_and_invalid_tag_are_rejected(self):
        self.git("checkout", "--detach", self.base)
        for tag in ["v1.2.3", "HEAD", "v1.2.3-rc.1"]:
            with self.subTest(tag=tag), self.assertRaises(release_source.ReleaseSourceError):
                self.validate(tag=tag)

    def test_beta_uses_beta_quality_gate_policy(self):
        self.git("checkout", "--detach", self.candidate)
        self.git("tag", "v1.2.3-beta.1")
        self.assertEqual(self.validate(pulls=[], runs=[], tag="v1.2.3-beta.1"),
                         {"tag": "v1.2.3-beta.1", "revision": self.candidate, "channel": "beta"})

    def test_release_pr_on_later_api_page_is_validated(self):
        pull, run = self.pull, self.run

        class PagedAPI(release_source.GitHubAPI):
            def get(self, endpoint):
                if endpoint.startswith("commits/"):
                    return [{"number": -1}] * 100 if endpoint.endswith("&page=1") else [pull]
                return {"workflow_runs": [run]}

        evidence = release_source.validate(self.root, "v1.2.3", "owner/project", PagedAPI("owner/project"))
        self.assertEqual(evidence["release_pr"], 7)
        self.assertEqual(evidence["ci_run"], 10)

    def test_invalid_api_response_and_missing_network_evidence_fail(self):
        class InvalidAPI(release_source.GitHubAPI):
            def get(self, endpoint):
                return {"unexpected": "object"}

        with self.assertRaises(release_source.ReleaseSourceError):
            release_source.validate(self.root, "v1.2.3", "owner/project", InvalidAPI("owner/project"))

        class UnavailableAPI(release_source.GitHubAPI):
            def get(self, endpoint):
                raise PermissionError("外部网络边界拒绝访问")

        with self.assertRaises(PermissionError):
            release_source.validate(self.root, "v1.2.3", "owner/project", UnavailableAPI("owner/project"))

    def test_cli_uses_policy_outside_historical_tag_tree(self):
        self.git("tag", "v1.2.3-beta.1")
        policy_directory = self.root / "runner-temp"
        policy_directory.mkdir()
        policy = policy_directory / "validate_release_source.py"
        policy.write_bytes(Path(SPEC.origin).read_bytes())
        # 样例 Tag 的树为空；校验器由工作流版本保存在 Runner 临时目录。
        self.assertEqual(self.git("ls-tree", "--name-only", "v1.2.3-beta.1"), "")
        result = subprocess.run([sys.executable, str(policy), "--tag", "v1.2.3-beta.1", "--repository", "owner/project"],
                                cwd=self.root, capture_output=True, text=True, encoding="utf-8", check=False,
                                env={**os.environ, "PYTHONIOENCODING": "utf-8"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(result.stdout.split(": ", 1)[1])
        self.assertEqual(evidence, {"tag": "v1.2.3-beta.1", "revision": self.revision, "channel": "beta"})


if __name__ == "__main__":
    unittest.main()
