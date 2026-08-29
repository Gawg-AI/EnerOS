#!/usr/bin/env python3
"""modbus_slave_e.py — SPEC-E WP-E.3 采集馈入从站（47 卷从站的 SOC 振荡变体）。

寄存器与 47 从站同布局（点表 configs/points.toml：1-6 → modbus-tcp 从站 2），
唯一差异：SOC 寄存器 0x0103 每次 FC03 读**交替** 475↔525（×0.001 →
0.475/0.525，跨 System1 出厂死区边界 0.5）——为 svc-ai-worker 死区学习闭环
提供确定性-moving 受信 SOC 流（振幅 0.05：半宽收敛 ~0.1396，带扩至
~[0.26,0.54]，触 catalog 上界 0.14 后转 LEARN-REJECT-CAP；相位按读计数
翻转，transcript 可复现）。
"""
import argparse, json, socket, struct, sys, time

VALUES = {
    0x0100: 4000,  # 电压 raw=4000 ×0.1 → 400 V
    0x0101: 500,   # 电流 raw=500 ×0.1 → 50 A
    0x0102: 5000,  # 频率 raw=5000 ×0.01 → 50 Hz
    0x0103: 475,   # SOC（振荡，见下）
    0x0104: 1000,  # 并网 raw=1000 ×0.001 → 1.0
    0x0105: 0,     # 告警 raw=0
    0x0200: 0,     # 控制点（有功功率 setpoint，kW raw=值 ×1.0）
}

WRITEABLE = {0x0200}
SOC_RAW_LO = 475  # 0.475
SOC_RAW_HI = 525  # 0.525


def log(transcript, entry):
    if transcript:
        with open(transcript, "a") as f:
            f.write(json.dumps(entry) + "\n")


def conn_handler(conn, conn_id, transcript, soc_state):
    buf = b""
    try:
        while True:
            data = conn.recv(4096)
            if not data:
                break
            buf += data
            while len(buf) >= 7:
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
                if func == 0x03 and len(pdu) == 5:
                    addr = struct.unpack(">H", pdu[1:3])[0]
                    count = struct.unpack(">H", pdu[3:5])[0]
                    if addr in VALUES and count == 1:
                        v = VALUES[addr]
                        if addr == 0x0103:
                            # SPEC-E：每次 SOC 读翻转相位（确定性振荡馈入）。
                            soc_state["n"] += 1
                            v = SOC_RAW_HI if soc_state["n"] % 2 else SOC_RAW_LO
                            VALUES[addr] = v
                        resp_pdu = bytes([0x03, 0x02]) + struct.pack(">H", v)
                        log(transcript, {"event": "READ", "conn": conn_id, "tid": tid,
                                         "unit": unit, "func": "fc03", "addr": hex(addr),
                                         "value": v})
                    else:
                        log(transcript, {"event": "ERROR", "conn": conn_id, "tid": tid,
                                         "func": "fc03", "addr": hex(addr), "count": count,
                                         "handled": "illegal-address"})
                        resp_pdu = bytes([0x83, 0x02])
                elif func == 0x06 and len(pdu) == 5:
                    addr = struct.unpack(">H", pdu[1:3])[0]
                    value = struct.unpack(">H", pdu[3:5])[0]
                    if addr in WRITEABLE:
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
    args = ap.parse_args()
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(8)
    sys.stderr.write("MODBUS-E-SLAVE-READY listening=%d\n" % args.port)
    sys.stderr.flush()
    cid = 0
    soc_state = {"n": 0}
    while True:
        conn, _ = srv.accept()
        cid += 1
        log(args.transcript, {"event": "CONNECT", "conn": cid})
        conn_handler(conn, cid, args.transcript, soc_state)


if __name__ == "__main__":
    main()
