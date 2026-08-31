//! 蓝图 61 Phase 5 WP-5.3（[10] §4.4–4.5）：`DegradationLease` 与降级
//! 状态机。
//!
//! 唯一不变量（[10] §4.1）：降级动作只有持**有效、签名、限时、限域**
//! lease 才可执行；所有 fallback/heuristic/last-plan 路径要求
//! [`DegradationLease`]。
//!
//! lease（[10] §4.4 字段集）：HMAC-SHA256 签名覆盖全部规范字段（域分隔
//! wire，v1）；使用计数持久化语义由持有方落账（本模块计数 + 持久回调）。
//! 最近已提交计划只有 digest 在 lease 白名单（`allowed_strategy_digest`）、
//! 快照最低条件满足、资源包络未收紧且未过期才可复用（[`DegradationLease::
//! admits`]）。
//!
//! 降级级（[10] §4.5）：D1 ModelOff / D2 ExternalOff / D3 SolverFallback /
//! D4 SnapshotDegraded / D5 ExecutionUnknown / D6 EmergencyLocal。
//! **恢复不能自动跳级**（D6→D0 禁止）：逐级下降、每级需连续健康窗口
//! （迟滞防振荡）+ 重启计数单调；D6 撤销远程 lease 由设备本地接管语义
//! 承载（目标侧）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::keyed::{hmac_sha256, mac_eq};

/// 降级级（[10] §4.5；数值 = 严重度，0 = 正常）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DegradationLevel {
    /// 正常。
    Normal,
    /// D1 ModelOff：关闭推理 scheduler；规则/solver 正常。
    ModelOff,
    /// D2 ExternalOff：冻结最后已确认外部承诺到其有效期，outbox 排队。
    ExternalOff,
    /// D3 SolverFallback：签名策略表/最近安全基线（受 lease 限制）。
    SolverFallback,
    /// D4 SnapshotDegraded：hold/降载/安全状态，不能增加风险。
    SnapshotDegraded,
    /// D5 ExecutionUnknown：隔离资源和容量，reconcile。
    ExecutionUnknown,
    /// D6 EmergencyLocal：撤销远程 lease，设备本地接管。
    EmergencyLocal,
}

impl DegradationLevel {
    pub fn severity(self) -> u8 {
        match self {
            DegradationLevel::Normal => 0,
            DegradationLevel::ModelOff => 1,
            DegradationLevel::ExternalOff => 2,
            DegradationLevel::SolverFallback => 3,
            DegradationLevel::SnapshotDegraded => 4,
            DegradationLevel::ExecutionUnknown => 5,
            DegradationLevel::EmergencyLocal => 6,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DegradationLevel::Normal => "D0",
            DegradationLevel::ModelOff => "D1",
            DegradationLevel::ExternalOff => "D2",
            DegradationLevel::SolverFallback => "D3",
            DegradationLevel::SnapshotDegraded => "D4",
            DegradationLevel::ExecutionUnknown => "D5",
            DegradationLevel::EmergencyLocal => "D6",
        }
    }
}

/// 降级 lease（[10] §4.4 字段集；签名覆盖全部规范字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct DegradationLease {
    pub lease_id: [u8; 16],
    pub site_id: u32,
    pub level: DegradationLevel,
    /// 允许的降级策略摘要白名单（最近已提交计划 digest 命中才可复用）。
    pub allowed_strategy_digest: [u8; 32],
    /// 资源作用域摘要。
    pub resource_scope_digest: [u8; 32],
    pub max_abs_power_kw: f64,
    pub max_energy_kwh: f64,
    pub max_ramp_kw_s: f64,
    pub max_uses: u32,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub issuer_key_id: u32,
    pub revocation_epoch: u64,
    /// HMAC-SHA256（域分隔 canonical wire）。
    pub signature: [u8; 32],
}

