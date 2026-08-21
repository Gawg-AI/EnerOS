#!/usr/bin/env bash
# 47（WP-47.3 私钥出仓）：生成本地测试密钥（qemu-test 用；永不入库）。
#
# 产出去向：product/security/keys-local/（.gitignore；key-policy.json 的
# private_keys_in_repository:false 从此为真）。
# 生产键：HSM/离线仪式（key-policy.json production_note；release 构建
# 必须 --sign-key / ENEROS_SIGN_KEY / KMS_DIR——无键 fail-closed）。
set -euo pipefail
cd "$(dirname "$0")/.."

DIR="product/security/keys-local"
mkdir -p "$DIR"

if [[ ! -f "$DIR/dev-signing.key" ]]; then
  # Ed25519 32B 原始私钥（xtask evidence/catalog 签章线）。
  openssl genpkey -algorithm ed25519 -out "$DIR/dev-signing.key.der" >/dev/null 2>&1
  openssl pkey -in "$DIR/dev-signing.key.der" -outform DER 2>/dev/null \
    | tail -c 32 > "$DIR/dev-signing.key"
  rm -f "$DIR/dev-signing.key.der"
  echo "generated $DIR/dev-signing.key (Ed25519 32B)"
else
  echo "skip $DIR/dev-signing.key (exists)"
fi

if [[ ! -f "$DIR/uefi-test.key" || ! -f "$DIR/uefi-test.pem" ]]; then
  # UEFI 测试键：RSA-3072 Authenticode（镜像厂/secure-boot 验证线）。
  openssl req -x509 -newkey rsa:3072 -nodes \
    -keyout "$DIR/uefi-test.key" -out "$DIR/uefi-test.pem" \
    -days 3650 -subj "/CN=EnerOS-UEFI-Test-Key/OU=test/O=EnerOS" >/dev/null 2>&1
  echo "generated $DIR/uefi-test.key + uefi-test.pem (RSA-3072)"
else
  echo "skip $DIR/uefi-test.{key,pem} (exists)"
fi

echo "keys-local ready: $(ls "$DIR")"
echo "注入方式：--sign-key $DIR/dev-signing.key 或 ENEROS_SIGN_KEY=$DIR/dev-signing.key"
