#!/usr/bin/env python3
"""EnerOS 蓝图 61 [07] §3.6.3（v0.314.31）：独立 IEC 60870-5-104 从站
（零 EnerOS 代码；evidence/46/iec104_slave.py 的 Phase2 扩展——原文件不动）。

监听 127.0.0.1:2404；QEMU user-mode（slirp）把 guest 10.0.2.2:2404 转发到
host 该端口。角色=从站（104 SLAVE）：
- 收 STARTDT_ACT（U 帧 0x07）→ 回 STARTDT_CON（0x0B）
- 收总召唤（TI=100, COT=Activation=6）→ 回 ActivationConfirm + 数据突发：
  * TI=13 浮点 ×3（46 基线：COT=InterrogatedByStation=20）
  * TI=36 带 CP56Time2a 时标浮点（iv=false 可信 / iv=true 时钟未同步变体）
  * TI=13 品质 invalid 帧（QDS bit4=IV，自发 COT=3）
  * 重复帧（同 ASDU 内容以新 N(S) 协议合法重发两遍）
- 收 TESTFR_ACT（0x43）→ 回 TESTFR_CON（0x83）
- 断连重连：客户端断开（EOF）后回到 accept 等新连接（每会话独立 N(S) 序列）

对端判据：QEMU 退出码 / 串口 READY / SELFCHECK / transcript ≥3 事务 +
v0.314.31 新判据（TI=36 时标帧 / IV 变体 / invalid 品质 / 重复帧）逐条入
transcript。svc-iec104（v0.314.31）走 IngestionModule：ACQ 证据行来自事件
wire 解码（去重后发射）——重复帧去重后只应出现一次发射。

APDU 布局（IEC 60870-5-101/104）：0x68 len ctrl(4B) [ASDU…]。
"""
import argparse, json, socket, struct, time

def u_frame(func):
    # U 格式：68 04 + 4 控制字节（STARTDT_CON=0x0B / TESTFR_CON=0x83）。
    return struct.pack(">BBBBBB", 0x68, 0x04, func, 0, 0, 0)

def i_frame(ssn, rsn, asdu: bytes):
    # I 格式：控制 4B = [N(S)<<1 (2B LE)] + [N(R)<<1 (2B LE)]（15 bit 序号）。
    ctrl0 = (ssn << 1) & 0xFF
    ctrl1 = ((ssn << 1) >> 8) & 0x7F
    ctrl2 = (rsn << 1) & 0xFF
    ctrl3 = ((rsn << 1) >> 8) & 0x7F
    body = bytes([ctrl0, ctrl1, ctrl2, ctrl3]) + asdu
    return struct.pack(">BB", 0x68, len(body)) + body

def cp56(second, ms, minute, hour, day, month, year, iv=False):
    """CP56Time2a 7 字节：毫秒(16B LE = second*1000+ms) 分|IV 时 日 月 年。"""
    total = second * 1000 + ms
    return bytes([
        total & 0xFF, (total >> 8) & 0xFF,
        (minute & 0x3F) | (0x40 if iv else 0x00),
        hour & 0x1F,
        day & 0x1F,
        month & 0x0F,
        year & 0x7F,
    ])

def float_asdu(ioa: int, values, cot=3, qds=0):
    """TI=13（短浮点遥测，无时标）：type VSQ COT OA CA IOA val(4B LE) q …"""
    asdu = bytearray([13, len(values), cot, 0])
    asdu += struct.pack("<H", 1)                 # CA=1
    for v in values:
        asdu += struct.pack("<I", (ioa & 0xFFFFFF))[:3]
        ioa += 1
        asdu += struct.pack("<f", v)
        asdu += bytes([qds])
    return bytes(asdu)

def float_tagged_asdu(ioa: int, v: float, tag, cot=20, qds=0):
    """TI=36（M_ME_TD_1 短浮点带 CP56Time2a 时标）。"""
    asdu = bytearray([36, 1, cot, 0])
    asdu += struct.pack("<H", 1)
    asdu += struct.pack("<I", (ioa & 0xFFFFFF))[:3]
    asdu += struct.pack("<f", v)
    asdu += bytes([qds])
    asdu += tag
    return bytes(asdu)

def total_interrogation_confirm():
    # TI=100, COT=7 (ActivationConfirm), QOI=20
    asdu = bytes([100, 1, 7, 0]) + struct.pack("<H", 1) + struct.pack("<I", 0x000000)[:3] + bytes([20])
    return asdu

