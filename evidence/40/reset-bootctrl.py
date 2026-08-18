#!/usr/bin/env python3
"""蓝图 40 证据工具：归零 raw 镜像 BOOTCTRL 双副本（seq=0 + 重算 CRC）。

与 xtask 的 reset_bootctrl_copies 等价（QEMU 重跑前置：同盘重跑后
BOOTCTRL seq≠0 + VARS 无锚点 → fail-closed Genesis）。
"""
import struct
import sys
import zlib

RAW = sys.argv[1] if len(sys.argv) > 1 else "target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
RECORD = 4096
SECTOR = 512


def parse_gpt(data):
    lba1 = data[SECTOR:SECTOR * 2]
    assert lba1[0:8] == b"EFI PART", "GPT 头缺失"
    entries_lba = struct.unpack_from("<Q", lba1, 72)[0]
    num_entries = struct.unpack_from("<I", lba1, 80)[0]
    entry_size = struct.unpack_from("<I", lba1, 84)[0]
    base = entries_lba * SECTOR
    parts = []
    for i in range(num_entries):
        e = data[base + i * entry_size: base + (i + 1) * entry_size]
        if e[0:16] == b"\x00" * 16:
            continue
        first = struct.unpack_from("<Q", e, 32)[0]
        last = struct.unpack_from("<Q", e, 40)[0]
        name = e[56:128].decode("utf-16-le").rstrip("\x00")
        parts.append((name, first, last))
    return parts


def main():
    img = bytearray(open(RAW, "rb").read())
    parts = parse_gpt(bytes(img))
    bc = [p for p in parts if p[0] == "BOOTCTRL"]
    assert bc, "无 BOOTCTRL 分区"
    start = bc[0][1] * SECTOR
    for off in (0, RECORD):
        base = start + off
        assert img[base:base + 8] == b"ENERBC01", "BOOTCTRL magic 缺失"
        struct.pack_into("<Q", img, base + 16, 0)  # sequence
        crc = zlib.crc32(bytes(img[base:base + RECORD - 4])) & 0xFFFFFFFF
        struct.pack_into("<I", img, base + RECORD - 4, crc)
    open(RAW, "wb").write(img)
    print(f"bootctrl reset: {RAW} (seq=0, CRC 重算)")


if __name__ == "__main__":
    main()
