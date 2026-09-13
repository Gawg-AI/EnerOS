//! 能力包络（[02] §6 / [08] §4.3）：铭牌 ∩ 合同 ∩ 电气 ∩ SOC/能量 ∩
//! 健康降额 ∩ 维护 ∩ 工艺 ∩ 拓扑 ∩ 市场承诺 ∩ 人工越权——每个收紧项
//! 记录 source/constraint id/原范围/收紧范围/有效期。

/// 可用性档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// 全能力可用。
    Full,
    /// 降额（derating_reason 指明收紧项）。
    Derated,
    /// 不可用（交集为空——原因必须给出）。
    Unavailable,
}

/// 降额原因码（闭枚举；可扩展）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeratingReason {
    None = 0,
    /// 电气限制（电压/电流/频率等状态超限）。
    Electrical = 1,
    /// SOC/能量边界。
    EnergyBound = 2,
    /// 健康降额。
    Health = 3,
    /// 维护窗口。
    Maintenance = 4,
    /// 工艺限制。
    Process = 5,
    /// 拓扑失电/孤岛。
    Topology = 6,
    /// 市场承诺。
    MarketCommitment = 7,
    /// 人工越权收紧。
    OperatorOverride = 8,
    /// 合同限制。
    Contract = 9,
}

/// 单条收紧证据。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tightening {
    /// 收紧来源（provider 名）。
    pub source: &'static str,
    /// 约束 id（合同/安全约束目录 id；0 = provider 内部）。
    pub constraint_id: u32,
    /// 原范围。
    pub original: (f64, f64),
    /// 收紧后范围。
    pub tightened: (f64, f64),
    /// 有效期（0 = 当前快照生命期）。
    pub valid_until_ns: u64,
}

/// 实时能力包络（[02] §6 字段集；全部 f64 kW/kvar/kWh）。
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityEnvelope {
    pub asset_id: u64,
    pub valid_from_ns: u64,
    pub valid_until_ns: u64,
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub q_min_kvar: f64,
    pub q_max_kvar: f64,
    pub energy_min_kwh: f64,
    pub energy_max_kwh: f64,
    pub ramp_up_kw_s: f64,
    pub ramp_down_kw_s: f64,
    pub min_on_ns: u64,
    pub min_off_ns: u64,
    pub availability: Availability,
    pub derating_reason: DeratingReason,
    pub source_snapshot_id: [u8; 16],
    /// 收紧证据链（排序输出，确定性）。
    pub tightenings: alloc::vec::Vec<Tightening>,
    /// 面世世代（成员变化 → generation 变化——聚合失效判定）。
    pub generation: u64,
}

impl CapabilityEnvelope {
    /// 求交内核（[08] §4.3）：`nameplate ∩ ...`；任一维交集为空 →
    /// Unavailable。
    pub fn intersect(
        mut self,
        source: &'static str,
        constraint_id: u32,
        window: (f64, f64),
        valid_until_ns: u64,
    ) -> CapabilityEnvelope {
        // 非法收紧输入：NaN/反向窗口 → 绝不放大，直接不可用（[08] §4.3
        // fail-closed）。±Inf 合法（半开区间：如放电方向 cap (-inf,0)）。
        let window_ok = !window.0.is_nan() && !window.1.is_nan() && window.0 <= window.1;
        if !window_ok {
            self.tightenings.push(Tightening {
                source,
                constraint_id,
                original: (self.p_min_kw, self.p_max_kw),
                tightened: window,
                valid_until_ns,
            });
            self.availability = Availability::Unavailable;
            if self.derating_reason == DeratingReason::None {
                self.derating_reason = DeratingReason::Contract;
            }
            return self;
        }
        let p = (self.p_min_kw.max(window.0), self.p_max_kw.min(window.1));
        self.tightenings.push(Tightening {
            source,
            constraint_id,
            original: (self.p_min_kw, self.p_max_kw),
            tightened: p,
            valid_until_ns,
        });
        if p.0 > p.1 + 1e-9 {
            self.availability = Availability::Unavailable;
            // 保留首个非 None 原因（调用方先设）
            if self.derating_reason == DeratingReason::None {
                self.derating_reason = DeratingReason::Contract;
            }
        }
        self.p_min_kw = p.0.max(self.p_min_kw);
        self.p_max_kw = p.1.min(self.p_max_kw);
        self
    }

    /// 爬坡维求交（[08] §4.3 同律）：合同 `ramp_kw_per_s` 是**运行限值**，
    /// 铭牌爬坡是**物理能力**——包络取二者之紧者（逐向 min），绝不放宽。
    ///
    /// `limit_kw_s <= 0` = **未声明**（编译面 `ramp_kw_per_s` 为可选键，
    /// 缺省 0.0）→ 本函数不收紧、不留证据（不得把"未声明"当"禁止爬坡"：
    /// 那会让一切计划在首个步间被判违约）。非有限/负值 = 非法输入 →
    /// 与 [`Self::intersect`] 同律 fail-closed（Unavailable）。
    pub fn intersect_ramp(
        mut self,
        source: &'static str,
        constraint_id: u32,
        limit_kw_s: f64,
        valid_until_ns: u64,
    ) -> CapabilityEnvelope {
        if !limit_kw_s.is_finite() || limit_kw_s < 0.0 {
            self.tightenings.push(Tightening {
                source,
                constraint_id,
                original: (self.ramp_up_kw_s, self.ramp_down_kw_s),
                tightened: (0.0, 0.0),
                valid_until_ns,
            });
            self.availability = Availability::Unavailable;
            if self.derating_reason == DeratingReason::None {
                self.derating_reason = DeratingReason::Contract;
            }
            return self;
        }
        if limit_kw_s == 0.0 {
            return self; // 未声明：无约束可施加
        }
        let original = (self.ramp_up_kw_s, self.ramp_down_kw_s);
        self.ramp_up_kw_s = self.ramp_up_kw_s.min(limit_kw_s);
        self.ramp_down_kw_s = self.ramp_down_kw_s.min(limit_kw_s);
        self.tightenings.push(Tightening {
            source,
            constraint_id,
            original,
            tightened: (self.ramp_up_kw_s, self.ramp_down_kw_s),
            valid_until_ns,
        });
        self
    }

