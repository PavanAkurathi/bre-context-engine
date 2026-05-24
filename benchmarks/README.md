# BRE Benchmark Harness

This harness is deliberately model-free. It tests whether BRE supplies the
files and failure evidence an agent would need before we spend tokens on LLM
runs.

Run it from the repo root:

```sh
python3 benchmarks/run.py
```

The suite materializes temporary repositories from `suite.json`, writes a
`bre.json` task contract, runs `bre-context-engine`, and compares loaded
context against oracle files.

## What It Measures

- `baseline_recall`: active files plus any explicit `baseline_files`.
- `bre_recall`: Active + Watching files emitted by BRE.
- `diagnostics`: number of diagnostics BRE exposed.
- `forbidden_loaded_files`: optional files that must not appear in Active or
  Watching context for precision-focused tasks.
- `result`: pass/fail against each task's required recall, diagnostics, and
  expected fail-closed/precision behavior.

## How To Make It Brutal

Add tasks that represent real agent failure modes:

- missing dependency hidden behind imports;
- risky config far away from edited code;
- invalid build/test setup;
- impossible token budgets;
- stale or malformed task contracts.

The next stage is an LLM-backed runner: use the same task suite, ask a fixed
agent/model to patch the temp repo with baseline context vs BRE context, then
score by tests, cost, time, and false-success rate.

## Agent Outcome Benchmark

Run the synthetic patch-outcome suite in dry-run mode:

```sh
python3 benchmarks/run_agent.py --dry-run
```

Run it end-to-end with the deterministic smoke agent:

```sh
python3 benchmarks/run_agent.py \
  --agent-command "python3 $(pwd)/benchmarks/scripted_agent.py"
```

The smoke agent is not an LLM; it only proves the harness can pass prompts,
observe patches, and run verification. To test a real coding agent, pass a
command that reads the prompt from stdin or from `$BRE_AGENT_PROMPT` and edits
the temporary repo in-place:

```sh
python3 benchmarks/run_agent.py \
  --agent-command 'your-agent-command --prompt-file "$BRE_AGENT_PROMPT"'
```

For each task, `run_agent.py` runs two modes by default:

- `baseline`: active files plus explicit `baseline_files`.
- `bre`: BRE Active + Watching context.

It scores verification commands, changed files, oracle-file changes, forbidden
changes, context bytes, and prompt file paths. Use `--keep-temp` to inspect the
materialized repositories and prompts after a run.

## Real Repo Candidate Set

`real-repos-gaming.json` lists 20 verified gaming/game-engine repositories for
the benchmark stage. The active suite also includes heavier monorepo add-ons,
such as VS Code, once the first-pass repos are stable.

Audit historical language mix for a pre-AI snapshot with:

```sh
python3 benchmarks/audit_repo_languages.py --date 2020-12-31
```

Use `2020-12-31` as the clean pre-AI baseline and `2023-01-01` as the
transition comparison point. See `pre-ai-language-audit.md` for the current
active-suite results.

Audit the enterprise candidate corpus with:

```sh
python3 benchmarks/audit_repo_languages.py \
  --suite benchmarks/enterprise-pre-ai-repos.json \
  --date 2020-12-31
```

`enterprise-pre-ai-strategy.md` explains where Kubernetes, Kafka, and framework
configuration fit into BRE: they are context boundary layers that must often be
loaded even when the active source file is in Java, C#, Python, C++, Go, or
TypeScript.

Run the current real-repo stress suite with:

```sh
python3 benchmarks/run_real_repos.py
```

This shallow-clones the repos in `real-repo-suite.json` into
`~/.cache/bre-real-repos`, writes a task-specific `bre.json`, and runs BRE with
diagnostics skipped by default so the trial measures context selection instead
of compiler setup time. Pass `--run-diagnostics` when a task explicitly needs
build evidence.

Compare scoring policies on the same real-repo tasks with:

```sh
python3 benchmarks/run_scoring_policies.py \
  --task vscode_keybinding_resolver \
  --task opennext_cloudflare_build_pipeline \
  --bre-timeout 300
```

This runs `max`, `additive`, and `p-norm` against each selected task. The
default BRE policy remains `max`; this comparison exists to gather evidence
before changing defaults or building the MMKP optimizer on top of a scoring
choice.

Verify the flagship `--report` snapshots with:

```sh
python3 benchmarks/snapshot_real_repo_reports.py
```

This reruns the VS Code and OpenNext Cloudflare stress tasks with `--report`,
normalizes machine-specific paths, and compares the nested report, selected
paths, counts, budgets, and file traces against snapshots in
`benchmarks/snapshots/real-repo-reports`. After intentional report-shape or
signal changes, update them with:

```sh
python3 benchmarks/snapshot_real_repo_reports.py --update
```

## JS Alias Handoff Coverage

The synthetic suite includes `js_alias_handoff_sidecar`, which writes a
`.bre-js-aliases.json` sidecar and runs BRE with `--js-aliases`. That task uses
a dynamic Vite-style alias assignment that the Rust fallback intentionally does
not try to evaluate, so it protects the editor/frontend handoff contract.

## Real Repo Agent Prompts

Audit baseline-vs-BRE prompts for a real repo task with:

```sh
python3 benchmarks/run_real_repo_agent.py \
  --dry-run \
  --task vscode_keybinding_resolver \
  --bre-timeout 300
```

This creates disposable git worktrees from the cached real-repo clone, writes
the task `bre.json`, and scores the provided prompt context. Compare all BRE
scoring policies at the prompt layer with:

```sh
python3 benchmarks/run_real_repo_agent.py \
  --dry-run \
  --scoring-policy all \
  --bre-timeout 300
```

To run an actual agent against the temp worktree, pass `--agent-command`; add
`--keep-temp` when you want to inspect the patched repository and prompt after
the run.
