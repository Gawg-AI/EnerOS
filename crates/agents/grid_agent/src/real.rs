//! grid_agent 真实采集/发布适配器（v0.225.0，B3-02）。
//!
//! 此前 sampler/publisher/pcc 仅有 Mock 实现。本模块提供生产路径：
//!
//! 1. [`RealGridSampler`] — 经真实驱动采集链（rs485/can/net）采样：
//!    接受三个注入闭包（频率/电压/功率读数），组装 [`GridState`]；
//! 2. [`DdsGridPublisher`] — 经 agent-bus-dds `DdsNode` 发布电网状态与
//!    告警（真实签名，topic 由 [`eneros_agent_bus_dds::standard_topics`] 注册）；
//! 3. [`RealPccReader`] — 经真实并网点计量（注入计量读数闭包）读取
//!    [`PccReading`]。
//!
//! 注入点均为构造器显式注入（B3 通用纪律：Mock 收编 cfg(test)）。

use alloc::vec::Vec;

use eneros_agent_bus_dds::node::DdsNode;
use eneros_agent_bus_dds::qos::{Durability, History, QosPolicy, Reliability};
use eneros_agent_bus_dds::types::WriterId;

use crate::pcc::{PccReader, PccReading};
use crate::publisher::GridPublisher;
use crate::sampler::GridSampler;
use crate::state::{DataQuality, GridState};
use crate::GridError;

/// 真实电网采样器：从驱动采集链（rs485/can/net 适配器）读取原始量。
///
/// 三个闭包分别提供频率（Hz）、三相电压（V）、三相电流（A）；
/// 功率由电压×电流×功率因数计算（真实数据源→PCC 状态→发布端到端）。
pub struct RealGridSampler<F, V, I> {
    /// 频率读数（Hz）。
    freq: F,
    /// 三相电压读数（[Va, Vb, Vc]）。
    voltages: V,
    /// 三相电流读数（[Ia, Ib, Ic]）。
    currents: I,
    /// 功率因数（0..1）。
    power_factor: f32,
}

impl<F, V, I> RealGridSampler<F, V, I>
where
    F: FnMut() -> Result<f32, GridError>,
    V: FnMut() -> Result<[f32; 3], GridError>,
    I: FnMut() -> Result<[f32; 3], GridError>,
{
    /// 创建真实采样器（构造器显式注入）。
    pub fn new(
        freq: F,
        voltages: V,
        currents: I,
        power_factor: f32,
    ) -> Self {
        Self {
            freq,
            voltages,
            currents,
            power_factor,
        }
    }
}

impl<F, V, I> GridSampler for RealGridSampler<F, V, I>
where
    F: FnMut() -> Result<f32, GridError>,
    V: FnMut() -> Result<[f32; 3], GridError>,
    I: FnMut() -> Result<[f32; 3], GridError>,
{
    fn sample(&mut self, now_ms: u64) -> Result<GridState, GridError> {
        let freq = (self.freq)()?;
        let [va, vb, vc] = (self.voltages)()?;
        let [ia, ib, ic] = (self.currents)()?;
        // 有功功率 ≈ 三相视在功率 × 功率因数（真实数据源路径）。
        let active_power =
            (va * ia + vb * ib + vc * ic) / 1000.0 * self.power_factor;
        let reactive_power = (va * ia + vb * ib + vc * ic) / 1000.0
            * sqrt_f32((1.0 - self.power_factor * self.power_factor).max(0.0));
        Ok(GridState {
            frequency: freq,
            voltage_a: va,
            voltage_b: vb,
            voltage_c: vc,
            current_a: ia,
            current_b: ib,
            current_c: ic,
            active_power,
            reactive_power,
            power_factor: self.power_factor,
            timestamp: now_ms,
            quality: DataQuality::Good,
        })
    }
}

/// 经 agent-bus-dds 的电网发布器（真实签名）。
///
/// 创建 writer 后，`publish_state`/`publish_alert` 将 GridState 编码为
/// DDS 样本（紧凑二进制：f32 序列）发布到指定 topic。
pub struct DdsGridPublisher<N: DdsNode> {
    /// DDS 节点。
    node: N,
    /// 状态 topic writer。
    state_writer: WriterId,
    /// 告警 topic writer。
    alert_writer: WriterId,
}

impl<N: DdsNode> DdsGridPublisher<N> {
    /// 创建发布器：创建 participant + 两个 writer（状态/告警）。
    ///
    /// # 参数
    /// - `node`: DDS 节点（MockDdsNode 或 CycloneDdsNode）
    /// - `state_topic`: 状态主题（默认 "eneros/grid/state"）
    /// - `alert_topic`: 告警主题（默认 "eneros/grid/alert"）
    pub fn new(mut node: N, state_topic: &str, alert_topic: &str) -> Result<Self, GridError> {
        let participant = node
            .create_participant()
            .map_err(|_| GridError::PublishFailed)?;
        let qos = QosPolicy {
            reliability: Reliability::Reliable,
            durability: Durability::Volatile,
            history: History::KeepLast(1),
            deadline: None,
            lifespan: None,
            priority: 0,
        };
        let state_writer = node
            .create_writer(participant, state_topic, qos)
            .map_err(|_| GridError::PublishFailed)?;
        let alert_writer = node
            .create_writer(participant, alert_topic, qos)
            .map_err(|_| GridError::PublishFailed)?;
        let _ = participant; // DdsNode 无需保留 participant 句柄
        Ok(Self {
            node,
            state_writer,
            alert_writer,
        })
    }
}

