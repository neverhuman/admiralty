#!/bin/bash
# Proxy+hygiene notice: post to each coord log NOW, then try an idle-only paste (incl. compute pointer) for up to 25 min.
D=/home/ubuntu/xbabe2_kicker/captains-20261017
declare -A SID=([joil]=2708475.captain_joil [dope]=2716151.captain_dope [jopedime]=2724936.captain_jopedime [jarambit]=2740538.captain_jarambit)
declare -A LOG=([joil]=/home/ubuntu/joil/COORD.md [dope]=/home/ubuntu/dope/COMMS.md [jopedime]=/home/ubuntu/JopeDime/COMMS.md [jarambit]=/home/ubuntu/jarambit/COMMS.md)
declare -A LCK=([joil]=/home/ubuntu/joil/.private/v5-workers/coord.lock [dope]=/home/ubuntu/dope/COMMS.md.lock [jopedime]=/home/ubuntu/JopeDime/COMMS.md.lock [jarambit]=/home/ubuntu/jarambit/COMMS.md.lock)
for k in joil dope jopedime jarambit; do
  msg="
## $(date -u +%Y-%m-%dT%H:%M:%SZ) - Grok Bot to ${SID[$k]}: PROXY + HYGIENE ORDERS (Jepson 10:01/10:02 PM MDT)

$(cat $D/captain_notice2.txt)
"
  flock -w 60 ${LCK[$k]} sh -c 'printf "%s\n" "$0" >> "$1"' "$msg" "${LOG[$k]}"; echo "logged $k $(date -u +%T)Z"
done
cat $D/captain_notice2.txt > $D/captain_notice2_paste.txt
printf ' (Also, if you have not yet: the COMPUTE notice in your coord log - xbabe1/xbabe2/xbabe3 available, never xbabe0 - see COMMON.md "Compute"; post COMPUTE ACK.)' >> $D/captain_notice2_paste.txt
idle(){ f=/tmp/hc_n2_$1; rm -f $f; screen -S $1 -X hardcopy $f; sleep 0.4; [ -s $f ] && grep -aq "? for shortcuts" $f && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
rm -f /tmp/sent_n2_*
for i in $(seq 1 100); do left=0
 for k in joil dope jopedime jarambit; do [ -e /tmp/sent_n2_$k ] && continue; left=1
   if idle ${SID[$k]}; then screen -S ${SID[$k]} -X readbuf $D/captain_notice2_paste.txt; screen -S ${SID[$k]} -X paste .; sleep 1; screen -S ${SID[$k]} -X stuff $'\r'; touch /tmp/sent_n2_$k; echo "pasted $k at $(TZ=America/Denver date +%H:%M:%S) MDT"; fi; done
 [ $left = 0 ] && break; sleep 15; done
echo DONE
