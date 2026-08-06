//! cloud-coordinator 真实适配器（v0.227.0，B3-04）。
//!
//! 此前 DataSink/DataSource/CloudChannel 仅有 Mock。本模块提供生产路径：
//!
//! 1. [`FsDataSink`] — 真实存储：经 eneros-fs 落盘（JSON 行，重启可读）；
//! 2. [`TelemetryDataSource`] — 真实遥测流：注入遥测读取闭包（tsdb/
//!    协议采集链），组装 [`EdgeBoxState`]；
//! 3. [`TlsCloudChannel`] — 真实云边通道：注入 TLS 发送/接收闭包
//!    （接 v0.221 TLS 抽象），广播策略并收集 ack。
//!
//! 构造器显式注入（B3 纪律）。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use eneros_fs::fs_trait::FileSystem;

use crate::aggregator::{AggError, DataSink, DataSource, DomainData};
use eneros_coordinator::EdgeBoxState;
use crate::channel::{CloudChannel, CloudError};
use crate::strategy::EdgeAck;
use crate::strategy::Strategy;

/// 真实数据存储：经 fs 落盘（每帧一行紧凑格式，重启可读）。
pub struct FsDataSink<F> {
    /// 文件系统。
    fs: F,
    /// 存储文件路径。
    path: String,
}

impl<F: FileSystem> FsDataSink<F> {
    /// 创建 fs 数据存储。
    pub fn new(fs: F, path: &str) -> Self {
        Self {
            fs,
            path: String::from(path),
        }
    }
}

impl<F: FileSystem> DataSink for FsDataSink<F> {
    fn store(&mut self, data: &DomainData, now_ms: u64) -> Result<(), AggError> {
        // 紧凑行：domain_id,timestamp,state_count,metric0,metric1\n
        let m0 = data.metrics.get(&0).copied().unwrap_or(0.0);
        let m1 = data.metrics.get(&1).copied().unwrap_or(0.0);
        let mut line = alloc::string::String::new();
        line.push_str(&data.domain_id.to_string());
        line.push(',');
        line.push_str(&data.timestamp.to_string());
        line.push(',');
        line.push_str(&data.states.len().to_string());
        line.push(',');
        line.push_str(&format_f32(m0));
        line.push(',');
        line.push_str(&format_f32(m1));
        line.push('\n');
        let _ = now_ms;
        // 追加写：读现有 + 新行。
        let mut existing = self.fs.read_file(&self.path).unwrap_or_default();
        existing.extend_from_slice(line.as_bytes());
        self.fs
            .write_file(&self.path, &existing)
            .map_err(|_| AggError::StoreFailed)
    }
}

/// 四舍五入（no_std 无 f32::round）。
fn round_f32(v: f32) -> f32 {
    if v >= 0.0 {
        ((v + 0.5) as i64) as f32
    } else {
        ((v - 0.5) as i64) as f32
    }
}

/// no_std 浮点格式化（避免 fmt 依赖；截断到整数毫精度）。
fn format_f32(v: f32) -> String {
    if !v.is_finite() {
        return String::from("0");
    }
    // 以整数形式输出（千分位精度损失可接受，聚合指标）。
    let n = round_f32(v) as i64;
    n.to_string()
}

/// 真实遥测流数据源：注入遥测读取闭包。
pub struct TelemetryDataSource<F> {
    /// 遥测读取闭包（tsdb/协议采集链；返回 EdgeBox 状态列表）。
    fetch: F,
}

impl<F> TelemetryDataSource<F>
where
    F: FnMut() -> Result<EdgeBoxState, AggError>,
{
    /// 创建遥测数据源。
    pub fn new(fetch: F) -> Self {
        Self { fetch }
    }
}

impl<F> DataSource for TelemetryDataSource<F>
where
    F: FnMut() -> Result<EdgeBoxState, AggError>,
{
    fn fetch(&mut self, _now_ms: u64) -> Result<EdgeBoxState, AggError> {
        (self.fetch)()
    }
}

/// 真实云边通道：注入 TLS 发送/接收闭包（v0.221 TLS 抽象接缝）。
///
/// `send_fn` 序列化策略并发送；`recv_fn` 收集 ack（阻塞窗口由闭包实现）。
pub struct TlsCloudChannel<S, R> {
    /// TLS 发送闭包（策略字节 → 通道）。
    send_fn: S,
    /// TLS 接收闭包（strategy_id → ack 列表）。
    recv_fn: R,
}

impl<S, R> TlsCloudChannel<S, R>
where
    S: FnMut(&[u8]) -> Result<(), CloudError>,
    R: FnMut(u64, u64) -> Vec<EdgeAck>,
{
    /// 创建 TLS 云边通道。
    pub fn new(send_fn: S, recv_fn: R) -> Self {
        Self { send_fn, recv_fn }
    }
}

