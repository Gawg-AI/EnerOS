//! v0.81.0 TSN 驱动抽象层 — 数据面 send/recv 抽象（无真实 netlink/socket）.
//!
//! 在 v0.80.0 TAS 配置面（[`crate::tas::NicApplier`]）之上，建立数据面抽象
//! [`TsnDriver`] trait，提供按 [`TrafficClass`] 发送 payload 与接收下一个数据包
//! 的接口。本模块交付纯 Rust trait + [`MockTsnDriver`]（D8：无真实 netlink/socket），
//! 真实网卡数据面集成延后到 v0.82.0+ Agent 使用阶段。
//!
//! 同时提供 [`driver_send_closure`] 适配器，将 `TsnDriver::send` 包装为
//! [`crate::latency_probe::LatencyProbe`] 所需的 `FnMut() -> Result<(), ()>` 闭包
//! （D26：`send(&mut self, ...)` 要求闭包实现 `FnMut` 而非 `Fn`），
//! 桥接驱动抽象与时延探针.
//!
//! # 核心类型
//!
//! - [`TsnError`] — TSN 驱动错误枚举（SendFailed / RecvFailed / NotInitialized）
//! - [`TsnDriver`] — TSN 数据面 trait（send / recv）
//! - [`MockTsnDriver`] — 测试用 Mock 实现（记录发送队列 + 接收队列）
//! - [`driver_send_closure`] — 闭包适配器

use alloc::vec::Vec;

use crate::tas::TrafficClass;

/// TSN 驱动错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsnError {
    /// `send()` 失败（队列满 / 网卡未就绪 / 内部错误）.
    SendFailed,
    /// `recv()` 失败（队列空 / 超时 / 内部错误）.
    RecvFailed,
    /// 驱动未初始化（未调用 `init()`）.
    NotInitialized,
}

/// TSN 网络驱动数据面 trait.
///
/// 抽象 TSN 网卡的数据面操作：按 [`TrafficClass`] 发送 payload，以及接收下一个
/// 数据包。真实实现由 v0.82.0+ Agent Runtime 提供（封装 netlink socket 或硬件驱动
/// 接口）；本版本仅提供 [`MockTsnDriver`] 用于测试.
pub trait TsnDriver {
    /// 按 `tc` 优先级发送 `payload`，失败返回 [`TsnError::SendFailed`] 等.
    fn send(&mut self, tc: TrafficClass, payload: &[u8]) -> Result<(), TsnError>;

    /// 接收下一个数据包，无数据时返回 [`TsnError::RecvFailed`].
    fn recv(&mut self) -> Result<Vec<u8>, TsnError>;
}

/// 测试用 Mock TSN 驱动.
///
/// 记录所有 `send()` 调用至 `sent` 队列；`recv()` 从 `recv_queue` 弹出数据
/// （后进先出，使用 `Vec::pop` 简化）.
///
/// # 字段
///
/// - `sent` — 已发送的 (TrafficClass, payload) 列表
/// - `recv_queue` — 待接收的数据包队列（`push_recv` 推入，`recv` 弹出）
/// - `fail_send` — 强制 `send()` 返回 `Err(SendFailed)`
/// - `fail_recv` — 强制 `recv()` 返回 `Err(RecvFailed)`
#[derive(Debug, Clone, Default)]
pub struct MockTsnDriver {
    /// 已发送的 (TrafficClass, payload) 列表.
    pub sent: Vec<(TrafficClass, Vec<u8>)>,
    /// 待接收的数据包队列（后进先出）.
    pub recv_queue: Vec<Vec<u8>>,
    /// 强制 `send()` 返回 `Err(SendFailed)`.
    pub fail_send: bool,
    /// 强制 `recv()` 返回 `Err(RecvFailed)`.
    pub fail_recv: bool,
}

impl MockTsnDriver {
    /// 构造空 Mock（队列空、不强制失败）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 推入一个待接收数据包到 `recv_queue`.
    pub fn push_recv(&mut self, data: Vec<u8>) {
        self.recv_queue.push(data);
    }
}

impl TsnDriver for MockTsnDriver {
    fn send(&mut self, tc: TrafficClass, payload: &[u8]) -> Result<(), TsnError> {
        if self.fail_send {
            return Err(TsnError::SendFailed);
        }
        self.sent.push((tc, payload.to_vec()));
        Ok(())
    }

    fn recv(&mut self) -> Result<Vec<u8>, TsnError> {
        if self.fail_recv {
            return Err(TsnError::RecvFailed);
        }
        self.recv_queue.pop().ok_or(TsnError::RecvFailed)
    }
}

/// 将 [`TsnDriver::send`] 包装为 [`crate::latency_probe::LatencyProbe`] 所需的闭包.
///
/// 返回 `impl FnMut() -> Result<(), ()> + 'a`，内部调用 `driver.send(tc, payload)`，
/// 将 `Result<(), TsnError>` 映射为 `Result<(), ()>`（错误细节丢弃，仅传递成败）.
///
/// # 为什么是 `FnMut` 而非 `Fn`（D26）
///
/// `driver.send(...)` 要求 `&mut self`，闭包捕获 `&'a mut dyn TsnDriver` 后调用
/// `&mut self` 方法 — Rust 类型系统约束该闭包只能实现 `FnMut`，不能实现 `Fn`
/// （`Fn::call(&self, ...)` 仅给出 `&self`，无法从 `&&mut T` 获取 `&mut T`）.
/// 因此 [`LatencyProbe::measure_round_trip`] 等方法签名同步改为 `&mut impl FnMut()`.
///
/// # 示例
///
/// ```ignore
/// use eneros_tsn_time::{driver_send_closure, MockTsnDriver, LatencyProbe, TrafficClass};
/// use core::time::Duration;
///
/// fn test_clock() -> u64 { 0 }
/// fn test_sleep(_: Duration) {}
///
/// let mut driver = MockTsnDriver::new();
/// let mut probe = LatencyProbe::new(test_clock, test_sleep);
/// let mut send = driver_send_closure(&mut driver, TrafficClass::CA, &[0x01, 0x02]);
/// let _ = probe.measure_round_trip(&mut send);
/// ```
pub fn driver_send_closure<'a>(
    driver: &'a mut dyn TsnDriver,
    tc: TrafficClass,
    payload: &'a [u8],
) -> impl FnMut() -> Result<(), ()> + 'a {
    move || driver.send(tc, payload).map_err(|_| ())
}
