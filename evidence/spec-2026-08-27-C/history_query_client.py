#!/usr/bin/env python3
"""SPEC-C（WP-C.2 · 判据 C2/C3）：SCADA 侧 history-query 客户端（纯 stdlib）。

经 MQTT 管理通道下发 MGT1 帧 action="history-query"，arg = point u32 LE ‖
last_n u32 LE；等待 mgmt_response 并按扩展帧布局解析：
    [status u8][detail ascii][payload_len u32 LE][rows×25B]
row = ts u64 ‖ device u32 ‖ point u32 ‖ value f64 ‖ quality u8（小端）。
输出 JSONL transcript（逐行事件）+ 终局 verdict 行。

仅最后 N 条快照窗口、非全量导出（README 能力边界同源口径）。
"""
import argparse
import json
import socket
import struct
import sys
import time

REQ_TOPIC = b"eneros/mgmt/request"
RESP_TOPIC = b"eneros/mgmt/response"

ROLE_CODE = {"viewer": 0, "operator": 1, "admin": 2, "superuser": 3}


def mqtt_connect(sock):
    vb = b"\x00\x04MQTT\x04" + struct.pack("!H", 60)
    payload = b"\x00\x07hqc-c1"          # client-id
    pkt = b"\x10" + _len(len(vb) + len(payload)) + vb + payload
    sock.sendall(pkt)
    hdr, body = read_packet(sock)
    assert hdr == 0x20 and body[0] == 0x00, f"CONNACK 异常 {hdr} {body!r}"


def _len(n):
    out = b""
    while True:
        b = n % 128
        n //= 128
        out += bytes([b | 0x80]) if n else bytes([b])
        if n == 0:
            return out


def read_packet(sock):
    b = sock.recv(1)
    if not b:
        return None, None
    rl, mult = 0, 1
    while True:
        e = sock.recv(1)[0]
        rl += (e & 0x7F) * mult
        if not e & 0x80:
            break
        mult *= 128
    body = b""
    while len(body) < rl:
        chunk = sock.recv(rl - len(body))
        if not chunk:
            return None, None
        body += chunk
    return b[0], body


def publish(sock, topic, payload):
    tb = topic
    body = struct.pack("!H", len(tb)) + tb + b"" + payload  # qos0 无 pid
    sock.sendall(b"\x30" + _len(len(body)) + body)


def subscribe(sock):
    body = struct.pack("!H", 1) + struct.pack("!H", len(RESP_TOPIC)) + RESP_TOPIC + b"\x00"
    sock.sendall(b"\x82" + _len(len(body) + 2) + struct.pack("!H", 1) + body)


def parse_publish(hdr, body):
    tlen = struct.unpack("!H", body[:2])[0]
    topic = body[2 : 2 + tlen]
    off = 2 + tlen
    if hdr & 0x08:
        off += 2  # qos>0 pid（本客户端只发 qos0，防御留位）
    return topic, body[off:]


def build_mgt1(request_id, action, arg=b"", role="viewer"):
    principal = b"hqc-c".ljust(16, b"\x00")
    roles = bytes([ROLE_CODE[role], 0, 0, 0])
    channel = b"mqtt:hqc-c".ljust(16, b"\x00")
    act = action.encode()
    frame = (
        b"MGT1" + principal + roles + struct.pack("<I", 1) + channel
        + struct.pack("<Q", int(time.time() * 1000) + 600_000)
        + struct.pack("<Q", request_id) + bytes([len(act)]) + act + arg
    )
    return frame


def decode_response(raw):
    """status‖detail‖[ext_len‖rows]。贪婪识别尾部扩展段。"""
    status = raw[0]
    ext_len_beams = None
    detail_end = len(raw)
    if len(raw) >= 5:
        cand = struct.unpack("<I", raw[-25 - 4 :][:4] if False else raw[-4 - 0 :]
        ) if False else None
    # 扩展段判定：存在 k 使 rows*25 == tail_len 且 header 合法。
    best = None
    for cut in range(len(raw) - 4, 0, -1):
        ln = struct.unpack("<I", raw[cut : cut + 4])[0]
        if ln > 0 and raw[cut + 4 :] and len(raw[cut + 4 :]) == ln and ln % 25 == 0:
            best = (raw[1:cut], ln, raw[cut + 4 :])
            break
    if best:
        detail, _, rows_b = best
    else:
        detail, rows_b = raw[1:], b""
    return status, detail.decode("utf-8", "replace"), rows_b


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--point", type=int, required=True)
    ap.add_argument("--last-n", type=int, default=16)
    ap.add_argument("--request-id", type=int, default=7000)
    ap.add_argument("--wait-secs", type=float, default=20)
    ap.add_argument("--transcript", default=None)
    a = ap.parse_args()
    emit = lambda e: (open(a.transcript, "a").write(json.dumps(e, ensure_ascii=False) + "\n") if a.transcript else None)

    sock = socket.create_connection(("127.0.0.1", a.port), timeout=5)
    mqtt_connect(sock)
    subscribe(sock)
    frame = build_mgt1(a.request_id, "history-query", struct.pack("<II", a.point, a.last_n))
    publish(sock, REQ_TOPIC, frame)
    emit({"ev": "publish-history-query", "request_id": a.request_id,
          "point": a.point, "last_n": a.last_n})
    deadline = time.time() + a.wait_secs
    sock.settimeout(1.0)
    while time.time() < deadline:
        try:
            hdr, body = read_packet(sock)
        except socket.timeout:
            continue
        if hdr is None:
            break
        if not 0x30 <= hdr < 0x40:
            continue
        topic, payload = parse_publish(hdr, body)
        if topic != RESP_TOPIC:
            continue
        status, detail, rows_b = decode_response(payload)
        rec = {
            "ev": "history-response",
            "status_byte": status,
            "completed": status == 1,
            "detail": detail,
            "rows": [],
        }
        for i in range(len(rows_b) // 25):
            ts, dev, pt, val, q = struct.unpack("<QIIdB", rows_b[i * 25 : (i + 1) * 25])
            rec["rows"].append(
                {"i": i, "ts_ms": ts, "device": dev, "point": pt,
                 "value": val, "quality": q}
            )
        print(json.dumps(rec, ensure_ascii=False))
        emit(rec)
        sock.close()
        verdict = "PASS" if status == 1 else ("EXPECTED-FAIL" if detail.startswith("history:") else "FAIL")
        v = {"verdict": verdict, "detail": detail}
        print(json.dumps(v))
        emit(v)
        return
    print(json.dumps({"verdict": "TIMEOUT"}))
    emit({"verdict": "TIMEOUT"})


if __name__ == "__main__":
    main()
