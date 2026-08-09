#!/usr/bin/env bash
# G-R1 闸门复现（一次性环境处理）：杀残留 QEMU → 清数据盘/BOOTCTRL →
# 全零 VARS → 跑 qemu-prod → 输出 supervisor 序列摘要。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
QEMU_BIN="${ENEROS_QEMU_BIN:-/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64}"

pkill -9 -f qemu-system 2>/dev/null || true
sleep 2
python3 -c "open('/tmp/G1_VARS.fd','wb').write(b'\x00'*(64*1024*1024))"
rm -f "${REPO}/target/g1-evidence/eneros-data.raw"
cp /tmp/G1_VARS.fd "${REPO}/target/g1-evidence/VARS.fd"
# BOOTCTRL 归零（若镜像被上次运行污染——重置脚本与 boot-manager 的
# CRC 存在不一致风险，引导失败时改为全重建（factory 生成正确 BOOTCTRL））
python3 "${REPO}/tools/reset-bootctrl.py" "${REPO}/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw" >/dev/null 2>&1 || true

cd "${REPO}"
export PATH="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH"
ENEROS_QEMU_BIN="${QEMU_BIN}" cargo xtask run qemu-prod \
  --image "${REPO}/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw" \
  --vars-template "${REPO}/target/g1-evidence/VARS.fd" \
  --evidence-dir "${REPO}/target/g1-evidence" \
  --expect PASS 2>&1 | tail -3 || true

echo "=== G-R1 序列摘要 ==="
grep -cE "SUPERVISOR" "${REPO}/target/g1-evidence/qemu-serial.log" || true
grep -E "SUPERVISOR|IPC: recv hello|restart|FAULTED|vm fault|OOM|spawn plan" \
  "${REPO}/target/g1-evidence/qemu-serial.log" | head -20 || true
