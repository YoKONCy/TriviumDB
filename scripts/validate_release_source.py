#!/usr/bin/env python3
"""核对发布 Tag、Release PR 与对应 master 提交的 CI 验收来源。"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlencode
from urllib.request import Request, urlopen


class ReleaseSourceError(Exception):
    """发布来源证据缺失或与 Tag 不一致。"""


def git(root: Path, *arguments: str) -> str:
    process = subprocess.run(
        ["git", *arguments], cwd=root, capture_output=True, text=True, check=False,
    )
    if process.returncode:
        raise ReleaseSourceError(f"Git 校验失败 (Git validation failed): {process.stderr.strip()}")
    return process.stdout.strip()


class GitHubAPI:
    def __init__(self, repository: str):
        self.prefix = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
        self.repository = repository

    def get(self, endpoint: str):
        headers = {"Accept": "application/vnd.github+json", "User-Agent": "TriviumDB-release-source"}
        if token := os.environ.get("GH_TOKEN"):
            headers["Authorization"] = f"Bearer {token}"
        request = Request(f"{self.prefix}/repos/{self.repository}/{endpoint}", headers=headers)
        with urlopen(request, timeout=30) as response:
            return json.load(response)

    def pages(self, endpoint: str, key: str | None = None) -> list[dict]:
        records = []
        for page in range(1, 101):
            separator = "&" if "?" in endpoint else "?"
            result = self.get(f"{endpoint}{separator}per_page=100&page={page}")
            batch = result if key is None else result[key]
            if not isinstance(batch, list):
                raise ReleaseSourceError("GitHub 返回的证据格式无效 (Invalid GitHub evidence)")
            records.extend(batch)
            if len(batch) < 100:
                return records
        raise ReleaseSourceError("发布来源证据超过分页上限 (Release evidence exceeds pagination limit)")


def validate(root: Path, tag: str, repository: str, api: GitHubAPI | None = None) -> dict:
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-beta\.[0-9]+)?", tag):
        raise ReleaseSourceError("发布 Tag 格式无效 (Invalid release tag)")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ReleaseSourceError("仓库身份格式无效 (Invalid repository identity)")
    revision = git(root, "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
    if git(root, "rev-parse", "HEAD") != revision:
        raise ReleaseSourceError("工作区未检出发布 Tag (Checkout does not match release tag)")
    evidence = {"tag": tag, "revision": revision, "channel": "beta" if "-beta." in tag else "stable"}
    # Beta 的公开测试准入沿用独立质量门禁；Stable 需要完整 Release PR 来源证据。
    if evidence["channel"] == "beta":
        return evidence
    if revision not in git(root, "rev-list", "--first-parent", "origin/master").splitlines():
        raise ReleaseSourceError("Stable Tag 未位于 master 发布主线 (Stable tag is outside master release history)")
    parents = git(root, "rev-list", "--parents", "-n", "1", revision).split()
    if len(parents) != 3:
        raise ReleaseSourceError("Stable Tag 需要指向 Release 合并提交 (Stable tag requires a release merge commit)")
    api = api or GitHubAPI(repository)
    pulls = api.pages(f"commits/{revision}/pulls")
    releases = [pull for pull in pulls if (
        pull.get("merged_at")
        and pull.get("merge_commit_sha") == revision
        and pull.get("base", {}).get("ref") == "master"
        and pull.get("head", {}).get("ref") == "dev"
        and pull.get("base", {}).get("repo", {}).get("full_name") == repository
        and (pull.get("head", {}).get("repo") or {}).get("full_name") == repository
        and pull.get("head", {}).get("sha") == parents[2]
    )]
    if len(releases) != 1:
        raise ReleaseSourceError("缺少唯一且已合并的同仓库 dev → master Release PR (Missing unique merged release PR)")
    query = urlencode({"head_sha": revision, "event": "push"})
    runs = api.pages(f"actions/workflows/ci.yml/runs?{query}", "workflow_runs")
    runs = [run for run in runs if (
        run.get("head_sha") == revision and run.get("head_branch") == "master"
        and run.get("event") == "push" and run.get("path") == ".github/workflows/ci.yml"
    )]
    latest = max(runs, key=lambda run: (run["id"], run.get("run_attempt", 1)), default=None)
    if latest is None or latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise ReleaseSourceError("Release 提交缺少成功的 master CI (Release commit requires successful master CI)")
    evidence.update(release_pr=releases[0]["number"], ci_run=latest["id"])
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True)
    arguments = parser.parse_args()
    try:
        evidence = validate(Path.cwd(), arguments.tag, arguments.repository)
    except (ReleaseSourceError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"发布来源校验失败 (Release source validation failed): {error}")
        return 1
    print("发布来源校验通过 (Release source validated): " + json.dumps(evidence, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
