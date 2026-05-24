# BRE

**BRE** stands for **Bounded Reasoning Environment**.

BRE is an AI-native engineering workspace focused on making engineering outcomes faster by keeping context bounded, relevant, and verifiable.

The core belief is simple:

> The agent should not reason over everything. It should reason over the right things, explain why those things matter, and verify its work.

## Product Thesis

Most AI coding tools begin with the editor. BRE begins with the work.

A real engineering task is bigger than code completion. It includes user intent, tickets, docs, assumptions, touched files, risky config, tests, logs, browser behavior, and code review. Dumping all of that into one growing chat context burns tokens and creates drift.

BRE treats context as a managed engineering resource.

## First Principles

- Context is bounded by design.
- Files have both relevance and criticality.
- A cold file can still be dangerous.
- Jira tickets and Confluence docs should become distilled task contracts, not raw token dumps.
- Agent work should be inspectable through plans, diffs, commands, evidence, and unresolved risks.
- The unit of success is a verified engineering outcome, not a generated patch.

## MVP Shape

The first useful version of BRE has three parts:

1. **Task Contract**
   A structured representation of the current goal, acceptance criteria, non-goals, assumptions, and verification status.

2. **Context Temperature Engine**
   A repo-aware system that classifies files as hot, warm, cold, and/or critical, with explanations for each classification.

3. **Agent Workbench**
   A workspace where the agent can select context deliberately, edit files, run commands, verify behavior, and leave a compact trail of evidence.

## First Prototype

Start with the Context Temperature Engine.

Given a repository and a task, BRE should produce a context tray:

```text
Active:
- files being edited
- failing tests
- stack-trace files

Watching:
- nearby dependencies
- high-criticality config
- schema and route boundaries

Background:
- summarized docs
- linked ticket acceptance criteria
- known project conventions
```

The prototype does not need to be a full IDE. It needs to prove that BRE can choose better context than "everything recent."

## Benchmark Harness

BRE includes a local benchmark harness for context-selection and fail-closed behavior:

```sh
python3 benchmarks/run.py
```

The harness creates temporary repos from `benchmarks/suite.json`, runs BRE, and compares Active + Watching files against oracle files. The real-repo harness also stress-tests pinned open-source repos such as VS Code and OpenNext Cloudflare. This is the first kill-test: if BRE cannot beat a simple active-file baseline on deterministic context tasks, it is not worth expanding into an agent workbench.

## JavaScript Alias Handoff

BRE accepts resolved JavaScript/TypeScript import aliases through `bre.json`:

```json
{
  "js_aliases": {
    "@/*": ["src/*"],
    "@components": ["src/components"]
  }
}
```

An editor or frontend adapter can also pass a sidecar map without rewriting the task contract:

```sh
bre-context-engine . --js-aliases .bre-js-aliases.json --format json
```

Use the Node hydrator when the JS side should evaluate `tsconfig.json`, `jsconfig.json`, or Vite config and write the flattened map back into `bre.json`:

```sh
node tools/hydrate-js-aliases.mjs .
```

The Rust resolver still has static fallbacks for `tsconfig`, `package.json#imports`, and simple Vite aliases, but this handoff is the preferred path for dynamic executable config.

## Reusable Engine API

The CLI is a thin wrapper around the reusable Rust engine:

```rust
use bre_context_engine::engine::{run_bre, BreRunOptions};

let payload = run_bre(repo_root, config, BreRunOptions {
    max_tokens: Some(60_000),
    skip_diagnostics: false,
    include_report: true,
    js_aliases,
    scoring_policy: None,
})?;
```

This keeps the core usable from an editor extension, background daemon, benchmark harness, or future workbench without shelling out to the binary. The engine returns the same `AgentPayload` shape as the CLI JSON output.

## Model Context Protocol (MCP) Server

BRE contains a built-in stdio-based Model Context Protocol (MCP) server. This allows developers to run BRE as a background daemon and plug it directly into IDEs and LLM clients like Cursor, Claude Desktop, and Windsurf.

To run BRE in MCP mode:
```sh
bre-context-engine --mcp
```

### Server Configuration

