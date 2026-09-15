#!/usr/bin/env python3
"""EnerOS 蓝图 70.13（方向 6）——独立 Modbus-RTU 从站 peer（零 EnerOS 代码）。

为什么需要它：被控资产 PCS-1 的**功率回读点 1001** 按站点点表
（`configs/points.toml`）走 **modbus-rtu**（slave 1 / reg 4097 / FC3）。
仿真窗口此前只有 modbus-tcp 链路 ⇒ 回读点永远采不到 ⇒ 独立认证器按
`ControlContractView::completeness` 的 `readback_ok` 判"合同不完整"并
fail-closed 拒证 ⇒ `dispatch=Committed` 恒为 0（方向 1–5 的判据共同阻塞）。
本 peer 补上那条链路：QEMU 由 `--rtu-socket <port>` 实例化第二条 PL011
（`pl011@9040000`）并把它桥到 TCP，本进程作为**客户端**接入该 socket，
按 RTU 成帧（slave + PDU + CRC16，CRC 低字节先发）应答。

服务点（与 `configs/points.toml` 的 modbus-rtu 段逐条对应）：

| 点 ID | 寄存器 | 功能码 | 工程量 |
|---|---|---|---|
| 1001 | 4097 | 03 | PCS 有功功率 kW，scale 0.1 |
| 1002 | 4098 | 03 | scale 0.01 |
| 1003 | 4099 | 03 | scale 0.1 |
| 1004 | 4100 | 01 | 数字量（线圈） |

对端双证据：`--transcript <path>` 记录每次 REQUEST/RESPONSE（jsonl）。

用法：`rtu_slave70.py --port 5599 [--transcript f.jsonl] [--deadline 600]`
"""
from __future__ import annotations

import argparse
import json
import socket
import struct
import sys
import time

SLAVE = 1
# 保持寄存器（FC3）初值：4097..4100
HOLDING = {
    4097: 120,   # P1001 PCS 有功 12.0 kW（scale 0.1）
    4098: 5000,  # P1002 scale 0.01 → 50.00
    4099: 400,   # P1003 scale 0.1 → 40.0
}
# 线圈（FC1）初值：4100（P1004 数字量）
COILS = {4100: 0}


def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    return crc & 0xFFFF


def frame(payload: bytes) -> bytes:
    return payload + struct.pack("<H", crc16(payload))


def log(fh, entry: dict) -> None:
    if fh is not None:
        fh.write(json.dumps(entry) + "\n")
        fh.flush()


def handle(pdu: bytes) -> bytes | None:
    """请求 PDU → 应答 PDU（None = 不应答/异常已内含）。"""
    func = pdu[0]
    if func == 0x03 and len(pdu) == 5:
        addr, count = struct.unpack(">HH", pdu[1:5])
        if not (1 <= count <= 125):
            return bytes([func | 0x80, 0x03])
        out = b""
        for i in range(count):
            out += struct.pack(">H", HOLDING.get(addr + i, 0))
        return bytes([func, len(out)]) + out
    if func == 0x01 and len(pdu) == 5:
        addr, count = struct.unpack(">HH", pdu[1:5])
        if not (1 <= count <= 2000):
            return bytes([func | 0x80, 0x03])
        byte_count = (count + 7) // 8
        bits = bytearray(byte_count)
        for i in range(count):
            if COILS.get(addr + i, 0):
                bits[i // 8] |= 1 << (i % 8)
        return bytes([func, byte_count]) + bytes(bits)
    if func == 0x06 and len(pdu) == 5:
        # 写单保持寄存器（本 peer 的 RTU 面当前只有读点；显式异常，
        # 不静默 ACK —— 静默 ACK 会制造"写了但没生效"的假象）。
        return bytes([func | 0x80, 0x02])
    return bytes([func | 0x80, 0x01])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--transcript", default="")
    ap.add_argument("--deadline", type=float, default=900.0)
    a = ap.parse_args()

    fh = open(a.transcript, "a", encoding="utf-8") if a.transcript else None
    started = time.time()
    # QEMU 侧 `server=on,wait=off` 在 socket chardev 建立后即监听；本进程作为
    # 客户端接入，重试若干次以容忍启动次序。
    conn = None
    for _ in range(60):
        try:
            conn = socket.create_connection((a.host, a.port), timeout=2.0)
            break
        except OSError:
            time.sleep(0.5)
    if conn is None:
        print(f"RTU-SLAVE: cannot connect {a.host}:{a.port}", file=sys.stderr)
        return 2
    log(fh, {"event": "CONNECT", "at": time.time()})
    print(f"RTU-SLAVE-READY slave={SLAVE} port={a.port} regs={sorted(HOLDING)}")

    buf = b""
    conn.settimeout(1.0)
    while time.time() - started < a.deadline:
        try:
            chunk = conn.recv(4096)
        except socket.timeout:
            continue
        except OSError:
            break
        if not chunk:
            break
        buf += chunk
        while len(buf) >= 4:
            # RTU 成帧：addr(1) + func(1) + ... + crc(2)。本 peer 的请求
            # 形态固定（FC1/FC3/FC6 长度 8 字节），按功能码定长取帧。
            slave = buf[0]
            func = buf[1]
            total = 8 if func in (0x01, 0x03, 0x06) else 0
            if total == 0 or len(buf) < total:
                break
            pkt, buf = buf[:total], buf[total:]
            if crc16(pkt[:-2]) != struct.unpack("<H", pkt[-2:])[0]:
                log(fh, {"event": "CRC-BAD", "hex": pkt.hex()})
                continue
            if slave != SLAVE:
                log(fh, {"event": "SLAVE-NOT-MINE", "slave": slave})
                continue
            pdu = pkt[1:-2]
            resp = handle(pdu)
            if resp is None:
                continue
            log(
                fh,
                {
                    "event": "REQUEST",
                    "func": pdu[0],
                    "addr": struct.unpack(">H", pdu[1:3])[0] if len(pdu) >= 3 else None,
                    "count": struct.unpack(">H", pdu[3:5])[0] if len(pdu) >= 5 else None,
                    "at": time.time(),
                },
            )
            conn.sendall(frame(bytes([SLAVE]) + resp))
            log(fh, {"event": "RESPONSE", "func": resp[0], "at": time.time()})
    try:
        conn.close()
    except OSError:
        pass
    log(fh, {"event": "CLOSE", "at": time.time()})
    if fh is not None:
        fh.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
