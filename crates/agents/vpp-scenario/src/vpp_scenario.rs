//! VPP 场景核心数据模型、trait 定义与 VppScenarioImpl 实现.
//!
//! 本文件包含数据模型（VppRole / EdgeNode / VppAggregation / VppError）、
//! VppScenario trait 定义与 VppScenarioImpl 实现（集群组建、Coordinator 选举、
//! 聚合计算、DR 响应仿真与响应时间测量、出口标准评估）。

use alloc::format;
use alloc::vec::Vec;

use crate::dr_participation::{DrEvent, DrHandler};
use crate::exit_report::{ExitCriterion, ExitEvidence, ExitReportBuilder, Phase2ExitReport};

/// VPP 集群节点角色.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VppRole {
    /// 协调者：负责联邦聚合、DR 事件调度与出口标准评估
    Coordinator,
    /// 边缘盒子：常规分布式能源接入节点
    EdgeBox,
    /// 储能节点：具备充放电能力的储能设备节点
    Storage,
    /// 需求响应平台：外部 DR 事件来源
    DrPlatform,
}

/// 边缘节点描述.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeNode {
    /// 节点唯一标识
    pub node_id: u32,
    /// 节点角色
    pub role: VppRole,
    /// 节点地址（≤ 64 字节）
    pub address: heapless::String<64>,
    /// 额定容量（kW）
    pub capacity_kw: f64,
    /// 荷电状态，取值范围 [0, 1]
    pub soc: f64,
    /// 是否在线
    pub online: bool,
}

/// VPP 聚合结果快照.
#[derive(Debug, Clone, PartialEq)]
pub struct VppAggregation {
    /// 聚合总容量（kW）
    pub total_capacity_kw: f64,
    /// 在线节点数
    pub online_nodes: usize,
    /// 当前可用容量（kW）
    pub available_kw: f64,
    /// 最近一次 DR 事件响应延迟（毫秒）
    pub response_latency_ms: u64,
}

/// VPP 场景错误类型.
#[derive(Debug, Clone, PartialEq)]
pub enum VppError {
    /// 节点离线
    NodeOffline {
        /// 离线节点标识
        id: u32,
    },
    /// 聚合可用容量不足
    InsufficientCapacity {
        /// 需求容量（kW）
        need: f64,
        /// 可用容量（kW）
        available: f64,
    },
    /// DR 响应超时
    ResponseTimeout {
        /// 响应时限预算（毫秒）
        deadline_ms: u64,
    },
    /// Coordinator 选举失败（节点集中无 Coordinator 角色）
    CoordinatorElectionFailed,
    /// DR 平台不可达
    DrPlatformUnreachable,
    /// 出口标准验证失败
    ExitCriteriaFailed {
        /// 未通过的出口标准编号
        id: u32,
    },
}

/// VPP 场景端到端验证接口.
///
/// 覆盖集群组建、DR 事件仿真、响应时间测量、出口标准评估与报告生成。
/// 实现见 [`VppScenarioImpl`]。
pub trait VppScenario {
    /// 组建 VPP 集群：存储节点集并选举首个 online 的 role=Coordinator 节点
    /// 为协调者（离线节点不参与选举，支撑崩溃重选）；节点集中无在线
    /// Coordinator 时返回 `Err(CoordinatorElectionFailed)`；
    /// 同时初始化 10 项 Phase 2 出口标准。
    fn setup_cluster(&mut self, nodes: &[EdgeNode]) -> Result<(), VppError>;

    /// 仿真一次 DR 事件：容量校验 → 比例分配 → 状态机推进至 Completed；
    /// 响应延迟经确定性仿真模型计算并记录于返回的聚合快照；
    /// 超过 `event.deadline_ms` 返回 `Err(ResponseTimeout)`。
    fn simulate_dr_event(
        &mut self,
        event: DrEvent,
        now_ms: u64,
    ) -> Result<VppAggregation, VppError>;

    /// 返回最近一次 DR 事件的响应延迟（毫秒）。
    fn measure_response_time(&self) -> u64;

    /// 逐项评估 10 项 Phase 2 出口标准并返回评估结果列表。
    fn run_exit_criteria(&mut self) -> alloc::vec::Vec<ExitCriterion>;