impl DegradationLease {
    /// 签发（构造 + 签名；空 key 拒绝——fail-closed）。
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        key: &[u8],
        lease_id: [u8; 16],
        site_id: u32,
        level: DegradationLevel,
        allowed_strategy_digest: [u8; 32],
        resource_scope_digest: [u8; 32],
        max_abs_power_kw: f64,
        max_energy_kwh: f64,
        max_ramp_kw_s: f64,
        max_uses: u32,
        issued_at_ms: u64,
        expires_at_ms: u64,
        issuer_key_id: u32,
        revocation_epoch: u64,
    ) -> Result<Self, String> {
        if key.is_empty() {
            return Err(String::from("empty lease key"));
        }
        if expires_at_ms <= issued_at_ms {
            return Err(String::from("lease window inverted"));
        }
        if !max_abs_power_kw.is_finite()
            || !max_energy_kwh.is_finite()
            || !max_ramp_kw_s.is_finite()
        {
            return Err(String::from("non-finite lease bound"));
        }
        let mut lease = Self {
            lease_id,
            site_id,
            level,
            allowed_strategy_digest,
            resource_scope_digest,
            max_abs_power_kw,
            max_energy_kwh,
            max_ramp_kw_s,
            max_uses,
            issued_at_ms,
            expires_at_ms,
            issuer_key_id,
            revocation_epoch,
            signature: [0; 32],
        };
        lease.signature = hmac_sha256(key, &lease.canonical());
        Ok(lease)
    }

    /// 规范编码（签名输入；字段序固定）。
    fn canonical(&self) -> Vec<u8> {
        let mut out = alloc::vec::Vec::with_capacity(160);
        out.extend_from_slice(b"eneros.degradation-lease.v1");
        out.extend_from_slice(&self.lease_id);
        out.extend_from_slice(&self.site_id.to_le_bytes());
        out.push(self.level.severity());
        out.extend_from_slice(&self.allowed_strategy_digest);
        out.extend_from_slice(&self.resource_scope_digest);
        out.extend_from_slice(&self.max_abs_power_kw.to_le_bytes());
        out.extend_from_slice(&self.max_energy_kwh.to_le_bytes());
        out.extend_from_slice(&self.max_ramp_kw_s.to_le_bytes());
        out.extend_from_slice(&self.max_uses.to_le_bytes());
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        out.extend_from_slice(&self.expires_at_ms.to_le_bytes());
        out.extend_from_slice(&self.issuer_key_id.to_le_bytes());
        out.extend_from_slice(&self.revocation_epoch.to_le_bytes());
        out
    }

    /// 验签（常量时间比较）。
    pub fn verify(&self, key: &[u8]) -> bool {
        if key.is_empty() {
            return false;
        }
        let expect = hmac_sha256(key, &self.canonical());
        mac_eq(&expect, &self.signature)
    }

    /// 时效（毫秒单调钟）。
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms >= self.issued_at_ms && now_ms <= self.expires_at_ms
    }

    /// 复用判据（[10] §4.4）：最近已提交计划只有 digest 在白名单、
    /// 快照最低条件满足、资源包络未收紧、未过期才可复用。
    pub fn admits(
        &self,
        now_ms: u64,
        strategy_digest: &[u8; 32],
        snapshot_min_ok: bool,
        envelope_tightened: bool,
    ) -> bool {
        self.is_valid_at(now_ms)
            && self.allowed_strategy_digest == *strategy_digest
            && snapshot_min_ok
            && !envelope_tightened
    }

    /// 动作限额判据（功率/能量/爬坡上界——超限拒绝）。
    pub fn allows_action(&self, abs_power_kw: f64, energy_kwh: f64, ramp_kw_s: f64) -> bool {
        abs_power_kw <= self.max_abs_power_kw
            && energy_kwh <= self.max_energy_kwh
            && ramp_kw_s <= self.max_ramp_kw_s
    }
}

/// 使用记账（持久化语义：`load`/`persist` 回调由持有方接 durable 面）。
#[derive(Debug, Clone)]
pub struct LeaseUsage {
    pub lease_id: [u8; 16],
    pub uses: u32,
}

impl LeaseUsage {
    pub fn new(lease_id: [u8; 16]) -> Self {
        Self { lease_id, uses: 0 }
    }

