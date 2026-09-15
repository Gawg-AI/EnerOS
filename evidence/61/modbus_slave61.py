#!/usr/bin/env python3
"""EnerOS 蓝图 47（WP-47.1/Exit）：独立 Modbus-TCP 从站 peer（零 EnerOS 代码）。

在 45 从站（遥测读）基础上新增：
- 控制点 reg 0x0200（FC03 读 / FC06 写单寄存器，写后状态保持——从站值变化
  外部可证）；
- FC06 协议应答（写单保持寄存器）。

transcript 记录 CONNECT/REQUEST（READ/WRITE）/RESPONSE——对端双证据之一。
"""
import argparse, json, socket, struct, sys, time

VALUES = {
    0x0100: 4000,  # 电压 raw=4000 ×0.1 → 400 V
    0x0101: 500,   # 电流 raw=500 ×0.1 → 50 A
    0x0102: 5000,  # 频率 raw=5000 ×0.01 → 50 Hz
    0x0103: 600,   # SOC raw=600 ×0.001 → 0.6
    0x0104: 1000,  # 并网 raw=1000 ×0.001 → 1.0
    0x0105: 0,     # 过流告警 raw=0
    0x0106: 0,     # 过温告警 raw=0（61 复查矫正：独立过温点，解 6/6 别名）
    0x0200: 0,     # 控制点（有功功率 setpoint，kW raw=值 ×1.0）
    # 2026-09-14（70.15 / REG-70.15-CORE-CHAIN-SINGLE-GAP 修根）：PCS-1 的
    # 遥测改由本链路提供——功率回读点 1001 在站点点表里的地址 =
    # slave 2 / 0x0107（见 configs/points.toml 的订正说明）。目标侧一次组读
    # 取 0x0107..0x0109（1001/1002/1003 三点连续）。
    0x0107: 120,   # P1001 PCS 有功功率 kW（scale 0.1 → 12.0）
    0x0108: 5000,  # P1002（scale 0.01 → 50.00）
    0x0109: 400,   # P1003（scale 0.1 → 40.0）
}

# 线圈（FC1）——P1004 数字量（slave 2 / 0x010A）。
COILS = {
    0x010A: 0,
}

WRITEABLE = {0x0200}


def log(transcript, entry):
    if transcript:
        with open(transcript, "a") as f:
            f.write(json.dumps(entry) + "\n")


