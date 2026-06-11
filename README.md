# aztarna

> Basque *aztarna* — "trace, footprint."

A tiny Rust CLI + SQLite database that observes every Bash command Claude Code
runs and ranks them per-project, with exponential decay and sequence
detection. New sessions get a "frequently-used commands here" context block
injected automatically.

## Why

Two recurring problems with coding agents:

1. **Permission-prompt churn.** Every new session re-prompts for the same
   commands (`npm test`, `cargo build`, ...). The `fewer-permission-prompts`
   skill mines transcripts reactively; aztarna logs them as they happen, so
   the data is always current.
2. **Lost context.** The model doesn't know that in this project we use `just
   test` instead of `cargo test`, or that `make migrate` always follows
   `cargo sqlx prepare`. Surfacing the actual habit pattern is cheap and
   useful.

## How it works

```
Claude Code → PostToolUse hook → `aztarna log` (stdin JSON) → sqlite
                                                                 ↓
                                  SessionStart hook ← `aztarna context`
```

- **Storage**: one SQLite db per user at
  `~/Library/Application Support/dev.eneko.aztarna/aztarna.sqlite` (macOS) or
  the XDG equivalent on Linux.
- **Ranking**: `score = Σ exp(-age_days · ln(2) / half_life_days)`. Default
  half-life is 14 days — a daily habit dominates ranking for ~a month before
  fading.
- **Scope**: by default, the git root of the current cwd. So a session in
  `~/eneko_projects/aztarna/src/` ranks against the whole repo, not just
  that subdir.
- **Sequences**: `LAG()` window over `commands` partitioned by `session_id`
  surfaces the most common `prev_head -> next_head` adjacency. Self-pairs
  filtered.

## Install

```bash
git clone <this repo> ~/eneko_projects/aztarna
cd ~/eneko_projects/aztarna
./install.sh
```

The installer is idempotent:

- builds `target/release/aztarna`
- copies it to `~/.local/bin/aztarna`
- patches `~/.claude/settings.json` to add a `PostToolUse(Bash)` hook and a
  `SessionStart` hook that emits the context block when there's data

To uninstall, delete the binary, drop the `hooks` entries from
`~/.claude/settings.json`, and remove the data dir.

## CLI

```
aztarna log               # ingest one PostToolUse payload from stdin (hook use)
aztarna top               # ranked commands for the current project
aztarna sequences         # common (A -> B) command pairs
aztarna context           # markdown block injected into SessionStart
aztarna stats             # row count, distinct scopes, etc.
aztarna init              # explicitly create the db (also auto-created)
```

Flags worth knowing:
- `--cwd <path>` query as if you were in `<path>`
- `--scope <path>` override the scope directly, skipping git-root resolution
- `--half-life-days <n>` change the decay constant for one query
- `--json` machine-readable output for `top`, `sequences`, `stats`
- `--db <path>` use a different sqlite file (handy for tests)

## Roadmap

- [ ] Argument-level fingerprinting (group `cargo test --release` and
  `cargo test --no-default-features` separately when the divergence is
  consistent enough to matter)
- [ ] Per-failure ranking so commands that always succeed surface above
  commands that always fail
- [ ] Export to `fewer-permission-prompts`-style allowlist
- [ ] Cross-project view: "what do I run in every Rust repo I touch?"

## Tests

```
cargo test          # 19 tests: 7 unit + 12 integration
```

Integration tests fabricate hook payloads against a temp SQLite, exercising
ingest, decay math, scope isolation, session boundary handling, and the
context block formatter.

## License

MIT.
