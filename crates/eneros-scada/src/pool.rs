//! 通用连接池（T029-14）
//!
//! 为 SCADA / Modbus / IEC 61850 协议提供统一的 TCP 连接池化能力。
//! 连接池通过 `max_size` 限制最大连接数，通过 `idle_timeout` 自动清理
//! 长时间空闲的连接，压测 1000 并发时连接数稳定在 `max_size` 内。
//!
//! # 架构
//!
//! ```text
//! 调用方（ScadaCollector / ModbusTcpAdapter / Iec61850Adapter）
//!         │
//!         │ acquire()
//!         ▼
//! ConnectionPool<T: Poolable>
//!   ├── Semaphore(max_size)  — 限制总连接数（活跃 + 空闲）
//!   ├── idle: VecDeque<IdleConn<T>>  — 空闲连接队列（FIFO）
//!   └── factory: async fn() -> Result<T>  — 新建连接工厂
//!         │
//!         ▼
//! PooledConnection<T>（RAII）
//!   ├── Deref<Target=T>  — 透明访问底层连接
//!   └── Drop  — 自动归还或关闭连接
//! ```
//!
//! # 线程安全
//!
//! - `ConnectionPool<T>` 内部使用 `Arc<PoolInner<T>>`，可安全克隆到多个任务
//! - 空闲队列使用 `tokio::sync::Mutex` 保护（异步锁）
//! - 连接数限制使用 `tokio::sync::Semaphore`（`OwnedSemaphorePermit` 随连接生命周期）
//!
//! # RAII 归还
//!
//! `PooledConnection<T>` 在 `Drop` 时自动将连接归还到池中（若仍有效）或关闭。
//! 归还逻辑通过 `tokio::spawn` 异步执行，因此必须在 Tokio 运行时上下文中使用。

use std::collections::VecDeque;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tracing::{debug, warn};

use eneros_core::{EnerOSError, Result};

// ---------------------------------------------------------------------------
// Poolable trait
// ---------------------------------------------------------------------------

/// 可池化连接的 trait。
///
/// 每种协议（TCP / Modbus / IEC 61850）的连接类型实现此 trait 后，
/// 即可被 `ConnectionPool<T>` 管理。
///
/// 注意：`is_valid` 使用 `&mut self` 而非 `&self`，这样只需 `T: Send` 而无需 `T: Sync`。
/// 这对于包含 `Box<dyn Trait>` 的连接类型（如 `tokio_modbus::client::Context`）至关重要，
/// 因为它们是 `Send` 但不是 `Sync`。
#[async_trait]
pub trait Poolable: Send + 'static {
    /// 检查连接是否仍然有效。
    ///
    /// 在从池中取出连接时调用，若返回 `false` 则连接被关闭并丢弃，
    /// 池会尝试取下一个空闲连接或新建连接。
    async fn is_valid(&mut self) -> bool;

    /// 关闭连接，释放底层资源（TCP 流、协议会话等）。
    async fn close(&mut self);
}

// ---------------------------------------------------------------------------
// PoolConfig
// ---------------------------------------------------------------------------

/// 连接池配置。
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// 最大连接数（活跃 + 空闲）。默认 16。
    pub max_size: usize,
    /// 空闲连接超时时间。超过此时间的空闲连接将在下次 `acquire` 时被清理。默认 30s。
    pub idle_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 16,
            idle_timeout: Duration::from_secs(30),
        }
    }
}

impl PoolConfig {
    /// 创建指定 `max_size` 和 `idle_timeout` 的配置。
    pub fn new(max_size: usize, idle_timeout: Duration) -> Self {
        Self { max_size, idle_timeout }
    }
}

// ---------------------------------------------------------------------------
// PoolStats
// ---------------------------------------------------------------------------

/// 连接池运行时统计信息。
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// 当前活跃（已借出）连接数
    pub active: usize,
    /// 当前空闲连接数
    pub idle: usize,
    /// 累计创建连接数
    pub total_created: u64,
    /// 累计获取连接数（含复用）
    pub total_acquired: u64,
    /// 累计归还连接数
    pub total_released: u64,
    /// 累计关闭连接数（含超时清理、无效清理、池关闭）
    pub total_closed: u64,
}

// ---------------------------------------------------------------------------
// ConnectionPool<T>
// ---------------------------------------------------------------------------

