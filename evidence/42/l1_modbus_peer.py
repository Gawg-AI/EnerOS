#!/usr/bin/env python3
"""42（WP-42.7/42.8）：独立 raw Modbus TCP peer 宿主互操作证据。

对端按 Modbus TCP 规范（MBAP 头 + PDU）自行编帧/解帧，不引用任何
EnerOS 帧代码——与服务器端是真正独立实现（spec 级交叉验证）。
- 正例：读保持寄存器(FC3) 回读种子值；
- 负例：非法功能码(FC2)→0x01；越界地址→0x02；数量=0→0x03；
  MBAP 协议 ID 错误→连接关闭；未授权写(FC6)→默认拒绝(非法功能)；
- 抓包：全部请求/响应字节写入 transcript（供 verifier capture 语义）。

用法：l1_modbus_peer.py <server-addr> <out-dir>
"""
import json
import socket
import struct
import sys
import time


def build_frame(txn: int, unit: int, pdu: bytes) -> bytes:
    mbap = struct.pack(">HHH", txn, 0, len(pdu) + 1)  # transaction/protocol/length
    return mbap + bytes([unit]) + pdu


def read_response(sock, txn_expected):
    header = b""
    while len(header) < 7:
        chunk = sock.recv(7 - len(header))
        if not chunk:
            return None
        header += chunk
    txn, proto, length = struct.unpack(">HHH", header[:6])
    body = b""
    while len(body) < length - 1:
        chunk = sock.recv(length - 1 - len(body))
        if not chunk:
            return None
        body += chunk
    return txn, proto, header[6:7] + body


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: l1_modbus_peer.py <host:port> <out-dir>")
    host, port = sys.argv[1].rsplit(":", 1)
    port = int(port)
    out_dir = sys.argv[2]
    os.makedirs(out_dir, exist_ok=True) if (os := __import__("os")) else None
    transcript = []
    transactions = []
    negatives = []

    def exchange(txn, unit, pdu, label):
        with socket.create_connection((host, port), timeout=5) as sock:
            sock.settimeout(5)
            req = build_frame(txn, unit, pdu)
            sock.sendall(req)
            # read_response 返回 (txn, proto, body)；body = unit + PDU。
            resp = read_response(sock, txn)
            body = resp[2] if resp else None
            transcript.append({"txn": txn, "req": req.hex(), "resp": body.hex() if body else None})
            return body

    # 正例：读保持寄存器 0..2 → 0x1111/0x2222/0x3333。
    resp = exchange(1, 1, bytes([0x03, 0x00, 0x00, 0x00, 0x03]), "read-holding")
    assert resp and resp[1] == 0x03 and resp[0] == 1, f"read-holding failed: {resp}"
    values = struct.unpack(">3H", resp[3:9])
    assert values == (0x1111, 0x2222, 0x3333), f"readback mismatch: {values}"
    transactions.append({"id": "read-holding-registers", "pdu": "03 00 00 00 03", "expected": "0x1111,0x2222,0x3333", "observed": [hex(v) for v in values]})

    # 负例：非法功能码 FC2 → exception 0x01。
    resp = exchange(2, 1, bytes([0x02, 0x00, 0x00, 0x00, 0x01]), "illegal-function")
    assert resp and resp[1] == 0x82 and resp[2] == 0x01, f"illegal-function: {resp}"
    negatives.append({"id": "illegal-function-code", "observed": "0x82 0x01 IllegalFunction"})

    # 负例：越界地址（16 个寄存器之外）→ 0x02。
    resp = exchange(3, 1, bytes([0x03, 0x00, 0x14, 0x00, 0x01]), "illegal-address")
    assert resp and resp[1] == 0x83 and resp[2] == 0x02, f"illegal-address: {resp}"
    negatives.append({"id": "illegal-data-address", "observed": "0x83 0x02 IllegalDataAddress"})

    # 负例：数量=0 → 0x03。
    resp = exchange(4, 1, bytes([0x03, 0x00, 0x00, 0x00, 0x00]), "illegal-value")
    assert resp and resp[1] == 0x83 and resp[2] == 0x03, f"illegal-value: {resp}"
    negatives.append({"id": "illegal-data-value", "observed": "0x83 0x03 IllegalDataValue"})

    # 负例：写寄存器(FC6) 无授权 → 默认拒绝（0x86 + 0x01）。
    resp = exchange(5, 1, bytes([0x06, 0x00, 0x00, 0x12, 0x34]), "write-default-deny")
    assert resp and resp[1] == 0x86 and resp[2] == 0x01, f"write-default-deny: {resp}"
    negatives.append({"id": "write-default-deny", "observed": "0x86 0x01 illegal-function (AuthorizedWriter 缺授权默认拒绝)"})

    # 负例：MBAP 协议 ID 错误 → 服务器应关闭连接（fail-closed）。
    with socket.create_connection((host, port), timeout=5) as sock:
        sock.settimeout(5)
        bad = struct.pack(">HHH", 6, 0x00FF, 3) + bytes([1, 0x03, 0x00])
        sock.sendall(bad)
        time.sleep(0.3)
        try:
            data = sock.recv(64)
            assert data == b"", f"malformed-mbap: expected close, got {data!r}"
        except (ConnectionResetError, ConnectionAbortedError):
            data = b""
        negatives.append({"id": "malformed-mbap-proto", "observed": "connection closed (fail-closed)"})
        transcript.append({"txn": 6, "req": bad.hex(), "resp": ""})

    report = {
        "schema": "eneros.interop-result.v1",
        "verdict": "PASS",
        "peer": "independent-raw-modbus-tcp-master",
        "peer_identity": "spec-level raw Modbus TCP peer (self-built MBAP+PDU, no EnerOS framing code)",
        "version": "MODBUS-TCP RFC-style (MBAP+PDU), run 2026-08-17",
        "transactions": transactions,
        "negative_cases": negatives,
        "capture": f"{out_dir}/modbus-transcript.json",
        "oracle": "raw peer readback matches seeded registers; exceptions match Modbus spec; write default-denied without AuthorizedCommand",
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    os.makedirs(out_dir, exist_ok=True)
    with open(f"{out_dir}/modbus-transcript.json", "w") as f:
        json.dump(transcript, f, indent=1)
    result_path = f"{out_dir}/modbus-result.json"
    with open(result_path, "w") as f:
        json.dump(report, f, indent=1)
    print(json.dumps(report, indent=1))
    print(f"MODBUS-L1-PEER PASS → {result_path}")


if __name__ == "__main__":
    main()