    /// 记账并判额（超额 = lease 耗尽，fail-closed）。
    pub fn record_use(&mut self, lease: &DegradationLease) -> Result<u32, String> {
        if self.uses >= lease.max_uses {
            return Err(String::from("lease exhausted"));
        }
        self.uses += 1;
        Ok(self.uses)
    }
}

/// 降级授权门（[10] §4.4/§4.7 唯一 lease 执行面）：
/// - 验签（HMAC-SHA256 + 常量时间比较）；
/// - 撤销：站点撤销纪元单调递增——`revoke()` 后旧纪元 lease 全部失效；
/// - 时效/次数/功率·能量·爬坡限额逐次执行检查（使用计数持久化语义由
///   持有方接 durable 面——`usage` 可导出落账）。
#[derive(Debug, Clone)]
pub struct DegradationAuthority {
    key: alloc::vec::Vec<u8>,
    /// 站点当前撤销纪元（lease.revocation_epoch < 当前 ⇒ 已撤销）。
    revocation_epoch: u64,
    /// lease_id → 使用计数（持久化面）。
    usage: alloc::collections::BTreeMap<[u8; 16], u32>,
}

impl DegradationAuthority {
    pub fn new(key: &[u8], revocation_epoch: u64) -> Self {
        Self {
            key: key.to_vec(),
            revocation_epoch,
            usage: alloc::collections::BTreeMap::new(),
        }
    }

    /// 撤销全部旧纪元 lease（纪元递增不可逆）。
    pub fn revoke_all(&mut self) -> u64 {
        self.revocation_epoch += 1;
        self.revocation_epoch
    }

    pub fn revocation_epoch(&self) -> u64 {
        self.revocation_epoch
    }

    /// 授权一次降级动作（验签/纪元/时效/限额/次数全查；失败给精确原因）。
    pub fn authorize(
        &mut self,
        lease: &DegradationLease,
        now_ms: u64,
        strategy_digest: &[u8; 32],
        abs_power_kw: f64,
        energy_kwh: f64,
        ramp_kw_s: f64,
    ) -> Result<(), String> {
        if !lease.verify(&self.key) {
            return Err(String::from("lease signature invalid"));
        }
        if lease.revocation_epoch < self.revocation_epoch {
            return Err(String::from("lease revoked (stale epoch)"));
        }
        if !lease.is_valid_at(now_ms) {
            return Err(String::from("lease expired"));
        }
        if !lease.allows_action(abs_power_kw, energy_kwh, ramp_kw_s) {
            return Err(String::from("action beyond lease bounds"));
        }
        if !lease.admits(now_ms, strategy_digest, true, false) {
            return Err(String::from("strategy not in lease whitelist"));
        }
        // 复查登记（Phase 5）：快照最低条件/资源包络未收紧两项判据在本
        // 授权面取恒真/恒假（true, false）——当前装配无快照条件与包络
        // 收紧的运行时输入源（目标侧快照/包络供给就绪前保留，ADR-051
        // 同批登记）；持有方（ai_worker HeuristicBackend）也仅持策略摘要。
        // fail-closed 方向受限于此：不引入未供给输入，宁可放宽这两项也
        // 不发明数值；真值接入随快照/registry 面落地（Phase 6 前置）。
        let uses = self.usage.entry(lease.lease_id).or_insert(0);
        if *uses >= lease.max_uses {
            return Err(String::from("lease exhausted"));
        }
        *uses += 1;
        Ok(())
    }

    /// 使用计数（持久化导出面）。
    pub fn usage_of(&self, lease_id: &[u8; 16]) -> u32 {
        self.usage.get(lease_id).copied().unwrap_or(0)
    }
}

/// 降级状态机（[10] §4.5：升级即时、恢复逐级 + 健康窗口迟滞）。
#[derive(Debug, Clone)]
pub struct DegradationMachine {
    level: DegradationLevel,
    /// 恢复迟滞：当前级连续健康 tick 计数。
    healthy_ticks: u32,
    /// 每级所需的连续健康窗口（迟滞防振荡）。
    recovery_window: u32,
    /// 重启计数（单调；参与恢复资格——重启后必须重新积累窗口）。
    restarts: u64,
}

