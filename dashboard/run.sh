#!/bin/bash
# Launched inside screen "captains_dash". Read-only collector + 127.0.0.1:18890 server, low priority.
cd /home/ubuntu/xbabe2_kicker/captains-dashboard || exit 1
PORT=${PORT:-18890}
nice -n 19 ionice -c3 python3 -B serve.py "$PORT" &
SRV=$!
echo "server pid $SRV on 127.0.0.1:$PORT"
trap 'kill $SRV 2>/dev/null' EXIT
while true; do
  nice -n 19 ionice -c3 python3 -B collector.py
  echo "collector exited $?; restarting in 10s"; sleep 10
done
