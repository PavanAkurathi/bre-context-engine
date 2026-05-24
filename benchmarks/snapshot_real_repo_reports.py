#!/usr/bin/env python3
"""Create or verify stable BRE report snapshots for real-repo stress tasks."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

from run_real_repos import (
    DEFAULT_CACHE_DIR,
    DEFAULT_SUITE,
    REPO_ROOT,
    build_bre_binary,
    clone_repo,
    git_commit,
    load_suite,
    write_bre_config,
)


DEFAULT_TASKS = ["vscode_keybinding_resolver", "opennext_cloudflare_build_pipeline"]
DEFAULT_SNAPSHOT_DIR = REPO_ROOT / "benchmarks" / "snapshots" / "real-repo-reports"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Snapshot BRE --report output for real repos")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE_DIR)
    parser.add_argument("--snapshot-dir", type=Path, default=DEFAULT_SNAPSHOT_DIR)
    parser.add_argument("--task", action="append", default=[], help="task id to snapshot")
    parser.add_argument("--update", action="store_true", help="write snapshots instead of checking")
    parser.add_argument("--refresh", action="store_true", help="re-clone cached repositories")
    parser.add_argument("--clone-timeout", type=int, default=300)
    parser.add_argument("--bre-timeout", type=int, default=300)
    parser.add_argument("--bre-bin", type=Path, default=None)
    return parser.parse_args()


def selected_tasks(suite: dict[str, Any], requested: list[str]) -> list[dict[str, Any]]:
    wanted = requested or DEFAULT_TASKS
    task_by_id = {task["id"]: task for task in suite.get("tasks", [])}
    missing = [task_id for task_id in wanted if task_id not in task_by_id]
    if missing:
        raise SystemExit(f"unknown task id(s): {', '.join(missing)}")
    return [task_by_id[task_id] for task_id in wanted]


def run_bre_report(
    task: dict[str, Any],
    repo_path: Path,
    bre_bin: Path,
    bre_timeout: int,
) -> dict[str, Any]:
    max_tokens = str(task.get("max_tokens", 30000))
    args = [
        str(bre_bin),
        str(repo_path),
        "--format",
        "json",
        "--max-tokens",
        max_tokens,
        "--skip-diagnostics",
        "--report",
    ]
    proc = subprocess.run(
        args,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=bre_timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"BRE exited {proc.returncode}: {(proc.stderr or proc.stdout).strip()}")
    return json.loads(proc.stdout)


def tray_paths(payload: dict[str, Any], tray: str) -> list[str]:
    return [item["path"] for item in payload["context"].get(tray, [])]


def normalize_payload(task: dict[str, Any], repo_path: Path, payload: dict[str, Any]) -> dict[str, Any]:
    report = payload["report"]
    report = json.loads(json.dumps(report, sort_keys=True))
    report["repo_root"] = "<repo_root>"
    report["weights"]["weights_path"] = "<repo_root>/bre_weights.json"

    return {
        "task_id": task["id"],
        "repo": task.get("full_name", task["repo"]),
        "url": task["repo"],
        "branch": task.get("branch", ""),
        "commit": git_commit(repo_path),
        "max_tokens": task.get("max_tokens", 30000),
        "selected_paths": {
            "active": tray_paths(payload, "active"),
            "watching": tray_paths(payload, "watching"),
            "background_count": len(payload["context"].get("background", [])),
        },
        "diagnostic_count": len(payload.get("diagnostics", [])),
        "report": report,
    }


def snapshot_path(snapshot_dir: Path, task_id: str) -> Path:
    return snapshot_dir / f"{task_id}.json"


def write_snapshot(path: Path, snapshot: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def read_snapshot(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    args = parse_args()
    suite = load_suite(args.suite)
    tasks = selected_tasks(suite, args.task)
    bre_bin = build_bre_binary(args.bre_bin)
    failures: list[str] = []

    for task in tasks:
        repo_path, clone_error = clone_repo(
            task,
            cache_dir=args.cache_dir,
            refresh=args.refresh,
            clone_timeout=args.clone_timeout,
        )
        if clone_error or repo_path is None:
            failures.append(f"{task['id']}: {clone_error or 'clone failed'}")
            continue

        write_bre_config(task, repo_path)
        try:
            payload = run_bre_report(task, repo_path, bre_bin, args.bre_timeout)
        except (RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError) as exc:
            failures.append(f"{task['id']}: {exc}")
            continue

        snapshot = normalize_payload(task, repo_path, payload)
        path = snapshot_path(args.snapshot_dir, task["id"])
        if args.update:
            write_snapshot(path, snapshot)
            print(f"updated {path}")
            continue

        if not path.is_file():
            failures.append(f"{task['id']}: missing snapshot {path}")
            continue

        expected = read_snapshot(path)
        if snapshot != expected:
            failures.append(f"{task['id']}: snapshot drifted; run with --update after review")
        else:
            print(f"ok {path}")

    if failures:
        print("\nFailures:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
