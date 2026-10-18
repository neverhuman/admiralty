#!/bin/bash
# Ticker for screen "super_captain": every 30 min, run the read-only audit (tools/sc_audit.py), then paste
# super_captain_tick.txt into super_captain ONLY when its agy prompt is idle.
# Waits up to 20 min for idle; otherwise skips that tick (super_captain also self-checks after 35 min).
# Also logs whether super_captain acted on the previous tick (status.json updated after it). Read-only otherwise.
D=/home/ubuntu/xbabe2_kicker/captains-20261017
S=super_captain
LOG=$D/super_captain_tick.log
AUDIT=/home/ubuntu/admiralty/tools/sc_audit.py
idle(){ f=/tmp/hc_sc_tick; rm -f $f; screen -S $S -X hardcopy $f; sleep 0.5; [ -s $f ] && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
echo "$(date -u +%FT%TZ) ticker start pid $$" >> $LOG
sleep ${FIRST_DELAY:-1800}
last_sent=0
while true; do
  if [ "$last_sent" != 0 ]; then
    st=$(stat -c %Y $D/super_captain_status.json 2>/dev/null || echo 0)
    if [ "$st" -gt "$last_sent" ]; then echo "$(date -u +%FT%TZ) previous tick acted on (status.json updated $(date -u -d @$st +%TZ))" >> $LOG
    else echo "$(date -u +%FT%TZ) WARNING previous tick NOT acted on (status.json not updated since tick)" >> $LOG; fi
  fi
  if [ -f $AUDIT ]; then
    timeout 300 nice -n 10 python3 $AUDIT --quiet >> $D/sc_audit_cron.log 2>&1 \
      && echo "$(date -u +%FT%TZ) audit ok: $(python3 -c "import json;print(json.load(open('$D/sc_audit/latest.json'))['summary'])" 2>/dev/null)" >> $LOG \
      || echo "$(date -u +%FT%TZ) audit FAILED (see sc_audit_cron.log)" >> $LOG
  fi
  sent=0
  for i in $(seq 1 80); do
    if ! screen -ls | grep -q "\.$S[[:space:]]"; then echo "$(date -u +%FT%TZ) super_captain screen missing" >> $LOG; break; fi
    if idle; then screen -S $S -X readbuf $D/super_captain_tick.txt; screen -S $S -X paste .; sleep 1; screen -S $S -X stuff $'\r'
      echo "$(date -u +%FT%TZ) tick sent ($(TZ=America/Denver date +%H:%M) MDT)" >> $LOG; sent=1; last_sent=$(date +%s); break; fi
    sleep 15
  done
  [ $sent = 0 ] && echo "$(date -u +%FT%TZ) tick skipped: super_captain busy 20 min" >> $LOG
  sleep 1800
done
