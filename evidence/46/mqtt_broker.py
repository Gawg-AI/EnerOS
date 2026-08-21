#!/usr/bin/env python3
"""EnerOS 蓝图 46 WP-46.5：独立 MQTT broker（最小独立软件，零 EnerOS 代码）。

监听 127.0.0.1:1883（guest svc-mqtt 经 slirp 10.0.2.2:1883 连接）。实现
MQTT 3.1.1 服务端最小面：CONNECT/CONNACK、PUBLISH（QoS0/1 + PUBACK）、
SUBSCRIBE/SUBACK、PINGREQ/PINGRESP、DISCONNECT。transcript ≥3 事务判据。
keepalive（46.4）：guest 空闲 PINGREQ → PINGRESP 维持连接。
"""
import argparse, json, socket, struct, time

def encode_varint(n):
    out = b""
    while True:
        b = n % 128
        n //= 128
        if n > 0:
            b |= 0x80
        out += bytes([b])
        if n == 0:
            return out

def read_packet(conn):
    hdr = conn.recv(1)
    if not hdr:
        return None
    first = hdr[0]
    mult = 1
    rl = 0
    while True:
        b = conn.recv(1)
        if not b:
            return None
        rl += (b[0] & 0x7F) * mult
        if not (b[0] & 0x80):
            break
        mult *= 128
    body = b""
    while len(body) < rl:
        chunk = conn.recv(rl - len(body))
        if not chunk:
            return None
        body += chunk
    return first, body

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--port", type=int, default=1883)
    args = ap.parse_args()
    recs = []
    def log(kind, **kw):
        recs.append({"t": time.time(), "kind": kind, **kw})
        with open(args.transcript, "a") as f:
            f.write(json.dumps(recs[-1]) + "\n")
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(2)
    log("LISTEN", port=args.port)
    conn_f, _ = srv.accept()
    conn_f.settimeout(2.0)
    log("CONNECT")

    def send(pkt):
        conn_f.sendall(pkt)

    # CONNACK 0x20 0x02 0x00 0x00
    send(bytes([0x20, 0x02, 0x00, 0x00]))
    log("CONNACK")
    tx = 0
    deadline = time.time() + 240  # guest 启动 ~150s 后才建连（39 时序教训）
    while time.time() < deadline:
        try:
            p = read_packet(conn_f)
        except (socket.timeout, OSError):
            continue
        if p is None:
            break
        first, body = p
        kind = first >> 4
        if kind == 3:  # PUBLISH
            # topic 长度 + topic + [pid if qos>0] + payload
            tl = struct.unpack(">H", body[0:2])[0]
            topic = body[2:2 + tl].decode(errors="replace")
            qos = (first >> 1) & 0x03
            off = 2 + tl
            pid = None
            if qos > 0:
                pid = struct.unpack(">H", body[off:off + 2])[0]
                off += 2
            payload = body[off:]
            tx += 1
            log("PUBLISH", topic=topic, qos=qos, pid=pid, payload_len=len(payload))
            if qos == 1:
                send(bytes([0x40]) + encode_varint(2) + struct.pack(">H", pid))
                log("PUBACK", pid=pid)
        elif kind == 8:  # SUBSCRIBE
            pid = struct.unpack(">H", body[0:2])[0]
            send(bytes([0x90]) + encode_varint(1 + 2 + 1) + struct.pack(">H", pid) + b"\x00")
            log("SUBACK", pid=pid)
        elif kind == 12:  # PINGREQ
            send(bytes([0xD0, 0x00]))
            log("PINGRESP")
        elif kind == 14:  # DISCONNECT
            log("DISCONNECT")
            break
    log("DONE", tx=tx)
    print(f"MQTT-BROKER tx={tx}")

if __name__ == "__main__":
    main()
