#!/bin/bash
D=/home/ubuntu/xbabe2_kicker/captains-20261017; S=4060655.super_captain; f=/tmp/hc_sc_o
for i in $(seq 1 120); do rm -f $f; screen -S $S -X hardcopy $f; sleep 0.5
  if [ -s $f ] && grep -aq "? for shortcuts" $f && ! grep -aq "esc to cancel" $f && [ "$(grep -a "^>" $f | tail -1 | tr -d " ")" = ">" ]; then
    screen -S $S -X readbuf $D/sc_paste_orders.txt; screen -S $S -X paste .; sleep 1; screen -S $S -X stuff $'\r'; echo "SC orders pasted $(TZ=America/Denver date +%H:%M:%S) MDT"; exit 0; fi
  sleep 10; done
echo "SC busy 20 min; not pasted"
