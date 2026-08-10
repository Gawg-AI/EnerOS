//! 指标数据模型与采集器实现（v0.120.0）.
//!
//! 对应蓝图 §4.1（数据结构）/ §4.2（接口定义）/ §4.4（错误处理）/ §4.5
//! （关键代码）。按偏差 D2，`record`/`increment` 通过 `now_ms` 参数传入
//! 时间戳，不依赖系统时钟；按偏差 D1，`query` 返回 `alloc::vec::Vec`。

use alloc::vec::Vec;

use heapless::String as HString;

use crate::registry::MetricRegistry;
use crate::store::MetricsStore;

/// 指标类型（蓝图 §4.1）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    /// 计数器（单调递增）.
    Counter,
    /// 瞬时值.
    Gauge,
    /// 直方图.
    Histogram,
    /// 摘要（分位数）.
    Summary,
}

/// 指标值（蓝图 §4.1）.
#[derive(Debug, Clone)]
pub enum MetricValue {
    /// 无符号 64 位整数.
    U64(u64),
    /// 64 位浮点.
    F64(f64),
    /// 有符号 64 位整数.
    I64(i64),
}

/// 标签（key≤32 字节，value≤64 字节）.
#[derive(Debug, Clone)]
pub struct Label {
    /// 标签键.
    pub key: HString<32>,
    /// 标签值.
    pub value: HString<64>,
}

/// 指标数据点（蓝图 §4.1，标签≤8 个）.
#[derive(Debug, Clone)]
pub struct Metric {
    /// 指标名.
    pub name: HString<64>,
    /// 指标类型.
    pub metric_type: MetricType,
    /// 指标值.
    pub value: MetricValue,
    /// 标签集合（容量 8，超出截断）.
    pub labels: heapless::Vec<Label, 8>,
    /// 时间戳（毫秒）.
    pub timestamp_ms: u64,
}

/// 指标注册项（help≤128 字节，unit≤16 字节）.
#[derive(Debug, Clone)]
pub struct MetricDef {
    /// 指标名.
    pub name: HString<64>,
    /// 指标类型.
    pub metric_type: MetricType,
    /// 帮助说明.
    pub help: HString<128>,
    /// 单位.
    pub unit: HString<16>,
}

/// 指标采集全流程错误（蓝图 §4.4）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsError {
    /// 指标未注册.
    NotRegistered,
    /// 值非法.
    InvalidValue,
    /// 标签数超限.
    LabelLimitExceeded,
    /// 存储满.
    StorageFull,
    /// 查询范围非法.
    QueryRangeInvalid,
    /// 文件 I/O 失败（33 OB-07：FileMetricsStore 导出后端）.
    IoFailed,
}

/// 指标采集器 trait（蓝图 §4.2；偏差 D2 增加 `now_ms` 参数）.
pub trait MetricsCollector {
    /// 注册指标定义.
    fn register(&mut self, def: MetricDef) -> Result<(), MetricsError>;
    /// 记录一条指标数据点；未注册返回 [`MetricsError::NotRegistered`].
    fn record(
        &mut self,
        name: &str,
        value: MetricValue,
        labels: &[Label],
        now_ms: u64,
    ) -> Result<(), MetricsError>;
    /// 计数器递增：基于 store 当前最新值累加后记录.
    fn increment(
        &mut self,
        name: &str,
        delta: u64,
        labels: &[Label],
        now_ms: u64,
    ) -> Result<(), MetricsError>;
    /// 范围查询：`start_ms > end_ms` 返回 [`MetricsError::QueryRangeInvalid`].
    fn query(&self, name: &str, start_ms: u64, end_ms: u64) -> Result<Vec<Metric>, MetricsError>;
}

