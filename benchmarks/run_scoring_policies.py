#!/usr/bin/env python3
"""Compare BRE scoring policies on the real-repo stress suite."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

from run_real_repos import (
    DEFAULT_CACHE_DIR,
    DEFAULT_SUITE,
    build_bre_binary,
    load_suite,
    score_task,
)


DEFAULT_POLICIES = ["max", "additive", "p-norm"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Compare BRE scoring policies on real repos")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE_DIR)
    parser.add_argument("--refresh", action="store_true", help="re-clone cached repositories")
    parser.add_argument("--run-diagnostics", action="store_true", help="allow BRE to run compiler diagnostics")
    parser.add_argument("--limit", type=int, default=None, help="run only the first N tasks")
    parser.add_argument("--task", action="append", default=[], help="run only matching task id(s)")
    parser.add_argument("--clone-timeout", type=int, default=300)
    parser.add_argument("--bre-timeout", type=int, default=300)
    parser.add_argument("--bre-bin", type=Path, default=None)
    parser.add_argument(
        "--policy",
        action="append",
        choices=DEFAULT_POLICIES,
        default=[],
        help="policy to run; may be passed multiple times",
    )
    parser.add_argument("--scoring-p", type=int, default=2)
    return parser.parse_args()


def selected_tasks(suite: dict[str, Any], requested: list[str], limit: int | None) -> list[dict[str, Any]]:
    tasks = suite.get("tasks", [])
    if requested:
        wanted = set(requested)
        tasks = [task for task in tasks if task["id"] in wanted]
    if limit is not None:
        tasks = tasks[:limit]
    return tasks


def policy_label(policy: str, scoring_p: int) -> str:
    return f"p-norm:{scoring_p}" if policy == "p-norm" else policy


def print_table(results: list[dict[str, Any]]) -> None:
    rows = []
    for result in results:
        bre = result.get("bre", {})
        rows.append(
            [
                result["id"],
                result["scoring_policy_label"],
                f"{bre.get('recall', 0.0):.2f}",
                f"{bre.get('precision', 0.0):.2f}",
                str(len(bre.get("loaded_files", []))),
                str(bre.get("bytes", 0)),
                f"{result['elapsed_seconds']:.1f}s",
                "PASS" if result["passed"] else "FAIL",
            ]
        )

    headers = ["task", "policy", "recall", "precision", "files", "bytes", "time", "result"]
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
            print(
                f"- {result['id']} [{result['scoring_policy_label']}]: "
                f"{'; '.join(result['failures'])}"
            )


def main() -> int:
    args = parse_args()
    suite = load_suite(args.suite)
    tasks = selected_tasks(suite, args.task, args.limit)
    policies = args.policy or DEFAULT_POLICIES
    bre_bin = build_bre_binary(args.bre_bin)

    results: list[dict[str, Any]] = []
    for task in tasks:
        for policy in policies:
            result = score_task(
                task,
                cache_dir=args.cache_dir,
                bre_bin=bre_bin,
                refresh=args.refresh,
                run_diagnostics=args.run_diagnostics,
                clone_timeout=args.clone_timeout,
                bre_timeout=args.bre_timeout,
                scoring_policy=policy,
                scoring_p=args.scoring_p,
            )
            result["scoring_policy_label"] = policy_label(policy, args.scoring_p)
            results.append(result)

    summary = {
        "suite": str(args.suite),
        "cache_dir": str(args.cache_dir),
        "policies": [policy_label(policy, args.scoring_p) for policy in policies],
        "task_count": len(tasks),
        "run_count": len(results),
        "passed": sum(1 for result in results if result["passed"]),
        "failed": sum(1 for result in results if not result["passed"]),
        "results": results,
    }

    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print_table(results)
        print(f"\nPassed {summary['passed']}/{summary['run_count']} policy runs.")

    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
