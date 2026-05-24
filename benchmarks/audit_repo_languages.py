#!/usr/bin/env python3
"""Audit language mix for real repos at a historical git snapshot.

The goal is to compare BRE's benchmark targets against a pre-AI-code baseline.
This script uses git history rather than today's working tree: it finds the
latest commit on the configured branch before a date, then counts files by
language using `git ls-tree` metadata.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SUITE = REPO_ROOT / "benchmarks" / "real-repo-suite.json"
DEFAULT_CACHE_DIR = Path.home() / ".cache" / "bre-language-audit"

LANGUAGE_BY_EXTENSION = {
    ".rs": "Rust",
    ".js": "JavaScript",
    ".jsx": "JavaScript",
    ".mjs": "JavaScript",
    ".cjs": "JavaScript",
    ".ts": "TypeScript",
    ".tsx": "TypeScript",
    ".py": "Python",
    ".go": "Go",
    ".java": "Java",
    ".kt": "Kotlin",
    ".kts": "Kotlin",
    ".swift": "Swift",
    ".c": "C",
    ".h": "C/C++ Header",
    ".cc": "C++",
    ".cpp": "C++",
    ".cxx": "C++",
    ".hpp": "C/C++ Header",
    ".hh": "C/C++ Header",
    ".cs": "C#",
    ".rb": "Ruby",
    ".php": "PHP",
    ".scala": "Scala",
    ".sh": "Shell",
    ".bash": "Shell",
    ".zsh": "Shell",
    ".fish": "Shell",
    ".html": "HTML",
    ".css": "CSS",
    ".scss": "CSS",
    ".sass": "CSS",
    ".less": "CSS",
    ".vue": "Vue",
    ".svelte": "Svelte",
    ".json": "JSON",
    ".jsonc": "JSON",
    ".toml": "TOML",
    ".yaml": "YAML",
    ".yml": "YAML",
    ".xml": "XML",
    ".md": "Markdown",
    ".mdx": "Markdown",
    ".rst": "reStructuredText",
    ".sql": "SQL",
    ".graphql": "GraphQL",
    ".proto": "Protocol Buffers",
    ".tf": "Terraform",
    ".lua": "Lua",
    ".dart": "Dart",
    ".ex": "Elixir",
    ".exs": "Elixir",
    ".erl": "Erlang",
    ".hrl": "Erlang",
    ".fs": "F#",
    ".fsx": "F#",
    ".clj": "Clojure",
    ".cljs": "Clojure",
    ".zig": "Zig",
}

LANGUAGE_BY_BASENAME = {
    "Dockerfile": "Dockerfile",
    "Makefile": "Makefile",
    "CMakeLists.txt": "CMake",
    "Cargo.toml": "TOML",
    "package.json": "JSON",
    "tsconfig.json": "JSON",
    "jsconfig.json": "JSON",
    "go.mod": "Go Module",
    "go.sum": "Go Module",
    "pyproject.toml": "TOML",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Audit historical repo language mix")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--date", default="2020-12-31", help="UTC cutoff date, e.g. 2020-12-31")
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE_DIR)
    parser.add_argument("--refresh", action="store_true", help="delete cached audit clones first")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--task", action="append", default=[], help="run only matching id/full_name")
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument("--top", type=int, default=5, help="number of top languages per repo")
    parser.add_argument("--clone-timeout", type=int, default=300)
    parser.add_argument("--count-timeout", type=int, default=120)
    return parser.parse_args()


def load_repos(path: Path) -> list[dict[str, Any]]:
    with path.open("r", encoding="utf-8") as handle:
        suite = json.load(handle)

    records = suite.get("tasks") or suite.get("repos") or []
    repos = []
    for record in records:
        full_name = record.get("full_name") or record.get("name") or record.get("id")
        url = record.get("repo") or record.get("url")
        if not full_name or not url:
            continue
        repos.append(
            {
                "id": record.get("id", slug(full_name)),
                "full_name": full_name,
                "repo": url if url.endswith(".git") else f"{url}.git",
                "branch": record.get("branch") or record.get("default_branch") or "main",
            }
        )
    return repos


def slug(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "__", value).strip("_")


def run_command(
    args: list[str],
    cwd: Path,
    timeout: int,
) -> subprocess.CompletedProcess[str] | subprocess.TimeoutExpired[str]:
    try:
        return subprocess.run(
            args,
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        return exc


def ensure_repo(
    repo: dict[str, Any],
    cache_dir: Path,
    refresh: bool,
    timeout: int,
) -> tuple[Path | None, str | None]:
    repo_path = cache_dir / slug(repo["full_name"])
    if refresh and repo_path.exists():
        shutil.rmtree(repo_path)

    if repo_path.exists():
        proc = run_command(
            [
                "git",
                "fetch",
                "--filter=blob:none",
                "origin",
                f"+refs/heads/{repo['branch']}:refs/remotes/origin/{repo['branch']}",
            ],
            repo_path,
            timeout,
        )
        if isinstance(proc, subprocess.TimeoutExpired):
            return None, f"fetch timed out after {timeout}s"
        if proc.returncode != 0:
            return None, f"fetch failed: {(proc.stderr or proc.stdout).strip()}"
        return repo_path, None

    repo_path.parent.mkdir(parents=True, exist_ok=True)
    proc = run_command(
        [
            "git",
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            "--single-branch",
            "--branch",
            repo["branch"],
            repo["repo"],
            str(repo_path),
        ],
        REPO_ROOT,
        timeout,
    )
    if isinstance(proc, subprocess.TimeoutExpired):
        return None, f"clone timed out after {timeout}s"
    if proc.returncode != 0:
        return None, f"clone failed: {(proc.stderr or proc.stdout).strip()}"
    return repo_path, None


def snapshot_commit(repo_path: Path, branch: str, date: str, timeout: int) -> tuple[str, str | None]:
    ref = f"refs/remotes/origin/{branch}"
    proc = run_command(
        ["git", "rev-list", "-n", "1", "--before", f"{date} 23:59:59 +0000", ref],
        repo_path,
        timeout,
    )
    if isinstance(proc, subprocess.TimeoutExpired):
        return "", f"rev-list timed out after {timeout}s"
    if proc.returncode != 0:
        return "", f"rev-list failed: {(proc.stderr or proc.stdout).strip()}"
    commit = proc.stdout.strip()
    if not commit:
        return "", f"no commit on {branch} before {date}"
    return commit, None


def git_short_commit(repo_path: Path, commit: str, timeout: int) -> str:
    proc = run_command(["git", "rev-parse", "--short", commit], repo_path, timeout)
    if isinstance(proc, subprocess.TimeoutExpired) or proc.returncode != 0:
        return commit[:12]
    return proc.stdout.strip()


def audit_commit(repo_path: Path, commit: str, timeout: int) -> tuple[Counter[str], Counter[str], str | None]:
    proc = run_command(["git", "ls-tree", "-r", "-z", "--name-only", commit], repo_path, timeout)
    if isinstance(proc, subprocess.TimeoutExpired):
        return Counter(), Counter(), f"ls-tree timed out after {timeout}s"
    if proc.returncode != 0:
        return Counter(), Counter(), f"ls-tree failed: {(proc.stderr or proc.stdout).strip()}"

    bytes_by_language: Counter[str] = Counter()
    files_by_language: Counter[str] = Counter()

    for file_path in proc.stdout.split("\0"):
        if not file_path:
            continue
        language = classify_language(file_path)
        files_by_language[language] += 1

    return bytes_by_language, files_by_language, None


def classify_language(file_path: str) -> str:
    name = Path(file_path).name
    if name in LANGUAGE_BY_BASENAME:
        return LANGUAGE_BY_BASENAME[name]
    suffixes = Path(file_path).suffixes
    if len(suffixes) >= 2:
        compound = "".join(suffixes[-2:])
        if compound in LANGUAGE_BY_EXTENSION:
            return LANGUAGE_BY_EXTENSION[compound]
    suffix = Path(file_path).suffix
    return LANGUAGE_BY_EXTENSION.get(suffix, "Other")


def top_languages(counter: Counter[str], top: int) -> list[dict[str, Any]]:
    total = sum(counter.values())
    return [
        {
            "language": language,
            "value": value,
            "share": round(value / total, 4) if total else 0.0,
        }
        for language, value in counter.most_common(top)
    ]


def audit_repo(
    repo: dict[str, Any],
    date: str,
    cache_dir: Path,
    refresh: bool,
    clone_timeout: int,
    count_timeout: int,
    top: int,
) -> dict[str, Any]:
    repo_path, repo_error = ensure_repo(repo, cache_dir, refresh, clone_timeout)
    result: dict[str, Any] = {
        "id": repo["id"],
        "full_name": repo["full_name"],
        "branch": repo["branch"],
        "date": date,
        "commit": "",
        "error": repo_error or "",
        "total_bytes": 0,
        "total_files": 0,
        "top_bytes": [],
        "top_files": [],
    }
    if repo_path is None:
        return result

    commit, commit_error = snapshot_commit(repo_path, repo["branch"], date, count_timeout)
    if commit_error:
        result["error"] = commit_error
        return result

    bytes_by_language, files_by_language, audit_error = audit_commit(repo_path, commit, count_timeout)
    if audit_error:
        result["error"] = audit_error
        return result

    result["commit"] = git_short_commit(repo_path, commit, count_timeout)
    result["total_bytes"] = sum(bytes_by_language.values())
    result["total_files"] = sum(files_by_language.values())
    result["top_bytes"] = top_languages(bytes_by_language, top)
    result["top_files"] = top_languages(files_by_language, top)
    return result


def render_languages(items: list[dict[str, Any]]) -> str:
    return ", ".join(f"{item['language']} {item['share']:.0%}" for item in items)


def print_table(results: list[dict[str, Any]]) -> None:
    headers = ["repo", "date", "commit", "files", "top_files", "result"]
    rows = []
    for result in results:
        rows.append(
            [
                result["full_name"],
                result["date"],
                result["commit"] or "-",
                str(result["total_files"]),
                render_languages(result["top_files"]) or "-",
                "PASS" if not result["error"] else f"SKIP: {result['error']}",
            ]
        )

    widths = [
        max(len(row[index]) for row in rows + [headers])
        for index in range(len(headers))
    ]
    print(" | ".join(header.ljust(widths[index]) for index, header in enumerate(headers)))
    print("-+-".join("-" * width for width in widths))
    for row in rows:
        print(" | ".join(cell.ljust(widths[index]) for index, cell in enumerate(row)))


def main() -> int:
    args = parse_args()
    repos = load_repos(args.suite)
    if args.task:
        requested = set(args.task)
        repos = [
            repo
            for repo in repos
            if repo["id"] in requested or repo["full_name"] in requested
        ]
    if args.limit is not None:
        repos = repos[: args.limit]

    results = [
        audit_repo(
            repo,
            args.date,
            args.cache_dir,
            args.refresh,
            args.clone_timeout,
            args.count_timeout,
            args.top,
        )
        for repo in repos
    ]

    if args.json:
        print(json.dumps({"date": args.date, "results": results}, indent=2))
    else:
        print_table(results)

    return 1 if any(result["error"] for result in results) else 0


if __name__ == "__main__":
    sys.exit(main())
