#!/usr/bin/env python3
"""EnerOS Modbus RTU 外部独立 peer（主站）：经 QEMU socket 串口与
seL4 root task 内的从站互操作。输出 eneros.mbus.v1 证据。"""
import socket, struct, sys, time, json

SOCK = sys.argv[1] if len(sys.argv) > 1 else "/tmp/eneros-mbus.sock"
ADDR = 1
results = []

def crc16(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc

def frame(pdu):
    f = bytes([ADDR]) + pdu
    c = crc16(f)
    return f + bytes([c & 0xFF, c >> 8])

def read_frame(s, timeout=1.0):
    s.settimeout(timeout)
    buf = b""
    last = time.time()
    while True:
        try:
            d = s.recv(64)
            if not d:
                return None
            buf += d
            last = time.time()
        except socket.timeout:
            if buf and time.time() - last > 0.05:
                return buf
            if not buf:
                return None
        if len(buf) > 260:
            return buf

def txn(s, pdu, expect_silence=True):
    s.sendall(frame(pdu))
    resp = read_frame(s)
    if resp is None:
        return None
    # 校验 CRC
    if crc16(resp[:-2]) != struct.unpack("<H", resp[-2:])[0]:
        return ("badcrc", resp)
    return resp

def check(name, cond, detail=""):
    results.append({"schema": "eneros.mbus.v1", "test": name,
                    "result": "PASS" if cond else "FAIL", "detail": detail})
    print(json.dumps(results[-1], ensure_ascii=False))

def main():
    # 等待 QEMU socket（server=on 创建）
    s = None
    for _ in range(100):
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(SOCK)
            break
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.2)
    if s is None:
        print(json.dumps({"schema": "eneros.mbus.v1", "test": "peer-connect",
                          "result": "FAIL", "detail": "socket 不可达"}))
        sys.exit(1)
    s.settimeout(1.0)
    check("peer-connect", True)

    # 0x03 读保持寄存器（start=0, cnt=4）→ 0x1000..0x1003
    r = txn(s, bytes([0x03, 0x00, 0x00, 0x00, 0x04]))
    ok = r is not None and len(r) == 11 and r[0] == ADDR and r[1] == 0x03 and \
        r[2] == 8 and struct.unpack(">HHHH", r[3:11]) == (0x1000, 0x1001, 0x1002, 0x1003)
    check("read-holding-regs", bool(ok), repr(r) if r else "noresp")

    # 0x04 读输入寄存器（start=0, cnt=2）→ 高区遥测点
    r = txn(s, bytes([0x04, 0x00, 0x00, 0x00, 0x02]))
    ok = r is not None and len(r) == 7 and r[1] == 0x04 and r[2] == 4
    check("read-input-regs", bool(ok), repr(r) if r else "noresp")

    # 0x06 写单个寄存器（reg=0, val=0x1234）→ echo + 读回
    r = txn(s, bytes([0x06, 0x00, 0x00, 0x12, 0x34]))
    ok = r is not None and r[1] == 0x06 and r[3:5] == b"\x12\x34"
    check("write-single-echo", bool(ok), repr(r) if r else "noresp")
    r = txn(s, bytes([0x03, 0x00, 0x00, 0x00, 0x01]))
    ok = r is not None and r[3:5] == b"\x12\x34"
    check("write-single-readback", bool(ok), repr(r) if r else "noresp")

    # 0x10 写多个（reg=0, cnt=2, [0x1111,0x2222]）
    r = txn(s, bytes([0x10, 0x00, 0x00, 0x00, 0x02, 0x04, 0x11, 0x11, 0x22, 0x22]))
    ok = r is not None and r[1] == 0x10 and r[3:5] == b"\x00\x02"
    check("write-multi-ack", bool(ok), repr(r) if r else "noresp")
    r = txn(s, bytes([0x03, 0x00, 0x00, 0x00, 0x02]))
    ok = r is not None and struct.unpack(">HH", r[3:7]) == (0x1111, 0x2222)
    check("write-multi-readback", bool(ok), repr(r) if r else "noresp")

    # 负向：非法功能码 0x01 → 异常 0x81, code=01
    r = txn(s, bytes([0x01, 0x00, 0x00, 0x00, 0x01]))
    ok = r is not None and r[1] == 0x81 and r[2] == 0x01
    check("illegal-function", bool(ok), repr(r) if r else "noresp")

    # 负向：0x03 越界（start=30, cnt=16）→ 异常 0x83, code=02
    r = txn(s, bytes([0x03, 0x00, 0x1E, 0x00, 0x10]))
    ok = r is not None and r[1] == 0x83 and r[2] == 0x02
    check("illegal-address", bool(ok), repr(r) if r else "noresp")

    # 负向：0x06 写未授权寄存器（reg=16 不在白名单）→ 异常 0x86, code=04
    r = txn(s, bytes([0x06, 0x00, 0x10, 0x00, 0x01]))
    ok = r is not None and r[1] == 0x86 and r[2] == 0x04
    check("ctl-deny-unlisted", bool(ok), repr(r) if r else "noresp")

    # 负向：CRC 错误帧 → 无响应（静默丢弃）
    bad = frame(bytes([0x03, 0x00, 0x00, 0x00, 0x01]))[:-1] + bytes([0x00])
    s.sendall(bad)
    r = read_frame(s, timeout=0.6)
    check("bad-crc-silent", r is None, repr(r) if r else "silent")

    # 负向：错误从站地址（addr=2）→ 无响应
    f = bytes([2]) + bytes([0x03, 0x00, 0x00, 0x00, 0x01])
    c = crc16(f)
    s.sendall(f + bytes([c & 0xFF, c >> 8]))
    r = read_frame(s, timeout=0.6)
    check("wrong-addr-silent", r is None, repr(r) if r else "silent")

    # 负向：畸形帧（长度不足）→ 无响应
    s.sendall(bytes([ADDR, 0x03, 0x00]))
    r = read_frame(s, timeout=0.6)
    check("truncated-silent", r is None, repr(r) if r else "silent")

    passed = sum(1 for x in results if x["result"] == "PASS")
    print(json.dumps({"schema": "eneros.mbus.v1", "test": "peer-summary",
                      "result": "PASS" if passed == len(results) else "FAIL",
                      "detail": f"{passed}/{len(results)}"}))

if __name__ == "__main__":
    main()
