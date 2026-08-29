#!/usr/bin/env python3
"""peer_brain_e.py — SPEC-E WP-E.1 假 peer（49 卷 peer_brain49 的清洗判据变体）。

在合法 soc 策略之上叠加**形态污染**（载荷语义保持合法/非法成对），专测
target 侧 normalize_llm_json 清洗单源：

  e1-cycle  序列策略（确定性，判据主路径）：
            #1,#2  fence   合法提案 ```json 围栏包裹     ⇒ target 必须 used=true
            #3,#4  drift   键漂移 power_kw→power_kw_x   ⇒ target 必须 Bad（schema 闭集）
            #5     strings 全字段数字加引号             ⇒ 归一后 used=true
            #6     fence   回到围栏合法                 ⇒ used=true（恢复面）
  fence     恒围栏合法；drift 恒键漂移；strings 恒字符串化；soc 原样透传。

用法同 peer_brain49.py（--policy e1-cycle 等）。
"""
import argparse
import json
import socket
import struct
import sys
import time


class MqttClient:
    def __init__(self, host, port, client_id, transcript=None, connect_timeout=30):
        deadline = time.time() + connect_timeout
        last_err = None
        while True:
            try:
                self.sock = socket.create_connection((host, port), timeout=2)
                break
            except OSError as e:
                last_err = e
                if time.time() > deadline:
                    raise
                time.sleep(0.5)
        self.sock.settimeout(1.0)
        self.cid = client_id
        self.transcript = transcript
        self.pid = 0

    def log(self, event, **kw):
        rec = {"ts": time.time(), "event": event, **kw}
        print(json.dumps(rec, ensure_ascii=False), flush=True)
        if self.transcript:
            with open(self.transcript, "a") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    def _send(self, hdr, body):
        out = b""
        n = len(body)
        while True:
            d = n % 128
            n //= 128
            if n:
                d |= 0x80
            out += bytes([d])
            if not n:
                break
        self.sock.sendall(bytes([hdr]) + out + body)

    @staticmethod
    def _s(x):
        b = x.encode() if isinstance(x, str) else x
        return struct.pack(">H", len(b)) + b

    def connect(self):
        payload = self._s("MQTT") + bytes([4]) + bytes([0x02]) + struct.pack(">H", 60)
        payload += self._s(self.cid)
        self._send(0x10, payload)
        hdr, body = self._recv()
        assert hdr >> 4 == 2 and body[0] == 0, f"CONNACK 失败: {hdr:02x} {body!r}"
        self.log("CONNACK")

    def subscribe(self, topic, qos=1):
        self.pid = self.pid % 65535 + 1
        body = struct.pack(">H", self.pid) + self._s(topic) + bytes([qos])
        self._send(0x82, body)
        deadline = time.time() + 5
        while time.time() < deadline:
            hdr, body = self._recv()
            if hdr is not None and hdr >> 4 == 9:
                self.log("SUBACK", topic=topic)
                return
        raise SystemExit("SUBACK 超时")

    def publish(self, topic, payload_bytes, qos=1):
        pid = 0
        body = self._s(topic)
        if qos > 0:
            self.pid = self.pid % 65535 + 1
            pid = self.pid
            body += struct.pack(">H", pid)
        body += payload_bytes
        self._send(0x30 | (qos << 1), body)
        if qos > 0:
            deadline = time.time() + 5
            while time.time() < deadline:
                hdr, b = self._recv()
                if hdr is not None and hdr >> 4 == 4:
                    self.log("PUBACK", pid=int.from_bytes(b[:2], "big"))
                    return
            raise SystemExit("PUBACK 超时")
        self.log("PUBLISH", topic=topic)

    def _recv(self):
        try:
            b1 = self.sock.recv(1)
            if not b1:
                return None, None
            hdr = b1[0]
            mult, rl = 1, 0
            while True:
                e = self.sock.recv(1)
                rl += (e[0] & 0x7F) * mult
                if not e[0] & 0x80:
                    break
                mult *= 128
            body = b""
            while len(body) < rl:
                body += self.sock.recv(rl - len(body))
            return hdr, body
        except socket.timeout:
            return None, None

    def serve_requests(self, handler, max_responses):
        served = 0
        while max_responses <= 0 or served < max_responses:
            hdr, body = self._recv()
            if hdr is None:
                continue
            kind = hdr >> 4
            if kind == 12:
                self._send(0xD0, b"")
                continue
            if kind != 3:
                continue
            qos = (hdr >> 1) & 0x03
            tl = int.from_bytes(body[0:2], "big")
            topic = body[2:2 + tl].decode()
            off = 2 + tl
            pid = 0
            if qos > 0:
                pid = int.from_bytes(body[off:off + 2], "big")
                off += 2
            payload = body[off:]
            if qos > 0:
                self._send(0x40, struct.pack(">H", pid))
            try:
                req = json.loads(payload.decode())
            except (ValueError, UnicodeDecodeError):
                self.log("BAD-REQUEST", payload=payload[:64].decode(errors="replace"))
                continue
            self.log("REQUEST", req=req)
            resp, mode = handler(req)
            if resp is None:
                self.log("NO-RESPONSE", req_id=req.get("req_id"))
                continue
            time.sleep(0.2)
            self.publish(topic.replace("request", "response"), resp, qos=1)
            served += 1
            self.log("RESPONDED", mode=mode, served=served)


