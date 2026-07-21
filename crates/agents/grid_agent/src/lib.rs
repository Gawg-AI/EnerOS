//! EnerOS v0.82.0 Grid Agent — 电网状态感知 Agent.
//!
//! Phase 2 多机联邦：实现电网状态感知 Agent，负责电网运行参数的周期性采样、
//! 异常检测与状态/告警发布。沿用 device-agent 模式，实现 `AgentRuntime` trait +
//! sync `on_tick(now_ms: u64)` API，作为电网侧感知节点接入 Agent 联邦。
//!
//! # 核心类型
//!
//! - [`GridAgent`] — 电网状态感知 Agent（实现 `AgentRuntime`）
//! - [`GridState`] — 电网状态（12 字段：频率/三相电压/三相电流/功率/时戳/质量）
//! - [`DataQuality`] — 数据质量（Good/Invalid/Uncertain，保守默认 Invalid）
//! - [`GridSampler`] / [`MockGridSampler`] — 采样器接口 + Mock 实现
//! - [`GridPublisher`] / [`MockGridPublisher`] — 发布器接口 + Mock 实现
//! - [`GridError`] — 错误类型（SampleFailed/PublishFailed/InvalidConfig）
//! - [`is_valid_grid`] / [`default_anomaly_detectors`] — 校验与默认异常检测器
//!
//! # 偏差声明（D1~D14，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `log::info!` / `log::warn!` / `log::error!` | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57/v0.64/v0.70/v0.71/v0.81 一致 |
//! | **D2** | `SystemTime::now()` / `UNIX_EPOCH` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57/v0.64/v0.70/v0.71 一致 |
//! | **D3** | `Instant::now()` 采样节拍 | 外部驱动 `on_tick(now_ms)` | no_std 无 `Instant`；由调度器/系统 Agent 驱动 tick |
//! | **D4** | `async fn sample()` | sync `fn sample(&mut self, now_ms) -> Result` | no_std 单线程，无 async runtime；沿用 device-agent sync 模式 |
//! | **D5** | DDS 主题发布（`dds::publish`） | `GridPublisher` trait 抽象 | `eneros-agent-bus-dds` 依赖较重；MVP 用 trait + Mock 解耦，具体 DDS 适配后续注入 |
//! | **D6** | `AgentRuntime` trait | 复用 `eneros-energy-market-agent::AgentRuntime` | 与 v0.72.0 Energy/Market Agent 统一运行时语义；避免重复定义 |
//! | **D7** | `AgentDescriptor { ..Default::default() }` | `AgentDescriptor::new(AgentType::Grid, name, now_ms)` | v0.33.0 `AgentDescriptor` 13 字段 + 构造器自动设置优先级/配额/信任等级 |
//! | **D8** | 异常检测闭包 `Box<dyn Fn(&GridState) -> bool>` | `fn(&GridState) -> bool` 函数指针 | no_std 无堆闭包开销；函数指针可存于 `Vec` 无需 `Box`；与 `register_anomaly_detector` 签名一致 |
//! | **D9** | `GridState` 含 `soc`/`device_status`/`alarms` | 12 字段纯电气量（频率/三相电压/三相电流/有功/无功/功率因数/时戳/质量） | Grid Agent 聚焦电网感知；储能 SoC 由 Energy Agent 管理；设备状态由 Device Agent 管理 |
//! | **D10** | `GridError` 5 变体（含 `Timeout`/`Disconnected`） | 3 变体（SampleFailed/PublishFailed/InvalidConfig） | MVP 收敛错误分类；超时/断连在采样器/发布器实现侧归一为 SampleFailed/PublishFailed |
//! | **D11** | 依赖 `eneros-protocol-abstract` | 不引入 | 蓝图约束：仅依赖 `eneros-agent` + `eneros-energy-market-agent`；采样/发布用本地 trait 抽象 |
//! | **D12** | 依赖 `eneros-agent-bus-dds` | 不引入 | 同 D5：`GridPublisher` trait 抽象 DDS 发布，具体适配后续注入 |
//! | **D13** | 依赖 `eneros-tsn-time` | 不引入 | 同 D2/D3：`now_ms` 由外部提供，无 TSN 时钟依赖 |
//! | **D14** | 依赖 `eneros-upa-model` | 不引入；本地定义 `GridState` | UPA 模型面向统一点表抽象；Grid Agent 仅需电气量，本地结构体自包含可测试 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*`，可交叉编译到 `aarch64-unknown-none`。
//! 禁止 `use std::*` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / `Instant::now()`。
//!
//! # 示例
//!
//! ```
//! use eneros_grid_agent::{DataQuality, GridState};
//!
//! let state = GridState {
//!     frequency: 50.0,
//!     voltage_a: 220.0,
//!     quality: DataQuality::Good,
//!     ..GridState::default()
//! };
//! assert!((state.frequency - 50.0).abs() < 1e-6);
//! assert_eq!(state.quality, DataQuality::Good);
//! assert_eq!(state.voltage_b, 0.0);
//! ```
//!
//! # v0.83.0 PCC 扩展
//!
//! v0.83.0 追加 PCC (Point of Common Coupling) 并网点管理模块 [`crate::pcc`]：
//! - [`PccManager`] / [`PccState`] / [`PccReading`] — PCC 状态管理
//! - [`BreakerStatus`] / [`PowerDirection`] / [`PccStatus`] — 枚举
//! - [`PccReader`] / [`MockPccReader`] — 读取器 trait + Mock
//! - [`compute_power_direction`] / [`compute_power_factor`] — 辅助函数
//!
//! ## v0.83.0 PCC 偏差声明 (D1~D14)
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | D1 | async fn update() | sync update(now_ms) | no_std 无 async runtime |
//! | D2 | pcc_id: String | pcc_id: u32 | no_std Copy 语义 |
//! | D3 | PointTable + format!() | PccReader trait + MockPccReader | 避免重依赖 |
//! | D4 | AgentError | 复用 GridError (SampleFailed) | surgical — 不新增错误类型 |
//! | D5 | 扩展 GridAgent 持有 PccManager | PccManager 独立组件 | surgical — 不破坏 v0.82.0 GridAgent |
//! | D6 | 状态防抖未明确 | Transitioning + debounce_ms | 最小防抖逻辑 |
//! | D7 | libm sqrt() | 手写 sqrt_f32 (牛顿迭代法) | no_std 下 f32::sqrt 不可用 |
//! | D8 | docs/phase2/ + config/ | docs/agents/ + configs/ | 工作区规则 §2.3.3 |
//! | D9 | tests/ 集成测试 | pcc.rs 内嵌单元测试 | 沿用 v0.82.0 模式 |
//! | D10 | BreakerStatus 4 变体 | 保持 | Tripped ≠ Open |
//! | D11 | PowerDirection 符号 | P>0 Import | §8.5 约定 |
//! | D12 | PccStatus 3 变体 | 保持 | Transitioning 用于防抖 |
//! | D13 | PccState 含 String | pcc_id: u32 全 Copy | D2 派生 |
//! | D14 | 无 PccReading | 新增一次性读取结构体 | D3 原子性 |
//!
//! # v0.84.0 并离网切换扩展
//!
//! v0.84.0 追加 2 个模块：
//! - [`crate::island_detect`] — 孤岛检测（双源融合 + 连续确认）
//!   - [`IslandResult`] / [`IslandConfig`] / [`IslandDetector`]
//! - [`crate::transfer`] — 切换状态机 + RTOS 快平面通道抽象
//!   - [`TransferState`] / [`TransferReason`] / [`TransferCommand`] / [`TransferRecord`]
//!   - [`TransferError`] / [`RtosChannel`] / [`MockRtosChannel`] / [`GridTransfer`]
//!
//! ## v0.84.0 偏差声明 (D1~D14)
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | D1 | Instant::now() + start.elapsed().as_millis() | now_ms: u64 参数 + RtosChannel 返回 elapsed_ms | no_std 无 Instant |
//! | D2 | RtosCommandChannel 具体类型 + wait_ack(Duration) | RtosChannel trait + MockRtosChannel | 沿用 v0.82.0 D5 trait 抽象 |
//! | D3 | detect(&self, &GridState) 单源 | detect(&mut self, &PccState, &GridState) 双源融合 | 蓝图 §5.2 多源融合 |
//! | D4 | self.last_grid_state() 未定义 | check_and_transfer(pcc, grid, now_ms) 显式参数 | 蓝图不完整 |
//! | D5 | “保持原状态或强制跳闸”未明确 | 通道失败时 state 回滚到 from | 保守路径 |
//! | D6 | “连续 3 次确认”未明确 | consecutive_count + confirmation_threshold=3 | 满足 §4.4 复检 |
//! | D7 | TransferError 未定义 | 4 变体 InvalidTarget/AlreadyInTarget/ChannelTimeout/ChannelError | 边界条件完备 |
//! | D8 | error!() 日志 | 移除；通过返回值传递 | no_std 无 log |
//! | D9 | docs/phase2/ + tests/ | docs/agents/ + 内嵌单元测试 | 工作区规则 §2.3.3 |
//! | D10 | 2 文件结构 | 保持 | 关注点分离 |
//! | D11 | TransferState 与 PccStatus 名称重叠 | 保持两套枚举 | 语义不同：观测态 vs 控制态 |
//! | D12 | Option<TransferRecord> 字段 | 保持 | Copy 语义，无堆分配 |
//! | D13 | 性能 < 100ms | 标注“硬件集成阶段验收” | Mock 通道无法验证真实延迟 |
//! | D14 | 未定义 IslandConfig | 新增 5 字段配置结构体 | D6 派生：阈值可配置 |

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod island_detect;
pub mod pcc;
pub mod publisher;
pub mod sampler;
pub mod state;
pub mod transfer;

