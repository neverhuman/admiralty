#!/bin/bash
# Ticker for screen "super_captain": every 30 min, paste super_captain_tick.txt into super_captain ONLY when its agy prompt is idle.
# Waits up to 20 min for idle; otherwise skips that tick (super_captain also self-checks after 35 min). Read-only otherwise.
D=/home/ubuntu/xbabe2_kicker/captains-20261017
S=super_captain
LOG=$D/super_captain_tick.log
idle(){ f=/tmp/hc_sc_tick; rm -f $f; screen -S $S -X hardcopy $f; sleep 0.5; [ -s $f ] && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; }
echo "$(date -u +%FT%TZ) ticker start pid $$" >> $LOG
sleep ${FIRST_DELAY:-1800}
while true; do
  sent=0
  for i in $(seq 1 80); do
    if ! screen -ls | grep -q "\.$S[[:space:]]"; then echo "$(date -u +%FT%TZ) super_captain screen missing" >> $LOG; break; fi
    if idle; then screen -S $S -X readbuf $D/super_captain_tick.txt; screen -S $S -X paste .; sleep 1; screen -S $S -X stuff $'\r'
      echo "$(date -u +%FT%TZ) tick sent ($(TZ=America/Denver date +%H:%M) MDT)" >> $LOG; sent=1; break; fi
    sleep 15
  done
  [ $sent = 0 ] && echo "$(date -u +%FT%TZ) tick skipped: super_captain busy 20 min" >> $LOG
  sleep 1800
done
