//! Diagnostician 系统诊断（Task 5，蓝图 §4.3 diag 命令，D5/D7）。
//!
//! - **D7**：`storage_ok` / `network_ok` / `temperature` 硬件字段经 [`HealthProbe`]
//!   探针注入而非直读硬件（可测试；[`DefaultProbe`] 恒 ok 占位）；
//! - **D5**：`partition_a_ok` / `partition_b_ok` / `boot_fail_count` / `last_error`
//!   真实查询自 eneros-ota-ab [`PartitionTable`]（v0.123.0 兼容回归），替代蓝图
//!   硬编码字符串。
//!
//! 分区健康语义：`state ∈ {Active, Verified, Written}` 判定为 ok；
//! `{Empty, Writing, Failed, Rolling}` 判定为非 ok。`last_error` 记录首个非 ok
//! 分区的描述，超过 128 字节按字符边界截断（heapless 容量上限，不 panic）。

use alloc::format;
use alloc::string::String;

use eneros_ota_ab::{PartitionSlot, PartitionState, PartitionTable};
use heapless::String as HString;

use crate::DiagResult;

/// 健康探针（D7）：硬件相关字段经此 trait 注入，便于测试与平台替换。
pub trait HealthProbe {
    /// 存储健康状态
    fn storage_ok(&self) -> bool;
    /// 网络健康状态
    fn network_ok(&self) -> bool;
    /// 芯片温度（摄氏度）
    fn temperature(&self) -> f32;
}

/// 默认探针：三方法恒 ok（temperature 返回 25.0 占位）。
///
/// 恢复模式默认场景下无平台特定驱动接入时使用；真实硬件探针由集成方实现
/// [`HealthProbe`] 注入。
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultProbe;

impl HealthProbe for DefaultProbe {
    fn storage_ok(&self) -> bool {
        true
    }

    fn network_ok(&self) -> bool {
        true
    }

    fn temperature(&self) -> f32 {
        25.0
    }
}

/// 分区健康语义：可启动/可回滚备用态判定为 ok，其余判定为非 ok。
fn slot_ok(state: PartitionState) -> bool {
    matches!(
        state,
        PartitionState::Active | PartitionState::Verified | PartitionState::Written
    )
}

/// 构造 ≤128 字节的 heapless 错误串：超长时按字符边界截断（不 panic）。
fn hstr_truncate<const N: usize>(s: &str) -> HString<N> {
    let mut out = HString::<N>::new();
    for ch in s.chars() {
        if out.push(ch).is_err() {
            break;
        }
    }
    out
}

/// 系统诊断器：探针字段 + ota-ab 分区表字段汇总为 [`DiagResult`]。
pub struct Diagnostician<P: HealthProbe> {
    /// 健康探针（D7 注入）
    probe: P,
    /// ota-ab 分区表快照（D5：经 [`Self::set_partition_table`] 注入）
    partition_table: Option<PartitionTable>,
}

impl<P: HealthProbe> Diagnostician<P> {
    /// 创建诊断器：注入探针，分区表初始为空（无分区表时按谨慎默认处理）。
    pub fn new(probe: P) -> Self {
        Self {
            probe,
            partition_table: None,
        }
    }

    /// 注入真实分区表（D5：来自 eneros-ota-ab `PartitionTable::snapshot`）。
    pub fn set_partition_table(&mut self, table: PartitionTable) {
        self.partition_table = Some(table);
    }

    /// 查询当前注入的分区表。
    pub fn partition_table(&self) -> Option<&PartitionTable> {
        self.partition_table.as_ref()
    }

