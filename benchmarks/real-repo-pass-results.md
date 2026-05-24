# Real Repo Pass Results

This file records live BRE trials against real gaming/game-engine and editor-scale repositories.
The point is to find engine failures, not to make BRE look good.

## 1. ggez/ggez

- Repo: https://github.com/ggez/ggez
- Task: improve keyboard repeat / pressed-key behavior from `examples/input_test.rs`.
- Active file: `examples/input_test.rs`
- Expected important context: `src/input/keyboard.rs`, `src/event.rs`, `src/context.rs` or related event/input plumbing, `Cargo.toml`, CI workflow.

Initial result:

- BRE loaded almost every sibling file in `examples/`.
- BRE missed core keyboard/event files.
- Verdict: fail.

Engine fixes made:

- Resolve imports from examples back to the root Rust crate name, including crate names where package hyphens become underscores.
- Do not treat broad directories like `examples/`, `tests/`, `docs/`, `resources/`, or fixtures as proximity context.
- Require exact filesystem casing during dependency resolution, so type names like `Context` do not become fake paths like `src/Context.rs` on case-insensitive filesystems.

After fixes, BRE selected:

```text
Active:
- examples/input_test.rs

Watching:
- .github/workflows/rust.yml
- Cargo.toml
- src/conf.rs
- src/event.rs
- src/graphics/mod.rs
- src/input/keyboard.rs
```

Verdict: pass, with minor noise. `src/graphics/mod.rs` comes from the example's drawing code and is reasonable.

## 2. not-fl3/macroquad

- Repo: https://github.com/not-fl3/macroquad
- Task: improve arrow-key movement behavior from `examples/input_keys.rs`.
- Active file: `examples/input_keys.rs`
- Expected important context: `src/input.rs`, `src/prelude.rs`, `Cargo.toml`, CI workflow.

Initial result:

- BRE selected only `.github/workflows/rust.yml` and `Cargo.toml`.
- It missed `src/input.rs` because the example uses `use macroquad::prelude::*`.
- Verdict: fail.

Engine fixes made:

- Treat Rust wildcard imports like `crate::module::*` as a dependency on `crate::module`.
- When a first-order dependency is a `prelude.rs`, expand one additional dependency hop to catch re-exported modules.
- Parse `pub use ...` declarations correctly, not only bare `use ...`.

After fixes, BRE selected:

```text
Active:
- examples/input_keys.rs

Watching:
- .github/workflows/rust.yml
- Cargo.toml
- src/camera.rs
- src/color.rs
- src/experimental.rs
- src/file.rs
- src/input.rs
- src/material.rs
- src/math.rs
- src/models.rs
- src/prelude.rs
- src/quad_gl.rs
- src/shapes.rs
- src/text.rs
- src/texture.rs
- src/time.rs
- src/window.rs
```

Verdict: partial pass.

It finds `src/input.rs`, which is the required win. But it pulls many prelude re-export modules. This is a real weakness: BRE needs symbol-aware prelude resolution so `is_key_down` and `KeyCode` map to `src/input.rs` without loading every prelude export.

## 3. avianphysics/avian

- Repo: https://github.com/avianphysics/avian
- Task: change collision layer behavior from `crates/avian2d/examples/collision_layers.rs`.
- Active file: `crates/avian2d/examples/collision_layers.rs`
- Expected important context: `src/collision/collider/layers.rs`, `crates/avian2d/Cargo.toml`, root `Cargo.toml`, CI workflow, plus nearby physics/math context if budget allows.

Initial result:

- BRE selected only `.github/workflows/ci.yml` and `Cargo.toml`.
- It missed `src/collision/collider/layers.rs`, the file that defines `PhysicsLayer`, `LayerMask`, and `CollisionLayers`.
- The miss happened because `avian2d` is a workspace crate whose `[lib]` points to `../../src/lib.rs`; BRE assumed the crate source lived under `crates/avian2d/src`.
- Verdict: fail.

