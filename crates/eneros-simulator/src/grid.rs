//! 电网模拟器
//!
//! 整合 [`PowerNetwork`] 潮流求解与暂态稳定仿真，提供统一的电网时域模拟接口。
//!
//! ## 功能
//!
//! - 稳态潮流模式：每个时间步重新求解潮流
//! - 暂态稳定模式：预留接口，当前按稳态求解（完整暂态积分待后续实现）
//! - 支持开关动作（开合支路）、发电机调整、负荷调整、区域负荷切除
//! - 提供 [`GridState`] 状态快照，包含母线电压、相角、支路功率、开关状态
//!
//! ## 设计说明
//!
//! [`GridSimulator`] 内部持有不可变的基础电网模型（`base_network`）和当前生效的
//! 电网模型（`network`）。所有动作通过修改覆盖表（`p_spec_overrides`、
//! `q_spec_overrides`、`opened_branch_ids`）记录，并在 `rebuild_network` 中从
//! 基础模型重建网络。这避免了 `PowerNetwork` 不支持 "恢复支路" 的限制，
//! 使 [`GridAction::CloseBranch`] 能够正确实现。
//!
//! 逻辑参考 [`eneros_network::NetworkSimulatorAdapter`]，但 [`GridSimulator`]
//! 独立实现，因为：
//! - `NetworkSimulatorAdapter` 包装 `Arc<RwLock<PowerNetwork>>` 用于 What-If 分析，
//!   而 `GridSimulator` 拥有网络所有权用于时域仿真
//! - `GridSimulator` 需要跟踪状态随时间的演化，`NetworkSimulatorAdapter` 是无状态查询

use crate::scenario::ScenarioAction;
use eneros_core::{ElementId, EnerOSError, Result};
use eneros_network::PowerNetwork;
use eneros_powerflow::{BusTypeNR, PowerFlowResult};
use std::collections::HashMap;

/// 默认系统频率（Hz）
const DEFAULT_FREQUENCY: f64 = 50.0;

/// 电网模拟器
///
/// 整合潮流求解与时序动作执行，支持稳态和暂态两种仿真模式。
pub struct GridSimulator {
    /// 基础电网模型（不可变，用于重建网络）
    base_network: PowerNetwork,
    /// 当前生效的电网模型（应用所有动作后）
    network: PowerNetwork,
    /// 已断开的支路 ID 列表
    opened_branch_ids: Vec<ElementId>,
    /// 有功注入覆盖表（bus_idx → p_pu）
    p_spec_overrides: HashMap<usize, f64>,
    /// 无功注入覆盖表（bus_idx → q_pu）
    q_spec_overrides: HashMap<usize, f64>,
    /// 当前时间（秒）
    current_time: f64,
    /// 当前状态快照
    current_state: GridState,
    /// 仿真模式
    mode: SimulationMode,
    /// 系统频率（Hz）
    frequency: f64,
    /// 系统基准容量（MVA），从 `PowerNetwork` 的 Y-Bus 矩阵读取。
    /// 用于 MW↔标幺值转换，避免硬编码 100 MVA 导致非标准基准网络转换出错。
    base_mva: f64,
}

/// 仿真模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationMode {
    /// 稳态潮流模式：每个时间步重新求解潮流
    SteadyState,
    /// 暂态稳定模式：积分发电机动态方程
    ///
    /// 注意：当前实现按稳态求解，完整暂态积分待后续实现。
    Transient,
}

/// 电网状态快照
#[derive(Debug, Clone, Default)]
pub struct GridState {
    /// 各母线电压幅值（标幺值）
    pub voltages: HashMap<ElementId, f64>,
    /// 各母线电压相角（弧度）
    pub angles: HashMap<ElementId, f64>,
    /// 各支路有功功率（MW，from 端）
    pub branch_power: HashMap<ElementId, f64>,
    /// 系统频率（Hz）
    pub frequency: f64,
    /// 开关状态（true=闭合，false=断开）
    pub switch_states: HashMap<ElementId, bool>,
}

/// 电网动作
#[derive(Debug, Clone)]
pub enum GridAction {
    /// 打开支路（开关分闸）
    OpenBranch { branch_id: ElementId },
    /// 闭合支路（开关合闸）
    CloseBranch { branch_id: ElementId },
    /// 调整发电机出力
    AdjustGenerator {
        gen_id: ElementId,
        p_mw: f64,
        q_mvar: f64,
    },
    /// 调整负荷（load_id 解释为母线 ID）
    AdjustLoad {
        load_id: ElementId,
        p_mw: f64,
        q_mvar: f64,
    },
    /// 按比例切除区域负荷
    ShedLoad { zone_id: u32, percentage: f64 },
}

