#!/usr/bin/env python3
"""EnerOS 蓝图 47（WP-47.1/Exit）：外部 SCADA 主站——向 MQTT broker 控制主题
发布下行写命令（零 EnerOS 代码）。负载格式对齐 MQTT 插件 control_topic 解析：
`SET <device_id> <point_id> <value>`（ASCII 空格分隔，value = 十进制 kW）。
"""
import argparse, socket, struct, sys, time


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


def publish(host, port, topic, payload):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(3)
    sock.connect((host, port))
    # CONNECT（MQTT 3.1.1，client id "scada"）。
    cid = b"scada-write"
    proto = b"\x00\x04MQTT\x04\x02"
    body = proto + struct.pack(">H", len(cid)) + cid
    sock.sendall(bytes([0x10]) + encode_varint(len(body)) + body)
    sock.recv(4)  # CONNACK
    # PUBLISH QoS0 topic + payload。
    tb = topic.encode()
    pbody = struct.pack(">H", len(tb)) + tb + payload.encode()
    sock.sendall(bytes([0x30]) + encode_varint(len(pbody)) + pbody)
    sock.close()
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--topic", default="eneros/control")
    ap.add_argument("--payload", default="SET 1 10 50.0")
    ap.add_argument("--delay", type=int, default=0)
    ap.add_argument("--retries", type=int, default=5)
    ap.add_argument("--interval", type=int, default=10)
    args = ap.parse_args()
    time.sleep(args.delay)
    for i in range(args.retries):
        try:
            publish(args.host, args.port, args.topic, args.payload)
            sys.stderr.write("SCADA-WRITE published topic=%s payload=%s\n"
                             % (args.topic, args.payload))
            sys.stderr.flush()
            return
        except OSError as e:
            sys.stderr.write("SCADA-WRITE retry %d/%d err=%s\n" % (i + 1, args.retries, e))
            sys.stderr.flush()
            time.sleep(args.interval)
    sys.exit(2)


if __name__ == "__main__":
    main()
