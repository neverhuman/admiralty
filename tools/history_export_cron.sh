#!/bin/bash
# Hourly history export for neverhuman/admiralty (cron on xbabe2, see data/history/README.md).
# Runs in its OWN clone so it never touches the live checkout at /home/ubuntu/admiralty or any service.
# Commits ONLY explicit allowlisted files under data/history/ and pushes them straight to main as jepsontaylor (documented exception).
set -euo pipefail
CLONE=${ADMIRALTY_HISTORY_CLONE:-/home/ubuntu/.admiralty-history}
LOG=${ADMIRALTY_HISTORY_LOG:-/home/ubuntu/xbabe2_kicker/captains-dashboard/history_export.log}
LOCK=${ADMIRALTY_HISTORY_LOCK:-/tmp/admiralty_history_export.lock}
export PATH=/home/ubuntu/.local/bin:/usr/local/bin:/usr/bin:/bin HOME=/home/ubuntu
# flock/ionice are part of this Linux cron job's custody boundary. A missing
# locking command must never be reported as a successful contended-lock skip.
if [ "$(uname -s)" != Linux ]; then
  echo "history export requires Linux with flock and ionice" >&2
  exit 1
fi
for required_tool in flock git nice ionice python3; do
  if ! command -v "$required_tool" >/dev/null 2>&1; then
    echo "history export missing required tool: $required_tool" >&2
    exit 1
  fi
done
exec >>"$LOG" 2>&1
exec 9>"$LOCK"
if flock -n -E 75 9; then
  :
else
  lock_status=$?
  if [ "$lock_status" -eq 75 ]; then
    echo "$(date -u +%FT%TZ) skip: previous run still active"
    exit 0
  fi
  echo "history export could not establish lock custody (exit $lock_status)" >&2
  exit "$lock_status"
fi
echo "$(date -u +%FT%TZ) start"
[ -d "$CLONE/.git" ] || git clone -q https://github.com/neverhuman/admiralty.git "$CLONE" || exit 1
cd "$CLONE" || exit 1
git config user.name jepsontaylor
git config user.email 130782313+jepsontaylor@users.noreply.github.com

is_allowed_export_path() {
  case "$1" in
    data/history/README.md|\
    data/history/rates_index.json|\
    data/history/status.jsonl|\
    data/history/summary.json|\
    data/history/quota/codex_quota.csv|\
    data/history/resources/disk_used.csv|\
    data/history/resources/host_snapshots.csv|\
    data/history/tokens/codex_sessions.csv|\
    data/history/tokens/claude_hourly.csv|\
    data/history/timeseries/history.csv|\
    data/history/timeseries/history_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].jsonl|\
    data/history/ledger/model_usage.csv|\
    data/history/ledger/model_usage_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].jsonl|\
    data/history/token_yield/token_yield_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].jsonl|\
    data/history/snapshots/projects_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].jsonl)
      return 0 ;;
    *)
      return 1 ;;
  esac
}

# Require full index and worktree custody BEFORE fetch/merge/export.
# Never reset or overwrite pre-existing staged or dirty work.
if ! git diff --cached --quiet; then
  echo "pre-existing staged index entries; refusing without modifying index or worktree"
  git status --short
  exit 1
fi
if [ -n "$(git status --porcelain --untracked-files=all -- . ':!data/history')" ]; then
  echo "dirty outside data/history; refusing without modifying index or worktree"
  git status --short
  exit 1
fi
if ! git diff --quiet -- data/history || [ -n "$(git status --porcelain --untracked-files=all -- data/history)" ]; then
  echo "pre-existing dirty or untracked files under data/history; refusing without modifying index or worktree"
  git status --short
  exit 1
fi

git fetch -q origin main || exit 1
git merge -q --ff-only origin/main || { echo "cannot fast-forward"; exit 1; }

if ! nice -n 15 ionice -c3 python3 -B tools/export_history.py --out "$CLONE/data/history"; then
  echo "export failed or scan hit - restoring clean HEAD data/history tracked files"
  git checkout -q HEAD -- data/history 2>/dev/null || true
  find "$CLONE/data/history" -name '*.tmp' -type f -delete 2>/dev/null || true
  exit 1
fi

candidate_paths=()
while IFS= read -r line; do
  [ -z "$line" ] && continue
  p="${line:3}"
  if ! is_allowed_export_path "$p"; then
    echo "refusing: exporter modified or created non-allowlisted path: $p"
    exit 1
  fi
  candidate_paths+=("$p")
done < <(git status --porcelain --untracked-files=all)

for p in "${candidate_paths[@]}"; do
  [ -f "$p" ] && git add -- "$p"
done

if git diff --cached --quiet; then
  echo "no data change"
  exit 0
fi

while IFS= read -r staged_path; do
  [ -z "$staged_path" ] && continue
  if ! is_allowed_export_path "$staged_path"; then
    echo "refusing: staged path outside exact export allowlist: $staged_path"
    exit 1
  fi
done < <(git diff --cached --name-only)

git commit -q -m "data(history): hourly export $(date -u +%FT%H:%MZ)" -m "Automated: tools/export_history.py via tools/history_export_cron.sh (exact allowlisted data/history paths only; see data/history/README.md)."
for i in 1 2 3; do
  git push -q origin HEAD:main && { echo "pushed $(git rev-parse --short HEAD)"; exit 0; }
  git fetch -q origin main && git rebase -q origin/main || { git rebase --abort; echo "rebase failed"; exit 1; }
done
echo "push failed after retries"
exit 1