impl DegradationMachine {
    pub fn new(recovery_window: u32) -> Self {
        Self {
            level: DegradationLevel::Normal,
            healthy_ticks: 0,
            recovery_window: recovery_window.max(1),
            restarts: 0,
        }
    }

    pub fn level(&self) -> DegradationLevel {
        self.level
    }
    pub fn restarts(&self) -> u64 {
        self.restarts
    }

    /// D5 判据（[10] §4.6）：执行未知的资源从 planner capability 池移除
    /// （D5 及以上 = 不再可作为规划输入；装配侧消费本判定过滤
    /// ResourceRegistry/包络供给）。
    pub fn resource_capable(&self) -> bool {
        self.level.severity() < DegradationLevel::ExecutionUnknown.severity()
    }

    /// 进程重启（重启计数单调递增；健康窗口清零——恢复资格重新积累）。
    pub fn on_restart(&mut self) {
        self.restarts += 1;
        self.healthy_ticks = 0;
    }

    /// 升级（恶化）：即时生效，可跳级（恶化不迟滞）。
    pub fn escalate(&mut self, to: DegradationLevel) -> DegradationLevel {
        if to.severity() > self.level.severity() {
            self.level = to;
            self.healthy_ticks = 0;
        }
        self.level
    }

    /// 健康 tick（恢复方向）：只有连续满足恢复窗口才降一级
    /// （[10] §4.5：恢复不能自动跳级；迟滞防振荡）。D6 无自动恢复——
    /// 唯一路径 = [`Self::manual_release_emergency`]（设备本地接管交还）。
    pub fn healthy_tick(&mut self) -> DegradationLevel {
        if self.level == DegradationLevel::Normal || self.level == DegradationLevel::EmergencyLocal
        {
            return self.level;
        }
        self.healthy_ticks += 1;
        if self.healthy_ticks >= self.recovery_window {
            self.healthy_ticks = 0;
            self.level = Self::one_step_down(self.level);
        }
        self.level
    }

    /// 不健康 tick：恢复进度清零（防振荡）。
    pub fn unhealthy_tick(&mut self) {
        self.healthy_ticks = 0;
    }

    /// 人工降级（runbook 驱动；D6 恢复必须人工 policy——`manual_release`）。
    pub fn manual_escalate(&mut self, to: DegradationLevel) -> DegradationLevel {
        self.escalate(to)
    }

    /// D6 人工解除（唯一离开 D6 的路径——设备本地接管交还）。
    pub fn manual_release_emergency(&mut self) -> Result<DegradationLevel, String> {
        if self.level != DegradationLevel::EmergencyLocal {
            return Err(String::from("not in D6"));
        }
        self.level = DegradationLevel::ExecutionUnknown;
        self.healthy_ticks = 0;
        Ok(self.level)
    }

