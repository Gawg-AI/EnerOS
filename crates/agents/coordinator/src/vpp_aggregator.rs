//! EnerOS v0.94.0 Edge Coordinator VPP 聚合.
//!
//! 聚合域内 DER 容量形成 [`VppProfile`]（容量/爬坡）→ 聚合出力控制（target 分配到
//! 各资源，复用 v0.93.0 [`DomainOptimizer`]）→ 市场申报（生成 Sell 报价，复用
//! v0.86.0 `Bid` 族），为 v0.95.0 云端策略下发与 v0.96.0 Cloud Coordinator 提供
//! 聚合基础（Phase 2 出口标准，P2-D 关键版）。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 模块位于既有 `crates/agents/coordinator/src/vpp_aggregator.rs`（工作区 §2.3.1 硬规则，v0.92.0/v0.93.0 D1 惯例；同 crate 追加） |
//! | **D2** | `resource_id: u64` + `BTreeMap<u64, VppResource>`（无堆字符串 + 确定性迭代，v0.87.0 D3 / v0.93.0 D2 惯例；聚合与分配顺序可重放） |
//! | **D3** | sync `dispatch(&mut self, market, target_mw, now_ms)`（no_std 无 async runtime；`&mut` 因 `DomainOptimizer::optimize` 需 `&mut` 且计数器更新，v0.93.0 D5 惯例） |
//! | **D4** | `AggregatedDispatch.timestamp = now_ms`（u64 ms 外部时间注入）；`aggregate(&mut self)` 因 `aggregate_count` 计数器更新（内部 profile 计算为私有 `&self` 免计数） |
//! | **D5** | `MarketData` 未派生 Default（v0.85.0 实现核实），蓝图 `MarketData::default()` 无法编译 → dispatch 增加 `market: &MarketData` 显式注入（净收益在 DomainPlan 内计算，v0.93.0 D12 链路透传） |
//! | **D6** | `VppResource.online` + `set_online` / `set_available`（蓝图 §6.5 资源离线聚合重算、§5.4 容量动态变化）；离线资源从聚合与分配排除，状态保留便于恢复（v0.93.0 D8 惯例） |
//! | **D7** | `VppResource` 增加 `efficiency: f32`（复用的 DomainOptimizer 损耗最小目标可区分高效/低效 DER，否则 LP 目标退化为常数；NaN→0.5 clamp [0,1]，v0.93.0 D12 一致） |
//! | **D8** | 复用 v0.93.0 `DomainOptimizer`：sync_boxes 将每在线资源映射为单设备 box（box_id=device_id=resource_id，`p_min=0`、`p_max=box capacity=available_mw`，`soc=1.0` 恒通过合格过滤）；"分配失败 → 重新优化"落地为 optimizer 内建容量比例兜底（不迭代重试 LP，v0.93.0 D10 惯例） |
//! | **D9** | 3 个 pub 计数器 `aggregate_count`/`dispatch_count`/`reject_count`（拒绝 = InvalidTarget + InsufficientCapacity + NoResource 三路合计；聚合容量经 `VppProfile` 可观测） |
//! | **D10** | 资源不足落地为**拒绝**（确定性）：`target_mw.abs() > profile.available_mw` → `Err(VppError::InsufficientCapacity)`（含负 target 充电场景，abs 判定） |
//! | **D11** | `ramp_down = ramp_up` 对称实现（蓝图关键代码一致）；`ramp_rate` 非有限或 <0 → 按 0 计入 profile（ramp 仅上报不参与分配，不阻断调度） |
//! | **D12** | NaN 防御（v0.88.0 C140 / v0.93.0 D12 教训）：capacity 非有限或 ≤0 → 资源从聚合排除；available 非有限 → 0 且 clamp [0, capacity]；efficiency NaN→0.5 clamp [0,1]；price 非有限 → bid price 按 0+margin。复用 domain_optimizer 的 `sanitize_capacity`/`sanitize_efficiency`/`sanitize_price`（可见性放宽为 `pub(crate)`，零逻辑改动） |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_energy_market_agent::{
    Bid, BidSide, BidStrategy, DeviceCapability, DevicePool, MarketData, MarketType, Period,
};
use eneros_solver_core::solver::Solver;

use crate::domain_optimizer::{
    sanitize_capacity, sanitize_efficiency, sanitize_price, DomainOptimizer, EdgeBoxState, OptError,
};

/// VPP 资源类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceType {
    /// 储能电池（默认）.
    #[default]
    Battery,
    /// 光伏.
    Pv,
    /// 可调负荷.
    Load,
    /// 充电桩.
    Charger,
}

/// VPP 资源（D2/D6/D7）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VppResource {
    /// 资源 ID（u64，无堆字符串，D2）.
    pub resource_id: u64,
    /// 额定容量（MW；非有限或 ≤ 0 → 资源从聚合排除，D12）.
    pub capacity_mw: f32,
    /// 当前可用容量（MW；非有限 → 0，clamp [0, capacity]，D12）.
    pub available_mw: f32,
    /// 爬坡速率（MW·min⁻¹；非有限或 <0 → 按 0 计入，D11）.
    pub ramp_rate: f32,
    /// 转换效率（0~1；NaN → 0.5 中性 clamp [0,1]，D7/D12）.
    pub efficiency: f32,
    /// 资源类型.
    pub type_: ResourceType,
    /// 在线标记（D6：离线从聚合与分配排除，状态保留便于恢复）.
    pub online: bool,
}

/// VPP 聚合画像（容量聚合结果）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VppProfile {
    /// 聚合总额定容量（MW，Σ在线有效 capacity）.
    pub total_capacity_mw: f32,
    /// 聚合当前可用容量（MW，Σsanitize(available)）.
    pub available_mw: f32,
    /// 聚合上爬坡速率（MW·min⁻¹，Σsanitize(ramp)，D11）.
    pub ramp_up_mw_per_min: f32,
    /// 聚合下爬坡速率（MW·min⁻¹，对称 = ramp_up，D11）.
    pub ramp_down_mw_per_min: f32,
}

