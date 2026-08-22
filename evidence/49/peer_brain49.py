#!/usr/bin/env python3
"""peer_brain49.py — 远程 dual-brain peer 参考实现（host 工具，非产品件）。

蓝图 49 WP-49.2 #3：包装站端推理供给产出计划提案供联调。订阅 AI 请求主题，
按策略应答到 AI 应答主题（QoS1）。**确定性策略**（联调判据可控）：

  soc        默认：SOC 规则提案（幅值 ≤ 预算——正向链路证据）
  overbudget 越权负例：提案 power = max_power_kw × --over-factor（target
             契约层必须拒绝 → 降级证据）
  silent     不应答（超时 → Unhealthy 降级证据窗口）
  openai     可选透传 OpenAI 兼容端点（--openai-base + --openai-key +
             --openai-model；端点不可达时回退 soc 策略并登记）

请求/应答契约 = intent-contract::peer（v1 JSON schema，deny_unknown_fields
同构——本工具只产合法字段）。

用法：
  python3 peer_brain49.py --port 1883 [--policy soc|overbudget|silent|openai]
      [--request-topic t] [--response-topic t] [--over-factor 2.0]
      [--transcript x.jsonl] [--max-responses N]
"""
import argparse
import json
import socket
import struct
import sys
import time


class MqttClient:
    """最小 MQTT 3.1.1 客户端（CONNECT/SUBSCRIBE/PUBLISH QoS≤1/PINGREQ）。"""

    def __init__(self, host, port, client_id, transcript=None, connect_timeout=30):
        # 带重试连接（broker 就绪竞态——49 第十轮实弹教训：ECONNREFUSED
        # 是即时错误，超时参数不重试）。
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
        """循环收 PUBLISH（请求）；handler(req_dict)->resp_dict or None。"""
        served = 0
        while max_responses <= 0 or served < max_responses:
            hdr, body = self._recv()
            if hdr is None:
                continue
            kind = hdr >> 4
            if kind == 12:  # PINGREQ
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
                # PUBACK 回 broker。
                self._send(0x40, struct.pack(">H", pid))
            try:
                req = json.loads(payload.decode())
            except (ValueError, UnicodeDecodeError):
                self.log("BAD-REQUEST", payload=payload[:64].decode(errors="replace"))
                continue
            self.log("REQUEST", req=req)
            resp = handler(req)
            if resp is None:
                self.log("NO-RESPONSE", req_id=req.get("req_id"))
                continue
            time.sleep(0.2)  # 模拟推理延迟（可观测的请求→应答间隔）
            self.publish(topic.replace("request", "response"), json.dumps(resp).encode(), qos=1)
            served += 1
            self.log("RESPONDED", seq=resp["seq"], req_id=resp["req_id"], served=served)


def make_handler(args):
    state = {"seq": 0}

    def handler(req):
        if req.get("v") != 1:
            return None  # 版本不符——不答（契约闭集外）
        budget = req.get("budget", {})
        max_kw = float(budget.get("max_power_kw", 5.0))
        ttl = int(budget.get("ttl_secs", 60))
        soc = float(req.get("telemetry", {}).get("soc_pct", 0.5))
        if args.policy == "silent":
            return None
        if args.policy == "openai":
            got = try_openai(args, req)
            if got is not None:
                return envelope(state, req["req_id"], got, ttl, "openai-endpoint", max_kw, budget_ok=True)
            # 端点不可达 → 回退 soc 策略（登记在 transcript 的 reason 内）。
        if args.policy == "overbudget":
            p = round(max_kw * args.over_factor, 3)  # 越权值（target 必拒）
            return envelope(state, req["req_id"], p, ttl, f"OVERBUDGET x{args.over_factor}", max_kw, budget_ok=False)
        # soc 策略（预算内）。
        if soc >= 0.6:
            p = round(max_kw * 0.8, 3)
            why = "soc-high-discharge"
        elif soc <= 0.4:
            p = round(-max_kw * 0.6, 3)
            why = "soc-low-charge"
        else:
            p = 0.0
            why = "soc-neutral"
        return envelope(state, req["req_id"], p, ttl, why, max_kw, budget_ok=True)

    def envelope(st, req_id, power, ttl, why, max_kw, budget_ok):
        st["seq"] += 1
        return {
            "v": 1,
            "req_id": req_id,
            "seq": st["seq"],
            "plan": {"power_kw": power, "target_device": "pcs", "ttl_secs": ttl},
            "reason": f"{why} limit={max_kw}",
        }

    return handler


def try_openai(args, req):
    if not args.openai_base:
        return None
    import urllib.request
    import urllib.error
    prompt = (
        "你是储能电站调度助手。根据遥测给出充电功率建议(kW)。"
        f"JSON 输入: {json.dumps(req)}; 只回答一个 JSON 数字。"
    )
    body = json.dumps({
        "model": args.openai_model,
        "messages": [{"role": "user", "content": prompt}],
    }).encode()
    req_http = urllib.request.Request(
        args.openai_base.rstrip("/") + "/chat/completions",
        data=body,
        headers={"Content-Type": "application/json",
                 "Authorization": f"Bearer {args.openai_key}"},
    )
    try:
        with urllib.request.urlopen(req_http, timeout=8) as r:
            txt = json.loads(r.read())["choices"][0]["message"]["content"]
            return float(txt.strip().strip('"'))
    except Exception as e:  # noqa: BLE001 — 对端不可达即回退（如实降级）
        print(json.dumps({"event": "OPENAI-FALLBACK", "err": str(e)}), flush=True)
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--policy", default="soc",
                    choices=["soc", "overbudget", "silent", "openai"])
    ap.add_argument("--request-topic", default="eneros/brain/request")
    ap.add_argument("--response-topic", default="eneros/brain/response")
    ap.add_argument("--over-factor", type=float, default=2.0)
    ap.add_argument("--transcript", default=None)
    ap.add_argument("--max-responses", type=int, default=0,
                    help="0 = 不限（演示窗口用 timeout 控制）")
    ap.add_argument("--openai-base", default=None)
    ap.add_argument("--openai-key", default="")
    ap.add_argument("--openai-model", default="gpt-4o-mini")
    args = ap.parse_args()

    c = MqttClient(args.host, args.port, "peer-brain49", args.transcript)
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
