#!/bin/bash
# Deliver compute_notice.txt to the 4 captains only when idle; retry 15 min; fallback = pointer in coordination log.
D=/home/ubuntu/xbabe2_kicker/captains-20261017
declare -A SID=([joil]=2708475.captain_joil [dope]=2716151.captain_dope [jopedime]=2724936.captain_jopedime [jarambit]=2740538.captain_jarambit)
declare -A LOG=([joil]=/home/ubuntu/joil/COORD.md [dope]=/home/ubuntu/dope/COMMS.md [jopedime]=/home/ubuntu/JopeDime/COMMS.md [jarambit]=/home/ubuntu/jarambit/COMMS.md)
declare -A LCK=([joil]=/home/ubuntu/joil/.private/v5-workers/coord.lock)
idle(){ f=/tmp/hc_cn_$1; rm -f $f; screen -S $1 -X hardcopy $f; sleep 0.4; [ -s $f ] && grep -aq "? for shortcuts" $f && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
for i in $(seq 1 60); do left=0
 for k in joil dope jopedime jarambit; do [ -e /tmp/sent_compute_$k ] && continue; left=1
   if idle ${SID[$k]}; then screen -S ${SID[$k]} -X readbuf $D/compute_notice.txt; screen -S ${SID[$k]} -X paste .; sleep 1; screen -S ${SID[$k]} -X stuff $(printf "\r"); touch /tmp/sent_compute_$k; echo "sent $k at $(TZ=America/Denver date +%H:%M:%S) MDT"; fi; done
 [ $left = 0 ] && break; sleep 15; done
for k in joil dope jopedime jarambit; do [ -e /tmp/sent_compute_$k ] && continue
  msg="
## $(date -u +%Y-%m-%dT%H:%M:%SZ) - Grok Bot to ${SID[$k]}: COMPUTE NOTICE (prompt stayed busy 15 min, so posting here)

$(cat $D/compute_notice.txt)
"
  if [ -n "${LCK[$k]}" ]; then flock -w 60 ${LCK[$k]} sh -c "printf \"%s\n\" \"\$0\" >> ${LOG[$k]}" "$msg"; else printf "%s\n" "$msg" >> ${LOG[$k]}; fi
  touch /tmp/logged_compute_$k; echo "busy 15m: pointer appended to ${LOG[$k]} for $k at $(TZ=America/Denver date +%H:%M:%S) MDT"
done
echo DONE
