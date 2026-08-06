#!/usr/bin/env bash
set -euo pipefail

# This is the only accepted product boot-test entry point.  It delegates to the
# fail-closed harness, which boots a signed UEFI application from the raw GPT
# disk and rejects QEMU loader, -kernel, timeout, and keyword-only success.
: "${ENEROS_QEMU_IMAGE:?set ENEROS_QEMU_IMAGE to the qemu-prod raw GPT image}"
: "${ENEROS_QEMU_VARS_TEMPLATE:?set ENEROS_QEMU_VARS_TEMPLATE to enrolled Secure Boot vars}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

cargo xtask run qemu-prod \
    --image "${ENEROS_QEMU_IMAGE}" \
    --vars-template "${ENEROS_QEMU_VARS_TEMPLATE}" \
    --expect PASS
