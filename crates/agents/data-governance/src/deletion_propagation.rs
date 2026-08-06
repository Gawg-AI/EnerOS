//! 删除传播与可验证删除报告 (DATA-005).
//!
//! MASTER §21.17 / G8: "deletion/backup propagation — 可验证删除报告".
//!
//! 数据删除必须传播到**所有副本**（主存、备份、副本、缓存、归档），
//! 且每个副本的删除必须有可验证证据（删除时间 + 证明哈希）。
//! "删除请求已发送"不代表删除完成 — 必须有每个副本的 Deleted 证据。
//!
//! # 证据推导
//!
//! 删除完成结论由证据推导：
//! 1. 每个已知副本均有 DeletionRecord
//! 2. 所有 record 状态为 Deleted
//! 3. 每个 record 有非空 proof_hash（删除证据）
//! 4. 删除请求总数 ≥ 1（非空）

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm3;

/// 数据副本存储类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreKind {
    /// 主存储.
    Primary,
    /// 备份存储.
    Backup,
    /// 远程副本.
    Replica,
    /// 缓存层.
    Cache,
    /// 冷归档.
    Archive,
}

impl StoreKind {
    /// 该存储类型的可读名称.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Backup => "backup",
            Self::Replica => "replica",
            Self::Cache => "cache",
            Self::Archive => "archive",
        }
    }
}

/// 单个副本的删除状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionStatus {
    /// 待删除.
    Pending,
    /// 删除进行中.
    InProgress,
    /// 已删除（有证据）.
    Deleted,
    /// 删除失败.
    Failed,
}

/// 单个副本的删除证据记录.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionRecord {
    /// 数据项 ID.
    pub data_id: String,
    /// 副本存储类型.
    pub store: StoreKind,
    /// 删除状态.
    pub status: DeletionStatus,
    /// 删除时间（Unix 秒）.
    pub deleted_at: u64,
    /// 删除证明哈希（SM3 hex，证明删除操作已执行）.
    pub proof_hash: String,
    /// 执行者（服务/操作者 ID）.
    pub actor: String,
}

impl DeletionRecord {
    /// 创建一条删除记录.
    pub fn new(
        data_id: &str,
        store: StoreKind,
        status: DeletionStatus,
        deleted_at: u64,
        proof: &[u8],
        actor: &str,
    ) -> Self {
        let proof_hash = sm3_hex(proof);
        Self {
            data_id: String::from(data_id),
            store,
            status,
            deleted_at,
            proof_hash,
            actor: String::from(actor),
        }
    }

    /// 是否为有效删除证据（Deleted 状态 + 非空证明哈希）.
    pub fn is_verified_deleted(&self) -> bool {
        self.status == DeletionStatus::Deleted && !self.proof_hash.is_empty()
    }

    /// 校验证明哈希是否与给定数据一致.
    pub fn verify_proof(&self, proof: &[u8]) -> bool {
        !self.proof_hash.is_empty() && self.proof_hash == sm3_hex(proof)
    }
}

/// 单个数据项的删除报告.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionReport {
    /// 数据项 ID.
    pub data_id: String,
    /// 已知的所有副本存储类型.
    pub known_stores: Vec<StoreKind>,
    /// 每个副本的删除记录.
    pub records: Vec<DeletionRecord>,
}

impl DeletionReport {
    /// 创建删除报告.
    pub fn new(data_id: &str, known_stores: Vec<StoreKind>, records: Vec<DeletionRecord>) -> Self {
        Self {
            data_id: String::from(data_id),
            known_stores,
            records,
        }
    }

    /// 缺失的副本（已知但无删除记录）.
    pub fn missing_stores(&self) -> Vec<StoreKind> {
        self.known_stores
            .iter()
            .filter(|s| !self.records.iter().any(|r| r.store == **s))
            .copied()
            .collect()
    }

    /// 未完成删除的副本（状态非 Deleted 或无证明）.
    pub fn incomplete_stores(&self) -> Vec<StoreKind> {
        self.known_stores
            .iter()
            .filter_map(|s| {
                let rec = self.records.iter().find(|r| r.store == *s);
                match rec {
                    Some(r) if r.is_verified_deleted() => None,
                    _ => Some(*s),
                }
            })
            .collect()
    }

    /// 删除是否完整传播（由证据推导）.
    ///
    /// 条件：
    /// 1. 已知副本非空
    /// 2. 无缺失副本
    /// 3. 无未完成副本
    /// 4. 所有记录属于该数据项
    pub fn is_complete(&self) -> bool {
        if self.known_stores.is_empty() {
            return false;
        }
        if !self.missing_stores().is_empty() {
            return false;
        }
        if !self.incomplete_stores().is_empty() {
            return false;
        }
        self.records
            .iter()
            .all(|r| r.data_id == self.data_id && r.is_verified_deleted())
    }
}

/// 删除传播管理器 — 汇总多个数据项的删除报告.
pub struct DeletionPropagation {
    reports: Vec<DeletionReport>,
}

impl DeletionPropagation {
    pub fn new(reports: Vec<DeletionReport>) -> Self {
        Self { reports }
    }

    /// 添加报告.
    pub fn add(&mut self, report: DeletionReport) {
        self.reports.push(report);
    }

    /// 未完成删除的数据项.
    pub fn incomplete_items(&self) -> Vec<&String> {
        self.reports
            .iter()
            .filter(|r| !r.is_complete())
            .map(|r| &r.data_id)
            .collect()
    }

    /// 全部删除是否完整传播（由证据推导）.
    ///
    /// 条件：
    /// 1. 非空（至少有报告）
    /// 2. 所有报告 is_complete
    pub fn all_propagated(&self) -> bool {
        if self.reports.is_empty() {
            return false;
        }
        self.reports.iter().all(|r| r.is_complete())
    }
}