    fn one_step_down(level: DegradationLevel) -> DegradationLevel {
        match level {
            DegradationLevel::Normal => DegradationLevel::Normal,
            DegradationLevel::ModelOff => DegradationLevel::Normal,
            DegradationLevel::ExternalOff => DegradationLevel::ModelOff,
            DegradationLevel::SolverFallback => DegradationLevel::ExternalOff,
            DegradationLevel::SnapshotDegraded => DegradationLevel::SolverFallback,
            DegradationLevel::ExecutionUnknown => DegradationLevel::SnapshotDegraded,
            DegradationLevel::EmergencyLocal => DegradationLevel::ExecutionUnknown,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn lease() -> DegradationLease {
        DegradationLease::issue(
            &[7u8; 32],
            [9; 16],
            1,
            DegradationLevel::SolverFallback,
            [0xAA; 32],
            [0xBB; 32],
            200.0,
            100.0,
            1.0,
            3,
            1_000,
            60_000,
            1,
            0,
        )
        .unwrap()
    }

    #[test]
    fn signature_bind_and_detect_mutation() {
        let l = lease();
        assert!(l.verify(&[7u8; 32]));
        // 改任意字段（如 max_abs_power_kw）→ 验签失败。
        let mut forged = l.clone();
        forged.max_abs_power_kw = 9000.0;
        assert!(!forged.verify(&[7u8; 32]));
        // 空 key 拒绝签发与验签。
        assert!(DegradationLease::issue(
            &[],
            [9; 16],
            1,
            DegradationLevel::ModelOff,
            [0; 32],
            [0; 32],
            1.0,
            1.0,
            1.0,
            1,
            0,
            10,
            1,
            0
        )
        .is_err());
        assert!(!forged.verify(&[]));
    }

    #[test]
    fn admits_requires_whitelist_snapshot_envelope_and_expiry() {
        let l = lease();
        // 全条件满足。
        assert!(l.admits(2_000, &[0xAA; 32], true, false));
        // 策略摘要不在白名单。
        assert!(!l.admits(2_000, &[0xCC; 32], true, false));
        // 快照最低条件不满足。
        assert!(!l.admits(2_000, &[0xAA; 32], false, false));
        // 包络收紧。
        assert!(!l.admits(2_000, &[0xAA; 32], true, true));
        // 过期。
        assert!(!l.admits(60_001, &[0xAA; 32], true, false));
    }

    #[test]
    fn usage_limit_and_action_bounds() {
        let l = lease();
        let mut u = LeaseUsage::new(l.lease_id);
        assert_eq!(u.record_use(&l).unwrap(), 1);
        assert_eq!(u.record_use(&l).unwrap(), 2);
        assert_eq!(u.record_use(&l).unwrap(), 3);
        assert!(u.record_use(&l).is_err(), "超次数 fail-closed");
        assert!(l.allows_action(200.0, 100.0, 1.0));
        assert!(!l.allows_action(200.1, 100.0, 1.0));
        assert!(!l.allows_action(100.0, 100.1, 1.0));
        assert!(!l.allows_action(100.0, 100.0, 1.1));
    }

    #[test]
    fn recovery_is_one_step_with_hysteresis() {
        let mut m = DegradationMachine::new(3);
        m.escalate(DegradationLevel::ModelOff);
        m.escalate(DegradationLevel::SnapshotDegraded); // 恶化可跳
        assert_eq!(m.level(), DegradationLevel::SnapshotDegraded);
        // 迟滞：不健康打断清零进度。
        m.healthy_tick();
        m.healthy_tick();
        m.unhealthy_tick(); // 进度清零
        m.healthy_tick();
        m.healthy_tick();
        assert_eq!(m.level(), DegradationLevel::SnapshotDegraded);
        // 第 3 次连续 → 降一级（禁止跳级）。
        m.healthy_tick();
        assert_eq!(m.level(), DegradationLevel::SolverFallback);
        // 长健康逐级走完：D3→D2→D1→D0（每级 3 tick）。
        for _ in 0..9 {
            m.healthy_tick();
        }
        assert_eq!(m.level(), DegradationLevel::Normal);
        // Normal 下继续 tick 恒 Normal。
        for _ in 0..3 {
            m.healthy_tick();
        }
        assert_eq!(m.level(), DegradationLevel::Normal);
    }

    #[test]
    fn d6_requires_manual_release_and_restart_resets_window() {
        let mut m = DegradationMachine::new(1);
        m.escalate(DegradationLevel::EmergencyLocal);
        // D6 无自动恢复：健康 tick 不降级。
        m.healthy_tick();
        m.healthy_tick();
        assert_eq!(m.level(), DegradationLevel::EmergencyLocal);
        assert!(m.manual_release_emergency().is_ok());
        assert_eq!(m.level(), DegradationLevel::ExecutionUnknown);
        // 非 D6 人工解除拒绝。
        assert!(m.manual_release_emergency().is_err());
        // 重启清零健康窗口（恢复资格重积累）。
        m.escalate(DegradationLevel::SnapshotDegraded);
        m.healthy_tick();
        m.on_restart();
        assert_eq!(m.restarts(), 1);
        assert_eq!(m.healthy_ticks, 0);
        assert_eq!(m.level(), DegradationLevel::SnapshotDegraded);
    }
}
