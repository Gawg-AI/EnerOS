//! WP-27.1 真实字节 fixture 回归测试（§5.2 步骤 2：原始字节喂 `BootControl::decode`）。
//!
//! 字节来源：`cargo xtask run qemu-prod` 失败现场（bootctrl-resolve-fail
//! error=Genesis）从 GPT BOOTCTRL 分区（LBA 18432，双副本偏移 0/4096）导出的
//! 原始字节，落盘 evidence/boot-unblock/ 后复制入库。
//!
//! 场景：双副本一致、CRC 正确、seq=1（上一次成功引导后 `prepare_attempt`
//! 递增的合法状态）+ VARS 模板无保护锚点 → 无锚点路径的 Genesis 判定
//! （fail-closed）。本测试锁定：seq=1 副本是**合法可解码**状态，Genesis
//! 是语义正确的拒绝，而不是解析 bug（H2 排除）；CRC 与 ipc-wire 最终异或
//! 一致（H2 的 Crc 分支排除）。

use eneros_boot_protocol::{
    resolve_boot_control, BootControl, BootControlError, BootSlot, SlotError,
    BOOT_CONTROL_RECORD_SIZE,
};

const COPY_1: &[u8; BOOT_CONTROL_RECORD_SIZE] =
    include_bytes!("fixtures/bootctrl-seq1-copy1.bin");
const COPY_2: &[u8; BOOT_CONTROL_RECORD_SIZE] =
    include_bytes!("fixtures/bootctrl-seq1-copy2.bin");

/// 真实 seq=1 双副本：decode 必须通过（字段与 CRC 合法），证明失败不是解析 bug。
#[test]
fn real_seq1_dual_copy_decodes_with_valid_crc() {
    let control = BootControl::decode(COPY_1).expect("real seq=1 copy must decode");
    assert_eq!(control.sequence, 1, "fixture must be the post-attempt generation");
    assert_eq!(control.security_floor, 1);
    assert_eq!(control.flags, 0);
    // slot A = 尝试中（prepare_attempt 遗留：tries 3→2, last_error=AttemptInProgress, attempts=1）
    assert_eq!(control.slot_a.priority, 15);
    assert_eq!(control.slot_a.tries_remaining, 2);
    assert!(!control.slot_a.successful);
    assert_eq!(control.slot_a.last_error, SlotError::AttemptInProgress);
    assert_eq!(control.slot_a.boot_attempts, 1);
    // slot B = 已成功（工厂默认）
    assert_eq!(control.slot_b.priority, 14);
    assert!(control.slot_b.successful);
    // 重编码必须逐字节还原（encode 往返 → decode 的 canonical 检查成立）
    assert_eq!(control.encode().unwrap().as_slice(), COPY_1.as_slice());
    // 双副本一致是 Genesis 判定链的前置条件
    assert_eq!(COPY_1.as_slice(), COPY_2.as_slice());
}

/// 无锚点 + 合法但非 genesis 状态（seq≠0）→ Err(Genesis)：真实字节上的
/// fail-closed 语义锁定。**禁止**通过放宽本判定来"解锁"引导（§5.2 红线）。
#[test]
fn real_seq1_dual_copy_without_anchor_is_genesis() {
    let error = resolve_boot_control(COPY_1, COPY_2, None).expect_err("must reject non-genesis state");
    assert_eq!(error, BootControlError::Genesis);
}

/// 锚点存在时 seq=1 副本可被正常解析（与 VARS 锚点匹配的路径不受 Genesis 影响）。
#[test]
fn real_seq1_dual_copy_resolves_with_matching_anchor() {
    let control = BootControl::decode(COPY_1).unwrap();
    let anchor = eneros_boot_protocol::BootControlAnchor::new(&control, COPY_1).unwrap();
    let resolved = resolve_boot_control(COPY_1, COPY_2, Some(&anchor)).unwrap();
    assert_eq!(resolved.control.sequence, 1);
    assert!(!resolved.needs_anchor_initialization);
    assert_eq!(resolved.control.select().unwrap().slot, BootSlot::A);
}

/// 异或回退的 CRC（reset-bootctrl.py 旧算法，无最终异或）写入的副本必须被
/// decode 拒绝——锁定"重置脚本与 boot-manager 的 CRC 不一致"缺陷的判定面
/// （防回归：修好 reset 脚本后，此测试保证 wire CRC 语义不被再次破坏）。
#[test]
fn crc_without_final_xor_is_rejected() {
    let mut record = *COPY_1;
    // 用"无最终异或"算法重算 CRC 并覆写 → decode 必须报 Crc（H2 的 Crc 分支）
    let no_xor = |data: &[u8]| -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        crc // 故意省略 ipc-wire 的最终 `!crc`
    };
    let (prefix, crc_bytes) = record.split_at_mut(4092);
    let computed = no_xor(prefix);
    crc_bytes.copy_from_slice(&computed.to_le_bytes());
    assert_eq!(BootControl::decode(&record), Err(BootControlError::Crc));
}