/// 指标采集器实现（蓝图 §4.5；256 条内存环形缓冲 + 定时批量刷入）.
pub struct MetricsCollectorImpl<S: MetricsStore> {
    /// 指标注册表.
    registry: MetricRegistry,
    /// 内存环形缓冲（满 256 丢弃最旧）.
    buffer: heapless::Vec<Metric, 256>,
    /// 32/33 OB-01：未 flush 到 store 的 increment 累计（基数 = store +
    /// pending——旧实现只读 store 最新值，漏 buffer 未 flush 增量）。
    pending: heapless::Vec<(heapless::String<64>, u64), 32>,
    /// 刷入间隔（毫秒）.
    flush_interval_ms: u64,
    /// 上次刷入时刻（毫秒）.
    last_flush_ms: u64,
    /// 指标存储后端.
    store: S,
}

impl<S: MetricsStore> MetricsCollectorImpl<S> {
    /// 创建采集器；`flush_interval_ms` 为批量刷入间隔.
    pub fn new(store: S, flush_interval_ms: u64) -> Self {
        Self {
            registry: MetricRegistry::new(),
            buffer: heapless::Vec::new(),
            pending: heapless::Vec::new(),
            flush_interval_ms,
            last_flush_ms: 0,
            store,
        }
    }

    /// 注册蓝图 §4.5 的 8 项系统默认指标.
    pub fn register_defaults(&mut self) {
        let defaults = [
            ("system.cpu_usage", MetricType::Gauge, "CPU 使用率", "%"),
            ("system.mem_usage", MetricType::Gauge, "内存使用率", "%"),
            ("system.temperature", MetricType::Gauge, "CPU 温度", "°C"),
            (
                "agent.active_count",
                MetricType::Gauge,
                "活跃 Agent 数",
                "count",
            ),
            (
                "agent.request_latency",
                MetricType::Histogram,
                "请求延迟",
                "ms",
            ),
            (
                "agent.error_count",
                MetricType::Counter,
                "错误计数",
                "count",
            ),
            (
                "business.control_commands",
                MetricType::Counter,
                "控制指令数",
                "count",
            ),
            (
                "business.optimization_calls",
                MetricType::Counter,
                "优化调用数",
                "count",
            ),
        ];
        for (name, mtype, help, unit) in defaults {
            // 默认指标均为已知常量，定容转换失败时静默跳过（重复调用时幂等）.
            if let (Ok(n), Ok(h), Ok(u)) = (
                HString::<64>::try_from(name),
                HString::<128>::try_from(help),
                HString::<16>::try_from(unit),
            ) {
                let def = MetricDef {
                    name: n,
                    metric_type: mtype,
                    help: h,
                    unit: u,
                };
                let _ = self.registry.register(def);
            }
        }
    }

    /// 尝试将缓冲批量刷入存储：距上次刷入不足间隔时返回 0.
    /// 只读访问存储后端（观测/测试查询用）。
    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn maybe_flush(&mut self, now_ms: u64) -> Result<usize, MetricsError> {
        if now_ms.saturating_sub(self.last_flush_ms) < self.flush_interval_ms {
            return Ok(0);
        }
        let count = self.buffer.len();
        // heapless::Vec 无 drain：按索引逐条刷入后清空缓冲
        for i in 0..count {
            self.store.append(&self.buffer[i])?;
        }
        self.buffer.clear();
        // 33 OB-01：flush 后 pending 归零（store 已见全部增量）。
        self.pending.clear();
        self.last_flush_ms = now_ms;
        Ok(count)
    }
}

impl<S: MetricsStore> MetricsCollector for MetricsCollectorImpl<S> {
    fn register(&mut self, def: MetricDef) -> Result<(), MetricsError> {
        self.registry.register(def)
    }

    fn record(
        &mut self,
        name: &str,
        value: MetricValue,
        labels: &[Label],
        now_ms: u64,
    ) -> Result<(), MetricsError> {
        // 检查指标是否已注册
        if !self.registry.contains(name) {
            return Err(MetricsError::NotRegistered);
        }
        // 构建标签集合（超过 8 个截断）
        let mut label_vec = heapless::Vec::new();
        for l in labels.iter().take(8) {
            let _ = label_vec.push(l.clone());
        }
        let metric = Metric {
            name: HString::<64>::try_from(name).map_err(|_| MetricsError::InvalidValue)?,
            metric_type: self.registry.get_type(name).unwrap_or(MetricType::Gauge),
            value,
            labels: label_vec,
            timestamp_ms: now_ms,
        };
        // 写入缓冲：满 256 时丢弃最旧一条
        if self.buffer.is_full() {
            self.buffer.remove(0);
        }
        let _ = self.buffer.push(metric);
        Ok(())
    }

