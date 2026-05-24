#!/usr/bin/env python3
"""Run outcome benchmarks with a pluggable coding agent command.

The context-selection harness asks "did BRE find the right files?" This runner
asks the next question: "does that context help an agent produce a verified
patch?" It runs each task twice by default:

* baseline: active files plus explicit baseline files
* bre: BRE Active + Watching context

The agent itself is supplied as a shell command so the benchmark can run against
Codex, another LLM agent, or a local smoke-test script without changing the
harness.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SUITE = REPO_ROOT / "benchmarks" / "agent-suite.json"
SKIP_DIRS = {".git", "target", "node_modules", "dist", "build", "__pycache__"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run BRE agent outcome benchmarks")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument("--keep-temp", action="store_true", help="keep temp repos and prompt files")
    parser.add_argument(
        "--mode",
        choices=["baseline", "bre", "both"],
        default="both",
        help="which context mode to run",
    )
    parser.add_argument(
        "--agent-command",
        default=None,
        help="shell command to run as the coding agent; prompt is sent on stdin",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="materialize repos and prompts but do not run an agent",
    )
    parser.add_argument("--agent-timeout", type=int, default=300)
    parser.add_argument("--verify-timeout", type=int, default=120)
    parser.add_argument("--run-diagnostics", action="store_true", help="include BRE diagnostics")
    parser.add_argument(
        "--bre-bin",
        type=Path,
        default=None,
        help="path to a prebuilt bre-context-engine binary",
    )
    parser.add_argument("--task", action="append", default=[], help="run only matching task id(s)")
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


def run_command(
    command: str,
    root: Path,
    timeout: int,
    stdin: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str] | subprocess.TimeoutExpired[str]:
    try:
        return subprocess.run(
            command,
            cwd=root,
            env=env,
            input=stdin,
            shell=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        return exc


def run_bre(
    task: dict[str, Any],
    root: Path,
    bre_bin: Path,
    run_diagnostics: bool,
) -> subprocess.CompletedProcess[str]:
    max_tokens = str(task.get("max_tokens", 30000))
    args = [str(bre_bin), str(root), "--format", "json", "--max-tokens", max_tokens]
    if not run_diagnostics:
        args.append("--skip-diagnostics")
    return subprocess.run(
        args,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def read_context_file(root: Path, rel_path: str) -> str:
    path = root / rel_path
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return path.read_bytes().decode("utf-8", errors="replace")


def baseline_payload(task: dict[str, Any], root: Path) -> dict[str, Any]:
    active_paths = list(task["bre_config"].get("active_files", []))
    baseline_paths = [
        path for path in task.get("baseline_files", []) if path not in set(active_paths)
    ]

    def context_file(path: str) -> dict[str, Any]:
        return {"path": path, "content": read_context_file(root, path), "is_skeleton": False}

    return {
        "goal": task["bre_config"].get("goal", ""),
        "acceptance_criteria": task["bre_config"].get("acceptance_criteria", []),
        "assumptions": task["bre_config"].get("assumptions", []),
        "non_goals": task["bre_config"].get("non_goals", []),
        "diagnostics": [],
        "context": {
            "active": [context_file(path) for path in active_paths],
            "watching": [context_file(path) for path in baseline_paths],
            "background": [],
        },
    }


def loaded_paths(payload: dict[str, Any]) -> set[str]:
    context = payload.get("context", {})
    paths = {item["path"] for item in context.get("active", [])}
    paths.update(item["path"] for item in context.get("watching", []))
    return paths


def context_bytes(payload: dict[str, Any]) -> int:
    total = 0
    for bucket in ("active", "watching"):
        for item in payload.get("context", {}).get(bucket, []):
            total += len(item.get("content", "").encode("utf-8"))
    return total


def make_prompt(task: dict[str, Any], payload: dict[str, Any], mode: str) -> str:
    verification = task.get("verification_commands", [])
    lines = [
        "You are a coding agent running inside a temporary repository.",
        "Modify the repository so the task is genuinely complete.",
        "Prefer using the provided context before exploring unrelated files.",
        "Do not edit bre.json or benchmark verification files unless the task explicitly asks.",
        "",
        f"Context mode: {mode}",
        "",
        "--- TASK CONTRACT ---",
        f"Goal: {payload.get('goal', '')}",
        "",
    ]

    if payload.get("acceptance_criteria"):
        lines.append("Acceptance Criteria:")
        lines.extend(f"- {item}" for item in payload["acceptance_criteria"])
        lines.append("")

    if payload.get("assumptions"):
        lines.append("Assumptions:")
        lines.extend(f"- {item}" for item in payload["assumptions"])
        lines.append("")

    if payload.get("non_goals"):
        lines.append("Non-Goals:")
        lines.extend(f"- {item}" for item in payload["non_goals"])
        lines.append("")

    if verification:
        lines.append("Verification Commands:")
        lines.extend(f"- {command}" for command in verification)
        lines.append("")

    diagnostics = payload.get("diagnostics", [])
    if diagnostics:
        lines.append("--- BUILD DIAGNOSTICS ---")
        for diagnostic in diagnostics:
            lines.append(
                f"[{diagnostic.get('compiler', '')}] {diagnostic.get('file', '')}:{diagnostic.get('line', 0)}"
            )
            lines.append(diagnostic.get("message", ""))
            lines.append("")

    lines.append("--- PROVIDED CONTEXT ---")
    for bucket in ("active", "watching"):
        title = "Active" if bucket == "active" else "Watching"
        lines.append(f"{title} Files:")
        files = payload.get("context", {}).get(bucket, [])
        if not files:
            lines.append("(none)")
            lines.append("")
            continue
        for file_info in files:
            path = file_info["path"]
            label = f"{path} (skeletonized)" if file_info.get("is_skeleton") else path
            lines.append("=" * 80)
            lines.append(f"File: {label}")
            lines.append("=" * 80)
            lines.append(file_info.get("content", ""))
            if not lines[-1].endswith("\n"):
                lines.append("")
        lines.append("")

    lines.append("--- OUTPUT EXPECTATION ---")
    lines.append("Apply the patch in-place. Then print a short summary and any verification run.")
    return "\n".join(lines)


def should_skip_dir(path: Path) -> bool:
    return any(part in SKIP_DIRS for part in path.parts)


def snapshot_files(root: Path) -> dict[str, str]:
    snapshot: dict[str, str] = {}
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        rel = path.relative_to(root)
        if should_skip_dir(rel):
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        snapshot[rel.as_posix()] = digest
    return snapshot


def changed_files(before: dict[str, str], after: dict[str, str]) -> list[str]:
    paths = set(before) | set(after)
    return sorted(path for path in paths if before.get(path) != after.get(path))


def run_verification(
    task: dict[str, Any],
    root: Path,
    timeout: int,
) -> tuple[bool, list[dict[str, Any]]]:
    results = []
    ok = True
    for command in task.get("verification_commands", []):
        started = time.monotonic()
        proc = run_command(command, root, timeout)
        elapsed = round(time.monotonic() - started, 3)
        if isinstance(proc, subprocess.TimeoutExpired):
            ok = False
            results.append(
                {
                    "command": command,
                    "exit_code": None,
                    "timed_out": True,
                    "elapsed_seconds": elapsed,
                    "stdout": proc.stdout or "",
                    "stderr": proc.stderr or "",
                }
            )
            continue

        if proc.returncode != 0:
            ok = False
        results.append(
            {
                "command": command,
                "exit_code": proc.returncode,
                "timed_out": False,
                "elapsed_seconds": elapsed,
                "stdout": proc.stdout.strip(),
                "stderr": proc.stderr.strip(),
            }
        )

    return ok, results


def score_changed_files(task: dict[str, Any], changes: set[str]) -> list[str]:
    failures = []
    required = set(task.get("required_changed_files", []))
    missing_required = sorted(required - changes)
    if missing_required:
        failures.append(f"missing required changes: {', '.join(missing_required)}")

    forbidden = set(task.get("forbidden_changed_files", []))
    forbidden_changed = sorted(forbidden & changes)
    if forbidden_changed:
        failures.append(f"changed forbidden files: {', '.join(forbidden_changed)}")

    forbidden_prefixes = task.get("forbidden_changed_prefixes", [])
    prefixed = sorted(
        path for path in changes if any(path.startswith(prefix) for prefix in forbidden_prefixes)
    )
    if prefixed:
        failures.append(f"changed forbidden prefixes: {', '.join(prefixed)}")

    return failures


def run_one_mode(
    task: dict[str, Any],
    mode: str,
    bre_bin: Path,
    args: argparse.Namespace,
) -> dict[str, Any]:
    root = Path(tempfile.mkdtemp(prefix=f"bre-agent-{task['id']}-{mode}-"))
    materialize_task(task, root)
    failures: list[str] = []
    payload: dict[str, Any] | None = None

    if mode == "baseline":
        try:
            payload = baseline_payload(task, root)
        except OSError as exc:
            failures.append(f"failed to build baseline context: {exc}")
    else:
        proc = run_bre(task, root, bre_bin, args.run_diagnostics)
        if proc.returncode != 0:
            failures.append(f"BRE exited {proc.returncode}: {(proc.stderr or proc.stdout).strip()}")
        else:
            try:
                payload = json.loads(proc.stdout)
            except json.JSONDecodeError as exc:
                failures.append(f"BRE stdout was not valid JSON: {exc}")

    prompt = make_prompt(task, payload or {}, mode)
    prompt_path = root / ".bre-agent-prompt.md"
    prompt_path.write_text(prompt, encoding="utf-8")

    result: dict[str, Any] = {
        "mode": mode,
        "repo_path": str(root),
        "prompt_path": str(prompt_path),
        "context_files": sorted(loaded_paths(payload or {})),
        "context_bytes": context_bytes(payload or {}),
        "agent": {},
        "verification": [],
        "changed_files": [],
        "oracle_changed_files": [],
        "failures": failures,
        "passed": False,
        "dry_run": args.dry_run,
    }

    if failures or args.dry_run:
        if args.dry_run:
            result["passed"] = True
        if not args.keep_temp:
            shutil.rmtree(root)
        return result

    if not args.agent_command:
        failures.append("missing --agent-command (or use --dry-run)")
        if not args.keep_temp:
            shutil.rmtree(root)
        return result

    before = snapshot_files(root)
    env = os.environ.copy()
    env.update(
        {
            "BRE_AGENT_TASK_ID": task["id"],
            "BRE_AGENT_MODE": mode,
            "BRE_AGENT_PROMPT": str(prompt_path),
            "BRE_AGENT_REPO": str(root),
        }
    )

    started = time.monotonic()
    agent_proc = run_command(
        args.agent_command,
        root,
        timeout=args.agent_timeout,
        stdin=prompt,
        env=env,
    )
    agent_elapsed = round(time.monotonic() - started, 3)
    if isinstance(agent_proc, subprocess.TimeoutExpired):
        failures.append(f"agent timed out after {args.agent_timeout}s")
        result["agent"] = {
            "exit_code": None,
            "timed_out": True,
            "elapsed_seconds": agent_elapsed,
            "stdout": agent_proc.stdout or "",
            "stderr": agent_proc.stderr or "",
        }
    else:
        if agent_proc.returncode != 0:
            failures.append(f"agent exited {agent_proc.returncode}")
        result["agent"] = {
            "exit_code": agent_proc.returncode,
            "timed_out": False,
            "elapsed_seconds": agent_elapsed,
            "stdout": agent_proc.stdout.strip(),
            "stderr": agent_proc.stderr.strip(),
        }

    after = snapshot_files(root)
    changes = set(changed_files(before, after))
    result["changed_files"] = sorted(changes)
    oracle = set(task.get("oracle_files", []))
    result["oracle_changed_files"] = sorted(changes & oracle)
    failures.extend(score_changed_files(task, changes))

    verification_ok, verification = run_verification(task, root, args.verify_timeout)
    result["verification"] = verification
    if not verification_ok:
        failures.append("verification failed")

    result["failures"] = failures
    result["passed"] = not failures
    if not args.keep_temp:
        shutil.rmtree(root)
    return result


def print_table(results: list[dict[str, Any]]) -> None:
    rows = []
    for task_result in results:
        for mode_result in task_result["modes"]:
            status = "PASS" if mode_result["passed"] else "FAIL"
            rows.append(
                [
                    task_result["id"],
                    mode_result["mode"],
                    str(len(mode_result["context_files"])),
                    str(mode_result["context_bytes"]),
                    str(len(mode_result["changed_files"])),
                    str(len(mode_result["oracle_changed_files"])),
                    status,
                ]
            )

    headers = ["task", "mode", "ctx_files", "ctx_bytes", "changed", "oracle_changed", "result"]
    widths = [len(header) for header in headers]
    for row in rows:
        for idx, value in enumerate(row):
            widths[idx] = max(widths[idx], len(value))

    print(" | ".join(header.ljust(widths[idx]) for idx, header in enumerate(headers)))
    print("-+-".join("-" * width for width in widths))
    for row in rows:
        print(" | ".join(value.ljust(widths[idx]) for idx, value in enumerate(row)))

    failures = [
        (task_result["id"], mode_result)
        for task_result in results
        for mode_result in task_result["modes"]
        if not mode_result["passed"]
    ]
    if failures:
        print("\nFailures:")
        for task_id, mode_result in failures:
            print(f"- {task_id}/{mode_result['mode']}: {'; '.join(mode_result['failures'])}")


def main() -> int:
    args = parse_args()
    if not args.dry_run and not args.agent_command:
        raise SystemExit("provide --agent-command or use --dry-run")

    suite = load_suite(args.suite)
    tasks = suite.get("tasks", [])
    if args.task:
        wanted = set(args.task)
        tasks = [task for task in tasks if task["id"] in wanted]

    modes = ["baseline", "bre"] if args.mode == "both" else [args.mode]
    bre_bin = build_bre_binary(args.bre_bin)
    results = []
    for task in tasks:
        mode_results = [run_one_mode(task, mode, bre_bin, args) for mode in modes]
        results.append({"id": task["id"], "description": task.get("description", ""), "modes": mode_results})

    summary = {
        "suite": str(args.suite),
        "task_count": len(results),
        "mode_count": sum(len(result["modes"]) for result in results),
        "passed": sum(
            1 for result in results for mode_result in result["modes"] if mode_result["passed"]
        ),
        "failed": sum(
            1 for result in results for mode_result in result["modes"] if not mode_result["passed"]
        ),
        "results": results,
    }

    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print_table(results)
        print(f"\nPassed {summary['passed']}/{summary['mode_count']} runs.")

    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