impl GridSimulator {
    /// 创建电网模拟器
    ///
    /// 以给定电网模型为基础，求解初始潮流并生成初始状态快照。
    pub fn new(network: PowerNetwork) -> Self {
        // 用 with_modifications(None, None) 创建工作副本（PowerNetwork 未派生 Clone）
        let working = network.with_modifications(None, None);
        // 从网络的 Y-Bus 矩阵读取系统基准容量，避免硬编码
        let base_mva = network.ybus().base_mva();
        let mut sim = Self {
            base_network: network,
            network: working,
            opened_branch_ids: Vec::new(),
            p_spec_overrides: HashMap::new(),
            q_spec_overrides: HashMap::new(),
            current_time: 0.0,
            current_state: GridState::default(),
            mode: SimulationMode::SteadyState,
            frequency: DEFAULT_FREQUENCY,
            base_mva,
        };
        // 初始化开关状态：所有支路默认闭合
        for &branch_id in sim.base_network.branch_ids() {
            sim.current_state.switch_states.insert(branch_id, true);
        }
        // 设置初始频率（即使潮流失败也能读到合理值）
        sim.current_state.frequency = sim.frequency;
        // 求解初始潮流（失败时记录警告，保持默认状态）
        if let Err(e) = sim.solve_power_flow() {
            tracing::warn!("GridSimulator 初始潮流求解失败: {}", e);
        }
        sim
    }

    /// 设置仿真模式（builder 风格）
    pub fn with_mode(mut self, mode: SimulationMode) -> Self {
        self.mode = mode;
        self
    }

    /// 推进一个时间步
    ///
    /// - 稳态模式：重求解潮流
    /// - 暂态模式：当前按稳态求解（完整暂态积分待后续实现）
    pub fn step(&mut self, dt: f64) -> Result<()> {
        if dt < 0.0 {
            return Err(EnerOSError::PowerFlow(format!(
                "时间步长 dt={} 不能为负",
                dt
            )));
        }
        self.current_time += dt;
        self.solve_power_flow()
    }

    /// 应用电网动作
    ///
    /// 根据动作类型修改网络拓扑或注入参数，然后重建网络并重求解潮流。
    ///
    /// 状态一致性保证：若 `solve_power_flow` 失败，会回滚 `opened_branch_ids`、
    /// `p_spec_overrides`、`q_spec_overrides` 并重建网络，使内部状态与动作应用前
    /// 完全一致。`switch_states` 仅在潮流成功后更新，避免与电压/相角状态不同步。
    pub fn apply_action(&mut self, action: &GridAction) -> Result<()> {
        // 快照当前覆盖状态，用于 solve_power_flow 失败时回滚
        let saved_opened_branch_ids = self.opened_branch_ids.clone();
        let saved_p_spec_overrides = self.p_spec_overrides.clone();
        let saved_q_spec_overrides = self.q_spec_overrides.clone();

        match action {
            GridAction::OpenBranch { branch_id } => {
                // 校验支路存在
                if find_branch_index(&self.base_network, *branch_id).is_none() {
                    return Err(EnerOSError::PowerFlow(format!(
                        "支路 {} 未找到",
                        branch_id
                    )));
                }
                if !self.opened_branch_ids.contains(branch_id) {
                    self.opened_branch_ids.push(*branch_id);
                }
            }
            GridAction::CloseBranch { branch_id } => {
                // 校验支路存在（与 OpenBranch 一致，避免对不存在的支路静默无操作）
                if find_branch_index(&self.base_network, *branch_id).is_none() {
                    return Err(EnerOSError::PowerFlow(format!(
                        "支路 {} 未找到",
                        branch_id
                    )));
                }
                self.opened_branch_ids.retain(|&id| id != *branch_id);
            }
            GridAction::AdjustGenerator {
                gen_id,
                p_mw,
                q_mvar,
            } => {
                self.apply_generator_adjustment(*gen_id, *p_mw, *q_mvar)?;
            }
            GridAction::AdjustLoad {
                load_id,
                p_mw,
                q_mvar,
            } => {
                self.apply_load_adjustment(*load_id, *p_mw, *q_mvar)?;
            }
            GridAction::ShedLoad {
                zone_id,
                percentage,
            } => {
                self.apply_load_shedding(*zone_id, *percentage)?;
            }
        }

        // 重建网络（仅更新 self.network，不更新 switch_states）
        self.rebuild_network();

        // 求解潮流；失败时回滚覆盖状态并重建网络，保证状态一致性。
        // 注意：solve_power_flow 失败时不会更新 current_state 的电压/相角/支路功率，
        // 因此回滚后 current_state 与失败前完全一致。
        if let Err(e) = self.solve_power_flow() {
            self.opened_branch_ids = saved_opened_branch_ids;
            self.p_spec_overrides = saved_p_spec_overrides;
            self.q_spec_overrides = saved_q_spec_overrides;
            self.rebuild_network();
            return Err(e);
        }

        // 潮流成功后更新开关状态，确保 switch_states 与电压/相角来自同一潮流解
        for &branch_id in self.base_network.branch_ids() {
            let is_closed = !self.opened_branch_ids.contains(&branch_id);
            self.current_state
                .switch_states
                .insert(branch_id, is_closed);
        }

        Ok(())
    }

    /// 获取当前状态快照
    pub fn snapshot(&self) -> &GridState {
        &self.current_state
    }

    /// 获取当前仿真时间（秒）
    pub fn current_time(&self) -> f64 {
        self.current_time
    }

