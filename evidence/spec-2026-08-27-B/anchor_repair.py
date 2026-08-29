#!/usr/bin/env python3
"""SPEC-B 窗口辅助：把 UEFI VARS 内 EnerOSBootControlAnchor 与（seq=0 归零后的）
盘上 bootctrl 双副本重新配对——boot_control.rs BootControlAnchor::encode 同构。

背景：resolve_boot_control 对 anchor 分支逐字段比对（sequence/security_floor/
record_digest）。窗口每次 fresh 盘起步（reset-bootctrl.py 置 seq=0 双副本），
旧 VARS 的锚必然错位 → AnchorMismatch 安全拒绝。Genesis 分支要求 VARS 无锚
（virt-fw-vars 不在环境）；本工具改为**原地等长改写锚数据体（64B）**——
变量头/state 属性零触碰，语义 = 仿佛 boot-manager 上轮以当前记录收尾。

仅用于 SPEC-B QEMU 证据窗口；不进入任何产品链路。
用法：anchor_repair.py <vars.fd> <disk.raw> [BOOTCTRL LBA=18432]
"""
import hashlib
import hmac  # noqa: F401  仅占位保证 hashlib.hmac 可用性提示
import struct
import sys

VARS_PATH, DISK_PATH = sys.argv[1], sys.argv[2]
FIRST_LBA = int(sys.argv[3], 0) if len(sys.argv) > 3 else 18432
SECTOR = 512
RECORD_SIZE = 4096
CRC_OFFSET = RECORD_SIZE - 4
NAME_UTF16 = "EnerOSBootControlAnchor".encode("utf-16-le")

data = bytearray(open(VARS_PATH, "rb").read())
name_at = data.find(NAME_UTF16)
assert name_at >= 0, "VARS 中未找到 EnerOSBootControlAnchor 变量名"

# 认证变量头：[State1][Attr3][NameSize4][DataSize4][Guid16][Name][Data]
guid_at = name_at - 16
namesize_at, datasize_at = guid_at - 8, guid_at - 4
name_size, data_size = struct.unpack_from("<II", data, namesize_at), struct.unpack_from(
    "<I", data, datasize_at)[0]
name_size = name_size[0]
# NameSize 含 UTF-16 NUL 终止符（23 字符 → 48 字节），Data 从 Name 起点整体
# 偏移 NameSize。
assert name_size >= len(NAME_UTF16), f"name size 异常: {name_size}"
assert data_size == 64, f"锚数据长度异常: {data_size}（期望 64）"
payload_at = name_at + name_size

# 盘上记录（reset 后应为 seq=0 双副本一致）。
disk = open(DISK_PATH, "rb")
base = FIRST_LBA * SECTOR
disk.seek(base)
first = disk.read(RECORD_SIZE)
second = disk.read(RECORD_SIZE)
disk.close()
assert first == second, "双副本不一致——先跑 reset-bootctrl.py"
seq, floor = struct.unpack_from("<Q", first, 16)[0], struct.unpack_from("<Q", first, 24)[0]
assert seq == 0 and floor != 0, f"记录状态异常 seq={seq} floor={floor}"
digest = hashlib.sha256(first).digest()

anchor = (
    b"ENRBCA01"
    + struct.pack("<HH", 1, 64)
    + b"\x00" * 4
    + struct.pack("<Q", seq)
    + struct.pack("<Q", floor)
    + digest
)
assert len(anchor) == 64
old = bytes(data[payload_at:payload_at + 64])
data[payload_at:payload_at + 64] = anchor
open(VARS_PATH, "wb").write(data)
print(
    f"anchor repaired: vars={VARS_PATH} seq={seq} floor={floor} "
    f"digest={digest.hex()[:16]}.. (旧 digest={old[32:].hex()[:16]}..)"
)
