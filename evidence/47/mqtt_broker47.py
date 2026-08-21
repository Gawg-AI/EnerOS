#!/usr/bin/env python3
"""EnerOS 蓝图 47（WP-47.1/Exit）：独立 MQTT broker（最小独立软件，零 EnerOS 代码）。

在 46 broker 基础上增加**子订阅路由**：guest（svc-mqtt）向控制主题订阅后，
SCADA 客户端经第二连接发布下行写命令 → broker 把该 PUBLISH 转发到
guest 连接（真实 broker 语义）——外部 SCADA 写命令到达 EnerOS 的载体。

监听 127.0.0.1:1883；MQTT 3.1.1 最小服务端面 + 主题订阅/发布路由。
"""
import argparse, json, socket, struct, sys, threading, time


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
    return first >> 4, rl, body


def encode_publish(topic: bytes, payload: bytes, qos: int = 0, pid: int = 0) -> bytes:
    body = struct.pack(">H", len(topic)) + topic
    if qos > 0:
        body += struct.pack(">H", pid)
    body += payload
    return bytes([0x30 | (qos << 1), len(body)]) + body


# 全局订阅表：topic -> {conn}
SUBS = {}
LOCK = threading.Lock()
TRANSCRIPT = None


def log(event, **kw):
    if TRANSCRIPT:
        with open(TRANSCRIPT, "a") as f:
            f.write(json.dumps({"event": event, **kw}) + "\n")


def send(conn, data):
    try:
        conn.sendall(data)
    except OSError:
        pass


def conn_loop(conn, cid):
    first = None
    while True:
        pk = read_packet(conn)
        if not pk:
            break
        kind, rl, body = pk
        if kind == 1:  # CONNECT
            send(conn, bytes([0x20, 0x02, 0x00, 0x00]))
            log("CONNACK", conn=cid)
        elif kind == 8:  # SUBSCRIBE
            pid = struct.unpack(">H", body[0:2])[0]
            off = 2
            topics = []
            while off < len(body):
                tlen = struct.unpack(">H", body[off:off + 2])[0]
                off += 2
                topic = body[off:off + tlen]
                off += tlen
                q = body[off]
                off += 1
                topics.append((topic.decode(errors="replace"), q))
            send(conn, bytes([0x90]) + encode_varint(1 + 1 + len(topics)) +
                 struct.pack(">H", pid) + b"\x00" * len(topics))
            for topic, q in topics:
                with LOCK:
                    SUBS.setdefault(topic, set()).add(conn)
                log("SUBSCRIBE", conn=cid, topic=topic, qos=q)
        elif kind == 3:  # PUBLISH
            off = 0
            tlen = struct.unpack(">H", body[off:off + 2])[0]
            off += 2
            topic = body[off:off + tlen].decode(errors="replace")
            off += tlen
            qos = (body[0] >> 1) & 0x03 if False else 0
            # 简化：忽略头内 QoS 位——SCADA 下发 QoS0。
            pid = None
            if off + 2 <= len(body) and False:
                pid = struct.unpack(">H", body[off:off + 2])[0]
                off += 2
            payload = body[off:]
            log("PUBLISH", conn=cid, topic=topic, payload_len=len(payload))
            # 转发到该主题订阅者（不含发送方）。
            with LOCK:
                targets = list(SUBS.get(topic, set()))
            for t in targets:
                if t is conn:
                    continue
                send(t, encode_publish(topic.encode(), payload, 0))
                log("FORWARD", conn=cid, topic=topic, payload=payload.decode(errors="replace"))
        elif kind == 12:  # PINGREQ
            send(conn, bytes([0xD0, 0x00]))
            log("PINGRESP", conn=cid)
        elif kind == 14:  # DISCONNECT
            log("DISCONNECT", conn=cid)
            break
        else:
            log("SKIP", conn=cid, kind=kind)
    with LOCK:
        for topic in list(SUBS):
            SUBS[topic].discard(conn)
    conn.close()
    log("CLOSE", conn=cid)


def main():
    global TRANSCRIPT
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--transcript", type=str, default=None)
    args = ap.parse_args()
    TRANSCRIPT = args.transcript
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(8)
    sys.stderr.write("MQTT47-BROKER-READY listening=%d\n" % args.port)
    sys.stderr.flush()
    cid = 0
    while True:
        conn, _ = srv.accept()
        cid += 1
        log("CONNECT", conn=cid)
        threading.Thread(target=conn_loop, args=(conn, cid), daemon=True).start()


if __name__ == "__main__":
    main()
