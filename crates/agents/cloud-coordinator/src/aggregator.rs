//! EnerOS v0.96.0 Cloud Coordinator 数据汇聚.
//!
//! 收集域内多个 Edge Box 状态（复用 v0.93.0 `EdgeBoxState`，D6）→ 汇总为
//! [`DomainData`]（轻量级快照 + metrics，D10）→ 通过 [`DataSink`] trait 存储
//! （D5）。单源失败不中断（`timeout_count` 计数，D7）；NaN 风暴防御（D9）；
//! 脱敏标记字段预留（D12）。为 v0.112.0 云端孪生主节点提供数据基础。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 复用既有 crate `crates/agents/cloud-coordinator/` 追加本模块（蓝图 `crates/cloud_coordinator/src/{aggregator,storage,schema}.rs` → §2.3.1 硬规则） |
//! | **D2** | `domain_id` / metrics 键全部 `u64` — 无堆字符串 + 确定性（v0.95.0 D2 惯例） |
//! | **D3** | sync `collect` / `store`（no_std 无 async runtime）；`now_ms: u64` 参数注入；`run` 不实现（集成阶段调用方循环驱动） |
//! | **D4** | metrics `BTreeMap<u64, f32>`（no_std alloc 无 HashMap；BTreeMap 确定性迭代可重放） |
//! | **D5** | [`DataSink`] 为 sync trait + [`MockDataSink`]（蓝图 `DataSink { Tsdb, File, S3 }` 枚举 → §5.5 防重复造轮子，真实存储后续注入 `Box<dyn DataSink>`） |
//! | **D6** | `EdgeBoxState` 复用 `eneros-coordinator`（v0.93.0 已导出，不重复定义） |
//! | **D7** | 不用 warn! 宏（no_std 无 log crate）；源失败经 [`AggError::SourceFailed`] + `timeout_count` 暴露可观测 |
//! | **D8** | 测试 crate 内嵌 `#[cfg(test)]` 40 个（蓝图 `tests/data_agg.rs` → v0.87.0~v0.95.0 项目惯例） |
//! | **D9** | NaN 防御：metric 值非有限 → 存入前 sanitize 为 0.0；容量非有限或 ≤0 → 按 0.0 汇总；数据量计数独立 u64 不依赖 metric |
//! | **D10** | 本版本不做压缩（no_std 无标准压缩库）；仅保留 EdgeBoxState 快照轻量汇总，压缩列入后续版本评估 |
//! | **D11** | 统一 u64 ms UTC epoch 时间戳（`now_ms` 外部注入），不涉及时区转换 |
//! | **D12** | 仅定义脱敏标记字段 `is_sensitive: bool`（默认 false）；脱敏执行逻辑后续 v0.101.0 断网处理实现 |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_coordinator::EdgeBoxState;

/// 域级汇聚数据（D2/D4：标识与 metrics 键全部 u64，BTreeMap 确定性；
/// 含 f32/Vec/BTreeMap，不可 derive Eq/Copy）.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainData {
    /// 域 ID（D2：u64；MVP 单域占位 0）.
    pub domain_id: u64,
    /// 汇聚时刻时间戳（u64 ms UTC epoch，D11：回显 `now_ms`）.
    pub timestamp: u64,
    /// 本轮成功收集的 Edge Box 状态快照（D6：复用 v0.93.0 [`EdgeBoxState`]）.
    pub states: Vec<EdgeBoxState>,
    /// 事件记录（本版本 collect 不产出，调用方可手工附加后 store）.
    pub events: Vec<EventRecord>,
    /// 汇总指标（D4/D9：键 0 = 收集到的 Edge Box 数；键 1 = 总容量 MW；
    /// 值存入前 sanitize，非有限 → 0.0）.
    pub metrics: BTreeMap<u64, f32>,
    /// 脱敏标记（D12：默认 false；脱敏执行逻辑后续 v0.101.0 实现）.
    pub is_sensitive: bool,
}

/// 事件记录（全 Copy 类型）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventRecord {
    /// 事件 ID.
    pub event_id: u64,
    /// 事件类型.
    pub event_type: EventType,
    /// 事件时间戳（u64 ms UTC epoch，D11）.
    pub timestamp: u64,
    /// 严重级别.
    pub severity: Severity,
}

