#!/usr/bin/env bash
# aztarna installer: build, copy binary, wire Claude Code hooks.
#
# Idempotent: safe to re-run after a code change.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="${AZTARNA_INSTALL_DIR:-$HOME/.local/bin}"
SETTINGS="${CLAUDE_SETTINGS:-$HOME/.claude/settings.json}"

echo "==> building release"
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

# Don't assume $REPO_DIR/target — a global build.target-dir in ~/.cargo/config.toml
# (or CARGO_TARGET_DIR) redirects it elsewhere. Ask cargo where it actually built.
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps \
    --manifest-path "$REPO_DIR/Cargo.toml" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/release/aztarna"
[[ -x "$BIN" ]] || { echo "!! no binary at $BIN after a successful build" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
cp "$BIN" "$INSTALL_DIR/aztarna"
echo "==> installed binary to $INSTALL_DIR/aztarna"

# Ensure the data dir + db exist before any hook fires (warmup).
"$INSTALL_DIR/aztarna" init >/dev/null

# Patch ~/.claude/settings.json — add PostToolUse(Bash) and SessionStart hooks.
# Uses python3 for safe JSON editing; aborts cleanly if Claude Code settings
# are missing.
if [[ ! -f "$SETTINGS" ]]; then
    echo "!! $SETTINGS not found — install the binary alone and wire the hooks by hand."
    exit 0
fi

python3 - "$SETTINGS" "$INSTALL_DIR/aztarna" <<'PY'
import json, sys, pathlib, copy

settings_path = pathlib.Path(sys.argv[1])
bin_path = sys.argv[2]

settings = json.loads(settings_path.read_text())
before = copy.deepcopy(settings)

hooks = settings.setdefault("hooks", {})

def upsert(event, matcher, command):
    """Add a single hook entry to settings.hooks[event] without duplicates."""
    bucket = hooks.setdefault(event, [])
    # Each "matcher group" has shape {"matcher": "...", "hooks": [{type,command,...}]}.
    # Find or create the matcher group.
    group = None
    for g in bucket:
        if g.get("matcher", "") == matcher:
            group = g
            break
    if group is None:
        group = {"matcher": matcher, "hooks": []}
        bucket.append(group)
    # Drop any prior aztarna hooks in this matcher group (idempotent reinstall).
    group["hooks"] = [h for h in group["hooks"] if "aztarna" not in h.get("command", "")]
    group["hooks"].append({"type": "command", "command": command})

# PostToolUse for Bash: pipe stdin JSON to aztarna log.
upsert("PostToolUse", "Bash", f"{bin_path} log")

# SessionStart: emit a context block to additionalContext when there's data.
# `aztarna context` prints empty when no rows exist, so this is harmless on a
# fresh install.
session_cmd = (
    f"{bin_path} context --cwd \"$CLAUDE_PROJECT_DIR\" 2>/dev/null "
    f"| awk 'NF{{f=1}} END{{exit !f}}' "
    f"&& {bin_path} context --cwd \"$CLAUDE_PROJECT_DIR\""
)
upsert("SessionStart", "", session_cmd)

if settings != before:
    settings_path.write_text(json.dumps(settings, indent=2) + "\n")
    print(f"==> patched {settings_path}")
else:
    print(f"==> {settings_path} already up to date")
PY

echo
echo "Done. New Claude Code sessions will:"
echo "  • log every Bash invocation to $($INSTALL_DIR/aztarna --help | head -1)"
echo "  • receive a 'frequently used commands' context block on session start"
echo
echo "Query manually:"
echo "  aztarna top         # most-used commands in the current project"
echo "  aztarna sequences   # common (A -> B) command pairs"
echo "  aztarna context     # exactly what SessionStart would inject"
echo "  aztarna stats       # row count, scopes, etc."