/// 异步连接工厂函数类型：`async fn() -> Result<T>`。
type ConnFactory<T> = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<T>> + Send + 'static>> + Send + Sync + 'static,
>;

/// 空闲连接条目（连接 + 空闲起始时间 + 信号量许可）。
struct IdleConn<T> {
    conn: T,
    idle_since: Instant,
    /// 随连接一起保存的信号量许可。连接归还时许可一并存入空闲队列；
    /// 连接被借出时许可随连接一起移交。这保证信号量始终准确反映
    /// 总连接数（活跃 + 空闲）。
    _permit: OwnedSemaphorePermit,
}

/// 连接池内部状态（被 `Arc` 共享）。
struct PoolInner<T: Poolable> {
    /// 空闲连接队列（FIFO，先入先出以均衡使用）
    idle: Mutex<VecDeque<IdleConn<T>>>,
    config: PoolConfig,
    factory: ConnFactory<T>,
    /// 活跃（已借出）连接数
    active: AtomicUsize,
    /// 空闲连接数（与 `idle` 队列长度同步，用于无锁读取 stats）
    idle_count: AtomicUsize,
    /// 累计创建连接数
    total_created: AtomicU64,
    /// 累计获取连接数
    total_acquired: AtomicU64,
    /// 累计归还连接数
    total_released: AtomicU64,
    /// 累计关闭连接数
    total_closed: AtomicU64,
    /// 信号量：限制总连接数（活跃 + 空闲）不超过 `max_size`
    semaphore: Arc<Semaphore>,
    /// 池是否已关闭（0 = 开启，1 = 关闭）
    closed: AtomicUsize,
    /// 通知机制：当连接归还到空闲队列时唤醒等待中的 acquire()
    notify: Notify,
}

