#!/bin/bash
# usage: mr_probe.sh outdir
O=${1:-/tmp/mr_probe}; mkdir -p $O
screen -ls | grep -oE '[0-9]+\.[A-Za-z0-9_]+' | while read sid; do
  n=${sid#*.}
  case $n in captains_dash|disk_janitor) continue;; esac
  screen -S $sid -X hardcopy -h $O/$n.txt 2>/dev/null
done
sleep 1
for f in $O/*.txt; do n=$(basename $f .txt)
  m=$(grep -aoE "GPT-[0-9.]+-[A-Za-z]+ (low|medium|high|xhigh|max|ultra)" $f | tail -1)
  [ -z "$m" ] && m=$(grep -aoE "Gemini [0-9] [A-Za-z]+( \(?[A-Za-z]+\)?)?" $f | tail -1)
  [ -z "$m" ] && m=$(grep -aoE "(Opus|Sonnet|Haiku|Fable) [0-9.]+[^│]{0,12}" $f | tail -1)
  # child processes of screen
  spid=$(screen -ls | grep -oE "[0-9]+\.$n\b" | head -1 | cut -d. -f1)
  cm=""
  if [ -n "$spid" ]; then
    for p in $(pgrep -P $spid); do
      cm="$cm $(ps -o args= --ppid $p -p $p 2>/dev/null | grep -aoE '(codex|claude|agy)[^ ]*( [^ ]+){0,12}' | grep -aoE '(-m|--model|model=|--effort|model_reasoning_effort=)[= ]?[^ ]+' | tr '\n' ' ')"
    done
    all=$(pstree -pa $spid 2>/dev/null | grep -aoE '(-m [a-z0-9.\-]+|--model[= ][a-z0-9.\-]+|model="?[a-z0-9.\-]+|model_reasoning_effort="?[a-z]+|--effort[= ][a-z]+)' | sort -u | tr '\n' ' ')
  fi
  printf "%-40s | TUI: %-28s | args: %s\n" "$n" "$m" "$all"
done
