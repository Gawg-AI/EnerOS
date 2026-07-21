//! v0.81.0 端到端时延探针与统计 — LatencyProbe + DelayStats（无真实网络 I/O）.
//!
//! 在 v0.79.0 gPTP 时间同步 + v0.80.0 TAS 门控调度之上，建立端到端时延测量能力。
//! 提供 [`DelayStats`]（min/max/mean/p99/p999/jitter/samples 统计结果）与
//! [`LatencyProbe`]（基于 closure 注入的多场景时延探针）.
//!
//! # 关键 no_std 设计（D24 / D25）
//!
//! - **无 `eneros_time::Instant` 依赖**（蓝图 API 不存在）：通过 `clock_fn: fn() -> u64`
//!   字段注入时钟源（返回纳秒），测试用静态 `AtomicU64` 计数器模拟.
//! - **无 `eneros_time::delay()` 依赖**：通过 `sleep_fn: fn(Duration)` 字段注入睡眠
//!   函数，测试用空函数模拟.
//! - **无真实 DDS / TSN 硬件 I/O**：通过 `send: impl FnMut() -> Result<(), ()>` 闭包注入
//!   发送动作（D26：`TsnDriver::send` 要求 `&mut self`，闭包实现 `FnMut` 而非 `Fn`），
//!   [`crate::driver_glue::driver_send_closure`] 适配器桥接 `TsnDriver` → send 闭包.
//!
//! # 核心类型
//!
//! - [`DelayStats`] — 统计结果（min/max/mean/p99/p999/jitter/samples）
//! - [`LatencyProbe`] — 时延探针（measure_round_trip / run / run_burst / compute_stats /
//!   measure_e2e / measure_under_load）

use alloc::vec::Vec;
use core::time::Duration;

/// 时延统计结果.
///
/// 7 字段：最小值 / 最大值 / 平均 / p99 / p999 / 抖动 / 样本数.
/// `Default` 返回所有 `Duration::ZERO` + `samples: 0` 的实例（用于空结果集）.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DelayStats {
    /// 最小观测时延.
    pub min: Duration,
    /// 最大观测时延.
    pub max: Duration,
    /// 平均时延（算术平均）.
    pub mean: Duration,
    /// 第 99 百分位时延.
    pub p99: Duration,
    /// 第 99.9 百分位时延.
    pub p999: Duration,
    /// 抖动（max - min）.
    pub jitter: Duration,
    /// 采集样本数.
    pub samples: u64,
}

/// 时延探针 — 基于 closure 注入的多场景时延测量.
///
/// # 字段
///
/// - `sample_count` — 已采集样本数（成功完成 measure_round_trip 的次数）
/// - `results` — 采集到的 Duration 列表（每次 measure_round_trip 成功后 push）
/// - `clock_fn` — 时钟注入（返回纳秒，避免 `Instant::now()` 依赖，D24）
/// - `sleep_fn` — 睡眠注入（避免 `eneros_time::delay()` 依赖，D24）
///
/// # 用法
///
/// ```ignore
/// use eneros_tsn_time::{DelayStats, LatencyProbe};
/// use core::time::Duration;
///
/// fn test_clock() -> u64 { 0 }
/// fn test_sleep(_: Duration) {}
///
/// let mut probe = LatencyProbe::new(test_clock, test_sleep);
/// // FnMut closure (Fn closures like || Ok(()) also satisfy FnMut)
/// let stats = probe.run_burst(10, Duration::from_millis(1), || Ok(()));
/// assert_eq!(stats.samples, 10);
/// ```
pub struct LatencyProbe {
    /// 已采集样本数（成功完成 measure_round_trip 的次数）.
    pub sample_count: u32,
    /// 采集到的 Duration 列表.
    pub results: Vec<Duration>,
    /// 时钟注入函数（返回纳秒）.
    pub clock_fn: fn() -> u64,
    /// 睡眠注入函数.
    pub sleep_fn: fn(Duration),
}

impl LatencyProbe {
    /// 构造探针，注入 `clock_fn` 与 `sleep_fn`.
    ///
    /// `clock_fn` 返回纳秒（与 `eneros_time::get_monotonic_ns` 兼容）.
    /// `sleep_fn` 接受 `Duration`（生产实现可包装 `eneros_time::sleep_until`）.
    pub fn new(clock_fn: fn() -> u64, sleep_fn: fn(Duration)) -> Self {
        Self {
            sample_count: 0,
            results: Vec::new(),
            clock_fn,
            sleep_fn,
        }
    }

