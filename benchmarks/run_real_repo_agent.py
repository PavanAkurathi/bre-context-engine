#!/usr/bin/env python3
"""Run agent outcome prompts against cached real repositories.

This is the real-repo sibling of run_agent.py. It uses the tasks from
real-repo-suite.json, creates a disposable git worktree for each task/mode, and
then compares baseline context against BRE context. By default it can be run as
a dry-run prompt/context audit; pass --agent-command to let a coding agent patch
the temp worktree.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

from run_agent import (
    changed_files,
    context_bytes,
    loaded_paths,
    make_prompt,
    read_context_file,
    run_command,
    run_verification,
    score_changed_files,
    snapshot_files,
)
from run_real_repos import (
    DEFAULT_CACHE_DIR,
    DEFAULT_SUITE,
    build_bre_binary,
    clone_repo,
    git_commit,
    precision,
    recall,
    write_bre_config,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
SCORING_POLICIES = ["max", "additive", "p-norm"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run real-repo agent outcome benchmarks")
    parser.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    parser.add_argument("--cache-dir", type=Path, default=DEFAULT_CACHE_DIR)
    parser.add_argument("--refresh", action="store_true", help="re-clone cached repositories")
    parser.add_argument("--task", action="append", default=[], help="run only matching task id(s)")
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
    parser.add_argument("--dry-run", action="store_true", help="create prompts but do not run an agent")
    parser.add_argument("--keep-temp", action="store_true", help="keep temp worktrees and prompt files")
    parser.add_argument("--clone-timeout", type=int, default=300)
    parser.add_argument("--bre-timeout", type=int, default=300)
    parser.add_argument("--agent-timeout", type=int, default=300)
    parser.add_argument("--verify-timeout", type=int, default=120)
    parser.add_argument("--run-diagnostics", action="store_true", help="include BRE diagnostics")
    parser.add_argument(
        "--scoring-policy",
        choices=[*SCORING_POLICIES, "all"],
        default="max",
        help="BRE scoring policy to use; pass all to run max, additive, and p-norm",
    )
    parser.add_argument(
        "--scoring-p",
        type=int,
        default=2,
        help="p value used when --scoring-policy p-norm or all is selected",
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


def selected_scoring_policies(args: argparse.Namespace) -> list[str]:
    if args.scoring_policy == "all":
        return SCORING_POLICIES
    return [args.scoring_policy]


def scoring_policy_label(scoring_policy: str | None, scoring_p: int) -> str:
    if scoring_policy is None:
        return "n/a"
    if scoring_policy == "p-norm":
        return f"p-norm:{scoring_p}"
    return scoring_policy


def worktree_slug(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-")


def baseline_payload(task: dict[str, Any], root: Path) -> dict[str, Any]:
    active_paths = list(task["bre_config"].get("active_files", []))
    active_set = set(active_paths)
    baseline_paths = [path for path in task.get("baseline_files", []) if path not in active_set]

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


def run_bre(
    task: dict[str, Any],
    root: Path,
    bre_bin: Path,
    run_diagnostics: bool,
    timeout: int,
    scoring_policy: str,
    scoring_p: int,
) -> subprocess.CompletedProcess[str] | subprocess.TimeoutExpired[str]:
    max_tokens = str(task.get("max_tokens", 30000))
    args = [str(bre_bin), str(root), "--format", "json", "--max-tokens", max_tokens]
    if not run_diagnostics:
        args.append("--skip-diagnostics")
    args.extend(["--scoring-policy", scoring_policy])
    if scoring_policy == "p-norm":
        args.extend(["--scoring-p", str(scoring_p)])

    try:
        return subprocess.run(
            args,
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        return exc


def create_worktree(repo_path: Path, task_id: str, mode_label: str) -> tuple[Path, Path]:
    temp_root = Path(
        tempfile.mkdtemp(prefix=f"bre-real-agent-{task_id}-{worktree_slug(mode_label)}-")
    )
    worktree = temp_root / "repo"
    proc = subprocess.run(
        ["git", "worktree", "add", "--detach", "--quiet", str(worktree), "HEAD"],
        cwd=repo_path,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        shutil.rmtree(temp_root, ignore_errors=True)
        raise RuntimeError((proc.stderr or proc.stdout).strip())
    return temp_root, worktree


def cleanup_worktree(repo_path: Path, temp_root: Path, worktree: Path) -> None:
    subprocess.run(
        ["git", "worktree", "remove", "--force", str(worktree)],
        cwd=repo_path,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    shutil.rmtree(temp_root, ignore_errors=True)


def score_context(payload: dict[str, Any], oracle: set[str]) -> dict[str, Any]:
    paths = loaded_paths(payload)
    return {
        "loaded_files": sorted(paths),
        "recall": recall(paths, oracle),
        "precision": precision(paths, oracle),
        "bytes": context_bytes(payload),
    }


def run_one_mode(
    task: dict[str, Any],
    mode: str,
    scoring_policy: str | None,
    source_repo: Path,
    bre_bin: Path,
    args: argparse.Namespace,
) -> dict[str, Any]:
    temp_root: Path | None = None
    worktree: Path | None = None
    failures: list[str] = []
    payload: dict[str, Any] | None = None
    oracle = set(task.get("oracle_files", []))
    policy_label = scoring_policy_label(scoring_policy, args.scoring_p)
    mode_label = mode if scoring_policy is None else f"{mode}:{policy_label}"

    try:
        temp_root, worktree = create_worktree(source_repo, task["id"], mode_label)
        write_bre_config(task, worktree)

        if mode == "baseline":
            payload = baseline_payload(task, worktree)
        else:
            assert scoring_policy is not None
            proc = run_bre(
                task,
                worktree,
                bre_bin,
                args.run_diagnostics,
                args.bre_timeout,
                scoring_policy,
                args.scoring_p,
            )
            if isinstance(proc, subprocess.TimeoutExpired):
                failures.append(f"BRE timed out after {args.bre_timeout}s")
            elif proc.returncode != 0:
                failures.append(f"BRE exited {proc.returncode}: {(proc.stderr or proc.stdout).strip()}")
            else:
                try:
                    payload = json.loads(proc.stdout)
                except json.JSONDecodeError as exc:
                    failures.append(f"BRE stdout was not valid JSON: {exc}")

        prompt = make_prompt(task, payload or {}, mode_label)
        prompt_path = worktree / ".bre-agent-prompt.md"
        prompt_path.write_text(prompt, encoding="utf-8")

        context_score = score_context(payload or {}, oracle)
        result: dict[str, Any] = {
            "mode": mode,
            "scoring_policy": policy_label,
            "repo_path": str(worktree),
            "prompt_path": str(prompt_path),
            "context_files": context_score["loaded_files"],
            "context_bytes": context_score["bytes"],
            "context_recall": context_score["recall"],
            "context_precision": context_score["precision"],
            "agent": {},
            "verification": [],
            "changed_files": [],
            "oracle_changed_files": [],
            "failures": failures,
            "passed": False,
            "dry_run": args.dry_run,
        }

        if failures or args.dry_run:
            if args.dry_run and not failures:
                result["passed"] = True
            return result

        if not args.agent_command:
            failures.append("missing --agent-command (or use --dry-run)")
            result["failures"] = failures
            return result

        before = snapshot_files(worktree)
        env = os.environ.copy()
        env.update(
            {
                "BRE_AGENT_TASK_ID": task["id"],
                "BRE_AGENT_MODE": mode,
                "BRE_AGENT_SCORING_POLICY": policy_label,
                "BRE_AGENT_PROMPT": str(prompt_path),
                "BRE_AGENT_REPO": str(worktree),
            }
        )

        started = time.monotonic()
        agent_proc = run_command(
            args.agent_command,
            worktree,
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

        after = snapshot_files(worktree)
        changes = set(changed_files(before, after))
        result["changed_files"] = sorted(changes)
        result["oracle_changed_files"] = sorted(changes & oracle)
        failures.extend(score_changed_files(task, changes))

        verification_ok, verification = run_verification(task, worktree, args.verify_timeout)
        result["verification"] = verification
        if not verification_ok:
            failures.append("verification failed")

        result["failures"] = failures
        result["passed"] = not failures
        return result
    except Exception as exc:
        return {
            "mode": mode,
            "scoring_policy": policy_label,
            "repo_path": str(worktree or ""),
            "prompt_path": "",
            "context_files": [],
            "context_bytes": 0,
            "context_recall": 0.0,
            "context_precision": 0.0,
            "agent": {},
            "verification": [],
            "changed_files": [],
            "oracle_changed_files": [],
            "failures": [str(exc)],
            "passed": False,
            "dry_run": args.dry_run,
        }
    finally:
        if temp_root is not None and worktree is not None and not args.keep_temp:
            cleanup_worktree(source_repo, temp_root, worktree)


def print_table(results: list[dict[str, Any]]) -> None:
    rows = []
    for task_result in results:
        for mode_result in task_result["modes"]:
            status = "PASS" if mode_result["passed"] else "FAIL"
            rows.append(
                [
                    task_result["id"],
                    mode_result["mode"],
                    mode_result.get("scoring_policy", "n/a"),
                    f"{mode_result.get('context_recall', 0.0):.2f}",
                    f"{mode_result.get('context_precision', 0.0):.2f}",
                    str(len(mode_result["context_files"])),
                    str(mode_result["context_bytes"]),
                    str(len(mode_result["changed_files"])),
                    str(len(mode_result["oracle_changed_files"])),
                    status,
                ]
            )

    headers = [
        "task",
        "mode",
        "policy",
        "ctx_recall",
        "ctx_precision",
        "ctx_files",
        "ctx_bytes",
        "changed",
        "oracle_changed",
        "result",
    ]
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
            policy = mode_result.get("scoring_policy", "n/a")
            print(
                f"- {task_id}/{mode_result['mode']}[{policy}]: "
                f"{'; '.join(mode_result['failures'])}"
            )


def main() -> int:
    args = parse_args()
    if not args.dry_run and not args.agent_command:
        raise SystemExit("provide --agent-command or use --dry-run")

    suite = load_suite(args.suite)
    tasks = suite.get("tasks", [])
    if args.task:
        wanted = set(args.task)
        tasks = [task for task in tasks if task["id"] in wanted]

    if args.scoring_p < 1:
        raise SystemExit("--scoring-p must be at least 1")

    scoring_policies = selected_scoring_policies(args)
    mode_specs: list[tuple[str, str | None]] = []
    if args.mode in ("baseline", "both"):
        mode_specs.append(("baseline", None))
    if args.mode in ("bre", "both"):
        mode_specs.extend(("bre", policy) for policy in scoring_policies)

    bre_bin = build_bre_binary(args.bre_bin)
    results = []
    for task in tasks:
        source_repo, clone_error = clone_repo(
            task,
            args.cache_dir,
            args.refresh,
            args.clone_timeout,
        )
        if clone_error or source_repo is None:
            mode_results = [
                {
                    "mode": mode,
                    "scoring_policy": scoring_policy_label(scoring_policy, args.scoring_p),
                    "repo_path": "",
                    "prompt_path": "",
                    "context_files": [],
                    "context_bytes": 0,
                    "context_recall": 0.0,
                    "context_precision": 0.0,
                    "agent": {},
                    "verification": [],
                    "changed_files": [],
                    "oracle_changed_files": [],
                    "failures": [clone_error or "clone failed"],
                    "passed": False,
                    "dry_run": args.dry_run,
                }
                for mode, scoring_policy in mode_specs
            ]
            commit = ""
        else:
            commit = git_commit(source_repo)
            mode_results = [
                run_one_mode(task, mode, scoring_policy, source_repo, bre_bin, args)
                for mode, scoring_policy in mode_specs
            ]
        results.append(
            {
                "id": task["id"],
                "repo": task.get("full_name", task["repo"]),
                "commit": commit,
                "description": task.get("description", ""),
                "modes": mode_results,
            }
        )

    summary = {
        "suite": str(args.suite),
        "cache_dir": str(args.cache_dir),
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