def interrogated_float_data():
    """总召数据突发（v0.314.31 判据集）：46 基线 3 浮点 + 时标/品质/重复帧。"""
    frames = []
    # 1) 46 基线：TI=13 COT=20 三点（50.5/3.25/0.75 @ ioa 10-12）。
    frames.append(interrogated_baseline())
    # 2) TI=36 可信时标（2026-08-30 12:00:00.500 UTC，iv=false）。
    tag_ok = cp56(0, 500, 0, 12, 30, 8, 26, iv=False)
    frames.append(float_tagged_asdu(100, 50.5, tag_ok, cot=20))
    # 3) TI=36 IV=true 变体（时钟未同步——svc-iec104 应回退接收时刻 +
    #    品质追加 UNCERTAIN，不冒充可信时标）。
    tag_iv = cp56(0, 500, 0, 12, 30, 8, 26, iv=True)
    frames.append(float_tagged_asdu(101, 61.5, tag_iv, cot=20))
    # 4) TI=13 品质 invalid（QDS=0x10）自发（COT=3）。
    frames.append(float_asdu(102, [7.25], cot=3, qds=0x10))
    # 5) 重复帧：同 ASDU 内容以新 N(S) 协议合法重发两遍（svc-iec104 去重
    #    ——seen 表同戳同指纹跳过；transcript 记两条 DATA 帧供核对）。
    dup = float_tagged_asdu(103, 9.75, cp56(1, 250, 0, 13, 30, 8, 26), cot=20)
    frames.append(dup)
    frames.append(dup)
    return frames

def interrogated_baseline():
    # TI=13, COT=20 (InterrogatedByStation), 3 点（46 基线值不变）。
    asdu = bytearray([13, 3, 20, 0]) + struct.pack("<H", 1)
    for ioa, v in [(10, 50.5), (11, 3.25), (12, 0.75)]:
        asdu += struct.pack("<I", ioa)[:3]
        asdu += struct.pack("<f", v)
        asdu += bytes([0])
    return bytes(asdu)

def serve_session(conn, addr, log, state):
    """单会话服务：STARTDT → 总召突发 → EOF/超时退出（调用方 accept 新连接）。"""
    conn.settimeout(20)
    log("CONNECT", peer=str(addr), session=state["sessions"])
    state["sessions"] += 1
    ssn = 0  # 每会话独立 N(S) 序列（主站重连后期望从 0 重新开始）
    try:
        while True:
            hdr = conn.recv(2)
            if len(hdr) < 2:
                log("EOF", session=state["sessions"] - 1)
                return  # EOF → 等主站重连（[07] §3.6.3 重连判据）
            if hdr[0] != 0x68:
                log("RAW_BYTES", b=hdr.hex())
                continue
            ln = hdr[1]
            body = b""
            while len(body) < ln:
                chunk = conn.recv(ln - len(body))
                if not chunk:
                    return
                body += chunk
            if len(body) < 4:
                log("RAW_BYTES", b=(hdr + body).hex())
                continue
            ctrl = body[0:4]
            t = ctrl[0] & 0x03
            if t == 0b11:  # U 帧
                f0 = ctrl[0]
                if f0 == 0x07:  # STARTDT_ACT
                    conn.sendall(u_frame(0x0B))
                    log("STARTDT_CON", session=state["sessions"] - 1)
                elif f0 == 0x43:  # TESTFR_ACT
                    conn.sendall(u_frame(0x83))
                    log("TESTFR_CON")
                else:
                    log("U_FRAME", func=hex(f0))
            elif t == 0:  # I 帧
                asdu = body[4:]
                if len(asdu) >= 3:
                    log("I_FRAME", type=asdu[0], cot=asdu[2], len=len(asdu))
                    if asdu[0] == 100 and asdu[2] == 6:  # 总召唤 TI=100 COT=6
                        rsn = int.from_bytes(body[2:4], "little") >> 1
                        out = i_frame(ssn, rsn, total_interrogation_confirm())
                        ssn += 1
                        for f in interrogated_float_data():
                            out += i_frame(ssn, rsn, f)
                            ssn += 1
                        conn.sendall(out)
                        log(
                            "INTERROGATION+DATA",
                            points=8,
                            dup_frames=2,
                            tagged=True,
                            iv_variant=True,
                            invalid_quality=True,
                        )
            elif t == 0b01:  # S 帧（主站确认）——记录但不响应
                log("S_FRAME", rsn=int.from_bytes(body[2:4], "little") >> 1)
    except (socket.timeout, OSError) as e:
        # 对端静默（轮询周期/t1 窗口内无帧）——断开回 accept（重连判据）。
        log("READ_TIMEOUT", err=str(e))
        try:
            conn.close()
        except OSError:
            pass

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--deadline", type=float, default=240)
    ap.add_argument("--port", type=int, default=2404)
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
    deadline = time.time() + args.deadline
    state = {"sessions": 0}
    while time.time() < deadline:
        try:
            conn, addr = srv.accept()
        except socket.timeout:
            continue
        except OSError:
            continue
        serve_session(conn, addr, log, state)
    log("DONE", sessions=state["sessions"])
    print("IEC104-SLAVE61 done")

if __name__ == "__main__":
    main()