impl<T: Poolable> PoolInner<T> {
    /// 归还连接到池中（由 `PooledConnection::drop` 调用）。
    async fn return_connection(&self, mut conn: T, permit: OwnedSemaphorePermit) {
        self.total_released.fetch_add(1, Ordering::Relaxed);
        self.active.fetch_sub(1, Ordering::Relaxed);

        // 池已关闭 → 关闭连接
        if self.closed.load(Ordering::SeqCst) == 1 {
            conn.close().await;
            self.total_closed.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // 连接无效 → 关闭
        if !conn.is_valid().await {
            conn.close().await;
            self.total_closed.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // 归还到空闲队列
        let entry = IdleConn {
            conn,
            idle_since: Instant::now(),
            _permit: permit,
        };
        let mut idle = self.idle.lock().await;
        self.idle_count.fetch_add(1, Ordering::Relaxed);
        idle.push_back(entry);
        drop(idle);

        // 唤醒一个等待 acquire() 的任务，让它从空闲队列取连接
        self.notify.notify_one();
    }
}

/// 通用连接池。
///
/// 通过 `acquire()` 获取连接，`PooledConnection` 在 `Drop` 时自动归还。
/// 连接池可克隆（内部 `Arc` 共享），多任务共享同一池实例。
///
/// # 示例
///
/// ```no_run
/// use std::time::Duration;
/// use eneros_scada::pool::{ConnectionPool, PoolConfig, PooledTcpStream};
///
/// # #[tokio::main]
/// # async fn main() -> eneros_core::Result<()> {
/// let pool = ConnectionPool::new(
///     PoolConfig::new(16, Duration::from_secs(30)),
///     || async {
///         let stream = tokio::net::TcpStream::connect("127.0.0.1:502").await?;
///         Ok(PooledTcpStream::new(stream))
///     },
/// );
///
/// let mut conn = pool.acquire().await?;
/// // 使用 conn（通过 Deref 访问底层 TcpStream）
/// conn.mark_invalid(); // 若出错则标记无效，归还时将被关闭
/// # Ok(())
/// # }
/// ```
pub struct ConnectionPool<T: Poolable> {
    inner: Arc<PoolInner<T>>,
}

impl<T: Poolable> Clone for ConnectionPool<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T: Poolable> ConnectionPool<T> {
    /// 创建连接池。
    ///
    /// `factory` 是一个异步闭包，用于新建连接。每次池需要新建连接时调用。
    pub fn new<F, Fut>(config: PoolConfig, factory: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        assert!(config.max_size > 0, "连接池 max_size 必须 > 0");
        let factory: ConnFactory<T> = Arc::new(move || {
            let fut = factory();
            Box::pin(fut)
        });
        let semaphore = Arc::new(Semaphore::new(config.max_size));
        Self {
            inner: Arc::new(PoolInner {
                idle: Mutex::new(VecDeque::new()),
                config,
                factory,
                active: AtomicUsize::new(0),
                idle_count: AtomicUsize::new(0),
                total_created: AtomicU64::new(0),
                total_acquired: AtomicU64::new(0),
                total_released: AtomicU64::new(0),
                total_closed: AtomicU64::new(0),
                semaphore,
                closed: AtomicUsize::new(0),
                notify: Notify::new(),
            }),
        }
    }

    /// 从池中获取一个连接。
    ///
    /// 1. 优先从空闲队列取连接（FIFO），跳过已超时或无效的连接
    /// 2. 若无可用空闲连接，通过 `factory` 新建
    /// 3. 若总连接数已达 `max_size`，等待其他连接归还或关闭
    ///
    /// 返回 `PooledConnection<T>`，`Drop` 时自动归还。
    pub async fn acquire(&self) -> Result<PooledConnection<T>> {
        if self.inner.closed.load(Ordering::SeqCst) == 1 {
            return Err(EnerOSError::Device("连接池已关闭".into()));
        }

        loop {
            // 池已关闭则直接返回错误
            if self.inner.closed.load(Ordering::SeqCst) == 1 {
                return Err(EnerOSError::Device("连接池已关闭".into()));
            }

            // 1. 尝试从空闲队列取连接
            loop {
                let entry = self.inner.idle.lock().await.pop_front();
                match entry {
                    Some(entry) => {
                        // 检查空闲超时
                        if entry.idle_since.elapsed() > self.inner.config.idle_timeout {
                            self.inner.idle_count.fetch_sub(1, Ordering::Relaxed);
                            let mut conn = entry.conn;
                            conn.close().await;
                            self.inner.total_closed.fetch_add(1, Ordering::Relaxed);
                            debug!("连接池: 清理超时空闲连接");
                            // permit 随 entry drop 而释放，唤醒等待者
                            self.inner.notify.notify_one();
                            continue;
                        }
                        // 检查连接有效性
                        let mut conn = entry.conn;
                        if !conn.is_valid().await {
                            self.inner.idle_count.fetch_sub(1, Ordering::Relaxed);
                            conn.close().await;
                            self.inner.total_closed.fetch_add(1, Ordering::Relaxed);
                            debug!("连接池: 清理无效空闲连接");
                            // permit 随 entry drop 而释放，唤醒等待者
                            self.inner.notify.notify_one();
                            continue;
                        }
                        // 获得有效连接
                        self.inner.active.fetch_add(1, Ordering::Relaxed);
                        self.inner.total_acquired.fetch_add(1, Ordering::Relaxed);
                        return Ok(PooledConnection {
                            inner: Some(conn),
                            pool: self.inner.clone(),
                            _permit: Some(entry._permit),
                        });
                    }
                    None => break, // 无空闲连接，转新建
                }
            }

            // 2. 获取信号量许可（限制总连接数）
            //    与 notify 竞争：若其他任务归还连接到空闲队列，
            //    notify 触发后回到步骤 1 重试从空闲队列获取
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);

            let permit = tokio::select! {
                p = self.inner.semaphore.clone().acquire_owned() => {
                    match p {
                        Ok(permit) => permit,
                        Err(_) => return Err(EnerOSError::Device("连接池信号量已关闭".into())),
                    }
                }
                _ = &mut notified => {
                    // 有连接归还到空闲队列，回到循环顶部重试
                    continue;
                }
            };

            // 3. 通过工厂新建连接
            let conn = match (self.inner.factory)().await {
                Ok(c) => c,
                Err(e) => {
                    // 工厂失败，许可随返回值 drop 而释放
                    return Err(e);
                }
            };

            self.inner.total_created.fetch_add(1, Ordering::Relaxed);
            self.inner.active.fetch_add(1, Ordering::Relaxed);
            self.inner.total_acquired.fetch_add(1, Ordering::Relaxed);

            return Ok(PooledConnection {
                inner: Some(conn),
                pool: self.inner.clone(),
                _permit: Some(permit),
            });
        }
    }

    /// 获取连接池统计信息。
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            active: self.inner.active.load(Ordering::Relaxed),
            idle: self.inner.idle_count.load(Ordering::Relaxed),
            total_created: self.inner.total_created.load(Ordering::Relaxed),
            total_acquired: self.inner.total_acquired.load(Ordering::Relaxed),
            total_released: self.inner.total_released.load(Ordering::Relaxed),
            total_closed: self.inner.total_closed.load(Ordering::Relaxed),
        }
    }

    /// 获取连接池配置。
    pub fn config(&self) -> &PoolConfig {
        &self.inner.config
    }

    /// 关闭连接池。
    ///
    /// - 标记池为已关闭，`acquire()` 将返回错误
    /// - 关闭所有空闲连接
    /// - 已借出的连接在归还时会被关闭（不会重新入队）
    pub async fn close(&self) {
        self.inner.closed.store(1, Ordering::SeqCst);
        let mut idle = self.inner.idle.lock().await;
        while let Some(entry) = idle.pop_front() {
            let mut conn = entry.conn;
            conn.close().await;
            self.inner.total_closed.fetch_add(1, Ordering::Relaxed);
            // permit 随 entry drop 释放
        }
        self.inner.idle_count.store(0, Ordering::Relaxed);
        // 唤醒所有等待 acquire() 的任务，让它们检查 closed 标志并返回错误
        self.inner.notify.notify_waiters();
        debug!("连接池: 已关闭，所有空闲连接已释放");
    }

    /// 主动清理超时空闲连接（可在后台定时调用）。
    pub async fn cleanup_expired(&self) -> usize {
        let mut cleaned = 0usize;
        let mut idle = self.inner.idle.lock().await;
        let timeout = self.inner.config.idle_timeout;
        let now = Instant::now();

        // 保留未超时的连接，关闭超时的
        let mut remaining = VecDeque::with_capacity(idle.len());
        while let Some(entry) = idle.pop_front() {
            if now.duration_since(entry.idle_since) > timeout {
                let mut conn = entry.conn;
                conn.close().await;
                self.inner.total_closed.fetch_add(1, Ordering::Relaxed);
                self.inner.idle_count.fetch_sub(1, Ordering::Relaxed);
                cleaned += 1;
            } else {
                remaining.push_back(entry);
            }
        }
        *idle = remaining;
        if cleaned > 0 {
            debug!("连接池: 主动清理 {} 个超时空闲连接", cleaned);
            // permit 随超时 entry drop 而释放，唤醒等待者
            self.inner.notify.notify_one();
        }
        cleaned
    }
}

