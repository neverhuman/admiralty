#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$task_root"
task_status="" task_index="" task_models=""
task_state="${ADMIRALTY_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/admiralty-preview}"
usage() {
  echo 'Usage: pnpm live --status /absolute/status.json --index /absolute/index.html [--models /private/models.json]'
  echo 'Serves real producer observations on loopback. No provider execution or machine enrollment.'
}
while (($#)); do
  case "$1" in
    --) shift ;;
    --help|-h) usage; exit 0 ;;
    --status|--index|--models)
      if (($# < 2)); then usage >&2; exit 2; fi
      case "$1" in
        --status) task_status="$2" ;; --index) task_index="$2" ;; --models) task_models="$2" ;;
      esac
      shift 2 ;;
    *) usage >&2; exit 2 ;;
  esac
done
for task_file in "$task_status" "$task_index"; do
  if [[ "$task_file" != /* || ! -f "$task_file" ]]; then
    echo 'Both --status and --index must name existing absolute producer files.' >&2
    exit 2
  fi
done
if [[ -n "$task_models" && ( "$task_models" != /* || ! -f "$task_models" ) ]]; then
  echo '--models must name an existing absolute private registry file.' >&2
  exit 2
fi
run() {
  if command -v rtk >/dev/null 2>&1; then rtk proxy "$@"; else "$@"; fi
}
export CARGO_TARGET_DIR="${ADMIRALTY_TARGET_DIR:-$task_root/target}"
# Create (or verify) a private 0700 preview state directory with a 0600 token.
# Existing state is never relaxed or rewritten; the token value is never printed.
run cargo run -p admiralty-cli --locked --quiet -- state init --dir "$task_state"
run pnpm --dir apps/dashboard build
task_args=(--database "$task_state/native.sqlite" --token-file "$task_state/dashboard-token"
  --listen "${ADMIRALTY_LIVE_LISTEN:-127.0.0.1:18891}" --assets "$task_root/apps/dashboard/dist"
  --legacy-status-file "$task_status" --legacy-index-file "$task_index" --legacy-public-read)
if [[ -n "$task_models" ]]; then task_args+=(--models-file "$task_models"); fi
echo 'Starting real read-only fleet view. Original page remains available at /legacy/.'
run cargo run -p admiralty-server --bin admiraltyd --locked -- "${task_args[@]}"