impl<N: DdsNode> GridPublisher for DdsGridPublisher<N> {
    fn publish_state(&mut self, state: &GridState) -> Result<(), GridError> {
        let payload = encode_grid_state(state);
        self.node
            .write(self.state_writer, &payload)
            .map_err(|_| GridError::PublishFailed)
    }

    fn publish_alert(&mut self, state: &GridState) -> Result<(), GridError> {
        let payload = encode_grid_state(state);
        self.node
            .write(self.alert_writer, &payload)
            .map_err(|_| GridError::PublishFailed)
    }
}

/// 手写平方根（no_std 无 f32::sqrt/powf；牛顿迭代）。
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x * 0.5;
    for _ in 0..8 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

/// 编码 GridState 为紧凑二进制（f32 × 13 字段，小端）。
fn encode_grid_state(state: &GridState) -> Vec<u8> {
    let mut out = Vec::with_capacity(13 * 4);
    for v in [
        state.frequency,
        state.voltage_a,
        state.voltage_b,
        state.voltage_c,
        state.current_a,
        state.current_b,
        state.current_c,
        state.active_power,
        state.reactive_power,
        state.power_factor,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&state.timestamp.to_le_bytes());
    out.extend_from_slice(&(state.quality as u8).to_le_bytes());
    out
}

/// 真实并网点计量读取器（注入真实计量读数闭包）。
pub struct RealPccReader<F> {
    /// 计量读数闭包（真实电能表/融合网关）。
    read_fn: F,
}

impl<F> RealPccReader<F>
where
    F: FnMut(u32, u64) -> Result<PccReading, GridError>,
{
    /// 创建真实 PCC 读取器。
    pub fn new(read_fn: F) -> Self {
        Self { read_fn }
    }
}

impl<F> PccReader for RealPccReader<F>
where
    F: FnMut(u32, u64) -> Result<PccReading, GridError>,
{
    fn read(&mut self, pcc_id: u32, now_ms: u64) -> Result<PccReading, GridError> {
        (self.read_fn)(pcc_id, now_ms)
    }
}

// ============================================================================
// Tests（真实闭包注入 + MockDdsNode）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcc::BreakerStatus;

    #[test]
    fn test_real_sampler_end_to_end() {
        // 真实采集链端到端：数据源 → GridState。
        let mut sampler = RealGridSampler::new(
            || Ok(50.0f32),
            || Ok([230.0, 231.0, 229.0]),
            || Ok([10.0, 10.5, 9.8]),
            0.95,
        );
        let state = sampler.sample(1000).expect("sample");
        assert_eq!(state.frequency, 50.0);
        assert_eq!(state.voltage_a, 230.0);
        assert_eq!(state.current_a, 10.0);
        // 有功 = (230*10 + 231*10.5 + 229*9.8)/1000 * 0.95。
        let expected = (230.0 * 10.0 + 231.0 * 10.5 + 229.0 * 9.8) / 1000.0 * 0.95;
        assert!((state.active_power - expected).abs() < 1e-3);
        assert_eq!(state.quality, DataQuality::Good);
        assert_eq!(state.timestamp, 1000);
    }

    #[test]
    fn test_real_sampler_error_propagates() {
        let mut sampler = RealGridSampler::new(
            || Err(GridError::SampleFailed),
            || Ok([230.0; 3]),
            || Ok([10.0; 3]),
            1.0,
        );
        assert!(sampler.sample(0).is_err());
    }

    #[test]
    fn test_dds_publisher_publishes_state() {
        use eneros_agent_bus_dds::MockDdsNode;
        let node = MockDdsNode::new_default();
        let mut pub_ = DdsGridPublisher::new(node, "eneros/grid/state", "eneros/grid/alert")
            .expect("publisher");
        let state = GridState {
            frequency: 50.0,
            voltage_a: 230.0,
            voltage_b: 231.0,
            voltage_c: 229.0,
            current_a: 10.0,
            current_b: 10.5,
            current_c: 9.8,
            active_power: 6.5,
            reactive_power: 1.2,
            power_factor: 0.95,
            timestamp: 42,
            quality: DataQuality::Good,
        };
        pub_.publish_state(&state).expect("publish");
        pub_.publish_alert(&state).expect("alert");
        // MockDdsNode 验证：writer 已创建且写入成功（无错误即证明端到端）。
    }

    #[test]
    fn test_real_pcc_reader_injected() {
        let mut reader = RealPccReader::new(|pcc_id, now_ms| {
            Ok(PccReading {
                breaker_status: BreakerStatus::Closed,
                active_power: 1000.0 + pcc_id as f32,
                reactive_power: 100.0 + now_ms as f32,
            })
        });
        let r = reader.read(7, 123).expect("read");
        assert_eq!(r.breaker_status, BreakerStatus::Closed);
        assert_eq!(r.active_power, 1007.0);
        assert_eq!(r.reactive_power, 223.0);
    }
}
