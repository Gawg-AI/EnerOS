#!/usr/bin/env bash
# 生成 SM2 第三方互操作向量（21-B C2）——对端 = OpenSSL 3（vetted 系统库）。
#
# 每组向量：
#   d       私钥标量（openssl genpkey 生成的 SM2 密钥，pkey -text 导出）
#   point   未压缩公钥点 04||X||Y（openssl pkey -pubout -outform DER 解析）
#   message 固定语义消息（能源域自然语言片段）
#   sig_der openssl pkeyutl -sign（SM2-SM3，distid 默认 1234567812345678）
#
# 输出：tests/vectors/sm2-interop/vectors.json + vectors.json.sha256（锚定）。
# 纪律：向量必须由 openssl 生成（禁止自己生成自己验）；脚本失败即退出非零。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="$ROOT/tests/vectors/sm2-interop"
OUT="$OUT_DIR/vectors.json"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

MESSAGES=(
  "谷电时段充电至 80%"
  "峰段放电 30kWh 参与需求响应"
  "负载率超过 85% 时切除非关键负荷"
  "SOC 低于 0.20 禁止放电"
  "上报昨日 96 点遥测数据"
  "切换至备用电源并通知运维"
  "订阅实时电价并按 0.5 元/度刷新策略"
  "厂区光伏出力预测 12kW 持续 3 小时"
  "执行变压器温度越限告警"
  "同步结算账本至边缘节点"
)
[ "${#MESSAGES[@]}" -ge 10 ] || { echo "need >=10 messages" >&2; exit 1; }

python3 - "$WORK" "$OUT" "${MESSAGES[@]}" <<'PY'
import json
import subprocess
import sys
import os

work, out, *messages = sys.argv[1:]
entries = []

for i, msg in enumerate(messages):
    key = os.path.join(work, f"sm2-{i}.key")
    # 1) openssl 生成 SM2 密钥（vetted 对端）
    subprocess.run(["openssl", "ecparam", "-name", "sm2", "-genkey", "-noout", "-out", key],
                   check=True, capture_output=True)
    # 2) 导出私钥标量 d（pkey -text 输出 "priv:" 十六进制块）
    text = subprocess.run(["openssl", "pkey", "-in", key, "-text", "-noout"],
                          check=True, capture_output=True, text=True).stdout
    priv_hex = ""
    in_priv = False
    for line in text.splitlines():
        if line.strip() == "priv:":
            in_priv = True
            continue
        if in_priv:
            if line.strip() == "pub:":
                break
            priv_hex += "".join(line.split()).replace(":", "")
    assert len(priv_hex) == 64, f"unexpected priv length {len(priv_hex)}"
    # 3) 导出公钥点（SPKI DER → 解析 04||X||Y）
    pub_der = subprocess.run(["openssl", "pkey", "-in", key, "-pubout", "-outform", "DER"],
                             check=True, capture_output=True).stdout
    idx = pub_der.find(b"\x03\x42\x00")
    assert idx >= 0, "SPKI BIT STRING marker not found"
    point = pub_der[idx + 3: idx + 3 + 65]
    assert len(point) == 65 and point[0] == 4, "bad point encoding"
    # 4) 消息写文件 + openssl 签名（SM2-SM3；distid 必须显式——
    #    OpenSSL 3.5 SM2 provider 无默认 distid，未设置时 Z 不含用户 ID，
    #    与 GB/T 默认 "1234567812345678" 不互认（2026-08-09 实测））
    msg_file = os.path.join(work, f"msg-{i}.bin")
    with open(msg_file, "wb") as f:
        f.write(msg.encode("utf-8"))
    sig_file = os.path.join(work, f"sig-{i}.der")
    subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", key,
                    "-in", msg_file, "-rawin", "-digest", "sm3",
                    "-pkeyopt", "distid:1234567812345678", "-out", sig_file],
                   check=True, capture_output=True)
    sig_der = open(sig_file, "rb").read()
    entries.append({
        "id": f"ossl-sm2-{i:02d}",
        "d": priv_hex,
        "point": point.hex(),
        "message": msg,
        "sig_der": sig_der.hex(),
    })

doc = {
    "schema": "eneros.sm2-interop-vectors.v1",
    "generator": "openssl 3 pkeyutl (SM2-SM3, distid=1234567812345678)",
    "vector_count": len(entries),
    "vectors": entries,
}
with open(out, "w") as f:
    json.dump(doc, f, indent=1, ensure_ascii=False)
    f.write("\n")
print(f"wrote {len(entries)} vectors to {out}")
PY

cd "$ROOT"
# sha256sum -c 需要 "<hash>  <filename>" 格式（--strict 校验）。
(cd "$OUT_DIR" && sha256sum vectors.json > vectors.json.sha256)
echo "anchor: $(cat "$OUT_DIR/vectors.json.sha256")"
echo "verify: $(cd "$OUT_DIR" && sha256sum -c vectors.json.sha256 --strict)"