/// SM3 hex 编码.
fn sm3_hex(data: &[u8]) -> String {
    let hash = sm3::hash(data);
    let mut out = String::with_capacity(64);
    for byte in &hash {
        use core::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        data_id: &str,
        store: StoreKind,
        status: DeletionStatus,
        proof: &[u8],
    ) -> DeletionRecord {
        DeletionRecord::new(data_id, store, status, 1_700_000_000, proof, "svc-data")
    }

    #[test]
    fn t1_complete_propagation_all_stores_deleted() {
        let stores = vec![
            StoreKind::Primary,
            StoreKind::Backup,
            StoreKind::Replica,
            StoreKind::Cache,
        ];
        let records = vec![
            rec(
                "d1",
                StoreKind::Primary,
                DeletionStatus::Deleted,
                b"proof-primary",
            ),
            rec(
                "d1",
                StoreKind::Backup,
                DeletionStatus::Deleted,
                b"proof-backup",
            ),
            rec(
                "d1",
                StoreKind::Replica,
                DeletionStatus::Deleted,
                b"proof-replica",
            ),
            rec(
                "d1",
                StoreKind::Cache,
                DeletionStatus::Deleted,
                b"proof-cache",
            ),
        ];
        let report = DeletionReport::new("d1", stores, records);
        assert!(report.is_complete());
    }

    #[test]
    fn t2_incomplete_when_store_missing() {
        let stores = vec![StoreKind::Primary, StoreKind::Backup, StoreKind::Replica];
        let records = vec![
            rec("d2", StoreKind::Primary, DeletionStatus::Deleted, b"p1"),
            rec("d2", StoreKind::Backup, DeletionStatus::Deleted, b"p2"),
            // Replica 缺失
        ];
        let report = DeletionReport::new("d2", stores, records);
        assert!(!report.is_complete());
        assert_eq!(report.missing_stores(), vec![StoreKind::Replica]);
    }

    #[test]
    fn t3_incomplete_when_status_not_deleted() {
        let stores = vec![StoreKind::Primary, StoreKind::Backup];
        let records = vec![
            rec("d3", StoreKind::Primary, DeletionStatus::Deleted, b"p1"),
            rec("d3", StoreKind::Backup, DeletionStatus::InProgress, b"p2"),
        ];
        let report = DeletionReport::new("d3", stores, records);
        assert!(!report.is_complete());
        assert_eq!(report.incomplete_stores(), vec![StoreKind::Backup]);
    }

    #[test]
    fn t4_incomplete_when_proof_empty() {
        let stores = vec![StoreKind::Primary];
        let records = vec![DeletionRecord {
            data_id: String::from("d4"),
            store: StoreKind::Primary,
            status: DeletionStatus::Deleted,
            deleted_at: 1_700_000_000,
            proof_hash: String::new(),
            actor: String::from("svc"),
        }];
        let report = DeletionReport::new("d4", stores, records);
        assert!(!report.is_complete());
    }

    #[test]
    fn t5_failed_status_blocks_completion() {
        let stores = vec![StoreKind::Primary, StoreKind::Archive];
        let records = vec![
            rec("d5", StoreKind::Primary, DeletionStatus::Deleted, b"p1"),
            rec("d5", StoreKind::Archive, DeletionStatus::Failed, b"p2"),
        ];
        let report = DeletionReport::new("d5", stores, records);
        assert!(!report.is_complete());
    }

    #[test]
    fn t6_propagation_all_complete() {
        let r1 = DeletionReport::new(
            "d1",
            vec![StoreKind::Primary, StoreKind::Backup],
            vec![
                rec("d1", StoreKind::Primary, DeletionStatus::Deleted, b"p1"),
                rec("d1", StoreKind::Backup, DeletionStatus::Deleted, b"p2"),
            ],
        );
        let r2 = DeletionReport::new(
            "d2",
            vec![StoreKind::Cache],
            vec![rec("d2", StoreKind::Cache, DeletionStatus::Deleted, b"p3")],
        );
        let mgr = DeletionPropagation::new(vec![r1, r2]);
        assert!(mgr.all_propagated());
    }

    #[test]
    fn t7_empty_propagation_fails() {
        let mgr = DeletionPropagation::new(Vec::new());
        assert!(!mgr.all_propagated());
    }

    #[test]
    fn t8_one_incomplete_blocks_all() {
        let r1 = DeletionReport::new(
            "d1",
            vec![StoreKind::Primary],
            vec![rec(
                "d1",
                StoreKind::Primary,
                DeletionStatus::Deleted,
                b"p1",
            )],
        );
        let r2 = DeletionReport::new(
            "d2",
            vec![StoreKind::Primary, StoreKind::Backup],
            vec![rec(
                "d2",
                StoreKind::Primary,
                DeletionStatus::Deleted,
                b"p2",
            )],
        );
        let mgr = DeletionPropagation::new(vec![r1, r2]);
        assert!(!mgr.all_propagated());
        assert_eq!(mgr.incomplete_items().len(), 1);
    }

    #[test]
    fn t9_verify_proof_roundtrip() {
        let r = rec(
            "d9",
            StoreKind::Primary,
            DeletionStatus::Deleted,
            b"my-proof",
        );
        assert!(r.verify_proof(b"my-proof"));
        assert!(!r.verify_proof(b"wrong-proof"));
    }

    #[test]
    fn t10_data_id_mismatch_blocks_completion() {
        let stores = vec![StoreKind::Primary];
        let records = vec![rec(
            "other-id",
            StoreKind::Primary,
            DeletionStatus::Deleted,
            b"p1",
        )];
        let report = DeletionReport::new("d10", stores, records);
        assert!(!report.is_complete());
    }
}
