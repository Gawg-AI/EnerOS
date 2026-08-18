//! init bundle v2 解析器（P0-2，v0.199.0）.
//!
//! 解析由 `xtask` 的 `compose_init_bundle`（SVC-002 schema）打包的 init
//! bundle：`ENRINIT2` magic + 64 B header + 96 B/module 表 + ELF payload。
//! root-task 在 `_start` 中消费 BootInfo 后，从 `init_bundle_phys` 解析
//! bundle，得到服务清单（名称/类型/哈希），供 EL0 spawn 与心跳监控使用。
//!
//! 布局（与 compose 完全对齐）：
//!
//! ```text
//! 0x00  magic  "ENRINIT2"           (8 B)
//! 0x08  version u16 = 3             (2 B；v2 兼容接受：无能力位图)
//! 0x0A  header_len u16 = 64         (2 B)
//! 0x0C  total_len u32               (4 B)
//! 0x10  source_date_epoch u64       (8 B)
//! 0x18  module_count u32            (4 B)
//! 0x20  payload_sha256              (32 B)
//! 0x40  module table (96 B × n)
//!       0x00 kind u32 | 0x04 flags u32 | 0x08 offset u64 | 0x10 len u64
//!       0x18 name (32 B) | 0x38 sha256 (32 B)
//! 0x40 + 96n  payload
//!
//! flags 布局（WP-37.1 v3）：低 16 位 = criticality 编码（31/25/20/15）；
//! 高 16 位 = 能力位图（[`CAP_*`]，WP-37.2 唯一授权来源；未知位解析期
//! 拒绝——"capability 未声明即拒绝"由构造保证）。
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

extern crate alloc;

/// init bundle magic。
pub const INIT_BUNDLE_MAGIC: &[u8; 8] = b"ENRINIT2";
/// bundle 版本（v3：flags 高 16 位携带能力位图；v2 兼容接受——无位图）。
pub const INIT_BUNDLE_VERSION: u16 = 3;
/// 兼容接受的旧版本（v2：flags 仅 criticality）。
pub const INIT_BUNDLE_VERSION_V2: u16 = 2;

// ==================== WP-37.2：能力位图（flags 高 16 位）====================

/// 能力位掩码（flags 高 16 位；低 16 位为 criticality 编码）。
pub const CAPABILITY_BIT_MASK: u32 = 0xFFFF_0000;

/// 系统监督能力（root-task 唯一：服务装配/重启/审计裁决）。
pub const CAP_SUPERVISOR: u32 = 1 << 16;
/// endpoint 发送能力（服务经其 ep 发起 IPC）。
pub const CAP_EP_SEND: u32 = 1 << 17;
/// endpoint 接收能力（服务经其 ep 接收 IPC）。
pub const CAP_EP_RECV: u32 = 1 << 18;
/// 设备 MMIO/IRQ 访问能力（驱动服务；38 真实化后启用）。
pub const CAP_DEVICE_IO: u32 = 1 << 19;

/// 能力位 → 名称注册表（runtime 侧单一事实源；新增能力必须登记——
/// 未登记位在解析期拒绝，杜绝"声明了但 runtime 不认识"的静默漂移）。
pub const CAPABILITY_NAMES: &[(u32, &str)] = &[
    (CAP_SUPERVISOR, "supervisor"),
    (CAP_EP_SEND, "ep-send"),
    (CAP_EP_RECV, "ep-recv"),
    (CAP_DEVICE_IO, "device-io"),
];

/// 能力位图 → 名称列表（host 测试/证据用；位图非法位返回 None——
/// 解析期已拒绝，此处防御）。
pub fn capability_names(bits: u32) -> Option<alloc::vec::Vec<&'static str>> {
    let mut names = Vec::new();
    for (bit, name) in CAPABILITY_NAMES {
        if bits & bit != 0 {
            names.push(*name);
        }
    }
    Some(names)
}
/// header 长度。
pub const INIT_BUNDLE_HEADER_LEN: usize = 64;
/// 每模块表项长度。
pub const INIT_BUNDLE_ENTRY_LEN: usize = 96;
/// 模块名最大长度。
pub const INIT_BUNDLE_NAME_MAX: usize = 32;