    /// 获取仿真模式
    pub fn mode(&self) -> SimulationMode {
        self.mode
    }

    /// 获取系统基准容量（MVA）
    pub fn base_mva(&self) -> f64 {
        self.base_mva
    }

    /// 获取当前电网模型引用
    pub fn network(&self) -> &PowerNetwork {
        &self.network
    }

    /// 求解潮流并更新状态快照
    fn solve_power_flow(&mut self) -> Result<()> {
        let result = self.network.solve()?;
        self.update_state_from_result(&result);
        Ok(())
    }

    /// 从潮流结果更新状态快照
    fn update_state_from_result(&mut self, result: &PowerFlowResult) {
        self.current_state.voltages.clear();
        self.current_state.angles.clear();
        self.current_state.branch_power.clear();
        self.current_state.frequency = self.frequency;

        // 构建母线索引 → 母线 ID 的反向映射
        let bus_map = self.network.bus_map();
        let mut idx_to_id: Vec<ElementId> = vec![0; bus_map.len()];
        for (&bus_id, &idx) in bus_map {
            if idx < idx_to_id.len() {
                idx_to_id[idx] = bus_id;
            }
        }

        for bus_result in &result.bus_results {
            let bus_id = idx_to_id
                .get(bus_result.bus_id as usize)
                .copied()
                .unwrap_or(bus_result.bus_id);
            self.current_state
                .voltages
                .insert(bus_id, bus_result.voltage_magnitude);
            self.current_state
                .angles
                .insert(bus_id, bus_result.voltage_angle);
        }

        // 构建母线对 → 真实支路 ID 的映射（双向，处理支路定义方向与求解器
        // 迭代方向相反的情况）。求解器中 BranchResult.branch_id 是合成的
        // `i * n + j`（母线索引对），不是 PowerNetwork::branch_ids() 返回
        // 的真实支路 ID，因此需要通过母线对反查。
        let mut branch_pair_to_id: HashMap<(ElementId, ElementId), ElementId> = HashMap::new();
        let branches = self.network.branches_data();
        let branch_ids = self.network.branch_ids();
        for (k, br) in branches.iter().enumerate() {
            let (from_id, to_id) = (br.0, br.1);
            if let Some(&bid) = branch_ids.get(k) {
                branch_pair_to_id.insert((from_id, to_id), bid);
                branch_pair_to_id.insert((to_id, from_id), bid);
            }
        }

        for branch_result in &result.branch_results {
            // BranchResult.from_bus / to_bus 是母线索引，需转换为母线 ID
            let from_bus_id = idx_to_id
                .get(branch_result.from_bus as usize)
                .copied()
                .unwrap_or(branch_result.from_bus);
            let to_bus_id = idx_to_id
                .get(branch_result.to_bus as usize)
                .copied()
                .unwrap_or(branch_result.to_bus);

            // 通过母线对反查真实支路 ID
            if let Some(&real_branch_id) = branch_pair_to_id.get(&(from_bus_id, to_bus_id)) {
                // 求解器输出的 p_from 实际是标幺值（基于 per-unit 电压和导纳计算），
                // 乘以 base_mva 转换为 MW，使 branch_power 的 "MW" 文档标注名副其实
                let p_mw = branch_result.p_from * self.base_mva;
                self.current_state
                    .branch_power
                    .insert(real_branch_id, p_mw);
            }
        }
    }

    /// 从基础网络重建当前网络（应用所有覆盖和断开支路）
    fn rebuild_network(&mut self) {
        // 1. 应用 p_spec / q_spec 覆盖
        let has_p_overrides = !self.p_spec_overrides.is_empty();
        let has_q_overrides = !self.q_spec_overrides.is_empty();
        let net = if has_p_overrides || has_q_overrides {
            let p_spec = self.build_modified_p_spec(&self.base_network);
            let q_spec = self.build_modified_q_spec(&self.base_network);
            self.base_network
                .with_modifications(Some(p_spec), Some(q_spec))
        } else {
            self.base_network.with_modifications(None, None)
        };

        // 2. 断开支路
        let net = if !self.opened_branch_ids.is_empty() {
            let open_indices: Vec<usize> = self
                .opened_branch_ids
                .iter()
                .filter_map(|&bid| find_branch_index(&self.base_network, bid))
                .collect();
            if open_indices.is_empty() {
                net
            } else {
                net.with_opened_branches(&open_indices)
            }
        } else {
            net
        };

        self.network = net;
        // switch_states 的更新已移至 apply_action 中 solve_power_flow 成功后，
        // 避免潮流失败时 switch_states 与电压/相角状态不一致。
    }

    /// 构建修改后的 p_spec（基础值 + 覆盖）
    fn build_modified_p_spec(&self, net: &PowerNetwork) -> Vec<f64> {
        let mut p_spec = net.p_spec().to_vec();
        for (&idx, &val) in &self.p_spec_overrides {
            if idx < p_spec.len() {
                p_spec[idx] = val;
            }
        }
        p_spec
    }

