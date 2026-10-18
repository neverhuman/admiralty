#!/bin/sh
# TEST FIXTURE ONLY: no provider, network, credentials or real inference.
case "$1" in
  --version) printf '%s\n' 'codex-cli 0.163.0'; exit 0 ;;
  --help) printf '%s\n' '--no-daemon --ask-for-approval'; exit 0 ;;
  exec) test "$2" = --help || exit 2; printf '%s\n' '--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd'; exit 0 ;;
esac
test "$1" = --no-daemon || exit 2
work=
while test "$#" -gt 0; do
  if test "$1" = --cd; then shift; work=$1; break; fi
  shift
done
test -n "$work" || exit 2
cat > /dev/null
cat > "$work/src/lib.rs" <<'RUST'
pub fn saturating_increment(value: u32) -> u32 {
    value.saturating_add(1)
}
RUST
printf '%s\n' \
  '{"type":"thread.started","thread_id":"fake-thread"}' \
  '{"type":"turn.started"}' \
  '{"type":"item.completed","item":{"id":"claim","type":"agent_message","text":"PASS is only a provider claim"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}'