/// 事件类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventType {
    /// 状态变更（默认）.
    #[default]
    StateChange,
    /// 告警.
    Alarm,
    /// 命令.
    Command,
    /// 指标.
    Metric,
}

/// 严重级别.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    /// 信息（默认）.
    #[default]
    Info,
    /// 警告.
    Warning,
    /// 错误.
    Error,
    /// 严重.
    Critical,
}

/// 数据汇聚错误（全 Copy 类型，机读审计）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggError {
    /// 数据源获取失败（携带失败源标识，D7）.
    SourceFailed(u64),
    /// 存储失败.
    StoreFailed,
    /// 无数据源（collect 前未 add_source）.
    EmptySources,
}

/// 数据源抽象（D3/D5：sync，no_std 单线程惯例，不要求 Send + Sync）.
pub trait DataSource {
    /// 获取一个 Edge Box 状态；失败返回 [`AggError::SourceFailed`]（D7）.
    fn fetch(&mut self, now_ms: u64) -> Result<EdgeBoxState, AggError>;
}

/// 数据存储抽象（D3/D5：sync；真实 TSDB/S3/File 后端后续以
/// `Box<dyn DataSink>` 注入，不在本版本）.
pub trait DataSink {
    /// 存储一帧汇聚数据；失败返回 [`AggError::StoreFailed`].
    fn store(&mut self, data: &DomainData, now_ms: u64) -> Result<(), AggError>;
}

/// metric 值 sanitize（D9：非有限 → 0.0）.
fn sanitize_metric(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// 容量 sanitize（D9：非有限或 ≤0 → 0.0，不参与汇总）.
fn sanitize_capacity(capacity_mw: f32) -> f32 {
    if capacity_mw.is_finite() && capacity_mw > 0.0 {
        capacity_mw
    } else {
        0.0
    }
}

/// 数据汇聚器（字段全 pub 可观测；collect 多源容错 + store 委托 + 3 计数器）.
pub struct DataAggregator {
    /// 数据源列表（D5：Box 单线程所有权）.
    pub sources: Vec<Box<dyn DataSource>>,
    /// 存储后端（D5：Box 单线程所有权）.
    pub sink: Box<dyn DataSink>,
    /// 成功汇聚轮次计数.
    pub collect_count: u64,
    /// 数据源失败累计次数（D7：单源失败不中断，逐源计数）.
    pub timeout_count: u64,
    /// 成功存储计数.
    pub store_count: u64,
}

impl DataAggregator {
    /// 创建汇聚器（sources 空、3 计数器全零）.
    pub fn new(sink: Box<dyn DataSink>) -> Self {
        Self {
            sources: Vec::new(),
            sink,
            collect_count: 0,
            timeout_count: 0,
            store_count: 0,
        }
    }

    /// 追加数据源（collect 顺序与 add 顺序一致）.
    pub fn add_source(&mut self, source: Box<dyn DataSource>) {
        self.sources.push(source);
    }

    /// 汇聚一轮：遍历 sources 逐个 `fetch(now_ms)`，Ok 加入 states，
    /// Err → `timeout_count += 1` 继续（D7 不中断）；sources 空 →
    /// `Err(EmptySources)`（不计数）。全部 fetch 后组装 [`DomainData`]
    ///（`timestamp = now_ms`，metrics 键 0 = states 数、键 1 = 总容量 MW，
    /// D9 sanitize），`collect_count += 1`.
    pub fn collect(&mut self, now_ms: u64) -> Result<DomainData, AggError> {
        if self.sources.is_empty() {
            return Err(AggError::EmptySources);
        }
        let mut states = Vec::new();
        for source in &mut self.sources {
            match source.fetch(now_ms) {
                Ok(state) => states.push(state),
                Err(_) => self.timeout_count += 1,
            }
        }
        let mut metrics = BTreeMap::new();
        metrics.insert(0, sanitize_metric(states.len() as f32));
        // D9：f64 累加避免 f32 求和中间溢出；转回 f32 后非有限 → 0.0.
        let total_capacity = states
            .iter()
            .map(|s| f64::from(sanitize_capacity(s.capacity_mw)))
            .sum::<f64>() as f32;
        metrics.insert(1, sanitize_metric(total_capacity));
        self.collect_count += 1;
        Ok(DomainData {
            domain_id: 0,
            timestamp: now_ms,
            states,
            events: Vec::new(),
            metrics,
            is_sensitive: false,
        })
    }

    /// 存储一帧汇聚数据：委托 sink.store；Ok → `store_count += 1`，
    /// Err → `Err(StoreFailed)`（store_count 不变）.
    pub fn store(&mut self, data: &DomainData, now_ms: u64) -> Result<(), AggError> {
        match self.sink.store(data, now_ms) {
            Ok(()) => {
                self.store_count += 1;
                Ok(())
            }
            Err(_) => Err(AggError::StoreFailed),
        }
    }
}

/// Mock 数据源（故障注入，D7/D8：[`MockCloudChannel`](crate::MockCloudChannel) 模式）.
///
/// - `fail_times > 0`：fetch 失败并将 `fail_times` 减一，
///   返回 `Err(SourceFailed(state.box_id))`；
/// - `fail_times == 0`：返回 `Ok(state.clone())`。
pub struct MockDataSource {
    /// 预置返回的 Edge Box 状态.
    pub state: EdgeBoxState,
    /// 剩余失败注入次数（每次失败 fetch 减一）.
    pub fail_times: u32,
}

impl MockDataSource {
    /// 创建无故障注入的 Mock（首次 fetch 即成功）.
    pub fn new(state: EdgeBoxState) -> Self {
        Self {
            state,
            fail_times: 0,
        }
    }

    /// 创建带故障注入的 Mock（前 `fail_times` 次 fetch 失败）.
    pub fn with_fail_times(state: EdgeBoxState, fail_times: u32) -> Self {
        Self { state, fail_times }
    }
}

impl DataSource for MockDataSource {
    fn fetch(&mut self, _now_ms: u64) -> Result<EdgeBoxState, AggError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(AggError::SourceFailed(self.state.box_id));
        }
        Ok(self.state.clone())
    }
}