    /// 汇总生成 Phase 2 出口验证报告。
    fn generate_report(&self) -> Phase2ExitReport;
}

/// VPP 场景端到端验证实现.
///
/// D1 确定性延迟模型：`link_latency_ms` / `coord_processing_ms` 构造注入，
/// DR 响应延迟 = `coord_processing_ms + 2 × link_latency_ms`（协调处理 + 链路
/// 往返），替代蓝图 `crate::current_time_ms()`（no_std 无系统时钟）。
pub struct VppScenarioImpl {
    /// 集群节点集
    nodes: Vec<EdgeNode>,
    /// 已选举的 Coordinator 节点标识（未组建集群时为 None）
    coordinator_id: Option<u32>,
    /// 最近一次 DR 事件响应延迟（毫秒；0 表示尚未执行 DR）
    last_response_ms: u64,
    /// DR 事件状态机
    dr_handler: DrHandler,
    /// Phase 2 出口标准报告构建器
    exit: ExitReportBuilder,
    /// 链路单程延迟（毫秒，D1 构造注入）
    link_latency_ms: u64,
    /// Coordinator 处理延迟（毫秒，D1 构造注入）
    coord_processing_ms: u64,
}

impl VppScenarioImpl {
    /// 创建场景实例：注入确定性延迟模型参数（D1）.
    pub fn new(link_latency_ms: u64, coord_processing_ms: u64) -> Self {
        Self {
            nodes: Vec::new(),
            coordinator_id: None,
            last_response_ms: 0,
            dr_handler: DrHandler::new(),
            exit: ExitReportBuilder::new(),
            link_latency_ms,
            coord_processing_ms,
        }
    }

    /// DR 状态机可变访问（测试与外部驱动用）.
    pub fn dr_handler_mut(&mut self) -> &mut DrHandler {
        &mut self.dr_handler
    }

    /// 注册第 4~10 项出口标准的外部验证证据（透传 ExitReportBuilder，D4）.
    pub fn register_evidence(&mut self, id: u32, ev: ExitEvidence) -> bool {
        self.exit.register_evidence(id, ev)
    }

    /// 已选举的 Coordinator 节点标识.
    pub fn coordinator_id(&self) -> Option<u32> {
        self.coordinator_id
    }

    /// 集群节点集只读切片.
    pub fn nodes(&self) -> &[EdgeNode] {
        &self.nodes
    }

    /// 聚合容量计算.
    ///
    /// 在线节点计入 online_nodes 与 total_capacity_kw；available_kw 仅累加
    /// online && soc > 0.1 的节点容量（蓝图 §4.5 同口径）；
    /// response_latency_ms 取最近一次 DR 响应延迟。
    fn compute_aggregation(&self) -> VppAggregation {
        let mut online_nodes = 0usize;
        let mut total_capacity_kw = 0.0f64;
        let mut available_kw = 0.0f64;
        for node in self.nodes.iter().filter(|n| n.online) {
            online_nodes += 1;
            total_capacity_kw += node.capacity_kw;
            if node.soc > 0.1 {
                available_kw += node.capacity_kw;
            }
        }
        VppAggregation {
            total_capacity_kw,
            online_nodes,
            available_kw,
            response_latency_ms: self.last_response_ms,
        }
    }
}

impl VppScenario for VppScenarioImpl {
    fn setup_cluster(&mut self, nodes: &[EdgeNode]) -> Result<(), VppError> {
        self.nodes = nodes.to_vec();
        // 选举第一个 online 的 role=Coordinator 节点（离线 Coordinator 不参与
        // 选举，支撑崩溃重选场景）；无在线 Coordinator → 选举失败
        self.coordinator_id = nodes
            .iter()
            .find(|n| n.role == VppRole::Coordinator && n.online)
            .map(|n| n.node_id);
        if self.coordinator_id.is_some() {
            Ok(())
        } else {
            Err(VppError::CoordinatorElectionFailed)
        }
    }