    /// 数值稳定性：全部边界有限（NaN/Inf → 按 Unavailable 处理——
    /// 任何 provider 输入异常都不得扩大能力）。
    pub fn sanitize(mut self) -> Self {
        for v in [
            &mut self.p_min_kw,
            &mut self.p_max_kw,
            &mut self.q_min_kvar,
            &mut self.q_max_kvar,
            &mut self.energy_min_kwh,
            &mut self.energy_max_kwh,
            &mut self.ramp_up_kw_s,
            &mut self.ramp_down_kw_s,
        ] {
            if !v.is_finite() {
                *v = 0.0;
                self.availability = Availability::Unavailable;
                if self.derating_reason == DeratingReason::None {
                    self.derating_reason = DeratingReason::Contract;
                }
            }
        }
        self
    }

    /// 包络有效性窗口（0 起点/无上限同样成立）。
    pub fn is_valid_at(&self, at_ns: u64) -> bool {
        at_ns >= self.valid_from_ns && at_ns <= self.valid_until_ns
    }

    /// 资源可控性：可用且非不可用（derated 仍可控，按收紧包络）。
    pub fn controllable(&self) -> bool {
        self.availability != Availability::Unavailable
    }

    /// 命令是否落在包络内（[02] §6：所有控制必须落在命令生效时的包络内）。
    pub fn contains_power(&self, power_kw: f64, at_ns: u64) -> bool {
        if !power_kw.is_finite() || !self.controllable() {
            return false;
        }
        if at_ns < self.valid_from_ns || at_ns > self.valid_until_ns {
            return false;
        }
        power_kw >= self.p_min_kw - 1e-9 && power_kw <= self.p_max_kw + 1e-9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> CapabilityEnvelope {
        CapabilityEnvelope {
            asset_id: 1,
            valid_from_ns: 0,
            valid_until_ns: u64::MAX,
            p_min_kw: -500.0,
            p_max_kw: 500.0,
            q_min_kvar: -100.0,
            q_max_kvar: 100.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: 1000.0,
            ramp_up_kw_s: 3.3333,
            ramp_down_kw_s: 3.3333,
            min_on_ns: 0,
            min_off_ns: 0,
            availability: Availability::Full,
            derating_reason: DeratingReason::None,
            source_snapshot_id: [1u8; 16],
            tightenings: alloc::vec::Vec::new(),
            generation: 1,
        }
    }

    #[test]
    fn intersection_tightens_with_evidence() {
        let e = base()
            .intersect("contract", 1, (-200.0, 200.0), u64::MAX)
            .intersect("health", 2, (-300.0, 350.0), u64::MAX);
        assert_eq!(e.p_min_kw, -200.0);
        assert_eq!(e.p_max_kw, 200.0);
        assert_eq!(e.tightenings.len(), 2);
        assert_eq!(e.tightenings[0].source, "contract");
        assert_eq!(e.tightenings[0].original, (-500.0, 500.0));
        assert_eq!(e.tightenings[0].tightened, (-200.0, 200.0));
        assert!(e.controllable());
        assert!(e.contains_power(150.0, 0));
        assert!(!e.contains_power(250.0, 0));
    }

    #[test]
    fn empty_intersection_unavailable_with_reason() {
        let e = base()
            .intersect("maintenance", 3, (-10.0, -5.0), u64::MAX)
            .intersect("market", 4, (50.0, 100.0), u64::MAX);
        // (-10,-5)∩(50,100) = 空 → Unavailable。
        assert_eq!(e.availability, Availability::Unavailable);
        assert!(!e.controllable());
        assert!(!e.contains_power(60.0, 0));
    }

    #[test]
    fn nan_inputs_never_expand() {
        let e = base()
            .intersect("electrical", 5, (f64::NAN, 200.0), u64::MAX)
            .sanitize();
        assert_eq!(e.availability, Availability::Unavailable);
    }

    #[test]
    fn envelope_validity_window_respected() {
        let e = base();
        // 有效期跨越命令执行时刻 → 拒绝（[08] §4.7）。
        assert!(e.contains_power(100.0, 0));
        let expired = CapabilityEnvelope {
            valid_from_ns: 100,
            valid_until_ns: 200,
            ..base()
        };
        assert!(!expired.contains_power(100.0, 50));
        assert!(!expired.contains_power(100.0, 300));
        assert!(expired.contains_power(100.0, 150));
    }
}
