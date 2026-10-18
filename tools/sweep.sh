#!/bin/bash -l
for r in ~/JopeDime ~/redlineDB ~/Dope; do echo "== $r $(git -C $r remote get-url origin) branch=$(git -C $r branch --show-current)"; done
for R in $(for r in ~/JopeDime ~/redlineDB ~/Dope; do git -C $r remote get-url origin | sed -E 's#.*github.com[:/]##; s#\.git$##'; done); do
 echo "##### $R PRs"
 gh-role reviewer pr list -R $R --state open --json number,title,author,isDraft,mergeStateStatus,reviewDecision,headRefOid,updatedAt --jq '.[]|"#\(.number) \(.author.login) draft=\(.isDraft) \(.mergeStateStatus) \(.reviewDecision) \(.headRefOid[0:8]) upd=\(.updatedAt) \(.title[0:70])"'
 echo "##### $R main runs"
 gh-role reviewer run list -R $R --branch main -L 8 --json headSha,workflowName,status,conclusion,createdAt --jq '.[]|"\(.headSha[0:9]) \(.workflowName) \(.status) \(.conclusion) \(.createdAt)"'
 echo "##### $R merged recent"
 gh-role reviewer pr list -R $R --state merged -L 4 --json number,title,mergedAt,mergedBy --jq '.[]|"#\(.number) \(.mergedAt) \(.mergedBy.login) \(.title[0:60])"'
done
echo "##### GPUs"
for h in xbabe1 xbabe3; do echo "$h: $(ssh -n -o ConnectTimeout=8 $h 'nvidia-smi --query-gpu=utilization.gpu,memory.used --format=csv,noheader; free -g | awk "/Mem/{print \"avail\",\$7,\"of\",\$2}"; nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader' 2>&1 | tr '\n' ' ')"; done
echo "x2: $(nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader | tr '\n' ' ')"
echo "##### dl_status"; ls -la --time-style=+%H:%M /home/ubuntu/vp_dashboard/dl_status.json; head -c 1500 /home/ubuntu/vp_dashboard/dl_status.json; echo
echo "##### jsonl mtimes (codex/claude)"
find ~/.codex/sessions -name '*.jsonl' -mmin -120 -printf '%TH:%TM %p\n' 2>/dev/null | sort | tail -8
find ~/.claude/projects -name '*.jsonl' -mmin -120 -printf '%TH:%TM %p\n' 2>/dev/null | sort | tail -8
echo "##### tmp/sibling worktrees"; ls -d /tmp/*worktree* /tmp/JopeDime* ~/JopeDime-* ~/Dope-* ~/redline*-* 2>/dev/null
git -C ~/JopeDime worktree list | grep -v '/.agent/worktrees/' 
echo "count jd wt: $(git -C ~/JopeDime worktree list | wc -l)"
echo "##### kicker cron"; crontab -l | grep -i kick
systemctl list-timers --all 2>/dev/null | grep -i -E 'guard|storage' 
df -h /fast-scratch /mnt/fast-scratch 2>/dev/null
uptime