/// Mock 数据存储（故障注入 + 存储记录，D5/D8）.
///
/// - `fail_times > 0`：store 失败并将 `fail_times` 减一，返回 `Err(StoreFailed)`；
/// - `fail_times == 0`：data 克隆记入 `stored`，返回 `Ok(())`。
pub struct MockDataSink {
    /// 已成功存储的数据记录（顺序与 store 调用一致）.
    pub stored: Vec<DomainData>,
    /// 剩余失败注入次数（每次失败 store 减一）.
    pub fail_times: u32,
}

impl MockDataSink {
    /// 创建空 Mock（无故障注入、无记录）.
    pub fn new() -> Self {
        Self {
            stored: Vec::new(),
            fail_times: 0,
        }
    }

    /// 创建带故障注入的 Mock（前 `fail_times` 次 store 失败）.
    pub fn with_fail_times(fail_times: u32) -> Self {
        Self {
            fail_times,
            ..Self::new()
        }
    }
}

impl Default for MockDataSink {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSink for MockDataSink {
    fn store(&mut self, data: &DomainData, _now_ms: u64) -> Result<(), AggError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(AggError::StoreFailed);
        }
        self.stored.push(data.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;
    use std::cell::RefCell;
    use std::rc::Rc;

    use eneros_energy_market_agent::DevicePool;

    use super::*;

    /// 辅助：构造最小 EdgeBoxState（空设备池 + 空 SOC 表 + 在线）.
    fn edge_state(box_id: u64, capacity_mw: f32) -> EdgeBoxState {
        EdgeBoxState {
            box_id,
            devices: DevicePool::new(),
            socs: BTreeMap::new(),
            capacity_mw,
            online: true,
        }
    }

    /// 辅助：构造空 DomainData.
    fn domain_data(domain_id: u64, timestamp: u64) -> DomainData {
        DomainData {
            domain_id,
            timestamp,
            states: Vec::new(),
            events: Vec::new(),
            metrics: BTreeMap::new(),
            is_sensitive: false,
        }
    }

    /// 辅助：记录 store 内容的 sink 桩.
    struct RecordingSink {
        stored: Rc<RefCell<Vec<DomainData>>>,
    }

    impl RecordingSink {
        fn new(stored: Rc<RefCell<Vec<DomainData>>>) -> Self {
            Self { stored }
        }
    }

    impl DataSink for RecordingSink {
        fn store(&mut self, data: &DomainData, _now_ms: u64) -> Result<(), AggError> {
            self.stored.borrow_mut().push(data.clone());
            Ok(())
        }
    }

    /// 辅助：记录 now_ms 透传的 sink 桩.
    struct NowMsRecorder {
        last: Rc<RefCell<Option<u64>>>,
    }

    impl DataSink for NowMsRecorder {
        fn store(&mut self, _data: &DomainData, now_ms: u64) -> Result<(), AggError> {
            *self.last.borrow_mut() = Some(now_ms);
            Ok(())
        }
    }

    // ===== T1~T6：数据结构派生语义 =====

    #[test]
    fn t01_domain_data_construct_clone_eq() {
        let mut d = domain_data(1, 1000);
        d.states.push(edge_state(10, 5.0));
        d.events.push(EventRecord {
            event_id: 7,
            event_type: EventType::Alarm,
            timestamp: 999,
            severity: Severity::Warning,
        });
        d.metrics.insert(0, 1.0);
        let c = d.clone();
        assert_eq!(d, c);
        assert_eq!(d.domain_id, 1);
        assert_eq!(d.timestamp, 1000);
        assert_eq!(d.states.len(), 1);
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.metrics.get(&0), Some(&1.0));
        assert!(!d.is_sensitive);
        // 修改克隆不影响原值（深克隆）.
        let mut c2 = d.clone();
        c2.states.push(edge_state(20, 3.0));
        assert_ne!(d, c2);
    }

