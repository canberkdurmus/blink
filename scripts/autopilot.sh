#!/usr/bin/env bash
# Blink autopilot: runs fresh Claude Code sessions until the final gate (or a real blocker).
# Usage: scripts/autopilot.sh            (run inside tmux)
# Optional env (or .autopilot/env):
#   MAX_SESSIONS   default 400
#   NTFY_TOPIC     push notifications via ntfy.sh
#   FIVE_HOUR_CAP  % of the 5-hour usage window the autopilot may use (default 80)
#   WEEKLY_CAP     % of the weekly usage window the autopilot may use (default 80)
#   STALL_MIN      kill a session whose log is silent this many minutes (default 90)
set -uo pipefail
cd "${BLINK_REPO:-$(dirname "$0")/..}"   # BLINK_REPO lets a copy outside the repo launch it
SELF="$PWD/scripts/autopilot.sh"
SELF_SUM=$(sha1sum < "$SELF")
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
for t in claude git gh cargo exiftool jq; do
  command -v "$t" >/dev/null || { echo "preflight: '$t' not on PATH"; preflight_ok=0; }
done
gh auth status >/dev/null 2>&1 || { echo "preflight: gh is not logged in (run: gh auth login)"; preflight_ok=0; }
git remote get-url origin >/dev/null 2>&1 || { echo "preflight: no 'origin' remote"; preflight_ok=0; }
(( preflight_ok )) || exit 1

MAX_SESSIONS="${MAX_SESSIONS:-400}"
FIVE_HOUR_CAP="${FIVE_HOUR_CAP:-80}"
WEEKLY_CAP="${WEEKLY_CAP:-80}"
STALL_SECS=$(( ${STALL_MIN:-90} * 60 ))
LIMITS=.autopilot/limits   # "<5h %> <5h reset epoch> <weekly %> <weekly reset epoch>"

notify() { [[ -n "${NTFY_TOPIC:-}" ]] && curl -s -d "$1" "https://ntfy.sh/${NTFY_TOPIC}" >/dev/null; true; }
say() { echo "$(date '+%F %R') $*" | tee -a .autopilot/sessions.log; }
hhmm() { date -d "@$1" '+%a %R'; }

# Sleep that follows the wall clock, so time spent suspended counts.
wall_sleep_until() { while (( $(date +%s) < $1 )); do sleep 30; done; }

