#!/usr/bin/env python3
"""EnerOS 蓝图 50（WP-50.1）：独立 MQTT broker + 遥测订阅观测（零 EnerOS 代码）。

在 49 卷 QoS1 broker 基础上内置一个**独立订阅者**：订阅遥测主题
`eneros/eneros-agent/telemetry`，收到 QoS1 帧后按 OBSF 固定二进制格式
解码（magic `OBSF` + schema version + batch_seq u64 LE + count u16 LE），
逐帧记录 frame_seq / 条目数 / 载荷长到 transcript——「QEMU 内指标经 broker
可订阅观测」的独立观测面 + 补传「outbox 序号连续」证据。

用法：python3 mqtt_telemetry_broker.py [--port 1883] [--transcript x.jsonl]
      [--telemetry-topic eneros/eneros-agent/telemetry]
"""
import argparse
import json
import socket
import socketserver
import struct
import sys
import threading
import time

OBSF_MAGIC = b"OBSF"
OBSF_VERSION = 1
OBSF_HEADER_LEN = 15  # magic(4) + version(1) + batch_seq(8) + count(2)


def decode_obsf(payload):
    """解码 OBSF 帧头；schema 版本不匹配 → 拒绝解析（负例语义）。"""
    if len(payload) < OBSF_HEADER_LEN:
        return None, "truncated"
    if payload[:4] != OBSF_MAGIC:
        return None, "bad-magic"
    if payload[4] != OBSF_VERSION:
        return None, "unsupported-version:%d" % payload[4]
    batch_seq = struct.unpack("<Q", payload[5:13])[0]
    count = struct.unpack("<H", payload[13:15])[0]
    return {"frame_seq": batch_seq, "items": count, "len": len(payload)}, None


class Broker:
    def __init__(self, transcript_path, telemetry_topic):
        self.transcript_path = transcript_path
        self.telemetry_topic = telemetry_topic
        self.subs = {}
        self.inflight = {}
        self._lock = threading.Lock()
        self.frames_seen = []
        self.rejected_frames = 0

    def log(self, event, **kw):
        rec = {"ts": time.time(), "event": event, **kw}
        print(json.dumps(rec, ensure_ascii=False), flush=True)
        if self.transcript_path:
            with open(self.transcript_path, "a") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")

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

    def route(self, topic, payload, pub_qos):
        for conn, filters in list(self.subs.items()):
            for filt, gqos in filters:
                if filt == "#" or filt == topic or (
                    filt.endswith("#") and topic.startswith(filt[:-1])
                ):
                    q = min(pub_qos, gqos)
                    tb = topic.encode()
                    pkt_body = len(tb).to_bytes(2, "big") + tb
                    pid = 0
                    if q > 0:
                        with self._lock:
                            self._pid[0] = self._pid[0] % 65535 + 1
                            pid = self._pid[0]
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
                off = 2
                subs = []
                while off < len(body):
                    t, off = self.utf(body, off)
                    q = body[off] & 0x03
                    off += 1
                    subs.append((t.decode(), min(q, 1)))
                self.subs.setdefault(conn, []).extend(subs)
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
                    payload_len=len(payload), qos=qos,
                )
                # 遥测帧解码观测（独立订阅面：broker 侧直接观测）。
                if topic.decode(errors="replace") == self.telemetry_topic:
                    meta, err = decode_obsf(payload)
                    if err:
                        self.rejected_frames += 1
                        self.log("TELFRAME-REJECT", conn=cid, reason=err,
                                 payload_len=len(payload))
                    else:
                        self.frames_seen.append(meta["frame_seq"])
                        self.log("TELFRAME", conn=cid, **meta)
                self.route(topic.decode(errors="replace"), payload, qos)
                if qos > 0:
                    conn.sendall(bytes([0x40, 0x02]) + pid.to_bytes(2, "big"))
                    self.log("PUBACK->publisher", conn=cid, pid=pid)
            elif kind == 4:  # PUBACK（订阅方确认）
                pid = int.from_bytes(body[:2], "big")
                self.inflight[conn] = [
                    (p, k) for (p, k) in self.inflight.get(conn, []) if p != pid
                ]
            elif kind == 12:  # PINGREQ
                conn.sendall(bytes([0xD0, 0x00]))
            elif kind == 14:  # DISCONNECT
                break
        self.subs.pop(conn, None)
        self.inflight.pop(conn, None)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--transcript", default=None)
    ap.add_argument("--telemetry-topic", default="eneros/eneros-agent/telemetry")
    args = ap.parse_args()

    broker = Broker(args.transcript, args.telemetry_topic)

    class H(socketserver.BaseRequestHandler):
        def handle(self):
            cid = f"{self.client_address[1]}"
            broker.handle(self.request, cid)

    class Srv(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True

    broker.log("BROKER-LISTEN", port=args.port, telemetry_topic=args.telemetry_topic)
    sys.stderr.write("TELBROKER-READY\n")
    sys.stderr.flush()
    Srv(("127.0.0.1", args.port), H).serve_forever()


if __name__ == "__main__":
    main()
