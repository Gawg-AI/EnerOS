//! 可信身份 capsule（蓝图 70.5 WP-70.5.1 / WP-70.5.4）。
//!
//! boot-manager 在验签 ReleaseManifest 并解析匹配 protected anchor 的
//! BootControl 之后，把本次启动的**已验证身份事实**编码为本 capsule，
//! 写入 BOOT-007 内核映射窗口尾页（[`IDENTITY_CAPSULE_PHYS`]，1:1 映射进
//! root-task VSpace）。root-task 经 [`IdentityCapsule::decode`] 校验后
//! 建立可信身份快照，并经 root 独占的 EIPC 通道分发给 svc-ota——MGT1
//! 管理请求不能构造它。
//!
//! # 绑定与信任语义
//!
//! - CRC32 只检损坏，**不构成认证**；认证来自 boot-manager 的验签链
//!   （ReleaseManifest Ed25519 + BootControl protected anchor 匹配）——
//!   capsule 是该链的只读投影，攻击者改写 capsule 字节必须同时伪造
//!   anchor 匹配的 BootControl 与验签 manifest 才能产出一致视图。
//! - capsule 与 manifest_digest / boot_id / bootctrl_generation 绑定
//!   （字段内嵌）；消费方（svc-ota）不得把 capsule 的 security_version
//!   当作 security_floor 的别名——两者分开字段，分别来自启动镜像签名
//!   声明与 anchor 匹配的水位。
//! - 与 dtb_window_for 同窗共存：DTB 窗口末端为 capsule 页预留
//!   （[`IDENTITY_CAPSULE_RESERVE`]）——boot-manager 与 root-task 经
//!   同一 `dtb_window_for` 推导，两端自动一致。
//!
//! # ABI 兼容
//!
//! capsule 自带 magic + version + size；旧 root-task 不读该页（零行为
//! 变化）；新 reader 对未知 version 拒绝（不静默复用 reserved 字节）。
//! 无 capsule（qemu-test 直启 / capsule 写失败）→ 身份不可得 → OTA
//! not-ready（fail-closed，不回 0.0.0/qemu 默认身份）。

use eneros_ipc_wire::crc32;

use crate::boot_info::{ENEROS_INIT_BUNDLE_PHYS, ENEROS_MAPPED_WINDOW_LEN};

/// capsule 魔数（"ENRIDC1\0"——EnerOS Identity Capsule v1）。
pub const IDENTITY_CAPSULE_MAGIC: [u8; 8] = *b"ENRIDC1\0";
/// capsule 协议版本。
pub const IDENTITY_CAPSULE_VERSION: u16 = 1;
/// capsule 编码长度（字节；定长布局）。
pub const IDENTITY_CAPSULE_SIZE: usize = 136;
/// CRC 覆盖范围 = [`0..IDENTITY_CAPSULE_CRC_OFFSET`)；CRC 字段本身除外。
pub const IDENTITY_CAPSULE_CRC_OFFSET: usize = 132;
/// capsule 固定占用页数（1 页 = 4 KiB；窗口尾页整页预留防邻接写入）。
pub const IDENTITY_CAPSULE_RESERVE: u64 = 0x1000;

/// capsule 固定物理地址 = BOOT-007 内核映射窗口尾页（1:1 映射进初始
/// 线程 VSpace；root-task 直接 volatile 读）。boot-manager 以
/// `AllocateType::Address` 显式锁定该页（在 kernel/init bundle 等
/// AnyPages 分配**之前**——防 UEFI 自顶向下分配撞页）。
pub const IDENTITY_CAPSULE_PHYS: u64 =
    ENEROS_INIT_BUNDLE_PHYS + ENEROS_MAPPED_WINDOW_LEN - IDENTITY_CAPSULE_RESERVE;

/// 已验证身份事实（boot-manager 验签链的只读投影）。
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityCapsule {
    pub magic: [u8; 8],
    pub version: u16,
    pub size: u16,
    /// selected_slot（[`BootSlot`] 的 u8 形态）。
    pub selected_slot: u8,
    pub reserved0: [u8; 3],
    pub boot_id: [u8; 16],
    /// 验签 ReleaseManifest 的 release_id（header 字段投影）。
    pub release_id: [u8; 16],
    /// 发布平台绑定（manifest header platform_id——SKU 语义）。
    pub sku: [u8; 16],
    pub manifest_digest: [u8; 32],
    pub bootctrl_generation: u64,
    /// 当前启动镜像的安全版本（镜像签名声明；≠ security_floor）。
    pub security_version: u64,
    /// 匹配 protected anchor 的可信水位（BootControl security_floor）。
    pub security_floor: u64,
    /// 产品语义版本（major/minor/patch，u16×3——构建身份投影）。
    pub product_version: [u16; 3],
    pub reserved1: [u8; 6],
    pub crc32: u32,
}