/// 单资源出力分配.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Allocation {
    /// 资源 ID（= DomainOptimizer 内 device_id，D8 映射）.
    pub resource_id: u64,
    /// 设定功率（MW）.
    pub setpoint_mw: f32,
}

/// 聚合出力分配结果（D4：timestamp 回显 now_ms）.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AggregatedDispatch {
    /// 聚合目标功率（MW，回显入参）.
    pub target_mw: f32,
    /// 各资源分配（resource_id 升序）.
    pub allocations: Vec<Allocation>,
    /// 分配时刻时间戳（u64 ms，回显 `now_ms`）.
    pub timestamp: u64,
}

/// VPP 聚合错误（D9：三路拒绝合计入 reject_count）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VppError {
    /// 聚合可用容量不足（|target| > available，D10）.
    InsufficientCapacity,
    /// 目标非法（NaN / ±∞）.
    InvalidTarget,
    /// 无可用资源（空聚合器 / 全离线 / 全部容量无效）.
    NoResource,
}

/// 可用容量过滤（D12）：非有限 → 0.0；否则 clamp [0, cap]（cap 无效 → 0）.
fn sanitize_available(avail: f32, cap: f32) -> f32 {
    if !avail.is_finite() {
        return 0.0;
    }
    match sanitize_capacity(cap) {
        Some(c) => avail.clamp(0.0, c),
        None => 0.0,
    }
}

/// 爬坡过滤（D11）：非有限或 < 0 → 0.0；否则原样.
fn sanitize_ramp(r: f32) -> f32 {
    if !r.is_finite() || r < 0.0 {
        0.0
    } else {
        r
    }
}

/// Edge Coordinator VPP 聚合器（D9：字段全 pub 可观测）.
pub struct VppAggregator {
    /// 域内 VPP 资源表（resource_id 升序，BTreeMap 确定性，D2）.
    pub resources: BTreeMap<u64, VppResource>,
    /// 复用 v0.93.0 域级优化器（D8：sync_boxes 映射单设备 box）.
    pub optimizer: DomainOptimizer,
    /// aggregate 调用次数.
    pub aggregate_count: u64,
    /// dispatch 调用次数.
    pub dispatch_count: u64,
    /// 拒绝次数（InvalidTarget + InsufficientCapacity + NoResource，D9）.
    pub reject_count: u64,
}

impl VppAggregator {
    /// 创建聚合器（计数器全零，无资源）.
    pub fn new(solver: Box<dyn Solver>) -> Self {
        Self {
            resources: BTreeMap::new(),
            optimizer: DomainOptimizer::new(solver),
            aggregate_count: 0,
            dispatch_count: 0,
            reject_count: 0,
        }
    }

    /// 添加/更新资源（同 id 覆盖）.
    pub fn add_resource(&mut self, resource: VppResource) {
        self.resources.insert(resource.resource_id, resource);
    }

    /// 移除资源；存在返回 true，不存在返回 false.
    pub fn remove_resource(&mut self, resource_id: u64) -> bool {
        self.resources.remove(&resource_id).is_some()
    }

    /// 设置在线标记（D6）；资源不存在返回 false，离线保留状态便于恢复.
    pub fn set_online(&mut self, resource_id: u64, online: bool) -> bool {
        if let Some(r) = self.resources.get_mut(&resource_id) {
            r.online = online;
            true
        } else {
            false
        }
    }

    /// 调整可用容量（蓝图 §5.4 容量动态变化）；资源不存在返回 false.
    pub fn set_available(&mut self, resource_id: u64, available_mw: f32) -> bool {
        if let Some(r) = self.resources.get_mut(&resource_id) {
            r.available_mw = available_mw;
            true
        } else {
            false
        }
    }

    /// 聚合容量画像（私有免计数内部版，D4）.
    ///
    /// 仅统计 `online && sanitize_capacity(capacity_mw).is_some()` 的资源；
    /// 空聚合器/全离线 → 全零 profile（Default）。
    fn compute_profile(&self) -> VppProfile {
        let mut profile = VppProfile::default();
        for r in self.resources.values() {
            if !r.online {
                continue;
            }
            let cap = match sanitize_capacity(r.capacity_mw) {
                Some(c) => c,
                None => continue,
            };
            profile.total_capacity_mw += cap;
            profile.available_mw += sanitize_available(r.available_mw, r.capacity_mw);
            let ramp = sanitize_ramp(r.ramp_rate);
            profile.ramp_up_mw_per_min += ramp;
            profile.ramp_down_mw_per_min += ramp;
        }
        profile
    }

    /// 聚合容量画像（D4：aggregate_count += 1 后调内部免计数版）.
    pub fn aggregate(&mut self) -> VppProfile {
        self.aggregate_count += 1;
        self.compute_profile()
    }