def make_handler(args):
    state = {"seq": 0}

    def soc_power(req):
        budget = req.get("budget", {})
        max_kw = float(budget.get("max_power_kw", 5.0))
        soc = float(req.get("telemetry", {}).get("soc_pct", 0.5))
        if soc >= 0.6:
            return round(max_kw * 0.8, 3), "soc-high-discharge"
        if soc <= 0.4:
            return round(-max_kw * 0.6, 3), "soc-low-charge"
        return 0.0, "soc-neutral"

    def handler(req):
        if req.get("v") != 1:
            return None, None
        budget = req.get("budget", {})
        max_kw = float(budget.get("max_power_kw", 5.0))
        ttl = int(budget.get("ttl_secs", 60))
        power, why = soc_power(req)
        state["seq"] += 1
        seq = state["seq"]
        body = {
            "v": 1,
            "req_id": req["req_id"],
            "seq": seq,
            "plan": {"power_kw": power, "target_device": "pcs", "ttl_secs": ttl},
            "reason": f"E1 {why} limit={max_kw}",
        }

        policy = args.policy
        if policy == "e1-cycle":
            policy = ("fence", "fence", "drift", "drift", "strings", "fence")[
                (seq - 1) % 6
            ]

        if policy == "silent":
            return None, None
        if policy == "overbudget":
            body["plan"]["power_kw"] = round(max_kw * args.over_factor, 3)
            body["reason"] = f"OVERBUDGET x{args.over_factor}"
            mode = "overbudget"
        elif policy == "fence":
            text = "```json\n" + json.dumps(body) + "\n```"
            return text.encode(), "fence"
        elif policy == "prose":
            text = "<answer>模型输出</answer>\n" + json.dumps(body) + "\n以上供参考"
            return text.encode(), "prose"
        elif policy == "drift":
            # 键漂移：plan 内字段改名（schema 闭集必须拒——清洗不得救坏键）。
            plan = body["plan"]
            plan["power_kw_x"] = plan.pop("power_kw")
            mode = "drift"
        elif policy == "strings":
            # 数字全加引号（真实小模型偏离——normalize 字符串数字归一后须过）。
            body["v"] = str(body["v"])
            body["req_id"] = str(body["req_id"])
            body["seq"] = str(body["seq"])
            body["plan"]["power_kw"] = str(body["plan"]["power_kw"])
            body["plan"]["ttl_secs"] = str(body["plan"]["ttl_secs"])
            mode = "strings"
        else:
            mode = "soc"
        return json.dumps(body).encode(), mode

    return handler


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--policy", default="e1-cycle",
                    choices=["soc", "overbudget", "silent", "fence", "prose",
                             "drift", "strings", "e1-cycle"])
    ap.add_argument("--request-topic", default="eneros/brain/request")
    ap.add_argument("--response-topic", default="eneros/brain/response")
    ap.add_argument("--over-factor", type=float, default=2.0)
    ap.add_argument("--transcript", default=None)
    ap.add_argument("--max-responses", type=int, default=0)
    args = ap.parse_args()

    c = MqttClient(args.host, args.port, "peer-brain-e", args.transcript)
    c.connect()
    c.subscribe(args.request_topic, qos=1)
    c.log("PEER-READY", policy=args.policy, request=args.request_topic,
          response=args.response_topic)
    try:
        c.serve_requests(make_handler(args), args.max_responses)
    except KeyboardInterrupt:
        pass
    finally:
        c.log("PEER-EXIT")


if __name__ == "__main__":
    main()