const _: () = assert!(core::mem::size_of::<IdentityCapsule>() == IDENTITY_CAPSULE_SIZE);
const _: () = assert!(core::mem::align_of::<IdentityCapsule>() == 8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCapsuleError {
    Length,
    Magic,
    Version,
    Size,
    Reserved,
    Slot,
    Identity,
    Crc,
}

impl IdentityCapsule {
    /// 由已验证启动事实构造（boot-manager 唯一生产者）。
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        selected_slot: u8,
        boot_id: [u8; 16],
        release_id: [u8; 16],
        sku: [u8; 16],
        manifest_digest: [u8; 32],
        bootctrl_generation: u64,
        security_version: u64,
        security_floor: u64,
        product_version: [u16; 3],
    ) -> Self {
        Self {
            magic: IDENTITY_CAPSULE_MAGIC,
            version: IDENTITY_CAPSULE_VERSION,
            size: IDENTITY_CAPSULE_SIZE as u16,
            selected_slot,
            reserved0: [0; 3],
            boot_id,
            release_id,
            sku,
            manifest_digest,
            bootctrl_generation,
            security_version,
            security_floor,
            product_version,
            reserved1: [0; 6],
            crc32: 0,
        }
    }

    /// 计算 CRC 并封印（写盘/写出前调用）。
    pub fn seal(&mut self) {
        self.crc32 = 0;
        self.crc32 = crc32(&self.to_bytes()[..IDENTITY_CAPSULE_CRC_OFFSET]);
    }

    /// 结构合法性（不含 CRC——构造侧即检；decode 内含 CRC 复核）。
    pub fn validate(&self) -> Result<(), IdentityCapsuleError> {
        if self.magic != IDENTITY_CAPSULE_MAGIC {
            return Err(IdentityCapsuleError::Magic);
        }
        if self.version != IDENTITY_CAPSULE_VERSION {
            return Err(IdentityCapsuleError::Version);
        }
        if self.size as usize != IDENTITY_CAPSULE_SIZE {
            return Err(IdentityCapsuleError::Size);
        }
        // slot 域 = BootSlot::{A=1,B=2,Recovery=3}（boot_info::BootSlot 同值域）。
        if !(1..=3).contains(&self.selected_slot) {
            return Err(IdentityCapsuleError::Slot);
        }
        if self.reserved0 != [0; 3] || self.reserved1 != [0; 6] {
            return Err(IdentityCapsuleError::Reserved);
        }
        // 身份四元组非空（floor==0 / 全零 digest / 全零版本 = 无效投影）。
        if self.security_floor == 0 || self.security_version == 0 {
            return Err(IdentityCapsuleError::Identity);
        }
        if self.manifest_digest.iter().all(|b| *b == 0) {
            return Err(IdentityCapsuleError::Identity);
        }
        if self.product_version == [0u16; 3] {
            return Err(IdentityCapsuleError::Identity);
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> [u8; IDENTITY_CAPSULE_SIZE] {
        let mut b = [0u8; IDENTITY_CAPSULE_SIZE];
        b[0..8].copy_from_slice(&self.magic);
        b[8..10].copy_from_slice(&self.version.to_le_bytes());
        b[10..12].copy_from_slice(&self.size.to_le_bytes());
        b[12] = self.selected_slot;
        b[13..16].copy_from_slice(&self.reserved0);
        b[16..32].copy_from_slice(&self.boot_id);
        b[32..48].copy_from_slice(&self.release_id);
        b[48..64].copy_from_slice(&self.sku);
        b[64..96].copy_from_slice(&self.manifest_digest);
        b[96..104].copy_from_slice(&self.bootctrl_generation.to_le_bytes());
        b[104..112].copy_from_slice(&self.security_version.to_le_bytes());
        b[112..120].copy_from_slice(&self.security_floor.to_le_bytes());
        for (i, v) in self.product_version.iter().enumerate() {
            b[120 + i * 2..122 + i * 2].copy_from_slice(&v.to_le_bytes());
        }
        b[126..132].copy_from_slice(&self.reserved1);
        b[IDENTITY_CAPSULE_CRC_OFFSET..IDENTITY_CAPSULE_CRC_OFFSET + 4]
            .copy_from_slice(&self.crc32.to_le_bytes());
        b
    }

    /// 解码 + 全量校验（magic/version/size/reserved/域/CRC）。
    pub fn decode(bytes: &[u8]) -> Result<Self, IdentityCapsuleError> {
        if bytes.len() != IDENTITY_CAPSULE_SIZE {
            return Err(IdentityCapsuleError::Length);
        }
        let mut c = Self::new(
            bytes[12],
            bytes[16..32].try_into().expect("16B boot_id"),
            bytes[32..48].try_into().expect("16B release_id"),
            bytes[48..64].try_into().expect("16B sku"),
            bytes[64..96].try_into().expect("32B digest"),
            u64::from_le_bytes(bytes[96..104].try_into().expect("8B")),
            u64::from_le_bytes(bytes[104..112].try_into().expect("8B")),
            u64::from_le_bytes(bytes[112..120].try_into().expect("8B")),
            [
                u16::from_le_bytes(bytes[120..122].try_into().expect("2B")),
                u16::from_le_bytes(bytes[122..124].try_into().expect("2B")),
                u16::from_le_bytes(bytes[124..126].try_into().expect("2B")),
            ],
        );
        c.reserved0.copy_from_slice(&bytes[13..16]);
        c.reserved1.copy_from_slice(&bytes[126..132]);
        c.magic.copy_from_slice(&bytes[0..8]);
        c.version = u16::from_le_bytes(bytes[8..10].try_into().expect("2B"));
        c.size = u16::from_le_bytes(bytes[10..12].try_into().expect("2B"));
        c.crc32 = u32::from_le_bytes(
            bytes[IDENTITY_CAPSULE_CRC_OFFSET..IDENTITY_CAPSULE_CRC_OFFSET + 4]
                .try_into()
                .expect("4B crc"),
        );
        c.validate()?;
        // CRC 复核：清零 CRC 字段后重算，且要求规范重编码与输入逐字节
        // 一致（canonical 拒绝——任何未登记字节形态都不可通过）。
        if c.crc32 == 0 {
            return Err(IdentityCapsuleError::Crc);
        }
        let expected = c.crc32;
        c.crc32 = 0;
        let recomputed = crc32(&bytes[..IDENTITY_CAPSULE_CRC_OFFSET]);
        let mut canonical = c.to_bytes();
        canonical[IDENTITY_CAPSULE_CRC_OFFSET..IDENTITY_CAPSULE_CRC_OFFSET + 4]
            .copy_from_slice(&expected.to_le_bytes());
        if recomputed != expected || canonical.as_slice() != bytes {
            return Err(IdentityCapsuleError::Crc);
        }
        c.crc32 = expected;
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot_info::dtb_window_for;

    fn valid() -> IdentityCapsule {
        let mut c = IdentityCapsule::new(
            1,
            [0x11; 16],
            [0x22; 16],
            [0x33; 16],
            [0xA5; 32],
            7,
            3,
            2,
            [0, 335, 0],
        );
        c.seal();
        c
    }

    #[test]
    fn layout_and_roundtrip() {
        let c = valid();
        assert_eq!(core::mem::size_of::<IdentityCapsule>(), 136);
        let bytes = c.to_bytes();
        assert_eq!(&bytes[0..8], b"ENRIDC1\0");
        assert_eq!(IdentityCapsule::decode(&bytes), Ok(c));
    }

    #[test]
    fn single_byte_corruption_detected() {
        let bytes = valid().to_bytes();
        for i in 0..bytes.len() {
            let mut bad = bytes;
            bad[i] ^= 0x01;
            assert!(
                IdentityCapsule::decode(&bad).is_err(),
                "单字节翻转 @ {i} 必须被拒（CRC/域检）"
            );
        }
    }

    #[test]
    fn zero_floor_or_digest_or_version_rejected() {
        let mut c = valid();
        c.security_floor = 0;
        c.seal();
        assert_eq!(
            IdentityCapsule::decode(&c.to_bytes()),
            Err(IdentityCapsuleError::Identity)
        );

        let mut c = valid();
        c.manifest_digest = [0; 32];
        c.seal();
        assert_eq!(
            IdentityCapsule::decode(&c.to_bytes()),
            Err(IdentityCapsuleError::Identity)
        );

        let mut c = valid();
        c.product_version = [0; 3];
        c.seal();
        assert_eq!(
            IdentityCapsule::decode(&c.to_bytes()),
            Err(IdentityCapsuleError::Identity)
        );
    }

    #[test]
    fn unknown_version_and_bad_slot_rejected() {
        let mut c = valid();
        c.version = 2;
        c.seal();
        assert_eq!(
            IdentityCapsule::decode(&c.to_bytes()),
            Err(IdentityCapsuleError::Version)
        );

        let mut c = valid();
        c.selected_slot = 9;
        c.seal();
        assert_eq!(
            IdentityCapsule::decode(&c.to_bytes()),
            Err(IdentityCapsuleError::Slot)
        );
    }

    #[test]
    fn fixed_phys_is_last_window_page() {
        assert_eq!(
            IDENTITY_CAPSULE_PHYS,
            ENEROS_INIT_BUNDLE_PHYS + ENEROS_MAPPED_WINDOW_LEN - 0x1000
        );
        // dtb_window_for 预留后：DTB 窗口不触及 capsule 页，也不触及
        // 见证 mailbox 页（WP-77.2.2 双预留——capsule 页前一页）。
        let (start, len) = dtb_window_for(0x1000).expect("小 bundle 有窗口");
        assert_eq!(
            start + len,
            crate::witness::ENEROS_BOOT_WITNESS_MAILBOX_PHYS,
            "DTB 窗口末端必须 = 见证 mailbox 页起点（双预留单源）"
        );
        assert_eq!(
            crate::witness::ENEROS_BOOT_WITNESS_MAILBOX_PHYS + 0x1000,
            IDENTITY_CAPSULE_PHYS,
            "mailbox 页 = capsule 页前一页"
        );
    }
}
