#!/usr/bin/env bash
# Blink autopilot: runs fresh Claude Code sessions until the final gate (or a real blocker).
# Usage: scripts/autopilot.sh            (run inside tmux)
# Optional env: MAX_SESSIONS (default 400), NTFY_TOPIC (push notifications via ntfy.sh)
set -uo pipefail
cd "$(dirname "$0")/.."
mkdir -p .autopilot/logs

# Local, git-ignored settings (e.g. NTFY_TOPIC=...).
[[ -f .autopilot/env ]] && source .autopilot/env
# rustup installs into ~/.cargo without touching shell profiles.
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

# Isolate autopilot sessions from the owner's personal Claude setup:
# no claude.ai connectors (Gmail, Notion, Drive...), no MCP servers, no auto-memory.
# Everything a session needs lives in the repo (CLAUDE.md).
export ENABLE_CLAUDEAI_MCP_SERVERS=false
export CLAUDE_CODE_DISABLE_AUTO_MEMORY=1

# Preflight: fail loudly instead of burning sessions on a broken environment.
preflight_ok=1
for t in claude git gh cargo exiftool; do
  command -v "$t" >/dev/null || { echo "preflight: '$t' not on PATH"; preflight_ok=0; }
done
gh auth status >/dev/null 2>&1 || { echo "preflight: gh is not logged in (run: gh auth login)"; preflight_ok=0; }
git remote get-url origin >/dev/null 2>&1 || { echo "preflight: no 'origin' remote"; preflight_ok=0; }
(( preflight_ok )) || exit 1

MAX_SESSIONS="${MAX_SESSIONS:-400}"
notify() { [[ -n "${NTFY_TOPIC:-}" ]] && curl -s -d "$1" "https://ntfy.sh/${NTFY_TOPIC}" >/dev/null; true; }

GOAL='Advance Blink following CLAUDE.md and SPEC.md, starting from PROGRESS.md (or Bootstrap if the repo has no Cargo.toml). The goal is met when ANY of these is shown in this session: (a) the current milestone gate passed, its report docs/reports/Mn.md is committed and pushed, and PROGRESS.md names the next milestone; (b) .autopilot/DONE exists after the M5 gate; (c) .autopilot/NEEDS_HUMAN.md exists for a blocker only a human can resolve, per the Escalation section of CLAUDE.md; (d) about 250 turns have been used, all work is committed and pushed, and PROGRESS.md has a concrete handoff in "Next up". Constraints: main stays green; no test, golden or baseline weakened without the required trailer.'

fails=0
for ((i = 1; i <= MAX_SESSIONS; i++)); do
  if [[ -f .autopilot/DONE ]]; then echo "Final gate reached."; notify "Blink: final gate reached, v1.0.0-rc ready for you."; exit 0; fi
  if [[ -f .autopilot/NEEDS_HUMAN.md ]]; then echo "Human needed:"; cat .autopilot/NEEDS_HUMAN.md; notify "Blink needs you: $(head -n 3 .autopilot/NEEDS_HUMAN.md)"; exit 1; fi

  reports_before=$(ls docs/reports 2>/dev/null | wc -l)
  ts=$(date -u +%Y%m%dT%H%M%SZ)
  start=$(date +%s)
  # --settings outranks project settings: commits and PRs never credit Claude.
  claude -p "/goal ${GOAL}" --permission-mode auto --strict-mcp-config \
    --settings '{"attribution":{"commit":"","pr":""},"includeCoAuthoredBy":false}' \
    --output-format stream-json --verbose \
    < /dev/null > ".autopilot/logs/${ts}.jsonl" 2>&1
  code=$?
  dur=$(( $(date +%s) - start ))
  echo "${ts} session=${i} exit=${code} duration=${dur}s" | tee -a .autopilot/sessions.log

  reports_after=$(ls docs/reports 2>/dev/null | wc -l)
  (( reports_after > reports_before )) && notify "Blink: new gate report $(ls -t docs/reports | head -n 1)"

  # Very short sessions usually mean a usage limit or an outage: back off (15 min steps, max 2 h).
  if (( dur < 300 )); then
    fails=$(( fails < 8 ? fails + 1 : 8 ))
    sleep $(( fails * 900 ))
  else
    fails=0
    sleep 30
  fi
done
echo "Reached MAX_SESSIONS=${MAX_SESSIONS}."
notify "Blink autopilot stopped after ${MAX_SESSIONS} sessions."