    /// 执行一次诊断，汇总分区字段与探针字段。
    ///
    /// - 无分区表：`partition_a_ok` / `partition_b_ok` 均为 `false`（恢复模式
    ///   谨慎默认），`boot_fail_count` 为 0，`last_error` 记分区表不可用；
    /// - 有分区表：按槽位查找分区并依 [`slot_ok`] 判定；`boot_fail_count` 取
    ///   双分区 `boot_count` 之和；`last_error` 记录首个非 ok 分区描述
    ///   （A 优先于 B，≤128 字节截断），全 ok 时为空串。
    pub fn run(&self) -> DiagResult {
        let mut result = DiagResult {
            partition_a_ok: false,
            partition_b_ok: false,
            boot_fail_count: 0,
            last_error: HString::<128>::new(),
            storage_ok: self.probe.storage_ok(),
            network_ok: self.probe.network_ok(),
            temperature: self.probe.temperature(),
        };

        let Some(table) = self.partition_table.as_ref() else {
            // 恢复模式谨慎默认：无分区表视为双分区均不可用
            result.last_error = hstr_truncate("partition table unavailable");
            return result;
        };

        let mut first_error: Option<String> = None;
        for slot in [PartitionSlot::A, PartitionSlot::B] {
            let Some(part) = table.partitions.iter().find(|p| p.slot == slot) else {
                // 槽位缺失视为非 ok
                if first_error.is_none() {
                    first_error = Some(format!("partition {slot:?} missing"));
                }
                continue;
            };
            let ok = slot_ok(part.state);
            match slot {
                PartitionSlot::A => result.partition_a_ok = ok,
                PartitionSlot::B => result.partition_b_ok = ok,
            }
            result.boot_fail_count = result.boot_fail_count.saturating_add(part.boot_count);
            if !ok && first_error.is_none() {
                first_error = Some(format!(
                    "partition {:?} state={:?} version={} size={} boot_count={} last_boot_ms={}",
                    part.slot,
                    part.state,
                    part.version.as_str(),
                    part.size,
                    part.boot_count,
                    part.last_boot_ms
                ));
            }
        }

        if let Some(desc) = first_error {
            result.last_error = hstr_truncate(&desc);
        }
        result
    }
}

/// 47（WP-47.6 真实探针集）：系统完整性探针——分区表 CRC、镜像 digest、
/// bootctrl 副本一致性、关键服务 readiness。**无证据不得报 ok**（每探针
/// 输出依据 detail；源缺失/读取失败 → 非 ok，fail-closed）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeVerdict {
    /// 是否通过（仅当存在可验证证据且一致）。
    pub ok: bool,
    /// 输出依据（证据/失败原因；≤160 字节，截断不 panic）。
    pub detail: HString<160>,
}

impl ProbeVerdict {
    pub fn ok(detail: impl core::fmt::Display) -> Self {
        Self {
            ok: true,
            detail: hstr_truncate(&alloc::format!("ok: {detail}")),
        }
    }
    pub fn fail(detail: impl core::fmt::Display) -> Self {
        Self {
            ok: false,
            detail: hstr_truncate(&alloc::format!("no-evidence: {detail}")),
        }
    }
}

/// 平台完整性事实源（注入面；None = 证据不可得 → 对应探针非 ok）。
pub trait PlatformIntegrity {
    /// 分区表区域实测摘要（SHA-256 / CRC 由平台计算并注入）。
    fn partition_table_digest(&self) -> Option<[u8; 32]>;
    /// 期望分区表摘要（出厂/最近一次部署校验值）。
    fn expected_partition_table_digest(&self) -> Option<[u8; 32]>;
    /// bootctrl 副本 A（字节）。
    fn bootctrl_copy_a(&self) -> Option<&[u8]>;
    /// bootctrl 副本 B（字节）。
    fn bootctrl_copy_b(&self) -> Option<&[u8]>;
    /// 实测镜像 digest。
    fn image_digest(&self) -> Option<[u8; 32]>;
    /// 期望镜像 digest。
    fn expected_image_digest(&self) -> Option<[u8; 32]>;
    /// 关键服务 readiness 标志（true = 该服务就绪；全 true 才 ok）。
    fn service_readiness(&self) -> Option<&[bool]>;
}

/// 四探针契约（集成方以 [`RealIntegrityProbe`] 注入真实源）。
pub trait SystemIntegrityProbe {
    fn partition_table_crc(&self) -> ProbeVerdict;
    fn image_digest(&self) -> ProbeVerdict;
    fn bootctrl_copies(&self) -> ProbeVerdict;
    fn service_readiness(&self) -> ProbeVerdict;
}

/// 真实探针：基于注入事实源做实际比对；源缺失 → fail（无证据不 ok）。
pub struct RealIntegrityProbe<S: PlatformIntegrity>(pub S);

