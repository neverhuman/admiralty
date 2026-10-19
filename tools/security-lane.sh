#!/usr/bin/env bash
set -euo pipefail
umask 077
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
exec bash scripts/check/security-supply-chain.sh
