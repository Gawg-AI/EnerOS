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
//! | **D7** | `VppResource` 增加 `efficiency: f32`（复用的 DomainOptimizer 损耗最小目标可区分高效/低效 DER，否则 LP 目标退化为常数；NaN→0.5 clamp `[0,1]`，v0.93.0 D12 一致） |
//! | **D8** | 复用 v0.93.0 `DomainOptimizer`：sync_boxes 将每在线资源映射为单设备 box（box_id=device_id=resource_id，`p_min=0`、`p_max=box capacity=available_mw`，`soc=1.0` 恒通过合格过滤）；"分配失败 → 重新优化"落地为 optimizer 内建容量比例兜底（不迭代重试 LP，v0.93.0 D10 惯例） |
//! | **D9** | 3 个 pub 计数器 `aggregate_count`/`dispatch_count`/`reject_count`（拒绝 = InvalidTarget + InsufficientCapacity + NoResource 三路合计；聚合容量经 `VppProfile` 可观测） |
//! | **D10** | 资源不足落地为**拒绝**（确定性）：`target_mw.abs() > profile.available_mw` → `Err(VppError::InsufficientCapacity)`（含负 target 充电场景，abs 判定） |
//! | **D11** | `ramp_down = ramp_up` 对称实现（蓝图关键代码一致）；`ramp_rate` 非有限或 <0 → 按 0 计入 profile（ramp 仅上报不参与分配，不阻断调度） |
//! | **D12** | NaN 防御（v0.88.0 C140 / v0.93.0 D12 教训）：capacity 非有限或 ≤0 → 资源从聚合排除；available 非有限 → 0 且 clamp [0, capacity]；efficiency NaN→0.5 clamp `[0,1]`；price 非有限 → bid price 按 0+margin。复用 domain_optimizer 的 `sanitize_capacity`/`sanitize_efficiency`/`sanitize_price`（可见性放宽为 `pub(crate)`，零逻辑改动） |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_energy_market_agent::{
    BidStrategy, DeviceCapability, DevicePool, MarketData, MarketType, Period, ResourceBid,
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
    /// 转换效率（0~1；NaN → 0.5 中性 clamp `[0,1]`，D7/D12）.
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

}

/// VPP 聚合/调度大方法（ARCH-G03 拆分）。
mod dispatch;

#[cfg(test)]
mod tests;