/// 模块 kind（与 compose 的 `(kind, flags, ...)` 对应；**单一语义**：
/// 36 起 compose 侧 `product_deriver::init_bundle` 编码
/// 1=root / 2=required / 3=optional。旧 runtime 曾把 3 解释为
/// `MODULE_KIND_DRIVER`——与 optional 数值相同但语义漂移，已统一）。
pub const MODULE_KIND_ROOT: u32 = 1;
pub const MODULE_KIND_REQUIRED: u32 = 2;
pub const MODULE_KIND_OPTIONAL: u32 = 3;

/// 已知 criticality 编码（compose 侧 `product_deriver::init_bundle` 映射：
/// System=31 / Control=25 / Data=20 / Support=15）。
/// WP-37.1：未知 flag 位必须失败，禁止把未定义语义静默当 optional。
pub const KNOWN_CRITICALITY_FLAGS: [u32; 4] = [31, 25, 20, 15];

/// WP-37.1：关键性（36 compose flags 编码的类型化映射；蓝图枚举命名
/// SafetyCritical/Control/Platform/Business/Auxiliary 与 36 冻结 schema 的
/// 编码冲突，按 35-00 §9 排斥矩阵保留 36 语义、以本枚举提供类型面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Criticality {
    /// 系统级（root/信任基座）——flags=31。
    System = 31,
    /// 控制级（设备控制链）——flags=25。
    Control = 25,
    /// 数据级（采集/存储）——flags=20。
    Data = 20,
    /// 支持级（辅助服务）——flags=15。
    Support = 15,
}

impl Criticality {
    /// flags 值 → 关键性；未知编码拒绝（与 parse 的 flags 校验一致）。
    pub fn from_flags(flags: u32) -> Option<Self> {
        match flags {
            31 => Some(Self::System),
            25 => Some(Self::Control),
            20 => Some(Self::Data),
            15 => Some(Self::Support),
            _ => None,
        }
    }
}

/// 服务必需性（WP-37.1：required 集合与 manifest 完全相等，坏 required
/// 必阻断；optional 失败只产生明确降级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// 必需：解析/分配失败立即终止 product boot。
    Required,
    /// 可选：失败写 `CapabilityUnavailable` 降级，不阻断 required 集合。
    Optional,
}

/// init bundle 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleError {
    /// 长度不足（< header）。
    TooShort,
    /// magic 不匹配。
    BadMagic,
    /// 版本不支持。
    BadVersion,
    /// header 长度/表长度不一致。
    BadLayout,
    /// payload 总长越界。
    Truncated,
    /// 模块名非法（超长/含 NUL）。
    BadName,
    /// 模块 blob 摘要与表项 sha256 不符（30 RT-004：供应链完整性校验）。
    DigestMismatch,
    /// 模块 kind 不在 {root=1, required=2, optional=3}（WP-37.1：
    /// 未知 kind 不得静默跳过或降级——compose 与 runtime 单一语义）。
    UnknownKind,
    /// 模块名重复（WP-37.1：拒绝重复 ID）。
    DuplicateModule,
    /// flags 不在已知 criticality 编码集（31/25/20/15，WP-37.1：
    /// 未知 flag 位必须失败，不再承载未经定义的“10–31 优先级”）。
    BadFlags,
}

/// bundle 中的一个模块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleModule<'a> {
    /// 模块类型（root=1/required=2/optional=3）。
    pub kind: u32,
    /// criticality 标志（System=31/Control=25/Data=20/Support=15）。
    pub flags: u32,
    /// WP-37.2：能力位图（flags 高 16 位；唯一授权来源——声明即授予，
    /// 未声明即拒绝；v2 兼容 bundle 为 0）。
    pub capabilities: u32,
    /// 模块名（如 "svc-hello"）。
    pub name: String,
    /// ELF 字节（43 内存修约：**借用**内核映射 bundle 区——不再整包
    /// 拷入堆，避免 bundle 增大后 init OOM；映射生命周期 = root-task）。
    pub blob: &'a [u8],
    /// 模块 sha256（compose 写入的摘要）。
    pub sha256: [u8; 32],
}

impl BundleModule<'_> {
    /// 该模块的必需性（kind → Requirement）。
    pub fn requirement(&self) -> Requirement {
        match self.kind {
            MODULE_KIND_ROOT | MODULE_KIND_REQUIRED => Requirement::Required,
            MODULE_KIND_OPTIONAL => Requirement::Optional,
            _ => Requirement::Required, // 解析期已拒绝未知 kind，不可达
        }
    }
}