Engine fixes made:

- Honor Cargo `[lib] path` when resolving workspace crate source roots, and normalize paths containing `..`.
- Add a symbol-definition fallback so task-visible symbols such as `CollisionLayers` and `LayerMask` can pull the files that define them even when the import goes through a prelude/glob.
- Weight files higher when they define multiple task-visible symbols.
- Tighten symbol fallback so Rust files are not matched with JavaScript `let` rules, Rust lifetimes like `&'static T` are not mistaken for `static T`, and method calls like `Transform::from_xyz` are not treated as symbols to chase.
- Backfill small useful Watching files after a large ejection leaves room under the token budget.
- Always include the nearest manifest for Active files, so `crates/avian2d/Cargo.toml` is available for examples inside that crate.

After fixes, BRE selected:

```text
Active:
- crates/avian2d/examples/collision_layers.rs

Watching:
- .github/workflows/ci.yml
- Cargo.toml
- crates/avian2d/Cargo.toml
- src/collision/collider/layers.rs
- src/dynamics/rigid_body/mod.rs
- src/math/mod.rs
```

Verdict: pass, with acceptable noise.

It now finds the core collision layer implementation and the crate manifest that explains the shared source layout. `src/dynamics/rigid_body/mod.rs` and `src/math/mod.rs` come from repeated active-file API usage (`RigidBody`, `Collider`, and `math::*`), so they are not random, but they are secondary for this task.

## Engine Lessons So Far

- Examples are not normal source directories. Directory proximity is dangerous there.
- Rust root-crate resolution is required for real examples.
- Case-insensitive development filesystems can create false dependency paths unless resolution checks exact casing.
- Prelude/glob imports are common in game frameworks and need symbol-aware handling.
- Workspace crates can point their library at shared source outside the crate directory; resolving only `crate/src` is not enough.
- Symbol fallback is necessary for prelude-heavy game APIs, but it must be language-aware and definition-aware or it will chase local variables and method calls.
- Token pruning needs a backfill step; otherwise one large ejection can strand small useful manifests in Background even when budget remains.
- Nearest active manifests matter more than generic root manifests for workspace examples.

## 4. Real-Repo Stress Suite

Command:

```sh
python3 benchmarks/run_real_repos.py
```

Run date: 2026-05-21. Diagnostics were skipped; this run measures context selection.

| Task | Repo commit | Required recall | Loaded files | Result |
| --- | --- | ---: | ---: | --- |
| `ggez_input_keyboard` | `ggez/ggez@64165d8` | 1.00 | 11 | pass |
| `macroquad_prelude_input` | `not-fl3/macroquad@a9b1e7e` | 1.00 | 6 | pass |
| `avian_collision_layers` | `avianphysics/avian@85de616` | 1.00 | 8 | pass |
| `tetra_keyboard_input` | `17cupsofcoffee/tetra@d769782` | 1.00 | 10 | pass |
| `boardgameio_core_reducer` | `boardgameio/boardgame.io@4f3c90d` | 1.00 | 24 | pass |

Stress fixes made during the run:

- Added a real-repo stress harness that shallow-clones open-source repos, injects `bre.json`, and scores Active + Watching context.
- Added `--skip-diagnostics` to BRE so context-selection stress runs do not compile cloned repos unless explicitly requested.
- Made prelude/glob expansion prefer task-visible symbols before falling back to active-file symbols.
- Stopped symbol fallback from chasing symbols defined inside the Active files, which prevented unrelated examples with local names like `MainState` from becoming Watching context.
- Normalized `..` in JS/TS dependency paths so imports like `../types` do not appear as duplicate paths such as `src/core/../types.ts`.
- Cooled unrelated test files unless the task asks for tests or the test file matches the active source file.
- Treated root `.env` files as globally critical while leaving nested demo/integration `.env` files out of global Watching context.

