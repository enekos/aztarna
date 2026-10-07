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
- **Ranking**: `score = Σ exp(-age_days · ln(2) / half_life_days) ·
  success_rate`. Default half-life is 14 days — a daily habit dominates
  ranking for ~a month before fading. `success_rate` is the fraction of runs
  that exited 0 (unknown exit codes count as success), so commands that
  chronically fail sink instead of being recommended.
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

### Linux packages

```bash
yay -S aztarna                                     # Arch, from the AUR
sudo apt install ./aztarna_<version>_amd64.deb       # Debian, Ubuntu
sudo dnf install ./aztarna-<version>-1.x86_64.rpm    # Fedora, RHEL
sudo apk add --allow-untrusted ./aztarna_<version>_x86_64.apk   # Alpine
nix run github:enekos/aztarna                     # Nix
```

The `.deb`, `.rpm` and `.apk` files are on each [release](https://github.com/enekos/aztarna/releases), for x86_64 and arm64.

These install the binary only; add the two hooks to `~/.claude/settings.json` as `install.sh` does.

## CLI

```
aztarna log               # ingest one PostToolUse payload from stdin (hook use)
aztarna top               # ranked commands for the current project
aztarna top --query test  # filter to commands containing "test"
aztarna top --success-only  # only commands that exited successfully
aztarna top --failing       # rank by decay-weighted failures instead
aztarna top --global        # aggregate across every project ("what do I run everywhere?")
aztarna sequences         # common (A -> B) command pairs
aztarna allowlist         # permissions.allow JSON for this project's top commands
aztarna context           # markdown block injected into SessionStart
aztarna stats             # row count, distinct scopes, etc.
aztarna init              # explicitly create the db (also auto-created)
```

Flags worth knowing:
- `--cwd <path>` query as if you were in `<path>`
- `--scope <path>` override the scope directly, skipping git-root resolution
- `--half-life-days <n>` change the decay constant for one query
- `--query <term>` filter `top` results to commands containing `<term>` (case-insensitive)
- `--success-only` only include commands with exit code 0
- `--json` machine-readable output for `top`, `sequences`, `stats`
- `--db <path>` use a different sqlite file (handy for tests)

### Closing the permission-prompt loop

`aztarna allowlist` turns this project's top-ranked commands into Claude Code
`permissions.allow` entries on stdout:

```json
{
  "permissions": {
    "allow": ["Bash(cargo test:*)", "Bash(cargo clippy:*)"]
  }
}
```

The `:*` suffix is Claude Code's prefix-match form (equivalent to a trailing
` *` wildcard), so one rule covers any arguments to that command head. Merge
the output into `.claude/settings.json` (shared with the team) or
`.claude/settings.local.json` (just you). Ranking is success-weighted, so
chronically failing commands demote themselves out of the list, commands
that have *never* succeeded are excluded outright, and commands Claude Code
treats as read-only anyway (`ls`, `git status`, ...) are skipped.
Flags: `-n` for count (default 15), `--min-score` to drop low scorers.

## Roadmap

- [x] Per-failure ranking so commands that always succeed surface above
  commands that always fail (`top` weights by success rate; `top --failing`
  shows the chronic failures)
- [x] Export to `fewer-permission-prompts`-style allowlist
  (`aztarna allowlist`)
- [x] Cross-project view: "what do I run in every Rust repo I touch?"
  (`aztarna top --global`, with a per-head project count)
- [ ] Argument-level fingerprinting (group `cargo test --release` and
  `cargo test --no-default-features` separately when the divergence is
  consistent enough to matter)

## Tests

```
cargo test          # 31 tests: 7 unit + 24 integration
```

Integration tests fabricate hook payloads against a temp SQLite, exercising
ingest, decay math, scope isolation, session boundary handling, and the
context block formatter.

## License

MIT.
