#!/usr/bin/env python3
"""EnerOS 蓝图 70.6（WP-70.6.5）：MGT1 v2 合法站端管理客户端。

与 mgt_v2_forger.py（70.5 R6 负例注入器）同 wire、**正确 station 私钥**
签名——正例注入器：真 principal 面经 TLS broker 发布 MGT1 v2 帧，等待
mgmt/response 应答并解析（status 字节 + detail 文本 + 可选载荷段）。

用法（gate 脚本驱动；单请求-单应答模式）：
  mgt_v2_client.py --broker 127.0.0.1:8883 --ca ca.pem --client client.pem \
      --client-key client.key --station-key station.key \
      --request-id 42 --action config-get --arg-hex <hex> [--principal ops-1] \
      [--role admin] [--wait 12]

退出码：0 = 收到 Completed/Accepted；2 = 收到 Failed；3 = 超时无应答。
应答全文打印到 stdout（`MGT-RESP status=<n> detail=<...> payload=<hex>`）。
"""
import argparse
import binascii
import hashlib
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


def recv_packet(sock, timeout):
    """读取一条完整 MQTT 报文（剩余长度变体解码）；超时/断连 = None。"""
    sock.settimeout(max(0.2, timeout))
    buf = b""
    while True:
        # 头部最少 2B。
        while len(buf) < 2:
            try:
                chunk = sock.recv(4096)
            except (socket.timeout, ssl.SSLError, OSError):
                return None
            if not chunk:
                return None
            buf += chunk
        mul, rl, i = 1, 0, 1
        while True:
            if i >= len(buf):
                rl = None
                break
            b = buf[i]
            rl += (b & 0x7F) * mul
            mul *= 128
            i += 1
            if not (b & 0x80):
                break
        if rl is None:
            continue
        while len(buf) < i + rl:
            try:
                chunk = sock.recv(4096)
            except (socket.timeout, ssl.SSLError, OSError):
                return None
            if not chunk:
                return None
            buf += chunk
        pkt, buf = buf[:i + rl], buf[i + rl:]
        return pkt[0], pkt[i:]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--broker", default="127.0.0.1:8883")
    ap.add_argument("--ca", default=None, help="TLS 模式必需")
    ap.add_argument("--client", default=None, help="TLS 模式必需")
    ap.add_argument("--client-key", default=None, help="TLS 模式必需")
    ap.add_argument("--station-key", required=True, help="station EC P-256 私钥（PEM）")
    ap.add_argument("--topic", default="eneros/mgmt/request")
    ap.add_argument("--resp-topic", default="eneros/mgmt/response")
    ap.add_argument("--request-id", type=int, required=True)
    ap.add_argument("--action", required=True)
    ap.add_argument("--arg-hex", default="")
    ap.add_argument("--principal", default="ops-1")
    ap.add_argument("--role", default="admin", help="viewer|operator|admin|superuser")
    ap.add_argument("--tenant", type=int, default=1)
    ap.add_argument("--expiry-s", type=int, default=3600)
    ap.add_argument("--wait", type=int, default=12, help="应答等待秒")
    ap.add_argument("--no-tls", action="store_true",
                    help="明文 TCP（qemu-test 装配窗；省略 --ca/--client/--client-key）")
    ap.add_argument("--transcript", default=None)
    args = ap.parse_args()
    if not args.no_tls and not (args.ca and args.client and args.client_key):
        ap.error("--ca/--client/--client-key 在 TLS 模式下必需（或用 --no-tls）")

    role_code = {"viewer": 0, "operator": 1, "admin": 2, "superuser": 3}[args.role]

    now_ms = int(time.time() * 1000)
    expiry = now_ms + args.expiry_s * 1000
    principal = args.principal.encode()[:16].ljust(16, b"\0")
    roles = bytes([role_code, 0, 0, 0])
    tenant = struct.pack("<I", args.tenant)
    channel = b"tls-relay".ljust(16, b"\0")
    expiry_b = struct.pack("<Q", expiry)
    session_v1 = principal + roles + tenant + channel + expiry_b

    token = TOKEN_MAGIC + principal + roles + tenant + channel + expiry_b
    token += struct.pack("<Q", now_ms)
    assert len(token) == 64

    # 正确 station 私钥签名（SHA-256(token) ECDSA-P256 DER——mgmt_principal
    # verify_signature 同域同形）。
    with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as f:
        f.write(token)
        token_file = f.name
    sig_der = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", args.station_key, token_file],
        capture_output=True, check=True,
    ).stdout
    sig_field = struct.pack("<H", len(sig_der)) + sig_der
    sig_field += b"\0" * (74 - len(sig_field))
    session_v2 = session_v1 + token + sig_field
    assert len(session_v2) == 186

    request_id = struct.pack("<Q", args.request_id)
    action = args.action.encode()
    arg = binascii.unhexlify(args.arg_hex)
    payload = b"\x00" + session_v2 + request_id + bytes([len(action)]) + action + arg
    frame = MAGIC + payload

    host, port = args.broker.rsplit(":", 1)
    if args.no_tls:
        sock = socket.create_connection((host, int(port)), timeout=10)
        print("PLAIN-CONNECTED")
    else:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.load_verify_locations(args.ca)
        ctx.load_cert_chain(args.client, args.client_key)
        raw = socket.create_connection((host, int(port)), timeout=10)
        sock = ctx.wrap_socket(raw, server_hostname="eneros-broker")
        print("TLS-CONNECTED", sock.version())

    # MQTT 会话：CONNECT（仅 client-id 载荷）→ CONNACK → SUBSCRIBE 应答
    # 主题 → SUBACK → PUBLISH 请求 → 读 PUBLISH 应答。
    connect = bytes([0x10])
    body = utf("MQTT") + bytes([4, 2]) + struct.pack(">H", 120) + utf("mgt-client")
    connect += remaining_len(len(body)) + body
    sock.sendall(connect)
    connack = recv_packet(sock, 5)
    if connack is None or connack[0] >> 4 != 2:
        print("MGT-RESP timeout (no CONNACK)")
        return 3
    # SUBSCRIBE：pid=1，应答主题 QoS0。
    sub_body = struct.pack(">H", 1) + utf(args.resp_topic) + bytes([0])
    sock.sendall(mqtt_packet(0x82, sub_body))
    suback = recv_packet(sock, 5)
    if suback is None or suback[0] >> 4 != 9:
        print("MGT-RESP timeout (no SUBACK)")
        return 3
    sock.sendall(mqtt_packet(0x30, utf(args.topic) + frame))
    print(f"PUBLISHED MGT1 v2 action={args.action} request_id={args.request_id} len={len(frame)}")

    # 读应答（PUBLISH 0x30；忽略其他报文型）。
    deadline = time.time() + args.wait
    result = None
    while time.time() < deadline:
        pkt = recv_packet(sock, max(0.5, deadline - time.time()))
        if pkt is None:
            break
        hdr, pbody = pkt
        if hdr >> 4 != 3:
            continue
        tl = struct.unpack(">H", pbody[:2])[0]
        topic = pbody[2:2 + tl].decode()
        payload = pbody[2 + tl:]
        if topic != args.resp_topic:
            continue
        result = payload
        break
    sock.close()

    if result is None:
        print("MGT-RESP timeout (no response within {}s)".format(args.wait))
        if args.transcript:
            import json
            with open(args.transcript, "a") as f:
                f.write(json.dumps({
                    "event": "MGT-REQUEST-TIMEOUT",
                    "action": args.action,
                    "request_id": args.request_id,
                }) + "\n")
        return 3
    status = result[0]
    rest = result[1:]
    # svc-mgmt 响应 = status(1) + detail(+尾部可选 [len u32][payload])。
    # detail/载荷边界对站端不可先验——完整呈现 hex+text，gate 判据用
    # status 字节 + detail 词形（与串口判据同纪律）。
    print(f"MGT-RESP status={status} raw_len={len(result)}")
    print("MGT-RESP-HEX", result.hex())
    print("MGT-RESP-TEXT", result.decode("utf-8", errors="replace"))
    if args.transcript:
        import json
        with open(args.transcript, "a") as f:
            f.write(json.dumps({
                "event": "MGT-RESPONSE",
                "action": args.action,
                "request_id": args.request_id,
                "status": status,
                "hex": result.hex(),
            }) + "\n")
    if status == 0:
        return 2
    return 0


if __name__ == "__main__":
    import sys
    sys.exit(main())
