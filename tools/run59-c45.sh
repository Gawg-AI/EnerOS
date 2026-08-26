#!/bin/bash
# SPEC-GAP-CLOSURE w7：C4(建联/发布) + C5(kill→恢复) 编排
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
D=target/evidence-w59/c45; mkdir -p $D
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200
start_broker(){ python3 evidence/47/mqtt_broker47.py --transcript "$1" & echo $!; }
B1=$(start_broker "$D/broker.gen1.jsonl"); sleep 2
python3 evidence/47/scada_write.py --delay 50 --retries 4 --interval 12 & P_SCADA=$!
cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd --expect PASS \
  --evidence-dir "$D" --net-mode present --net-user --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 750 >$D/run.log 2>&1 &
QPID=$!
# 等 gen1 出现首个 CONNACK → 杀 broker（制造 FIN/RST 可感死亡）。
for i in $(seq 1 90); do grep -q CONNACK $D/broker.gen1.jsonl 2>/dev/null && break; sleep 5; done
sleep 10; kill -9 $B1 2>/dev/null; KILL_T=$SECONDS; echo "broker killed @$KILL_T" > $D/timeline.txt
sleep 25; B2=$(start_broker "$D/broker.gen2.jsonl")
# 等待窗口内 gen2 收到新连接（自动恢复直证）。
for i in $(seq 1 60); do grep -q CONNACK $D/broker.gen2.jsonl 2>/dev/null && break; sleep 5; done
grep -c CONNACK $D/broker.gen2.jsonl > $D/gen2_connacks.txt 2>/dev/null || echo 0 > $D/gen2_connacks.txt
wait $QPID; RC=$?; [ "$RC" = 124 ] && RC=0
kill $P_SCADA $B2 2>/dev/null || true
echo "=== RC=$RC gen2_connack=$(cat $D/gen2_connacks.txt) ===" | tee -a $D/timeline.txt
exit $RC
