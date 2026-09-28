//! WP-77.2.2（蓝图 77 卷二）：boot witness 契约对拍门。
//!
//! 锁定见证链跨面字面——Rust 常量（boot-protocol::witness）↔ elfloader
//! 补丁（0003）↔ 内核补丁（0004）三面互不锁、注释已腐化的历史病灶
//! 复发即红（R15-B 同款工艺，74 卷 window_contract_lock.rs 先例）。
//!
//! R14 守卫自咬教训：期望串一律由常量运行期 format! 拼接，**测试文本
//! 禁止出现裸字面**（include_str 文本与针文本互数）。

use eneros_boot_protocol::witness::{
    ENEROS_BOOT_WITNESS_ENVELOPE_LEN, ENEROS_BOOT_WITNESS_MAILBOX_PHYS,
    ENEROS_BOOT_WITNESS_VERSION, SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS,
};
use eneros_boot_protocol::IDENTITY_CAPSULE_PHYS;

const ELFLOADER_PATCH: &str =
    include_str!("../../../../third_party/sel4-patches/0003-elfloader-eneros-boot-witness.patch");
const KERNEL_PATCH: &str =
    include_str!("../../../../third_party/sel4-patches/0004-kernel-eneros-witness-extra-bi.patch");
const CONVERGE: &str = include_str!("../../../../third_party/sel4-converge-patches.sh");

/// `#define NAME 0x...UL` 形态的字面提取（剥除注释后按名定位）。
fn define_value(src: &str, name: &str) -> u64 {
    let line = match src
        .lines()
        .find(|l| l.contains(&format!("#define {name} ")))
    {
        Some(l) => l.to_string(),
        None => format!("#define {name} 0xMISSING"),
    };
    let raw = line
        .split(&format!("#define {name} "))
        .nth(1)
        .unwrap_or("0xMISSING")
        .trim()
        .trim_end_matches("UL")
        .trim();
    u64::from_str_radix(&raw.replace('_', "").replace("0x", ""), 16).unwrap_or(u64::MAX)
}

#[test]
fn mailbox_paddr_consistent_across_c_patches() {
    let expected = ENEROS_BOOT_WITNESS_MAILBOX_PHYS;
    // mailbox 页 = capsule 页前一页（同窗双预留）。
    assert_eq!(expected, IDENTITY_CAPSULE_PHYS - 0x1000);
    let want = format!("{:#x}", expected)
        .to_uppercase()
        .replace("0X", "0x");
    for (name, src) in [("0003", ELFLOADER_PATCH), ("0004", KERNEL_PATCH)] {
        assert_eq!(
            define_value(src, "ENEROS_WITNESS_MAILBOX_PADDR"),
            expected,
            "{name}: mailbox 物理址与 boot-protocol 常量漂移（期望 {want}）"
        );
    }
}

#[test]
fn chunk_id_consistent_with_kernel_patch() {
    let got = define_value(KERNEL_PATCH, "ENEROS_WITNESS_CHUNK_ID");
    assert_eq!(
        got, SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS,
        "内核补丁 chunk id 与 boot-protocol 常量漂移"
    );
}

#[test]
fn envelope_shape_consistent_across_c_patches() {
    // 信封长度 = 16 头 + 256 payload；两侧补丁各自持同一算式。
    assert_eq!(ENEROS_BOOT_WITNESS_ENVELOPE_LEN, 272);
    for (name, src) in [("0003", ELFLOADER_PATCH), ("0004", KERNEL_PATCH)] {
        assert!(
            src.contains("#define ENEROS_WITNESS_PAYLOAD_LEN 256"),
            "{name}: payload 长度字面漂移（wire 256B 冻结）"
        );
        assert!(
            src.contains("#define ENEROS_WITNESS_ENVELOPE_LEN (16 + ENEROS_WITNESS_PAYLOAD_LEN)"),
            "{name}: 信封长度算式漂移（magic8+version4+len4+256）"
        );
    }
    // 版本字面（u32 LE = 1）：两补丁的版本校验/构形都在 8..12 字节。
    assert_eq!(ENEROS_BOOT_WITNESS_VERSION, 1);
}

#[test]
fn converge_chain_manages_both_patches() {
    // 收敛器必须把 0003/0004 纳入受管链（否则缓存树漂移不收敛——
    // 定义值仍在、构建面回退的静默断裂形态）。
    assert!(
        CONVERGE.contains("0004-kernel-eneros-witness-extra-bi.patch"),
        "converge 未管理内核 witness 补丁"
    );
    assert!(
        CONVERGE.contains("0003-elfloader-eneros-boot-witness.patch"),
        "converge 未管理 elfloader witness 补丁"
    );
    // 终态 sha 与本仓受管补丁链绑定（指纹出现即表——非手抄值的弱锁，
    // 强锁在 converge 的 sha256 校验本体）。
    assert!(CONVERGE.contains("BOOT_C_SHA="));
    assert!(CONVERGE.contains("SYS_BOOT_C_SHA="));
}

/// WP-77.2.2 次生缺陷回归锁（QEMU 实弹 2026-09-28）：BootInfo 结构体
/// 的**内存字节**必须与 wire 编码逐字节一致——boot-manager 把结构体
/// write 到页上、elfloader/kernel 按 wire 布局直接读。repr(C) 与
/// to_le_bytes 一旦分裂（floor 曾用 u64 字段被 8 对齐挪到 240、crc32
/// 被推到 248），本测即红。置于集成测试面（lib forbid(unsafe)）。
#[test]
fn boot_info_struct_memory_layout_matches_wire() {
    use eneros_boot_protocol::boot_info::{BootInfo, BootReason, BootSlot, BOOT_INFO_SIZE};
    let mut info = BootInfo::new(BootSlot::A, BootReason::Cold);
    info.boot_id = [0x11; 16];
    info.manifest_digest = [0x22; 32];
    info.bootctrl_generation = 7;
    info.security_version = 3;
    info.set_security_floor(2);
    info.kernel_entry = 0x4100_0000;
    info.seal();
    let mem: [u8; BOOT_INFO_SIZE] =
        unsafe { core::ptr::read(&info as *const BootInfo as *const [u8; BOOT_INFO_SIZE]) };
    assert_eq!(mem, info.to_le_bytes(), "BootInfo 内存布局 ≠ wire 布局");
    // wire 侧 CRC 域位置复核（elfloader 按 240 域校验）。
    let stored = u32::from_le_bytes(mem[240..244].try_into().unwrap());
    assert_ne!(stored, 0);
}

#[test]
fn kernel_patch_records_chunk_publish_and_snapshot_order() {
    // 快照必须是 try_init_kernel 首语句（注释锚）——populate_bi_frame 的
    // clearMemory 会回收 freemem 页，快照晚于它 = 物理前提失效。
    assert!(KERNEL_PATCH.contains("eneros_witness_snapshot();"));
    assert!(
        KERNEL_PATCH.contains("FIRST -- copy the boot witness"),
        "快照时序锚注释缺失（首语句位置失去自述）"
    );
    // extra chunk 发布 + 容量记账两半边都在。
    assert!(KERNEL_PATCH.contains("header.id = ENEROS_WITNESS_CHUNK_ID;"));
    assert!(KERNEL_PATCH.contains("extra_bi_size += sizeof(seL4_BootInfoHeader)"));
}
