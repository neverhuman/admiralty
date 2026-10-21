#!/usr/bin/env bash
# Activate an installed Admiralty release, restart its systemd units and
# require readiness. If the new version is not ready in time, roll back to
# the previous version automatically and exit 1.
#
# Preconditions (see docs/operators/deployment-systemd.md):
#   - the version was installed with `admiralty release install` into
#     <releases-root>/<version> (a private, integrity-pinned directory);
#   - the units in deploy/systemd/ (or equivalents) run
#     <releases-root>/current/bin/*.
# This script is the single deployment writer's tool. It never edits
# configuration, data, tokens or units, and never starts a second service.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/deploy/rollout.sh --releases-root /srv/admiralty/releases --version VERSION
         [--listen 127.0.0.1:18891] [--units "admiraltyd admiralty-observe"]
         [--user] [--timeout-seconds 60] [--dry-run]
USAGE
}

releases_root="" version="" listen="127.0.0.1:18891" units="admiraltyd admiralty-observe"
timeout=60 dry_run=0 user_scope=0
while (($#)); do
  case "$1" in
    --releases-root|--version|--listen|--units|--timeout-seconds)
      if (($# < 2)); then usage >&2; exit 2; fi
      case "$1" in
        --releases-root) releases_root="$2" ;;
        --version) version="$2" ;;
        --listen) listen="$2" ;;
        --units) units="$2" ;;
        --timeout-seconds) timeout="$2" ;;
      esac
      shift 2 ;;
    --user) user_scope=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
if [[ "$releases_root" != /* || -z "$version" || "$version" == */* ]]; then
  usage >&2
  exit 2
fi
if [[ ! "$timeout" =~ ^[0-9]+$ ]]; then
  echo 'rollout: --timeout-seconds must be a whole number' >&2
  exit 2
fi
candidate="$releases_root/$version/bin/admiralty"
if [[ ! -x "$candidate" || -L "$releases_root/$version" ]]; then
  echo "rollout: $releases_root/$version is not an installed release directory" >&2
  exit 2
fi
read -r -a unit_list <<<"$units"
current_cli() { printf '%s/current/bin/admiralty' "$releases_root"; }
restart_units() {
  # Bash 3.2 treats an empty array expansion as unbound under set -u.
  if ((user_scope)); then
    systemctl --user restart "${unit_list[@]}"
  else
    systemctl restart "${unit_list[@]}"
  fi
}

if ((dry_run)); then
  echo "rollout (dry run): would activate $version, restart ${unit_list[*]} and require $listen/readyz"
  exit 0
fi

"$candidate" release activate --releases-root "$releases_root" --version "$version"
restart_units
if "$(current_cli)" ready --listen "$listen" --timeout-seconds "$timeout"; then
  echo "rollout: $version is active and ready"
  exit 0
fi

echo "rollout: $version did not become ready within ${timeout}s; rolling back" >&2
"$(current_cli)" release rollback --releases-root "$releases_root"
restart_units
if ! "$(current_cli)" ready --listen "$listen" --timeout-seconds "$timeout"; then
  echo 'rollout: the restored version is not ready either; inspect `journalctl -u admiraltyd` and `admiralty logs digest`' >&2
fi
exit 1
