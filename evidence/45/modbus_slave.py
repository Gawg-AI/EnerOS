#!/usr/bin/env python3
"""EnerOS 蓝图 45 采集实弹：独立 raw Modbus-TCP 从站（零 EnerOS 代码）。

监听 127.0.0.1:5502；QEMU user-mode（slirp）把 guest 10.0.2.2:5502 转发到
host 该端口。应答保持寄存器（FC=0x03）——地址 → 值映射与 configs/points.toml
的 modbus-tcp 点（1-6：电压/电流/频率/SOC/并网/告警）寄存器一致，且经
scale×raw → 字节落在 svc-telemetry 信任量程（Volt[0,1000]/Ampere[-1000,1000]/
Hertz[45,55]/Ratio[0,1]/Other[0,1]×2）。

用法：
  python3 evidence/45/modbus_slave.py --transcript <path>            # 常开
  python3 evidence/45/modbus_slave.py --transcript <path> --serve-after 15   # 15s 后拒答（拔 peer 负例）
每次事务记录到 transcript（jsonl）：CONNECT/REQUEST/RESPONSE/…；拒答后
REQUEST 仍记录 handled=fail（上层读超时 → 采集断链负例 oracle）。
"""
import argparse
import json
import socket
import struct
import sys
import time

# configs/points.toml modbus-tcp 点 1-6 寄存器 → 原始值（scale×raw ∈ 信任量程）。
VALUES = {
    0x0100: [4000],  # 电压 raw=4000 ×0.1 → 400.0 V ∈ [0,1000]
    0x0101: [500],   # 电流 raw=500 ×0.1 → 50.0 A ∈ [-1000,1000]
    0x0102: [5000],  # 频率 raw=5000 ×0.01 → 50.0 Hz ∈ [45,55]
    0x0103: [600],   # SOC raw=600 ×0.001 → 0.6 ∈ [0,1]
    0x0104: [1000],  # 并网 raw=1000 ×0.001 → 1.0 ∈ [0,1]
    0x0105: [0],     # 告警 raw=0 ×0.001 → 0.0 ∈ [0,1]
}


def parse_mbap_and_pdu(pkt: bytes):
    if len(pkt) < 7:
        return None
    tid, pid, length, unit = struct.unpack(">HHHB", pkt[:7])
    if length != len(pkt) - 6:
        return None
    pdu = pkt[7:]
    if len(pdu) < 2:
        return None
    return tid, pid, unit, pdu


def response(transcript, conn_id, tid, unit, pdu, serve: bool) -> bytes:
    func = pdu[0]
    if serve and func == 0x03 and len(pdu) == 5:
        addr = struct.unpack(">H", pdu[1:3])[0]
        count = struct.unpack(">H", pdu[3:5])[0]
        vals = VALUES.get(addr)
        if vals is None or count != len(vals):
            entry = {"event": "REQUEST", "conn": conn_id, "tid": tid,
                     "unit": unit, "func": func, "addr": addr, "count": count,
                     "handled": "error-illegal-data-address"}
            transcript.write(json.dumps(entry) + "\n")
            return struct.pack(">HHHB", tid, 0, 3, unit) + bytes([0x83, 0x02])
        byte_count = len(vals) * 2
        resp_pdu = bytes([func, byte_count]) + b"".join(
            struct.pack(">H", v) for v in vals
        )
        entry = {"event": "REQUEST", "conn": conn_id, "tid": tid,
                 "unit": unit, "func": func, "addr": addr, "count": count,
                 "handled": "ok", "values": vals}
        transcript.write(json.dumps(entry) + "\n")
        return struct.pack(">HHHB", tid, 0, 3 + byte_count, unit) + resp_pdu
    # 拒答 / 非法功能：记录 fail 事务（拔 peer 负例 oracle）。
    entry = {"event": "REQUEST", "conn": conn_id, "tid": tid,
             "unit": unit, "func": func, "handled": "fail" if not serve else "error-illegal-function"}
    transcript.write(json.dumps(entry) + "\n")
    return struct.pack(">HHHB", tid, 0, 3, unit) + bytes([func | 0x80, 0x01])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=5502)
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--serve-after", type=float, default=0.0,
                    help="启动 serve-after 秒后拒答（模拟拔 peer）")
    args = ap.parse_args()
    deadline = None if args.serve_after <= 0 else time.monotonic() + args.serve_after

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(4)
    sys.stderr.write("SLAVE-READY listening=%d serve_after=%s\n" % (args.port, args.serve_after))
    sys.stderr.flush()

    conn_id = 0
    with open(args.transcript, "w") as tf:
        while True:
            conn, _ = srv.accept()
            conn_id += 1
            tf.write(json.dumps({"event": "CONNECT", "conn": conn_id,
                                 "peer": str(conn.getpeername())}) + "\n")
            tf.flush()
            buf = b""
            try:
                conn.settimeout(30)
                while True:
                    data = conn.recv(4096)
                    if not data:
                        break
                    buf += data
                    parsed = parse_mbap_and_pdu(buf)
                    if parsed is not None:
                        tid, _pid, unit, pdu = parsed
                        serve = deadline is None or time.monotonic() < deadline
                        resp = response(tf, conn_id, tid, unit, pdu, serve)
                        conn.sendall(resp)
                        tf.write(json.dumps({"event": "RESPONSE", "conn": conn_id,
                                             "tid": tid, "bytes": len(resp),
                                             "serve": serve}) + "\n")
                        tf.flush()
                        buf = b""
            except socket.timeout:
                tf.write(json.dumps({"event": "TIMEOUT", "conn": conn_id}) + "\n")
                tf.flush()
            except OSError as e:
                tf.write(json.dumps({"event": "OSERROR", "conn": conn_id, "err": str(e)}) + "\n")
                tf.flush()
            finally:
                conn.close()


if __name__ == "__main__":
    main()