    fn simulate_dr_event(
        &mut self,
        event: DrEvent,
        _now_ms: u64,
    ) -> Result<VppAggregation, VppError> {
        // _now_ms 仅承载事件时间戳语义（保持与蓝图接口一致）；D1 确定性延迟
        // 模型不读取系统时钟，响应延迟完全由构造注入参数计算。
        // 1. 容量校验：聚合可用容量不足 → InsufficientCapacity
        let agg = self.compute_aggregation();
        if agg.available_kw < event.target_kw.abs() {
            return Err(VppError::InsufficientCapacity {
                need: event.target_kw.abs(),
                available: agg.available_kw,
            });
        }
        // 2. 按容量比例生成调度计划：仅 online && soc > 0.1 节点参与，
        //    share = target_kw × capacity_kw / total（total 为参与节点容量和）
        let mut total = 0.0f64;
        for node in self.nodes.iter().filter(|n| n.online && n.soc > 0.1) {
            total += node.capacity_kw;
        }
        let mut plan: heapless::Vec<(u32, f64), 16> = heapless::Vec::new();
        let mut participant_ids: heapless::Vec<u32, 16> = heapless::Vec::new();
        for node in self.nodes.iter().filter(|n| n.online && n.soc > 0.1) {
            let share = event.target_kw * node.capacity_kw / total;
            // 调度计划容量上限 16：参与节点超 16 时仅取前 16 个
            //（本场景集群规模 5~10 台，不会触发截断）
            if plan.push((node.node_id, share)).is_err() {
                break;
            }
            let _ = participant_ids.push(node.node_id);
        }
        // 3. 驱动 DR 状态机：接收 → 调度
        self.dr_handler.receive(event.clone());
        let _ = self.dr_handler.dispatch(plan);
        // 4. D1 确定性响应延迟 = 协调处理 + 链路往返
        let response = self.coord_processing_ms + 2 * self.link_latency_ms;
        // 5. 超时判定：超过响应时限预算 → 状态机置 Expired 并返回超时错误。
        //    超时检查必须在确认/完成前执行：DrHandler::check_timeout 仅对
        //    Received/Dispatched 状态生效，超时事件视为未获全部节点确认。
        if response > event.deadline_ms {
            self.dr_handler.check_timeout(response);
            return Err(VppError::ResponseTimeout {
                deadline_ms: event.deadline_ms,
            });
        }
        // 6. 全部参与节点确认 → 完成结算，记录响应延迟并返回更新后的聚合快照
        for node_id in participant_ids.iter() {
            self.dr_handler.ack(*node_id);
        }
        let _ = self.dr_handler.complete();
        self.last_response_ms = response;
        Ok(self.compute_aggregation())
    }

    fn measure_response_time(&self) -> u64 {
        self.last_response_ms
    }

    fn run_exit_criteria(&mut self) -> Vec<ExitCriterion> {
        let agg = self.compute_aggregation();
        // 第 1 项：VPP 节点数 ≥ 5 台
        let _ = self.exit.evaluate(
            1,
            &format!("{} 台在线", agg.online_nodes),
            agg.online_nodes >= 5,
        );
        // 第 2 项：聚合容量 ≥ 100kW
        let _ = self.exit.evaluate(
            2,
            &format!("{:.1} kW", agg.total_capacity_kw),
            agg.total_capacity_kw >= 100.0,
        );
        // 第 3 项：DR 响应时间 < 30s；尚未执行 DR（last_response_ms == 0）时
        // 不得谎报 "0 ms < 30s"（D4 精神），标记"未执行 DR"且不通过
        if self.last_response_ms == 0 {
            let _ = self.exit.evaluate(3, "未执行 DR", false);
        } else {
            let _ = self.exit.evaluate(
                3,
                &format!("{} ms", self.last_response_ms),
                self.last_response_ms < 30_000,
            );
        }
        // 第 4~10 项由 register_evidence 证据注入，build 时统一汇总
        self.exit.build().criteria
    }

