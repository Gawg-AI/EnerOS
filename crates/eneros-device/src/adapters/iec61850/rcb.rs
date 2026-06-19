//! IEC 61850 Report Control Block (RCB) management.
//!
//! Implements URCB (Unbuffered) and BRCB (Buffered) report control blocks
//! per IEC 61850-7-2 §17.5. Reports are the primary mechanism for IEDs to
//! push data changes to clients asynchronously.
//!
//! # Report Flow
//!
//! ```text
//! Client                          IED
//!   │                              │
//!   ├── Enable RCB ──────────────►│
//!   │   (SetRCBValues + RptEna=1) │
//!   │                              │
//!   │   ◄──── Report (trgop) ─────┤  data-change / quality-change /
//!   │                              │  trigger / periodic / GI
//!   ├── Disable RCB ─────────────►│
//!   │   (RptEna=0)                 │
//! ```
//!
//! # Trigger Options (TrgOp)
//!
//! - `dchg`: data change
//! - `qchg`: quality change
//! - `dupd`: data update
//! - `period`: periodic integrity
//! - `gi`: general interrogation

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::adapter::{DataValue, DataQuality};

/// Report trigger options (bitmask)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrgOp(pub u8);

impl TrgOp {
    /// Data change trigger
    pub const DCHG: Self = Self(0x01);
    /// Quality change trigger
    pub const QCHG: Self = Self(0x02);
    /// Data update trigger
    pub const DUPD: Self = Self(0x04);
    /// Periodic integrity scan
    pub const PERIOD: Self = Self(0x08);
    /// General interrogation trigger
    pub const GI: Self = Self(0x10);

    pub fn has(self, flag: Self) -> bool {
        self.0 & flag.0 != 0
    }

    pub fn add(self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }

    pub fn remove(self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }

    pub fn as_string(self) -> String {
        let mut parts = Vec::new();
        if self.has(Self::DCHG) { parts.push("dchg"); }
        if self.has(Self::QCHG) { parts.push("qchg"); }
        if self.has(Self::DUPD) { parts.push("dupd"); }
        if self.has(Self::PERIOD) { parts.push("period"); }
        if self.has(Self::GI) { parts.push("gi"); }
        parts.join(",")
    }
}

/// Report control block type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcbType {
    /// Unbuffered RCB — reports lost on IED restart
    Unbuffered,
    /// Buffered RCB — reports buffered across IED restart
    Buffered,
}

/// Report control block configuration
#[derive(Debug, Clone)]
pub struct ReportControlBlock {
    /// RCB reference: `LD/LLN.RCB_name`
    pub rcb_ref: String,
    /// RCB type (URCB or BRCB)
    pub rcb_type: RcbType,
    /// Dataset reference: `LD/LLN.dataset_name`
    pub dataset_ref: String,
    /// Report ID (unique per RCB)
    pub report_id: String,
    /// Trigger options
    pub trg_op: TrgOp,
    /// Integrity period in milliseconds (0 = disabled)
    pub integrity_period_ms: u32,
    /// Options bitmask (sequence-number, reason-for-inclusion, dataset-ref,
    /// buffer-overflow, entry-id, conf-rev)
    pub opt_fields: u8,
    /// Enabled flag
    pub enabled: bool,
    /// Reservation flag (for URCB only)
    pub reserved: bool,
    /// Owner (client ID that reserved the RCB, for URCB)
    pub owner: Option<String>,
    /// Configuration revision
    pub conf_rev: u32,
}

impl Default for ReportControlBlock {
    fn default() -> Self {
        Self {
            rcb_ref: "LD0/LLN0.brcbGeneric01".to_string(),
            rcb_type: RcbType::Buffered,
            dataset_ref: "LD0/LLN0.dsGeneric".to_string(),
            report_id: "brcbGeneric01".to_string(),
            trg_op: TrgOp::DCHG.add(TrgOp::QCHG).add(TrgOp::GI),
            integrity_period_ms: 0,
            opt_fields: 0x06, // sequence-number + reason-for-inclusion
            enabled: false,
            reserved: false,
            owner: None,
            conf_rev: 1,
        }
    }
}

