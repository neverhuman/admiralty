#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$task_root"
export CARGO_TARGET_DIR="${ADMIRALTY_TARGET_DIR:-$task_root/target}"
run() {
  if command -v rtk >/dev/null 2>&1; then rtk proxy "$@"; else "$@"; fi
}
run pnpm --dir apps/dashboard build
run cargo run -p admiralty-cli --locked -- demo --listen "${ADMIRALTY_DEMO_LISTEN:-127.0.0.1:18891}" --assets "$task_root/apps/dashboard/dist"
