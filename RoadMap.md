# BRE Engine: Architectural Roadmap

This document outlines the execution sequence for the Bounded Reasoning Environment Engine.

## Phase 1: The Foundation (Current)

**Ticket 1.01: Core Routing logic**

- [x] Establish CLI wrapper using `clap`.
- [x] Parse `bre.json` Task Contract.
- [x] Implement Hot/Warm/Cold logic based on directory proximity.

**Ticket 1.02: Task Contract Upgrades**

- [x] Expand `bre.json` schema to include `assumptions` and `non_goals`.
- [x] Add serialization logic to output a structured prompt payload instead of raw arrays.

## Phase 2: Heuristic Expansion (The "Warm" Tray)

**Ticket 2.01: Git State Integration**

- [x] Integrate `git2` crate.
- [x] Identify untracked or modified files (Git dirty state).
- [x] Automatically route modified files to the `Active` or `Watching` tray regardless of `bre.json`.

**Ticket 2.02: Criticality Scanning**

- [x] Implement a regex-based or extension-based scanner to identify high-risk files globally.
- [x] Route files like `.env`, `Dockerfile`, `nginx.conf`, and CI/CD yaml files to the `Watching` tray automatically.

## Phase 3: Abstract Syntax Tree (AST) Integration

**Ticket 3.01: Dependency Graphing**

- [x] Integrate `tree-sitter` to parse code structure rather than just file paths.
- [x] Identify `import` or `use` statements within `Active` files.
- [x] Route explicitly imported dependencies into the `Watching` tray.

## Phase 4: Token Management

**Ticket 4.01: Context Boundary Enforcement**

- [x] Integrate a fast tokenizer (e.g., `tiktoken-rs`).
- [x] Calculate the token weight of the `Active` and `Watching` trays.
- [x] Set a hard limit (e.g., 30,000 tokens). If the tray exceeds the limit, eject the coldest "Watching" files back to "Background" to prevent LLM token bloat.

## Phase 5: Reusable Engine Core

**Ticket 5.01: Library Runtime Boundary**

- [x] Extract the full run sequence into `run_bre(repo_root, BreConfig, BreRunOptions)`.
- [x] Keep the CLI as a thin wrapper around the reusable engine.
- [x] Return a stable `AgentPayload` for CLIs, IDE extensions, daemons, and benchmark harnesses.

**Ticket 5.02: Evidence Layer**

- [x] Add optional run reports with diagnostics, context counts, token budget actions, and feedback status.
- [x] Add per-file reason traces with score, tray, representation, and contributing signals.
- [x] Add stable report snapshots for VS Code and OpenNext Cloudflare stress tasks.

## Phase 6: Mathematical Core

**Ticket 6.01: Formal Context Optimization**

- [x] Document BRE as a constrained utility maximization problem over file inclusion, skeletonization, token budget, and loaded-file budget.
- [x] Document adaptive weights as a projected feedback update with decay toward baselines.
- [x] Implement a first-class signal vector per file and expose it through structured run reports.
- [x] Expose scoring policies so the engine can compare max-norm, p-norm, and additive utility aggregators under the same benchmark suite.
- [x] Add a real-repo scoring policy comparison runner for max, additive, and p-norm trials.
- [x] Extend real-repo agent dry-runs so prompts can compare baseline vs BRE under all scoring policies.
- [x] Implement MMKP-style representation selection with token and loaded-file budgets.
- [x] Add momentum-backed projected feedback after benchmark evidence justifies the extra state.

**Ticket 6.02: Outcome Benchmarks**

- [x] Run baseline-context vs BRE-context agent trials on real repos.
- [x] Score patch correctness, verification pass rate, changed-file precision, token cost, wall time, and false-success rate.

## Phase 7: Pluggable Ecosystem Integration

**Ticket 7.01: Model Context Protocol (MCP) Integration**

- [x] Implement stdio-based Model Context Protocol (MCP) server daemon.
- [x] Add `--mcp` CLI argument flag in `src/config.rs` and handle it in `src/main.rs`.
- [x] Expose the `run_bre` tool to MCP clients with dynamic workspace mapping and option overrides.
- [x] Add comprehensive unit and integration tests for JSON-RPC 2.0 parser validation.
- [x] Document configuration schema for desktop applications (Claude Desktop, Cursor, etc.).