// ---------------------------------------------------------------------------
// PooledConnection<T>
// ---------------------------------------------------------------------------

/// 池化连接句柄（RAII）。
///
/// 通过 `Deref` / `DerefMut` 透明访问底层连接 `T`。
/// `Drop` 时自动将连接归还到池中（若仍有效）或关闭。
pub struct PooledConnection<T: Poolable> {
    inner: Option<T>,
    pool: Arc<PoolInner<T>>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl<T: Poolable> PooledConnection<T> {
    /// 取出底层连接（不再归还到池中）。
    ///
    /// 调用方负责关闭返回的连接。信号量许可随 `PooledConnection` drop 而释放。
    pub fn take(mut self) -> Option<T> {
        self.inner.take()
    }

    /// 获取底层连接的引用。
    pub fn as_ref(&self) -> Option<&T> {
        self.inner.as_ref()
    }

    /// 获取底层连接的可变引用。
    pub fn as_mut(&mut self) -> Option<&mut T> {
        self.inner.as_mut()
    }
}

impl<T: Poolable> Deref for PooledConnection<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("PooledConnection inner 不应为 None")
    }
}

impl<T: Poolable> DerefMut for PooledConnection<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("PooledConnection inner 不应为 None")
    }
}

impl<T: Poolable> Drop for PooledConnection<T> {
    fn drop(&mut self) {
        let conn = self.inner.take();
        let permit = self._permit.take();

        match (conn, permit) {
            (Some(conn), Some(permit)) => {
                let pool = self.pool.clone();
                // 在 Tokio 运行时上下文中异步归还连接
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        handle.spawn(async move {
                            pool.return_connection(conn, permit).await;
                        });
                    }
                    Err(_) => {
                        // 无运行时上下文，无法异步关闭连接。
                        // 连接和许可随 drop 释放（TcpStream drop 会关闭 socket）。
                        warn!("PooledConnection 在无 Tokio 运行时上下文中 drop，连接未正常归还");
                        drop(conn);
                        drop(permit);
                    }
                }
            }
            (Some(conn), None) => {
                drop(conn);
            }
            (None, Some(permit)) => {
                // 连接已被 take() 取出，但许可仍持有。
                // 递减活跃计数并释放许可，唤醒等待者。
                self.pool.active.fetch_sub(1, Ordering::Relaxed);
                self.pool.total_released.fetch_add(1, Ordering::Relaxed);
                drop(permit);
                self.pool.notify.notify_one();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// PooledTcpStream — TCP 连接的 Poolable 实现
// ---------------------------------------------------------------------------

/// 包装 `tokio::net::TcpStream` 的池化 TCP 连接。
///
/// 用于 SCADA IEC 104 等基于 TCP 的协议连接池化。
#[derive(Debug)]
pub struct PooledTcpStream {
    stream: Option<TcpStream>,
    valid: bool,
}

impl PooledTcpStream {
    /// 创建新的池化 TCP 连接。
    pub fn new(stream: TcpStream) -> Self {
        Self { stream: Some(stream), valid: true }
    }

    /// 获取底层 TCP 流的引用。
    pub fn stream(&self) -> Option<&TcpStream> {
        self.stream.as_ref()
    }

    /// 获取底层 TCP 流的可变引用。
    pub fn stream_mut(&mut self) -> Option<&mut TcpStream> {
        self.stream.as_mut()
    }

    /// 标记连接为无效（出错时调用）。归还时将被关闭而非复用。
    pub fn mark_invalid(&mut self) {
        self.valid = false;
    }
}

#[async_trait]
impl Poolable for PooledTcpStream {
    async fn is_valid(&mut self) -> bool {
        self.valid && self.stream.is_some()
    }

    async fn close(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.shutdown().await;
        }
    }
}

// ---------------------------------------------------------------------------
// PooledModbusConn — Modbus TCP 连接的 Poolable 实现
// ---------------------------------------------------------------------------

/// 包装 `tokio_modbus::client::Context` 的池化 Modbus 连接。
///
/// 用于 Modbus TCP 协议连接池化，支持多请求复用同一 TCP 连接。
pub struct PooledModbusConn {
    ctx: Option<tokio_modbus::client::Context>,
    valid: bool,
}

impl PooledModbusConn {
    /// 创建新的池化 Modbus 连接。
    pub fn new(ctx: tokio_modbus::client::Context) -> Self {
        Self { ctx: Some(ctx), valid: true }
    }