    #[test]
    fn t02_domain_data_debug_contains_type_name() {
        let d = domain_data(1, 1000);
        let dbg = alloc::format!("{d:?}");
        assert!(dbg.contains("DomainData"));
        assert!(dbg.contains("domain_id"));
    }

    #[test]
    fn t03_event_record_copy_semantics() {
        let e = EventRecord {
            event_id: 42,
            event_type: EventType::Command,
            timestamp: 5000,
            severity: Severity::Error,
        };
        let copied = e; // Copy 语义.
        assert_eq!(e, copied);
        assert_eq!(e.event_id, 42);
        assert_eq!(e.event_type, EventType::Command);
        assert_eq!(e.timestamp, 5000);
        assert_eq!(e.severity, Severity::Error);
    }

    #[test]
    fn t04_event_type_default_and_eq() {
        assert_eq!(EventType::default(), EventType::StateChange);
        assert_eq!(EventType::Alarm, EventType::Alarm);
        assert_ne!(EventType::StateChange, EventType::Metric);
        // Copy 语义.
        let m = EventType::Metric;
        let copied = m;
        assert_eq!(m, copied);
    }

    #[test]
    fn t05_severity_default_and_eq() {
        assert_eq!(Severity::default(), Severity::Info);
        assert_eq!(Severity::Critical, Severity::Critical);
        assert_ne!(Severity::Info, Severity::Error);
        let w = Severity::Warning;
        let copied = w; // Copy 语义.
        assert_eq!(w, copied);
    }

    #[test]
    fn t06_agg_error_variants_eq() {
        assert_eq!(AggError::SourceFailed(1), AggError::SourceFailed(1));
        assert_ne!(AggError::SourceFailed(1), AggError::SourceFailed(2));
        assert_eq!(AggError::StoreFailed, AggError::StoreFailed);
        assert_eq!(AggError::EmptySources, AggError::EmptySources);
        assert_ne!(AggError::StoreFailed, AggError::EmptySources);
        assert_ne!(AggError::SourceFailed(1), AggError::StoreFailed);
        let e = AggError::SourceFailed(9);
        let copied = e; // Copy 语义.
        assert_eq!(e, copied);
        let dbg = alloc::format!("{:?}", AggError::StoreFailed);
        assert!(dbg.contains("StoreFailed"));
    }