    fn increment(
        &mut self,
        name: &str,
        delta: u64,
        labels: &[Label],
        now_ms: u64,
    ) -> Result<(), MetricsError> {
        // 33 OB-01：基数 = store 最新值 + 未 flush 的 pending 累计（旧实现
        // 只读 store——buffer 未 flush 的增量丢失，col10 测试靠手工 flush
        // 掩盖）。
        let mut pending_delta = 0u64;
        {
            let mut found = false;
            for (n, d) in self.pending.iter_mut() {
                if n.as_str() == name {
                    *d = d.saturating_add(delta);
                    pending_delta = *d;
                    found = true;
                    break;
                }
            }
            if !found {
                let mut n: heapless::String<64> = heapless::String::new();
                let _ = n.push_str(name);
                let _ = self.pending.push((n, delta));
                pending_delta = delta;
            }
        }
        let current = self.store.get_latest(name).unwrap_or(0);
        self.record(
            name,
            MetricValue::U64(current.saturating_add(pending_delta)),
            labels,
            now_ms,
        )
    }

    fn query(&self, name: &str, start_ms: u64, end_ms: u64) -> Result<Vec<Metric>, MetricsError> {
        if start_ms > end_ms {
            return Err(MetricsError::QueryRangeInvalid);
        }
        self.store.query_range(name, start_ms, end_ms)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::store::MemoryMetricsStore;
    /// 33 OB-01 判据：连续 increment 不 flush → flush 后总值 = 累计和
    ///（旧实现基数只读 store，未 flush 增量丢失）。
    #[test]
    fn test_increment_pending_accumulates() {
        let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 1_000_000);
        let _ = c.register(MetricDef {
            name: HString::<64>::try_from("counter").unwrap(),
            metric_type: MetricType::Counter,
            help: HString::<128>::try_from("counter").unwrap(),
            unit: HString::<16>::try_from("count").unwrap(),
        });
        for _ in 0..5 {
            c.increment("counter", 3, &[], 100).unwrap();
        }
        // 不 flush：store 无值（间隔 1_000_000 未到）——pending 累计 15。
        assert_eq!(c.store.get_latest("counter").unwrap_or(0), 0);
        let n = c.maybe_flush(1_000_001).unwrap();
        assert_eq!(n, 5);
        assert_eq!(
            c.store.get_latest("counter").unwrap(),
            15,
            "flush 后总值必须=累计和"
        );
    }


