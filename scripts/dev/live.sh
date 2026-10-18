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
# Create a private preview state directory; never relax an existing directory.
run python3 - "$task_state" <<'PY'
import os, pathlib, secrets, stat, sys
path = pathlib.Path(sys.argv[1]).expanduser()
if not path.is_absolute() or path.is_symlink():
    raise SystemExit('Preview state directory must be absolute and not a symlink')
path.mkdir(mode=0o700, parents=True, exist_ok=True)
info = path.stat()
if info.st_uid != os.getuid() or info.st_mode & 0o077:
    raise SystemExit('Existing preview state directory must be owned and mode 0700')
token = path / 'dashboard-token'
try:
    fd = os.open(token, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0), 0o600)
except FileExistsError:
    info = token.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise SystemExit('Existing dashboard token must be a private owned regular file')
else:
    with os.fdopen(fd, 'w') as stream:
        stream.write(secrets.token_urlsafe(48))
        stream.flush()
        os.fsync(stream.fileno())
print('Dashboard access token is in', token, '(value not displayed)')
PY
export CARGO_TARGET_DIR="${ADMIRALTY_TARGET_DIR:-$task_root/target}"
run pnpm --dir apps/dashboard build
task_args=(--database "$task_state/native.sqlite" --token-file "$task_state/dashboard-token"
  --listen "${ADMIRALTY_LIVE_LISTEN:-127.0.0.1:18891}" --assets "$task_root/apps/dashboard/dist"
  --legacy-status-file "$task_status" --legacy-index-file "$task_index" --legacy-public-read)
if [[ -n "$task_models" ]]; then task_args+=(--models-file "$task_models"); fi
echo 'Starting real read-only fleet view. Original page remains available at /legacy/.'
run cargo run -p admiralty-server --bin admiraltyd --locked -- "${task_args[@]}"
