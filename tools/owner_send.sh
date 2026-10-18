#!/bin/bash
# usage: send.sh SCREEN FILE
S=$1; F=$2
screen -S "$S" -X readbuf "$F"; screen -S "$S" -X paste .; sleep 1; screen -S "$S" -X stuff "$(printf '\r')"