    /// 获取底层 Modbus Context 的引用。
    pub fn ctx(&self) -> Option<&tokio_modbus::client::Context> {
        self.ctx.as_ref()
    }

    /// 获取底层 Modbus Context 的可变引用。
    pub fn ctx_mut(&mut self) -> Option<&mut tokio_modbus::client::Context> {
        self.ctx.as_mut()
    }

    /// 标记连接为无效（出错时调用）。
    pub fn mark_invalid(&mut self) {
        self.valid = false;
    }
}

#[async_trait]
impl Poolable for PooledModbusConn {
    async fn is_valid(&mut self) -> bool {
        self.valid && self.ctx.is_some()
    }

    async fn close(&mut self) {
        // tokio_modbus::client::Context 无显式 close，drop 即关闭底层 TCP
        self.ctx.take();
    }
}

// ---------------------------------------------------------------------------
// PooledMmsConn — IEC 61850 MMS 连接的 Poolable 实现
// ---------------------------------------------------------------------------

/// 包装 `eneros_device::adapters::iec61850::mms::MmsClient` 的池化 IEC 61850 连接。
///
/// 用于 IEC 61850 MMS 协议连接池化。
pub struct PooledMmsConn {
    client: Option<eneros_device::adapters::iec61850::mms::MmsClient>,
    valid: bool,
}

impl PooledMmsConn {
    /// 创建新的池化 IEC 61850 MMS 连接。
    pub fn new(client: eneros_device::adapters::iec61850::mms::MmsClient) -> Self {
        Self { client: Some(client), valid: true }
    }

