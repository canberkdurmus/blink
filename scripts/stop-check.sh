#!/usr/bin/env bash
# Stop hook: block ending the session (exit 2, message on stderr) when tracked files have
# uncommitted changes and PROGRESS.md has no handoff note newer than the last commit.
cd "$(dirname "$0")/.." || exit 0
# Tracked files with uncommitted changes (staged or not). Untracked files don't count.
if git diff --quiet HEAD -- 2>/dev/null; then
  exit 0
fi
last_commit=$(git log -1 --format=%ct 2>/dev/null || echo 0)
progress_mtime=$(stat -c %Y PROGRESS.md 2>/dev/null || stat -f %m PROGRESS.md 2>/dev/null || echo 0)
# A handoff counts only if PROGRESS.md itself was edited after the last commit.
if (( progress_mtime > last_commit )) && ! git diff --quiet HEAD -- PROGRESS.md; then
  exit 0
fi
{
  echo "Uncommitted changes to tracked files:"
  git diff --stat HEAD -- | tail -n 20
  echo "Commit and push your work, or write a handoff in PROGRESS.md (\"Next up\" + session log) before stopping."
} >&2
exit 2
