#!/usr/bin/env bash
# 33 WP-33.5：掉电 ≥20 次转真证据——QEMU 掉电注入 + DurableLog 恢复。
#
# 每轮：tools/qemu-powercut.sh 在 `durable-log: append` 判据行后 kill
# QEMU（cache=none/O_DIRECT → 真实断电语义）；下一轮启动 recover
# committed 单调续增 = 跨掉电持久化证据。同一数据盘贯穿全部轮次。
#
# 用法：bash tests/powercut-durable.sh --image <img> --data <data.raw>
#       [--vars <vars.fd>] [--out <dir>] [--cycles N]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# 34 GV-20（复验）：fail-closed QEMU 解析——旧实现硬编码个人主机路径，换机即碎。
if [[ -n "${ENEROS_QEMU_BIN:-}" ]]; then
    QEMU_BIN="${ENEROS_QEMU_BIN}"
elif command -v qemu-system-aarch64 >/dev/null 2>&1; then
    QEMU_BIN="$(command -v qemu-system-aarch64)"
else
    echo "qemu-system-aarch64 not found in PATH; set ENEROS_QEMU_BIN" >&2
    exit 2
fi
FIRMWARE="/usr/share/AAVMF/AAVMF_CODE.secboot.fd"
CYCLES=20
OUT_DIR=""
IMAGE=""
DATA=""
VARS="${REPO_ROOT}/target/wp335-irq-evidence/run2/VARS.fd"

while [ $# -gt 0 ]; do
  case "$1" in
    --image) IMAGE="$2"; shift 2 ;;
    --data) DATA="$2"; shift 2 ;;
    --vars) VARS="$2"; shift 2 ;;
    --out) OUT_DIR="$2"; shift 2 ;;
    --cycles) CYCLES="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[ -n "${IMAGE}" ] && [ -n "${DATA}" ] || {
  echo "require --image and --data" >&2
  exit 2
}
OUT_DIR="${OUT_DIR:-${REPO_ROOT}/target/powercut-durable}"
mkdir -p "${OUT_DIR}"

# qemu argv（与 xtask run qemu-prod 一致；无 netdev——本证据不需网络；
# 数据盘 cache=none = kill 即断电）。
run_cycle() {
  local cycle="$1"
  local log="${OUT_DIR}/cycle-${cycle}.log"
  bash "${REPO_ROOT}/tools/qemu-powercut.sh" "${log}" "durable-log: append" \
    "${QEMU_BIN}" \
    -machine virt,gic-version=2,acpi=off -smp 4 -cpu cortex-a57 -m 1G \
    -boot order=c -nographic -no-reboot \
    -drive if=pflash,format=raw,readonly=on,file="${FIRMWARE}" \
    -drive if=pflash,format=raw,file="${VARS}" \
    -drive if=none,format=raw,file="${IMAGE}",id=enerosdisk \
    -device virtio-blk-device,bus=virtio-mmio-bus.0,drive=enerosdisk \
    -object rng-random,filename=/dev/urandom,id=enerosrng \
    -device virtio-rng-device,rng=enerosrng \
    -drive if=none,format=raw,cache=none,file="${DATA}",id=enerosdata \
    -device virtio-blk-device,drive=enerosdata
}

RECOVER=()
APPEND=()
for ((c = 1; c <= CYCLES; c++)); do
  echo "[powercut] cycle ${c}/${CYCLES} — booting until durable append marker..."
  run_cycle "$c"
  recover="$(grep -a -o "durable-log: recover committed=[0-9]*" "${OUT_DIR}/cycle-${c}.log" | head -1 || true)"
  append="$(grep -a -o "durable-log: append seq=[0-9]*" "${OUT_DIR}/cycle-${c}.log" | head -1 || true)"
  echo "[powercut]   ${recover:-MISSING} / ${append:-MISSING}"
  RECOVER+=("${recover:-MISSING}")
  APPEND+=("${append:-MISSING}")
done

# 判据：每轮 recover 行存在且 committed 单调（跨掉电续增）。
miss=0
last=-1
monotonic=1
for ((i = 0; i < CYCLES; i++)); do
  r="${RECOVER[$i]#durable-log: recover committed=}"
  if [[ "${RECOVER[$i]}" == MISSING || ! "${r}" =~ ^[0-9]+$ ]]; then
    miss=$((miss + 1))
    monotonic=0
    continue
  fi
  if ((r < last)); then
    monotonic=0
  fi
  last=$r
done

python3 - "${OUT_DIR}" "${CYCLES}" "${miss}" "${monotonic}" <<'PY'
import json, sys
out_dir, cycles, miss, monotonic = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
recover, append = [], []
for c in range(1, cycles + 1):
    try:
        text = open(f"{out_dir}/cycle-{c}.log", "rb").read().decode("utf-8", "replace")
    except FileNotFoundError:
        text = ""
    import re
    r = re.search(r"durable-log: recover committed=(\d+)", text)
    a = re.search(r"durable-log: append seq=(\d+)", text)
    recover.append(int(r.group(1)) if r else None)
    append.append(int(a.group(1)) if a else None)
committed_seq = [x for x in recover if x is not None]
verdict = "PASS" if miss == 0 and monotonic == 1 and len(committed_seq) >= 2 and committed_seq == sorted(committed_seq) and committed_seq[0] == 0 else "FAIL"
summary = {
    "schema": "eneros.powercut-durable.v1",
    "cycles": cycles,
    "data_disk": "same across cycles (cache=none, kill=power cut)",
    "marker_cut": "durable-log: append",
    "recover_committed_per_cycle": recover,
    "append_seq_per_cycle": append,
    "missing_recover_lines": miss,
    "monotonic": bool(monotonic),
    "verdict": verdict,
    "acceptance": "≥20 次掉电，每次恢复判据行存在且 committed 单调续增",
}
with open(f"{out_dir}/summary.json", "w", encoding="utf-8") as f:
    json.dump(summary, f, ensure_ascii=False, indent=1)
print(f"[powercut] verdict={verdict} cycles={cycles} missing={miss} monotonic={monotonic}")
PY

if [[ "${miss}" -gt 0 ]]; then
  echo "[powercut] FAIL: ${miss} cycles missing recover 判据行" >&2
  exit 1
fi
echo "[powercut] DONE — evidence: ${OUT_DIR}/summary.json + cycle-*.log"
