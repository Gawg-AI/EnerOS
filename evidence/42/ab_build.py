#!/usr/bin/env python3
"""42（WP-42.6 启动矩阵"update A→B trial/commit"）证据生成辅助。

把 base raw（旧候选，sv=N）与 target raw（新候选，sv=N+1）合成一张
A/B 试炼盘：
- A 槽分区（BOOT_A/SYS_A）保留 base 内容（旧 manifest）；
- B 槽分区（BOOT_B/SYS_B）替换为 target 内容（新 manifest）；
- BOOTCTRL 写成"已分阶段"状态（OTA 服务在生产中写入的状态）：
  slot_a = A 槽成功项（旧 release），slot_b = B 槽 pending trial
  （新 release，tries=1, successful=false）→ 引导管理器应选择 B 并
  以 UPDATE_TRIAL 原因引导，验签通过后进入产品就绪；
- `--commit` 模式把 slot_b 写成成功项（B 已成为活跃槽位）。

任何"分阶段"状态都与生产 OTA 服务写入的字节语义一致（boot-protocol
encode/decode 双向一致；CRC32 与 boot-manager 一致）。本工具只产生
盘面状态，不读写 NVRAM anchor——anchor 由引导管理器首次引导初始化。

用法：
  python3 evidence/42/ab_build.py <base.raw> <target.raw> <out.raw> [--commit]
"""
import struct
import sys

SECTOR = 512
CRC_OFFSET = 4092  # BootControl::CRC_OFFSET = 4096 - 4
SLOT_SIZE = 64
SLOT_A_OFFSET = 64
SLOT_B_OFFSET = SLOT_A_OFFSET + SLOT_SIZE
MANIFEST_HDR_OFFSET = 0  # MANIFEST_MEDIA_OFFSET = 0（分区首字节）
B_SIDE_PARTITIONS = ("BOOT_B", "SYS_B")


