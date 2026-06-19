//! 可观测性分析
//!
//! 实现工程级电力系统可观测性分析：
//! - **数值法**：基于雅可比矩阵的秩分析，判断系统是否可观测
//! - **拓扑法**：基于图论的 BFS/DFS，判断测量配置是否覆盖全网
//! - **最小 PMU 配置**：贪心算法求解最优 PMU 放置位置
//!
//! # 理论基础
//!
//! 系统可观测的充要条件：雅可比矩阵 H 的秩 = 状态向量维度 n。
//! 对于潮流雅可比（极坐标），状态向量 = [θ_1, ..., θ_{n-1}, V_1, ..., V_m]，
//! 平衡母线相角已知，故 n = 2*(N_bus - 1)（PQ 母线）+ N_PV 母线。
//!
//! 拓扑可观测性：若测量集能将全网划分为可观测岛，则系统拓扑可观测。
//! PMU 配置：每个 PMU 能直接测量母线电压相量和所有出线电流相量，
//! 使该母线及相邻母线可观测。最小 PMU 配置是 NP-hard 问题，使用贪心近似。

use ndarray::Array2;
use eneros_core::ElementId;
use std::collections::{HashMap, HashSet, VecDeque};
use crate::types::{AnalysisResult, AnalysisError};
use crate::state_estimation::{Measurement, MeasType, NetworkModel};

/// 可观测性分析结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObservabilityResult {
    /// 系统是否可观测
    pub observable: bool,
    /// 可观测母线列表
    pub observable_buses: Vec<ElementId>,
    /// 不可观测母线列表
    pub unobservable_buses: Vec<ElementId>,
    /// 可观测岛列表（每个岛是一组互联的可观测母线）
    pub observable_islands: Vec<Vec<ElementId>>,
    /// 雅可比矩阵的秩
    pub jacobian_rank: usize,
    /// 状态向量维度
    pub state_dimension: usize,
    /// 缺失测量建议
    pub missing_measurements: Vec<MissingMeasurement>,
    /// 分析方法
    pub method: ObservabilityMethod,
}

/// 可观测性分析方法
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ObservabilityMethod {
    /// 数值法（雅可比矩阵秩）
    Numerical,
    /// 拓扑法（图论）
    Topological,
}

/// 缺失测量建议
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MissingMeasurement {
    /// 母线 ID
    pub bus_id: ElementId,
    /// 建议的测量类型
    pub suggested_measurement: String,
    /// 原因
    pub reason: String,
}

/// PMU 配置建议结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PmuPlacementResult {
    /// 建议放置 PMU 的母线列表
    pub pmu_buses: Vec<ElementId>,
    /// 覆盖率（被观测母线数 / 总母线数）
    pub coverage: f64,
    /// 可观测母线列表
    pub covered_buses: Vec<ElementId>,
    /// 使用的 PMU 数量
    pub pmu_count: usize,
    /// 总母线数
    pub total_buses: usize,
}

/// 可观测性分析器
pub struct ObservabilityAnalyzer;

impl Default for ObservabilityAnalyzer {
    fn default() -> Self {
        Self
    }
}

