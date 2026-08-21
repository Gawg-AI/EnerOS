#!/usr/bin/env python3
"""EnerOS 蓝图 46 WP-46.5：独立 IEC 60870-5-104 从站（零 EnerOS 代码）。

监听 127.0.0.1:2404；QEMU user-mode（slirp）把 guest 10.0.2.2:2404 转发到 host
该端口。角色=从站（104 SLAVE）：
- 收 STARTDT_ACT（U 帧 0x07 0x00 0x00 0x00）→ 回 STARTDT_CON（0x0B 0x00 0x00 0x00）
- 收总召唤（TI=100, COT=Activation=6）→ 回 ActivationConfirm + 3 个浮点遥测
  （TI=13, COT=InterrogatedByStation=20）
- 收 TESTFR_ACT（0x43）→ 回 TESTFR_CON（0x83）

对端四判据：QEMU 退出码 / 串口 READY / SELFCHECK / **transcript ≥3 事务**。
46.5 二修：长窗口单连接可靠读（旧版 2s 超时即抛回 accept 丢失会话——guest
STARTDT 到达前就被丢弃）；每帧记 transcript，未知 I 帧也记（type/COT）以便
核验主站行为。

APDU 布局（IEC 60870-5-101/104）：0x68 len ctrl(4B) [ASDU…]。
"""
import argparse, json, socket, struct, time

def u_frame(func):
    # U 格式（IEC 60870-5-104）：68 04 + 4 个控制字节，功能字节在控制[0]。
    # STARTDT_CON=0x0B / TESTFR_CON=0x83。
    # 46.5（QEMU 实弹）：>BBBBB 只打包 5 字节（68 04 + 3 控制）——规范 U 帧
    # 为 6 字节（68 04 + 4 控制，末尾三个 0）；主站 Apdu::decode 读 ctrl[0..4]
    # 拒收 5 字节帧 → 停在 StartDtPending。修正为 6 字节。
    return struct.pack(">BBBBBB", 0x68, 0x04, func, 0, 0, 0)

def i_frame(ssn, rsn, asdu: bytes):
    # I 格式：控制 4B = [N(S)<<1｜bit0=0] 2 字节 + [N(R)<<1] 2 字节（简化 8 位 seq）。
    ctrl0 = ((ssn << 1) & 0xFF)
    ctrl1 = 0
    ctrl2 = ((rsn << 1) & 0xFF)
    ctrl3 = 0
    body = bytes([ctrl0, ctrl1, ctrl2, ctrl3]) + asdu
    return struct.pack(">BB", 0x68, len(body)) + body

def float_asdu(ioa: int, values):
    """TI=13（短浮点遥测）ASDU：type VSQ COT OA CA IOA val(4B LE) q …"""
    asdu = bytearray([13, len(values), 3, 0])   # type, VSQ, COT=Spontaneous(3), OA=0
    asdu += struct.pack("<H", 1)                 # CA=1
    for v in values:
        asdu += struct.pack("<I", (ioa & 0xFFFFFF))
        ioa += 1
        asdu += struct.pack("<f", v)  # little-endian float
        asdu += bytes([0])                       # QDS=good
    return bytes(asdu)

def total_interrogation_confirm():
    # TI=100, COT=7 (ActivationConfirm), QOI=20
    asdu = bytes([100, 1, 7, 0]) + struct.pack("<H", 1) + struct.pack("<I", 0x000000) + bytes([20])
    return asdu

def interrogated_float_data():
    # TI=13, COT=20 (InterrogatedByStation), 3 点
    asdu = bytearray([13, 3, 20, 0]) + struct.pack("<H", 1)
    for ioa, v in [(10, 50.5), (11, 3.25), (12, 0.75)]:
        asdu += struct.pack("<I", ioa)[:3]
        asdu += struct.pack("<f", v)
        asdu += bytes([0])
    return bytes(asdu)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--deadline", type=float, default=240)
    args = ap.parse_args()
    recs = []
    def log(kind, **kw):
        recs.append({"t": time.time(), "kind": kind, **kw})
        with open(args.transcript, "a") as f:
            f.write(json.dumps(recs[-1]) + "\n")

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.settimeout(5)
    srv.bind(("127.0.0.1", 2404))
    srv.listen(4)
    log("LISTEN", port=2404)
    deadline = time.time() + args.deadline
    sent = 0
    while time.time() < deadline:
        try:
            conn, addr = srv.accept()
        except socket.timeout:
            continue
        except OSError:
            continue
        conn.settimeout(20)
        log("CONNECT", peer=str(addr))
        try:
            while True:
                hdr = conn.recv(2)
                if len(hdr) < 2:
                    break  # EOF → 等主站重连
                if hdr[0] != 0x68:
                    log("RAW_BYTES", b=hdr.hex())
                    continue
                ln = hdr[1]
                body = b""
                while len(body) < ln:
                    chunk = conn.recv(ln - len(body))
                    if not chunk:
                        break
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
                        log("STARTDT_CON")
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
                            conn.sendall(i_frame(sent, 1, total_interrogation_confirm()))
                            conn.sendall(i_frame(sent + 1, 1, interrogated_float_data()))
                            sent += 2
                            log("INTERROGATION+DATA", points=3)
        except (socket.timeout, OSError) as e:
            # 对端保持连接但静默（轮询周期/t1 窗口内无帧）——继续读，不断开。
            log("READ_TIMEOUT", err=str(e))
            try:
                conn.close()
            except OSError:
                pass
        except Exception as e:
            log("ERROR", err=str(e))
            try:
                conn.close()
            except OSError:
                pass
    log("DONE")
    print("IEC104-SLAVE done")

if __name__ == "__main__":
    main()