#!/usr/bin/env python3
"""Run BRE against shallow clones of real open-source repositories."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SUITE = REPO_ROOT / "benchmarks" / "real-repo-suite.json"
DEFAULT_CACHE_DIR = Path.home() / ".cache" / "bre-real-repos"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run BRE stress trials on real repos")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE_DIR)
    parser.add_argument("--refresh", action="store_true", help="re-clone cached repositories")
    parser.add_argument("--run-diagnostics", action="store_true", help="allow BRE to run compiler diagnostics")
    parser.add_argument("--limit", type=int, default=None, help="run only the first N tasks")
    parser.add_argument("--task", action="append", default=[], help="run only matching task id(s)")
    parser.add_argument("--clone-timeout", type=int, default=300)
    parser.add_argument("--bre-timeout", type=int, default=180)
    parser.add_argument(
        "--scoring-policy",
        choices=["max", "additive", "p-norm"],
        default=None,
        help="override the task scoring policy for BRE",
    )
    parser.add_argument(
        "--scoring-p",
        type=int,
        default=2,
        help="p value used when --scoring-policy p-norm is selected",
    )
    parser.add_argument(
        "--bre-bin",
        type=Path,
        default=None,
        help="path to a prebuilt bre-context-engine binary",
    )
    return parser.parse_args()


def load_suite(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def build_bre_binary(explicit_bin: Path | None) -> Path:
    if explicit_bin is not None:
        if not explicit_bin.is_file():
            raise SystemExit(f"BRE binary does not exist: {explicit_bin}")
        return explicit_bin

    subprocess.run(
        ["cargo", "build", "--quiet", "--bin", "bre-context-engine"],
        cwd=REPO_ROOT,
        check=True,
    )
    executable = "bre-context-engine.exe" if os.name == "nt" else "bre-context-engine"
    return REPO_ROOT / "target" / "debug" / executable


def slug(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "__", value).strip("_")


def repo_cache_path(task: dict[str, Any], cache_dir: Path) -> Path:
    repo_name = task.get("full_name") or task.get("repo") or task["id"]
    return cache_dir / slug(repo_name)


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


def clone_repo(
    task: dict[str, Any],
    cache_dir: Path,
    refresh: bool,
    clone_timeout: int,
) -> tuple[Path | None, str | None]:
    url = task["repo"]
    branch = task.get("branch")
    repo_path = repo_cache_path(task, cache_dir)

    if refresh and repo_path.exists():
        shutil.rmtree(repo_path)

    if repo_path.exists():
        return repo_path, None

    repo_path.parent.mkdir(parents=True, exist_ok=True)
    args = ["git", "clone", "--depth", "1", "--single-branch"]
    if branch:
        args.extend(["--branch", branch])
    args.extend([url, str(repo_path)])

    proc = run_command(args, REPO_ROOT, clone_timeout)
    if isinstance(proc, subprocess.TimeoutExpired):
        return None, f"clone timed out after {clone_timeout}s"
    if proc.returncode != 0:
        message = (proc.stderr or proc.stdout).strip()
        return None, f"clone failed: {message}"

    return repo_path, None


def git_commit(repo_path: Path) -> str:
    proc = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=repo_path,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return proc.stdout.strip() if proc.returncode == 0 else ""


def write_bre_config(task: dict[str, Any], repo_path: Path) -> None:
    config = dict(task["bre_config"])
    if "max_tokens" not in config and "max_tokens" in task:
        config["max_tokens"] = task["max_tokens"]

    (repo_path / "bre.json").write_text(
        json.dumps(config, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def run_bre(
    task: dict[str, Any],
    repo_path: Path,
    bre_bin: Path,
    run_diagnostics: bool,
    bre_timeout: int,
    scoring_policy: str | None = None,
    scoring_p: int = 2,
) -> subprocess.CompletedProcess[str] | subprocess.TimeoutExpired[str]:
    max_tokens = str(task.get("max_tokens", 30000))
    args = [str(bre_bin), str(repo_path), "--format", "json", "--max-tokens", max_tokens]
    if not run_diagnostics:
        args.append("--skip-diagnostics")
    if scoring_policy:
        args.extend(["--scoring-policy", scoring_policy])
        if scoring_policy == "p-norm":
            args.extend(["--scoring-p", str(scoring_p)])
    return run_command(args, REPO_ROOT, bre_timeout)


def loaded_paths(payload: dict[str, Any]) -> set[str]:
    context = payload["context"]
    paths = {item["path"] for item in context.get("active", [])}
    paths.update(item["path"] for item in context.get("watching", []))
    return paths


def context_bytes(payload: dict[str, Any]) -> int:
    total = 0
    for bucket in ("active", "watching"):
        for item in payload["context"].get(bucket, []):
            total += len(item.get("content", "").encode("utf-8"))
    return total


def recall(loaded: set[str], oracle: set[str]) -> float:
    if not oracle:
        return 1.0
    return len(loaded & oracle) / len(oracle)


def precision(loaded: set[str], oracle: set[str]) -> float:
    if not loaded:
        return 1.0 if not oracle else 0.0
    if not oracle:
        return 1.0
    return len(loaded & oracle) / len(loaded)


def baseline_loaded_paths(task: dict[str, Any]) -> set[str]:
    loaded = set(task["bre_config"].get("active_files", []))
    loaded.update(task.get("baseline_files", []))
    return loaded


def missing_files(repo_path: Path, rel_paths: list[str]) -> list[str]:
    return [rel_path for rel_path in rel_paths if not (repo_path / rel_path).is_file()]


def score_task(
    task: dict[str, Any],
    cache_dir: Path,
    bre_bin: Path,
    refresh: bool,
    run_diagnostics: bool,
    clone_timeout: int,
    bre_timeout: int,
    scoring_policy: str | None = None,
    scoring_p: int = 2,
) -> dict[str, Any]:
    started = time.monotonic()
    failures: list[str] = []
    repo_path, clone_error = clone_repo(task, cache_dir, refresh, clone_timeout)
    result: dict[str, Any] = {
        "id": task["id"],
        "repo": task.get("full_name", task["repo"]),
        "url": task["repo"],
        "branch": task.get("branch", ""),
        "commit": "",
        "repo_path": str(repo_path) if repo_path else "",
        "exit_code": None,
        "stderr": "",
        "baseline": {},
        "bre": {},
        "scoring_policy": scoring_policy or "max",
        "failures": failures,
        "elapsed_seconds": 0.0,
    }

    if clone_error or repo_path is None:
        failures.append(clone_error or "clone failed")
        result["passed"] = False
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        return result

    result["commit"] = git_commit(repo_path)

    active_files = task["bre_config"].get("active_files", [])
    missing_active = missing_files(repo_path, active_files)
    if missing_active:
        failures.append(f"missing active files: {', '.join(missing_active)}")

    oracle = set(task.get("oracle_files", []))
    missing_oracle = missing_files(repo_path, sorted(oracle))
    if missing_oracle:
        failures.append(f"missing oracle files: {', '.join(missing_oracle)}")

    baseline_paths = baseline_loaded_paths(task)
    result["baseline"] = {
        "loaded_files": sorted(baseline_paths),
        "recall": recall(baseline_paths, oracle),
        "precision": precision(baseline_paths, oracle),
    }

    if not failures:
        write_bre_config(task, repo_path)
        proc = run_bre(
            task,
            repo_path,
            bre_bin,
            run_diagnostics,
            bre_timeout,
            scoring_policy=scoring_policy,
            scoring_p=scoring_p,
        )
        if isinstance(proc, subprocess.TimeoutExpired):
            failures.append(f"BRE timed out after {bre_timeout}s")
        else:
            result["exit_code"] = proc.returncode
            result["stderr"] = proc.stderr.strip()
            if proc.returncode != 0:
                failures.append(f"BRE exited {proc.returncode}: {(proc.stderr or proc.stdout).strip()}")
            else:
                try:
                    payload = json.loads(proc.stdout)
                except json.JSONDecodeError as exc:
                    failures.append(f"stdout was not valid JSON: {exc}")
                else:
                    bre_paths = loaded_paths(payload)
                    diagnostics = payload.get("diagnostics", [])
                    context = payload.get("context", {})
                    bre_recall = recall(bre_paths, oracle)
                    bre_precision = precision(bre_paths, oracle)
                    result["bre"] = {
                        "loaded_files": sorted(bre_paths),
                        "active_count": len(context.get("active", [])),
                        "watching_count": len(context.get("watching", [])),
                        "background_count": len(context.get("background", [])),
                        "recall": bre_recall,
                        "precision": bre_precision,
                        "bytes": context_bytes(payload),
                        "diagnostic_count": len(diagnostics),
                    }

                    min_recall = float(task.get("min_bre_recall", 1.0 if oracle else 0.0))
                    if bre_recall < min_recall:
                        failures.append(
                            f"BRE recall {bre_recall:.2f} below required {min_recall:.2f}"
                        )

                    min_precision = task.get("min_bre_precision")
                    if min_precision is not None and bre_precision < float(min_precision):
                        failures.append(
                            f"BRE precision {bre_precision:.2f} below required {float(min_precision):.2f}"
                        )

                    max_loaded_files = task.get("max_loaded_files")
                    if max_loaded_files is not None and len(bre_paths) > int(max_loaded_files):
                        failures.append(
                            f"BRE loaded {len(bre_paths)} files, above limit {max_loaded_files}"
                        )

                    forbidden_files = set(task.get("forbidden_loaded_files", []))
                    forbidden_loaded = sorted(bre_paths & forbidden_files)
                    if forbidden_loaded:
                        failures.append(f"BRE loaded forbidden files: {', '.join(forbidden_loaded)}")

                    forbidden_prefixes = task.get("forbidden_loaded_prefixes", [])
                    prefixed = sorted(
                        path
                        for path in bre_paths
                        if any(path.startswith(prefix) for prefix in forbidden_prefixes)
                    )
                    if prefixed:
                        failures.append(f"BRE loaded forbidden prefixes: {', '.join(prefixed)}")

    result["passed"] = not failures
    result["elapsed_seconds"] = round(time.monotonic() - started, 3)
    return result


def print_table(results: list[dict[str, Any]]) -> None:
    rows = []
    for result in results:
        bre = result.get("bre", {})
        status = "PASS" if result["passed"] else "FAIL"
        rows.append(
            [
                result["id"],
                result["repo"],
                f"{bre.get('recall', 0.0):.2f}",
                f"{bre.get('precision', 0.0):.2f}",
                str(len(bre.get("loaded_files", []))),
                str(bre.get("bytes", 0)),
                f"{result['elapsed_seconds']:.1f}s",
                status,
            ]
        )

    headers = ["task", "repo", "recall", "precision", "files", "bytes", "time", "result"]
    widths = [len(header) for header in headers]
    for row in rows:
        for idx, value in enumerate(row):
            widths[idx] = max(widths[idx], len(value))

    print(" | ".join(header.ljust(widths[idx]) for idx, header in enumerate(headers)))
    print("-+-".join("-" * width for width in widths))
    for row in rows:
        print(" | ".join(value.ljust(widths[idx]) for idx, value in enumerate(row)))

    failed = [result for result in results if not result["passed"]]
    if failed:
        print("\nFailures:")
        for result in failed:
            print(f"- {result['id']}: {'; '.join(result['failures'])}")


def main() -> int:
    args = parse_args()
    suite = load_suite(args.suite)
    tasks = suite.get("tasks", [])
    if args.task:
        wanted = set(args.task)
        tasks = [task for task in tasks if task["id"] in wanted]
    if args.limit is not None:
        tasks = tasks[: args.limit]

    bre_bin = build_bre_binary(args.bre_bin)
    results = [
        score_task(
            task,
            cache_dir=args.cache_dir,
            bre_bin=bre_bin,
            refresh=args.refresh,
            run_diagnostics=args.run_diagnostics,
            clone_timeout=args.clone_timeout,
            bre_timeout=args.bre_timeout,
            scoring_policy=args.scoring_policy,
            scoring_p=args.scoring_p,
        )
        for task in tasks
    ]
    summary = {
        "suite": str(args.suite),
        "cache_dir": str(args.cache_dir),
        "task_count": len(results),
        "passed": sum(1 for result in results if result["passed"]),
        "failed": sum(1 for result in results if not result["passed"]),
        "results": results,
    }

    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print_table(results)
        print(f"\nPassed {summary['passed']}/{summary['task_count']} tasks.")

    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
