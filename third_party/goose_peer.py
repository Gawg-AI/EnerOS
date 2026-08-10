#!/usr/bin/env python3
"""EnerOS GOOSE 互操作参考对端（31 PR-16）。

按 IEC 61850-8-1 §12 生成标准 GOOSE 帧（四字节头 + 标准 BER）：
  dst MAC(6) + src(6) + EtherType 0x88B8(2) + APPID(2)
  + Length(2) + Reserved1(2) + Reserved2(2) + APDU(0x61 ...)

allData 条目使用标准 tag（BOOLEAN=0x83 / INTEGER=0x85 / FLOAT=0x87）。
malformed 变体用于拒绝路径验证。
"""

import struct
import sys

DST = bytes.fromhex("010ccd010001")
SRC = bytes.fromhex("010ccd010000")
APPID = 0x0001


def ber_len(n: int) -> bytes:
    if n < 0x80:
        return bytes([n])
    if n < 0x100:
        return bytes([0x81, n])
    return bytes([0x82]) + struct.pack(">H", n)


def tlv(tag: int, content: bytes) -> bytes:
    return bytes([tag]) + ber_len(len(content)) + content


def build_goose(st_num: int, sq_num: int, values: list) -> bytes:
    content = b""
    content += tlv(0x80, b"IED1LD/LLN0$GO$gocb1")  # gocbRef
    content += tlv(0x81, struct.pack(">H", 5000))  # timeAllowedToLive
    content += tlv(0x82, b"IED1LD/LLN0$ds1")  # datSet
    content += tlv(0x83, b"gocb1")  # goID
    content += tlv(0x84, struct.pack(">Q", 1700000000))  # t (UtcTime 8B)
    content += tlv(0x85, struct.pack(">I", st_num))  # stNum
    content += tlv(0x86, struct.pack(">I", sq_num))  # sqNum
    content += tlv(0x87, b"\x00")  # simulation=false
    content += tlv(0x88, b"\x01")  # confRef
    content += tlv(0x89, b"\x00")  # ndsCom=false
    content += tlv(0x8A, bytes([len(values)]))  # numDatSetEntries
    all_data = b""
    for tag, payload in values:
        all_data += tlv(tag, payload)
    content += tlv(0xAB, all_data)  # allData

    apdu = tlv(0x61, content)
    frame = DST + SRC + struct.pack(">H", 0x88B8) + struct.pack(">H", APPID)
    length = 6 + len(apdu)  # Length 字段含自身 2B + Reserved 4B + APDU
    frame += struct.pack(">H", length) + b"\x00\x00" + b"\x00\x00" + apdu
    return frame


def build_malformed() -> bytes:
    """malformed：Length 字段与实际不符（声称 100 实际 20）——解码必须拒绝。"""
    apdu = tlv(0x61, b"\x80\x03abc")
    frame = DST + SRC + struct.pack(">H", 0x88B8) + struct.pack(">H", APPID)
    frame += struct.pack(">H", 100) + b"\x00\x00" + b"\x00\x00" + apdu
    return frame


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "goose_ref.bin"
    frames = {
        "goose_valid_1": build_goose(1, 0, [(0x83, b"\x01"), (0x85, struct.pack(">i", 42))]),
        "goose_valid_2": build_goose(2, 0, [(0x83, b"\x00"), (0x87, struct.pack(">f", 3.14))]),
        "goose_malformed_len": build_malformed(),
    }
    with open(out, "wb") as f:
        for name, frame in frames.items():
            f.write(struct.pack(">H", len(frame)))
            f.write(frame)
            print(f"[goose_peer] {name}: {len(frame)} bytes")
    print(f"[goose_peer] written {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
