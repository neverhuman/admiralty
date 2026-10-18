#!/bin/bash
# Second idle-only delivery pass for joil + jarambit (log pointers already posted at 03:38:57Z).
D=/home/ubuntu/xbabe2_kicker/captains-20261017
declare -A SID=([joil]=2708475.captain_joil [jarambit]=2740538.captain_jarambit)
idle(){ f=/tmp/hc_a2_$1; screen -S $1 -X hardcopy $f; sleep 0.3; grep -aq "? for shortcuts" $f && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
pastef(){ screen -S $1 -X readbuf $2; screen -S $1 -X paste .; sleep 1; screen -S $1 -X stuff $(printf "\r"); echo "sent $1 $(basename $2) at $(TZ=America/Denver date +%H:%M:%S) MDT"; }
for i in $(seq 1 40); do left=0
 for k in joil jarambit; do [ -e /tmp/sent_add2_$k ] && continue; left=1
   idle ${SID[$k]} && { pastef ${SID[$k]} $D/kick_add2_$k.txt; touch /tmp/sent_add2_$k; }; done
 [ $left = 0 ] && break; sleep 15; done
echo DONE2