    /// 构建修改后的 q_spec（基础值 + 覆盖）
    fn build_modified_q_spec(&self, net: &PowerNetwork) -> Vec<f64> {
        let mut q_spec = net.q_spec_view().to_vec();
        for (&idx, &val) in &self.q_spec_overrides {
            if idx < q_spec.len() {
                q_spec[idx] = val;
            }
        }
        q_spec
    }

    /// 应用发电机出力调整
    ///
    /// 参考 [`eneros_network::NetworkSimulatorAdapter`] 的 `simulate_generator` 逻辑：
    /// 净注入 = (发电 - 负荷) / base_mva
    ///
    /// 校验规则（与 `NetworkSimulatorAdapter::simulate_generator` 一致）：
    /// - 若发电机位于 Slack 母线，返回错误（Slack 母线的 p_spec 对潮流无影响）
    /// - 若 `p_mw` 不在 `[gen.p_min_mw, gen.p_max_mw]` 范围内，返回错误
    fn apply_generator_adjustment(
        &mut self,
        gen_id: ElementId,
        p_mw: f64,
        q_mvar: f64,
    ) -> Result<()> {
        if !p_mw.is_finite() || !q_mvar.is_finite() {
            return Err(EnerOSError::PowerFlow(format!(
                "发电机 {} 参数不合法: p_mw={}, q_mvar={}",
                gen_id, p_mw, q_mvar
            )));
        }

        let gen = self
            .base_network
            .generator_at(gen_id)
            .ok_or_else(|| EnerOSError::PowerFlow(format!("发电机 {} 未找到", gen_id)))?;

        let &bus_idx = self
            .base_network
            .bus_map()
            .get(&gen.bus_id)
            .ok_or_else(|| {
                EnerOSError::PowerFlow(format!("发电机 {} 的母线 {} 未找到", gen_id, gen.bus_id))
            })?;

        // Slack 母线的 p_spec 对潮流无影响（有功由平衡机自动平衡），
        // 调整 Slack 母线上的发电机出力无意义，返回错误。
        if matches!(
            self.base_network.bus_types().get(bus_idx),
            Some(BusTypeNR::Slack)
        ) {
            return Err(EnerOSError::PowerFlow(format!(
                "发电机 {} 位于 Slack 母线 {}，其 p_spec 对潮流无影响",
                gen_id, gen.bus_id
            )));
        }

        // 校验出力上下限：p_mw 必须在 [p_min_mw, p_max_mw] 范围内
        if p_mw < gen.p_min_mw || p_mw > gen.p_max_mw {
            return Err(EnerOSError::PowerFlow(format!(
                "发电机 {} 出力 {} MW 超出范围 [{}, {}] MW",
                gen_id, p_mw, gen.p_min_mw, gen.p_max_mw
            )));
        }

        // 净注入 = (发电 - 负荷) / base_mva
        let p_pu = (p_mw - gen.p_load_mw) / self.base_mva;
        let q_pu = q_mvar / self.base_mva;

        self.p_spec_overrides.insert(bus_idx, p_pu);
        self.q_spec_overrides.insert(bus_idx, q_pu);

        Ok(())
    }

    /// 应用负荷调整
    ///
    /// `load_id` 解释为母线 ID。若该母线有发电机，则保留发电、调整负荷；
    /// 否则按纯负荷母线处理。
    fn apply_load_adjustment(
        &mut self,
        load_id: ElementId,
        p_mw: f64,
        q_mvar: f64,
    ) -> Result<()> {
        if !p_mw.is_finite() || !q_mvar.is_finite() {
            return Err(EnerOSError::PowerFlow(format!(
                "负荷 {} 参数不合法: p_mw={}, q_mvar={}",
                load_id, p_mw, q_mvar
            )));
        }

        let &bus_idx = self
            .base_network
            .bus_map()
            .get(&load_id)
            .ok_or_else(|| EnerOSError::PowerFlow(format!("负荷母线 {} 未找到", load_id)))?;

        // 检查该母线是否有发电机
        let gen_at_bus = self
            .base_network
            .generator_table()
            .iter()
            .find(|g| g.bus_id == load_id);

        let p_pu = if let Some(gen) = gen_at_bus {
            // 有发电机的母线：净注入 = (发电 - 负荷) / base_mva
            (gen.p_gen_mw - p_mw) / self.base_mva
        } else {
            // 纯负荷母线：净注入 = -负荷 / base_mva
            -p_mw / self.base_mva
        };
        let q_pu = -q_mvar / self.base_mva;

        self.p_spec_overrides.insert(bus_idx, p_pu);
        self.q_spec_overrides.insert(bus_idx, q_pu);

        Ok(())
    }

