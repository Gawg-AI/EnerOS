#!/usr/bin/env python3
"""EnerOS 蓝图 46 WP-46.5：独立 MMS server peer（零 EnerOS 代码）。

监听 127.0.0.1:5102（QEMU slirp 把 guest 10.0.2.2:5102 转发到 host 该端口）。
按 IEC 61850-8-1 / ISO 9506 手写 MMS 服务端最小面（COTP + ACSE + MMS Read），
与 crates/protocols/iec61850-mms/tests/mms_interop.rs 的独立对端逐字节同构
（BER TLV 编解码与该 crate 零共享）：

1. COTP CR（TPDU 0xE0，10B）→ CC（LI=9, 0xD0）；
2. ACSE AARQ（0x60）→ AARE（0x61 03 02 01 00 —— 客户端 decode_aare 从
   data[0] 解析，不剥 COTP 头，故 AARE 裸发）；
3. MMS Confirmed-RequestPdu（0xA0，Read）→ Confirmed-ResponsePdu（0xA1）
   + Read-Response（0xA5）+ SequenceOf（0xA0）+ Float 25.5（0x87 f32 BE）；
   invokeID 按客户端请求字节 [5..8] 排布回显。

QEMU L2 四判据之一：transcript ≥3 事务（此处记 CONNECT/CC/AARE/READ_RESP，
并有实际 Read 应答闭环）。
"""
import argparse
import json
import socket
import struct
import time

COTP_CC = bytes([0x09, 0xD0, 0x00, 0x00, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A])
AARE = bytes([0x61, 0x03, 0x02, 0x01, 0x00])
COTP_DATA = bytes([0x02, 0xF0, 0x80])


def read_cotp_data(conn):
    """读一个完整 COTP 数据段（03 02 F0 80 打头 + TLV，支持多字节长度）。"""
    acc = b""
    while True:
        chunk = conn.recv(512)
        if not chunk:
            return None if not acc else acc
        acc += chunk
        if len(acc) >= 5:
            len0 = acc[4]
            if len0 & 0x80:
                nbytes = len0 & 0x7F
                if len(acc) < 5 + nbytes:
                    continue
                content_len = 0
                for i in range(nbytes):
                    content_len = (content_len << 8) | acc[5 + i]
            else:
                content_len = len0
            total = 3 + 1 + (1 + (len0 & 0x7F) if len0 & 0x80 else 1) + content_len
            if len(acc) >= total:
                return acc


def read_response(invoke_id):
    """MMS Read-Response：Float 25.5（invokeID 回显）——与 mms_interop.rs 同构。

    客户端 decode_read_response 从 data[0] 解析（不剥 COTP 头），故与 AARE
    一样**裸发**（无 COTP 包装），1:1 复刻 mms_interop.rs 的服务端行为。
    """
    value = struct.pack(">f", 25.5)
    return bytes([0xA1, 0x0C, 0x02, 0x01, invoke_id & 0xFF,
                  0xA5, 0x07, 0xA0, 0x05, 0x87, 0x04]) + value


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--port", type=int, default=5102)
    args = ap.parse_args()
    recs = []

    def log(kind, **kw):
        recs.append({"t": time.time(), "kind": kind, **kw})
        with open(args.transcript, "a") as f:
            f.write(json.dumps(recs[-1]) + "\n")

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.settimeout(5)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(4)
    log("LISTEN", port=args.port)
    deadline = time.time() + 240  # guest 启动 ~150s 后才建连（39 时序教训）
    reads = 0
    while time.time() < deadline:
        try:
            conn, addr = srv.accept()
        except socket.timeout:
            continue
        conn.settimeout(3.0)
        log("CONNECT", peer=str(addr))
        try:
            # 1. COTP CR → CC
            cr = conn.recv(10)
            if len(cr) < 2 or cr[1] != 0xE0:
                log("ERROR", err="no cotp cr")
                conn.close()
                continue
            conn.sendall(COTP_CC)
            log("CC_SENT")
            # 2. AARQ → AARE
            seg = read_cotp_data(conn)
            if seg is None:
                conn.close()
                continue
            if len(seg) < 4 or seg[3] != 0x60:
                log("ERROR", err="no aarq tag")
                conn.close()
                continue
            conn.sendall(AARE)
            log("AARE")
            # 3. Read 请求 → Read-Response（可多次）
            while True:
                seg = read_cotp_data(conn)
                if seg is None:
                    break
                if len(seg) >= 8 and seg[:3] == COTP_DATA and seg[3] == 0xA0:
                    invoke_id = (seg[5] << 16) | (seg[6] << 8) | seg[7]
                    conn.sendall(read_response(invoke_id))
                    reads += 1
                    log("READ_RESP", invoke_id=invoke_id)
        except (socket.timeout, OSError) as e:
            log("ERROR", err=str(e))
        finally:
            try:
                conn.close()
            except OSError:
                pass
    log("DONE", reads=reads)
    print(f"MMS-PEER reads={reads}")


if __name__ == "__main__":
    main()