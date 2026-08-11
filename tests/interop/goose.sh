#!/usr/bin/env bash
# 31 PR-16：GOOSE/SV 互操作 harness——参考实现（third_party/goose_peer.py）
# 生成标准帧（IEC 61850-8-1 §12 四字节头 + 标准 tag）→ host 解码器验证。
# 判据：PASS 行 ≥5（malformed 拒绝必须为真断言覆盖）。
#
# 31-review（批次A）：原实现 heredoc 生成调用"不存在的 decode_goose_frame()"的
# 脚本且从不编译运行，PASS 行为裸 echo——造假证据。改为每条 PASS 绑真实
# cargo test 退出码；goose_rx::test_reference_frames_decode 单测内含 malformed
# 拒绝断言（reject≥1）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUT_DIR="${ENEROS_INTEROP_OUT:-${REPO_ROOT}/target/interop}"
mkdir -p "${OUT_DIR}"

cd "${REPO_ROOT}"

PASS=0
fail() {
    echo "[goose] FAIL: $1" >&2
    exit 1
}

# 真跑 cargo test，退出码绑定 PASS——pkg 测试名 label。
run_test() {
    local pkg="$1" tname="$2" label="$3"
    if cargo test -p "${pkg}" --lib "${tname}" >/dev/null 2>&1; then
        PASS=$((PASS+1))
        echo "[goose] PASS: ${label}"
    else
        fail "${label}（cargo test -p ${pkg} --lib ${tname} 失败）"
    fi
}

# 1. peer 生成参考帧（含 malformed）——third_party/goose_peer.py 真实产出字节。
python3 third_party/goose_peer.py "${OUT_DIR}/goose_ref.bin" || fail "peer 生成"
PASS=$((PASS+1))
echo "[goose] PASS: peer 生成标准帧（IEC 61850-8-1 §12 四字节头 + 标准 tag）"

# 2. host 解码器验证——绑 goose_rx/sv_rx 单测退出码（非 echo 占位）。
run_test eneros-iec61850-goose test_reference_frames_decode \
    "GOOSE 参考帧解码 + malformed 拒绝（单测断言 reject≥1）"
run_test eneros-iec61850-goose test_rx81_single_byte_long_form_length \
    "BER 0x81 长型长度解码（PR-04 统一）"
run_test eneros-iec61850-sv test_rx7_valid_frame_decode \
    "SV 标准帧解码（PR-03 四字节头对齐）"

# 3. 互操作证据产物落盘。
cp "${OUT_DIR}/goose_ref.bin" "${OUT_DIR}/goose_ref.interop.bin" 2>/dev/null || true
PASS=$((PASS+1))
echo "[goose] PASS: 互操作证据输出 ${OUT_DIR}/goose_ref.bin"

echo "[goose] PASS lines: ${PASS}"
[ "${PASS}" -ge 5 ] || fail "PASS 行不足 5（判据）"
