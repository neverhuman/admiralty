#!/bin/bash
for s in jopedime_lane_b_codex jopedime_lane_c_codex jopedime_kernel2_codex jopedime_codex_cleanup jopeDime_cleanup_claude redline_codex redclaude dope grok_dope dope_paper_grok grok_sit_watch grok_jeryu_audit xbabe2_storage jankurai grok_red; do
 f=/home/ubuntu/xbabe2_kicker/hc_$s.txt; screen -S $s -X hardcopy $f; sleep 0.5
 echo "=================== $s"; tr -cd '\11\12\15\40-\176' < $f | grep -v '^\s*$' | tail -n 9 | cut -c1-180
done