    /// 同步在线资源为 DomainOptimizer 的单设备 EdgeBoxState（D8）.
    ///
    /// 先清后填：仅写入 `online && sanitize_capacity 有效` 的资源；
    /// box_id = device_id = resource_id，`p_min=0`、`p_max=box capacity=sanitize(available)`，
    /// `soc=1.0` 恒通过合格过滤，离线/无效 capacity 不写入。
    fn sync_boxes(&mut self) {
        self.optimizer.edge_boxes.clear();
        for (rid, r) in self.resources.iter() {
            if !r.online {
                continue;
            }
            if sanitize_capacity(r.capacity_mw).is_none() {
                continue;
            }
            let avail = sanitize_available(r.available_mw, r.capacity_mw);
            let mut devices = DevicePool::new();
            devices.add_device(
                *rid,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: avail,
                    ramp_rate: sanitize_ramp(r.ramp_rate),
                    efficiency: sanitize_efficiency(r.efficiency),
                },
            );
            let mut socs = BTreeMap::new();
            socs.insert(*rid, 1.0f32);
            self.optimizer.edge_boxes.insert(
                *rid,
                EdgeBoxState {
                    box_id: *rid,
                    devices,
                    socs,
                    capacity_mw: avail,
                    online: true,
                },
            );
        }
    }

    /// 聚合出力分配（D3/D5/D8/D10）.
    ///
    /// 流程：dispatch_count += 1 → target 非有限拒绝（InvalidTarget）→
    /// `|target| > profile.available` 拒绝（InsufficientCapacity，D10）→
    /// sync_boxes（D8）→ `optimizer.optimize`：Ok → flat_map assignments 为
    /// allocations（device_id 即 resource_id）；Err(EmptyDomain) → 拒绝（NoResource）；
    /// Err(InvalidTarget) → 拒绝（InvalidTarget，防御分支）。`timestamp = now_ms`。
    pub fn dispatch(
        &mut self,
        market: &MarketData,
        target_mw: f32,
        now_ms: u64,
    ) -> Result<AggregatedDispatch, VppError> {
        self.dispatch_count += 1;
        if !target_mw.is_finite() {
            self.reject_count += 1;
            return Err(VppError::InvalidTarget);
        }
        let profile = self.compute_profile();
        if target_mw.abs() > profile.available_mw {
            self.reject_count += 1;
            return Err(VppError::InsufficientCapacity);
        }
        self.sync_boxes();
        match self.optimizer.optimize(market, target_mw, now_ms) {
            Ok(plan) => {
                // box_plans BTreeMap 天然 box_id 升序 = resource_id 升序（D2）
                let allocations: Vec<Allocation> = plan
                    .box_plans
                    .values()
                    .flat_map(|p| {
                        p.assignments.iter().map(|a| Allocation {
                            resource_id: a.device_id,
                            setpoint_mw: a.setpoint,
                        })
                    })
                    .collect();
                Ok(AggregatedDispatch {
                    target_mw,
                    allocations,
                    timestamp: now_ms,
                })
            }
            Err(OptError::EmptyDomain) => {
                self.reject_count += 1;
                Err(VppError::NoResource)
            }
            Err(OptError::InvalidTarget) => {
                // 防御分支：target 已在上方校验，理论上不可达
                self.reject_count += 1;
                Err(VppError::InvalidTarget)
            }
        }
    }

    /// 市场申报（纯查询无计数器更新，D12）.
    ///
    /// 按 resource_id 升序遍历在线资源，跳过 sanitize(available) ≤ 0 者；
    /// `quantity = min(available, strategy.max_quantity)`（max_quantity 非有限或 ≤0 →
    /// 按 available 全额）；`price = sanitize_price(market.current_price) + strategy.margin`；
    /// bid_id 从 1 顺序递增；空聚合器/全离线 → 空 Vec。
    pub fn market_bid(&self, market: &MarketData, strategy: &BidStrategy, now_ms: u64) -> Vec<Bid> {
        let mut bids = Vec::new();
        let mut bid_id = 1u64;
        for (rid, r) in self.resources.iter() {
            if !r.online {
                continue;
            }
            let avail = sanitize_available(r.available_mw, r.capacity_mw);
            if avail <= 0.0 {
                continue;
            }
            let quantity = if strategy.max_quantity.is_finite() && strategy.max_quantity > 0.0 {
                avail.min(strategy.max_quantity)
            } else {
                avail
            };
            let price = sanitize_price(market.current_price as f32) + strategy.margin;
            bids.push(Bid {
                bid_id,
                market_type: MarketType::Spot,
                resource_id: *rid,
                price,
                quantity,
                side: BidSide::Sell,
                period: Period::Flat,
                timestamp: now_ms,
            });
            bid_id += 1;
        }
        bids
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_energy_market_agent::MarketSignal;
    use eneros_solver_core::{error::SolverError, problem::LpProblem, result::SolveResult};

    use super::*;

    // ===== RecordingSolver 测试辅助（照搬 domain_optimizer.rs 测试模式）=====
    struct RecordingSolver {
        result: Option<SolveResult>,
        fail: bool,
    }

    impl RecordingSolver {
        fn new() -> Self {
            Self {
                result: None,
                fail: false,
            }
        }
        fn with_result(result: SolveResult) -> Self {
            Self {
                result: Some(result),
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                result: None,
                fail: true,
            }
        }
    }

    impl Solver for RecordingSolver {
        fn solve(
            &mut self,
            _problem: &LpProblem,
            _now_ms: u64,
        ) -> Result<SolveResult, SolverError> {
            if self.fail {
                return Err(SolverError::RunFailed(-1));
            }
            match &self.result {
                Some(r) => Ok(r.clone()),
                None => Ok(SolveResult::optimal(0.0, vec![])),
            }
        }
        fn name(&self) -> &'static str {
            "RecordingSolver"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
        fn set_param(&mut self, _key: &str, _value: &str) -> Result<(), SolverError> {
            Ok(())
        }
        fn status(&self) -> eneros_solver_core::solver::SolverStatus {
            eneros_solver_core::solver::SolverStatus::Idle
        }
    }

    /// 辅助：构造在线资源（ramp=1.0、eff=0.9、Battery）.
    fn res(id: u64, cap: f32, avail: f32) -> VppResource {
        VppResource {
            resource_id: id,
            capacity_mw: cap,
            available_mw: avail,
            ramp_rate: 1.0,
            efficiency: 0.9,
            type_: ResourceType::Battery,
            online: true,
        }
    }

    /// 辅助：构造仅含 current_price 的 MarketData.
    fn market(price: f64) -> MarketData {
        MarketData {
            timestamp: 0,
            price_forecast: Vec::new(),
            current_price: price,
            load_forecast: None,
            signal_type: MarketSignal::RealtimePrice,
        }
    }

    /// 辅助：默认报价策略（margin=5.0 / max_quantity=3.0）.
    fn strategy() -> BidStrategy {
        BidStrategy {
            margin: 5.0,
            max_quantity: 3.0,
            soc_threshold: 0.2,
        }
    }

    // ===== T1: ResourceType 默认 Battery + 4 变体 Debug 非空 =====
    #[test]
    fn t01_resource_type_default_and_debug() {
        assert_eq!(ResourceType::default(), ResourceType::Battery);
        assert!(!format!("{:?}", ResourceType::Battery).is_empty());
        assert!(!format!("{:?}", ResourceType::Pv).is_empty());
        assert!(!format!("{:?}", ResourceType::Load).is_empty());
        assert!(!format!("{:?}", ResourceType::Charger).is_empty());
    }

    // ===== T2: VppResource 字段回显 + Copy =====
    #[test]
    fn t02_vpp_resource_fields_and_copy() {
        let r = VppResource {
            resource_id: 3,
            capacity_mw: 5.0,
            available_mw: 4.0,
            ramp_rate: 1.5,
            efficiency: 0.95,
            type_: ResourceType::Pv,
            online: true,
        };
        assert_eq!(r.resource_id, 3);
        assert_eq!(r.capacity_mw, 5.0);
        assert_eq!(r.available_mw, 4.0);
        assert_eq!(r.ramp_rate, 1.5);
        assert_eq!(r.efficiency, 0.95);
        assert_eq!(r.type_, ResourceType::Pv);
        assert!(r.online);
        let r2 = r; // Copy 语义
        assert_eq!(r, r2);
    }

    // ===== T3: VppProfile Default 全零 =====
    #[test]
    fn t03_vpp_profile_default_zero() {
        let p = VppProfile::default();
        assert_eq!(p.total_capacity_mw, 0.0);
        assert_eq!(p.available_mw, 0.0);
        assert_eq!(p.ramp_up_mw_per_min, 0.0);
        assert_eq!(p.ramp_down_mw_per_min, 0.0);
    }

    // ===== T4: Allocation 字段回显 =====
    #[test]
    fn t04_allocation_fields() {
        let a = Allocation {
            resource_id: 7,
            setpoint_mw: 2.5,
        };
        assert_eq!(a.resource_id, 7);
        assert_eq!(a.setpoint_mw, 2.5);
    }

    // ===== T5: AggregatedDispatch Default =====
    #[test]
    fn t05_aggregated_dispatch_default() {
        let d = AggregatedDispatch::default();
        assert_eq!(d.target_mw, 0.0);
        assert!(d.allocations.is_empty());
        assert_eq!(d.timestamp, 0);
    }

    // ===== T6: VppError 3 变体 Debug + Eq =====
    #[test]
    fn t06_vpp_error_variants() {
        assert_eq!(
            VppError::InsufficientCapacity,
            VppError::InsufficientCapacity
        );
        assert_eq!(VppError::InvalidTarget, VppError::InvalidTarget);
        assert_eq!(VppError::NoResource, VppError::NoResource);
        assert_ne!(VppError::InsufficientCapacity, VppError::InvalidTarget);
        assert_ne!(VppError::InvalidTarget, VppError::NoResource);
        assert_ne!(VppError::InsufficientCapacity, VppError::NoResource);
        assert!(!format!("{:?}", VppError::InsufficientCapacity).is_empty());
        assert!(!format!("{:?}", VppError::InvalidTarget).is_empty());
        assert!(!format!("{:?}", VppError::NoResource).is_empty());
    }

    // ===== T7: new 计数器全零 + 资源空 =====
    #[test]
    fn t07_new_counters_zero() {
        let agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        assert_eq!(agg.aggregate_count, 0);
        assert_eq!(agg.dispatch_count, 0);
        assert_eq!(agg.reject_count, 0);
        assert!(agg.resources.is_empty());
    }

    // ===== T8: add_resource 插入与同 id 覆盖 =====
    #[test]
    fn t08_add_resource_insert_and_overwrite() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        assert_eq!(agg.resources.len(), 1);
        assert_eq!(agg.resources.get(&1).unwrap().capacity_mw, 5.0);
        // 同 id 覆盖
        agg.add_resource(res(1, 8.0, 6.0));
        assert_eq!(agg.resources.len(), 1);
        assert_eq!(agg.resources.get(&1).unwrap().capacity_mw, 8.0);
        assert_eq!(agg.resources.get(&1).unwrap().available_mw, 6.0);
    }

    // ===== T9: remove_resource true/false =====
    #[test]
    fn t09_remove_resource_true_false() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(3, 5.0, 4.0));
        assert!(agg.remove_resource(3));
        assert!(agg.resources.is_empty());
        assert!(!agg.remove_resource(3)); // 已删除 → false
        assert!(!agg.remove_resource(99)); // 不存在 → false
    }

    // ===== T10: set_online/set_available true/false + 离线状态保留 =====
    #[test]
    fn t10_set_online_and_available() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        // set_online 存在 → true，离线后状态保留（D6）
        assert!(agg.set_online(1, false));
        assert!(!agg.resources.get(&1).unwrap().online);
        assert_eq!(agg.resources.get(&1).unwrap().capacity_mw, 5.0);
        assert_eq!(agg.resources.get(&1).unwrap().available_mw, 4.0);
        // 恢复在线
        assert!(agg.set_online(1, true));
        assert!(agg.resources.get(&1).unwrap().online);
        // set_available 存在 → true
        assert!(agg.set_available(1, 2.5));
        assert_eq!(agg.resources.get(&1).unwrap().available_mw, 2.5);
        // 不存在的 id → false
        assert!(!agg.set_online(99, false));
        assert!(!agg.set_available(99, 1.0));
    }

    // ===== T11: 单资源聚合 =====
    #[test]
    fn t11_aggregate_single_resource() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let p = agg.aggregate();
        assert_eq!(p.total_capacity_mw, 5.0);
        assert_eq!(p.available_mw, 4.0);
        assert_eq!(p.ramp_up_mw_per_min, 1.0);
        assert_eq!(p.ramp_down_mw_per_min, 1.0);
    }

    // ===== T12: 3 资源聚合求和（spec 场景）=====
    #[test]
    fn t12_aggregate_three_resources_sum() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        // cap 5/3/2、avail 4/3/2、ramp 1/0.5/0.5
        agg.add_resource(res(1, 5.0, 4.0));
        let mut r2 = res(2, 3.0, 3.0);
        r2.ramp_rate = 0.5;
        agg.add_resource(r2);
        let mut r3 = res(3, 2.0, 2.0);
        r3.ramp_rate = 0.5;
        agg.add_resource(r3);
        let p = agg.aggregate();
        assert_eq!(p.total_capacity_mw, 10.0);
        assert_eq!(p.available_mw, 9.0);
        assert_eq!(p.ramp_up_mw_per_min, 2.0);
        assert_eq!(p.ramp_down_mw_per_min, 2.0);
    }

    // ===== T13: aggregate_count 递增 =====
    #[test]
    fn t13_aggregate_count_increments() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        assert_eq!(agg.aggregate_count, 0);
        agg.aggregate();
        assert_eq!(agg.aggregate_count, 1);
        agg.aggregate();
        assert_eq!(agg.aggregate_count, 2);
    }

    // ===== T14: 空聚合器全零 =====
    #[test]
    fn t14_aggregate_empty_aggregator() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        let p = agg.aggregate();
        assert_eq!(p, VppProfile::default());
        assert_eq!(agg.aggregate_count, 1);
    }

    // ===== T15: 全离线聚合全零 =====
    #[test]
    fn t15_aggregate_all_offline_zero() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        agg.add_resource(res(2, 3.0, 3.0));
        assert!(agg.set_online(1, false));
        assert!(agg.set_online(2, false));
        let p = agg.aggregate();
        assert_eq!(p, VppProfile::default());
    }

    // ===== T16: set_online(false) 后重算排除（spec 场景）=====
    #[test]
    fn t16_aggregate_recalc_after_offline() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let mut r2 = res(2, 3.0, 3.0);
        r2.ramp_rate = 0.5;
        agg.add_resource(r2);
        let mut r3 = res(3, 2.0, 2.0);
        r3.ramp_rate = 0.5;
        agg.add_resource(r3);
        // 资源 2 离线 → total 7.0 / avail 6.0 / ramp 1.5
        assert!(agg.set_online(2, false));
        let p = agg.aggregate();
        assert_eq!(p.total_capacity_mw, 7.0);
        assert_eq!(p.available_mw, 6.0);
        assert_eq!(p.ramp_up_mw_per_min, 1.5);
        assert_eq!(p.ramp_down_mw_per_min, 1.5);
    }

    // ===== T17: dispatch_count 递增 =====
    #[test]
    fn t17_dispatch_count_increments() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let m = market(1.0);
        assert_eq!(agg.dispatch_count, 0);
        let _ = agg.dispatch(&m, 1.0, 1000);
        assert_eq!(agg.dispatch_count, 1);
        let _ = agg.dispatch(&m, 1.0, 2000);
        assert_eq!(agg.dispatch_count, 2);
    }

    // ===== T18: NaN target → InvalidTarget + reject_count+1 =====
    #[test]
    fn t18_dispatch_nan_target_invalid() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let err = agg.dispatch(&market(1.0), f32::NAN, 1000).unwrap_err();
        assert_eq!(err, VppError::InvalidTarget);
        assert_eq!(agg.dispatch_count, 1);
        assert_eq!(agg.reject_count, 1);
    }

    // ===== T19: ±Inf target → InvalidTarget =====
    #[test]
    fn t19_dispatch_infinite_target_invalid() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let m = market(1.0);
        assert_eq!(
            agg.dispatch(&m, f32::INFINITY, 1000).unwrap_err(),
            VppError::InvalidTarget
        );
        assert_eq!(
            agg.dispatch(&m, f32::NEG_INFINITY, 1000).unwrap_err(),
            VppError::InvalidTarget
        );
        assert_eq!(agg.reject_count, 2);
    }

    // ===== T20: |target| > available → InsufficientCapacity（含负 target abs）=====
    #[test]
    fn t20_dispatch_insufficient_capacity() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        agg.add_resource(res(2, 5.0, 5.0));
        let m = market(1.0);
        // 总 available = 9.0，target=10.0 → 拒绝
        assert_eq!(
            agg.dispatch(&m, 10.0, 1000).unwrap_err(),
            VppError::InsufficientCapacity
        );
        // 负 target 充电场景 abs 判定：-10.0 同样拒绝
        assert_eq!(
            agg.dispatch(&m, -10.0, 1000).unwrap_err(),
            VppError::InsufficientCapacity
        );
        assert_eq!(agg.reject_count, 2);
        assert_eq!(agg.dispatch_count, 2);
    }

    // ===== T21: 2 资源 Optimal → allocations 升序 + timestamp + target 回显 =====
    #[test]
    fn t21_dispatch_two_resource_optimal() {
        // spec 场景：r1 avail 6.0 eff 0.95 / r2 avail 4.0 eff 0.75，target=8.0
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.8, vec![6.0, 2.0]),
        )));
        let mut r1 = res(1, 6.0, 6.0);
        r1.efficiency = 0.95;
        agg.add_resource(r1);
        let mut r2 = res(2, 4.0, 4.0);
        r2.efficiency = 0.75;
        agg.add_resource(r2);
        let d = agg.dispatch(&market(2.0), 8.0, 5000).unwrap();
        assert_eq!(d.allocations.len(), 2);
        // resource_id 升序
        assert_eq!(d.allocations[0].resource_id, 1);
        assert_eq!(d.allocations[0].setpoint_mw, 6.0);
        assert_eq!(d.allocations[1].resource_id, 2);
        assert_eq!(d.allocations[1].setpoint_mw, 2.0);
        assert_eq!(d.timestamp, 5000);
        assert_eq!(d.target_mw, 8.0);
        assert_eq!(agg.dispatch_count, 1);
        assert_eq!(agg.reject_count, 0);
    }

    // ===== T22: solver Err → 容量比例兜底仍 Ok（r1=6.0 r2=4.0）=====
    #[test]
    fn t22_dispatch_solver_err_fallback() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        agg.add_resource(res(1, 6.0, 6.0));
        agg.add_resource(res(2, 4.0, 4.0));
        let d = agg.dispatch(&market(1.0), 10.0, 1000).unwrap();
        assert_eq!(d.allocations.len(), 2);
        assert_eq!(d.allocations[0].resource_id, 1);
        assert_eq!(d.allocations[0].setpoint_mw, 6.0);
        assert_eq!(d.allocations[1].resource_id, 2);
        assert_eq!(d.allocations[1].setpoint_mw, 4.0);
        assert_eq!(agg.optimizer.fallback_count, 1);
    }

    // ===== T23: solver 解长度不符 → 兜底 =====
    #[test]
    fn t23_dispatch_solution_len_mismatch_fallback() {
        // Optimal 但解长度 2 vs 实际 1 列 → 兜底
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.0, vec![1.0, 2.0]),
        )));
        agg.add_resource(res(1, 6.0, 6.0));
        let d = agg.dispatch(&market(1.0), 3.0, 1000).unwrap();
        assert_eq!(agg.optimizer.fallback_count, 1);
        assert_eq!(d.allocations.len(), 1);
        assert_eq!(d.allocations[0].resource_id, 1);
        assert_eq!(d.allocations[0].setpoint_mw, 3.0);
    }

    // ===== T24: 空聚合器 dispatch → Err(NoResource) + reject =====
    #[test]
    fn t24_dispatch_empty_aggregator_no_resource() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        // 空聚合器 available=0，target=0 通过容量校验 → EmptyDomain → NoResource
        let err = agg.dispatch(&market(1.0), 0.0, 1000).unwrap_err();
        assert_eq!(err, VppError::NoResource);
        assert_eq!(agg.dispatch_count, 1);
        assert_eq!(agg.reject_count, 1);
    }

    // ===== T25: 全离线 → Err(NoResource) =====
    #[test]
    fn t25_dispatch_all_offline_no_resource() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        assert!(agg.set_online(1, false));
        let err = agg.dispatch(&market(1.0), 0.0, 1000).unwrap_err();
        assert_eq!(err, VppError::NoResource);
        assert_eq!(agg.reject_count, 1);
        // 状态保留未删除（D6）
        assert!(agg.resources.contains_key(&1));
        assert!(!agg.resources.get(&1).unwrap().online);
    }

    // ===== T26: EmptyDomain → NoResource 映射（容量无效资源）=====
    #[test]
    fn t26_dispatch_empty_domain_maps_no_resource() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        // capacity NaN → 聚合排除（available=0）且 sync_boxes 不写入 → EmptyDomain
        let mut r = res(1, 5.0, 4.0);
        r.capacity_mw = f32::NAN;
        agg.add_resource(r);
        let err = agg.dispatch(&market(1.0), 0.0, 1000).unwrap_err();
        assert_eq!(err, VppError::NoResource);
        assert_eq!(agg.reject_count, 1);
        assert_eq!(agg.optimizer.empty_count, 1);
    }

    // ===== T27: 兜底路径字段不 panic 且有限 =====
    #[test]
    fn t27_dispatch_fallback_fields_finite() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        agg.add_resource(res(1, 6.0, 6.0));
        agg.add_resource(res(2, 4.0, 4.0));
        let d = agg.dispatch(&market(1.0), 10.0, 7000).unwrap();
        assert_eq!(d.timestamp, 7000);
        assert_eq!(d.target_mw, 10.0);
        for a in &d.allocations {
            assert!(a.setpoint_mw.is_finite());
        }
        let total: f32 = d.allocations.iter().map(|a| a.setpoint_mw).sum();
        assert_eq!(total, 10.0);
    }

    // ===== T28: target == available 边界恰好接受 =====
    #[test]
    fn t28_dispatch_target_equal_available_accepted() {
        // |target| == available（不 >）→ 不拒绝，兜底各分满
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        agg.add_resource(res(1, 6.0, 6.0));
        agg.add_resource(res(2, 4.0, 4.0));
        let d = agg.dispatch(&market(1.0), 10.0, 1000).unwrap();
        assert_eq!(agg.reject_count, 0);
        assert_eq!(d.allocations[0].setpoint_mw, 6.0);
        assert_eq!(d.allocations[1].setpoint_mw, 4.0);
    }

    // ===== T29: dispatch 后离线再 dispatch → allocations 不含该资源 =====
    #[test]
    fn t29_offline_excluded_after_redispatch() {
        // 3 在线资源 avail 均 3.0，target=6.0，failing solver 走兜底
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        for id in 1..=3u64 {
            agg.add_resource(res(id, 3.0, 3.0));
        }
        let m = market(1.0);
        let d1 = agg.dispatch(&m, 6.0, 1000).unwrap();
        assert_eq!(d1.allocations.len(), 3);
        // 各分 6 × 3/9 = 2.0
        assert_eq!(d1.allocations[0].setpoint_mw, 2.0);
        // 资源 2 离线 → 重分配不含资源 2，target 全分给 1/3（各 3.0）
        assert!(agg.set_online(2, false));
        let d2 = agg.dispatch(&m, 6.0, 2000).unwrap();
        assert_eq!(d2.allocations.len(), 2);
        assert!(d2.allocations.iter().all(|a| a.resource_id != 2));
        assert_eq!(d2.allocations[0].resource_id, 1);
        assert_eq!(d2.allocations[0].setpoint_mw, 3.0);
        assert_eq!(d2.allocations[1].resource_id, 3);
        assert_eq!(d2.allocations[1].setpoint_mw, 3.0);
    }

    // ===== T30: 恢复 online 后重新纳入 =====
    #[test]
    fn t30_online_restore_reincluded() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        for id in 1..=3u64 {
            agg.add_resource(res(id, 3.0, 3.0));
        }
        let m = market(1.0);
        assert!(agg.set_online(2, false));
        let d1 = agg.dispatch(&m, 6.0, 1000).unwrap();
        assert!(d1.allocations.iter().all(|a| a.resource_id != 2));
        // 恢复在线 → 重新纳入（各 2.0）
        assert!(agg.set_online(2, true));
        let d2 = agg.dispatch(&m, 6.0, 2000).unwrap();
        assert_eq!(d2.allocations.len(), 3);
        assert!(d2.allocations.iter().any(|a| a.resource_id == 2));
        assert_eq!(d2.allocations[1].setpoint_mw, 2.0);
    }

    // ===== T31: set_available 调额后 dispatch 尊重新额度 =====
    #[test]
    fn t31_set_available_respected_in_dispatch() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        agg.add_resource(res(1, 6.0, 6.0));
        agg.add_resource(res(2, 4.0, 4.0));
        // 调额：资源 1 available 6.0 → 3.0
        assert!(agg.set_available(1, 3.0));
        // 总 available 3+4=7，target=7 → 兜底 r1=3.0 r2=4.0
        let d = agg.dispatch(&market(1.0), 7.0, 1000).unwrap();
        assert_eq!(d.allocations[0].resource_id, 1);
        assert_eq!(d.allocations[0].setpoint_mw, 3.0);
        assert_eq!(d.allocations[1].resource_id, 2);
        assert_eq!(d.allocations[1].setpoint_mw, 4.0);
    }

    // ===== T32: sync_boxes 排除离线（edge_boxes 长度断言）=====
    #[test]
    fn t32_sync_boxes_excludes_offline() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        for id in 1..=3u64 {
            agg.add_resource(res(id, 3.0, 3.0));
        }
        assert!(agg.set_online(2, false));
        let _ = agg.dispatch(&market(1.0), 6.0, 1000).unwrap();
        // sync_boxes 后 optimizer 仅含在线资源 1/3
        assert_eq!(agg.optimizer.edge_boxes.len(), 2);
        assert!(agg.optimizer.edge_boxes.contains_key(&1));
        assert!(!agg.optimizer.edge_boxes.contains_key(&2));
        assert!(agg.optimizer.edge_boxes.contains_key(&3));
        // 恢复后再次 dispatch → 3 个 box
        assert!(agg.set_online(2, true));
        let _ = agg.dispatch(&market(1.0), 6.0, 2000).unwrap();
        assert_eq!(agg.optimizer.edge_boxes.len(), 3);
    }

    // ===== T33: 2 资源报价（max_quantity clamp + price + bid_id 递增）=====
    #[test]
    fn t33_market_bid_two_resources() {
        // spec 场景：avail 4.0/6.0，margin 5.0，max_quantity 3.0，price=400.0
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        let mut r1 = res(1, 5.0, 4.0);
        r1.ramp_rate = 0.5;
        agg.add_resource(r1);
        agg.add_resource(res(2, 6.0, 6.0));
        let bids = agg.market_bid(&market(400.0), &strategy(), 9000);
        assert_eq!(bids.len(), 2);
        // bid_id 从 1 顺序递增，quantity = min(avail, 3.0) = 3.0
        assert_eq!(bids[0].bid_id, 1);
        assert_eq!(bids[0].resource_id, 1);
        assert_eq!(bids[0].quantity, 3.0);
        assert_eq!(bids[0].price, 405.0);
        assert_eq!(bids[1].bid_id, 2);
        assert_eq!(bids[1].resource_id, 2);
        assert_eq!(bids[1].quantity, 3.0);
        assert_eq!(bids[1].price, 405.0);
        // 固定 Spot / Sell / Flat / timestamp 回显
        for b in &bids {
            assert_eq!(b.market_type, MarketType::Spot);
            assert_eq!(b.side, BidSide::Sell);
            assert_eq!(b.period, Period::Flat);
            assert_eq!(b.timestamp, 9000);
        }
    }

    // ===== T34: 跳过 available ≤ 0 的资源 =====
    #[test]
    fn t34_market_bid_skips_zero_available() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 0.0)); // available 0 → 跳过
        agg.add_resource(res(2, 5.0, 4.0));
        let bids = agg.market_bid(&market(400.0), &strategy(), 1000);
        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].resource_id, 2);
        assert_eq!(bids[0].bid_id, 1);
    }

    // ===== T35: 跳过离线资源 =====
    #[test]
    fn t35_market_bid_skips_offline() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        agg.add_resource(res(2, 5.0, 4.0));
        assert!(agg.set_online(1, false));
        let bids = agg.market_bid(&market(400.0), &strategy(), 1000);
        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].resource_id, 2);
    }

    // ===== T36: max_quantity ≤ 0 → 全额 available =====
    #[test]
    fn t36_market_bid_max_quantity_nonpositive_full() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let s = BidStrategy {
            margin: 5.0,
            max_quantity: 0.0, // ≤ 0 → 按 available 全额
            soc_threshold: 0.2,
        };
        let bids = agg.market_bid(&market(400.0), &s, 1000);
        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].quantity, 4.0);
        // 非有限 max_quantity → 同样全额
        let s_nan = BidStrategy {
            margin: 5.0,
            max_quantity: f32::NAN,
            soc_threshold: 0.2,
        };
        let bids2 = agg.market_bid(&market(400.0), &s_nan, 1000);
        assert_eq!(bids2[0].quantity, 4.0);
    }

    // ===== T37: current_price NaN → price = 0 + margin =====
    #[test]
    fn t37_market_bid_nan_price() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        agg.add_resource(res(1, 5.0, 4.0));
        let bids = agg.market_bid(&market(f64::NAN), &strategy(), 1000);
        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].price, 5.0); // 0.0 + margin 5.0
        assert!(bids[0].price.is_finite());
    }

    // ===== T38: 空聚合器 → 空 Vec + market_bid 不更新计数器 =====
    #[test]
    fn t38_market_bid_empty_and_no_counters() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::new()));
        let bids = agg.market_bid(&market(400.0), &strategy(), 1000);
        assert!(bids.is_empty());
        assert_eq!(agg.aggregate_count, 0);
        assert_eq!(agg.dispatch_count, 0);
        assert_eq!(agg.reject_count, 0);
        // 全离线 → 空 Vec，同样不计数
        agg.add_resource(res(1, 5.0, 4.0));
        assert!(agg.set_online(1, false));
        assert!(agg.market_bid(&market(400.0), &strategy(), 1000).is_empty());
        assert_eq!(agg.aggregate_count, 0);
        assert_eq!(agg.dispatch_count, 0);
        assert_eq!(agg.reject_count, 0);
    }

    // ===== T39: 5 资源集成（aggregate + dispatch + market_bid 全链路）=====
    #[test]
    fn t39_five_resource_integration() {
        // 5 资源：cap/avail 均 2.0，ramp 0.5，eff 递降，混合类型，资源 5 离线
        let types = [
            ResourceType::Battery,
            ResourceType::Pv,
            ResourceType::Load,
            ResourceType::Charger,
            ResourceType::Battery,
        ];
        let effs = [0.95f32, 0.90, 0.85, 0.80, 0.75];
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.6, vec![2.0, 2.0, 2.0, 0.0]),
        )));
        for i in 0..5usize {
            let id = (i + 1) as u64;
            let mut r = res(id, 2.0, 2.0);
            r.ramp_rate = 0.5;
            r.efficiency = effs[i];
            r.type_ = types[i];
            agg.add_resource(r);
        }
        assert!(agg.set_online(5, false));
        // aggregate：4 在线 → total 8 / avail 8 / ramp 2.0
        let p = agg.aggregate();
        assert_eq!(p.total_capacity_mw, 8.0);
        assert_eq!(p.available_mw, 8.0);
        assert_eq!(p.ramp_up_mw_per_min, 2.0);
        assert_eq!(agg.aggregate_count, 1);
        // dispatch target=6 → 4 项 allocations（Optimal 解 [2,2,2,0]）
        let d = agg.dispatch(&market(2.0), 6.0, 5000).unwrap();
        assert_eq!(d.allocations.len(), 4);
        assert!(d.allocations.iter().all(|a| a.resource_id != 5));
        assert_eq!(d.allocations[0].setpoint_mw, 2.0);
        assert_eq!(d.allocations[3].setpoint_mw, 0.0);
        assert_eq!(d.timestamp, 5000);
        assert_eq!(agg.dispatch_count, 1);
        assert_eq!(agg.reject_count, 0);
        // market_bid：4 条报价（资源 5 离线跳过）
        let s = BidStrategy {
            margin: 5.0,
            max_quantity: 10.0,
            soc_threshold: 0.2,
        };
        let bids = agg.market_bid(&market(400.0), &s, 6000);
        assert_eq!(bids.len(), 4);
        for (i, b) in bids.iter().enumerate() {
            assert_eq!(b.bid_id, (i + 1) as u64);
            assert_eq!(b.resource_id, (i + 1) as u64);
            assert_eq!(b.quantity, 2.0);
            assert_eq!(b.price, 405.0);
            assert_eq!(b.side, BidSide::Sell);
            assert_eq!(b.timestamp, 6000);
        }
    }

    // ===== T40: NaN 风暴（混合非法输入不 panic，输出有限）=====
    #[test]
    fn t40_nan_storm_no_panic_finite_output() {
        let mut agg = VppAggregator::new(Box::new(RecordingSolver::failing()));
        // r1：capacity NaN → 聚合/分配/报价全排除
        let mut r1 = res(1, 5.0, 4.0);
        r1.capacity_mw = f32::NAN;
        agg.add_resource(r1);
        // r2：capacity -1 → 排除
        let r2 = res(2, -1.0, 4.0);
        agg.add_resource(r2);
        // r3：capacity +Inf → 排除
        let r3 = res(3, f32::INFINITY, 4.0);
        agg.add_resource(r3);
        // r4：available NaN → 按 0；ramp NaN → 0；efficiency NaN → 0.5
        let mut r4 = res(4, 5.0, f32::NAN);
        r4.ramp_rate = f32::NAN;
        r4.efficiency = f32::NAN;
        agg.add_resource(r4);
        // r5：available +Inf → 0；ramp -1 → 0；唯一有效出力资源改为 r6
        let mut r5 = res(5, 5.0, f32::INFINITY);
        r5.ramp_rate = -1.0;
        agg.add_resource(r5);
        // r6：唯一有效资源（cap 5 / avail 4）
        agg.add_resource(res(6, 5.0, 4.0));
        // aggregate：仅 r4(avail→0)/r5(avail→0)/r6(4.0) 计入 total
        let p = agg.aggregate();
        assert!(p.total_capacity_mw.is_finite());
        assert!(p.available_mw.is_finite());
        assert!(p.ramp_up_mw_per_min.is_finite());
        assert_eq!(p.total_capacity_mw, 15.0); // r4 5 + r5 5 + r6 5
        assert_eq!(p.available_mw, 4.0); // 仅 r6
        assert_eq!(p.ramp_up_mw_per_min, 1.0); // 仅 r6（r4/r5 ramp sanitize 为 0）
                                               // dispatch：target=4.0 走兜底，仅 r6 参与 LP（r4/r5 capacity_mw=0 被 build_domain_lp 排除），输出有限
        let d = agg.dispatch(&market(f64::NAN), 4.0, 1000).unwrap();
        assert_eq!(d.allocations.len(), 1); // 仅 r6
        assert_eq!(d.allocations[0].resource_id, 6);
        assert_eq!(d.allocations[0].setpoint_mw, 4.0);
        for a in &d.allocations {
            assert!(a.setpoint_mw.is_finite());
        }
        let total: f32 = d.allocations.iter().map(|a| a.setpoint_mw).sum();
        assert_eq!(total, 4.0);
        // market_bid：price NaN → 0+margin；r4/r5 available≤0 跳过；仅 r6
        let bids = agg.market_bid(&market(f64::NAN), &strategy(), 2000);
        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].resource_id, 6);
        assert!(bids[0].price.is_finite());
        assert_eq!(bids[0].price, 5.0);
        assert!(bids[0].quantity.is_finite());
        // 计数器路径：1 aggregate + 1 dispatch + 0 reject
        assert_eq!(agg.aggregate_count, 1);
        assert_eq!(agg.dispatch_count, 1);
        assert_eq!(agg.reject_count, 0);
    }
}
