//! 本次启动见证（boot witness）wire 契约（蓝图 77 卷二 WP-77.2.2）。
//!
//! 根因（REG-70.5-CAPSULE-STALENESS）：固定 capsule 页在暖重启下可残留
//! 上一次 boot 的 CRC 合法记录——CRC 只检损坏，不证明"本次启动"。本卷
//! 建立**经真实 handoff 的独立见证**：boot-manager 验签后 seal
//! [`crate::BootInfo`]（含 `security_floor`）→ elfloader 入口验证 arg
//! 指针的 256B 记录并复制 → 以**版本化信封**写入本模块定义的固定
//! mailbox 页 → 内核快照并作为具名 extra bootinfo chunk 交给初始线程 →
//! root 从 seL4 BootInfo extra 区取出并逐字段比较 capsule。
//!
//! 链上每一跳的地址/ID/长度字面全部单源在本模块；C 侧（elfloader/
//! 内核补丁）的字面由 `boot-protocol` 的对拍测试互锁
//! （`tests/witness_contract_lock.rs`）。
//!
//! 信任语义：见证不是认证（无签名）——它把"本记录来自**本次**
//! boot-manager 验签链"变成 handoff 结构事实（reach elfloader ⇒
//! WP-77.2.1 的硬失败已关掉一切绕过路径）；新鲜性裁决在 root 侧
//! 六字段比较（capsule 与 witness 同 boot 才一致）。缺席/坏 ABI/
//! 字段错配都不得推送 OTA 身份或注入身份页。

use crate::boot_info::{ENEROS_INIT_BUNDLE_PHYS, ENEROS_MAPPED_WINDOW_LEN};

/// 见证信封魔数（"ENRWIT1\0"——EnerOS Boot Witness mailbox v1）。
pub const ENEROS_BOOT_WITNESS_MAGIC: [u8; 8] = *b"ENRWIT1\0";
/// 见证信封版本（版本化独立传递——非第七寄存器参数）。
pub const ENEROS_BOOT_WITNESS_VERSION: u32 = 1;
/// 信封编码长度：magic(8) + version(4) + len(4) + payload(256)。
pub const ENEROS_BOOT_WITNESS_ENVELOPE_LEN: usize = 16 + crate::boot_info::BOOT_INFO_SIZE;

/// 见证页预留（单源 = `boot_info::ENEROS_BOOT_WITNESS_RESERVE`）。
pub use crate::boot_info::ENEROS_BOOT_WITNESS_RESERVE;

/// 见证 mailbox 固定物理地址 = capsule 页前一页（BOOT-007 内核映射窗
/// 内、DTB 窗之外——`dtb_window_for` 已同步收缩预留；overlay 保留窗
/// 32 MiB 覆盖 ⇒ 内核 freemem/untyped 永不触及）。
pub const ENEROS_BOOT_WITNESS_MAILBOX_PHYS: u64 = ENEROS_INIT_BUNDLE_PHYS
    + ENEROS_MAPPED_WINDOW_LEN
    - crate::identity::IDENTITY_CAPSULE_RESERVE
    - ENEROS_BOOT_WITNESS_RESERVE;

/// seL4 extra bootinfo 里本卷具名 chunk 的 ID（vendor 命名空间字面
/// "ENR7"；上游保留 0..=6，内核补丁与本常量逐字节互锁）。
pub const SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS: u64 = 0x454E_5237;

/// seL4 extra bootinfo 区相对 BootInfo 帧基址的偏移（= seL4
/// `seL4_BootInfoFrameSize` = 4 KiB；bootinfo_types.h 契约：extra 区
/// "starting at the offset seL4_BootInfoFrameSize"）。
pub const SEL4_BOOTINFO_EXTRA_REGION_OFFSET: u64 = 0x1000;

const _: () = assert!(
    ENEROS_BOOT_WITNESS_MAILBOX_PHYS
        == crate::identity::IDENTITY_CAPSULE_PHYS - ENEROS_BOOT_WITNESS_RESERVE,
    "mailbox 页必须与 capsule 页相邻（前一页）"
);
const _: () = assert!(ENEROS_BOOT_WITNESS_ENVELOPE_LEN == 272);