    /// 单次往返时延测量.
    ///
    /// 1. 调用 `clock_fn` 记起始纳秒 `start_ns`
    /// 2. 调用 `send()`，失败立即返回 `Err(())`（不增加 `sample_count`，不调第二次 `clock_fn`）
    /// 3. 调用 `clock_fn` 记结束纳秒 `end_ns`
    /// 4. 返回 `Ok(Duration::from_nanos(end_ns.saturating_sub(start_ns)))`
    ///
    /// 注意：本方法不修改 `sample_count` 或 `results`，由调用者（如 `run_burst`）
    /// 决定何时计入. 这样允许上层组合自定义采集策略.
    ///
    /// 闭包以 `&mut impl FnMut()` 传入（D26），以允许同一闭包在 `run_burst` 等
    /// 多轮循环中被反复调用. 捕获 `&mut T` 的 `driver_send_closure` 返回值天然满足
    /// `FnMut`，普通 `|| Ok(())` 闭包同时满足 `Fn`/`FnMut`/`FnOnce`.
    pub fn measure_round_trip(
        &mut self,
        send: &mut impl FnMut() -> Result<(), ()>,
    ) -> Result<Duration, ()> {
        let start_ns = (self.clock_fn)();
        send()?;
        let end_ns = (self.clock_fn)();
        Ok(Duration::from_nanos(end_ns.saturating_sub(start_ns)))
    }

    /// 突发测量：循环 `count` 次，每次 `measure_round_trip` 后 `sleep_fn(interval)`.
    ///
    /// 即使 `send` 失败也调用 `sleep_fn`，确保采样间隔稳定. 成功的样本被 push 到
    /// `results` 且 `sample_count` 自增.
    pub fn run_burst(
        &mut self,
        count: u32,
        interval: Duration,
        mut send: impl FnMut() -> Result<(), ()>,
    ) -> DelayStats {
        for _ in 0..count {
            if let Ok(d) = self.measure_round_trip(&mut send) {
                self.results.push(d);
                self.sample_count += 1;
            }
            (self.sleep_fn)(interval);
        }
        self.compute_stats()
    }

    /// 基于时长 `duration` 的连续测量.
    ///
    /// 与 `run_burst` 区别：不调用 `sleep_fn`，连续采集直到 `clock_fn() >= deadline`.
    pub fn run(
        &mut self,
        duration: Duration,
        mut send: impl FnMut() -> Result<(), ()>,
    ) -> DelayStats {
        let start_ns = (self.clock_fn)();
        let deadline = start_ns + duration.as_nanos() as u64;
        while (self.clock_fn)() < deadline {
            if let Ok(d) = self.measure_round_trip(&mut send) {
                self.results.push(d);
                self.sample_count += 1;
            }
        }
        self.compute_stats()
    }

    /// 端到端测量便捷方法：等价于 `run_burst(samples, Duration::from_millis(1), send)`.
    ///
    /// D16：蓝图 `topic` / `payload_size` 参数简化为闭包注入（调用者通过
    /// `driver_send_closure` 适配器注入）.
    pub fn measure_e2e(
        &mut self,
        samples: u32,
        mut send: impl FnMut() -> Result<(), ()>,
    ) -> DelayStats {
        self.run_burst(samples, Duration::from_millis(1), &mut send)
    }

    /// 负载下测量：每轮先调用 `background_load()` 注入背景流量，再 measure_round_trip.
    ///
    /// 适用于评估关键 TC 在背景流量下的时延变化.
    pub fn measure_under_load(
        &mut self,
        samples: u32,
        interval: Duration,
        background_load: impl Fn(),
        mut send: impl FnMut() -> Result<(), ()>,
    ) -> DelayStats {
        for _ in 0..samples {
            background_load();
            if let Ok(d) = self.measure_round_trip(&mut send) {
                self.results.push(d);
                self.sample_count += 1;
            }
            (self.sleep_fn)(interval);
        }
        self.compute_stats()
    }

    /// 计算当前 `results` 的统计结果.
    ///
    /// 空结果集返回 `DelayStats::default()`；否则克隆后 `sort()`，计算
    /// min/max/mean/p99/p999/jitter/samples.
    pub fn compute_stats(&self) -> DelayStats {
        if self.results.is_empty() {
            return DelayStats::default();
        }
        let mut sorted = self.results.clone();
        sorted.sort_unstable();
        let n = sorted.len();
        let min = sorted[0];
        let max = sorted[n - 1];
        let total: Duration = sorted.iter().sum();
        let mean = total / n as u32;
        let p99_idx = ((n as f64 * 0.99) as usize).min(n - 1);
        let p999_idx = ((n as f64 * 0.999) as usize).min(n - 1);
        let p99 = sorted[p99_idx];
        let p999 = sorted[p999_idx];
        DelayStats {
            min,
            max,
            mean,
            p99,
            p999,
            jitter: max - min,
            samples: n as u64,
        }
    }
}