    /// 构造注册表 + 内存存储采集器，并注册一项测试指标.
    fn new_collector() -> MetricsCollectorImpl<MemoryMetricsStore> {
        let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 1000);
        let def = MetricDef {
            name: HString::<64>::try_from("test.gauge").unwrap(),
            metric_type: MetricType::Gauge,
            help: HString::<128>::try_from("测试指标").unwrap(),
            unit: HString::<16>::try_from("count").unwrap(),
        };
        c.register(def).unwrap();
        c
    }

    /// COL5：record 未注册指标返回 NotRegistered.
    #[test]
    fn col5_record_unregistered() {
        let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 1000);
        let r = c.record("ghost.metric", MetricValue::U64(1), &[], 100);
        assert_eq!(r, Err(MetricsError::NotRegistered));
    }

    /// COL6：record 成功入缓冲，flush 后 store 可见.
    #[test]
    fn col6_record_buffered_then_flushed() {
        let mut c = new_collector();
        c.record("test.gauge", MetricValue::U64(42), &[], 100)
            .unwrap();
        // 未刷入前 store 无数据
        assert_eq!(c.store.get_latest("test.gauge"), None);
        // 到间隔刷入后 store 可见
        let n = c.maybe_flush(1100).unwrap();
        assert_eq!(n, 1);
        assert_eq!(c.store.get_latest("test.gauge"), Some(42));
    }

    /// COL7：缓冲满 256 时丢弃最旧.
    #[test]
    fn col7_buffer_full_drops_oldest() {
        let mut c = new_collector();
        for i in 0..300u64 {
            c.record("test.gauge", MetricValue::U64(i), &[], i).unwrap();
        }
        assert_eq!(c.buffer.len(), 256);
        // 最旧的 44 条（0..43）已被丢弃，缓冲首条为 44
        match &c.buffer[0].value {
            MetricValue::U64(v) => assert_eq!(*v, 44),
            _ => panic!("unexpected value type"),
        }
        match &c.buffer[255].value {
            MetricValue::U64(v) => assert_eq!(*v, 299),
            _ => panic!("unexpected value type"),
        }
    }

    /// COL8：maybe_flush 未到间隔返回 0.
    #[test]
    fn col8_flush_before_interval_returns_zero() {
        let mut c = new_collector();
        c.record("test.gauge", MetricValue::U64(1), &[], 0).unwrap();
        let n = c.maybe_flush(500).unwrap();
        assert_eq!(n, 0);
        // 缓冲未刷入
        assert_eq!(c.store.get_latest("test.gauge"), None);
    }

    /// COL9：到间隔批量刷入、缓冲清空、last_flush 更新.
    #[test]
    fn col9_flush_at_interval_clears_buffer() {
        let mut c = new_collector();
        for i in 0..5u64 {
            c.record("test.gauge", MetricValue::U64(i), &[], i).unwrap();
        }
        let n = c.maybe_flush(1000).unwrap();
        assert_eq!(n, 5);
        assert_eq!(c.buffer.len(), 0);
        assert_eq!(c.last_flush_ms, 1000);
        assert_eq!(c.store.get_latest("test.gauge"), Some(4));
        // 基于新 last_flush 未到间隔再次返回 0
        let n = c.maybe_flush(1500).unwrap();
        assert_eq!(n, 0);
    }

    /// COL10：increment 两次累加（5+3=8；基于 store 最新值，间隔刷入）.
    #[test]
    fn col10_increment_accumulates() {
        let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 10);
        let def = MetricDef {
            name: HString::<64>::try_from("test.counter").unwrap(),
            metric_type: MetricType::Counter,
            help: HString::<128>::try_from("测试计数器").unwrap(),
            unit: HString::<16>::try_from("count").unwrap(),
        };
        c.register(def).unwrap();
        c.increment("test.counter", 5, &[], 0).unwrap();
        // 第一次刷入后 store 最新值为 5
        c.maybe_flush(10).unwrap();
        c.increment("test.counter", 3, &[], 10).unwrap();
        c.maybe_flush(20).unwrap();
        assert_eq!(c.store.get_latest("test.counter"), Some(8));
    }

    /// COL11：query 范围非法返回 QueryRangeInvalid.
    #[test]
    fn col11_query_invalid_range() {
        let c = new_collector();
        let r = c.query("test.gauge", 200, 100);
        assert!(matches!(r, Err(MetricsError::QueryRangeInvalid)));
    }

    /// COL12：register_defaults 注册 8 项默认指标且类型正确.
    #[test]
    fn col12_register_defaults() {
        let mut c = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 1000);
        c.register_defaults();
        let expected = [
            ("system.cpu_usage", MetricType::Gauge),
            ("system.mem_usage", MetricType::Gauge),
            ("system.temperature", MetricType::Gauge),
            ("agent.active_count", MetricType::Gauge),
            ("agent.request_latency", MetricType::Histogram),
            ("agent.error_count", MetricType::Counter),
            ("business.control_commands", MetricType::Counter),
            ("business.optimization_calls", MetricType::Counter),
        ];
        for (name, mtype) in expected {
            assert!(c.registry.contains(name), "missing default metric {name}");
            assert_eq!(c.registry.get_type(name), Some(mtype));
        }
    }
}
