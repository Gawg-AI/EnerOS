#!/usr/bin/env python3
"""SPEC-SERIES-2026-08-27/B（WP-B.2）：凭据感知版假 MQTT broker。

以 evidence/50/mqtt_telemetry_broker.py 为骨架（零 EnerOS 代码依赖），
按 SPEC-B §3 WP-B.2 改动要点 4 补 CONNECT flags 解析：
  - 0x80 Username Flag / 0x40 Password Flag —— u16 长度前缀 UTF-8 字段；
  - `--expect-username`/`--expect-password` 给出对照表：
      * 用户名缺失 / 口令不符 → CONNACK rc=0x04（坏用户名/口令）+ 关连接，
        记 CONNACK-REJECT 行（B.2-C3 负例断言面）；
      * 匹配 → 正常 CONNACK，记 CONNECT-CRED 行（只记用户名字面量与口令
        **长度**，不落口令内容）。
  - 未给 expect 参数 = 旧语义（来者不拒），49/50 既有编排行为不变
    （B.2-C4 回归面：transcript 中不出现 CONNECT-CRED 行即未启用凭据校验）。

遥测 OBSF 观测面与 50 卷逐字节同语义，供带凭据建联后「telemetry 照常出站」
判据复用。
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
OBSF_HEADER_LEN = 15

CONNACK_OK = bytes([0x20, 0x02, 0x00, 0x00])
CONNACK_BAD_CRED = bytes([0x20, 0x02, 0x00, 0x04])  # rc=4 bad user name or password


def decode_obsf(payload):
    if len(payload) < OBSF_HEADER_LEN:
        return None, "truncated"
    if payload[:4] != OBSF_MAGIC:
        return None, "bad-magic"
    if payload[4] != OBSF_VERSION:
        return None, "unsupported-version:%d" % payload[4]
    batch_seq = struct.unpack("<Q", payload[5:13])[0]
    count = struct.unpack("<H", payload[13:15])[0]
    return {"frame_seq": batch_seq, "items": count, "len": len(payload)}, None


def parse_connect(body):
    """CONNECT 变长头+载荷最小解析 → dict；字段缺失返回部分结果。"""
    try:
        proto_len = int.from_bytes(body[0:2], "big")
        off = 2 + proto_len
        level = body[off]
        off += 1
        flags = body[off]
        off += 1
        keep_alive = int.from_bytes(body[off:off + 2], "big")
        off += 2

        def utf(o):
            n = int.from_bytes(body[o:o + 2], "big")
            return body[o + 2:o + 2 + n], o + 2 + n

        cid, off = utf(off)
        will_topic = will_msg = None
        if flags & 0x04:
            will_topic, off = utf(off)
            will_msg, off = utf(off)
        username = password = None
        if flags & 0x80:
            username, off = utf(off)
        if flags & 0x40:
            password, off = utf(off)
            # 原文仅驻内存供对照（不进任何 log/transcript 键）。
            ret_extra_pw = password
        info = {
            "level": level,
            "flags": flags,
            "keep_alive": keep_alive,
            "client_id": cid.decode(errors="replace"),
            "will_present": bool(flags & 0x04),
            "username_present": bool(flags & 0x80),
            "password_present": bool(flags & 0x40),
            "username": (username or b"").decode(errors="replace"),
            "password_len": len(password) if password is not None else -1,
        }
        if flags & 0x40:
            info["_pw_raw"] = password
        return info
    except Exception as exc:  # noqa: BLE001 — fixture 域内解析容错
        return {"parse_error": str(exc), "body_len": len(body)}


class Broker:
    def __init__(self, transcript_path, telemetry_topic, expect_username=None,
                 expect_password=None):
        self.transcript_path = transcript_path
        self.telemetry_topic = telemetry_topic
        self.expect_username = expect_username
        self.expect_password = expect_password
        self.subs = {}
        self.inflight = {}
        self._lock = threading.Lock()
        self.frames_seen = []
        self.rejected_frames = 0
        self.cred_rejects = 0
        self.cred_accepts = 0

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

    def check_credentials(self, cinfo, conn, cid):
        """返回 True=放行；False=已回 CONNACK rc=4 并登记拒绝。

        对照规则：给了 --expect-username 才启用校验；用户名必须出现且等于
        表值。--expect-password 未给时禁止密码位（合法组合），给了则要求
        密码位出现且内容逐字节匹配（_pw_ok 由 handle 层比对后写入）。"""
        if self.expect_username is None:
            return True  # 未启用凭据校验（旧语义）
        ok_user = cinfo.get("username_present") and (
            cinfo.get("username") == self.expect_username)
        if self.expect_password is None:
            ok_pass = not cinfo.get("password_present")
        else:
            ok_pass = bool(cinfo.get("_pw_ok"))
        if ok_user and ok_pass:
            self.cred_accepts += 1
            self.log("CONNECT-CRED", conn=cid, verdict="accept",
                     username=cinfo.get("username"),
                     username_len=len(cinfo.get("username") or ""),
                     password_len=cinfo.get("password_len"))
            return True
        self.cred_rejects += 1
        why = ("no-user" if not cinfo.get("username_present")
               else ("wrong-user" if not ok_user else "wrong-pass"))
        self.log("CONNACK-REJECT", conn=cid, reason=why,
                 username=cinfo.get("username"),
                 password_len=cinfo.get("password_len"))
        try:
            conn.sendall(CONNACK_BAD_CRED)
            conn.close()
        except OSError:
            pass
        return False

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
            if kind == 1:  # CONNECT —— SPEC-B 凭据解析点
                cinfo = parse_connect(body)
                pw = cinfo.pop("_pw_raw", None)
                if self.expect_password is not None:
                    cinfo["_pw_ok"] = (pw is not None
                                       and pw == self.expect_password.encode())
                self.log("CONNECT", conn=cid,
                         username_present=cinfo.get("username_present"),
                         password_present=cinfo.get("password_present"))
                if not self.check_credentials(cinfo, conn, cid):
                    return
                conn.sendall(CONNACK_OK)
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
            elif kind == 4:  # PUBACK
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
    # SPEC-B WP-B.2：凭据对照表（不给 = 旧语义全收）。
    ap.add_argument("--expect-username", default=None)
    ap.add_argument("--expect-password", default=None)
    args = ap.parse_args()

    broker = Broker(args.transcript, args.telemetry_topic,
                    expect_username=args.expect_username,
                    expect_password=args.expect_password)

    class H(socketserver.BaseRequestHandler):
        def handle(self):
            cid = f"{self.client_address[1]}"
            broker.handle(self.request, cid)

    class Srv(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True

    mode = ("cred-check user=%s" % args.expect_username
            if args.expect_username is not None else "legacy-accept-all")
    broker.log("BROKER-LISTEN", port=args.port, telemetry_topic=args.telemetry_topic,
               mode=mode)
    sys.stderr.write("TELBROKER-READY\n")
    sys.stderr.flush()
    Srv(("127.0.0.1", args.port), H).serve_forever()


if __name__ == "__main__":
    main()