/// mailbox 信封合法 payload 切片提取（host/target 共用纯函数）：
/// magic + version + len 判据，任一不符 → `None`（非 EnerOS 直启 /
/// 陈旧乱字节 = 见证缺席，不猜测）。
pub fn parse_witness_envelope(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() != ENEROS_BOOT_WITNESS_ENVELOPE_LEN {
        return None;
    }
    if bytes[0..8] != ENEROS_BOOT_WITNESS_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    if version != ENEROS_BOOT_WITNESS_VERSION {
        return None;
    }
    let len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    if len != crate::boot_info::BOOT_INFO_SIZE {
        return None;
    }
    Some(&bytes[16..])
}

/// seL4 extra bootinfo chunk 链中的具名见证 chunk 提取（root 消费入口；
/// chunk 布局 = `seL4_BootInfoHeader { id: u64, len: u64 }` + payload，
/// len 含头）。链走完未命中 / id 不符 / len ≠ 16+256 → `None`。
pub fn parse_witness_extra_chunk(extra: &[u8]) -> Option<[u8; crate::boot_info::BOOT_INFO_SIZE]> {
    let mut off = 0usize;
    while off + 16 <= extra.len() {
        let id = u64::from_le_bytes(extra[off..off + 8].try_into().ok()?);
        let len = u64::from_le_bytes(extra[off + 8..off + 16].try_into().ok()?) as usize;
        // 溢出 fail-closed：len 域来自内存（u64→usize 无损），off+len 回绕会
        // 绕过界检查致滑窗错位/切片 panic——违背本函数「不滑窗、不猜测」合同。
        if len < 16 || off.checked_add(len).map_or(true, |end| end > extra.len()) {
            return None; // 链损坏：fail-closed（不继续滑窗）
        }
        if id == SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS {
            let payload = &extra[off + 16..off + len];
            let out: [u8; crate::boot_info::BOOT_INFO_SIZE] = payload.try_into().ok()?;
            return Some(out);
        }
        off += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot_info::{BootInfo, BootReason, BootSlot};

    fn sealed_record() -> [u8; crate::boot_info::BOOT_INFO_SIZE] {
        let mut info = BootInfo::new(BootSlot::A, BootReason::Cold);
        info.boot_id = [0x11; 16];
        info.security_version = 3;
        info.set_security_floor(2);
        info.bootctrl_generation = 7;
        info.kernel_entry = 0x4100_0000;
        info.seal();
        info.to_le_bytes()
    }

    fn envelope(record: &[u8]) -> alloc::vec::Vec<u8> {
        let mut env = alloc::vec::Vec::new();
        env.extend_from_slice(&ENEROS_BOOT_WITNESS_MAGIC);
        env.extend_from_slice(&ENEROS_BOOT_WITNESS_VERSION.to_le_bytes());
        env.extend_from_slice(&(crate::boot_info::BOOT_INFO_SIZE as u32).to_le_bytes());
        env.extend_from_slice(record);
        env
    }

    #[test]
    fn mailbox_is_page_before_capsule() {
        assert_eq!(
            ENEROS_BOOT_WITNESS_MAILBOX_PHYS,
            crate::identity::IDENTITY_CAPSULE_PHYS - 0x1000
        );
        // DTB 窗不再触及见证页（dtb_window_for 已收缩预留）。
        let (start, len) = crate::boot_info::dtb_window_for(0x1000).expect("窗口存在");
        assert_eq!(start + len, ENEROS_BOOT_WITNESS_MAILBOX_PHYS);
    }

    #[test]
    fn envelope_roundtrip_and_negatives() {
        let rec = sealed_record();
        let env = envelope(&rec);
        assert_eq!(env.len(), ENEROS_BOOT_WITNESS_ENVELOPE_LEN);
        assert_eq!(parse_witness_envelope(&env), Some(&rec[..]));

        // 魔数错 / 版本错 / len 错 / 截断 = 缺席。
        let mut bad = env.clone();
        bad[0] = b'X';
        assert_eq!(parse_witness_envelope(&bad), None);
        let mut bad = env.clone();
        bad[8] = 9;
        assert_eq!(parse_witness_envelope(&bad), None);
        let mut bad = env.clone();
        bad[13] = 0; // len 256→0（LE 第二字节清零）
        assert_eq!(parse_witness_envelope(&bad), None);
        assert_eq!(parse_witness_envelope(&env[..env.len() - 1]), None);
        // 全零页（直启/未写）= 缺席。
        assert_eq!(
            parse_witness_envelope(&[0u8; ENEROS_BOOT_WITNESS_ENVELOPE_LEN]),
            None
        );
    }

    #[test]
    fn extra_chunk_walk_finds_named_witness() {
        let rec = sealed_record();
        // 模拟内核 extra 区：padding 头 + 见证 chunk + 尾 padding。
        let mut extra: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        // ① padding chunk（id=0, len=16）。
        extra.extend_from_slice(&0u64.to_le_bytes());
        extra.extend_from_slice(&16u64.to_le_bytes());
        // ② FDT chunk（id=6，仿内核先写 DTB 后写见证/或反之均可——链有序）。
        extra.extend_from_slice(&6u64.to_le_bytes());
        extra.extend_from_slice(&20u64.to_le_bytes());
        extra.extend_from_slice(&[0xd0, 0x0d, 0xfe, 0xed]);
        // ③ 见证 chunk。
        extra.extend_from_slice(&SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS.to_le_bytes());
        extra.extend_from_slice(&((16 + rec.len()) as u64).to_le_bytes());
        extra.extend_from_slice(&rec);
        assert_eq!(parse_witness_extra_chunk(&extra), Some(rec));

        // 见证 chunk 在首（root 只认 id，不依赖位置）。
        let mut first: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        first.extend_from_slice(&SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS.to_le_bytes());
        first.extend_from_slice(&((16 + rec.len()) as u64).to_le_bytes());
        first.extend_from_slice(&rec);
        first.extend_from_slice(&0u64.to_le_bytes());
        first.extend_from_slice(&16u64.to_le_bytes());
        assert_eq!(parse_witness_extra_chunk(&first), Some(rec));
    }

    #[test]
    fn extra_chunk_negatives_fail_closed() {
        let rec = sealed_record();
        // 无见证 chunk（非 EnerOS 直启）→ None。
        let no_witness = [0u8; 32];
        assert_eq!(parse_witness_extra_chunk(&no_witness), None);

        // id 对但 len 域坏（<16 / 越界）→ None（链损坏不滑窗）。
        let mut bad: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        bad.extend_from_slice(&SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS.to_le_bytes());
        bad.extend_from_slice(&8u64.to_le_bytes());
        assert_eq!(parse_witness_extra_chunk(&bad), None);
        let mut bad2: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        bad2.extend_from_slice(&SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS.to_le_bytes());
        bad2.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(parse_witness_extra_chunk(&bad2), None);

        // id 对、len 对、payload 短（截断记录）→ None。
        let mut short: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        short.extend_from_slice(&SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS.to_le_bytes());
        short.extend_from_slice(&((16 + rec.len()) as u64).to_le_bytes());
        short.extend_from_slice(&rec[..rec.len() - 1]);
        assert_eq!(parse_witness_extra_chunk(&short), None);

        // 空区（extraLen=0）→ None。
        assert_eq!(parse_witness_extra_chunk(&[]), None);
    }

    /// 链损坏（len 域 = u64::MAX）必须 `None` fail-closed——`off + len`
    /// 整型溢出（debug panic / release 回绕绕过界检查）违背本函数
    /// 「不滑窗、不猜测」合同。首块放合法非目标 chunk 使 off>0。
    #[test]
    fn extra_chunk_overflow_len_fails_closed() {
        let mut extra: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        extra.extend_from_slice(&0u64.to_le_bytes());
        extra.extend_from_slice(&16u64.to_le_bytes());
        extra.extend_from_slice(&SEL4_BOOTINFO_EXTRA_CHUNK_ENEROS_WITNESS.to_le_bytes());
        extra.extend_from_slice(&u64::MAX.to_le_bytes());
        extra.extend_from_slice(&[0u8; 32]);
        assert_eq!(parse_witness_extra_chunk(&extra), None);
    }
}
