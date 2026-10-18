#!/bin/bash
screen -S "$1" -X hardcopy /tmp/peek_$1; tr -cd '\11\12\15\40-\176' < /tmp/peek_$1 | grep -v '^\s*$' | tail -${2:-25}
