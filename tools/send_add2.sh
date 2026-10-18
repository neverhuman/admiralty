#!/bin/bash
# Deliver Addendum 2 to the 4 captain screens: only when the agy prompt is idle; retry ~10 min; fallback = pointer in coordination log.
D=/home/ubuntu/xbabe2_kicker/captains-20261017
declare -A SID=([joil]=2708475.captain_joil [dope]=2716151.captain_dope [jopedime]=2724936.captain_jopedime [jarambit]=2740538.captain_jarambit)
declare -A LOG=([joil]=/home/ubuntu/joil/COORD.md [dope]=/home/ubuntu/dope/COMMS.md [jopedime]=/home/ubuntu/JopeDime/COMMS.md [jarambit]=/home/ubuntu/jarambit/COMMS.md)
declare -A LCK=([joil]=/home/ubuntu/joil/.private/v5-workers/coord.lock)
idle(){ f=/tmp/hc_a2_$1; screen -S $1 -X hardcopy $f; sleep 0.3; grep -aq "? for shortcuts" $f && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
pastef(){ screen -S $1 -X readbuf $2; screen -S $1 -X paste .; sleep 1; screen -S $1 -X stuff $(printf "\r"); echo "sent $1 $(basename $2) at $(TZ=America/Denver date +%H:%M:%S) MDT"; }
for i in $(seq 1 40); do left=0
 for k in joil dope jopedime jarambit; do [ -e /tmp/sent_add2_$k ] && continue; left=1
   idle ${SID[$k]} && { pastef ${SID[$k]} $D/kick_add2_$k.txt; touch /tmp/sent_add2_$k; }; done
 [ $left = 0 ] && break; sleep 15; done
for k in joil dope jopedime jarambit; do [ -e /tmp/sent_add2_$k ] && continue
  msg="
## $(date -u +%Y-%m-%dT%H:%M:%SZ) - Grok Bot to ${SID[$k]}: ADDENDUM 2 (Jepson, 9:24 PM MDT) waiting for you

Captain: your prompt stayed busy for 10 minutes, so this pointer is going in the log instead. $(cat $D/kick_add2_$k.txt)
"
  if [ -n "${LCK[$k]}" ]; then flock -w 60 ${LCK[$k]} sh -c "printf '%s\n' \"\$0\" >> ${LOG[$k]}" "$msg"; else printf '%s\n' "$msg" >> ${LOG[$k]}; fi
  touch /tmp/logged_add2_$k; echo "busy 10m: pointer appended to ${LOG[$k]} for $k at $(TZ=America/Denver date +%H:%M:%S) MDT"
done
echo DONE