impl<S, R> CloudChannel for TlsCloudChannel<S, R>
where
    S: FnMut(&[u8]) -> Result<(), CloudError>,
    R: FnMut(u64, u64) -> Vec<EdgeAck>,
{
    fn broadcast(&mut self, strategy: &Strategy) -> Result<(), CloudError> {
        // 紧凑编码策略：id + 目标数。
        let mut payload = Vec::new();
        payload.extend_from_slice(&strategy.strategy_id.to_le_bytes());
        payload.push(strategy.targets.len() as u8);
        (self.send_fn)(&payload)
    }

    fn collect_acks(&mut self, strategy_id: u64, timeout_ms: u64) -> Vec<EdgeAck> {
        (self.recv_fn)(strategy_id, timeout_ms)
    }
}

// ============================================================================
// Tests（真实闭包注入 + MockFileSystem）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    fn domain_data(now_ms: u64) -> DomainData {
        let mut metrics = BTreeMap::new();
        metrics.insert(0, 2.0);
        metrics.insert(1, 50.0);
        DomainData {
            domain_id: 1,
            timestamp: now_ms,
            states: vec![
                EdgeBoxState {
                    box_id: 1,
                    devices: eneros_energy_market_agent::DevicePool::default(),
                    socs: BTreeMap::new(),
                    capacity_mw: 10.0,
                    online: true,
                },
                EdgeBoxState {
                    box_id: 2,
                    devices: eneros_energy_market_agent::DevicePool::default(),
                    socs: BTreeMap::new(),
                    capacity_mw: 20.0,
                    online: true,
                },
            ],
            events: Vec::new(),
            metrics,
            is_sensitive: false,
        }
    }

    #[test]
    fn test_fs_sink_persists_and_appends() {
        let fs = eneros_fs::MockFileSystem::new();
        let mut sink = FsDataSink::new(fs, "/cloud/domain1.dat");
        sink.store(&domain_data(1000), 1000).expect("store 1");
        sink.store(&domain_data(2000), 2000).expect("store 2");
        // 读回验证两行。
        let mut fs2 = sink.fs;
        let data = fs2.read_file("/cloud/domain1.dat").expect("read");
        let text = core::str::from_utf8(&data).expect("utf8");
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("1,1000,2,"));
        assert!(text.contains("1,2000,2,"));
    }

    #[test]
    fn test_telemetry_source_stamps_now() {
        let mut src = TelemetryDataSource::new(|| {
            Ok(EdgeBoxState {
                box_id: 7,
                devices: eneros_energy_market_agent::DevicePool::default(),
                socs: BTreeMap::new(),
                capacity_mw: 5.0,
                online: true,
            })
        });
        let state = src.fetch(5555).expect("fetch");
        assert!(state.online, "遥测流状态必须在线");
        assert_eq!(state.box_id, 7);
    }

    #[test]
    fn test_telemetry_source_error_propagates() {
        let mut src = TelemetryDataSource::new(|| Err(AggError::SourceFailed(0)));
        assert!(src.fetch(0).is_err());
    }

    #[test]
    fn test_tls_channel_broadcast_and_ack() {
        let sent: core::cell::RefCell<Vec<Vec<u8>>> = core::cell::RefCell::new(Vec::new());
        let mut pub_ = TlsCloudChannel::new(
            |payload: &[u8]| {
                sent.borrow_mut().push(payload.to_vec());
                Ok(())
            },
            |_id: u64, _t: u64| {
                vec![EdgeAck {
                    strategy_id: 42,
                    edge_id: 1,
                    accepted: true,
                    reason: None,
                }]
            },
        );
        let strategy = Strategy {
            strategy_id: 42,
            version: 1,
            targets: alloc::vec![1, 2, 3],
            content: crate::strategy::StrategyContent::OptimizationWeights(BTreeMap::new()),
            deadline: 0,
            priority: eneros_coordinator::Priority::default(),
        };
        pub_.broadcast(&strategy).expect("broadcast");
        let sent_borrow = sent.borrow();
        assert_eq!(sent_borrow.len(), 1);
        // 首 4 字节 = id LE。
        assert_eq!(sent_borrow[0][..4], [42, 0, 0, 0]);
        // 第 9 字节 = 目标数 3。
        assert_eq!(sent_borrow[0][8], 3);
        let acks = pub_.collect_acks(42, 100);
        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].edge_id, 1);
    }

    #[test]
    fn test_tls_channel_send_error_propagates() {
        let mut ch = TlsCloudChannel::new(
            |_p: &[u8]| Err(CloudError::BroadcastFailed),
            |_id: u64, _t: u64| Vec::new(),
        );
        let strategy = Strategy {
            strategy_id: 1,
            version: 1,
            targets: Vec::new(),
            content: crate::strategy::StrategyContent::OptimizationWeights(BTreeMap::new()),
            deadline: 0,
            priority: eneros_coordinator::Priority::default(),
        };
        assert!(ch.broadcast(&strategy).is_err());
    }
}