/// 解析后的 init bundle。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitBundle<'a> {
    /// bundle 总长。
    pub total_len: u32,
    /// 构建时间戳。
    pub source_date_epoch: u64,
    /// payload 整体 sha256。
    pub payload_sha256: [u8; 32],
    /// 模块清单。
    pub modules: Vec<BundleModule<'a>>,
}

impl<'a> InitBundle<'a> {
    /// 从原始字节解析 init bundle（v3 携带能力位图；v2 兼容接受——
    /// capabilities 为 0）。模块 blob 借用 `raw`（不拷贝）。
    pub fn parse(raw: &'a [u8]) -> Result<Self, BundleError> {
        if raw.len() < INIT_BUNDLE_HEADER_LEN {
            return Err(BundleError::TooShort);
        }
        if &raw[..8] != INIT_BUNDLE_MAGIC {
            return Err(BundleError::BadMagic);
        }
        let version = u16::from_le_bytes([raw[8], raw[9]]);
        if version != INIT_BUNDLE_VERSION && version != INIT_BUNDLE_VERSION_V2 {
            return Err(BundleError::BadVersion);
        }
        let header_len = u16::from_le_bytes([raw[10], raw[11]]) as usize;
        if header_len != INIT_BUNDLE_HEADER_LEN {
            return Err(BundleError::BadLayout);
        }
        let total_len = u32::from_le_bytes(raw[12..16].try_into().unwrap()) as usize;
        let source_date_epoch = u64::from_le_bytes(raw[16..24].try_into().unwrap());
        let module_count = u32::from_le_bytes(raw[24..28].try_into().unwrap()) as usize;
        let mut payload_sha256 = [0u8; 32];
        payload_sha256.copy_from_slice(&raw[32..64]);

        let table_len = INIT_BUNDLE_HEADER_LEN + module_count * INIT_BUNDLE_ENTRY_LEN;
        if table_len > raw.len() || total_len > raw.len() || total_len < table_len {
            return Err(BundleError::Truncated);
        }

        let mut modules = Vec::with_capacity(module_count);
        let mut seen_names = alloc::collections::BTreeSet::new();
        for index in 0..module_count {
            let entry = INIT_BUNDLE_HEADER_LEN + index * INIT_BUNDLE_ENTRY_LEN;
            let kind = u32::from_le_bytes(raw[entry..entry + 4].try_into().unwrap());
            // WP-37.1：kind 必须在 {root=1, required=2, optional=3}——未知
            // kind 直接失败（旧实现把 kind=3 当 driver、未知 kind 静默跳过）。
            if kind != MODULE_KIND_ROOT
                && kind != MODULE_KIND_REQUIRED
                && kind != MODULE_KIND_OPTIONAL
            {
                return Err(BundleError::UnknownKind);
            }
            let flags = u32::from_le_bytes(raw[entry + 4..entry + 8].try_into().unwrap());
            // WP-37.1/37.2：flags 低 16 位必须是已知 criticality 编码
            // （31/25/20/15）；高 16 位为能力位图（WP-37.2）——**未知
            // 能力位一律拒绝**（"capability 未声明即拒绝"由构造保证；
            // 未登记位 = runtime 不认识的能力，静默通过即授权漂移）。
            let criticality = flags & !CAPABILITY_BIT_MASK;
            if !KNOWN_CRITICALITY_FLAGS.contains(&criticality) {
                return Err(BundleError::BadFlags);
            }
            let capabilities = flags & CAPABILITY_BIT_MASK;
            let known_caps = CAPABILITY_NAMES
                .iter()
                .fold(0u32, |acc, (bit, _)| acc | bit);
            if capabilities & !known_caps != 0 {
                return Err(BundleError::BadFlags);
            }
            let offset =
                u64::from_le_bytes(raw[entry + 8..entry + 16].try_into().unwrap()) as usize;
            let len = u64::from_le_bytes(raw[entry + 16..entry + 24].try_into().unwrap()) as usize;
            let name_raw = &raw[entry + 24..entry + 24 + INIT_BUNDLE_NAME_MAX];
            let name_end = name_raw
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_raw.len());
            if name_end == 0 {
                return Err(BundleError::BadName);
            }
            let name = core::str::from_utf8(&name_raw[..name_end])
                .map_err(|_| BundleError::BadName)?
                .to_string();
            // WP-37.1：拒绝重复 ID（manifest 侧同样拒绝重复 service id）。
            if !seen_names.insert(name.clone()) {
                return Err(BundleError::DuplicateModule);
            }
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&raw[entry + 56..entry + 88]);

