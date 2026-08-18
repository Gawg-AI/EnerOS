#!/usr/bin/env python3
"""EnerOS L2 互操作独立 Modbus-TCP 从站（43）。

独立 raw 实现（socket + struct，零 EnerOS 代码），监听 127.0.0.1:5502；
QEMU user-mode（slirp）把 guest 10.0.2.2:5502 转发到 host 该端口。
记录每个事务到 --transcript（jsonl）：CONNECT / REQUEST(received) /
RESPONSE(sent)。应答保持寄存器：0x1111/0x2222/0x3333（按寄存器地址回
对应值）。畸形请求 → 返回非法功能/异常码且记录（负例 oracle）。
"""
import argparse
import json
import socket
import struct
import sys

VALUES = {
    0x1111: [0x1111],
    0x2222: [0x2222],
    0x3333: [0x3333],
}


def parse_mbap_and_pdu(pkt: bytes):
    """返回 (tid, unit_id, func, payload_pdu_bytes) 或 None（不足/畸形）。"""
    if len(pkt) < 7:
        return None
    tid, pid, length, unit = struct.unpack(">HHHB", pkt[:7])
    if length != len(pkt) - 6:
        return None
    pdu = pkt[7:]
    if len(pdu) < 2:
        return None
    return tid, pid, unit, pdu


def response(transcript, conn_id, tid, unit, pdu: bytes) -> bytes:
    """读保持寄存器（FC=0x03）应答；其余 FC → 非法功能 0x01。"""
    func = pdu[0]
    if func == 0x03 and len(pdu) == 5:
        addr = struct.unpack(">H", pdu[1:3])[0]
        count = struct.unpack(">H", pdu[3:5])[0]
        vals = VALUES.get(addr)
        if vals is None or count != len(vals):
            resp_pdu = bytes([0x83, 0x02])  # 非法数据地址
            entry = {"event": "REQUEST", "conn": conn_id, "tid": tid,
                     "unit": unit, "func": func, "addr": addr, "count": count,
                     "handled": "error-illegal-data-address"}
            transcript.write(json.dumps(entry) + "\n")
            return struct.pack(">HHHB", tid, 0, 3, unit) + resp_pdu
        byte_count = len(vals) * 2
        resp_pdu = bytes([func, byte_count]) + b"".join(
            struct.pack(">H", v) for v in vals
        )
        entry = {"event": "REQUEST", "conn": conn_id, "tid": tid,
                 "unit": unit, "func": func, "addr": addr, "count": count,
                 "handled": "ok", "values": vals}
        transcript.write(json.dumps(entry) + "\n")
        return struct.pack(">HHHB", tid, 0, 3 + byte_count, unit) + resp_pdu
    resp_pdu = bytes([func | 0x80, 0x01])  # 非法功能
    entry = {"event": "REQUEST", "conn": conn_id, "tid": tid, "unit": unit,
             "func": func, "handled": "error-illegal-function"}
    transcript.write(json.dumps(entry) + "\n")
    return struct.pack(">HHHB", tid, 0, 3, unit) + resp_pdu


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=5502)
    ap.add_argument("--transcript", required=True)
    args = ap.parse_args()

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(4)
    sys.stderr.write("SLAVE-READY listening=%d\n" % args.port)
    sys.stderr.flush()

    conn_id = 0
    with open(args.transcript, "w") as tf:
        while True:
            conn, _ = srv.accept()
            conn_id += 1
            tf.write(json.dumps({"event": "CONNECT", "conn": conn_id, "peer": str(conn.getpeername())}) + "\n")
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
                        # 长连接：单帧处理（主站每次请求独立帧；不足则等待）。
                        resp = response(tf, conn_id, tid, unit, pdu)
                        conn.sendall(resp)
                        tf.write(json.dumps({"event": "RESPONSE", "conn": conn_id,
                                             "tid": tid, "bytes": len(resp)}) + "\n")
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