    // ===== T7~T14：collect 核心 =====

    #[test]
    fn t07_new_default_values() {
        let agg = DataAggregator::new(Box::new(MockDataSink::new()));
        assert!(agg.sources.is_empty());
        assert_eq!(agg.collect_count, 0);
        assert_eq!(agg.timeout_count, 0);
        assert_eq!(agg.store_count, 0);
    }

    #[test]
    fn t08_add_source_appends() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        assert_eq!(agg.sources.len(), 1);
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 20.0))));
        assert_eq!(agg.sources.len(), 2);
    }

    #[test]
    fn t09_collect_two_sources_all_success() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 20.0))));
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 2);
        assert_eq!(data.timestamp, 1000);
        assert_eq!(agg.collect_count, 1);
        assert_eq!(agg.timeout_count, 0);
    }

    #[test]
    fn t10_collect_empty_sources_err() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        assert_eq!(agg.collect(1000), Err(AggError::EmptySources));
        // 空源不计数.
        assert_eq!(agg.collect_count, 0);
        assert_eq!(agg.timeout_count, 0);
    }

    #[test]
    fn t11_collect_partial_failure_continues() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        agg.add_source(Box::new(MockDataSource::with_fail_times(
            edge_state(2, 20.0),
            1,
        )));
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 1);
        assert_eq!(data.states[0].box_id, 1);
        assert_eq!(agg.timeout_count, 1);
        assert_eq!(agg.collect_count, 1);
    }

    #[test]
    fn t12_collect_all_fail_ok_with_empty_states() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::with_fail_times(
            edge_state(1, 10.0),
            1,
        )));
        agg.add_source(Box::new(MockDataSource::with_fail_times(
            edge_state(2, 20.0),
            1,
        )));
        let data = agg.collect(1000).unwrap();
        assert!(data.states.is_empty());
        assert_eq!(agg.timeout_count, 2);
        assert_eq!(agg.collect_count, 1);
    }

    #[test]
    fn t13_collect_metrics_key0_states_count() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 20.0))));
        agg.add_source(Box::new(MockDataSource::with_fail_times(
            edge_state(3, 30.0),
            1,
        )));
        let data = agg.collect(1000).unwrap();
        // 键 0 == 成功收集的 states 数（2，失败源不计）.
        assert_eq!(data.metrics.get(&0), Some(&2.0));
    }

    #[test]
    fn t14_collect_metrics_key1_total_capacity() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 20.0))));
        let data = agg.collect(1000).unwrap();
        // 键 1 == 总容量和 10 + 20 = 30 MW.
        assert_eq!(data.metrics.get(&1), Some(&30.0));
    }

    // ===== T15~T20：store =====

    #[test]
    fn t15_store_success_counts_and_records() {
        let stored = Rc::new(RefCell::new(Vec::new()));
        let mut agg = DataAggregator::new(Box::new(RecordingSink::new(stored.clone())));
        let d = domain_data(0, 1000);
        assert_eq!(agg.store(&d, 1000), Ok(()));
        assert_eq!(agg.store_count, 1);
        // Mock 记录侧（直接对 MockDataSink 调 trait store）.
        let mut sink = MockDataSink::new();
        assert_eq!(DataSink::store(&mut sink, &d, 1000), Ok(()));
        assert_eq!(sink.stored.len(), 1);
        assert_eq!(sink.stored[0], d);
        // RecordingSink 侧（经 aggregator）.
        assert_eq!(stored.borrow().len(), 1);
        assert_eq!(stored.borrow()[0], d);
    }

    #[test]
    fn t16_store_failure_err_no_count() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::with_fail_times(1)));
        let d = domain_data(0, 1000);
        assert_eq!(agg.store(&d, 1000), Err(AggError::StoreFailed));
        assert_eq!(agg.store_count, 0);
    }

    #[test]
    fn t17_store_independent_of_collect() {
        // 不经过 collect：直接构造 DomainData store.
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        let mut d = domain_data(5, 7777);
        d.states.push(edge_state(1, 10.0));
        d.metrics.insert(0, 1.0);
        assert_eq!(agg.store(&d, 7777), Ok(()));
        assert_eq!(agg.store_count, 1);
        assert_eq!(agg.collect_count, 0);
    }

    #[test]
    fn t18_store_multiple_counts_accumulate() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        let d = domain_data(0, 1000);
        assert_eq!(agg.store(&d, 1000), Ok(()));
        assert_eq!(agg.store(&d, 2000), Ok(()));
        assert_eq!(agg.store(&d, 3000), Ok(()));
        assert_eq!(agg.store_count, 3);
    }

    #[test]
    fn t19_mock_sink_stored_order_consistent() {
        let mut sink = MockDataSink::new();
        let d1 = domain_data(0, 1000);
        let mut d2 = domain_data(0, 2000);
        d2.states.push(edge_state(9, 1.0));
        assert_eq!(DataSink::store(&mut sink, &d1, 1000), Ok(()));
        assert_eq!(DataSink::store(&mut sink, &d2, 2000), Ok(()));
        assert_eq!(sink.stored.len(), 2);
        assert_eq!(sink.stored[0], d1);
        assert_eq!(sink.stored[1], d2);
    }

    #[test]
    fn t20_store_forwards_now_ms() {
        let last = Rc::new(RefCell::new(None));
        let mut agg = DataAggregator::new(Box::new(NowMsRecorder { last: last.clone() }));
        let d = domain_data(0, 1000);
        assert_eq!(agg.store(&d, 12_345), Ok(()));
        assert_eq!(*last.borrow(), Some(12_345));
    }

    // ===== T21~T26：Mock 故障注入 =====

    #[test]
    fn t21_mock_source_fail_once_then_ok() {
        let mut src = MockDataSource::with_fail_times(edge_state(7, 10.0), 1);
        assert!(src.fetch(1000).is_err());
        assert_eq!(src.fail_times, 0);
        let ok = src.fetch(2000).unwrap();
        assert_eq!(ok.box_id, 7);
    }

    #[test]
    fn t22_mock_source_fail_twice_then_ok() {
        let mut src = MockDataSource::with_fail_times(edge_state(7, 10.0), 2);
        assert!(src.fetch(1000).is_err());
        assert!(src.fetch(2000).is_err());
        assert_eq!(src.fail_times, 0);
        assert!(src.fetch(3000).is_ok());
    }

    #[test]
    fn t23_mock_source_err_variant_source_failed_box_id() {
        let mut src = MockDataSource::with_fail_times(edge_state(42, 10.0), 1);
        assert_eq!(src.fetch(1000), Err(AggError::SourceFailed(42)));
    }

    #[test]
    fn t24_mock_sink_fail_twice_then_ok_recorded() {
        let mut sink = MockDataSink::with_fail_times(2);
        let d = domain_data(0, 1000);
        assert_eq!(
            DataSink::store(&mut sink, &d, 1000),
            Err(AggError::StoreFailed)
        );
        assert_eq!(
            DataSink::store(&mut sink, &d, 1000),
            Err(AggError::StoreFailed)
        );
        assert_eq!(sink.fail_times, 0);
        assert_eq!(DataSink::store(&mut sink, &d, 1000), Ok(()));
        assert_eq!(sink.stored.len(), 1);
        assert_eq!(sink.stored[0], d);
    }

    #[test]
    fn t25_mock_zero_fail_times_first_ok() {
        let mut src = MockDataSource::with_fail_times(edge_state(1, 10.0), 0);
        assert!(src.fetch(1000).is_ok());
        let mut sink = MockDataSink::with_fail_times(0);
        let d = domain_data(0, 1000);
        assert_eq!(DataSink::store(&mut sink, &d, 1000), Ok(()));
        assert_eq!(sink.stored.len(), 1);
        // with_fail_times(0) 与 new() 行为一致.
        let mut src2 = MockDataSource::new(edge_state(2, 5.0));
        assert_eq!(src2.fetch(1000).unwrap().box_id, 2);
    }

    #[test]
    fn t26_mock_as_box_dyn_into_aggregator() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        let source: Box<dyn DataSource> = Box::new(MockDataSource::new(edge_state(1, 10.0)));
        agg.add_source(source);
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 1);
        assert_eq!(agg.store(&data, 1000), Ok(()));
        assert_eq!(agg.store_count, 1);
    }

    // ===== T27~T30：NaN 防御 =====

    #[test]
    fn t27_nan_capacity_treated_as_zero() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, f32::NAN))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 10.0))));
        let data = agg.collect(1000).unwrap();
        // NaN 容量按 0.0 → 总容量 10.0.
        assert_eq!(data.metrics.get(&1), Some(&10.0));
        // states 照常收集（NaN 不影响快照）.
        assert_eq!(data.states.len(), 2);
    }

    #[test]
    fn t28_nonpositive_capacity_treated_as_zero() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 0.0))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, -5.0))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(3, 8.0))));
        let data = agg.collect(1000).unwrap();
        // 0 与负容量按 0.0 → 总容量 8.0.
        assert_eq!(data.metrics.get(&1), Some(&8.0));
    }

    #[test]
    fn t29_infinite_capacity_treated_as_zero() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, f32::INFINITY))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(
            2,
            f32::NEG_INFINITY,
        ))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(3, 6.0))));
        let data = agg.collect(1000).unwrap();
        // ±Inf 容量按 0.0 → 总容量 6.0.
        assert_eq!(data.metrics.get(&1), Some(&6.0));
    }

    #[test]
    fn t30_capacity_sum_overflow_sanitized() {
        // 两个 f32::MAX 容量相加（f64 累加有限，转回 f32 溢出 Inf）→ sanitize 0.0.
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, f32::MAX))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, f32::MAX))));
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.metrics.get(&1), Some(&0.0));
        // 键 0 不受 NaN 风暴影响（D9：计数独立）.
        assert_eq!(data.metrics.get(&0), Some(&2.0));
    }

    // ===== T31~T32：脱敏标记 =====

    #[test]
    fn t31_is_sensitive_default_false() {
        let d = domain_data(0, 1000);
        assert!(!d.is_sensitive);
        // collect 产出同样默认 false.
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        let data = agg.collect(1000).unwrap();
        assert!(!data.is_sensitive);
    }

    #[test]
    fn t32_is_sensitive_true_preserved_through_store() {
        let stored = Rc::new(RefCell::new(Vec::new()));
        let mut agg = DataAggregator::new(Box::new(RecordingSink::new(stored.clone())));
        let mut d = domain_data(0, 1000);
        d.is_sensitive = true;
        assert_eq!(agg.store(&d, 1000), Ok(()));
        assert!(stored.borrow()[0].is_sensitive);
    }

    // ===== T33~T36：多 source 汇聚 =====

    #[test]
    fn t33_three_sources_two_ok_one_fail() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        agg.add_source(Box::new(MockDataSource::with_fail_times(
            edge_state(2, 20.0),
            1,
        )));
        agg.add_source(Box::new(MockDataSource::new(edge_state(3, 30.0))));
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 2);
        assert_eq!(agg.timeout_count, 1);
        assert_eq!(data.metrics.get(&0), Some(&2.0));
        // 失败源容量不参与汇总：10 + 30 = 40.
        assert_eq!(data.metrics.get(&1), Some(&40.0));
    }

    #[test]
    fn t34_five_sources_order_matches_add_order() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        for id in [5u64, 3, 8, 1, 9] {
            agg.add_source(Box::new(MockDataSource::new(edge_state(id, 1.0))));
        }
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 5);
        let ids: Vec<u64> = data.states.iter().map(|s| s.box_id).collect();
        assert_eq!(ids, vec![5, 3, 8, 1, 9]);
    }

    #[test]
    fn t35_mixed_fail_times_multi_collect_counts() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        agg.add_source(Box::new(MockDataSource::with_fail_times(
            edge_state(1, 10.0),
            2,
        )));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 20.0))));
        // 第 1 轮：源 1 失败（timeout+1），源 2 成功.
        let d1 = agg.collect(1000).unwrap();
        assert_eq!(d1.states.len(), 1);
        // 第 2 轮：源 1 仍失败（timeout+1），源 2 成功.
        let d2 = agg.collect(2000).unwrap();
        assert_eq!(d2.states.len(), 1);
        // 第 3 轮：故障耗尽，两源全成功.
        let d3 = agg.collect(3000).unwrap();
        assert_eq!(d3.states.len(), 2);
        assert_eq!(agg.collect_count, 3);
        assert_eq!(agg.timeout_count, 2);
    }

    #[test]
    fn t36_ten_sources_collect() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        for id in 1..=10u64 {
            agg.add_source(Box::new(MockDataSource::new(edge_state(id, 2.0))));
        }
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 10);
        assert_eq!(agg.collect_count, 1);
        assert_eq!(agg.timeout_count, 0);
        assert_eq!(data.metrics.get(&0), Some(&10.0));
        assert_eq!(data.metrics.get(&1), Some(&20.0));
    }

    // ===== T37~T40：全链路集成 =====

    #[test]
    fn t37_full_pipeline_collect_store() {
        let stored = Rc::new(RefCell::new(Vec::new()));
        let mut agg = DataAggregator::new(Box::new(RecordingSink::new(stored.clone())));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        agg.add_source(Box::new(MockDataSource::new(edge_state(2, 20.0))));
        let data = agg.collect(1000).unwrap();
        assert_eq!(agg.store(&data, 1000), Ok(()));
        assert_eq!(agg.collect_count, 1);
        assert_eq!(agg.store_count, 1);
        // stored 内容与 collect 返回完全一致.
        let s = stored.borrow();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0], data);
    }

    #[test]
    fn t38_pipeline_sink_failure_then_recover() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::with_fail_times(1)));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        let d1 = agg.collect(1000).unwrap();
        // sink 故障：store 失败，store_count 不变.
        assert_eq!(agg.store(&d1, 1000), Err(AggError::StoreFailed));
        assert_eq!(agg.store_count, 0);
        // 恢复：再 collect → store 成功，计数语义正确.
        let d2 = agg.collect(2000).unwrap();
        assert_eq!(agg.store(&d2, 2000), Ok(()));
        assert_eq!(agg.store_count, 1);
        assert_eq!(agg.collect_count, 2);
        assert_eq!(agg.timeout_count, 0);
    }

    #[test]
    fn t39_events_attached_pass_through_store() {
        let stored = Rc::new(RefCell::new(Vec::new()));
        let mut agg = DataAggregator::new(Box::new(RecordingSink::new(stored.clone())));
        agg.add_source(Box::new(MockDataSource::new(edge_state(1, 10.0))));
        let mut data = agg.collect(1000).unwrap();
        // 手工附加事件记录.
        data.events.push(EventRecord {
            event_id: 100,
            event_type: EventType::Alarm,
            timestamp: 1000,
            severity: Severity::Critical,
        });
        data.events.push(EventRecord {
            event_id: 101,
            event_type: EventType::StateChange,
            timestamp: 1001,
            severity: Severity::Info,
        });
        assert_eq!(agg.store(&data, 1000), Ok(()));
        let s = stored.borrow();
        assert_eq!(s[0].events.len(), 2);
        assert_eq!(s[0].events[0].event_id, 100);
        assert_eq!(s[0].events[0].severity, Severity::Critical);
        assert_eq!(s[0].events[1].event_type, EventType::StateChange);
        assert_eq!(s[0], data);
    }

    #[test]
    fn t40_forty_sources_stress() {
        let mut agg = DataAggregator::new(Box::new(MockDataSink::new()));
        for id in 1..=40u64 {
            agg.add_source(Box::new(MockDataSource::new(edge_state(id, 1.0))));
        }
        let data = agg.collect(1000).unwrap();
        assert_eq!(data.states.len(), 40);
        assert_eq!(agg.collect_count, 1);
        assert_eq!(agg.timeout_count, 0);
        assert_eq!(data.metrics.get(&0), Some(&40.0));
        assert_eq!(data.metrics.get(&1), Some(&40.0));
        // 顺序与 add 顺序一致.
        let ids: Vec<u64> = data.states.iter().map(|s| s.box_id).collect();
        let expect: Vec<u64> = (1..=40u64).collect();
        assert_eq!(ids, expect);
    }
}
