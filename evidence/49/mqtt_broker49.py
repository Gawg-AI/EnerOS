#!/usr/bin/env python3
"""独立 MQTT broker（最小独立软件，零 EnerOS 代码）——49 卷 QEMU 实弹用。

与 47 卷 mqtt_broker47.py 同源思路，但支持 **QoS1**（蓝图 WP-49.2 传输判据）：
- 发布方 QoS1 PUBLISH → 路由后回 PUBACK；
- 订阅方按授予 QoS 投递；QoS1 投递未确认 → 按连接保留待重发（重发于
  该连接下一活动事件——演示窗口足够；不实现跨重启持久）。
- CONNECT/SUBSCRIBE/SUBACK/PINGREQ/DISCONNECT 基本面 + JSONL transcript。

用法：python3 mqtt_broker49.py [--port 1883] [--transcript x.jsonl]
"""
import argparse
import json
import socket
import socketserver
import time


class Broker:
    def __init__(self, transcript_path):
        self.transcript_path = transcript_path
        # subscriptions: conn -> list[(filter:str, granted_qos:int)]
        self.subs = {}
        # pending qos1 deliveries: conn -> list[(pid:int, packet:bytes)]
        self.inflight = {}

    def log(self, event, **kw):
        rec = {"ts": time.time(), "event": event, **kw}
        print(json.dumps(rec, ensure_ascii=False), flush=True)
        if self.transcript_path:
            with open(self.transcript_path, "a") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    # ---- MQTT 编解码 helpers ----
    @staticmethod
    def read_packet(rfile):
        b = rfile.read(1)
        if not b:
            return None, None
        hdr = b[0]
        mult, rl = 1, 0
        while True:
            e = rfile.read(1)
            if not e:
                return None, None
            rl += (e[0] & 0x7F) * mult
            if not e[0] & 0x80:
                break
            mult *= 128
        body = rfile.read(rl)
        if len(body) != rl:
            return None, None
        return hdr, body

    @staticmethod
    def utf(body, off):
        n = int.from_bytes(body[off:off + 2], "big")
        return body[off + 2:off + 2 + n], off + 2 + n

    def remaining_len(self, n):
        out = b""
        while True:
            d = n % 128
            n //= 128
            if n:
                d |= 0x80
            out += bytes([d])
            if not n:
                return out

    def publish_pkt(self, topic, payload, qos, pid):
        body = len(topic).to_bytes(2, "big") + topic
        if qos > 0:
            body += pid.to_bytes(2, "big")
        body += payload
        return bytes([0x30 | (qos << 1)]) + self.remaining_len(len(body)) + body

    def route(self, topic, payload, pub_qos):
        """投递到匹配订阅者；QoS1 投递记入 per-conn 待确认表（PUBACK 清）。"""
        for conn, filters in list(self.subs.items()):
            for filt, gqos in filters:
                if filt == "#" or filt == topic or (
                    filt.endswith("#") and topic.startswith(filt[:-1])
                ):
                    q = min(pub_qos, gqos)
                    pid = 0
                    tb = topic.encode()
                    pkt_body = len(tb).to_bytes(2, "big") + tb
                    if q > 0:
                        pid = self.next_pid()
                        pkt_body += pid.to_bytes(2, "big")
                    pkt_body += payload
                    hdr = 0x30 | (q << 1)
                    pkt = bytes([hdr]) + self.remaining_len(len(pkt_body)) + pkt_body
                    try:
                        conn.sendall(pkt)
                        if q > 0:
                            self.inflight.setdefault(conn, []).append((pid, pkt))
                    except OSError:
                        pass
                    break

    _pid = [0]

    def next_pid(self):
        self._pid[0] = self._pid[0] % 65535 + 1
        return self._pid[0]

    def handle(self, conn, cid):
        rfile = conn.makefile("rb")
        while True:
            hdr, body = self.read_packet(rfile)
            if hdr is None:
                break
            kind = hdr >> 4
            if kind == 1:  # CONNECT
                conn.sendall(bytes([0x20, 0x02, 0x00, 0x00]))
                self.log("CONNACK", conn=cid)
            elif kind == 8:  # SUBSCRIBE
                off = 2  # 跳过 packet id
                subs = []
                while off < len(body):
                    t, off = self.utf(body, off)
                    q = body[off] & 0x03
                    off += 1
                    subs.append((t.decode(), min(q, 1)))
                self.subs.setdefault(conn, []).extend(subs)
                # SUBACK = packet_id(2B 回显) + 各主题授予 QoS（MQTT 3.1.1；
                # 漏回显 pid → 客户端同步等确认超时重发，49 第五轮实弹根因）。
                payload = body[:2] + bytes(min(q, 1) for _, q in subs)
                conn.sendall(bytes([0x90]) + self.remaining_len(len(payload)) + payload)
                self.log("SUBSCRIBE", conn=cid, topics=subs)
            elif kind == 3:  # PUBLISH
                qos = (hdr >> 1) & 0x03
                topic, off = self.utf(body, 0)
                pid = 0
                if qos > 0:
                    pid = int.from_bytes(body[off:off + 2], "big")
                    off += 2
                payload = body[off:]
                self.log(
                    "PUBLISH", conn=cid, topic=topic.decode(errors="replace"),
                    payload=payload.decode(errors="replace"), qos=qos,
                )
                self.route(topic.decode(errors="replace"), payload, qos)
                if qos > 0:
                    conn.sendall(bytes([0x40, 0x02]) + pid.to_bytes(2, "big"))
                    self.log("PUBACK->publisher", conn=cid, pid=pid)
            elif kind == 4:  # PUBACK（订阅方确认）
                pid = int.from_bytes(body[:2], "big")
                self.inflight[conn] = [
                    (p, k) for (p, k) in self.inflight.get(conn, []) if p != pid
                ]
                self.log("PUBACK<-subscriber", conn=cid, pid=pid)
            elif kind == 12:  # PINGREQ
                conn.sendall(bytes([0xD0, 0x00]))
            elif kind == 14:  # DISCONNECT
                break
        self.subs.pop(conn, None)
        self.inflight.pop(conn, None)


class Handler(socketserver.BaseRequestHandler):
    """占位防误用：真实入口 = main() 内局部 H（持有 broker 闭包）。"""
    def handle(self):
        raise SystemExit("use main()")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--transcript", default=None)
    args = ap.parse_args()

    broker = Broker(args.transcript)

    class H(socketserver.BaseRequestHandler):
        def handle(self):
            cid = f"{self.client_address[1]}"
            broker.handle(self.request, cid)

    class Srv(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True

    broker.log("BROKER-LISTEN", port=args.port)
    Srv(("127.0.0.1", args.port), H).serve_forever()


if __name__ == "__main__":
    main()