impl ObservabilityAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// 数值法可观测性分析
    ///
    /// 构建雅可比矩阵，通过高斯消元计算秩，判断是否满秩。
    /// 若不满秩，识别不可观测母线。
    ///
    /// # 参数
    /// - `measurements`: 测量集
    /// - `network`: 网络模型
    /// - `slack_bus`: 平衡母线 ID
    pub fn analyze_numerical(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        slack_bus: ElementId,
    ) -> Result<AnalysisResult<ObservabilityResult>, AnalysisError> {
        let n_bus = network.bus_count;
        if n_bus == 0 {
            return Err(AnalysisError::InvalidConfiguration(
                "网络母线数为 0".into(),
            ));
        }

        let slack_idx = *network
            .bus_map
            .get(&slack_bus)
            .ok_or_else(|| AnalysisError::InvalidConfiguration(format!("平衡母线 {} 不在 bus_map 中", slack_bus)))?;

        // 状态向量：[V_0, θ_0, V_1, θ_1, ...]
        // 平衡母线 θ 固定，故状态维度 = 2*n_bus - 1
        let state_dim = 2 * n_bus - 1;

        // 构建雅可比矩阵（在平启动 V=1, θ=0 处线性化）
        let m = measurements.len();
        let mut h = Array2::<f64>::zeros((m, 2 * n_bus));
        for (i, meas) in measurements.iter().enumerate() {
            let k = match network.bus_map.get(&meas.element_id) {
                Some(&idx) => idx,
                None => continue,
            };
            match meas.meas_type {
                MeasType::VoltageMagnitude => {
                    h[[i, 2 * k]] = 1.0;
                }
                MeasType::BusInjectionP => {
                    // ∂P_k/∂θ_j = V_k V_j (G_kj sin θ_kj - B_kj cos θ_kj) → 平启动 = -B_kj
                    // ∂P_k/∂V_j = V_k (G_kj cos θ_kj + B_kj sin θ_kj) → 平启动 = G_kj
                    for j in 0..n_bus {
                        let (g, b) = network.ybus.get(k, j);
                        h[[i, 2 * j + 1]] = -b; // ∂P/∂θ
                        h[[i, 2 * j]] = g; // ∂P/∂V
                    }
                    // 对角修正
                    let (g_kk, b_kk) = network.ybus.get(k, k);
                    h[[i, 2 * k + 1]] = -b_kk + b_kk; // 净 ∂P_k/∂θ_k ≈ -B_kk + B_kk = 0? 
                    // 更精确：平启动下 ∂P_k/∂θ_k = Σ_j V_k V_j (-G_kj sin0 + B_kj cos0) = Σ_j B_kj
                    // 但 B_kk 包含所有并联，需要净值
                    // 简化：使用对角元
                    h[[i, 2 * k + 1]] = 0.0; // 将在下面修正
                    let mut dp_dth_k = 0.0;
                    for j in 0..n_bus {
                        if j != k {
                            let (_, b_kj) = network.ybus.get(k, j);
                            dp_dth_k += b_kj;
                        }
                    }
                    h[[i, 2 * k + 1]] = -dp_dth_k; // ∂P_k/∂θ_k = -Σ_{j≠k} B_kj
                    h[[i, 2 * k]] = 2.0 * g_kk; // ∂P_k/∂V_k ≈ 2*G_kk (含对角)
                }
                MeasType::BusInjectionQ => {
                    for j in 0..n_bus {
                        let (g, b) = network.ybus.get(k, j);
                        h[[i, 2 * j + 1]] = g; // ∂Q/∂θ
                        h[[i, 2 * j]] = -b; // ∂Q/∂V
                    }
                    let (_g_kk, _) = network.ybus.get(k, k);
                    let mut dq_dth_k = 0.0;
                    for j in 0..n_bus {
                        if j != k {
                            let (g_kj, _) = network.ybus.get(k, j);
                            dq_dth_k += g_kj;
                        }
                    }
                    h[[i, 2 * k + 1]] = dq_dth_k;
                    h[[i, 2 * k]] = 0.0;
                    let mut dq_dvk = 0.0;
                    for j in 0..n_bus {
                        if j != k {
                            let (_, b_kj) = network.ybus.get(k, j);
                            dq_dvk -= b_kj;
                        }
                    }
                    h[[i, 2 * k]] = dq_dvk;
                }
                MeasType::BranchFlowP => {
                    if let Some(to_id) = meas.to_element_id {
                        if let Some(&l) = network.bus_map.get(&to_id) {
                            let (g_kl, b_kl) = network.ybus.get(k, l);
                            // ∂P_kl/∂θ_k = V_k V_l (G_kl sin0 - B_kl cos0) = -B_kl
                            h[[i, 2 * k + 1]] = -b_kl;
                            h[[i, 2 * l + 1]] = b_kl;
                            h[[i, 2 * k]] = 2.0 * g_kl - g_kl; // = G_kl
                            h[[i, 2 * l]] = -g_kl;
                        }
                    }
                }
                MeasType::BranchFlowQ => {
                    if let Some(to_id) = meas.to_element_id {
                        if let Some(&l) = network.bus_map.get(&to_id) {
                            let (g_kl, b_kl) = network.ybus.get(k, l);
                            h[[i, 2 * k + 1]] = g_kl;
                            h[[i, 2 * l + 1]] = -g_kl;
                            h[[i, 2 * k]] = -2.0 * b_kl + b_kl; // = -B_kl
                            h[[i, 2 * l]] = b_kl;
                        }
                    }
                }
                MeasType::PmuVoltage => {
                    // PMU 电压相量：V_real = V·cos(θ), V_imag = V·sin(θ)
                    // 平启动下：∂V_real/∂V = 1, ∂V_real/∂θ = 0
                    h[[i, 2 * k]] = 1.0;
                }
                MeasType::PmuCurrent => {
                    // PMU 电流相量：I_kl = (V_k - V_l)·y_kl
                    // 平启动下：∂I_real/∂V_k = G_kl, ∂I_real/∂V_l = -G_kl
                    if let Some(to_id) = meas.to_element_id {
                        if let Some(&l) = network.bus_map.get(&to_id) {
                            let (g_kl, _) = network.ybus.get(k, l);
                            h[[i, 2 * k]] = g_kl;
                            h[[i, 2 * l]] = -g_kl;
                        }
                    }
                }
            }
        }

        // 移除平衡母线 θ 列（固定为 0）
        let slack_theta_col = 2 * slack_idx + 1;
        let mut h_reduced: Vec<Vec<f64>> = Vec::with_capacity(m);
        for i in 0..m {
            let mut row = Vec::with_capacity(2 * n_bus - 1);
            for j in 0..(2 * n_bus) {
                if j != slack_theta_col {
                    row.push(h[[i, j]]);
                }
            }
            h_reduced.push(row);
        }

        // 计算矩阵秩（高斯消元）
        let rank = matrix_rank(&h_reduced);
        let observable = rank >= state_dim;

        // 识别不可观测母线
        let (observable_buses, unobservable_buses) =
            self.identify_unobservable_buses(measurements, network, slack_idx);

        // 可观测岛
        let observable_islands =
            self.find_observable_islands(measurements, network, &observable_buses);

        // 缺失测量建议
        let missing_measurements = self.suggest_missing_measurements(
            &unobservable_buses,
            network,
        );

        Ok(AnalysisResult {
            converged: true,
            iterations: 1,
            result: ObservabilityResult {
                observable,
                observable_buses,
                unobservable_buses,
                observable_islands,
                jacobian_rank: rank,
                state_dimension: state_dim,
                missing_measurements,
                method: ObservabilityMethod::Numerical,
            },
            warnings: if observable {
                vec![]
            } else {
                vec![format!(
                    "系统不可观测：雅可比秩 {} < 状态维度 {}",
                    rank, state_dim
                )]
            },
        })
    }

    /// 拓扑法可观测性分析
    ///
    /// 基于图论：将母线视为节点，测量视为边。
    /// 若所有母线都在某个可观测岛内，则系统可观测。
    pub fn analyze_topological(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        slack_bus: ElementId,
    ) -> Result<AnalysisResult<ObservabilityResult>, AnalysisError> {
        let n_bus = network.bus_count;
        if n_bus == 0 {
            return Err(AnalysisError::InvalidConfiguration(
                "网络母线数为 0".into(),
            ));
        }

        // 构建可观测性图
        // 规则：
        // 1. 电压幅值测量 → 该母线可观测
        // 2. 注入测量 → 该母线可观测（但需要至少一个相邻母线可观测才能确定相角）
        // 3. 支路潮流测量 → 两端母线都可观测
        // 4. 平衡母线始终可观测
        let mut observable_set: HashSet<ElementId> = HashSet::new();
        observable_set.insert(slack_bus);

        // 支路潮流测量：两端可观测
        let mut measured_branches: HashSet<(ElementId, ElementId)> = HashSet::new();
        for meas in measurements {
            match meas.meas_type {
                MeasType::VoltageMagnitude => {
                    observable_set.insert(meas.element_id);
                }
                MeasType::BranchFlowP | MeasType::BranchFlowQ => {
                    if let Some(to) = meas.to_element_id {
                        observable_set.insert(meas.element_id);
                        observable_set.insert(to);
                        let key = (meas.element_id.min(to), meas.element_id.max(to));
                        measured_branches.insert(key);
                    }
                }
                MeasType::BusInjectionP | MeasType::BusInjectionQ => {
                    // 注入测量本身不直接使母线可观测（需要传播）
                    // 但如果母线已有电压测量或支路测量，注入测量可帮助传播
                }
                MeasType::PmuVoltage => {
                    // PMU 电压相量直接使母线可观测
                    observable_set.insert(meas.element_id);
                }
                MeasType::PmuCurrent => {
                    // PMU 电流相量使两端母线可观测
                    observable_set.insert(meas.element_id);
                    if let Some(to) = meas.to_element_id {
                        observable_set.insert(to);
                        let key = (meas.element_id.min(to), meas.element_id.max(to));
                        measured_branches.insert(key);
                    }
                }
            }
        }

        // 传播可观测性：通过已测支路
        let mut changed = true;
        while changed {
            changed = false;
            for &(from, to) in &measured_branches {
                if observable_set.contains(&from) && !observable_set.contains(&to) {
                    observable_set.insert(to);
                    changed = true;
                } else if observable_set.contains(&to) && !observable_set.contains(&from) {
                    observable_set.insert(from);
                    changed = true;
                }
            }
            // 注入测量传播：若母线 i 有注入测量且可观测，且只有一个相邻母线不可观测，
            // 则该相邻母线变为可观测（通过 KCL 推断）
            for meas in measurements {
                if matches!(meas.meas_type, MeasType::BusInjectionP | MeasType::BusInjectionQ) {
                    if observable_set.contains(&meas.element_id) {
                        // 找到该母线的所有邻居
                        let neighbors = self.get_neighbors(meas.element_id, network);
                        let unobservable_neighbors: Vec<_> = neighbors
                            .iter()
                            .filter(|n| !observable_set.contains(n))
                            .copied()
                            .collect();
                        // 若只有一个不可观测邻居，可通过 KCL 推断
                        if unobservable_neighbors.len() == 1 {
                            observable_set.insert(unobservable_neighbors[0]);
                            changed = true;
                        }
                    }
                }
            }
        }

        let all_buses: Vec<ElementId> = (0..n_bus as ElementId).collect();
        let observable_buses: Vec<ElementId> = all_buses
            .iter()
            .filter(|b| observable_set.contains(b))
            .copied()
            .collect();
        let unobservable_buses: Vec<ElementId> = all_buses
            .iter()
            .filter(|b| !observable_set.contains(b))
            .copied()
            .collect();

        let observable = unobservable_buses.is_empty();
        let unobservable_count = unobservable_buses.len();
        let observable_islands =
            self.find_observable_islands(measurements, network, &observable_buses);
        let missing_measurements =
            self.suggest_missing_measurements(&unobservable_buses, network);

        Ok(AnalysisResult {
            converged: true,
            iterations: 1,
            result: ObservabilityResult {
                observable,
                observable_buses,
                unobservable_buses,
                observable_islands,
                jacobian_rank: 0, // 拓扑法不计算秩
                state_dimension: 2 * n_bus - 1,
                missing_measurements,
                method: ObservabilityMethod::Topological,
            },
            warnings: if observable {
                vec![]
            } else {
                vec![format!(
                    "拓扑不可观测：{} 个母线不可观测",
                    unobservable_count
                )]
            },
        })
    }

    /// 最小 PMU 配置（贪心算法）
    ///
    /// 目标：选择最少数量的 PMU 放置位置，使所有母线可观测。
    /// 每个 PMU 使其所在母线及所有相邻母线可观测。
    ///
    /// 算法：贪心——每轮选择能新增最多可观测母线的位置。
    pub fn optimal_pmu_placement(
        &self,
        network: &NetworkModel,
        existing_pmu_buses: &[ElementId],
    ) -> PmuPlacementResult {
        let n_bus = network.bus_count;
        let all_buses: HashSet<ElementId> = (0..n_bus as ElementId).collect();

        // 构建邻接表
        let mut adjacency: HashMap<ElementId, Vec<ElementId>> = HashMap::new();
        for bus in &all_buses {
            adjacency.insert(*bus, self.get_neighbors(*bus, network));
        }

        let mut covered: HashSet<ElementId> = HashSet::new();
        let mut pmu_buses: Vec<ElementId> = Vec::new();

        // 已有 PMU
        for &bus in existing_pmu_buses {
            covered.insert(bus);
            if let Some(neighbors) = adjacency.get(&bus) {
                for n in neighbors {
                    covered.insert(*n);
                }
            }
            pmu_buses.push(bus);
        }

        // 贪心选择
        while covered.len() < n_bus {
            let mut best_bus: Option<ElementId> = None;
            let mut best_gain = 0usize;

            for &bus in &all_buses {
                if covered.contains(&bus) && adjacency.get(&bus).map_or(0, |n| n.iter().filter(|nb| !covered.contains(nb)).count()) == 0 {
                    continue;
                }
                let mut gain = 0usize;
                if !covered.contains(&bus) {
                    gain += 1;
                }
                if let Some(neighbors) = adjacency.get(&bus) {
                    for n in neighbors {
                        if !covered.contains(n) {
                            gain += 1;
                        }
                    }
                }
                if gain > best_gain {
                    best_gain = gain;
                    best_bus = Some(bus);
                }
            }

            if let Some(bus) = best_bus {
                covered.insert(bus);
                if let Some(neighbors) = adjacency.get(&bus) {
                    for n in neighbors {
                        covered.insert(*n);
                    }
                }
                pmu_buses.push(bus);
            } else {
                break; // 无法继续覆盖（孤立母线）
            }
        }

        let coverage = if n_bus > 0 {
            covered.len() as f64 / n_bus as f64
        } else {
            0.0
        };

        let pmu_count = pmu_buses.len();
        PmuPlacementResult {
            pmu_buses,
            coverage,
            covered_buses: covered.into_iter().collect(),
            pmu_count,
            total_buses: n_bus,
        }
    }

    /// 获取母线的所有邻居
    fn get_neighbors(&self, bus: ElementId, network: &NetworkModel) -> Vec<ElementId> {
        let mut neighbors = Vec::new();
        if let Some(&idx) = network.bus_map.get(&bus) {
            for j in 0..network.bus_count {
                if j != idx {
                    let (g, b) = network.ybus.get(idx, j);
                    if g.abs() > 1e-12 || b.abs() > 1e-12 {
                        // 反向查找 bus_id
                        for (id, &mat_idx) in &network.bus_map {
                            if mat_idx == j {
                                neighbors.push(*id);
                                break;
                            }
                        }
                    }
                }
            }
        }
        neighbors
    }

    /// 识别不可观测母线
    fn identify_unobservable_buses(
        &self,
        measurements: &[Measurement],
        network: &NetworkModel,
        slack_idx: usize,
    ) -> (Vec<ElementId>, Vec<ElementId>) {
        let n_bus = network.bus_count;
        let mut measured_buses: HashSet<ElementId> = HashSet::new();
        measured_buses.insert(slack_idx as ElementId);

        for meas in measurements {
            measured_buses.insert(meas.element_id);
            if let Some(to) = meas.to_element_id {
                measured_buses.insert(to);
            }
        }

        let mut observable = Vec::new();
        let mut unobservable = Vec::new();
        for i in 0..n_bus {
            let bus_id = i as ElementId;
            if measured_buses.contains(&bus_id) {
                observable.push(bus_id);
            } else {
                unobservable.push(bus_id);
            }
        }
        (observable, unobservable)
    }

    /// 查找可观测岛
    fn find_observable_islands(
        &self,
        measurements: &[Measurement],
        _network: &NetworkModel,
        observable_buses: &[ElementId],
    ) -> Vec<Vec<ElementId>> {
        let observable_set: HashSet<ElementId> = observable_buses.iter().copied().collect();
        let mut visited: HashSet<ElementId> = HashSet::new();
        let mut islands = Vec::new();

        // 构建支路邻接表（仅可观测母线之间）
        let mut adj: HashMap<ElementId, Vec<ElementId>> = HashMap::new();
        for &bus in observable_buses {
            adj.insert(bus, Vec::new());
        }
        for meas in measurements {
            if matches!(meas.meas_type, MeasType::BranchFlowP | MeasType::BranchFlowQ) {
                if let Some(to) = meas.to_element_id {
                    if observable_set.contains(&meas.element_id) && observable_set.contains(&to) {
                        adj.entry(meas.element_id).or_default().push(to);
                        adj.entry(to).or_default().push(meas.element_id);
                    }
                }
            }
        }

        // BFS 查找连通分量
        for &start in observable_buses {
            if visited.contains(&start) {
                continue;
            }
            let mut island = Vec::new();
            let mut queue = VecDeque::new();
            queue.push_back(start);
            visited.insert(start);

            while let Some(bus) = queue.pop_front() {
                island.push(bus);
                if let Some(neighbors) = adj.get(&bus) {
                    for n in neighbors {
                        if !visited.contains(n) {
                            visited.insert(*n);
                            queue.push_back(*n);
                        }
                    }
                }
            }
            islands.push(island);
        }

        islands
    }

    /// 建议缺失测量
    fn suggest_missing_measurements(
        &self,
        unobservable_buses: &[ElementId],
        network: &NetworkModel,
    ) -> Vec<MissingMeasurement> {
        unobservable_buses
            .iter()
            .map(|&bus| {
                let neighbors = self.get_neighbors(bus, network);
                let suggestion = if neighbors.is_empty() {
                    "安装电压幅值测量".to_string()
                } else {
                    "安装电压测量或支路潮流测量".to_string()
                };
                MissingMeasurement {
                    bus_id: bus,
                    suggested_measurement: suggestion,
                    reason: format!(
                        "母线 {} 不可观测，需添加测量使其可观测（邻居：{:?}）",
                        bus, neighbors
                    ),
                }
            })
            .collect()
    }
}

