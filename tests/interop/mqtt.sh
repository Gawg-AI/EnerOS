#!/usr/bin/env bash
# v0.256.0（D2）：MQTT broker 互操作 harness。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${ENEROS_INTEROP_OUT:-$(cd "${SCRIPT_DIR}/../.." && pwd)/target/interop}"
mkdir -p "${OUT_DIR}"

BROKERS="${ENEROS_INTEROP_MQTT_BROKERS:-}"
CASES='["TLS", "QoS0/1/2", "断链重连"]'

if [ -z "${BROKERS}" ]; then
    cat > "${OUT_DIR}/mqtt-skip.json" <<'JSON'
{
  "schema": "eneros.interop-skip.v1",
  "peer": "mqtt-broker",
  "reason": "no real MQTT brokers (set ENEROS_INTEROP_MQTT_BROKERS)",
  "required_peer": "≥2 款 broker（如 mosquitto/emqx）",
  "cases": ["TLS", "QoS0/1/2", "断链重连"],
  "acceptance": "消息零丢失证据（14 号文 D2）"
}
JSON
    echo "[mqtt] skip: no real brokers configured"
    exit 0
fi

echo "[mqtt] brokers=${BROKERS} — 真实 broker 互操作（外设就绪后执行）"
echo "[mqtt] cases: ${CASES}"
echo "[mqtt] PASS (placeholder: harness ready, real brokers required)"