Remaining noise:

- TypeScript source directories are still warm enough to pull broad same-directory support files. The boardgame.io reducer task passes its budget, but `src/ai/*` and `src/server/api.ts` show that symbol fallback can still over-match exported names in larger TS repos.

## 5. Valence Command Stress Add-On

Command:

```sh
python3 benchmarks/run_real_repos.py --task valence_command_graph
```

Run date: 2026-05-21. Diagnostics were skipped; this run measures context selection.

| Task | Repo commit | Required recall | Loaded files | Result |
| --- | --- | ---: | ---: | --- |
| `valence_command_graph` | `valence-rs/valence@2b70535` | 1.00 | 21 | pass |

## 6. VS Code Fork Stress Add-On

Command:

```sh
python3 benchmarks/run_real_repos.py --task vscode_keybinding_resolver --bre-timeout 300
```

Run date: 2026-05-21. Diagnostics were skipped; this run measures context selection on an editor-scale TypeScript monorepo.

| Task | Repo commit | Required recall | BRE recall | Loaded files | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `vscode_keybinding_resolver` | `microsoft/vscode@a74acf03` | 0.80 | 0.90 | 36 | pass |

The initial VS Code run exposed two different large-repo failures:

- BRE spent most of the run tokenizing and tree-sitter parsing hundreds of large Watching candidates before deciding what to keep.
- Once performance was acceptable, high-value direct imports such as `keybindingResolver.ts` competed with broad symbol/path matches from fixtures, workbench actions, and test support files.

Engine fixes made:

- Use task-scoped symbol scans on very large repositories so symbol fallback does not read every source file when task path terms can narrow the search.
- Pre-skeletonize large Watching source files before token budgeting, using a fast declaration-line skeleton for very large files and AST skeletonization for smaller files.
- Use cheap token estimates while ranking/ejecting candidates, then exact-tokenize only the final kept set.
- Run a final exact backfill pass for high-relevance files after exact token counts are known.
- Boost task-matching direct AST dependencies above path-only matches, so imported keybinding/keyboard platform files beat unrelated keybinding-ish files.
- Skip symbol hits and critical-file warming from unrequested `test`, `tests`, `fixtures`, `examples`, and similar broad context directories.
- Trim the final context by file count after token fitting, keeping the highest-scoring files when spare tokens would otherwise admit too many low-value files.

Full stress suite after this add-on:

| Task | Repo commit | Required recall | BRE recall | Loaded files | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `ggez_input_keyboard` | `ggez/ggez@64165d8` | 1.00 | 1.00 | 12 | pass |
| `macroquad_prelude_input` | `not-fl3/macroquad@a9b1e7e` | 1.00 | 1.00 | 6 | pass |
| `avian_collision_layers` | `avianphysics/avian@85de616` | 1.00 | 1.00 | 10 | pass |
| `tetra_keyboard_input` | `17cupsofcoffee/tetra@d769782` | 1.00 | 1.00 | 11 | pass |
| `boardgameio_core_reducer` | `boardgameio/boardgame.io@4f3c90d` | 1.00 | 1.00 | 20 | pass |
| `valence_command_graph` | `valence-rs/valence@2b70535` | 1.00 | 1.00 | 20 | pass |
| `vscode_keybinding_resolver` | `microsoft/vscode@a74acf03` | 0.80 | 0.90 | 36 | pass |

## 7. VS Code Real-Repo Agent Prompt Audit

Command:

```sh
python3 benchmarks/run_real_repo_agent.py --dry-run --task vscode_keybinding_resolver --bre-timeout 300
```

Run date: 2026-05-22. Diagnostics were skipped; this run measures the prompt
surface that a real coding agent would receive before patching.

| Task | Mode | Context recall | Context files | Context bytes | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `vscode_keybinding_resolver` | `baseline` | 0.00 | 1 | 38,447 | pass |
| `vscode_keybinding_resolver` | `bre` | 0.90 | 36 | 202,544 | pass |