            let blob_start = offset;
            let blob_end = offset.checked_add(len).ok_or(BundleError::Truncated)?;
            if blob_end > raw.len() {
                return Err(BundleError::Truncated);
            }
            modules.push(BundleModule {
                kind,
                flags,
                capabilities,
                name,
                blob: &raw[blob_start..blob_end],
                sha256,
            });
        }

        Ok(Self {
            total_len: total_len as u32,
            source_date_epoch,
            payload_sha256,
            modules,
        })
    }

    /// 验签式解析（30 RT-004）：在 [`Self::parse`] 基础上校验每个模块 blob
    /// 的摘要与模块表项 sha256 一致——init bundle 模块 sha256 字段此前
    /// 从不校验（供应链完整性缺口，测试注释自认占位）。哈希实现经函数
    /// 注入（target 与打包侧同一算法：sha2 Sha256——打包侧 xtask 已写
    /// 真实摘要，存量全零摘要 bundle 会被拒，属应拦项）。
    pub fn parse_verified(
        raw: &'a [u8],
        expect_sha256: fn(&[u8]) -> [u8; 32],
    ) -> Result<Self, BundleError> {
        let bundle = Self::parse(raw)?;
        for module in &bundle.modules {
            if expect_sha256(module.blob) != module.sha256 {
                return Err(BundleError::DigestMismatch);
            }
        }
        Ok(bundle)
    }

    /// 返回指定 kind 的模块名列表。
    pub fn names_of_kind(&self, kind: u32) -> Vec<&str> {
        self.modules
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.name.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use sha2::{Digest, Sha256};

    use super::*;

    /// 构造一个最小 bundle（对齐 compose 布局）。
    fn build_bundle(modules: &[(&str, u32, u32, &[u8])]) -> Vec<u8> {
        let table_len = INIT_BUNDLE_HEADER_LEN + modules.len() * INIT_BUNDLE_ENTRY_LEN;
        let mut payload_len = 0usize;
        for (_, _, _, blob) in modules {
            payload_len += blob.len();
        }
        let total = table_len + payload_len;
        let mut bytes = vec![0u8; total];
        bytes[..8].copy_from_slice(INIT_BUNDLE_MAGIC);
        bytes[8..10].copy_from_slice(&INIT_BUNDLE_VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&(INIT_BUNDLE_HEADER_LEN as u16).to_le_bytes());
        bytes[12..16].copy_from_slice(&(total as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&1700000000u64.to_le_bytes());
        bytes[24..28].copy_from_slice(&(modules.len() as u32).to_le_bytes());
        let mut cursor = table_len;
        for (index, (name, kind, flags, blob)) in modules.iter().enumerate() {
            let entry = INIT_BUNDLE_HEADER_LEN + index * INIT_BUNDLE_ENTRY_LEN;
            bytes[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[entry + 8..entry + 16].copy_from_slice(&(cursor as u64).to_le_bytes());
            bytes[entry + 16..entry + 24].copy_from_slice(&(blob.len() as u64).to_le_bytes());
            let nb = name.as_bytes();
            bytes[entry + 24..entry + 24 + nb.len()].copy_from_slice(nb);
            // 30 RT-004：写真实摘要（对齐打包侧 compose_init_bundle）。
            let digest: [u8; 32] = Sha256::digest(blob).into();
            bytes[entry + 56..entry + 88].copy_from_slice(&digest);
            bytes[cursor..cursor + blob.len()].copy_from_slice(blob);
            cursor += blob.len();
        }
        bytes
    }

    /// 30 RT-004 判据正例：合法摘要 bundle → parse_verified Ok。
    #[test]
    fn test_parse_verified_accepts_matching_digest() {
        let raw = build_bundle(&[("svc-hello", MODULE_KIND_REQUIRED, 15, &[1, 2, 3, 4])]);
        let bundle = InitBundle::parse_verified(&raw, |blob| {
            use sha2::{Digest, Sha256};
            Sha256::digest(blob).into()
        })
        .expect("真实摘要必须通过（30 RT-004）");
        assert_eq!(bundle.modules.len(), 1);
    }

    /// 30 RT-004 判据负例：篡改模块 blob 任一字节 → DigestMismatch
    /// （修复前解析器从不校验摘要，篡改静默通过）。
    #[test]
    fn test_parse_verified_rejects_tampered_blob() {
        let mut raw = build_bundle(&[("svc-hello", MODULE_KIND_REQUIRED, 15, &[1, 2, 3, 4])]);
        // 篡改 payload 区（header 64 + 表 96 = 160 之后）。
        raw[160] ^= 0xFF;
        assert_eq!(
            InitBundle::parse_verified(&raw, |blob| {
                use sha2::{Digest, Sha256};
                Sha256::digest(blob).into()
            }),
            Err(BundleError::DigestMismatch)
        );
    }

    #[test]
    fn test_parse_valid_bundle() {
        let raw = build_bundle(&[
            ("root-task", MODULE_KIND_ROOT, 31, &[0x7F, b'E', b'L', b'F']),
            ("svc-hello", MODULE_KIND_REQUIRED, 15, &[1, 2, 3, 4]),
            ("svc-drv-rng", MODULE_KIND_OPTIONAL, 20, &[9, 9]),
        ]);
        let bundle = InitBundle::parse(&raw).expect("parse");
        assert_eq!(bundle.total_len, raw.len() as u32);
        assert_eq!(bundle.source_date_epoch, 1700000000);
        assert_eq!(bundle.modules.len(), 3);
        assert_eq!(bundle.modules[0].name, "root-task");
        assert_eq!(bundle.modules[1].blob, vec![1, 2, 3, 4]);
        // WP-37.1：kind 语义单一——2=required、3=optional（旧 driver 语义撤销）。
        assert_eq!(
            bundle.names_of_kind(MODULE_KIND_REQUIRED),
            vec!["svc-hello"]
        );
        assert_eq!(
            bundle.names_of_kind(MODULE_KIND_OPTIONAL),
            vec!["svc-drv-rng"]
        );
        assert_eq!(bundle.modules[0].requirement(), Requirement::Required);
        assert_eq!(bundle.modules[1].requirement(), Requirement::Required);
        assert_eq!(bundle.modules[2].requirement(), Requirement::Optional);
    }

    /// WP-37.1：未知 kind 必须拒绝（旧实现把 kind=3 当 driver、未知 kind
    /// 静默跳过——required 语义漂移）。
    #[test]
    fn test_parse_rejects_unknown_kind() {
        let raw = build_bundle(&[("svc-x", 42, 15, &[1])]);
        assert_eq!(InitBundle::parse(&raw), Err(BundleError::UnknownKind));
    }

    /// WP-37.1：未知 flag 位必须拒绝（flags 不再承载未定义“10–31 优先级”）。
    #[test]
    fn test_parse_rejects_unknown_flags() {
        let raw = build_bundle(&[("svc-x", MODULE_KIND_REQUIRED, 12, &[1])]);
        assert_eq!(InitBundle::parse(&raw), Err(BundleError::BadFlags));
    }

    /// WP-37.1：重复模块名必须拒绝（manifest 侧同样拒绝重复 service id）。
    #[test]
    fn test_parse_rejects_duplicate_module_name() {
        let raw = build_bundle(&[
            ("svc-x", MODULE_KIND_REQUIRED, 15, &[1]),
            ("svc-x", MODULE_KIND_OPTIONAL, 20, &[2]),
        ]);
        assert_eq!(InitBundle::parse(&raw), Err(BundleError::DuplicateModule));
    }

    #[test]
    fn test_parse_bad_magic() {
        let mut raw = build_bundle(&[("svc-hello", MODULE_KIND_REQUIRED, 15, &[1])]);
        raw[0] = b'X';
        assert_eq!(InitBundle::parse(&raw), Err(BundleError::BadMagic));
    }

    #[test]
    fn test_parse_truncated() {
        let raw = build_bundle(&[("svc-hello", MODULE_KIND_REQUIRED, 15, &[1, 2, 3])]);
        let truncated = &raw[..raw.len() - 2];
        assert_eq!(InitBundle::parse(truncated), Err(BundleError::Truncated));
    }

    #[test]
    fn test_parse_too_short() {
        assert_eq!(InitBundle::parse(&[0u8; 10]), Err(BundleError::TooShort));
    }
}
