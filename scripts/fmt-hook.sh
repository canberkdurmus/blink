#!/usr/bin/env bash
# PostToolUse hook: after an Edit/Write of a *.rs file, run `cargo fmt` on the workspace.
# Reads the hook JSON on stdin. Never blocks the tool call.
input=$(cat)
path=$(printf '%s' "$input" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
case "$path" in
  *.rs)
    cd "$(dirname "$0")/.." || exit 0
    cargo fmt --all >/dev/null 2>&1 || true
    ;;
esac
exit 0
