#!/usr/bin/env bash
# Enforced scope: Cargo.lock/pnpm-lock.yaml integrity, cargo metadata --locked,
# workflow permissions & checkout credential persistence, actionlint (.github/actionlint.yaml),
# gitleaks git range/merge-diff scan, and SHA-256 provenance digest in a fresh 0700 private directory.
# Note: cargo audit, pnpm audit, pip-audit, and grype are NOT claimed by this script and remain
# open in receipts/integration/jankurai-triage-20261019.json until provisioned on all LAN runners.
set -euo pipefail
umask 077
if [[ "${ADMIRALTY_SECURITY_NO_DEFAULT_PATH:-0}" != "1" ]]; then
  export PATH="${HOME}/.cargo/bin:${HOME}/.local/bin:${PATH}"
fi
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

# Validate scratch before querying dependencies; this denial is independently testable.
scratch_parent="${TMPDIR:-/tmp}"
if [[ -L "${scratch_parent}" ]]; then
  echo "[security-supply-chain] FAIL: scratch parent '${scratch_parent}' must not be a symlink" >&2
  exit 1
fi

for req_tool in cargo git actionlint gitleaks; do
  if ! command -v "${req_tool}" >/dev/null 2>&1; then
    echo "[security-supply-chain] FAIL: required tool '${req_tool}' is missing from PATH (no missing-tool skip allowed)" >&2
    exit 1
  fi
done
if command -v sha256sum >/dev/null 2>&1; then
  sha256_cmd=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
  sha256_cmd=(shasum -a 256)
else
  echo "[security-supply-chain] FAIL: required tool 'sha256sum' (or 'shasum') is missing from PATH (no missing-tool skip allowed)" >&2
  exit 1
fi

dir_mode_octal() {
  local target="$1"
  stat -c %a "${target}" 2>/dev/null || stat -f %Lp "${target}" 2>/dev/null || echo "unknown"
}

echo "[security-supply-chain] 1. Checking Cargo.lock and pnpm-lock.yaml integrity..."
test -s Cargo.lock
test -s pnpm-lock.yaml
cargo metadata --locked --format-version 1 > /dev/null

echo "[security-supply-chain] 2. Checking GitHub Actions workflow permissions, checkout credential persistence, actionlint, and gitleaks..."
for wf in .github/workflows/*.yml; do
  grep -q "permissions:" "${wf}"
  grep -q "persist-credentials: false" "${wf}"
done
actionlint -config-file .github/actionlint.yaml
# GitHub test merges have no patch under plain `git log -1 -p`. Scan the
# complete trusted event range, including merge-parent diffs. Without an event
# base (for example local/manual runs), inspect the latest commit's merge diffs.
scan_log_opts="-1 -m"
scan_base="${ADMIRALTY_SECURITY_SCAN_BASE:-}"
if [[ -n "$scan_base" ]]; then
  if [[ ! "$scan_base" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
    echo "[security-supply-chain] FAIL: scan base must be a complete Git object ID" >&2
    exit 1
  fi
  if [[ "$scan_base" =~ ^0+$ ]]; then
    # First push of a ref has no predecessor; inspect its complete reachable history.
    scan_log_opts="HEAD -m"
  else
    if ! git cat-file -e "$scan_base^{commit}" || ! git merge-base --is-ancestor "$scan_base" HEAD; then
      echo "[security-supply-chain] FAIL: scan base must be an available ancestor commit" >&2
      exit 1
    fi
    scan_log_opts="$scan_base..HEAD -m"
  fi
fi
gitleaks git --no-banner --redact --log-opts="$scan_log_opts" .

echo "[security-supply-chain] 3. Emitting deterministic dependency & provenance digest receipt..."
out_dir="$(mktemp -d "${scratch_parent%/}/admiralty-sec-supply-chain.XXXXXX")"
if [[ -L "${out_dir}" ]] || [[ "$(dir_mode_octal "${out_dir}")" != "700" ]]; then
  echo "[security-supply-chain] FAIL: output directory '${out_dir}' is not a private 0700 non-symlink directory" >&2
  exit 1
fi
"${sha256_cmd[@]}" Cargo.lock pnpm-lock.yaml .github/workflows/*.yml > "${out_dir}/supply-chain-digests.sha256"
echo "[security-supply-chain] PASS (${out_dir}/supply-chain-digests.sha256)"
