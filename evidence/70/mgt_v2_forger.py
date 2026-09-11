#!/usr/bin/env python3
"""EnerOS 蓝图 70.5（WP-70.5.6 R6 负例）：MGT1 v2 伪 principal 注入器。

用**错误 station 私钥**（非目标部署公钥配对）签发 principal token 并经
TLS broker 发布 MGT1 v2 帧——目标 svc-mgmt 验签必败（MGT-PRINCIPAL
rejected），伪造 Role/Admin 不得进入授权。判据由 gate 扫串口词形。
"""
import argparse
import socket
import ssl
import struct
import subprocess
import tempfile
import time

MAGIC = b"MGT1"
TOKEN_MAGIC = b"MGTPTK1\0"


def remaining_len(n):
    out = b""
    while True:
        b = n % 128
        n //= 128
        if n:
            out += bytes([b | 0x80])
        else:
            out += bytes([b])
            return out


def mqtt_packet(hdr, body):
    return bytes([hdr]) + remaining_len(len(body)) + body


def utf(s):
    b = s.encode()
    return struct.pack(">H", len(b)) + b


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--broker", default="127.0.0.1:8883")
    ap.add_argument("--ca", required=True)
    ap.add_argument("--client", required=True)
    ap.add_argument("--client-key", required=True)
    ap.add_argument("--topic", default="eneros/mgmt/request")
    ap.add_argument("--transcript", default=None)
    args = ap.parse_args()

    # session v1（48B）：principal/roles/tenant/channel/expiry
    principal = b"evil-admin" + b"\0" * 6  # 16B
    roles = bytes([3, 0, 0, 0])  # 3 = superuser 角色码（rbac 表：2=admin，3=superuser；伪造目标——签名先拒，角色码值不影响 R6）
    tenant = struct.pack("<I", 1)
    channel = b"tls-relay" + b"\0" * 7
    expiry = struct.pack("<Q", 4_102_444_800_000)  # 2100 远期
    session_v1 = principal + roles + tenant + channel + expiry

    # token（64B）：magic(8) ‖ principal(16) ‖ roles(4) ‖ tenant(4) ‖
    # channel(16) ‖ expiry(8) ‖ issued(8) —— 与 mgmt_principal 域布局逐
    # 字段同构（svc-mgmt.rs handle 域检偏移同源）。
    token = TOKEN_MAGIC + principal + roles + tenant + channel + expiry
    issued = struct.pack("<Q", int(time.time() * 1000))
    token += issued
    assert len(token) == 64, f"token={len(token)}"

    # 错钥 DER 签名（SHA-256(token)）——与目标部署公钥不配对。
    with tempfile.NamedTemporaryFile(suffix=".der", delete=False) as f:
        f.write(token)
        token_file = f.name
    sig_der = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", args.client_key, token_file],
        capture_output=True, check=True,
    ).stdout

    sig_field = struct.pack("<H", len(sig_der)) + sig_der
    sig_field += b"\0" * (74 - len(sig_field))
    session_v2 = session_v1 + token + sig_field
    assert len(session_v2) == 48 + 64 + 74

    request_id = struct.pack("<Q", 0xBEEF)
    action = b"reboot"
    # [4] = 保留字节（svc-mgmt handler 自 offset 5 起 parse session）。
    payload = b"\x00" + session_v2 + request_id + bytes([len(action)]) + action
    frame = MAGIC + payload

    host, port = args.broker.rsplit(":", 1)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.load_verify_locations(args.ca)
    ctx.load_cert_chain(args.client, args.client_key)
    raw = socket.create_connection((host, int(port)), timeout=10)
    sock = ctx.wrap_socket(raw, server_hostname="eneros-broker")
    print("TLS-CONNECTED", sock.version(), sock.cipher()[0])

    # MQTT CONNECT（accept 简版）+ PUBLISH MGT 帧（QoS0）
    vhost = "10.0.2.2"
    connect = bytes([0x10])
    body = utf("MQTT") + bytes([4, 2]) + struct.pack(">H", 60) + utf("forger")
    connect += remaining_len(len(body)) + body
    sock.sendall(connect)
    time.sleep(0.5)
    sock.sendall(mqtt_packet(0x30, utf(args.topic) + frame))
    print("PUBLISHED forged MGT1 v2 (admin role, wrong-key signature)")
    rec = {"event": "FORGED-MGT-PUBLISH", "topic": args.topic, "len": len(frame)}
    print(rec)
    if args.transcript:
        with open(args.transcript, "a") as f:
            import json
            f.write(json.dumps(rec) + "\n")
    time.sleep(3)
    sock.close()


if __name__ == "__main__":
    main()
