#!/usr/bin/env python3
"""Deterministic smoke-test agent for benchmarks/run_agent.py.

This is not an LLM. It proves the outcome harness can pass prompts to an agent,
observe file changes, and run verification. Real comparisons should use
`benchmarks/run_agent.py --agent-command ...` with an actual coding agent.
"""

from __future__ import annotations

import os
from pathlib import Path


def replace(path: Path, before: str, after: str) -> None:
    content = path.read_text(encoding="utf-8")
    path.write_text(content.replace(before, after), encoding="utf-8")


def main() -> int:
    repo = Path(os.environ["BRE_AGENT_REPO"])
    task_id = os.environ["BRE_AGENT_TASK_ID"]

    if task_id == "secure_startup_modules":
        replace(repo / "src/db/connection.rs", '"plain"', '"secure"')
        replace(repo / "src/db/pool.rs", '"plain"', '"secure"')
        print("Updated connection and pool startup modes.")
        return 0

    if task_id == "release_workflow_runs_tests":
        replace(repo / ".github/workflows/release.yaml", "cargo check", "cargo test")
        print("Updated release workflow to run cargo test.")
        return 0

    print(f"No scripted behavior for task {task_id}.")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
