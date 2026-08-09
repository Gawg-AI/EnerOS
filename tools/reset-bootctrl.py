#!/usr/bin/env python3
"""G-R1 复现辅助：把 raw 镜像的 BOOTCTRL 双副本归零（seq=0，双副本一致）。

QEMU 用 cache=none 直写镜像，引导成功后 BOOTCTRL seq=1 → 下次运行
boot-manager 的 Genesis 检查（first==second && seq==0）拒绝引导。
重建镜像（10 分钟）等效于本脚本（秒级）：置双副本 seq=0 且内容一致，
anchor 由 boot-manager 首次引导重新初始化。
用法：python3 tools/reset-bootctrl.py <raw-image> [BOOTCTRL LBA=18432]
"""
import struct
import sys
import zlib

path = sys.argv[1] if len(sys.argv) > 1 else "target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
first_lba = int(sys.argv[2], 0) if len(sys.argv) > 2 else 18432
sector = 512

CRC_OFFSET = 4092  # BootControl::CRC_OFFSET = RECORD_SIZE(4096) - 4

def crc32(data: bytes) -> int:
    # 与 ipc-wire 的 crc32 一致（IEEE 802.3 0xEDB88320，初值 0xFFFFFFFF，
    # 无最终异或）——zlib 有最终异或，需回退。
    crc = 0xFFFFFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xEDB88320 if crc & 1 else crc >> 1
    return crc & 0xFFFFFFFF

with open(path, "r+b") as f:
    # 读 first copy（4096 B 记录）
    f.seek(first_lba * sector)
    first = bytearray(f.read(4096))
    # 置 seq=0（offset 16，u64 LE）
    struct.pack_into("<Q", first, 16, 0)
    # 重算 CRC（bytes[..CRC_OFFSET] 的 CRC 写入 CRC_OFFSET）
    new_crc = crc32(bytes(first[:CRC_OFFSET]))
    struct.pack_into("<I", first, CRC_OFFSET, new_crc)
    # 双副本写回（second copy 在 4096 偏移——BOOT_CONTROL_COPY_OFFSETS）
    f.seek(first_lba * sector)
    f.write(first)
    f.seek(first_lba * sector + 4096)
    f.write(first)
print(f"bootctrl reset: {path} first_lba={first_lba} seq=0 crc={new_crc:#x} (dual copies)")
