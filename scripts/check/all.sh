#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$task_root"
# Keep this checkout's tests independent of unavailable workstation-wide build mounts.
export CARGO_TARGET_DIR="${ADMIRALTY_TARGET_DIR:-$task_root/target}"
run() {
  if command -v rtk >/dev/null 2>&1; then rtk proxy "$@"; else "$@"; fi
}
run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
run cargo test --workspace --locked
run python3 -m unittest discover -s tests/legacy -v
run python3 tests/fixtures/legacy/verify.py
run cargo run -p admiralty-cli --locked -- contracts check
run pnpm contracts:check
run pnpm --dir packages/contracts test
if [[ -f apps/dashboard/package.json ]]; then
  run pnpm --dir apps/dashboard typecheck
  run pnpm --dir apps/dashboard test --run
  run pnpm --dir apps/dashboard build
fi