impl<S: PlatformIntegrity> SystemIntegrityProbe for RealIntegrityProbe<S> {
    fn partition_table_crc(&self) -> ProbeVerdict {
        match (
            self.0.partition_table_digest(),
            self.0.expected_partition_table_digest(),
        ) {
            (Some(measured), Some(expected)) if measured == expected => {
                ProbeVerdict::ok(format!("partition-table digest match {:02x?}", measured))
            }
            (Some(measured), Some(expected)) => ProbeVerdict::fail(format!(
                "partition-table digest mismatch measured={:02x?} expected={:02x?}",
                measured, expected
            )),
            (None, _) => ProbeVerdict::fail("partition-table digest source unavailable"),
            (_, None) => ProbeVerdict::fail("expected partition-table digest unavailable"),
        }
    }

    fn image_digest(&self) -> ProbeVerdict {
        match (self.0.image_digest(), self.0.expected_image_digest()) {
            (Some(m), Some(e)) if m == e => {
                ProbeVerdict::ok(format!("image digest match {m:02x?}"))
            }
            (Some(m), Some(e)) => ProbeVerdict::fail(format!(
                "image digest mismatch measured={m:02x?} expected={e:02x?}"
            )),
            (None, _) => ProbeVerdict::fail("image digest source unavailable"),
            (_, None) => ProbeVerdict::fail("expected image digest unavailable"),
        }
    }

    fn bootctrl_copies(&self) -> ProbeVerdict {
        match (self.0.bootctrl_copy_a(), self.0.bootctrl_copy_b()) {
            (Some(a), Some(b)) if a == b => {
                ProbeVerdict::ok(format!("bootctrl copies consistent ({}B)", a.len()))
            }
            (Some(a), Some(b)) => ProbeVerdict::fail(format!(
                "bootctrl copies MISMATCH (a={}B b={}B)",
                a.len(),
                b.len()
            )),
            (None, _) | (_, None) => ProbeVerdict::fail("bootctrl copies source unavailable"),
        }
    }

    fn service_readiness(&self) -> ProbeVerdict {
        match self.0.service_readiness() {
            Some(flags) if !flags.is_empty() && flags.iter().all(|&f| f) => ProbeVerdict::ok(
                format!("services ready ({} of {})", flags.len(), flags.len()),
            ),
            Some(flags) => {
                let ready = flags.iter().filter(|&&f| f).count();
                ProbeVerdict::fail(format!("services NOT ready ({} of {})", ready, flags.len()))
            }
            None => ProbeVerdict::fail("service readiness source unavailable"),
        }
    }
}

/// 默认探针：全部源缺失 —— 每个探针均无证据 → 非 ok（fail-closed）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEvidenceIntegrity;

impl PlatformIntegrity for NoEvidenceIntegrity {
    fn partition_table_digest(&self) -> Option<[u8; 32]> {
        None
    }
    fn expected_partition_table_digest(&self) -> Option<[u8; 32]> {
        None
    }
    fn bootctrl_copy_a(&self) -> Option<&[u8]> {
        None
    }
    fn bootctrl_copy_b(&self) -> Option<&[u8]> {
        None
    }
    fn image_digest(&self) -> Option<[u8; 32]> {
        None
    }
    fn expected_image_digest(&self) -> Option<[u8; 32]> {
        None
    }
    fn service_readiness(&self) -> Option<&[bool]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use eneros_ota_ab::{Partition, PartitionSlot, PartitionState, PartitionTable};
    use heapless::String as HString;

    use super::*;

    /// 构造指定槽位/状态的分区，其余字段取给定参数或默认零值。
    fn make_partition(
        slot: PartitionSlot,
        state: PartitionState,
        version: &str,
        boot_count: u32,
    ) -> Partition {
        Partition {
            slot,
            state,
            version: HString::<32>::try_from(version).unwrap_or_default(),
            size: 0,
            hash: [0; 32],
            signature: [0; 64],
            boot_count,
            last_boot_ms: 0,
        }
    }

    /// 故障探针：三方法全部反映异常（DIA2 注入用）。
    struct FailingProbe;

    impl HealthProbe for FailingProbe {
        fn storage_ok(&self) -> bool {
            false
        }

        fn network_ok(&self) -> bool {
            false
        }

        fn temperature(&self) -> f32 {
            86.5
        }
    }

