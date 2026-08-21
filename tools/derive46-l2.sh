#!/bin/bash
# 46.5：从 L2 对端 transcript + 串口生成每协议 l2-<proto>-verified.json。
#
# 判据（43 卷 L2 四件套）：
#   1) QEMU 退出码（run46-l2.sh 已处理；timeout=拿到证据窗口）
#   2) 串口 READY（[svc-*] NET-TRANSPORT READY 或 capability 描述行）
#   3) SELFCHECK（svc-storage SELFCHECK 等；协议侧经 capability）
#   4) 对端 transcript ≥3 事务（剔除 ERROR/LISTEN 单事件）
#
# 用法：tools/derive46-l2.sh <git-commit> [--verify-dir <dir>]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
COMMIT="${1:?git-commit}"
EVID="$ROOT/evidence/46/qemu-l2"
SERIAL="$ROOT/target/evidence46/qemu-l2/qemu-serial.log"
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# proto → (transcript 文件名, 串口 READY 关键字)
# 串口行格式 `[svc-iec104] NET-TRANSPORT READY capability=...`——关键字须含
# 右方括号以精确匹配该服务（46.5 实跑：漏 `]` → ready=0 误判 PENDING）。
declare -A TRANS=(
  [modbus-tcp]="modbus.transcript.jsonl:svc-modbus-tcp] NET-TRANSPORT READY"
  [iec104]="iec104.transcript.jsonl:svc-iec104] NET-TRANSPORT READY"
  [mqtt]="mqtt.transcript.jsonl:svc-mqtt] NET-TRANSPORT READY"
  [mms]="mms.transcript.jsonl:svc-mms] NET-TRANSPORT READY"
  [goose]="goose.transcript.jsonl:svc-goose] NET-TRANSPORT READY"
  [sv]="sv.transcript.jsonl:svc-sv] NET-TRANSPORT READY"
)

for proto in "${!TRANS[@]}"; do
  tfile="${TRANS[$proto]%%:*}"; ready_kw="${TRANS[$proto]#*:}"
  tp="$EVID/$tfile"
  [ -f "$tp" ] || { echo "SKIP $proto: transcript 缺失"; continue; }
  # 事务数：剔除 LISTEN/ERROR（单事件非事务）；计数其余（CONNECT/PUBLISH/READ_RESP…）。
  # modbus 对端（evidence/45）用 "event" 键，其余对端用 "kind"——两者都计；
  # iec104 对端记 I_FRAME / INTERROGATION+DATA（每次总召唤即一事务）。
  n=$(grep -cE '"(kind|event)": "(CONNECT|STARTDT_CON|INTERROGATION|I_FRAME|INTERROGATION\+DATA|PUBLISH|PUBACK|SUBACK|PINGRESP|CC_SENT|AARE|READ_RESP|TX_GOOSE|RX_GOOSE|TX_SV|RX_FRAME|RESPONSE|REQUEST|CONNACK|DONE)"' "$tp" || true)
  serial_hit=$(grep -c "$ready_kw" "$SERIAL" 2>/dev/null || true)
  if [ "$n" -ge 3 ] && [ "$serial_hit" -ge 1 ]; then
    cat > "evidence/46/l2-$proto-verified.json" <<EOF
{
  "schema": "eneros.l2-verified.v1",
  "protocol": "$proto",
  "level": "L2",
  "verdict": "VERIFIED",
  "generated_at": "$NOW",
  "commit": "$COMMIT",
  "oracle": "独立非 EnerOS 进程（python/system tool）+ QEMU/EL0；transcript ≥3 事务 + 串口 NET-TRANSPORT READY",
  "transcript": "evidence/46/qemu-l2/$tfile",
  "transactions": $n,
  "serial_ready_marker": $serial_hit
}
EOF
    echo "VERIFIED $proto (tx=$n ready=$serial_hit)"
  else
    echo "NOT-VERIFIED $proto (tx=$n ready=$serial_hit) —— 如实登记 PENDING"
  fi
done

# Modbus RTU —— 46.1 决策 b：QEMU 无第二串口，串口移交 BLOCKED（真机 51 卷）。
cat > evidence/46/l2-modbus-rtu-blocked.json <<EOF
{
  "schema": "eneros.l2-verified.v1",
  "protocol": "modbus-rtu",
  "level": "L0",
  "verdict": "BLOCKED",
  "generated_at": "$NOW",
  "commit": "$COMMIT",
  "reason": "46.1 决策 b：QEMU virt 无第二 PL011（-device help 实测）；Serial 句柄移交须真机/板卡 bring-up（51 卷），QEMU 下 X3/X4/X5=0 → 能力壳 fail-closed",
  "interop_level": "CodecReady"
}
EOF
echo "BLOCKED modbus-rtu（如实登记，真机 51 卷解锁）"