    /// 获取底层 MmsClient 的引用。
    pub fn client(&self) -> Option<&eneros_device::adapters::iec61850::mms::MmsClient> {
        self.client.as_ref()
    }

    /// 获取底层 MmsClient 的可变引用。
    pub fn client_mut(&mut self) -> Option<&mut eneros_device::adapters::iec61850::mms::MmsClient> {
        self.client.as_mut()
    }

    /// 标记连接为无效（出错时调用）。
    pub fn mark_invalid(&mut self) {
        self.valid = false;
    }
}

#[async_trait]
impl Poolable for PooledMmsConn {
    async fn is_valid(&mut self) -> bool {
        self.valid && self.client.is_some()
    }

    async fn close(&mut self) {
        if let Some(mut client) = self.client.take() {
            let _ = client.disconnect().await;
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// 测试用的简单池化连接（计数器，无真实 TCP）
    struct TestConn {
        id: u64,
        valid: bool,
    }

    #[async_trait]
    impl Poolable for TestConn {
        async fn is_valid(&mut self) -> bool {
            self.valid
        }
        async fn close(&mut self) {
            self.valid = false;
        }
    }

    fn make_test_pool(
        max_size: usize,
        idle_timeout: Duration,
    ) -> (ConnectionPool<TestConn>, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let pool = ConnectionPool::new(PoolConfig::new(max_size, idle_timeout), move || {
            let c = counter_clone.clone();
            async move {
                let id = c.fetch_add(1, Ordering::SeqCst) as u64 + 1;
                Ok(TestConn { id, valid: true })
            }
        });
        (pool, counter)
    }

    #[tokio::test]
    async fn test_acquire_release_basic() {
        let (pool, counter) = make_test_pool(4, Duration::from_secs(30));

        let conn = pool.acquire().await.unwrap();
        assert_eq!(conn.inner.as_ref().unwrap().id, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        let stats = pool.stats();
        assert_eq!(stats.active, 1);
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.total_created, 1);

        drop(conn);

        // 等待异步归还
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.active, 0);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_released, 1);
    }

    #[tokio::test]
    async fn test_pool_reuses_connections() {
        let (pool, counter) = make_test_pool(4, Duration::from_secs(30));

        {
            let conn = pool.acquire().await.unwrap();
            assert_eq!(conn.inner.as_ref().unwrap().id, 1);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        {
            let conn = pool.acquire().await.unwrap();
            // 应复用 id=1 的连接，而非新建
            assert_eq!(conn.inner.as_ref().unwrap().id, 1);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1); // 只创建了 1 个连接
        let stats = pool.stats();
        assert_eq!(stats.total_created, 1);
        assert_eq!(stats.total_acquired, 2);
    }

    #[tokio::test]
    async fn test_pool_respects_max_size() {
        let (pool, _counter) = make_test_pool(3, Duration::from_secs(30));

        let c1 = pool.acquire().await.unwrap();
        let _c2 = pool.acquire().await.unwrap();
        let _c3 = pool.acquire().await.unwrap();

        let stats = pool.stats();
        assert_eq!(stats.active, 3);
        assert_eq!(stats.total_created, 3);

        // 第 4 个 acquire 应阻塞（无可用连接）
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            let _c4 = pool_clone.acquire().await.unwrap();
        });

        // 确认它确实在等待
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!handle.is_finished());

