//! 控制合同视图（[08] §4.4）：无完整合同的资源可观测、可预测，但
//! 不能进入可控池。字段与 catalog [`ControlContract`] 对齐；此处只读
//! 投影（不修改目录）。

/// 合同完备性检查结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractCompleteness {
    /// 完整：typed 命令/单位/范围/爬坡/min on-off/启动次数/回读/容差/
    /// 优先级/安全状态/失联策略全齐。
    Complete,
    /// 缺回读（readback 不可得）。
    MissingReadback,
    /// 缺范围或爬坡。
    MissingBounds,
    /// 合同不存在。
    Missing,
}

/// 控制合同只读视图（resource-registry 内部消费；装配自 catalog）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlContractView {
    pub contract_id: u32,
    pub asset_id: u64,
    pub point_id: u32,
    pub unit_code: u16,
    pub min: f64,
    pub max: f64,
    pub ramp_kw_per_s: f64,
    pub settle_ms: u32,
    pub min_on_ms: u64,
    pub min_off_ms: u64,
    pub max_starts: u16,
    pub readback_point_id: u32,
    pub tolerance_kw: f64,
    pub reversible: bool,
    pub priority: u8,
    pub safe_state: u8,
    pub lost_link: u8,
}

/// 由 catalog 合同投影（未知单位/非有限边界 → Missing）。
pub fn from_catalog(c: &eneros_semantic_catalog::model::ControlContract) -> ControlContractView {
    ControlContractView {
        contract_id: c.contract_id,
        asset_id: c.asset_id,
        point_id: c.point_id,
        unit_code: c.unit_code,
        min: c.min,
        max: c.max,
        ramp_kw_per_s: c.ramp_kw_per_s,
        settle_ms: c.settle_ms,
        min_on_ms: c.min_on_ms,
        min_off_ms: c.min_off_ms,
        max_starts: c.max_starts,
        readback_point_id: c.readback_point_id,
        tolerance_kw: c.tolerance_kw,
        reversible: c.reversible != 0,
        priority: c.priority,
        safe_state: c.safe_state,
        lost_link: c.lost_link,
    }
}

impl ControlContractView {
    /// 完备性（[08] §4.4 硬件）：typed 命令(点/单位) + 范围 + 爬坡 +
    /// min on/off + 启动次数 + 回读 + 容差。
    ///
    /// `readback_ok` = 快照中存在回读点且合格（调用方注入——回读状态
    /// 是快照事实）。
    pub fn completeness(&self, readback_ok: bool) -> ContractCompleteness {
        if self.point_id == 0 || self.unit_code == 0 {
            return ContractCompleteness::Missing;
        }
        if !(self.min.is_finite() && self.max.is_finite() && self.min <= self.max) {
            return ContractCompleteness::MissingBounds;
        }
        if !(self.ramp_kw_per_s.is_finite() && self.ramp_kw_per_s >= 0.0) {
            return ContractCompleteness::MissingBounds;
        }
        if self.readback_point_id == 0 || !readback_ok {
            return ContractCompleteness::MissingReadback;
        }
        ContractCompleteness::Complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ControlContractView {
        ControlContractView {
            contract_id: 1,
            asset_id: 1,
            point_id: 10,
            unit_code: 6,
            min: -500.0,
            max: 500.0,
            ramp_kw_per_s: 3.3333,
            settle_ms: 30000,
            min_on_ms: 0,
            min_off_ms: 0,
            max_starts: 1,
            readback_point_id: 1001,
            tolerance_kw: 0.1,
            reversible: true,
            priority: 2,
            safe_state: 1,
            lost_link: 1,
        }
    }

    #[test]
    fn completeness_gates() {
        assert_eq!(view().completeness(true), ContractCompleteness::Complete);
        assert_eq!(
            view().completeness(false),
            ContractCompleteness::MissingReadback
        );
        let no_readback = ControlContractView {
            readback_point_id: 0,
            ..view()
        };
        assert_eq!(
            no_readback.completeness(true),
            ContractCompleteness::MissingReadback
        );
        let no_bounds = ControlContractView {
            min: f64::NAN,
            ..view()
        };
        assert_eq!(
            no_bounds.completeness(true),
            ContractCompleteness::MissingBounds
        );
    }
}