The baseline prompt only contains the active
`WorkbenchKeybindingService` file. The BRE prompt includes the platform
keybinding resolver, registry/item models, keyboard layout contracts, and
workbench keybinding support files needed for a real agent run.

The initial Valence run failed with 0.86 recall. BRE found the command graph,
handler, manager, parsers, and scopes, but missed
`crates/valence_command_macros/src/lib.rs`. The example imports
`command_macros::Command`; in Valence this is exposed through a root crate
re-export alias:

```rust
pub use valence_command_macros as command_macros;
```

Engine fix made:

- Resolve Rust workspace crate aliases declared as `pub use crate_name as alias;`.
- When an import points at a root-level symbol such as `command_macros::Command`, fall back to the target crate's `lib.rs`.

Full stress suite after this add-on:

| Task | Repo commit | Required recall | Loaded files | Result |
| --- | --- | ---: | ---: | --- |
| `ggez_input_keyboard` | `ggez/ggez@64165d8` | 1.00 | 12 | pass |
| `macroquad_prelude_input` | `not-fl3/macroquad@a9b1e7e` | 1.00 | 6 | pass |
| `avian_collision_layers` | `avianphysics/avian@85de616` | 1.00 | 9 | pass |
| `tetra_keyboard_input` | `17cupsofcoffee/tetra@d769782` | 1.00 | 11 | pass |
| `boardgameio_core_reducer` | `boardgameio/boardgame.io@4f3c90d` | 1.00 | 24 | pass |
| `valence_command_graph` | `valence-rs/valence@2b70535` | 1.00 | 21 | pass |

## 8. OpenNext Cloudflare Adapter Stress Add-On

Command:

```sh
python3 benchmarks/run_real_repos.py --task opennext_cloudflare_build_pipeline --bre-timeout 300
```

Run date: 2026-05-22. Diagnostics were skipped; this run measures context
selection on a framework-adapter TypeScript monorepo that transforms Next.js
build output for Cloudflare Workers.

| Task | Repo commit | Required recall | BRE recall | Loaded files | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `opennext_cloudflare_build_pipeline` | `opennextjs/opennextjs-cloudflare@596f924` | 1.00 | 1.00 | 26 | pass |

The first OpenNext run exposed two useful precision issues:

- Generated Worker type files under `create-cloudflare/next` were warmed because
  the task used the domain word `Cloudflare`.
- The run report labeled large-file pre-skeletonization as loaded-file-cap
  trimming even though no file had actually been ejected.

Engine fixes made:

- Treat scaffold/template/demo roots such as `create-cloudflare`, `templates`,
  `demo`, `demos`, `sample`, and `samples` as broad context directories unless
  directly active or structurally imported.
- Split the run report budget action so it can distinguish token trimming,
  loaded-file-cap trimming, ordinary file-budget adjustment, and large-file
  pre-skeletonization.

Full stress suite after this add-on:

| Task | Repo commit | Required recall | BRE recall | Loaded files | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `ggez_input_keyboard` | `ggez/ggez@64165d8` | 1.00 | 1.00 | 12 | pass |
| `macroquad_prelude_input` | `not-fl3/macroquad@a9b1e7e` | 1.00 | 1.00 | 6 | pass |
| `avian_collision_layers` | `avianphysics/avian@85de616` | 1.00 | 1.00 | 10 | pass |
| `tetra_keyboard_input` | `17cupsofcoffee/tetra@d769782` | 1.00 | 1.00 | 11 | pass |
| `boardgameio_core_reducer` | `boardgameio/boardgame.io@4f3c90d` | 1.00 | 1.00 | 20 | pass |
| `valence_command_graph` | `valence-rs/valence@2b70535` | 1.00 | 1.00 | 20 | pass |
| `vscode_keybinding_resolver` | `microsoft/vscode@a74acf03` | 0.80 | 0.90 | 36 | pass |
| `opennext_cloudflare_build_pipeline` | `opennextjs/opennextjs-cloudflare@596f924` | 1.00 | 1.00 | 26 | pass |

