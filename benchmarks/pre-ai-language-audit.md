# Pre-AI Language Audit

Use `2020-12-31` as BRE's clean pre-AI benchmark snapshot.

Why:

- It predates GitHub Copilot's 2021 technical preview.
- It predates Copilot general availability in 2022.
- It predates ChatGPT's public launch on November 30, 2022.

Use `2023-01-01` as a transition snapshot, not as the clean baseline. By 2023,
AI coding tools were entering mainstream developer workflows, so it is useful
for comparison but less useful as a "mostly human-written repo history" anchor.

## Current Active Suite

Command:

```sh
python3 benchmarks/audit_repo_languages.py --date 2020-12-31
```

Result:

| Repo | 2020 Snapshot | Dominant Languages By File Count |
| --- | --- | --- |
| `ggez/ggez` | `595dd97` | Rust 51%, Other 30%, Markdown 13%, YAML 3%, TOML 3% |
| `not-fl3/macroquad` | `b39616f` | Rust 69%, Other 18%, TOML 7%, YAML 2%, Markdown 2% |
| `avianphysics/avian` | no commit before date | Not available for 2020 baseline |
| `17cupsofcoffee/tetra` | `c4ba4ca` | Rust 64%, Other 25%, Markdown 6%, YAML 4%, TOML 1% |
| `boardgameio/boardgame.io` | `32a7714` | JavaScript 31%, TypeScript 28%, Markdown 9%, Svelte 9%, Other 8% |
| `valence-rs/valence` | no commit before date | Not available for 2020 baseline |
| `microsoft/vscode` | `98106c48a07` | TypeScript 64%, JSON 12%, Other 11%, JavaScript 5%, CSS 4% |

Command:

```sh
python3 benchmarks/audit_repo_languages.py --date 2023-01-01
```

Result:

| Repo | 2023 Snapshot | Dominant Languages By File Count |
| --- | --- | --- |
| `ggez/ggez` | `921ab3c` | Rust 51%, Other 32%, Markdown 13%, TOML 3%, YAML 1% |
| `not-fl3/macroquad` | `318536e` | Rust 65%, Other 26%, TOML 4%, Markdown 3%, YAML 1% |
| `avianphysics/avian` | `54bdd56` | Rust 75%, TOML 14%, Other 8%, Markdown 3% |
| `17cupsofcoffee/tetra` | `a872e79` | Rust 64%, Other 26%, YAML 6%, Markdown 3%, TOML 1% |
| `boardgameio/boardgame.io` | `85df91b` | TypeScript 31%, JavaScript 29%, Markdown 10%, Svelte 8%, Other 7% |
| `valence-rs/valence` | `7574fa3` | Rust 64%, Other 9%, Java 8%, TOML 7%, JSON 5% |
| `microsoft/vscode` | `40b80995a90` | TypeScript 64%, Other 11%, JSON 11%, JavaScript 4%, CSS 4% |

## Language Implication

The current real-repo suite mostly exercises:

- Rust
- TypeScript
- JavaScript
- Config surfaces such as JSON, TOML, YAML, Markdown, CSS, and Docker-style files

That matches BRE's current strongest implementation areas. The next parser
upgrade should be true TypeScript/TSX support via `tree-sitter-typescript`,
because VS Code and boardgame.io are TypeScript-heavy and currently share the
JavaScript parser path.
