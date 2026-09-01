#!/usr/bin/env python3
"""EnerOS 蓝图 62 卷 62.2：独立 IEC 60870-5-104 从站（62 扩展版）。

基于 evidence/61/iec104_slave61.py（原文件不动），总召数据突发**追加一组
与 qemu-prod 点表（configs/points.toml）对齐的数据帧**——61 版的 46 基线
IOA（10-12/100-103）不在产品点表（3001/3002/3003，common_addr=1）内，
svc-iec104 的映射索引 (ca, ioa) 不命中 → 无事件发射。62 卷 D-1 判据
（svc-iec104 周期 ACQ 行实弹）需要点表对齐数据源。

追加帧（common_addr=1，值恒定——svc-iec104 死区/去重后首轮发射）：
- ioa=1001 TI=13 短浮点 50.5（point 3001 analog scale 0.1）
- ioa=2001 TI=1  单点   1   （point 3002 digital）
- ioa=3001 TI=13 短浮点 0.075（point 3003 analog scale 0.001）

其余协议行为与 61 版逐字节一致（STARTDT/TESTFR/总召确认/重复帧/重连）。
"""
import argparse, json, socket, struct, time, sys

sys.path.insert(0, __file__.rsplit("/", 1)[0] if "/" in __file__ else ".")

# ---- 复用 61 版帧构造（import 不可行——脚本是扁平文件；逐函数对齐拷贝） ----

def u_frame(func):
    return struct.pack(">BBBBBB", 0x68, 0x04, func, 0, 0, 0)

def i_frame(ssn, rsn, asdu: bytes):
    ctrl0 = (ssn << 1) & 0xFF
    ctrl1 = ((ssn << 1) >> 8) & 0x7F
    ctrl2 = (rsn << 1) & 0xFF
    ctrl3 = ((rsn << 1) >> 8) & 0x7F
    body = bytes([ctrl0, ctrl1, ctrl2, ctrl3]) + asdu
    return struct.pack(">BB", 0x68, len(body)) + body

def cp56(second, ms, minute, hour, day, month, year, iv=False):
    total = second * 1000 + ms
    return bytes([
        total & 0xFF, (total >> 8) & 0xFF,
        (minute & 0x3F) | (0x40 if iv else 0x00),
        hour & 0x1F,
        day & 0x1F,
        month & 0x0F,
        year & 0x0F if False else (year & 0x7F),
    ])

def float_asdu(ioa: int, values, cot=3, qds=0):
    """TI=13（M_ME_TD_1 风格短浮点，无时标帧内变体——61 版同款构造）。"""
    asdu = bytearray([13, len(values), cot, 0])
    asdu += struct.pack("<H", 1)
    for v in values:
        asdu += struct.pack("<I", (ioa & 0xFFFFFF))[:3]
        asdu += struct.pack("<f", v)
        asdu += bytes([qds])
        ioa += 1
    return bytes(asdu)

def single_asdu(ioa: int, value: int, cot=3, qds=0):
    """TI=1（M_SP_NA_1 单点遥信）。"""
    asdu = bytearray([1, 1, cot, 0])
    asdu += struct.pack("<H", 1)
    asdu += struct.pack("<I", (ioa & 0xFFFFFF))[:3]
    asdu += bytes([value & 0x01, qds])
    return bytes(asdu)

def total_interrogation_confirm():
    return bytes([100, 1, 7, 0]) + struct.pack("<H", 1) + struct.pack("<I", 0x000000)[:3] + bytes([20])

def interrogated_baseline():
    asdu = bytearray([13, 3, 20, 0]) + struct.pack("<H", 1)
    for ioa, v in [(10, 50.5), (11, 3.25), (12, 0.75)]:
        asdu += struct.pack("<I", ioa)[:3]
        asdu += struct.pack("<f", v)
        asdu += bytes([0])
    return bytes(asdu)

def float_tagged_asdu(ioa: int, v: float, tag, cot=20, qds=0):
    asdu = bytearray([36, 1, cot, 0])
    asdu += struct.pack("<H", 1)
    asdu += struct.pack("<I", (ioa & 0xFFFFFF))[:3]
    asdu += struct.pack("<f", v)
    asdu += bytes([qds])
    asdu += tag
    return bytes(asdu)

def interrogated_float_data():
    frames = []
    frames.append(interrogated_baseline())
    tag_ok = cp56(0, 500, 0, 12, 30, 8, 26, iv=False)
    frames.append(float_tagged_asdu(100, 50.5, tag_ok, cot=20))
    tag_iv = cp56(0, 500, 0, 12, 30, 8, 26, iv=True)
    frames.append(float_tagged_asdu(101, 61.5, tag_iv, cot=20))
    frames.append(float_asdu(102, [7.25], cot=3, qds=0x10))
    dup = float_tagged_asdu(103, 9.75, cp56(1, 250, 0, 13, 30, 8, 26), cot=20)
    frames.append(dup)
    frames.append(dup)
    # ---- 62 扩展：产品点表对齐（configs/points.toml 3001/3002/3003）----
    frames.append(float_asdu(1001, [50.5], cot=20))       # point 3001
    frames.append(single_asdu(2001, 1, cot=20))           # point 3002
    frames.append(float_asdu(3001, [0.075], cot=20))      # point 3003
    return frames

def serve_session(conn, addr, log, state):
    conn.settimeout(20)
    log("CONNECT", peer=str(addr), session=state["sessions"])
    state["sessions"] += 1
    ssn = 0
    try:
        while True:
            hdr = conn.recv(2)
            if len(hdr) < 2:
                log("EOF", session=state["sessions"] - 1)
                return
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
            if t == 0b11:
                f0 = ctrl[0]
                if f0 == 0x07:
                    conn.sendall(u_frame(0x0B))
                    log("STARTDT_CON", session=state["sessions"] - 1)
                elif f0 == 0x43:
                    conn.sendall(u_frame(0x83))
                    log("TESTFR_CON")
                else:
                    log("U_FRAME", func=hex(f0))
            elif t == 0:
                asdu = body[4:]
                if len(asdu) >= 3:
                    log("I_FRAME", type=asdu[0], cot=asdu[2], len=len(asdu))
                    if asdu[0] == 100 and asdu[2] == 6:
                        rsn = int.from_bytes(body[2:4], "little") >> 1
                        out = i_frame(ssn, rsn, total_interrogation_confirm())
                        ssn += 1
                        for f in interrogated_float_data():
                            out += i_frame(ssn, rsn, f)
                            ssn += 1
                        conn.sendall(out)
                        log(
                            "INTERROGATION+DATA",
                            points=11,
                            dup_frames=2,
                            tagged=True,
                            iv_variant=True,
                            invalid_quality=True,
                            points62_aligned=3,
                        )
            elif t == 0b01:
                log("S_FRAME", rsn=int.from_bytes(body[2:4], "little") >> 1)
    except (socket.timeout, OSError) as e:
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

    t0 = time.time()

    def log(kind, **kw):
        rec = {"t": round(time.time() - t0 + time.time(), 2), "kind": kind}
        rec.update(kw)
        rec["t"] = time.time()
        with open(args.transcript, "a") as f:
            f.write(json.dumps(rec) + "\n")

    open(args.transcript, "w").close()
    log("LISTEN", port=args.port)
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", args.port))
    srv.listen(4)
    srv.settimeout(1.0)
    state = {"sessions": 0}
    while time.time() - t0 < args.deadline:
        try:
            conn, addr = srv.accept()
        except socket.timeout:
            continue
        serve_session(conn, addr, log, state)
    log("DEADLINE", deadline=args.deadline)

if __name__ == "__main__":
    main()