def crc32(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xEDB88320 if crc & 1 else crc >> 1
    return (~crc) & 0xFFFFFFFF


def parse_gpt(path: str):
    """返回 {partition_name: (first_lba, last_lba)}。"""
    with open(path, "rb") as f:
        disk = f.read()
    if disk[510:512] != b"\x55\xaa":
        raise SystemExit("no protective MBR")
    hdr = disk[SECTOR : SECTOR * 2]
    if hdr[0:8] != b"EFI PART":
        raise SystemExit("no GPT header")
    entry_lba = struct.unpack_from("<Q", hdr, 72)[0]
    num_entries = struct.unpack_from("<I", hdr, 80)[0]
    entry_size = struct.unpack_from("<I", hdr, 84)[0]
    parts = {}
    for i in range(num_entries):
        off = entry_lba * SECTOR + i * entry_size
        ent = disk[off : off + entry_size]
        if ent == b"\x00" * entry_size:
            continue
        first = struct.unpack_from("<Q", ent, 32)[0]
        last = struct.unpack_from("<Q", ent, 40)[0]
        name = ent[56:128].decode("utf-16-le").rstrip("\x00")
        parts[name] = (first, last)
    return disk, parts


def read_manifest_sv_release(disk: bytes, parts: dict, name: str):
    first, last = parts[name]
    start = first * SECTOR
    header = disk[start + MANIFEST_HDR_OFFSET : start + MANIFEST_HDR_OFFSET + 112]
    if header[0:8] != b"ENERMAN1":
        raise SystemExit(f"{name}: no manifest magic {header[0:8]!r}")
    sv = struct.unpack_from("<Q", header, 52)[0]
    release_id = header[84:100]
    return sv, release_id


def make_record(seq, floor, rec_sv, rec_id, slot_a, slot_b, flags=0):
    rec = bytearray(4096)
    rec[0:8] = b"ENERBC01"
    struct.pack_into("<H", rec, 8, 1)  # BOOT_CONTROL_VERSION
    struct.pack_into("<H", rec, 10, 192)  # BOOT_CONTROL_HEADER_SIZE
    struct.pack_into("<I", rec, 12, 4096)  # BOOT_CONTROL_RECORD_SIZE
    struct.pack_into("<Q", rec, 16, seq)
    struct.pack_into("<Q", rec, 24, floor)
    struct.pack_into("<Q", rec, 32, rec_sv)
    rec[40:56] = rec_id
    struct.pack_into("<I", rec, 56, flags)

    def put_slot(offset, prio, tries, successful, sv, image_id):
        struct.pack_into("<BBB", rec, offset, prio, tries, 1 if successful else 0)
        struct.pack_into("<Q", rec, offset + 8, sv)
        rec[offset + 16 : offset + 32] = image_id
        struct.pack_into("<I", rec, offset + 32, 0)  # SlotError::None
        struct.pack_into("<Q", rec, offset + 40, 0)  # boot_attempts

    put_slot(SLOT_A_OFFSET, *slot_a)
    put_slot(SLOT_B_OFFSET, *slot_b)
    struct.pack_into("<I", rec, CRC_OFFSET, crc32(bytes(rec[:CRC_OFFSET])))
    return bytes(rec)


def main():
    if len(sys.argv) not in (4, 5):
        raise SystemExit(
            "usage: ab_build.py <base.raw> <target.raw> <out.raw> [--commit]"
        )
    base_path, target_path, out_path = sys.argv[1:4]
    commit = len(sys.argv) == 5 and sys.argv[4] == "--commit"

    base, base_parts = parse_gpt(base_path)
    target, target_parts = parse_gpt(target_path)

    # 1) base → out，替换 B 槽分区内容（BOOT_B/SYS_B）。
    out = bytearray(base)
    for name in B_SIDE_PARTITIONS:
        b_first, b_last = base_parts[name]
        t_first, t_last = target_parts[name]
        b_len = (b_last - b_first + 1) * SECTOR
        t_len = (t_last - t_first + 1) * SECTOR
        if b_len != t_len:
            raise SystemExit(f"{name}: base size {b_len} != target size {t_len}")
        out[b_first * SECTOR : (b_last + 1) * SECTOR] = target[
            t_first * SECTOR : (t_last + 1) * SECTOR
        ]

    # 2) 从 base 的 BOOTCTRL 取 recovery 字段 + security_floor。
    b_first, _ = base_parts["BOOTCTRL"]
    ctrl = base[b_first * SECTOR : b_first * SECTOR + 4096]
    if ctrl[0:8] != b"ENERBC01":
        raise SystemExit("base BOOTCTRL has no magic (run image build first)")
    floor = struct.unpack_from("<Q", ctrl, 24)[0]
    rec_sv = struct.unpack_from("<Q", ctrl, 32)[0]
    rec_id = ctrl[40:56]
    if b_first != target_parts["BOOTCTRL"][0]:
        raise SystemExit("BOOTCTRL partition moved between base and target")

    # 3) 从分区 manifest 读取 sv/release_id（不猜测任何地址/标识）。
    a_sv, a_release = read_manifest_sv_release(base, base_parts, "BOOT_A")
    b_sv, b_release = read_manifest_sv_release(target, target_parts, "BOOT_B")

    # 4) 组装分阶段控制记录。
    slot_a = (14, 0, True, a_sv, a_release)  # 旧槽成功项
    slot_b = (15, 1, False, b_sv, b_release) if not commit else (
        15,
        0,
        True,
        b_sv,
        b_release,
    )
    record = make_record(0, floor, rec_sv, rec_id, slot_a, slot_b)
    ctrl_start = b_first * SECTOR
    out[ctrl_start : ctrl_start + 4096] = record
    out[ctrl_start + 4096 : ctrl_start + 8192] = record
    with open(out_path, "wb") as f:
        f.write(bytes(out))

    mode = "commit" if commit else "trial"
    print(
        f"AB-BUILD {mode} out={out_path} "
        f"A=(sv={a_sv} release={a_release.hex()}) "
        f"B=(sv={b_sv} release={b_release.hex()}) "
        f"floor={floor} crc={record[CRC_OFFSET]:08x}"
    )


if __name__ == "__main__":
    main()