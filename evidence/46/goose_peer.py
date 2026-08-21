#!/usr/bin/env python3
"""EnerOS 蓝图 46 WP-46.5：独立 GOOSE raw peer（零 EnerOS 代码）。

连接 QEMU socket(L2) 后端槽 1（127.0.0.1:5577，43 G3 通道）——链路为
"原始以太帧"。QEMU socket 后端帧格式 = **4 字节大端长度前缀 + 帧**（39 卷
实测登记），对端收发均须带前缀。

双向：
- 收：guest svc-goose 发布帧（EtherType 0x88B8）→ transcript（≥3 判据）
- 发：向 guest 订阅侧推合法 GOOSE 帧（dst=010ccd010000, APPID=16385=协议配置值），
  st_num 递增（事件），供 svc-goose 消费。窗口拉长至 210s（guest 启动
  ~150s 才完成网卡初始化+订阅，过早的帧被 virtio RX 队列丢弃——39 时序教训）。

GOOSE 帧结构 = 以太头 + APPID + Length/Res1/Res2（IEC 61850-8-1 §12）+ APDU（BER TLV，
与 EnerOS goose_tx build_frame 同构，保证订阅侧解码成功）。
"""
import argparse, json, socket, struct, time

SRC = bytes.fromhex("010ccd010000")
DST = bytes.fromhex("010ccd010000")
APID = 0x4001  # 16385 与 configs/protocol-goose.toml app_id 一致

def ber_len(n):
    if n < 0x80:
        return bytes([n])
    return bytes([0x82]) + struct.pack(">H", n)

def goose_frame(st, sq, conf=1):
    content = bytearray()
    content += bytes([0x81, 0x08]) + struct.pack(">Q", 0)          # t
    content += bytes([0x82, 0x01, st])                              # stNum
    content += bytes([0x83, 0x01, sq])                              # sqNum
    content += bytes([0x88, 0x01, conf])                            # confRev
    content += bytes([0x85, 0x00])                                  # ndsCom=false
    content += bytes([0x8A, 0x01, 0x00])                            # numDatasetEntries=1
    content += bytes([0xAB, 0x00])                                  # allData（空）
    apdu = bytes([0x61]) + ber_len(len(content)) + content
    hdr = DST + SRC + struct.pack(">H", 0x88B8) + struct.pack(">H", APID)
    hdr += struct.pack(">H", 6 + len(apdu)) + b"\x00\x00" + b"\x00\x00"
    return hdr + apdu

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--frames", type=int, default=400)
    ap.add_argument("--port", type=int, default=5577)
    ap.add_argument("--deadline", type=float, default=210)
    args = ap.parse_args()
    recs = []
    def log(kind, **kw):
        recs.append({"t": time.time(), "kind": kind, **kw})
        with open(args.transcript, "a") as f:
            f.write(json.dumps(recs[-1]) + "\n")
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(1.0)
    # QEMU socket 后端监听就绪前重试连接（≤30s）。
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
    rx_count = 0
    sent = 0
    buf = b""
    deadline = time.time() + args.deadline
    while time.time() < deadline and sent < args.frames:
        # 推帧（st/sq 递增），QEMU socket 后端 4B 长度前缀。
        frm = goose_frame(1 + (sent % 3), sent)
        sock.sendall(struct.pack(">I", len(frm)) + frm)
        log("TX_GOOSE", st=1 + (sent % 3), sq=sent)
        sent += 1
        time.sleep(0.5)
        # 收帧（剥 4B 前缀，多帧粘包按长度切分）
        try:
            d = sock.recv(65536)
            if d:
                buf += d
                while len(buf) >= 4:
                    ln = struct.unpack(">I", buf[:4])[0]
                    if len(buf) < 4 + ln:
                        break
                    frame = buf[4:4 + ln]
                    buf = buf[4 + ln:]
                    if len(frame) >= 18:
                        et = struct.unpack(">H", frame[12:14])[0]
                        if et == 0x88B8:
                            rx_count += 1
                            log("RX_GOOSE", appid=struct.unpack(">H", frame[14:16])[0])
        except socket.timeout:
            pass
    log("DONE", tx=sent, rx=rx_count)
    print(f"GOOSE-PEER tx={sent} rx={rx_count}")

if __name__ == "__main__":
    main()