    fn generate_report(&self) -> Phase2ExitReport {
        self.exit.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DrState;

    /// 构造测试节点
    fn make_node(
        node_id: u32,
        role: VppRole,
        capacity_kw: f64,
        soc: f64,
        online: bool,
    ) -> EdgeNode {
        let mut address = heapless::String::<64>::new();
        let _ = address.push_str("edge://vpp-node");
        EdgeNode {
            node_id,
            role,
            address,
            capacity_kw,
            soc,
            online,
        }
    }

    /// 构造 8 节点集群：node_id 1~7 为 EdgeBox（各 50kW、soc=0.8、在线），
    /// node_id 8 为 Coordinator（容量 0kW、soc=0.0 不参与调度、在线）。
    /// 聚合口径：total = 7 × 50 = 350kW（Coordinator 容量 0 无贡献）。
    fn make_cluster() -> Vec<EdgeNode> {
        let mut nodes = Vec::new();
        for id in 1..=7u32 {
            nodes.push(make_node(id, VppRole::EdgeBox, 50.0, 0.8, true));
        }
        nodes.push(make_node(8, VppRole::Coordinator, 0.0, 0.0, true));
        nodes
    }

    /// DR 事件样例：降载 300kW，响应时限预算 30s
    fn make_event() -> DrEvent {
        DrEvent {
            event_id: 1,
            timestamp_ms: 1_000,
            target_kw: -300.0,
            duration_min: 30,
            deadline_ms: 30_000,
            reward: 128.5,
        }
    }

    // VPP1：8 节点聚合——online_nodes=8、total=350kW（7×50，Coordinator 容量 0）、
    // available=350kW
    #[test]
    fn vpp1_aggregation_eight_nodes() {
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        assert!(s.setup_cluster(&make_cluster()).is_ok());
        assert_eq!(s.coordinator_id(), Some(8));
        assert_eq!(s.nodes().len(), 8);
        let agg = s.compute_aggregation();
        assert_eq!(agg.online_nodes, 8);
        assert!((agg.total_capacity_kw - 350.0).abs() < 1e-9);
        assert!((agg.available_kw - 350.0).abs() < 1e-9);
        assert_eq!(agg.response_latency_ms, 0);
    }

    // VPP2：无 Coordinator → setup_cluster Err(CoordinatorElectionFailed)
    #[test]
    fn vpp2_no_coordinator_election_fails() {
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        let nodes: Vec<EdgeNode> = (1..=5u32)
            .map(|id| make_node(id, VppRole::EdgeBox, 50.0, 0.8, true))
            .collect();
        assert_eq!(
            s.setup_cluster(&nodes),
            Err(VppError::CoordinatorElectionFailed)
        );
        assert_eq!(s.coordinator_id(), None);
    }

    // VPP3：soc≤0.1 节点不计入 available，但计入 total
    #[test]
    fn vpp3_low_soc_filtered_from_available() {
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        let mut nodes = make_cluster();
        // node 3 边界 soc=0.1、node 4 低 soc=0.05：计入 total，不计入 available
        nodes[2].soc = 0.1;
        nodes[3].soc = 0.05;
        assert!(s.setup_cluster(&nodes).is_ok());
        let agg = s.compute_aggregation();
        assert!((agg.total_capacity_kw - 350.0).abs() < 1e-9);
        assert!((agg.available_kw - 250.0).abs() < 1e-9);
    }

    // VPP4：DR 全流程——link=8000/coord=2000，deadline=30000，target=-300 → Ok，
    // measure_response_time()=18000，DrHandler 终态 Completed
    #[test]
    fn vpp4_dr_full_flow_response_18000() {
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        assert!(s.setup_cluster(&make_cluster()).is_ok());
        let agg = s
            .simulate_dr_event(make_event(), 1_000)
            .expect("DR 事件应执行成功");
        assert_eq!(s.measure_response_time(), 18_000);
        assert_eq!(agg.response_latency_ms, 18_000);
        assert_eq!(s.dr_handler_mut().state(), DrState::Completed);
        assert!(s.dr_handler_mut().is_terminal());
        // 7 台 EdgeBox 等容量均摊 -300kW
        assert_eq!(s.dr_handler_mut().plan().len(), 7);
    }

    // VPP5：比例分配容差 1e-6——3 个参与节点 100/150/100（总 350）对 target=-300，
    // 份额 -85.714…/-128.571…/-85.714…，allocated 总和 ≈ -300
    #[test]
    fn vpp5_proportional_allocation_tolerance() {
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        let nodes = vec![
            make_node(1, VppRole::EdgeBox, 100.0, 0.8, true),
            make_node(2, VppRole::EdgeBox, 150.0, 0.8, true),
            make_node(3, VppRole::EdgeBox, 100.0, 0.8, true),
            make_node(8, VppRole::Coordinator, 0.0, 0.0, true),
        ];
        assert!(s.setup_cluster(&nodes).is_ok());
        assert!(s.simulate_dr_event(make_event(), 1_000).is_ok());
        let plan = s.dr_handler_mut().plan();
        assert_eq!(plan.len(), 3);
        let total = 350.0f64;
        let expected = [
            (1u32, -300.0 * 100.0 / total),
            (2u32, -300.0 * 150.0 / total),
            (3u32, -300.0 * 100.0 / total),
        ];
        let mut allocated = 0.0f64;
        for (idx, &(id, share)) in plan.iter().enumerate() {
            assert_eq!(id, expected[idx].0);
            assert!(
                (share - expected[idx].1).abs() < 1e-6,
                "node {} 份额 {} 与期望 {} 偏差超限",
                id,
                share,
                expected[idx].1
            );
            allocated += share;
        }
        assert!((allocated - (-300.0)).abs() < 1e-6);
    }

    // VPP6：容量不足——target=-500 对 available=350 →
    // Err(InsufficientCapacity{need:500.0, available:350.0})
    #[test]
    fn vpp6_insufficient_capacity() {
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        assert!(s.setup_cluster(&make_cluster()).is_ok());
        let mut event = make_event();
        event.target_kw = -500.0;
        assert_eq!(
            s.simulate_dr_event(event, 1_000),
            Err(VppError::InsufficientCapacity {
                need: 500.0,
                available: 350.0
            })
        );
        // 容量校验失败不推进状态机、不记录响应时间
        assert_eq!(s.measure_response_time(), 0);
        assert_eq!(s.dr_handler_mut().state(), DrState::Idle);
    }

    // VPP7：超时——link=20000/coord=2000 → response=42000 > deadline=30000 →
    // Err(ResponseTimeout{deadline_ms:30000}) 且 dr_handler state=Expired
    #[test]
    fn vpp7_response_timeout_expires() {
        let mut s = VppScenarioImpl::new(20_000, 2_000);
        assert!(s.setup_cluster(&make_cluster()).is_ok());
        assert_eq!(
            s.simulate_dr_event(make_event(), 1_000),
            Err(VppError::ResponseTimeout {
                deadline_ms: 30_000
            })
        );
        assert_eq!(s.dr_handler_mut().state(), DrState::Expired);
        assert!(s.dr_handler_mut().is_terminal());
        // 超时事件不记录为有效响应
        assert_eq!(s.measure_response_time(), 0);
    }

    // VPP8：出口报告——setup+DR 后第 1~3 项按实计算通过，4~10 未注册
    // actual="未验证" passed=false，all_passed=false；未跑 DR 时第 3 项
    // actual="未执行 DR" passed=false
    #[test]
    fn vpp8_exit_report_partial_and_unexecuted_dr() {
        // 已执行 DR：第 1~3 项按实通过，第 4~10 项未注册证据不谎报
        let mut s = VppScenarioImpl::new(8_000, 2_000);
        assert!(s.setup_cluster(&make_cluster()).is_ok());
        assert!(s.simulate_dr_event(make_event(), 1_000).is_ok());
        let criteria = s.run_exit_criteria();
        assert_eq!(criteria.len(), 10);
        assert!(criteria[0].passed);
        assert_eq!(criteria[0].actual.as_str(), "8 台在线");
        assert!(criteria[1].passed);
        assert_eq!(criteria[1].actual.as_str(), "350.0 kW");
        assert!(criteria[2].passed);
        assert_eq!(criteria[2].actual.as_str(), "18000 ms");
        for c in criteria.iter().skip(3) {
            assert!(!c.passed, "id={} 未注册证据不得通过", c.id);
            assert_eq!(c.actual.as_str(), "未验证");
        }
        let report = s.generate_report();
        assert_eq!(report.exit_criteria_total, 10);
        assert_eq!(report.exit_criteria_passed, 3);
        assert!(!report.all_passed);

        // 未执行 DR：第 3 项不得谎报 0ms < 30s（D4 精神）
        let mut s2 = VppScenarioImpl::new(8_000, 2_000);
        assert!(s2.setup_cluster(&make_cluster()).is_ok());
        let criteria2 = s2.run_exit_criteria();
        assert!(criteria2[0].passed);
        assert!(criteria2[1].passed);
        assert!(!criteria2[2].passed);
        assert_eq!(criteria2[2].actual.as_str(), "未执行 DR");
    }
}
