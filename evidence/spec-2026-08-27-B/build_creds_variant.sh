#!/usr/bin/env bash
# SPEC-B 凭据变体构建：临时启用 protocol-mqtt.toml 的 username/password，
# 走真实 config compile（签名 bundle）→ 重打包 qemu-prod 介质。
# 用法：build_creds_variant.sh on|off
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
MODE=$1
TOML=configs/protocol-mqtt.toml

if [[ "$MODE" == "on" ]]; then
  python3 - "$TOML" <<'EOF'
import sys
p=sys.argv[1]; s=open(p).read()
if "username =" not in s.replace("# username",""):
    s=s.replace('# username = "eneros-agent"','username = "eneros-agent"')
    s=s.replace('# password = "change-me"','password = "specb-pass"')
else:
    print("creds already on")
open(p,"w").write(s)
EOF
elif [[ "$MODE" != "off" ]]; then
  echo "usage: $0 on|off" >&2; exit 2
fi

if [[ "$MODE" == "off" ]]; then
  python3 - "$TOML" <<'EOF'
import sys,re
p=sys.argv[1]; s=open(p).read()
s=re.sub(r'^username = .*$','# username = "eneros-agent"', s, flags=re.M)
s=re.sub(r'^password = .*$', '# password = "change-me"',   s, flags=re.M)
open(p,"w").write(s)
EOF
fi

ENEROS_KMS_DIR="$ROOT/configs/kms-dev" SOURCE_DATE_EPOCH=1756243200 cargo xtask config compile --profile qemu-prod --root-key-hex "$(printf '5a%.0s' {1..64})"
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$PATH"
SOURCE_DATE_EPOCH=1756243200 cargo xtask image raw qemu-prod \
  --signing-key product/security/keys-local/dev-signing.key \
  --uefi-key product/security/keys-local/uefi-test.key \
  --uefi-cert product/security/keys-local/uefi-test.pem 2>&1 | tail -1
