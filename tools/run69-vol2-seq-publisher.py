#!/usr/bin/env python3
"""69.2-review：VOL69.2-GATE 段0 host 侧 seq 跨重启窗的脚本发布器。

真 MQTT 3.1.1 最小客户端：连 evidence/50 假 broker，订阅 response 主题，
发布 N 条 PeerRequest（schema/域检按 crates/ai/intent-contract/src/peer.rs
与 planner-peer solve_request 的边界构造：soc∈[0,1]、digest 64 位小写 hex、
budget.max_power_kw>0——电压/电流/频率/时戳 peer 侧不检），逐条收
PeerResponse 并断言 seq 严格大于 --expect-min 且批内严格递增。

判据（Q7 主证据）：peer 被 kill -9 重启（同 state 文件）后，其应答 seq
必须严格大于重启前末值——SeqState 持久化的实弹。QEMU 窗内该判据空转
（目标侧 MQTT 面 broker 死亡后不复重连，重启后零请求；REG-69.2-GATE-VACUOUS），
本窗为确定性替代证据。
"""

import argparse
import json
import socket
import sys
import time


def encode_len(n: int) -> bytes:
    out = bytearray()
    while True:
        d = n % 128
        n //= 128
        if n:
            d |= 0x80
        out.append(d)
        if not n:
            return bytes(out)


def utf(s: str) -> bytes:
    b = s.encode()
    return len(b).to_bytes(2, "big") + b


def read_exact(sock: socket.socket, n: int) -> bytes:
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("broker EOF")
        buf += chunk
    return buf


def read_packet(sock: socket.socket):
    hdr = read_exact(sock, 1)[0]
    rem = 0
    mult = 1
    while True:
        b = read_exact(sock, 1)[0]
        rem += (b & 0x7F) * mult
        if not b & 0x80:
            return hdr, rem
        mult *= 128


def build_request(req_id: int, now_ms: int) -> bytes:
    # 域检边界（solve_request）：soc 有限且 ∈[0,1]；budget.min 后 >0；
    # digest_hex 64 位小写 hex = 32B。电压/电流/频率/时戳 peer 侧不检。
    req = {
        "v": 1,
        "req_id": req_id,
        "ts_ms": now_ms,
        "telemetry": {
            "soc_pct": 0.6,
            "voltage_v": 400.0,
            "current_a": 50.0,
            "frequency_hz": 50.0,
            "generated_at_ms": now_ms,
            "digest_hex": "ab" * 32,
        },
        "budget": {"max_power_kw": 5.0, "ttl_secs": 60},
    }
    return json.dumps(req, separators=(",", ":")).encode()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--broker", default="127.0.0.1:1883")
    ap.add_argument("--count", type=int, required=True)
    ap.add_argument("--expect-min", type=int, default=0, help="批内每条 seq 须严格大于此值")
    ap.add_argument("--req-base", type=int, default=0, help="本批 req_id 起始（前值 +i）")
    ap.add_argument("--tag", required=True, help="HOSTWIN 行标签（PRE0/C1/C2/C3）")
    ap.add_argument("--timeout", type=float, default=15.0, help="单条响应等待秒")
    args = ap.parse_args()

    host, _, port = args.broker.rpartition(":")
    sock = socket.create_connection((host or "127.0.0.1", int(port)), timeout=5)
    sock.settimeout(args.timeout)

    # CONNECT（clean session，keepalive 60s）
    body = utf("MQTT") + bytes([0x04, 0x02]) + (60).to_bytes(2, "big") + utf("eneros-seq-pub")
    pkt = bytes([0x10]) + encode_len(len(body)) + body
    sock.sendall(pkt)
    hdr, _ = read_packet(sock)
    if hdr >> 4 != 0x2:
        print(f"HOSTWIN {args.tag} FAIL 期望 CONNACK 得 {hdr >> 4:#x}")
        return 1
    connack = read_exact(sock, 2)
    if connack[1] != 0:
        print(f"HOSTWIN {args.tag} FAIL CONNACK rc={connack[1]}")
        return 1

    # SUBSCRIBE response（QoS1）
    sub_body = (1).to_bytes(2, "big") + utf("eneros/brain/response") + bytes([1])
    sock.sendall(bytes([0x82]) + encode_len(len(sub_body)) + sub_body)
    hdr, _ = read_packet(sock)
    if hdr >> 4 != 0x9:
        print(f"HOSTWIN {args.tag} FAIL 期望 SUBACK 得 {hdr >> 4:#x}")
        return 1
    suback = read_exact(sock, 2 + 1)
    if suback[2] == 0x80:
        print("HOSTWIN FAIL SUBACK 拒绝")
        return 1

    seqs = []
    for i in range(1, args.count + 1):
        req_id = args.req_base + i
        payload = build_request(req_id, int(time.time() * 1000))
        body = utf("eneros/brain/request") + (req_id & 0xFFFF).to_bytes(2, "big") + payload
        sock.sendall(bytes([0x32]) + encode_len(len(body)) + body)
        deadline = time.time() + args.timeout
        got = None
        while got is None:
            remain = deadline - time.time()
            if remain <= 0:
                print(f"HOSTWIN {args.tag} FAIL req_id={req_id} 响应超时（{args.timeout}s）")
                return 1
            sock.settimeout(remain)
            hdr, rem = read_packet(sock)
            body_buf = read_exact(sock, rem) if rem else b""
            kind = hdr >> 4
            if kind == 0x4:  # PUBACK（broker 对我方发布的回执）
                continue
            if kind == 0xD:  # PINGRESP
                continue
            if kind == 0x3:  # PUBLISH（应答）——回 PUBACK 后解析
                qos = (hdr >> 1) & 0x03
                if qos > 0 and rem >= 2:
                    tlen = int.from_bytes(body_buf[0:2], "big")
                    pid_off = 2 + tlen
                    if rem >= pid_off + 2:
                        sock.sendall(
                            bytes([0x40, 0x02])
                            + body_buf[pid_off : pid_off + 2]
                        )
                tlen = int.from_bytes(body_buf[0:2], "big")
                qos = (hdr >> 1) & 0x03
                off = 2 + tlen + (2 if qos > 0 else 0)  # 跳过 packet id
                payload_buf = body_buf[off:]
                try:
                    resp = json.loads(payload_buf)
                except json.JSONDecodeError:
                    print(f"HOSTWIN {args.tag} FAIL 应答非 JSON: {payload_buf[:60]!r}")
                    return 1
                if resp.get("req_id") != req_id:
                    print(
                        f"HOSTWIN {args.tag} FAIL req_id 错配 期望={req_id} 得={resp.get('req_id')}"
                    )
                    return 1
                got = resp.get("seq")
        if not isinstance(got, int) or got <= args.expect_min:
            print(
                f"HOSTWIN {args.tag} FAIL seq={got} 未严格大于 base={args.expect_min}"
            )
            return 1
        if seqs and got <= seqs[-1]:
            print(f"HOSTWIN {args.tag} FAIL seq 批内不递增: {got} ≤ {seqs[-1]}")
            return 1
        seqs.append(got)

    print(
        f"HOSTWIN {args.tag} OK base={args.expect_min} seqs={','.join(map(str, seqs))}"
    )
    sock.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
