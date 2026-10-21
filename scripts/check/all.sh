#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/../.." && pwd -P)"
cd "$task_root"

# Match CI's runtime/profile and keep workstation-wide targets out of this run.
export CARGO_TARGET_DIR="${ADMIRALTY_TARGET_DIR:-$task_root/target}"
case "$CARGO_TARGET_DIR" in /*) ;; *) echo 'ADMIRALTY_TARGET_DIR must be absolute' >&2; exit 1 ;; esac
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_TEST_THREADS=1
if [[ "$(node -p 'process.versions.node.split(".")[0]')" != 24 ]]; then
  echo 'Local CI requires Node 24, matching .github/workflows/ci.yml.' >&2
  exit 1
fi
if [[ "$(pnpm --version)" != 11.15.1 ]]; then
  echo 'Local CI requires pnpm 11.15.1, matching package.json.' >&2
  exit 1
fi

# Short canonical scratch avoids Unix-socket path limits and shared ancestors.
# If /tmp/.git exists on a shared host, use /var/tmp so scratch stays outside Git.
# Restore the caller's umask so private-at-creation fixtures remain exercised.
caller_umask="$(umask)"
umask 077
scratch_base="/tmp"
if [[ -e /tmp/.git ]] && [[ -d /var/tmp ]] && [[ ! -e /var/tmp/.git ]]; then
  scratch_base="/var/tmp"
fi
check_scratch="$(mktemp -d "${scratch_base}/adm.XXXXXX")"
check_scratch="$(cd "$check_scratch" && pwd -P)"
mkdir "$check_scratch/logs" "$check_scratch/canary"
umask "$caller_umask"
export TMPDIR="$check_scratch"
printf 'Local CI logs: %s/logs\n' "$check_scratch"
{
  date -u '+started_at_utc=%Y-%m-%dT%H:%M:%SZ'
  git rev-parse HEAD
  git status --short
  rustc --version
  node --version
  pnpm --version
  printf 'target=%s build_jobs=%s test_threads=%s caller_umask=%s\n' "$CARGO_TARGET_DIR" "$CARGO_BUILD_JOBS" "$RUST_TEST_THREADS" "$caller_umask"
} > "$check_scratch/logs/environment.log"
stage=0
run() {
  stage=$((stage + 1))
  local check_log result
  check_log="$(printf '%s/logs/%02d.log' "$check_scratch" "$stage")"
  printf '%02d ' "$stage"
  printf '%q ' "$@"
  printf '\n'
  printf '%q ' "$@" >> "$check_scratch/logs/commands.log"
  printf '\n' >> "$check_scratch/logs/commands.log"
  if { if command -v rtk >/dev/null 2>&1; then rtk proxy "$@"; else "$@"; fi; } > "$check_log" 2>&1; then
    printf '%02d PASS %s\n' "$stage" "$check_log"
  else
    result=$?
    printf '%02d FAIL exit=%s log=%s\n' "$stage" "$result" "$check_log" >&2
    tail -n 80 "$check_log" >&2
    return "$result"
  fi
}

run pnpm install --frozen-lockfile --ignore-scripts
run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
run cargo test --workspace --locked
run python3 -m unittest discover -s tests/legacy -v
run python3 -m unittest discover -s tests -p 'test_sc_audit*.py' -v
run python3 -m unittest discover -s tests -p 'test_native_queue_health.py' -v
run python3 tests/fixtures/legacy/verify.py
run python3 scripts/release/package.py --self-test
run python3 scripts/check/canary-smoke.py --self-test
run cargo build -p admiralty-server -p admiralty-cli -p admiralty-observe --locked
run python3 scripts/check/canary-smoke.py --admiraltyd-binary "$CARGO_TARGET_DIR/debug/admiraltyd" --admiralty-binary "$CARGO_TARGET_DIR/debug/admiralty" --scratch-root "$check_scratch/canary"
run cargo run -p admiralty-cli --locked -- contracts check
run pnpm contracts:check
run pnpm --dir packages/contracts test
run pnpm --dir apps/dashboard typecheck
run pnpm --dir apps/dashboard test --run
run pnpm --dir apps/dashboard exec tsc -p e2e/tsconfig.json --noEmit
run env NODE_ENV=production pnpm --dir apps/dashboard build

export ADMIRALTY_E2E_ADMIRALTYD_BIN="$CARGO_TARGET_DIR/debug/admiraltyd"
export ADMIRALTYD_BINARY="$ADMIRALTY_E2E_ADMIRALTYD_BIN"
export ADMIRALTY_E2E_OBSERVER_BIN="$CARGO_TARGET_DIR/debug/admiralty-observe"
export ADMIRALTY_E2E_ADMIRALTYD_SHA256
ADMIRALTY_E2E_ADMIRALTYD_SHA256="$(node -e 'process.stdout.write(require("node:crypto").createHash("sha256").update(require("node:fs").readFileSync(process.argv[1])).digest("hex"))' "$ADMIRALTY_E2E_ADMIRALTYD_BIN")"
run node -e 'const fs=require("node:fs"),crypto=require("node:crypto");if(crypto.createHash("sha256").update(fs.readFileSync("node_modules/axe-core/axe.min.js")).digest("hex")!=="20c09fe157a8a34a30e241aaa1fcdade657734f08ab379ecfbeb7d45cc46e878")process.exit(1)'
run pnpm --dir apps/dashboard test --run --config e2e/vitest.config.ts
printf 'PASS: all %s local CI stages; logs retained in %s/logs\n' "$stage" "$check_scratch"