        // 归还一个连接
        drop(c1);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 现在第 4 个应完成
        handle.await.unwrap();
        // 等待 _c4 drop 后的异步归还完成
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        // c2, c3 仍活跃；c1 被 c4 复用后归还到空闲队列
        assert_eq!(stats.active, 2);
        // 未创建新连接，复用了 c1
        assert_eq!(stats.total_created, 3);
    }

    #[tokio::test]
    async fn test_idle_timeout_cleanup() {
        let (pool, counter) = make_test_pool(4, Duration::from_millis(100));

        {
            let conn = pool.acquire().await.unwrap();
            assert_eq!(conn.inner.as_ref().unwrap().id, 1);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.idle, 1);

        // 等待超时
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 下次 acquire 应清理超时连接并新建
        let conn = pool.acquire().await.unwrap();
        assert_eq!(conn.inner.as_ref().unwrap().id, 2); // 新建
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        let stats = pool.stats();
        assert_eq!(stats.total_closed, 1); // 超时的被关闭
    }

    #[tokio::test]
    async fn test_invalid_connection_not_returned() {
        let (pool, _counter) = make_test_pool(4, Duration::from_secs(30));

        {
            let mut conn = pool.acquire().await.unwrap();
            conn.inner.as_mut().unwrap().valid = false; // 标记无效
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.idle, 0); // 无效连接未归还
        assert_eq!(stats.total_closed, 1);
    }

    #[tokio::test]
    async fn test_concurrent_acquire() {
        let (pool, counter) = make_test_pool(8, Duration::from_secs(30));
        let pool_clone = pool.clone();

        let handles: Vec<_> = (0..20)
            .map(|i| {
                let p = pool_clone.clone();
                tokio::spawn(async move {
                    let conn = p.acquire().await.unwrap();
                    // 模拟工作
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let id = conn.inner.as_ref().unwrap().id;
                    drop(conn);
                    (i, id)
                })
            })
            .collect();

        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.unwrap());
        }

        assert_eq!(results.len(), 20);
        // 创建的连接数不应超过 max_size
        let stats = pool.stats();
        assert!(
            stats.total_created <= 8,
            "创建连接数 {} 超过 max_size 8",
            stats.total_created
        );
        assert_eq!(counter.load(Ordering::SeqCst), stats.total_created as usize);
    }

    #[tokio::test]
    async fn test_pool_close() {
        let (pool, _counter) = make_test_pool(4, Duration::from_secs(30));

        let conn = pool.acquire().await.unwrap();
        let _conn2 = pool.acquire().await.unwrap();

        // 归还一个
        drop(conn);
        tokio::time::sleep(Duration::from_millis(50)).await;

        pool.close().await;

        let stats = pool.stats();
        assert_eq!(stats.idle, 0); // 空闲连接已关闭

        // acquire 应失败
        let result = pool.acquire().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pool_stats_accuracy() {
        let (pool, _counter) = make_test_pool(2, Duration::from_secs(30));

        let c1 = pool.acquire().await.unwrap();
        let c2 = pool.acquire().await.unwrap();

        let stats = pool.stats();
        assert_eq!(stats.active, 2);
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.total_created, 2);
        assert_eq!(stats.total_acquired, 2);

        drop(c1);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.active, 1);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_released, 1);

        drop(c2);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.active, 0);
        assert_eq!(stats.idle, 2);
        assert_eq!(stats.total_released, 2);
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let (pool, _counter) = make_test_pool(4, Duration::from_millis(100));

        {
            let _c = pool.acquire().await.unwrap();
            let _c2 = pool.acquire().await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.idle, 2);

        // 等待超时
        tokio::time::sleep(Duration::from_millis(100)).await;

        let cleaned = pool.cleanup_expired().await;
        assert_eq!(cleaned, 2);

        let stats = pool.stats();
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.total_closed, 2);
    }

    // ---- PooledTcpStream 测试 ----

    #[tokio::test]
    async fn test_pooled_tcp_stream_with_real_server() {
        // 启动 mock TCP 服务器
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connection_count = Arc::new(AtomicUsize::new(0));
        let cc = connection_count.clone();

        tokio::spawn(async move {
            while let Ok((_stream, _)) = listener.accept().await {
                cc.fetch_add(1, Ordering::SeqCst);
                // 保持连接打开
            }
        });

        let pool = ConnectionPool::new(
            PoolConfig::new(4, Duration::from_secs(30)),
            move || async move {
                let stream = TcpStream::connect(addr).await?;
                Ok(PooledTcpStream::new(stream))
            },
        );

        // 获取连接
        let c1 = pool.acquire().await.unwrap();
        let c2 = pool.acquire().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(connection_count.load(Ordering::SeqCst), 2);

        drop(c1);
        drop(c2);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 复用连接
        let c3 = pool.acquire().await.unwrap();
        let c4 = pool.acquire().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(connection_count.load(Ordering::SeqCst), 2); // 未新建

        let stats = pool.stats();
        assert_eq!(stats.total_created, 2);

        drop(c3);
        drop(c4);
    }

    #[tokio::test]
    async fn test_pooled_tcp_stream_mark_invalid() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            while listener.accept().await.is_ok() {}
        });

        let pool = ConnectionPool::new(
            PoolConfig::new(4, Duration::from_secs(30)),
            move || async move {
                let stream = TcpStream::connect(addr).await?;
                Ok(PooledTcpStream::new(stream))
            },
        );

        {
            let mut conn = pool.acquire().await.unwrap();
            conn.mark_invalid(); // 标记无效
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = pool.stats();
        assert_eq!(stats.idle, 0); // 无效连接未归还
        assert_eq!(stats.total_closed, 1);
    }

    #[tokio::test]
    async fn test_pooled_tcp_stream_take() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            while listener.accept().await.is_ok() {}
        });

        let pool = ConnectionPool::new(
            PoolConfig::new(4, Duration::from_secs(30)),
            move || async move {
                let stream = TcpStream::connect(addr).await?;
                Ok(PooledTcpStream::new(stream))
            },
        );

        let conn = pool.acquire().await.unwrap();
        let _stream = conn.take().unwrap();

        let stats = pool.stats();
        assert_eq!(stats.active, 0); // take 后 active 归零
        assert_eq!(stats.idle, 0); // 未归还
    }

    // ---- 压测：1000 并发，max_size=16 ----

    #[tokio::test]
    async fn test_stress_1000_concurrent_max_size_16() {
        // 启动 mock TCP 服务器，跟踪连接数
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let current_concurrent = Arc::new(AtomicUsize::new(0));
        let total_connections = Arc::new(AtomicUsize::new(0));

        let mc = max_concurrent.clone();
        let cc = current_concurrent.clone();
        let tc = total_connections.clone();

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let cur = cc.fetch_add(1, Ordering::SeqCst) + 1;
                tc.fetch_add(1, Ordering::SeqCst);
                // 更新峰值
                loop {
                    let prev = mc.load(Ordering::SeqCst);
                    if cur <= prev
                        || mc
                            .compare_exchange(prev, cur, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        break;
                    }
                }

                // 保持连接直到客户端关闭
                let mut buf = [0u8; 1];
                let _ = stream.read(&mut buf).await;
                cc.fetch_sub(1, Ordering::SeqCst);
            }
        });

        let max_size = 16usize;
        let pool = ConnectionPool::new(
            PoolConfig::new(max_size, Duration::from_secs(30)),
            move || async move {
                let stream = TcpStream::connect(addr).await?;
                Ok(PooledTcpStream::new(stream))
            },
        );

        // 1000 并发请求
        let total_requests = 1000usize;
        let mut handles = Vec::with_capacity(total_requests);

        for _i in 0..total_requests {
            let p = pool.clone();
            handles.push(tokio::spawn(async move {
                let conn = p.acquire().await.unwrap();
                // 模拟短暂工作
                tokio::time::sleep(Duration::from_millis(1)).await;
                drop(conn);
            }));
        }

        // 等待所有完成
        for h in handles {
            h.await.unwrap();
        }

        // 等待所有归还完成
        tokio::time::sleep(Duration::from_millis(100)).await;

        let stats = pool.stats();
        let peak = max_concurrent.load(Ordering::SeqCst);
        let total = total_connections.load(Ordering::SeqCst);

        // 验证：所有请求完成
        assert_eq!(stats.total_acquired, total_requests as u64);
        // 验证：创建的连接数不超过 max_size
        assert!(
            stats.total_created <= max_size as u64,
            "创建连接数 {} 超过 max_size {}",
            stats.total_created,
            max_size
        );
        // 验证：服务器端峰值并发连接数不超过 max_size
        assert!(
            peak <= max_size,
            "服务器端峰值并发连接数 {} 超过 max_size {}",
            peak,
            max_size
        );
        // 验证：服务器端总连接数不超过 max_size
        assert!(
            total <= max_size,
            "服务器端总连接数 {} 超过 max_size {}",
            total,
            max_size
        );

        println!(
            "压测结果: 1000 并发, max_size={}, 创建连接={}, 峰值并发={}, 总连接={}",
            max_size, stats.total_created, peak, total
        );
    }

    #[tokio::test]
    async fn test_factory_error_handling() {
        let pool: ConnectionPool<TestConn> = ConnectionPool::new(
            PoolConfig::new(4, Duration::from_secs(30)),
            || async { Err(EnerOSError::Device("工厂失败".to_string())) },
        );

        let result = pool.acquire().await;
        assert!(result.is_err());

        let stats = pool.stats();
        assert_eq!(stats.total_created, 0);
        assert_eq!(stats.active, 0);
    }
}