use eneros_energy_market_agent::AgentRuntimeError;
pub use island_detect::{IslandConfig, IslandDetector, IslandResult};
pub use pcc::{
    compute_power_direction, compute_power_factor, BreakerStatus, MockPccReader, PccManager,
    PccReader, PccReading, PccState, PccStatus, PowerDirection,
};
pub use publisher::{publish_state, GridPublisher, MockGridPublisher};
pub use sampler::{default_anomaly_detectors, is_valid_grid, GridSampler, MockGridSampler};
pub use state::{DataQuality, GridAgent, GridState};
pub use transfer::{
    GridTransfer, MockRtosChannel, RtosChannel, TransferCommand, TransferError, TransferReason,
    TransferRecord, TransferState,
};

/// Grid Agent 错误类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridError {
    /// 采样失败.
    SampleFailed,
    /// 发布失败.
    PublishFailed,
    /// 配置无效.
    InvalidConfig,
}

impl From<GridError> for AgentRuntimeError {
    fn from(e: GridError) -> Self {
        let msg = alloc::string::String::from(match e {
            GridError::SampleFailed => "grid sample failed",
            GridError::PublishFailed => "grid publish failed",
            GridError::InvalidConfig => "grid invalid config",
        });
        AgentRuntimeError::DeviceError(msg)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use eneros_agent::{AgentState, AgentType};
    use eneros_energy_market_agent::{AgentRuntime, HeartbeatStatus};

    use super::*;

    // ===== 辅助函数 =====

    /// 构造频率=50.0/电压=220.0 的正常电网状态.
    fn make_normal_state() -> GridState {
        GridState {
            frequency: 50.0,
            voltage_a: 220.0,
            quality: DataQuality::Good,
            ..GridState::default()
        }
    }

    /// 异常检测器：频率偏低.
    fn freq_low(s: &GridState) -> bool {
        s.frequency < 49.5
    }

    /// 异常检测器：恒返回 true.
    fn always_anomaly(_s: &GridState) -> bool {
        true
    }

    /// 异常检测器：恒返回 false.
    fn never_anomaly(_s: &GridState) -> bool {
        false
    }

    // ===== T1: GridState::default() 全零 + Invalid =====
    #[test]
    fn t1_grid_state_default() {
        let s = GridState::default();
        assert!(s.frequency.abs() < 1e-9);
        assert!(s.voltage_a.abs() < 1e-9);
        assert!(s.voltage_b.abs() < 1e-9);
        assert!(s.voltage_c.abs() < 1e-9);
        assert!(s.current_a.abs() < 1e-9);
        assert!(s.current_b.abs() < 1e-9);
        assert!(s.current_c.abs() < 1e-9);
        assert!(s.active_power.abs() < 1e-9);
        assert!(s.reactive_power.abs() < 1e-9);
        assert!(s.power_factor.abs() < 1e-9);
        assert_eq!(s.timestamp, 0);
        assert_eq!(s.quality, DataQuality::Invalid);
    }

    // ===== T2: GridState 字段可读访问 =====
    #[test]
    fn t2_grid_state_field_access() {
        let s = GridState {
            frequency: 49.8,
            voltage_a: 221.5,
            current_a: 12.3,
            active_power: 4.2,
            reactive_power: 0.7,
            power_factor: 0.98,
            timestamp: 12345,
            quality: DataQuality::Good,
            ..GridState::default()
        };
        assert!((s.frequency - 49.8).abs() < 1e-6);
        assert!((s.voltage_a - 221.5).abs() < 1e-6);
        assert!((s.current_a - 12.3).abs() < 1e-6);
        assert!((s.active_power - 4.2).abs() < 1e-6);
        assert!((s.reactive_power - 0.7).abs() < 1e-6);
        assert!((s.power_factor - 0.98).abs() < 1e-6);
        assert_eq!(s.timestamp, 12345);
        assert_eq!(s.quality, DataQuality::Good);
    }

    // ===== T3: GridState::default() == GridState::default() =====
    #[test]
    fn t3_grid_state_partial_eq_consistency() {
        let a = GridState::default();
        let b = GridState::default();
        assert_eq!(a, b);
        assert_eq!(a, a);
    }

    // ===== T4: DataQuality::default() 返回 Invalid =====
    #[test]
    fn t4_data_quality_default_invalid() {
        assert_eq!(DataQuality::default(), DataQuality::Invalid);
    }

    // ===== T5: DataQuality 3 变体 Debug 输出非空 =====
    #[test]
    fn t5_data_quality_debug_nonempty() {
        for q in [
            DataQuality::Good,
            DataQuality::Invalid,
            DataQuality::Uncertain,
        ] {
            let s = alloc::format!("{:?}", q);
            assert!(!s.is_empty());
        }
    }

    // ===== T6: MockGridSampler::new(state) fail == false =====
    #[test]
    fn t6_mock_grid_sampler_new_not_fail() {
        let sampler = MockGridSampler::new(make_normal_state());
        assert!(!sampler.fail);
    }

    // ===== T7: MockGridSampler::new_failing() fail == true =====
    #[test]
    fn t7_mock_grid_sampler_new_failing() {
        let sampler = MockGridSampler::new_failing();
        assert!(sampler.fail);
    }

    // ===== T8: MockGridSampler::sample 成功路径 =====
    #[test]
    fn t8_mock_grid_sampler_sample_ok() {
        let mut sampler = MockGridSampler::new(make_normal_state());
        let result = sampler.sample(9999);
        assert!(result.is_ok());
        let s = result.unwrap();
        assert!((s.frequency - 50.0).abs() < 1e-6);
        assert_eq!(s.timestamp, 9999);
    }

    // ===== T9: MockGridSampler::sample 失败路径 =====
    #[test]
    fn t9_mock_grid_sampler_sample_err() {
        let mut sampler = MockGridSampler::new_failing();
        let result = sampler.sample(1000);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), GridError::SampleFailed);
    }

    // ===== T10: MockGridSampler::with_state builder =====
    #[test]
    fn t10_mock_grid_sampler_with_state_builder() {
        let sampler = MockGridSampler::new(GridState::default()).with_state(make_normal_state());
        assert!(!sampler.fail);
        assert!((sampler.next_state.frequency - 50.0).abs() < 1e-6);
    }

    // ===== T11: MockGridPublisher::new() 空 Vec + false =====
    #[test]
    fn t11_mock_grid_publisher_new_empty() {
        let pub_ = MockGridPublisher::new();
        assert!(pub_.published_states.is_empty());
        assert!(pub_.published_alerts.is_empty());
        assert!(!pub_.fail_state);
        assert!(!pub_.fail_alert);
    }

    // ===== T12: MockGridPublisher::new_failing_state() =====
    #[test]
    fn t12_mock_grid_publisher_new_failing_state() {
        let pub_ = MockGridPublisher::new_failing_state();
        assert!(pub_.fail_state);
        assert!(!pub_.fail_alert);
    }

    // ===== T13: MockGridPublisher::new_failing_alert() =====
    #[test]
    fn t13_mock_grid_publisher_new_failing_alert() {
        let pub_ = MockGridPublisher::new_failing_alert();
        assert!(!pub_.fail_state);
        assert!(pub_.fail_alert);
    }

    // ===== T14: MockGridPublisher::publish_state 成功路径 =====
    #[test]
    fn t14_mock_grid_publisher_publish_state_ok() {
        let mut pub_ = MockGridPublisher::new();
        let state = make_normal_state();
        let result = pub_.publish_state(&state);
        assert!(result.is_ok());
        assert_eq!(pub_.published_states.len(), 1);
        assert_eq!(pub_.published_states[0], state);
    }

    // ===== T15: MockGridPublisher::publish_state 失败路径 =====
    #[test]
    fn t15_mock_grid_publisher_publish_state_err() {
        let mut pub_ = MockGridPublisher::new_failing_state();
        let state = make_normal_state();
        let result = pub_.publish_state(&state);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), GridError::PublishFailed);
        assert!(pub_.published_states.is_empty());
    }

    // ===== T16: MockGridPublisher::publish_alert 成功路径 =====
    #[test]
    fn t16_mock_grid_publisher_publish_alert_ok() {
        let mut pub_ = MockGridPublisher::new();
        let state = make_normal_state();
        let result = pub_.publish_alert(&state);
        assert!(result.is_ok());
        assert_eq!(pub_.published_alerts.len(), 1);
        assert_eq!(pub_.published_alerts[0], state);
    }

    // ===== T17: MockGridPublisher::publish_alert 失败路径 =====
    #[test]
    fn t17_mock_grid_publisher_publish_alert_err() {
        let mut pub_ = MockGridPublisher::new_failing_alert();
        let state = make_normal_state();
        let result = pub_.publish_alert(&state);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), GridError::PublishFailed);
        assert!(pub_.published_alerts.is_empty());
    }

    // ===== T18: publish_state 辅助函数委托 =====
    #[test]
    fn t18_publish_state_helper_delegates() {
        let mut pub_ = MockGridPublisher::new();
        let state = make_normal_state();
        let result = publish_state(&mut pub_, &state);
        assert!(result.is_ok());
        assert_eq!(pub_.published_states.len(), 1);
    }

    // ===== T19: GridAgent::new 初始化 =====
    #[test]
    fn t19_grid_agent_new() {
        let agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            1000,
        );
        assert_eq!(agent.descriptor().agent_type, AgentType::Grid);
        assert_eq!(agent.state, GridState::default());
        assert_eq!(agent.agent_state, AgentState::Created);
        assert_eq!(agent.tick_count, 0);
    }

    // ===== T20: GridAgent::register_anomaly_detector =====
    #[test]
    fn t20_grid_agent_register_anomaly_detector() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        assert!(agent.anomaly_handlers.is_empty());
        agent.register_anomaly_detector(freq_low);
        assert_eq!(agent.anomaly_handlers.len(), 1);
    }

    // ===== T21: GridAgent::current_state 返回 &self.state =====
    #[test]
    fn t21_grid_agent_current_state() {
        let agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        assert!(agent.current_state().frequency.abs() < 1e-9);
        assert_eq!(agent.current_state().quality, DataQuality::Invalid);
    }

    // ===== T22: impl AgentRuntime::descriptor =====
    #[test]
    fn t22_grid_agent_descriptor() {
        let agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        assert_eq!(agent.descriptor().agent_type, AgentType::Grid);
        assert_eq!(agent.descriptor().name.as_str(), "grid");
    }

    // ===== T23: on_start → agent_state == Running =====
    #[test]
    fn t23_grid_agent_on_start() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        let result = agent.on_start(1000);
        assert!(result.is_ok());
        assert_eq!(agent.agent_state, AgentState::Running);
        assert_eq!(agent.on_heartbeat(1000), HeartbeatStatus::Alive);
    }

    // ===== T24: on_tick 成功采样 → tick_count == 1 + frequency == 50.0 =====
    #[test]
    fn t24_grid_agent_on_tick_success() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        assert_eq!(agent.tick_count, 1);
        assert!((agent.current_state().frequency - 50.0).abs() < 1e-6);
        assert_eq!(agent.current_state().timestamp, 2000);
    }

    // ===== T25: on_tick 采样失败 → Err + tick_count 不变 =====
    #[test]
    fn t25_grid_agent_on_tick_sample_fail() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new_failing()),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_err());
        assert_eq!(agent.tick_count, 0);
    }

    // ===== T26: on_tick 发布 state 失败 → Err + tick_count 不变 =====
    #[test]
    fn t26_grid_agent_on_tick_publish_state_fail() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new_failing_state()),
            100,
            0,
        );
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_err());
        assert_eq!(agent.tick_count, 0);
    }

    // ===== T27: on_tick 异常检测触发 alert 仍正常完成 =====
    #[test]
    fn t27_grid_agent_on_tick_anomaly_alert() {
        let low_freq_state = GridState {
            frequency: 49.0,
            ..GridState::default()
        };
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(low_freq_state)),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.register_anomaly_detector(freq_low);
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        assert_eq!(agent.tick_count, 1);
        assert!((agent.current_state().frequency - 49.0).abs() < 1e-6);
    }

    // ===== T28: on_tick 无异常时不影响流程 =====
    #[test]
    fn t28_grid_agent_on_tick_no_anomaly() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.register_anomaly_detector(freq_low);
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        assert_eq!(agent.tick_count, 1);
    }

    // ===== T29: 多个 anomaly_handlers 任一 true 即触发 =====
    #[test]
    fn t29_grid_agent_multiple_detectors_any_true() {
        let low_freq_state = GridState {
            frequency: 49.0,
            ..GridState::default()
        };
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(low_freq_state)),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.register_anomaly_detector(never_anomaly);
        agent.register_anomaly_detector(freq_low);
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_ok());
        assert_eq!(agent.tick_count, 1);
    }

    // ===== T30: on_tick alert 发布失败 → Err + tick_count 不变 =====
    #[test]
    fn t30_grid_agent_on_tick_alert_publish_fail() {
        let low_freq_state = GridState {
            frequency: 49.0,
            ..GridState::default()
        };
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(low_freq_state)),
            Box::new(MockGridPublisher::new_failing_alert()),
            100,
            0,
        );
        agent.register_anomaly_detector(always_anomaly);
        agent.on_start(1000).unwrap();
        let result = agent.on_tick(2000);
        assert!(result.is_err());
        assert_eq!(agent.tick_count, 0);
    }

    // ===== T31: on_stop → on_heartbeat 返回 Dead =====
    #[test]
    fn t31_grid_agent_on_stop() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.on_start(1000).unwrap();
        let result = agent.on_stop(2000);
        assert!(result.is_ok());
        assert_eq!(agent.agent_state, AgentState::Dead);
        assert_eq!(agent.on_heartbeat(2000), HeartbeatStatus::Dead);
    }

    // ===== T32: on_heartbeat Running → Alive =====
    #[test]
    fn t32_grid_agent_heartbeat_alive() {
        let mut agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        agent.on_start(1000).unwrap();
        assert_eq!(agent.on_heartbeat(2000), HeartbeatStatus::Alive);
    }

    // ===== T33: on_heartbeat 非 Running → Dead =====
    #[test]
    fn t33_grid_agent_heartbeat_dead() {
        let agent = GridAgent::new(
            "grid",
            Box::new(MockGridSampler::new(make_normal_state())),
            Box::new(MockGridPublisher::new()),
            100,
            0,
        );
        // Created 状态 → Dead
        assert_eq!(agent.on_heartbeat(1000), HeartbeatStatus::Dead);
    }

    // ===== T34: is_valid_grid(50.0, 220.0) == true =====
    #[test]
    fn t34_is_valid_grid_normal() {
        assert!(is_valid_grid(50.0, 220.0));
    }

    // ===== T35: is_valid_grid(49.0, 220.0) == false =====
    #[test]
    fn t35_is_valid_grid_freq_low() {
        assert!(!is_valid_grid(49.0, 220.0));
    }

    // ===== T36: is_valid_grid(51.0, 220.0) == false =====
    #[test]
    fn t36_is_valid_grid_freq_high() {
        assert!(!is_valid_grid(51.0, 220.0));
    }

    // ===== T37: is_valid_grid(50.0, 199.0) == false =====
    #[test]
    fn t37_is_valid_grid_voltage_low() {
        assert!(!is_valid_grid(50.0, 199.0));
    }

    // ===== T38: is_valid_grid(50.0, 241.0) == false =====
    #[test]
    fn t38_is_valid_grid_voltage_high() {
        assert!(!is_valid_grid(50.0, 241.0));
    }

    // ===== T39: default_anomaly_detectors() 长度 3 =====
    #[test]
    fn t39_default_anomaly_detectors_len() {
        let dets = default_anomaly_detectors();
        assert_eq!(dets.len(), 3);
    }

    // ===== T40: default_anomaly_detectors()[0] 对 frequency=49.0 返回 true =====
    #[test]
    fn t40_default_detector_frequency_out_of_range() {
        let dets = default_anomaly_detectors();
        let state = GridState {
            frequency: 49.0,
            ..GridState::default()
        };
        assert!(dets[0](&state));
    }

    // ===== T41: default_anomaly_detectors()[1] 对 voltage_a=199.0 返回 true =====
    #[test]
    fn t41_default_detector_voltage_out_of_range() {
        let dets = default_anomaly_detectors();
        let state = GridState {
            voltage_a: 199.0,
            ..GridState::default()
        };
        assert!(dets[1](&state));
    }

    // ===== T42: default_anomaly_detectors()[2] 对 Invalid 返回 true =====
    #[test]
    fn t42_default_detector_quality_invalid() {
        let dets = default_anomaly_detectors();
        let state = GridState::default(); // quality == Invalid
        assert!(dets[2](&state));
    }

    // ===== T43: GridError::SampleFailed → AgentRuntimeError::DeviceError =====
    #[test]
    fn t43_grid_error_sample_failed_into() {
        let e: AgentRuntimeError = GridError::SampleFailed.into();
        assert!(matches!(e, AgentRuntimeError::DeviceError(_)));
    }

    // ===== T44: GridError::PublishFailed → AgentRuntimeError::DeviceError =====
    #[test]
    fn t44_grid_error_publish_failed_into() {
        let e: AgentRuntimeError = GridError::PublishFailed.into();
        assert!(matches!(e, AgentRuntimeError::DeviceError(_)));
    }

    // ===== T45: GridError::InvalidConfig → AgentRuntimeError::DeviceError =====
    #[test]
    fn t45_grid_error_invalid_config_into() {
        let e: AgentRuntimeError = GridError::InvalidConfig.into();
        assert!(matches!(e, AgentRuntimeError::DeviceError(_)));
    }

    // 防止未使用导入告警（String/vec 在测试中被使用）
    #[test]
    fn _ensure_imports_used() {
        let _s: String = String::from("x");
        let _v: vec::Vec<u8> = vec![0u8];
    }
}