    /// 应用区域负荷切除
    ///
    /// 参考 [`eneros_network::NetworkSimulatorAdapter`] 的 `simulate_shed_load` 逻辑：
    /// 按各负荷母线的当前负荷比例分配切除量。切除负荷使净注入更正（少负）。
    fn apply_load_shedding(&mut self, zone_id: u32, percentage: f64) -> Result<()> {
        if !(0.0..=1.0).contains(&percentage) {
            return Err(EnerOSError::PowerFlow(format!(
                "切除比例 {} 超出范围 [0, 1]",
                percentage
            )));
        }

        let bus_ids = self
            .base_network
            .zone_buses(zone_id)
            .ok_or_else(|| EnerOSError::PowerFlow(format!("区域 {} 未找到", zone_id)))?;

        let bus_map = self.base_network.bus_map();
        // 读取当前 p_spec（已包含之前的覆盖）
        let p_spec = self.network.p_spec();

        // 计算区域内各母线的负荷
        let mut load_buses: Vec<(usize, f64)> = Vec::new();
        for &bid in bus_ids {
            if let Some(&idx) = bus_map.get(&bid) {
                if idx < p_spec.len() {
                    let net_p_pu = p_spec[idx];
                    let load_mw = (-net_p_pu).max(0.0) * self.base_mva;
                    if load_mw > 0.0 {
                        load_buses.push((idx, load_mw));
                    }
                }
            }
        }

        // 无负荷可切
        if load_buses.is_empty() {
            return Ok(());
        }

        // 切除各母线 percentage 比例的负荷
        for (idx, load_mw) in &load_buses {
            let shed_mw = load_mw * percentage;
            let current_p_pu = p_spec[*idx];
            // 切除负荷使净注入增加（更不负）
            let new_p_pu = current_p_pu + shed_mw / self.base_mva;
            self.p_spec_overrides.insert(*idx, new_p_pu);
        }

        Ok(())
    }
}

impl GridAction {
    /// 从场景动作构建电网动作
    ///
    /// 提供 [`ScenarioAction`] 与 [`GridAction`] 之间的桥接，使场景脚本引擎
    /// 可以驱动 [`GridSimulator`]。
    pub fn from_scenario_action(
        action: &ScenarioAction,
        params: &HashMap<String, serde_json::Value>,
    ) -> Option<Self> {
        match action {
            ScenarioAction::LineTrip => {
                let branch_id = params.get("branch_id")?.as_u64()?;
                Some(GridAction::OpenBranch { branch_id })
            }
            ScenarioAction::GeneratorTrip => {
                let gen_id = params.get("gen_id")?.as_u64()?;
                Some(GridAction::AdjustGenerator {
                    gen_id,
                    p_mw: 0.0,
                    q_mvar: 0.0,
                })
            }
            ScenarioAction::LoadChange => {
                let load_id = params.get("load_id")?.as_u64()?;
                let p_mw = params.get("p_mw")?.as_f64()?;
                let q_mvar = params.get("q_mvar").and_then(|v| v.as_f64()).unwrap_or(0.0);
                Some(GridAction::AdjustLoad {
                    load_id,
                    p_mw,
                    q_mvar,
                })
            }
            ScenarioAction::LoadShed => {
                let zone_id = params.get("zone_id")?.as_u64()? as u32;
                let percentage = params.get("percentage")?.as_f64()?;
                Some(GridAction::ShedLoad {
                    zone_id,
                    percentage,
                })
            }
            ScenarioAction::InjectFault | ScenarioAction::ClearFault | ScenarioAction::Observe => {
                None
            }
        }
    }
}

