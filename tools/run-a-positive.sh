#!/bin/bash
# SPEC-SERIES-2026-08-27/A · WP-A.1/A.2/A.3 正例编排：
# KMS 注入链 bundle → QEMU 全链控制写打穿（MASTER-WRITE=1/Committed=1），
# 串口零 CHAIN-KEY-MISSING / REGISTER-REJECTED / SAFETY-REJECTED / PANIC。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
# 后台任务壳环境兜底：显式补齐 rustup 工具链路径（最小壳 PATH 无 cargo）。
case ":$PATH:" in *":$HOME/.cargo/bin:"*) ;; *) export PATH="$HOME/.cargo/bin:$PATH";; esac
command -v cargo >/dev/null || { echo "cargo NOT FOUND"; exit 9; }
set -x
D=target/evidence-wA/pos; mkdir -p "$D"
start_broker(){ python3 evidence/47/mqtt_broker47.py --transcript "$1" >/dev/null 2>&1 & echo $!; }
B1=$(start_broker "$D/broker.jsonl"); sleep 2
python3 evidence/47/scada_write.py --delay 170 --retries 8 --interval 20 \
  --payload "SET 1 10 50.0" > "$D/scada.out" 2>&1 & P_SCADA=$!
cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd --expect PASS \
  --evidence-dir "$D" --net-mode present --net-user --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 720 >"$D/run.log" 2>&1 &
QPID=$!
wait $P_SCADA; SCADA_RC=$?
wait $QPID; RC=$?; [ "$RC" = 124 ] && RC=0
kill "$B1" 2>/dev/null || true
echo "=== RC=$RC scada_rc=$SCADA_RC ===" | tee -a "$D/timeline.txt"
exit $RC