def conn_handler(conn, conn_id, transcript, wrong_scale=1.0, reject_writes=False, fail_readback=False):
    buf = b""
    try:
        while True:
            data = conn.recv(4096)
            if not data:
                break
            buf += data
            while len(buf) >= 7:
                if len(buf) < 7:
                    break
                tid = struct.unpack(">H", buf[0:2])[0]
                length = struct.unpack(">H", buf[4:6])[0]
                total = 6 + length
                if len(buf) < total:
                    break
                pkt = buf[:total]
                buf = buf[total:]
                unit = pkt[6]
                pdu = pkt[7:]
                func = pdu[0]
                resp_pdu = None
                if func == 0x03 and len(pdu) == 5:  # 读保持寄存器
                    addr = struct.unpack(">H", pdu[1:3])[0]
                    count = struct.unpack(">H", pdu[3:5])[0]
                    # 61 复查 M4 补证：写后读回不可得——FC03 读控制点回异常
                    #（写照常 ACK），writer 判 AppliedReadbackUnavailable →
                    # dispatch Indeterminate（蓝图 05 §4.5「读回不可得」行）。
                    if fail_readback and addr == 0x0200:
                        log(transcript, {"event": "READBACK-FI", "conn": conn_id,
                                         "tid": tid, "func": "fc03",
                                         "addr": hex(addr),
                                         "injected": "fail-readback"})
                        resp_pdu = bytes([0x83, 0x02])
                    elif addr in VALUES and count == 1:
                        v = VALUES[addr]
                        # 61 复查矫正：错回读注入只作用于**控制点 0x0200**
                        # （蓝图 05 §4.5 M2 行语义 = 写点读回 5→7.5 超容差）。
                        # 历史实现对全部 FC03 读生效——把 ACQ 遥测一并污染
                        # （freq→75Hz 触发 OutOfRange，反馈链饿死）；旧轮
                        # M2 之所以能跑通，是陈旧 root 合成源（好值）与被
                        # 污染 ACQ 交替喂养的偶然，非注入语义正确。
                        if wrong_scale != 1.0 and addr == 0x0200:
                            v = int(v * wrong_scale)
                            log(transcript, {"event": "READ-FI", "conn": conn_id,
                                             "tid": tid, "func": "fc03",
                                             "addr": hex(addr), "value": v,
                                             "injected": "wrong-readback-scale"})
                        resp_pdu = bytes([0x03, 0x02]) + struct.pack(">H", v)
                        log(transcript, {"event": "READ", "conn": conn_id, "tid": tid,
                                         "unit": unit, "func": "fc03", "addr": hex(addr),
                                         "value": v})
                    elif addr in VALUES and count > 1:
                        # 蓝图 61 v0.314.30（[07] §2.3 组读）：多寄存器读——
                        # 逐地址查表（缺表地址=0），标准 0x03 响应
                        # （byte-count + 2N 大端寄存器）。
                        regs = [VALUES.get(addr + k, 0) for k in range(count)]
                        if wrong_scale != 1.0 and addr <= 0x0200 < addr + count:
                            regs[0x0200 - addr] = int(regs[0x0200 - addr] * wrong_scale)
                            log(transcript, {"event": "READ-FI", "conn": conn_id,
                                             "tid": tid, "func": "fc03-group",
                                             "addr": hex(0x0200),
                                             "injected": "wrong-readback-scale"})
                        resp_pdu = bytes([0x03, 2 * count]) + b"".join(
                            struct.pack(">H", r) for r in regs)
                        log(transcript, {"event": "READ-GROUP", "conn": conn_id,
                                         "tid": tid, "func": "fc03",
                                         "addr": hex(addr), "count": count,
                                         "values": regs})
                    else:
                        log(transcript, {"event": "ERROR", "conn": conn_id, "tid": tid,
                                         "func": "fc03", "addr": hex(addr), "count": count,
                                         "handled": "illegal-address"})
                        resp_pdu = bytes([0x83, 0x02])
                elif func == 0x01 and len(pdu) == 5:  # 读线圈（P1004 数字量）
                    addr = struct.unpack(">H", pdu[1:3])[0]
                    count = struct.unpack(">H", pdu[3:5])[0]
                    if not (1 <= count <= 2000):
                        resp_pdu = bytes([0x81, 0x03])
                    else:
                        nbytes = (count + 7) // 8
                        bits = bytearray(nbytes)
                        for i in range(count):
                            if COILS.get(addr + i, 0):
                                bits[i // 8] |= 1 << (i % 8)
                        resp_pdu = bytes([0x01, nbytes]) + bytes(bits)
                elif func == 0x06 and len(pdu) == 5:  # 写单寄存器
                    addr = struct.unpack(">H", pdu[1:3])[0]
                    value = struct.unpack(">H", pdu[3:5])[0]
                    if reject_writes:
                        log(transcript, {"event": "WRITE-FI", "conn": conn_id,
                                         "tid": tid, "func": "fc06",
                                         "addr": hex(addr),
                                         "injected": "reject-writes"})
                        resp_pdu = bytes([0x86, 0x02])
                    elif addr in WRITEABLE:
                        VALUES[addr] = value
                        resp_pdu = bytes([0x06]) + pdu[1:5]
                        log(transcript, {"event": "WRITE", "conn": conn_id, "tid": tid,
                                         "unit": unit, "func": "fc06", "addr": hex(addr),
                                         "value": value})
                    else:
                        log(transcript, {"event": "ERROR", "conn": conn_id, "tid": tid,
                                         "func": "fc06", "addr": hex(addr),
                                         "handled": "illegal-address"})
                        resp_pdu = bytes([0x86, 0x02])
                else:
                    log(transcript, {"event": "UNSUPPORTED", "conn": conn_id, "tid": tid,
                                     "func": hex(func)})
                    resp_pdu = bytes([func | 0x80, 0x01])
                resp = struct.pack(">HHHB", tid, 0, len(resp_pdu) + 1, unit) + resp_pdu
                conn.sendall(resp)
                log(transcript, {"event": "RESPONSE", "conn": conn_id, "tid": tid})
    except OSError as e:
        log(transcript, {"event": "OSERROR", "conn": conn_id, "err": str(e)})
    finally:
        conn.close()
        log(transcript, {"event": "CLOSE", "conn": conn_id})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=5502)
    ap.add_argument("--transcript", type=str, default=None)
    ap.add_argument("--serve-after", type=int, default=0)
    ap.add_argument("--wrong-readback-scale", type=float, default=1.0,
                    help="故障注入：读回值乘该系数（默认 1.0=正常；如 1.5=错回读）")
    ap.add_argument("--fail-readback", action="store_true",
                    help="故障注入：FC03 读控制点 0x0200 回异常（读回不可得）")
    ap.add_argument("--reject-writes", action="store_true",
                    help="故障注入：写单寄存器一律拒绝（illegal-data-address）")
    args = ap.parse_args()
    time.sleep(args.serve_after)
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(8)
    sys.stderr.write("MODBUS47-SLAVE-READY listening=%d\n" % args.port)
    sys.stderr.flush()
    cid = 0
    while True:
        conn, _ = srv.accept()
        cid += 1
        log(args.transcript, {"event": "CONNECT", "conn": cid})
        conn_handler(conn, cid, args.transcript, args.wrong_readback_scale,
                     args.reject_writes, args.fail_readback)


if __name__ == "__main__":
    main()