/// 查找支路 ID 在 `branch_ids` 中的索引
fn find_branch_index(net: &PowerNetwork, branch_id: ElementId) -> Option<usize> {
    net.branch_ids().iter().position(|&id| id == branch_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_network::PowerNetwork;
    use eneros_powerflow::{BusTypeNR, YBusMatrix};

    /// 创建基于 IEEE 14 节点系统的 GridSimulator
    fn make_simulator() -> GridSimulator {
        GridSimulator::new(PowerNetwork::from_ieee14())
    }

    #[test]
    fn test_grid_simulator_new() {
        let sim = make_simulator();
        // 初始时间为 0
        assert!((sim.current_time() - 0.0).abs() < 1e-9);
        // 默认模式为稳态
        assert_eq!(sim.mode(), SimulationMode::SteadyState);
        // 初始状态应包含 14 个母线的电压
        assert_eq!(sim.snapshot().voltages.len(), 14);
        // 初始状态应包含 20 条支路的开关状态
        assert_eq!(sim.snapshot().switch_states.len(), 20);
        // 所有开关初始应为闭合
        assert!(sim.snapshot().switch_states.values().all(|&v| v));
        // 频率应为默认值 50 Hz
        assert!((sim.snapshot().frequency - 50.0).abs() < 1e-9);
    }

    #[test]
    fn test_grid_simulator_step() {
        let mut sim = make_simulator();
        let initial_time = sim.current_time();
        // 推进 0.1 秒
        sim.step(0.1).expect("step 失败");
        assert!((sim.current_time() - initial_time - 0.1).abs() < 1e-9);
        // 再推进 0.5 秒
        sim.step(0.5).expect("step 失败");
        assert!((sim.current_time() - 0.6).abs() < 1e-9);
        // 状态应仍然有效
        assert_eq!(sim.snapshot().voltages.len(), 14);
    }

    #[test]
    fn test_grid_simulator_snapshot() {
        let sim = make_simulator();
        let snapshot = sim.snapshot();
        // 快照应包含电压、相角、支路功率
        assert!(!snapshot.voltages.is_empty());
        assert!(!snapshot.angles.is_empty());
        assert!(!snapshot.branch_power.is_empty());
        // 电压应在合理范围 [0.9, 1.1] p.u.
        for &v in snapshot.voltages.values() {
            assert!(v > 0.9 && v < 1.1, "电压 {} 超出合理范围", v);
        }
        // 频率应为 50 Hz
        assert!((snapshot.frequency - 50.0).abs() < 1e-9);
    }

    #[test]
    fn test_grid_simulator_apply_open_branch() {
        let mut sim = make_simulator();
        // 记录初始状态
        let initial_voltages = sim.snapshot().voltages.clone();
        // 断开支路 1（1→2），网络仍连通（经 1→5→2）
        sim.apply_action(&GridAction::OpenBranch { branch_id: 1 })
            .expect("打开支路失败");
        // 支路 1 的开关状态应为 false
        assert_eq!(
            sim.snapshot().switch_states.get(&1),
            Some(&false),
            "支路 1 应已断开"
        );
        // 其他支路仍应闭合
        assert_eq!(
            sim.snapshot().switch_states.get(&2),
            Some(&true),
            "支路 2 应仍闭合"
        );
        // 电压应发生变化（拓扑改变后潮流不同）
        let new_voltages = sim.snapshot().voltages.clone();
        assert_eq!(new_voltages.len(), 14, "断开支路后仍应有 14 个母线电压");
        // 至少有一个母线电压发生变化
        let changed = initial_voltages
            .iter()
            .any(|(id, &v)| (new_voltages.get(id).copied().unwrap_or(0.0) - v).abs() > 1e-6);
        assert!(changed, "断开支路后电压应发生变化");
    }

    #[test]
    fn test_grid_simulator_apply_adjust_generator() {
        let mut sim = make_simulator();
        let initial_voltages = sim.snapshot().voltages.clone();
        // 调整发电机 2（母线 2）出力为 50 MW
        sim.apply_action(&GridAction::AdjustGenerator {
            gen_id: 2,
            p_mw: 50.0,
            q_mvar: 10.0,
        })
        .expect("调整发电机失败");
        // 状态应仍然有效
        assert_eq!(sim.snapshot().voltages.len(), 14);
        // 电压应发生变化
        let new_voltages = sim.snapshot().voltages.clone();
        let changed = initial_voltages
            .iter()
            .any(|(id, &v)| (new_voltages.get(id).copied().unwrap_or(0.0) - v).abs() > 1e-6);
        assert!(changed, "调整发电机后电压应发生变化");
    }

    #[test]
    fn test_grid_simulator_apply_shed_load() {
        let mut sim = make_simulator();
        let initial_voltages = sim.snapshot().voltages.clone();
        // 切除区域 0 的 10% 负荷
        sim.apply_action(&GridAction::ShedLoad {
            zone_id: 0,
            percentage: 0.1,
        })
        .expect("切除负荷失败");
        // 状态应仍然有效
        assert_eq!(sim.snapshot().voltages.len(), 14);
        // 电压应发生变化（负荷减少，电压通常升高）
        let new_voltages = sim.snapshot().voltages.clone();
        let changed = initial_voltages
            .iter()
            .any(|(id, &v)| (new_voltages.get(id).copied().unwrap_or(0.0) - v).abs() > 1e-6);
        assert!(changed, "切除负荷后电压应发生变化");
    }

    #[test]
    fn test_grid_simulator_steady_state_mode() {
        let sim = make_simulator();
        assert_eq!(sim.mode(), SimulationMode::SteadyState);
        // 使用 builder 设置暂态模式
        let sim_transient = GridSimulator::new(PowerNetwork::from_ieee14())
            .with_mode(SimulationMode::Transient);
        assert_eq!(sim_transient.mode(), SimulationMode::Transient);
    }

    #[test]
    fn test_grid_simulator_current_time() {
        let mut sim = make_simulator();
        // 验证时间推进
        assert!((sim.current_time() - 0.0).abs() < 1e-9);
        sim.step(1.0).expect("step 失败");
        assert!((sim.current_time() - 1.0).abs() < 1e-9);
        sim.step(2.5).expect("step 失败");
        assert!((sim.current_time() - 3.5).abs() < 1e-9);
        sim.step(0.01).expect("step 失败");
        assert!((sim.current_time() - 3.51).abs() < 1e-9);
    }

    /// 验证 branch_power 的键是真实支路 ID（与 network.branch_ids() 一致），
    /// 而非求解器合成的 `i * n + j` 索引对 ID。
    #[test]
    fn test_branch_power_uses_real_branch_id() {
        let sim = make_simulator();
        let snapshot = sim.snapshot();
        let network = sim.network();
        let branch_ids = network.branch_ids();

        // branch_power 不应为空
        assert!(
            !snapshot.branch_power.is_empty(),
            "branch_power 不应为空"
        );

        // 每个键都应是真实的支路 ID（来自 network.branch_ids()）
        for &key in snapshot.branch_power.keys() {
            assert!(
                branch_ids.contains(&key),
                "branch_power 的键 {} 不在 network.branch_ids() 中（可能是合成 ID）",
                key
            );
        }

        // 每个真实支路 ID 都应在 branch_power 中有对应条目
        for &bid in branch_ids {
            assert!(
                snapshot.branch_power.contains_key(&bid),
                "真实支路 ID {} 未在 branch_power 中找到",
                bid
            );
        }

        // 验证 switch_states 与 branch_power 的键集合一致
        assert_eq!(
            snapshot.branch_power.len(),
            snapshot.switch_states.len(),
            "branch_power 与 switch_states 的支路数量不一致"
        );
    }

    /// 验证 GridSimulator 正确读取非标准 base_mva，并据此将标幺值转换为 MW。
    ///
    /// 构建一个 base_mva=50 的 2 节点网络：母线 1 为 Slack，母线 2 负荷 50 MW + 25 MVar
    /// （即 1.0 + j0.5 标幺值）。若 base_mva 读取正确，支路功率应为 MW 量级（~50 MW），
    /// 而非标幺值量级（~1.0）。
    #[test]
    fn test_non_standard_base_mva() {
        let mut bus_map = HashMap::new();
        bus_map.insert(1u64, 0usize);
        bus_map.insert(2u64, 1usize);

        let branches = vec![(1u64, 2u64, 0.01, 0.1, 0.0, 1.0)];
        let mut ybus = YBusMatrix::from_branches(&branches, &bus_map);
        ybus.set_base_mva(50.0);

        // p_spec: 母线 2 负荷 50 MW = 1.0 pu (base_mva=50)
        // q_spec: 母线 2 负荷 25 MVar = 0.5 pu
        let network = PowerNetwork::new(
            ybus,
            vec![0.0, -1.0],
            vec![0.0, -0.5],
            vec![BusTypeNR::Slack, BusTypeNR::PQ],
            branches,
            bus_map,
        )
        .with_branch_ids(vec![1])
        .with_initial_voltages(vec![1.0, 1.0]);

        let sim = GridSimulator::new(network);

        // 验证 base_mva 被正确读取为 50.0，而非硬编码的 100.0
        assert!(
            (sim.base_mva() - 50.0).abs() < 1e-9,
            "base_mva 应为 50.0，实际为 {}",
            sim.base_mva()
        );

        let snapshot = sim.snapshot();

        // branch_power 应包含真实支路 ID 1
        let p_mw = snapshot
            .branch_power
            .get(&1)
            .expect("支路 1 应在 branch_power 中");
        // 母线 2 负荷 50 MW，支路功率应在 MW 量级（约 50 MW），
        // 而非标幺值量级（约 1.0）。阈值 10 MW 可区分两种情况。
        assert!(
            p_mw.abs() > 10.0,
            "支路功率 {} 应为 MW 量级（>10），若为标幺值量级则说明未乘以 base_mva",
            p_mw
        );
    }

    /// 验证对 Slack 母线上的发电机调整出力时返回错误。
    ///
    /// IEEE-14 中发电机 1 位于 Slack 母线（母线 1），其 p_spec 对潮流无影响
    /// （有功由平衡机自动平衡），调整出力无意义，应返回错误。
    #[test]
    fn test_adjust_generator_slack_bus_error() {
        let mut sim = make_simulator();
        let result = sim.apply_action(&GridAction::AdjustGenerator {
            gen_id: 1, // 发电机 1 位于 Slack 母线 1
            p_mw: 100.0,
            q_mvar: 0.0,
        });
        assert!(
            result.is_err(),
            "对 Slack 母线上的发电机调整出力应返回错误，实际: {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Slack"),
            "错误信息应包含 Slack，实际: {}",
            err_msg
        );
    }

    /// 验证调整发电机出力超出 Pmin/Pmax 范围时返回错误。
    ///
    /// IEEE-14 中发电机 2 的 p_max_mw=140.0，调整到 200.0 MW 超出上限，应返回错误。
    #[test]
    fn test_adjust_generator_out_of_range() {
        let mut sim = make_simulator();
        let result = sim.apply_action(&GridAction::AdjustGenerator {
            gen_id: 2, // 发电机 2，p_max_mw=140.0
            p_mw: 200.0,
            q_mvar: 0.0,
        });
        assert!(
            result.is_err(),
            "调整出力超出 Pmax 应返回错误，实际: {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("超出范围"),
            "错误信息应包含超出范围，实际: {}",
            err_msg
        );
    }

    /// 验证闭合不存在的支路时返回错误。
    ///
    /// 支路 999 不存在于 IEEE-14 网络（仅有 1..=20），CloseBranch 应返回错误，
    /// 而非静默无操作。
    #[test]
    fn test_close_branch_nonexistent() {
        let mut sim = make_simulator();
        let result = sim.apply_action(&GridAction::CloseBranch { branch_id: 999 });
        assert!(
            result.is_err(),
            "闭合不存在的支路应返回错误，实际: {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("未找到"),
            "错误信息应包含未找到，实际: {}",
            err_msg
        );
    }

    /// 验证闭合已断开的支路成功。
    ///
    /// 先断开支路 1，再闭合支路 1，验证开关状态恢复为闭合。
    #[test]
    fn test_close_branch_valid() {
        let mut sim = make_simulator();
        // 先断开支路 1
        sim.apply_action(&GridAction::OpenBranch { branch_id: 1 })
            .expect("打开支路失败");
        assert_eq!(
            sim.snapshot().switch_states.get(&1),
            Some(&false),
            "支路 1 应已断开"
        );
        // 闭合已断开的支路 1
        sim.apply_action(&GridAction::CloseBranch { branch_id: 1 })
            .expect("闭合支路失败");
        assert_eq!(
            sim.snapshot().switch_states.get(&1),
            Some(&true),
            "支路 1 应已闭合"
        );
        // 其他支路应仍闭合
        assert_eq!(
            sim.snapshot().switch_states.get(&2),
            Some(&true),
            "支路 2 应仍闭合"
        );
    }

    /// 验证调整负荷后净注入发生变化。
    ///
    /// 母线 9 为纯负荷母线，调整其负荷后 p_spec 应改变，电压也应随之变化。
    #[test]
    fn test_adjust_load() {
        let mut sim = make_simulator();
        // 读取母线 9 的初始 p_spec
        let bus_map = sim.network().bus_map();
        let &bus_idx = bus_map.get(&9).expect("母线 9 应存在");
        let initial_p_spec = sim.network().p_spec()[bus_idx];
        // 调整母线 9 的负荷为 50 MW + 10 MVar
        sim.apply_action(&GridAction::AdjustLoad {
            load_id: 9,
            p_mw: 50.0,
            q_mvar: 10.0,
        })
        .expect("调整负荷失败");
        // 验证净注入已变化
        let new_p_spec = sim.network().p_spec()[bus_idx];
        assert!(
            (new_p_spec - initial_p_spec).abs() > 1e-6,
            "调整负荷后净注入应发生变化: 初始={}, 新={}",
            initial_p_spec,
            new_p_spec
        );
        // 状态应仍然有效
        assert_eq!(sim.snapshot().voltages.len(), 14);
    }

    /// 验证 `GridAction::from_scenario_action` 正确转换各场景动作。
    #[test]
    fn test_from_scenario_action() {
        use crate::scenario::ScenarioAction;

        // LineTrip → OpenBranch
        let mut params = HashMap::new();
        params.insert("branch_id".to_string(), serde_json::json!(5));
        let action = GridAction::from_scenario_action(&ScenarioAction::LineTrip, &params);
        assert!(
            matches!(action, Some(GridAction::OpenBranch { branch_id: 5 })),
            "LineTrip 应转换为 OpenBranch"
        );

        // GeneratorTrip → AdjustGenerator（p_mw=0, q_mvar=0）
        let mut params = HashMap::new();
        params.insert("gen_id".to_string(), serde_json::json!(2));
        let action =
            GridAction::from_scenario_action(&ScenarioAction::GeneratorTrip, &params);
        assert!(
            matches!(
                action,
                Some(GridAction::AdjustGenerator {
                    gen_id: 2,
                    p_mw: 0.0,
                    q_mvar: 0.0
                })
            ),
            "GeneratorTrip 应转换为 AdjustGenerator"
        );

        // LoadChange → AdjustLoad
        let mut params = HashMap::new();
        params.insert("load_id".to_string(), serde_json::json!(9));
        params.insert("p_mw".to_string(), serde_json::json!(50.0));
        params.insert("q_mvar".to_string(), serde_json::json!(10.0));
        let action = GridAction::from_scenario_action(&ScenarioAction::LoadChange, &params);
        assert!(
            matches!(
                action,
                Some(GridAction::AdjustLoad {
                    load_id: 9,
                    p_mw: 50.0,
                    q_mvar: 10.0
                })
            ),
            "LoadChange 应转换为 AdjustLoad"
        );

        // LoadShed → ShedLoad
        let mut params = HashMap::new();
        params.insert("zone_id".to_string(), serde_json::json!(1u64));
        params.insert("percentage".to_string(), serde_json::json!(0.3));
        let action = GridAction::from_scenario_action(&ScenarioAction::LoadShed, &params);
        assert!(
            matches!(
                action,
                Some(GridAction::ShedLoad {
                    zone_id: 1,
                    percentage: 0.3
                })
            ),
            "LoadShed 应转换为 ShedLoad"
        );

        // InjectFault / ClearFault / Observe → None
        let empty_params = HashMap::new();
        assert!(
            GridAction::from_scenario_action(&ScenarioAction::InjectFault, &empty_params)
                .is_none(),
            "InjectFault 应转换为 None"
        );
        assert!(
            GridAction::from_scenario_action(&ScenarioAction::ClearFault, &empty_params)
                .is_none(),
            "ClearFault 应转换为 None"
        );
        assert!(
            GridAction::from_scenario_action(&ScenarioAction::Observe, &empty_params)
                .is_none(),
            "Observe 应转换为 None"
        );

        // 缺少必要参数 → None
        assert!(
            GridAction::from_scenario_action(&ScenarioAction::LineTrip, &empty_params)
                .is_none(),
            "缺少 branch_id 时应返回 None"
        );
    }
}
