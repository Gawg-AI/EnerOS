#!/usr/bin/env python3
"""EnerOS 蓝图 50（WP-50.2）：外部运维客户端（独立非 EnerOS 进程）。

经 MQTT 管理通道（mgmt_request_topic / mgmt_response_topic）下发 MGT1
管理命令并等待应答。MGT1 帧为固定二进制布局（business-services::mgmt_rpc）：
    magic "MGT1"(4) ‖ principal(16) ‖ roles(4) ‖ tenant u32 LE ‖
    channel(16) ‖ expiry u64 LE ‖ request_id u64 LE ‖ action_len u8 ‖
    action ‖ arg

正例：viewer 发 "status"（只读查询 → Completed）。
负例：viewer 发 "agent-restart"（受控重启需 admin → 越权拒绝 Failed +
审计 denied）。
"""
import argparse
import json
import socket
import struct
import sys
import time

REQ_ID = 1000


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


def read_packet(s):
    b = s.recv(1)
    if not b:
        return None, None
    hdr = b[0]
    mult, rl = 1, 0
    while True:
        e = s.recv(1)
        if not e:
            return None, None
        rl += (e[0] & 0x7F) * mult
        if not e[0] & 0x80:
            break
        mult *= 128
    body = b""
    while len(body) < rl:
        chunk = s.recv(rl - len(body))
        if not chunk:
            return None, None
        body += chunk
    return hdr, body


def encode_publish(topic, payload, qos, pid):
    tb = topic.encode()
    body = struct.pack(">H", len(tb)) + tb
    if qos > 0:
        body += struct.pack(">H", pid)
    body += payload
    return bytes([0x30 | (qos << 1)]) + encode_varint(len(body)) + body


def parse_publish(hdr, body):
    qos = (hdr >> 1) & 0x03
    tlen = int.from_bytes(body[0:2], "big")
    topic = body[2:2 + tlen].decode()
    off = 2 + tlen
    pid = None
    if qos > 0:
        pid = int.from_bytes(body[off:off + 2], "big")
        off += 2
    return topic, body[off:], pid


ROLE_CODES = {"viewer": 0, "operator": 1, "admin": 2, "superuser": 3}


def build_mgt1(principal, role, request_id, action, arg=b""):
    # wire 布局（svc-mgmt.rs MgtHandler）：magic(4) ‖ cmd(1，服务端忽略)
    # ‖ principal(16) ‖ roles(4) ‖ tenant u32 LE ‖ channel(16) ‖
    # expiry u64 LE ‖ request_id u64 LE ‖ action_len u8 ‖ action ‖ arg
    # roles 字段 = mgmt-api 角色码（Role::to_u8 首字节、余位补零——4B 装
    # 不下完整角色名；svc-mgmt 按 Role::from_u8 解码）。
    frame = b"MGT1" + b"\x01"
    p = principal.encode()[:16].ljust(16, b"\x00")
    r = struct.pack("<I", ROLE_CODES[role])
    tenant = struct.pack("<I", 1)
    ch = b"mqtt:ops-client".ljust(16, b"\x00")
    expiry = struct.pack("<Q", 0)  # 0 = 不过期
    rid = struct.pack("<Q", request_id)
    ab = action.encode()
    frame += p + r + tenant + ch + expiry + rid + bytes([len(ab)]) + ab + arg
    return frame


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--request-topic", default="eneros/mgmt/request")
    ap.add_argument("--response-topic", default="eneros/mgmt/response")
    ap.add_argument("--transcript", default=None)
    args = ap.parse_args()

    def log(event, **kw):
        rec = {"ts": time.time(), "event": event, **kw}
        print(json.dumps(rec, ensure_ascii=False), flush=True)
        if args.transcript:
            with open(args.transcript, "a") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    s = socket.create_connection(("127.0.0.1", args.port), timeout=10)
    s.settimeout(10)
    # CONNECT（clean session）
    payload = struct.pack(">H", 4) + b"MQTT" + bytes([4, 2]) + struct.pack(">H", 60)
    cid = b"ops-mgmt-client"
    body = struct.pack(">H", len(cid)) + cid + payload
    s.sendall(bytes([0x10]) + encode_varint(len(body)) + body)
    hdr, _ = read_packet(s)
    assert hdr is not None and hdr >> 4 == 2, "CONNACK expected"
    log("OPS-CONNECTED")
    # SUBSCRIBE response topic（QoS1）
    sub_body = struct.pack(">H", 1) + struct.pack(">H", len(args.response_topic.encode()))
    sub_body += args.response_topic.encode() + bytes([1])
    s.sendall(bytes([0x82]) + encode_varint(len(sub_body)) + sub_body)
    hdr, _ = read_packet(s)
    assert hdr is not None and hdr >> 4 == 9, "SUBACK expected"
    log("OPS-SUBSCRIBED", topic=args.response_topic)

    def send_and_wait(name, action, role, arg=b"", wait_secs=30, retries=1):
        """发命令等应答；超时重试（request_id 递增；现场运维工具同语义）。"""
        for attempt in range(retries + 1):
            r = _send_once(name, action, role, arg, wait_secs, attempt)
            if r is not None:
                return r
        return None

    def _send_once(name, action, role, arg, wait_secs, attempt):
        nonlocal pid_counter
        global REQ_ID
        REQ_ID += 1
        req_id = REQ_ID
        frame = build_mgt1("ops-client", role, req_id, action, arg)
        pid_counter = pid_counter % 65000 + 1
        pkt = encode_publish(args.request_topic, frame, 1, pid_counter)
        s.sendall(pkt)
        log("MGMT-CMD", name=name, action=action, role=role,
            request_id=req_id, frame_len=len(frame), attempt=attempt)
        deadline = time.time() + wait_secs
        while time.time() < deadline:
            s.settimeout(max(0.5, deadline - time.time()))
            try:
                hdr, body = read_packet(s)
            except socket.timeout:
                continue
            except OSError:
                continue
            if hdr is None:
                break
            kind = hdr >> 4
            if kind == 3:
                topic, payload, pid = parse_publish(hdr, body)
                if topic == args.response_topic:
                    status = payload[0] if payload else None
                    detail = payload[1:].decode(errors="replace") if len(payload) > 1 else ""
                    log("MGMT-RESP", name=name, action=action, status=status,
                        detail=detail[:120])
                    return {"status": status, "detail": detail}
                elif pid is not None:
                    s.sendall(bytes([0x40, 0x02]) + struct.pack(">H", pid))
            elif kind == 12:
                s.sendall(bytes([0xD0, 0x00]))
        log("MGMT-TIMEOUT", name=name, action=action)
        return None

    pid_counter = 100
    # 正例：viewer 查询服务状态（role_for=status→viewer；Completed）。
    send_and_wait("status-viewer", "status", "viewer")
    # 只读配置查询：operator 查 points。
    send_and_wait("config-get-operator", "config-get", "operator",
                  arg=b"points")
    # 越权负例：viewer 发受控重启（admin 才可 → 拒绝 + 审计 denied）。
    send_and_wait("restart-viewer-denied", "agent-restart", "viewer",
                  arg=b"svc-ota")
    # 正例：admin 受控重启请求（Accepted——root supervisor 执行）。
    send_and_wait("restart-admin", "agent-restart", "admin", arg=b"svc-ota")
    log("OPS-DONE")
    s.close()


if __name__ == "__main__":
    main()