impl ReportControlBlock {
    /// Option field bits
    pub const OPT_SEQ_NUM: u8 = 0x01;
    pub const OPT_REASON: u8 = 0x02;
    pub const OPT_DATASET_REF: u8 = 0x04;
    pub const OPT_BUFFER_OVERFLOW: u8 = 0x08;
    pub const OPT_ENTRY_ID: u8 = 0x10;
    pub const OPT_CONF_REV: u8 = 0x20;

    pub fn has_opt_field(&self, bit: u8) -> bool {
        self.opt_fields & bit != 0
    }
}

/// A received report from an IED
#[derive(Debug, Clone)]
pub struct Iec61850ReportData {
    /// Report ID
    pub report_id: String,
    /// Sequence number (if OPT_SEQ_NUM)
    pub seq_num: Option<u32>,
    /// Dataset reference
    pub dataset_ref: String,
    /// Reason for inclusion (per entry)
    pub reason_for_inclusion: Vec<String>,
    /// Values: dataset member index → (value, quality)
    pub values: Vec<(DataValue, DataQuality)>,
    /// Timestamp of report
    pub timestamp_ms: i64,
    /// Entry ID (for BRCB, if OPT_ENTRY_ID)
    pub entry_id: Option<u64>,
    /// Buffer overflow flag (for BRCB, if OPT_BUFFER_OVERFLOW)
    pub buffer_overflow: Option<bool>,
    /// Configuration revision (if OPT_CONF_REV)
    pub conf_rev: Option<u32>,
}

/// RCB manager — tracks enabled RCBs and dispatches reports
pub struct RcbManager {
    rcbs: Arc<Mutex<HashMap<String, ReportControlBlock>>>,
    /// Last received report per RCB
    last_reports: Arc<Mutex<HashMap<String, Iec61850ReportData>>>,
}