You can register the BRE MCP server in your client configuration files (e.g., `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "bre-context-engine": {
      "command": "bre-context-engine",
      "args": ["--mcp"]
    }
  }
}
```

### Exposed Tools

The server exposes the `run_bre` tool:

- **`run_bre`**: Executes the Bounded Reasoning Environment context optimizer on a repository.
  - **Arguments**:
    - `path` (string, required): The absolute path to the repository directory (must contain `bre.json`).
    - `max_tokens` (integer, optional): Override the maximum token limit.
    - `skip_diagnostics` (boolean, optional): Skip running compiler/linter diagnostics.
    - `report` (boolean, optional): Include an evidence report summary.
    - `format` (string, optional): Output format, either `"prompt"` (default) to get formatted LLM instructions, or `"json"` for raw telemetry representation.
    - `js_aliases` (object, optional): Dynamic key-value mappings for JS/TS import aliases.

When `include_report` is `false`, the payload stays lean: task contract, context trays, and diagnostics only. When `include_report` is `true`, the same payload includes a structured `RunReport` with diagnostics, context-budget telemetry, weight feedback status, and file-level decision traces.

## Scoring Policies

BRE scores files from stable signal traces. The default policy is `max`, which preserves the original conservative behavior: one strong structural signal can keep a file hot.

You can opt into additive or p-norm aggregation for experiments and benchmark comparisons:

```sh
bre-context-engine . --scoring-policy additive --report
bre-context-engine . --scoring-policy p-norm --scoring-p 2 --report
```

The same policy can live in `bre.json`:

```json
{
  "scoring_policy": { "mode": "additive" }
}
```

```json
{
  "scoring_policy": { "mode": "p_norm", "p": 2 }
}
```

Reports include the selected policy under `report.context.scoring_policy`. The full MMKP optimizer, dual relaxation, and momentum feedback model remain future work.

## Project-Aware Diagnostics

BRE chooses diagnostics by project shape instead of blindly running the same compiler everywhere:

- Rust repos use `cargo check --message-format=json`.
- Svelte/SvelteKit repos use the package `check` script when present, otherwise local `svelte-check`.
- Plain TypeScript/JavaScript repos use local `tsc --noEmit`, falling back to `bun x tsc --noEmit` if a Bun lockfile is present.

This matters because framework-aware tools understand generated types and component semantics that raw compilers often miss.

## Diagnostic Exclusions

BRE diagnostics feed the adaptive weight loop, so known compiler/tooling noise should be filtered before it can affect learning. Add `ignored_diagnostics` entries to `bre.json` to match file paths, compiler names, or diagnostic message substrings:

```json
{
  "ignored_diagnostics": [
    "src/lib/components/ui/button/index.ts",
    "Module \"*.svelte\" has no exported member"
  ]
}
```

Ignored diagnostics are removed from the payload and never reach the feedback loop.

## Run Reports

Pass `--report` to include an evidence summary in the payload:

```sh
bre-context-engine . --format json --report
```

The report is nested so each subsystem owns its evidence:

```json
{
  "report": {
    "repo_root": "/path/to/repo",
    "diagnostics": {
      "skipped": false,
      "project_types": ["rust", "typescript"],
      "adapters": ["cargo check --message-format=json", "local tsc --noEmit"],
      "ignored_patterns": 1,
      "raw_errors": 3,
      "ignored_errors": 1,
      "emitted_errors": 2
    },
    "context": {
      "scoring_policy": { "mode": "max" },
      "counts": { "active": 2, "watching": 14, "background": 120 },
      "budget_action": "trimmed_to_token_budget",
      "max_tokens": 30000,
      "initial_tokens": 48000,
      "final_tokens": 29600,
      "active_tokens": 8200,
      "skeletonized_files": 3,
      "ejected_files": 8,
      "max_loaded_files": 48,
      "loaded_files": 16
    },
    "weights": {
      "weights_path": "/path/to/repo/bre_weights.json",
      "changed": true,
      "file_exists_after_run": true,
      "feedback_applied": true
    },
    "file_traces": []
  }
}
```

Each file trace explains a final tray decision without mixing report metadata into prompt content:

```json
{
  "path": "src/platform/keybindingResolver.ts",
  "tray": "watching",
  "representation": "full",
  "score": 120,
  "signals": {
    "ast_dependency": true,
    "symbol_score": 110
  }
}
```

`tray` is one of `active`, `watching`, or `background`. `representation` is one of `full`, `skeleton`, or `omitted`. Signal labels used in prompt reports are stable: `ast-import`, `symbol-match`, `manifest`, `git-dirty`, `proximity`, `task-term+ast`, `task-term`, and `critical`.

Background traces are especially important for learning: if an ejected file later appears in diagnostics, BRE boosts only the weights for signals that actually fired for that file. If a diagnostic points at a background file with no trace, BRE treats it as a signal coverage gap instead of blindly inflating all weights.

## Papers

The practical BRE overview remains in `whitepaper.tex`. The formal math framing is a separate artifact at `papers/formal-bounded-context-selection.tex` with its bibliography in `papers/formal-bounded-context-selection.bib`. The formal paper intentionally separates implemented runtime behavior from proposed optimizer work such as MMKP solving, dual relaxation, and momentum feedback.