## 9. Scoring Policy Matrix

Command:

```sh
python3 benchmarks/run_scoring_policies.py --bre-timeout 300
```

Run date: 2026-05-23. Diagnostics were skipped; this run compares the
implemented max, additive, and p-norm scoring policies while holding the same
real-repo task contracts, token budgets, and loaded-file caps.

| Task | Policy | Recall | Precision | Loaded files | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `ggez_input_keyboard` | `max` | 1.00 | 0.17 | 12 | pass |
| `ggez_input_keyboard` | `additive` | 1.00 | 0.17 | 12 | pass |
| `ggez_input_keyboard` | `p-norm:2` | 1.00 | 0.17 | 12 | pass |
| `macroquad_prelude_input` | `max` | 1.00 | 0.33 | 6 | pass |
| `macroquad_prelude_input` | `additive` | 1.00 | 0.33 | 6 | pass |
| `macroquad_prelude_input` | `p-norm:2` | 1.00 | 0.33 | 6 | pass |
| `avian_collision_layers` | `max` | 1.00 | 0.20 | 10 | pass |
| `avian_collision_layers` | `additive` | 1.00 | 0.20 | 10 | pass |
| `avian_collision_layers` | `p-norm:2` | 1.00 | 0.20 | 10 | pass |
| `tetra_keyboard_input` | `max` | 1.00 | 0.18 | 11 | pass |
| `tetra_keyboard_input` | `additive` | 1.00 | 0.18 | 11 | pass |
| `tetra_keyboard_input` | `p-norm:2` | 1.00 | 0.18 | 11 | pass |
| `boardgameio_core_reducer` | `max` | 1.00 | 0.30 | 20 | pass |
| `boardgameio_core_reducer` | `additive` | 1.00 | 0.30 | 20 | pass |
| `boardgameio_core_reducer` | `p-norm:2` | 1.00 | 0.30 | 20 | pass |
| `valence_command_graph` | `max` | 1.00 | 0.35 | 20 | pass |
| `valence_command_graph` | `additive` | 1.00 | 0.35 | 20 | pass |
| `valence_command_graph` | `p-norm:2` | 1.00 | 0.35 | 20 | pass |
| `vscode_keybinding_resolver` | `max` | 0.90 | 0.25 | 36 | pass |
| `vscode_keybinding_resolver` | `additive` | 0.90 | 0.26 | 34 | pass |
| `vscode_keybinding_resolver` | `p-norm:2` | 0.90 | 0.26 | 34 | pass |
| `opennext_cloudflare_build_pipeline` | `max` | 1.00 | 0.54 | 26 | pass |
| `opennext_cloudflare_build_pipeline` | `additive` | 1.00 | 0.54 | 26 | pass |
| `opennext_cloudflare_build_pipeline` | `p-norm:2` | 1.00 | 0.54 | 26 | pass |

Summary: all 24 policy runs passed. Additive and p-norm preserved recall on
every task. They only changed the selected file count on the VS Code stress
task, where both reduced the tray from 36 to 34 files while keeping recall at
0.90. The default remains `max` until agent-outcome trials prove that the
slightly tighter VS Code tray improves patch correctness or verification pass
rate.

## 10. Real-Repo Agent Prompt Policy Matrix

Command:

```sh
python3 benchmarks/run_real_repo_agent.py \
  --dry-run \
  --scoring-policy all \
  --bre-timeout 300
```

Run date: 2026-05-23. This is a dry-run prompt/context audit, not a live patch
trial. The harness created disposable git worktrees and generated the actual
agent prompts for baseline and BRE contexts under each scoring policy.

