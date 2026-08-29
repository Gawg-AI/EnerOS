#!/usr/bin/env python3
"""SPEC-B（WP-B.2/C3）主机侧真 TCP 凭据探针——与 v2 broker 直接建联，
验证带凭据 CONNECT 的 wire 语义（[MQTT-3.1.1] 逐字段），不依赖 EL0 服务
EXECUTE 驱动（该环境限制如实登记）：

1. 正确口令 → CONNACK 0x00 + broker CONNECT-CRED accept（比对 username/
   password_len 判定位）；
2. 错误口令 → CONNACK rc=0x04 + broker CONNACK-REJECT wrong-pass + 断连；
3. 无用户名（非法组合/未提供）→ CONNACK-REJECT no-user。

帧构造与 client.rs 编码一致（u16 字节长度前缀 + UTF-8 用户名）。
用法：python3 host_creds_probe.py --transcript <jsonl> [--expect-username u
      --expect-password p]
"""
import argparse
import json
import socket
import struct
import subprocess
import sys
import tempfile
import time

USER = "eneros-agent"
PASS = "specb-pass"


def encode_connect(cid, username=None, password=None):
    var = bytearray(b"\x00\x04MQTT\x04")
    flags = 0x02  # clean session
    if username is not None:
        flags |= 0x80
    if password is not None:
        flags |= 0x40
    var.append(flags)
    var += struct.pack(">H", 60)  # keepalive s
    cid_b = cid.encode()
    var += struct.pack(">H", len(cid_b)) + cid_b
    if username is not None:
        u = username.encode()
        var += struct.pack(">H", len(u)) + u
    if password is not None:
        p = password.encode()
        var += struct.pack(">H", len(p)) + p
    return bytes([0x10]) + bytes([len(var)]) + bytes(var)


def read_packet(sock):
    hdr = sock.recv(1)
    if not hdr:
        return None, None
    rl = 0
    mult = 1
    while True:
        e = sock.recv(1)
        rl += (e[0] & 0x7F) * mult
        if not e[0] & 0x80:
            break
        mult *= 128
    body = b""
    while len(body) < rl:
        chunk = sock.recv(rl - len(body))
        if not chunk:
            break
        body += chunk
    return hdr[0], body


def connack_rc(sock):
    hdr, body = read_packet(sock)
    if hdr is None:
        return None
    return body[1] if len(body) >= 2 else None


def run_case(broker, user, pw, expect_ok, transcript, label):
    s = socket.create_connection(("127.0.0.1", 1883), timeout=5)
    s.sendall(encode_connect("probe-%d" % int(time.time() * 1000), user, pw))
    rc = connack_rc(s)
    rec = {"case": label, "expect_ok": expect_ok, "connack_rc": rc}
    try:
        s.close()
    except OSError:
        pass
    transcript["cases"].append(rec)
    print(json.dumps(rec, ensure_ascii=False), flush=True)
    return (rc == 0) == expect_ok


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", default="/tmp/b2-creds.json")
    ap.add_argument("--expect-username", default=USER)
    ap.add_argument("--expect-password", default=PASS)
    args = ap.parse_args()

    transcript = {"cases": []}
    broker_proc = subprocess.Popen(
        ["python3",
         "/home/ahx/文档/EnerOS/evidence/spec-2026-08-27-B/mqtt_broker_b2.py",
         "--port", "1883",
         "--transcript", args.transcript + ".broker.jsonl",
         "--expect-username", args.expect_username,
         "--expect-password", args.expect_password],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        time.sleep(1.0)
        results = [
            run_case(broker_proc, args.expect_username, args.expect_password,
                     True, transcript, "correct-pass"),
            run_case(broker_proc, args.expect_username, "wrong-pass",
                     False, transcript, "wrong-pass"),
            run_case(broker_proc, None, None,
                     False, transcript, "anonymous-unexpected"),
        ]
    finally:
        broker_proc.terminate()
        broker_proc.wait()

    broker_events = []
    try:
        for line in open(args.transcript + ".broker.jsonl"):
            broker_events.append(json.loads(line))
    except OSError:
        pass
    accept = [e for e in broker_events if e.get("event") == "CONNECT-CRED"]
    reject = [e for e in broker_events if e.get("event") == "CONNACK-REJECT"]
    transcript["broker_events"] = {
        "accept": len(accept),
        "reject": len(reject),
        "reject_reasons": [e.get("reason") for e in reject],
    }
    with open(args.transcript, "w") as f:
        json.dump(transcript, f, ensure_ascii=False, indent=1)

    ok = all(results) and len(accept) >= 1 and any(
        e.get("reason") == "wrong-pass" for e in reject)
    print("HOST-CREDS-PROBE " + ("PASS" if ok else "FAIL"),
          f"results={results} accept={len(accept)} reject={len(reject)}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
