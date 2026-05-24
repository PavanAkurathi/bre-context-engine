#!/usr/bin/env python3
"""Run BRE's local context-selection benchmark suite.

This is intentionally model-free. It checks whether BRE provides the context and
failure evidence that an agent would need before we spend tokens on LLM trials.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SUITE = REPO_ROOT / "benchmarks" / "suite.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run BRE benchmark tasks")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument(
        "--keep-temp",
        action="store_true",
        help="keep materialized benchmark repos for debugging",
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


def materialize_task(task: dict[str, Any], root: Path) -> None:
    for rel_path, content in task.get("files", {}).items():
        path = root / rel_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    config = dict(task["bre_config"])
    if "max_tokens" not in config and "max_tokens" in task:
        config["max_tokens"] = task["max_tokens"]

    (root / "bre.json").write_text(
        json.dumps(config, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    if "js_aliases" in task:
        (root / ".bre-js-aliases.json").write_text(
            json.dumps(task["js_aliases"], indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )


def run_bre(task: dict[str, Any], root: Path, bre_bin: Path) -> subprocess.CompletedProcess[str]:
    max_tokens = str(task.get("max_tokens", 30000))
    command = [str(bre_bin), str(root), "--format", "json", "--max-tokens", max_tokens]
    if "js_aliases" in task:
        command.extend(["--js-aliases", ".bre-js-aliases.json"])

    return subprocess.run(
        command,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


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
    config = task["bre_config"]
    loaded = set(config.get("active_files", []))
    loaded.update(task.get("baseline_files", []))
    return loaded


def baseline_bytes(task: dict[str, Any], loaded: set[str]) -> int:
    files = task.get("files", {})
    return sum(len(files.get(path, "").encode("utf-8")) for path in loaded)


def diagnostics_match(payload: dict[str, Any], expected: list[dict[str, str]]) -> list[str]:
    failures = []
    diagnostics = payload.get("diagnostics", [])
    for expectation in expected:
        compiler = expectation.get("compiler")
        message_contains = expectation.get("message_contains")
        matched = False
        for diagnostic in diagnostics:
            compiler_ok = compiler is None or diagnostic.get("compiler") == compiler
            message_ok = message_contains is None or message_contains in diagnostic.get("message", "")
            if compiler_ok and message_ok:
                matched = True
                break
        if not matched:
            failures.append(f"missing diagnostic {expectation}")
    return failures


def score_task(task: dict[str, Any], bre_bin: Path, keep_temp: bool) -> dict[str, Any]:
    root = Path(tempfile.mkdtemp(prefix=f"bre-bench-{task['id']}-"))
    materialize_task(task, root)

    proc = run_bre(task, root, bre_bin)
    expected_exit_nonzero = bool(task.get("expected_exit_nonzero", False))
    failures: list[str] = []
    payload: dict[str, Any] | None = None

    result: dict[str, Any] = {
        "id": task["id"],
        "description": task.get("description", ""),
        "repo_path": str(root),
        "exit_code": proc.returncode,
        "stderr": proc.stderr.strip(),
        "stdout": proc.stdout.strip(),
        "baseline": {},
        "bre": {},
        "failures": failures,
    }

    if expected_exit_nonzero:
        if proc.returncode == 0:
            failures.append("expected nonzero exit, got success")
        expected_stderr = task.get("expected_stderr_contains")
        if expected_stderr and expected_stderr not in proc.stderr:
            failures.append(f"stderr did not contain {expected_stderr!r}")
        result["passed"] = not failures
        if not keep_temp:
            shutil.rmtree(root)
        return result

    if proc.returncode != 0:
        failures.append(f"unexpected nonzero exit: {proc.returncode}")
    else:
        try:
            payload = json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            failures.append(f"stdout was not valid JSON: {exc}")

    oracle = set(task.get("oracle_files", []))
    baseline_paths = baseline_loaded_paths(task)
    baseline = {
        "loaded_files": sorted(baseline_paths),
        "recall": recall(baseline_paths, oracle),
        "precision": precision(baseline_paths, oracle),
        "bytes": baseline_bytes(task, baseline_paths),
    }
    result["baseline"] = baseline

    if payload is not None:
        bre_paths = loaded_paths(payload)
        bre_recall = recall(bre_paths, oracle)
        bre_precision = precision(bre_paths, oracle)
        diagnostics = payload.get("diagnostics", [])
        result["bre"] = {
            "loaded_files": sorted(bre_paths),
            "recall": bre_recall,
            "precision": bre_precision,
            "bytes": context_bytes(payload),
            "diagnostic_count": len(diagnostics),
            "diagnostic_compilers": sorted({item.get("compiler", "") for item in diagnostics}),
        }

        min_recall = float(task.get("min_bre_recall", 1.0 if oracle else 0.0))
        if bre_recall < min_recall:
            failures.append(f"BRE recall {bre_recall:.2f} below required {min_recall:.2f}")

        forbidden_files = set(task.get("forbidden_loaded_files", []))
        forbidden_loaded = sorted(bre_paths & forbidden_files)
        if forbidden_loaded:
            failures.append(f"BRE loaded forbidden files: {', '.join(forbidden_loaded)}")

        failures.extend(diagnostics_match(payload, task.get("expected_diagnostics", [])))

    result["passed"] = not failures
    if not keep_temp:
        shutil.rmtree(root)
    return result


def print_table(results: list[dict[str, Any]]) -> None:
    rows = []
    for result in results:
        baseline = result.get("baseline", {})
        bre = result.get("bre", {})
        status = "PASS" if result["passed"] else "FAIL"
        rows.append(
            [
                result["id"],
                f"{baseline.get('recall', 1.0):.2f}",
                f"{bre.get('recall', 1.0):.2f}",
                str(len(bre.get("loaded_files", []))),
                str(bre.get("diagnostic_count", 0)),
                status,
            ]
        )

    headers = ["task", "baseline_recall", "bre_recall", "bre_files", "diagnostics", "result"]
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
    bre_bin = build_bre_binary(args.bre_bin)

    results = [
        score_task(task, bre_bin=bre_bin, keep_temp=args.keep_temp)
        for task in suite.get("tasks", [])
    ]
    summary = {
        "suite": str(args.suite),
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