| Task | Mode | Policy | Context recall | Context precision | Context files | Context bytes | Result |
| --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| `ggez_input_keyboard` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 5,854 | pass |
| `ggez_input_keyboard` | `bre` | `max` | 1.00 | 0.17 | 12 | 105,101 | pass |
| `ggez_input_keyboard` | `bre` | `additive` | 1.00 | 0.17 | 12 | 105,101 | pass |
| `ggez_input_keyboard` | `bre` | `p-norm:2` | 1.00 | 0.17 | 12 | 105,101 | pass |
| `macroquad_prelude_input` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 649 | pass |
| `macroquad_prelude_input` | `bre` | `max` | 1.00 | 0.33 | 6 | 12,998 | pass |
| `macroquad_prelude_input` | `bre` | `additive` | 1.00 | 0.33 | 6 | 12,998 | pass |
| `macroquad_prelude_input` | `bre` | `p-norm:2` | 1.00 | 0.33 | 6 | 12,998 | pass |
| `avian_collision_layers` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 3,389 | pass |
| `avian_collision_layers` | `bre` | `max` | 1.00 | 0.20 | 10 | 76,189 | pass |
| `avian_collision_layers` | `bre` | `additive` | 1.00 | 0.20 | 10 | 76,189 | pass |
| `avian_collision_layers` | `bre` | `p-norm:2` | 1.00 | 0.20 | 10 | 76,189 | pass |
| `tetra_keyboard_input` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 1,972 | pass |
| `tetra_keyboard_input` | `bre` | `max` | 1.00 | 0.18 | 11 | 70,754 | pass |
| `tetra_keyboard_input` | `bre` | `additive` | 1.00 | 0.18 | 11 | 70,754 | pass |
| `tetra_keyboard_input` | `bre` | `p-norm:2` | 1.00 | 0.18 | 11 | 70,754 | pass |
| `boardgameio_core_reducer` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 15,173 | pass |
| `boardgameio_core_reducer` | `bre` | `max` | 1.00 | 0.30 | 20 | 106,003 | pass |
| `boardgameio_core_reducer` | `bre` | `additive` | 1.00 | 0.30 | 20 | 106,831 | pass |
| `boardgameio_core_reducer` | `bre` | `p-norm:2` | 1.00 | 0.30 | 20 | 107,368 | pass |
| `valence_command_graph` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 24,811 | pass |
| `valence_command_graph` | `bre` | `max` | 1.00 | 0.35 | 20 | 117,122 | pass |
| `valence_command_graph` | `bre` | `additive` | 1.00 | 0.35 | 20 | 117,122 | pass |
| `valence_command_graph` | `bre` | `p-norm:2` | 1.00 | 0.35 | 20 | 117,122 | pass |
| `vscode_keybinding_resolver` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 38,447 | pass |
| `vscode_keybinding_resolver` | `bre` | `max` | 0.90 | 0.25 | 36 | 202,544 | pass |
| `vscode_keybinding_resolver` | `bre` | `additive` | 0.90 | 0.26 | 34 | 266,728 | pass |
| `vscode_keybinding_resolver` | `bre` | `p-norm:2` | 0.90 | 0.26 | 34 | 266,728 | pass |
| `opennext_cloudflare_build_pipeline` | `baseline` | `n/a` | 0.00 | 0.00 | 1 | 4,489 | pass |
| `opennext_cloudflare_build_pipeline` | `bre` | `max` | 1.00 | 0.54 | 26 | 87,935 | pass |
| `opennext_cloudflare_build_pipeline` | `bre` | `additive` | 1.00 | 0.54 | 26 | 87,935 | pass |
| `opennext_cloudflare_build_pipeline` | `bre` | `p-norm:2` | 1.00 | 0.54 | 26 | 87,935 | pass |

Summary: all 32 prompt-context runs passed. The dry-run harness now supports
the same scoring policy matrix as the context-selection benchmark, so the next
step can run real agents against identical worktree/prompt generation.
