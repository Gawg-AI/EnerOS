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

path = sys.argv[1] if len(sys.argv) > 1 else "target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
first_lba = int(sys.argv[2], 0) if len(sys.argv) > 2 else 18432
sector = 512

with open(path, "r+b") as f:
    # 读 first copy（4096 B 记录）
    f.seek(first_lba * sector)
    first = bytearray(f.read(4096))
    # 置 seq=0（offset 16，u64 LE）
    struct.pack_into("<Q", first, 16, 0)
    # 双副本写回（second copy 在 4096 偏移——BOOT_CONTROL_COPY_OFFSETS）
    f.seek(first_lba * sector)
    f.write(first)
    f.seek(first_lba * sector + 4096)
    f.write(first)
print(f"bootctrl reset: {path} first_lba={first_lba} seq=0 (dual copies)")
