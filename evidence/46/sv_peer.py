#!/usr/bin/env python3
"""EnerOS 蓝图 46 WP-46.5：独立 SV（采样值，IEC 61850-9-2）raw peer。

连接 QEMU socket(L2) 后端槽 13（127.0.0.1:5582）。向 guest svc-sv 订阅侧推
SV 帧：EtherType 0x88BA, APPID 16385（协议配置），dst=010ccd040001（配置），
smpCnt 递增（采样流）。SV PDU 与 EnerOS sv_rx 解码面同构（BER TLV 0x80
smpCnt / 0x81 timestamp / 0x82 channels），保证订阅侧解码成功。

QEMU socket 后端帧格式 = **4 字节大端长度前缀 + 帧**（39 卷实测登记）；
窗口拉长至 210s（guest 启动 ~150s 网卡才初始化，过早帧被 RX 队列丢弃）。
"""
import argparse, json, socket, struct, time

SRC = bytes.fromhex("010ccd040002")
DST = bytes.fromhex("010ccd040001")
APID = 0x4001  # 16401→16385（0x4001）与 configs/protocol-sv.toml app_id 一致

def sv_frame(smp_cnt, chans):
    pdu = bytearray()
    pdu += bytes([0x80, 0x02]) + struct.pack(">H", smp_cnt)
    pdu += bytes([0x81, 0x08]) + struct.pack(">Q", smp_cnt)
    pdu += bytes([0x82, len(chans) * 4])
    for c in chans:
        pdu += struct.pack("<f", c)
    hdr = DST + SRC + struct.pack(">H", 0x88BA) + struct.pack(">H", APID)
    # IEC 61850-9-2：Length(2)+Reserved1(2)+Reserved2(2) 之后是 PDU（sv_rx 从
    # eth+APPID+6B 头后解码）。
    hdr += struct.pack(">H", 6 + len(pdu)) + b"\x00\x00" + b"\x00\x00"
    return hdr + pdu

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--frames", type=int, default=400)
    ap.add_argument("--port", type=int, default=5582)
    ap.add_argument("--deadline", type=float, default=210)
    args = ap.parse_args()
    recs = []
    def log(kind, **kw):
        recs.append({"t": time.time(), "kind": kind, **kw})
        with open(args.transcript, "a") as f:
            f.write(json.dumps(recs[-1]) + "\n")
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(1.0)
    for _ in range(30):
        try:
            sock.connect(("127.0.0.1", args.port))
            break
        except OSError:
            time.sleep(1)
    else:
        log("ERROR", err="qemu listener not ready")
        print("PEER-FAIL listener not ready")
        return
    log("CONNECT", port=args.port)
    sent = 0
    buf = b""
    deadline = time.time() + args.deadline
    while time.time() < deadline and sent < args.frames:
        frm = sv_frame((sent % 4096) + 1, [220.0, 50.0, 0.0, 100.0])
        sock.sendall(struct.pack(">I", len(frm)) + frm)
        log("TX_SV", smp_cnt=(sent % 4096) + 1, channels=4)
        sent += 1
        time.sleep(1.0)
        # 收（guest 一般不回 SV；有回则剥前缀记帧数）
        try:
            d = sock.recv(65536)
            if d:
                buf += d
                while len(buf) >= 4:
                    ln = struct.unpack(">I", buf[:4])[0]
                    if len(buf) < 4 + ln:
                        break
                    buf = buf[4 + ln:]
                    log("RX_FRAME")
        except socket.timeout:
            pass
    log("DONE", tx=sent)
    print(f"SV-PEER tx={sent}")

if __name__ == "__main__":
    main()