# Update $LIMITS from the newest usage event in a session log (keeps old values for missing windows).
update_limits() {
  local fh=0 fhr=0 wk=0 wkr=0 nfh nfhr nwk nwkr
  [[ -f $LIMITS ]] && read -r fh fhr wk wkr < "$LIMITS"
  read -r nfh nfhr nwk nwkr < <(grep -h '"rate_limit_event"' "$1" 2>/dev/null | tail -1 | jq -r '
    .rate_limit_info as $i | ($i.unifiedWindows // {}) as $w
    | def win(k): ($w[k] // (if $i.rateLimitType == k then $i else null end));
      [ (win("five_hour").utilization // null | if . == null then "-" else (. * 100 | floor) end),
        (win("five_hour").resetsAt   // "-"),
        (win("seven_day").utilization // null | if . == null then "-" else (. * 100 | floor) end),
        (win("seven_day").resetsAt   // "-") ] | @tsv' 2>/dev/null)
  [[ -z ${nfh:-} ]] && return
  [[ $nfh != - ]] && fh=$nfh; [[ $nfhr != - ]] && fhr=$nfhr
  [[ $nwk != - ]] && wk=$nwk; [[ $nwkr != - ]] && wkr=$nwkr
  echo "$fh $fhr $wk $wkr" > "$LIMITS"
}

# If a usage cap is reached, print "<window> <percent> <reset epoch>" and succeed.
capped() {
  [[ -f $LIMITS ]] || return 1
  local fh fhr wk wkr now; read -r fh fhr wk wkr < "$LIMITS"; now=$(date +%s)
  if (( wk >= WEEKLY_CAP && now < wkr )); then echo "weekly $wk $wkr"; return 0; fi
  if (( fh >= FIVE_HOUR_CAP && now < fhr )); then echo "5-hour $fh $fhr"; return 0; fi
  return 1
}

# Watch a running session (process group leader $1, log $2): stall watchdog and usage caps.
# Sets STOP_REASON when it ends the session itself.
supervise() {
  local pid=$1 log=$2 idle c
  STOP_REASON=""
  while kill -0 "$pid" 2>/dev/null; do
    sleep 60
    kill -0 "$pid" 2>/dev/null || break
    update_limits "$log"
    idle=$(( $(date +%s) - $(stat -c %Y "$log") ))
    if (( idle > STALL_SECS )); then STOP_REASON="stall"; say "watchdog: log silent for ${idle}s, ending session"
    elif c=$(capped); then STOP_REASON="cap"; set -- $c; say "usage cap: $1 window at $2%, ending session (resets $(hhmm "$3"))"
    else continue; fi
    kill -TERM -- "-$pid" 2>/dev/null; sleep 20; kill -KILL -- "-$pid" 2>/dev/null
    break
  done
}

# Best effort: keep the machine from idle-suspending while the autopilot runs.
systemd-inhibit --what=sleep:idle --who=blink-autopilot --why="Blink autopilot running" sleep infinity 2>/dev/null &
inhibit_pid=$!
trap 'kill $inhibit_pid 2>/dev/null' EXIT
sleep 1; kill -0 $inhibit_pid 2>/dev/null || echo "note: could not inhibit suspend; keep the machine awake yourself"

# Adopt a session left running by a previous launcher instead of starting a second one.
if old=$(pgrep -u "$(id -u)" -f '^claude -p /goal' | head -1); [[ -n $old ]]; then
  say "adopting running session (pid $old)"
  supervise "$old" "$(ls -t .autopilot/logs/*.jsonl | head -1)"
  while kill -0 "$old" 2>/dev/null; do sleep 30; done
fi

GOAL='Advance Blink following CLAUDE.md and SPEC.md, starting from PROGRESS.md (or Bootstrap if the repo has no Cargo.toml). The goal is met when ANY of these is shown in this session: (a) the current milestone gate passed, its report docs/reports/Mn.md is committed and pushed, and PROGRESS.md names the next milestone; (b) .autopilot/DONE exists after the M5 gate; (c) .autopilot/NEEDS_HUMAN.md exists for a blocker only a human can resolve, per the Escalation section of CLAUDE.md; (d) about 250 turns have been used, all work is committed and pushed, and PROGRESS.md has a concrete handoff in "Next up". Constraints: main stays green; no test, golden or baseline weakened without the required trailer.'

fails=0
i=0
while (( i < MAX_SESSIONS )); do
  if [[ -f .autopilot/DONE ]]; then echo "Final gate reached."; notify "Blink: final gate reached, v1.0.0-rc ready for you."; exit 0; fi
  if [[ -f .autopilot/NEEDS_HUMAN.md ]]; then echo "Human needed:"; cat .autopilot/NEEDS_HUMAN.md; notify "Blink needs you: $(head -n 3 .autopilot/NEEDS_HUMAN.md)"; exit 1; fi

  # Pick up a new version of this launcher between sessions.
  if [[ $(sha1sum < "$SELF") != "$SELF_SUM" ]]; then say "launcher changed, restarting it"; kill $inhibit_pid 2>/dev/null; exec bash "$SELF"; fi

  # Respect the usage caps: wait for the window to reset (+2 min margin).
  if c=$(capped); then
    set -- $c
    say "paused: $1 usage at $2% (cap ${FIVE_HOUR_CAP}%/${WEEKLY_CAP}% for 5-hour/weekly), resuming $(hhmm $(( $3 + 120 )))"
    [[ $1 == weekly ]] && notify "Blink paused: weekly usage at $2%, resumes $(hhmm $(( $3 + 120 )))"
    wall_sleep_until $(( $3 + 120 ))
    continue
  fi

  i=$(( i + 1 ))
  reports_before=$(ls docs/reports 2>/dev/null | wc -l)
  ts=$(date -u +%Y%m%dT%H%M%SZ)
  start=$(date +%s)
  log=".autopilot/logs/${ts}.jsonl"
  # --settings outranks project settings: commits and PRs never credit Claude.
  setsid claude -p "/goal ${GOAL}" --permission-mode auto --strict-mcp-config \
    --settings '{"attribution":{"commit":"","pr":""},"includeCoAuthoredBy":false}' \
    --output-format stream-json --verbose \
    < /dev/null > "$log" 2>&1 &
  cpid=$!
  supervise "$cpid" "$log"
  wait "$cpid"; code=$?
  update_limits "$log"
  dur=$(( $(date +%s) - start ))
  echo "${ts} session=${i} exit=${code} duration=${dur}s" | tee -a .autopilot/sessions.log

  reports_after=$(ls docs/reports 2>/dev/null | wc -l)
  (( reports_after > reports_before )) && notify "Blink: new gate report $(ls -t docs/reports | head -n 1)"

  # A cap or limit is handled at the top of the loop. Other very short sessions usually
  # mean an outage: back off (15 min steps, max 2 h).
  if capped >/dev/null || [[ $STOP_REASON == cap ]]; then
    fails=0
  elif (( dur < 300 )); then
    fails=$(( fails < 8 ? fails + 1 : 8 ))
    wall_sleep_until $(( $(date +%s) + fails * 900 ))
  else
    fails=0
    wall_sleep_until $(( $(date +%s) + 30 ))
  fi
done
echo "Reached MAX_SESSIONS=${MAX_SESSIONS}."
notify "Blink autopilot stopped after ${MAX_SESSIONS} sessions."