    /// DIA1：默认探针 + 无分区表 → 探针字段恒 ok，双分区谨慎判定为非 ok，
    /// last_error 含 "unavailable"。
    #[test]
    fn dia1_default_probe_without_partition_table() {
        let diag = Diagnostician::new(DefaultProbe);
        assert!(diag.partition_table().is_none());

        let r = diag.run();
        assert!(r.storage_ok);
        assert!(r.network_ok);
        assert!((r.temperature - 25.0).abs() < f32::EPSILON);
        assert!(!r.partition_a_ok, "无分区表时 A 应谨慎判定非 ok");
        assert!(!r.partition_b_ok, "无分区表时 B 应谨慎判定非 ok");
        assert_eq!(r.boot_fail_count, 0);
        assert!(r.last_error.as_str().contains("unavailable"));
    }

    /// DIA2：故障探针注入 → run() 三字段正确反映探针值。
    #[test]
    fn dia2_failing_probe_reflected() {
        let diag = Diagnostician::new(FailingProbe);
        let r = diag.run();
        assert!(!r.storage_ok);
        assert!(!r.network_ok);
        assert!((r.temperature - 86.5).abs() < f32::EPSILON);
    }

    /// DIA3：分区表真实查询（B Failed 场景）——直接字面量构造 PartitionTable
    /// （partitions / active_slot 均为 pub 字段），A Active + B Failed。
    #[test]
    fn dia3_partition_table_b_failed() {
        let table = PartitionTable {
            partitions: [
                make_partition(PartitionSlot::A, PartitionState::Active, "0.124.0", 3),
                make_partition(PartitionSlot::B, PartitionState::Failed, "0.125.0", 4),
            ],
            active_slot: PartitionSlot::A,
        };
        let mut diag = Diagnostician::new(DefaultProbe);
        diag.set_partition_table(table);
        assert!(diag.partition_table().is_some());

        let r = diag.run();
        assert!(r.partition_a_ok, "A Active 应为 ok");
        assert!(!r.partition_b_ok, "B Failed 应为非 ok");
        assert_eq!(r.boot_fail_count, 3 + 4, "双分区 boot_count 之和");
        assert!(r.last_error.as_str().contains("B"), "错误描述应含槽位 B");
        assert!(
            r.last_error.as_str().contains("Failed"),
            "错误描述应含状态 Failed"
        );
    }

    /// DIA4：last_error 截断——version 填满 32 字节 + 各数值字段取最大值，
    /// 拼装的描述 >128 字节，断言截断后不超容量且不 panic。
    #[test]
    fn dia4_last_error_truncated_to_128() {
        let long_version = "0123456789abcdef0123456789abcdef"; // 32 字节填满
        let mut part_b = make_partition(
            PartitionSlot::B,
            PartitionState::Failed,
            long_version,
            u32::MAX,
        );
        part_b.size = u64::MAX;
        part_b.last_boot_ms = u64::MAX;
        let table = PartitionTable {
            partitions: [
                make_partition(PartitionSlot::A, PartitionState::Active, "0.124.0", 0),
                part_b,
            ],
            active_slot: PartitionSlot::A,
        };
        let mut diag = Diagnostician::new(DefaultProbe);
        diag.set_partition_table(table);

        let r = diag.run();
        assert!(!r.partition_b_ok);
        assert!(r.last_error.len() <= 128, "截断后不得超过 128 字节");
        assert!(!r.last_error.is_empty(), "截断后仍应保留前缀描述");
        assert!(r
            .last_error
            .as_str()
            .starts_with("partition B state=Failed"));
    }

    // ============ 47（WP-47.6）真实完整性探针 ============

    /// 注入源（可返回真实/缺失/不一致证据）。
    struct MockIntegrity {
        table: Option<([u8; 32], [u8; 32])>,
        image: Option<([u8; 32], [u8; 32])>,
        boot: Option<(Vec<u8>, Vec<u8>)>,
        services: Option<Vec<bool>>,
    }

