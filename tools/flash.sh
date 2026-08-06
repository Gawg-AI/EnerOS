#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' \
  'tools/flash.sh is retired and cannot write media or start QEMU.' \
  'Use the single fail-closed product entry: cargo xtask flash <list|write> ...' >&2
exit 64