impl Default for RcbManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RcbManager {
    pub fn new() -> Self {
        Self {
            rcbs: Arc::new(Mutex::new(HashMap::new())),
            last_reports: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register an RCB configuration
    pub async fn register(&self, rcb: ReportControlBlock) {
        self.rcbs.lock().await.insert(rcb.rcb_ref.clone(), rcb);
    }

    /// Enable an RCB (sends SetRCBValues + RptEna=true)
    pub async fn enable(&self, rcb_ref: &str) -> Result<(), String> {
        let mut rcbs = self.rcbs.lock().await;
        let rcb = rcbs.get_mut(rcb_ref).ok_or_else(|| format!("RCB not found: {}", rcb_ref))?;
        if rcb.enabled {
            return Ok(());
        }
        rcb.enabled = true;
        tracing::info!("IEC 61850: enabled RCB {} (dataset={})", rcb_ref, rcb.dataset_ref);
        Ok(())
    }

    /// Disable an RCB (RptEna=false)
    pub async fn disable(&self, rcb_ref: &str) -> Result<(), String> {
        let mut rcbs = self.rcbs.lock().await;
        let rcb = rcbs.get_mut(rcb_ref).ok_or_else(|| format!("RCB not found: {}", rcb_ref))?;
        if !rcb.enabled {
            return Ok(());
        }
        rcb.enabled = false;
        tracing::info!("IEC 61850: disabled RCB {}", rcb_ref);
        Ok(())
    }

    /// Reserve a URCB (required before enabling)
    pub async fn reserve(&self, rcb_ref: &str, owner: &str) -> Result<(), String> {
        let mut rcbs = self.rcbs.lock().await;
        let rcb = rcbs.get_mut(rcb_ref).ok_or_else(|| format!("RCB not found: {}", rcb_ref))?;
        if rcb.rcb_type != RcbType::Unbuffered {
            return Err(format!("{} is not a URCB", rcb_ref));
        }
        if rcb.reserved && rcb.owner.as_deref() != Some(owner) {
            return Err(format!("{} reserved by another client", rcb_ref));
        }
        rcb.reserved = true;
        rcb.owner = Some(owner.to_string());
        Ok(())
    }

    /// Update trigger options
    pub async fn set_trg_op(&self, rcb_ref: &str, trg_op: TrgOp) -> Result<(), String> {
        let mut rcbs = self.rcbs.lock().await;
        let rcb = rcbs.get_mut(rcb_ref).ok_or_else(|| format!("RCB not found: {}", rcb_ref))?;
        rcb.trg_op = trg_op;
        Ok(())
    }

    /// Set integrity period (0 = disabled)
    pub async fn set_integrity_period(&self, rcb_ref: &str, period_ms: u32) -> Result<(), String> {
        let mut rcbs = self.rcbs.lock().await;
        let rcb = rcbs.get_mut(rcb_ref).ok_or_else(|| format!("RCB not found: {}", rcb_ref))?;
        rcb.integrity_period_ms = period_ms;
        Ok(())
    }

    /// Get all enabled RCBs
    pub async fn enabled_rcbs(&self) -> Vec<ReportControlBlock> {
        self.rcbs.lock().await
            .values()
            .filter(|r| r.enabled)
            .cloned()
            .collect()
    }

    /// Get all registered RCBs
    pub async fn all_rcbs(&self) -> Vec<ReportControlBlock> {
        self.rcbs.lock().await.values().cloned().collect()
    }

    /// Receive a report and store it as the last report for its RCB
    pub async fn receive_report(&self, report: Iec61850ReportData) {
        let rcb_ref = format!("{}.{}", report.dataset_ref, report.report_id);
        tracing::debug!(
            "IEC 61850: received report from {} ({} values)",
            rcb_ref, report.values.len()
        );
        self.last_reports.lock().await.insert(rcb_ref, report);
    }

    /// Get the last received report for an RCB
    pub async fn last_report(&self, rcb_ref: &str) -> Option<Iec61850ReportData> {
        self.last_reports.lock().await.get(rcb_ref).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trg_op_bitmask() {
        let t = TrgOp::DCHG.add(TrgOp::QCHG);
        assert!(t.has(TrgOp::DCHG));
        assert!(t.has(TrgOp::QCHG));
        assert!(!t.has(TrgOp::DUPD));
    }

    #[test]
    fn test_trg_op_remove() {
        let t = TrgOp::DCHG.add(TrgOp::QCHG).remove(TrgOp::DCHG);
        assert!(!t.has(TrgOp::DCHG));
        assert!(t.has(TrgOp::QCHG));
    }

    #[test]
    fn test_trg_op_as_string() {
        let t = TrgOp::DCHG.add(TrgOp::GI);
        assert_eq!(t.as_string(), "dchg,gi");
    }

    #[test]
    fn test_rcb_default() {
        let rcb = ReportControlBlock::default();
        assert_eq!(rcb.rcb_type, RcbType::Buffered);
        assert!(!rcb.enabled);
        assert!(rcb.trg_op.has(TrgOp::DCHG));
        assert!(rcb.trg_op.has(TrgOp::QCHG));
        assert!(rcb.trg_op.has(TrgOp::GI));
    }

    #[test]
    fn test_opt_fields() {
        let rcb = ReportControlBlock {
            opt_fields: ReportControlBlock::OPT_SEQ_NUM | ReportControlBlock::OPT_REASON,
            ..Default::default()
        };
        assert!(rcb.has_opt_field(ReportControlBlock::OPT_SEQ_NUM));
        assert!(rcb.has_opt_field(ReportControlBlock::OPT_REASON));
        assert!(!rcb.has_opt_field(ReportControlBlock::OPT_DATASET_REF));
    }

    #[tokio::test]
    async fn test_rcb_manager_register_enable_disable() {
        let mgr = RcbManager::new();
        let rcb = ReportControlBlock {
            rcb_ref: "LD0/LLN0.brcbTest".to_string(),
            ..Default::default()
        };
        mgr.register(rcb).await;

        assert_eq!(mgr.all_rcbs().await.len(), 1);
        assert!(mgr.enable("LD0/LLN0.brcbTest").await.is_ok());
        assert_eq!(mgr.enabled_rcbs().await.len(), 1);
        assert!(mgr.disable("LD0/LLN0.brcbTest").await.is_ok());
        assert_eq!(mgr.enabled_rcbs().await.len(), 0);
    }

    #[tokio::test]
    async fn test_rcb_manager_enable_not_found() {
        let mgr = RcbManager::new();
        assert!(mgr.enable("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn test_rcb_manager_reserve_urcb() {
        let mgr = RcbManager::new();
        let rcb = ReportControlBlock {
            rcb_ref: "LD0/LLN0.urcbTest".to_string(),
            rcb_type: RcbType::Unbuffered,
            ..Default::default()
        };
        mgr.register(rcb).await;
        assert!(mgr.reserve("LD0/LLN0.urcbTest", "client1").await.is_ok());
        // Reservation by another client should fail
        assert!(mgr.reserve("LD0/LLN0.urcbTest", "client2").await.is_err());
        // Same client re-reserve should succeed
        assert!(mgr.reserve("LD0/LLN0.urcbTest", "client1").await.is_ok());
    }

    #[tokio::test]
    async fn test_rcb_manager_reserve_brcb_fails() {
        let mgr = RcbManager::new();
        let rcb = ReportControlBlock {
            rcb_ref: "LD0/LLN0.brcbTest".to_string(),
            rcb_type: RcbType::Buffered,
            ..Default::default()
        };
        mgr.register(rcb).await;
        assert!(mgr.reserve("LD0/LLN0.brcbTest", "client1").await.is_err());
    }

    #[tokio::test]
    async fn test_rcb_manager_set_trg_op() {
        let mgr = RcbManager::new();
        mgr.register(ReportControlBlock {
            rcb_ref: "LD0/LLN0.brcbTest".to_string(),
            ..Default::default()
        }).await;
        let new_trg = TrgOp::DUPD.add(TrgOp::PERIOD);
        assert!(mgr.set_trg_op("LD0/LLN0.brcbTest", new_trg).await.is_ok());
        let rcbs = mgr.all_rcbs().await;
        assert_eq!(rcbs[0].trg_op, new_trg);
    }

    #[tokio::test]
    async fn test_rcb_manager_set_integrity_period() {
        let mgr = RcbManager::new();
        mgr.register(ReportControlBlock {
            rcb_ref: "LD0/LLN0.brcbTest".to_string(),
            ..Default::default()
        }).await;
        assert!(mgr.set_integrity_period("LD0/LLN0.brcbTest", 5000).await.is_ok());
        let rcbs = mgr.all_rcbs().await;
        assert_eq!(rcbs[0].integrity_period_ms, 5000);
    }

    #[tokio::test]
    async fn test_rcb_manager_receive_and_get_report() {
        let mgr = RcbManager::new();
        let report = Iec61850ReportData {
            report_id: "brcbTest".to_string(),
            seq_num: Some(1),
            dataset_ref: "LD0/LLN0.dsGeneric".to_string(),
            reason_for_inclusion: vec!["dchg".to_string()],
            values: vec![(DataValue::Bool(true), DataQuality::Good)],
            timestamp_ms: 1234567890,
            entry_id: Some(42),
            buffer_overflow: Some(false),
            conf_rev: Some(1),
        };
        mgr.receive_report(report).await;
        let last = mgr.last_report("LD0/LLN0.dsGeneric.brcbTest").await;
        assert!(last.is_some());
        assert_eq!(last.unwrap().seq_num, Some(1));
    }
}
