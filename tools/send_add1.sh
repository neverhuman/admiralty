D=/home/ubuntu/xbabe2_kicker/captains-20261017
idle(){ f=/tmp/hc_a_$1; screen -S $1 -X hardcopy $f; sleep 0.3; grep -aq "? for shortcuts" $f && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
pastef(){ screen -S $1 -X readbuf $2; screen -S $1 -X paste .; sleep 1; screen -S $1 -X stuff $(printf "\r"); echo "sent $1 $(basename $2) at $(TZ=America/Denver date +%H:%M:%S)"; }
for k in dope jopedime; do printf "%s" "ADDENDUM 1 UPDATE from Jepson (8:52 PM MDT): \"I have more Codex quota I can bring on, so please run out the current.\" Do NOT conserve Codex quota: use Sol/Luna screens freely until it is gone (still within the load cap and safety rules). When Codex hits its limit, log NEEDS JEPSON: Codex quota exhausted, and fall back to Claude/agy lanes. Updated text is in $D/addendum1_${k}.txt." > $D/kick_add1b_$k.txt; done
for k in joil jarambit; do printf "%s" "ADDENDUM 1 from Jepson (Oct 17 2026, 8:51 PM MDT, updated 8:52 PM): keep this going strong all night with no stopping, and add Codex Sol/Luna screens where it speeds things up (use Codex quota freely, do not conserve it). Read $D/addendum1_${k}.txt in full now and follow it alongside your brief. Acknowledge in the coordination log, then act." > $D/kick_add1_$k.txt; done
for i in $(seq 1 45); do left=0
 for k in dope jopedime; do [ -e /tmp/sent_add1b_$k ] && continue; left=1; idle captain_$k && { pastef captain_$k $D/kick_add1b_$k.txt; touch /tmp/sent_add1b_$k; }; done
 for k in joil jarambit; do [ -e /tmp/sent_add1_$k ] && continue; left=1; idle captain_$k && { pastef captain_$k $D/kick_add1_$k.txt; touch /tmp/sent_add1_$k; }; done
 [ $left = 0 ] && break; sleep 15; done; ls /tmp/sent_add1*
