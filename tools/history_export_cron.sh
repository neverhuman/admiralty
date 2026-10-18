#!/bin/bash
# Hourly history export for neverhuman/admiralty (cron on xbabe2, see data/history/README.md).
# Runs in its OWN clone so it never touches the live checkout at /home/ubuntu/admiralty or any service.
# Commits ONLY files under data/history/ and pushes them straight to main as jepsontaylor (documented exception).
set -u
CLONE=${ADMIRALTY_HISTORY_CLONE:-/home/ubuntu/.admiralty-history}
LOG=${ADMIRALTY_HISTORY_LOG:-/home/ubuntu/xbabe2_kicker/captains-dashboard/history_export.log}
export PATH=/home/ubuntu/.local/bin:/usr/local/bin:/usr/bin:/bin HOME=/home/ubuntu
exec >>"$LOG" 2>&1
exec 9>/tmp/admiralty_history_export.lock
flock -n 9 || { echo "$(date -u +%FT%TZ) skip: previous run still active"; exit 0; }
echo "$(date -u +%FT%TZ) start"
[ -d "$CLONE/.git" ] || git clone -q https://github.com/neverhuman/admiralty.git "$CLONE" || exit 1
cd "$CLONE" || exit 1
git config user.name jepsontaylor; git config user.email 130782313+jepsontaylor@users.noreply.github.com
# never carry stray local edits: anything outside data/history is reset to origin
git fetch -q origin main || exit 1
if [ -n "$(git status --porcelain -- . ':!data/history')" ]; then echo "dirty outside data/history; refusing"; git status --short; exit 1; fi
git merge -q --ff-only origin/main || { echo "cannot fast-forward"; exit 1; }
nice -n 15 ionice -c3 python3 -B tools/export_history.py --out "$CLONE/data/history" || { echo "export failed or secret scan hit (exit $?) - nothing committed"; git checkout -q -- data/history 2>/dev/null; git clean -fdq -- data/history; exit 1; }
git add -A data/history
if git diff --cached --quiet; then echo "no data change"; exit 0; fi
bad=$(git diff --cached --name-only | grep -v '^data/history/' || true)
[ -n "$bad" ] && { echo "refusing: staged files outside data/history: $bad"; git reset -q; exit 1; }
git commit -q -m "data(history): hourly export $(date -u +%FT%H:%MZ)" -m "Automated: tools/export_history.py via tools/history_export_cron.sh (data/history only; see data/history/README.md)."
for i in 1 2 3; do
  git push -q origin HEAD:main && { echo "pushed $(git rev-parse --short HEAD)"; exit 0; }
  git fetch -q origin main && git rebase -q origin/main || { git rebase --abort; echo "rebase failed"; exit 1; }
done
echo "push failed after retries"; exit 1
