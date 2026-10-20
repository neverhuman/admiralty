#!/bin/sh

usage() {
    printf '%s\n' "usage: codex-luna.sh EXISTING_WORKING_DIRECTORY" >&2
    exit 64
}

if [ "$#" -ne 1 ]; then
    usage
fi

case $1 in
    -* )
        printf '%s\n' 'codex-luna.sh: options are not accepted' >&2
        exit 64
        ;;
esac

if [ ! -d "$1" ]; then
    printf '%s\n' 'codex-luna.sh: working directory must already exist' >&2
    exit 64
fi

working_dir=$(CDPATH= cd -P "$1" 2>/dev/null && pwd -P) || {
    printf '%s\n' 'codex-luna.sh: cannot resolve working directory' >&2
    exit 64
}

if ! command -v codex >/dev/null 2>&1; then
    printf '%s\n' 'codex-luna.sh: codex executable was not found on PATH' >&2
    exit 127
fi

CDPATH= cd -P "$working_dir" || exit 64
exec codex -m gpt-6-luna -c model_reasoning_effort=medium -C "$working_dir"