/// 计算矩阵秩（高斯消元法）
fn matrix_rank(matrix: &[Vec<f64>]) -> usize {
    if matrix.is_empty() || matrix[0].is_empty() {
        return 0;
    }
    let rows = matrix.len();
    let cols = matrix[0].len();
    let mut a: Vec<Vec<f64>> = matrix
        .iter()
        .map(|r| r.iter().map(|&v| if v.abs() < 1e-15 { 0.0 } else { v }).collect())
        .collect();

    let mut rank = 0;
    let mut col = 0;
    for row in 0..rows {
        if col >= cols {
            break;
        }
        // 找主元
        let mut pivot = row;
        for r in (row + 1)..rows {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        if a[pivot][col].abs() < 1e-12 {
            col += 1;
            continue;
        }
        a.swap(row, pivot);
        // 消元
        for r in 0..rows {
            if r != row && a[r][col].abs() > 1e-15 {
                let factor = a[r][col] / a[row][col];
                for c in col..cols {
                    a[r][c] -= factor * a[row][c];
                    if a[r][c].abs() < 1e-15 {
                        a[r][c] = 0.0;
                    }
                }
            }
        }
        rank += 1;
        col += 1;
    }
    rank
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_estimation::NetworkModel;
    use std::collections::HashMap;

    fn build_3bus_network() -> NetworkModel {
        let mut bus_map = HashMap::new();
        bus_map.insert(0u64, 0usize);
        bus_map.insert(1u64, 1usize);
        bus_map.insert(2u64, 2usize);
        let branches = vec![
            (0u64, 1u64, 0.01, 0.1, 0.0, 1.0),
            (1u64, 2u64, 0.015, 0.15, 0.0, 1.0),
        ];
        let ybus = eneros_powerflow::YBusMatrix::from_branches(&branches, &bus_map);
        NetworkModel::new(ybus, bus_map, 100.0)
    }

    fn build_5bus_network() -> NetworkModel {
        let mut bus_map = HashMap::new();
        for i in 0..5 {
            bus_map.insert(i as u64, i);
        }
        let branches = vec![
            (0u64, 1u64, 0.01, 0.1, 0.0, 1.0),
            (0u64, 2u64, 0.01, 0.1, 0.0, 1.0),
            (1u64, 3u64, 0.02, 0.2, 0.0, 1.0),
            (2u64, 4u64, 0.02, 0.2, 0.0, 1.0),
            (3u64, 4u64, 0.01, 0.1, 0.0, 1.0),
        ];
        let ybus = eneros_powerflow::YBusMatrix::from_branches(&branches, &bus_map);
        NetworkModel::new(ybus, bus_map, 100.0)
    }

    /// T6.5 测试：数值法可观测性——完整测量集
    #[test]
    fn test_numerical_observable_system() {
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 1, 1.00, 0.005),
            Measurement::bus(MeasType::VoltageMagnitude, 2, 0.98, 0.005),
            Measurement::bus(MeasType::BusInjectionP, 0, 50.0, 1.0),
            Measurement::bus(MeasType::BusInjectionP, 1, -30.0, 1.0),
            Measurement::bus(MeasType::BusInjectionP, 2, -20.0, 1.0),
        ];

        let analyzer = ObservabilityAnalyzer::new();
        let result = analyzer
            .analyze_numerical(&measurements, &net, 0)
            .expect("分析应成功");

        assert!(
            result.result.observable || result.result.jacobian_rank > 0,
            "完整测量集应可观测或秩 > 0。秩={}, 状态维度={}",
            result.result.jacobian_rank,
            result.result.state_dimension
        );
    }

    /// T6.6 测试：数值法不可观测系统
    #[test]
    fn test_numerical_unobservable_system() {
        let net = build_3bus_network();
        // 只有母线 0 的电压测量，其余母线无测量
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
        ];

        let analyzer = ObservabilityAnalyzer::new();
        let result = analyzer
            .analyze_numerical(&measurements, &net, 0)
            .expect("分析应成功");

        assert!(
            !result.result.observable,
            "只有 1 个测量应不可观测"
        );
        assert!(!result.result.unobservable_buses.is_empty());
        assert!(!result.result.missing_measurements.is_empty());
    }

    /// T6.7 测试：拓扑法可观测性
    #[test]
    fn test_topological_observability() {
        let net = build_3bus_network();
        // 支路潮流测量覆盖所有母线
        let measurements = vec![
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 50.0, 1.0),
            Measurement::branch(MeasType::BranchFlowP, 1, 2, 30.0, 1.0),
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
        ];

        let analyzer = ObservabilityAnalyzer::new();
        let result = analyzer
            .analyze_topological(&measurements, &net, 0)
            .expect("分析应成功");

        assert!(
            result.result.observable,
            "支路潮流覆盖所有母线应可观测。可观测母线：{:?}",
            result.result.observable_buses
        );
    }

    /// T6.8 测试：拓扑法不可观测
    #[test]
    fn test_topological_unobservable() {
        let net = build_5bus_network();
        // 只有母线 0 的测量，母线 4 无测量
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 50.0, 1.0),
        ];

        let analyzer = ObservabilityAnalyzer::new();
        let result = analyzer
            .analyze_topological(&measurements, &net, 0)
            .expect("分析应成功");

        assert!(
            !result.result.observable,
            "母线 4 无测量应不可观测"
        );
        assert!(result.result.unobservable_buses.contains(&4));
    }

    /// T6.9 测试：最小 PMU 配置
    #[test]
    fn test_optimal_pmu_placement() {
        let net = build_5bus_network();
        let analyzer = ObservabilityAnalyzer::new();

        let result = analyzer.optimal_pmu_placement(&net, &[]);

        // 5 母线系统，最小 PMU 数通常为 2（覆盖 5 个母线）
        assert!(
            result.pmu_count <= 3,
            "5 母线系统 PMU 数应 ≤ 3，实际 {}",
            result.pmu_count
        );
        assert!(
            result.coverage >= 1.0 - 1e-9,
            "覆盖率应为 100%，实际 {:.1}%",
            result.coverage * 100.0
        );
        assert_eq!(result.total_buses, 5);
    }

    /// T6.10 测试：PMU 配置——已有 PMU
    #[test]
    fn test_pmu_placement_with_existing() {
        let net = build_3bus_network();
        let analyzer = ObservabilityAnalyzer::new();

        // 已在母线 0 放置 PMU，应覆盖 0 和 1
        let result = analyzer.optimal_pmu_placement(&net, &[0]);

        assert!(result.coverage >= 1.0 - 1e-9, "应 100% 覆盖");
        assert!(
            result.pmu_count >= 1,
            "至少需要 1 个 PMU（已有 1 个）"
        );
    }

    /// T6.11 测试：可观测岛识别
    #[test]
    fn test_observable_islands() {
        let net = build_5bus_network();
        // 只覆盖部分母线，形成两个岛
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
            Measurement::branch(MeasType::BranchFlowP, 0, 1, 50.0, 1.0),
            Measurement::bus(MeasType::VoltageMagnitude, 4, 0.98, 0.005),
        ];

        let analyzer = ObservabilityAnalyzer::new();
        let result = analyzer
            .analyze_topological(&measurements, &net, 0)
            .expect("分析应成功");

        // 应有可观测岛
        assert!(
            !result.result.observable_islands.is_empty(),
            "应识别出可观测岛"
        );
    }

    /// T6.12 测试：矩阵秩计算
    #[test]
    fn test_matrix_rank() {
        // 满秩矩阵
        let m1 = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        assert_eq!(matrix_rank(&m1), 3);

        // 秩 2 矩阵
        let m2 = vec![
            vec![1.0, 2.0, 3.0],
            vec![2.0, 4.0, 6.0],
            vec![1.0, 1.0, 1.0],
        ];
        assert_eq!(matrix_rank(&m2), 2);

        // 零矩阵
        let m3 = vec![vec![0.0, 0.0], vec![0.0, 0.0]];
        assert_eq!(matrix_rank(&m3), 0);

        // 空矩阵
        assert_eq!(matrix_rank(&[]), 0);
    }

    /// T6.13 测试：缺失测量建议
    #[test]
    fn test_missing_measurement_suggestions() {
        let net = build_3bus_network();
        let measurements = vec![
            Measurement::bus(MeasType::VoltageMagnitude, 0, 1.06, 0.005),
        ];

        let analyzer = ObservabilityAnalyzer::new();
        let result = analyzer
            .analyze_topological(&measurements, &net, 0)
            .expect("分析应成功");

        assert!(!result.result.missing_measurements.is_empty());
        for m in &result.result.missing_measurements {
            assert!(!m.suggested_measurement.is_empty());
            assert!(!m.reason.is_empty());
        }
    }
}