    impl PlatformIntegrity for MockIntegrity {
        fn partition_table_digest(&self) -> Option<[u8; 32]> {
            self.table.map(|t| t.0)
        }
        fn expected_partition_table_digest(&self) -> Option<[u8; 32]> {
            self.table.map(|t| t.1)
        }
        fn bootctrl_copy_a(&self) -> Option<&[u8]> {
            self.boot.as_ref().map(|b| b.0.as_slice())
        }
        fn bootctrl_copy_b(&self) -> Option<&[u8]> {
            self.boot.as_ref().map(|b| b.1.as_slice())
        }
        fn image_digest(&self) -> Option<[u8; 32]> {
            self.image.map(|t| t.0)
        }
        fn expected_image_digest(&self) -> Option<[u8; 32]> {
            self.image.map(|t| t.1)
        }
        fn service_readiness(&self) -> Option<&[bool]> {
            self.services.as_deref()
        }
    }

    /// 无证据默认 → 每探针非 ok（fail-closed，无证据不得报 ok）。
    #[test]
    fn dia6_no_evidence_all_probes_fail() {
        let p = RealIntegrityProbe(NoEvidenceIntegrity);
        let v = p.partition_table_crc();
        assert!(!v.ok, "无证据不得报 ok");
        assert!(v.detail.as_str().starts_with("no-evidence"));
        assert!(!p.image_digest().ok);
        assert!(!p.bootctrl_copies().ok);
        assert!(!p.service_readiness().ok);
    }

    /// 正例：全证据一致 → 全探针 ok + 依据。
    #[test]
    fn dia7_matching_evidence_all_ok() {
        let src = MockIntegrity {
            table: Some(([0xAA; 32], [0xAA; 32])),
            image: Some(([0xBB; 32], [0xBB; 32])),
            boot: Some((vec![0u8; 64], vec![0u8; 64])),
            services: Some(vec![true, true, true]),
        };
        let p = RealIntegrityProbe(src);
        assert!(p.partition_table_crc().ok);
        assert!(p.image_digest().ok);
        assert!(p.bootctrl_copies().ok);
        assert!(p.service_readiness().ok);
        assert!(p.partition_table_crc().detail.as_str().starts_with("ok:"));
    }

    /// 负例：分区表摘要不符 / 镜像不符 / bootctrl 双副本不一致 / 服务未就绪
    /// → 各自非 ok 且 detail 含依据。
    #[test]
    fn dia8_mismatch_sources_all_fail_with_evidence() {
        let src = MockIntegrity {
            table: Some(([0xAA; 32], [0xAB; 32])),
            image: Some(([0xBB; 32], [0xBC; 32])),
            boot: Some((vec![0u8; 64], vec![1u8; 64])),
            services: Some(vec![true, false, true]),
        };
        let p = RealIntegrityProbe(src);
        let t = p.partition_table_crc();
        assert!(!t.ok);
        assert!(t.detail.as_str().contains("mismatch"), "{}", t.detail);
        assert!(p.image_digest().detail.as_str().contains("mismatch"));
        assert!(p.bootctrl_copies().detail.as_str().contains("MISMATCH"));
        assert!(p.service_readiness().detail.as_str().contains("NOT ready"));
    }

    /// 局部源缺失 → 该探针 fail（其余独立判定）。
    #[test]
    fn dia9_partial_sources_isolated() {
        // 只有分区表证据，其余缺失。
        let src = MockIntegrity {
            table: Some(([0xAA; 32], [0xAA; 32])),
            image: None,
            boot: None,
            services: None,
        };
        let p = RealIntegrityProbe(src);
        assert!(p.partition_table_crc().ok);
        assert!(!p.image_digest().ok, "镜像源缺失 → 非 ok");
        assert!(!p.bootctrl_copies().ok);
        assert!(!p.service_readiness().ok);
    }

    /// DIA5：全 ok 场景——A Active + B Verified，双分区均 ok 且 last_error 为空。
    #[test]
    fn dia5_all_partitions_ok() {
        let table = PartitionTable {
            partitions: [
                make_partition(PartitionSlot::A, PartitionState::Active, "0.124.0", 1),
                make_partition(PartitionSlot::B, PartitionState::Verified, "0.125.0", 2),
            ],
            active_slot: PartitionSlot::A,
        };
        let mut diag = Diagnostician::new(DefaultProbe);
        diag.set_partition_table(table);

        let r = diag.run();
        assert!(r.partition_a_ok);
        assert!(r.partition_b_ok);
        assert_eq!(r.boot_fail_count, 3);
        assert!(r.last_error.is_empty(), "全 ok 时 last_error 应为空串");
    }
}
