#!/bin/bash
# SPEC-SERIES-2026-08-27/A · WP-A.1 负例（缺节变体）：
# 以不含 security 节的 qemu-dev 清单 bundle 装入 qemu-test 镜像 →
# 两控制服务运行期取键不可得 ⇒ 串口 CHAIN-KEY-MISSING 拒载判定行；
# 控制写被拒（无 Committed）。执行前 MUST 已保存正例 bundle。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
D=target/evidence-wA/neg-nokey; mkdir -p "$D"
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200
export ENEROS_RELEASE_PUBLIC_KEY_HEX="8e5150375a22ae2ad55ea85a3e7a8faee8196315a413163fd4778c82c59af9a7"
export ENEROS_RELEASE_KEY_ID_HEX="4e28c578ebd8c3c68b83ff1d0b1a16df"
export ENEROS_PLATFORM_ID_HEX="6afc39cf058a1ca52a5e36d8eefa1463"
K=product/security/keys-local
# 备份 bundle + registry；临时摘除 security 条目生成无密钥节 bundle。
# （qemu-dev 清单缺 ai-peer 属预存 CF-01 红，故走 qemu-test + 摘条目路线。）
cp -f product/config-bundle.bin "$D/bundle.true.bak"
cp -f product/config-bundle-pubkey.hex "$D/bundle-pubkey.true.bak"
cp -f configs/registry.toml "$D/registry.true.bak"
trap 'cp -f "$D/bundle.true.bak" product/config-bundle.bin; cp -f "$D/bundle-pubkey.true.bak" product/config-bundle-pubkey.hex; cp -f "$D/registry.true.bak" configs/registry.toml' EXIT
python3 - "$D" <<'PY'
import re, pathlib, sys
p = pathlib.Path("configs/registry.toml")
t = p.read_text()
new = re.sub(r'\[active\."security"\][^\n]*\n(consumer[^\n]*\nnote[^\n]*\n)?', '', t, count=1)
assert "security" not in new.split("[template]")[0], "security 条目未摘除"
p.write_text(new)
print("registry security entry stripped")
PY
ENEROS_KMS_DIR=configs/kms-dev cargo +nightly-2026-04-04 xtask config compile --profile qemu-prod --root-key-hex "$(printf '5a%.0s' {1..64})" >"$D/compile-nosecurity.log" 2>&1 || { echo COMPILE_FAIL; exit 2; }
grep -q "CONFIG-COMPILE: security" "$D/compile-nosecurity.log" && { echo "UNEXPECTED: security present"; exit 3; }
cargo +nightly-2026-04-04 xtask image raw qemu-test \
  --signing-key $K/dev-signing.key --uefi-key $K/uefi-test.key --uefi-cert $K/uefi-test.pem \
  >"$D/image.log" 2>&1 || { echo IMAGE_FAIL; exit 4; }
start_broker(){ python3 evidence/47/mqtt_broker47.py --transcript "$1" >/dev/null 2>&1 & echo $!; }
B=$(start_broker "$D/broker.jsonl"); sleep 2
python3 evidence/47/scada_write.py --delay 60 --retries 3 --interval 15 \
  --payload "SET 1 10 50.0" > "$D/scada.out" 2>&1 & P_SCADA=$!
cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd --expect PASS \
  --evidence-dir "$D" --net-mode present --net-user --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 720 >"$D/run.log" 2>&1 &
QPID=$!
wait $QPID; RC=$?; [ "$RC" = 124 ] && RC=0
kill "$B" "$P_SCADA" 2>/dev/null || true
# 恢复正例 bundle/公钥/registry（防污染后续构建与 gate）。
cp -f "$D/bundle.true.bak" product/config-bundle.bin
cp -f "$D/bundle-pubkey.true.bak" product/config-bundle-pubkey.hex
cp -f "$D/registry.true.bak" configs/registry.toml
echo "=== NEG_RC=$RC ===" | tee -a "$D/timeline.txt"
exit $RC
