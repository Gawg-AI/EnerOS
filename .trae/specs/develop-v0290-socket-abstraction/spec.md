# v0.29.0 Socket 抽象层 Spec

## Why

v0.28.0 提供了底层 TcpSocket/UdpSocket/IcmpSocket 包装，但每个操作都需要传入 `&mut NetworkInterface`，对上层应用（Modbus TCP、IEC 104、Agent 通信）使用不便。v0.29.0 在 tcpip 层之上构建统一 Socket API 抽象层：通过 SocketManager 集中管理 NetworkInterface 和所有 Socket，提供标准化的 read/write/close 接口、非阻塞 IO、select/poll 多路复用，使上层应用无需关心底层协议细节。

## What Changes

- **新增 socket/ 子模块**（5 个源文件，添加到 eneros-net crate）：
  - `socket/mod.rs` — 模块声明 + re-exports + crate 文档
  - `socket/api.rs` — `Socket` trait + `SocketId` / `SocketKind` / `SocketError` + `TcpStream` / `TcpListener` / `UdpSocket` 句柄类型
  - `socket/manager.rs` — `SocketManager<D: NetDevice>` 集中管理 NetworkInterface + 所有 Socket
  - `socket/poll.rs` — `Poll` / `Interest` / `Readiness` + 注册/注销/轮询
  - `socket/event.rs` — `Event` 事件类型
- **修改 lib.rs**：添加 `pub mod socket;` + re-exports + VERSION 升级至 "0.29.0"
- **新增文档**：`docs/drivers/socket-abstraction-design.md`
- **新增配置**：`configs/socket.toml`（默认 Socket 配置：缓冲区大小、最大连接数、poll 超时）
- **版本标识更新**：根 `Cargo.toml`、`crates/drivers/net/Cargo.toml`、`Makefile`、`ci.yml`、`gate.rs`
- **BREAKING**：无（纯新增模块，不修改 v0.27.0/v0.28.0 现有源文件）

## Impact

- **Affected specs**: 解锁 v0.46.0 Modbus TCP、v0.48.0 IEC 104、Phase 2 DDS/gRPC
- **Affected code**:
  - 修改：`crates/drivers/net/Cargo.toml`（version → "0.29.0"）、`crates/drivers/net/src/lib.rs`（添加 socket 模块 + 版本号）
  - 新增：`crates/drivers/net/src/socket/` 整个子模块（5 文件）
  - **不修改**：v0.27.0 的 6 个源文件 + v0.28.0 的 7 个 tcpip/ 源文件（Surgical Changes）
  - 修改：根 `Cargo.toml`（version）、`Makefile`、`ci.yml`、`gate.rs`

## 设计决策（Karpathy 原则应用）

### 1. Think Before Coding — SocketManager 集中管理，非 Box<dyn Socket>

**蓝图设计**（phase1.md §4.1）：
```rust
pub struct SocketManager {
    sockets: HashMap<SocketId, Box<dyn Socket>>,
    next_id: usize,
    poll: Poll,
}
```

**问题分析**：
- 蓝图的 `Socket` trait 方法签名是 `fn read(&mut self, buf: &mut [u8]) -> Result<usize, SocketError>`
- smoltcp 的 Socket 数据存储在 `SocketSet`（在 `NetworkInterface` 内部），不在 Socket 对象本身
- 要实现 `Socket` trait，TcpStream 必须能独立访问 NetworkInterface —— 这需要 `Rc<RefCell<>>` 或全局静态引用，增加复杂度
- 蓝图的 `HashMap<SocketId, Box<dyn Socket>>` 设计假设 Socket 对象自包含，但 smoltcp 架构不是这样

**决策**：SocketManager 集中拥有 NetworkInterface，所有操作通过 SocketManager 方法（传入 SocketId）完成。Socket trait 定义保留（用于文档和未来扩展），但不为 smoltcp 后端实现。

**理由**：
- smoltcp 的 SocketSet 架构要求集中式 socket 所有权
- 实现 Socket trait 需要 Rc<RefCell<>>（no_std 不友好）或 unsafe 全局指针（复杂且不安全）
- SocketManager 方法 API 提供相同功能，代码更简单
- 遵循 "Simplicity First"：不添加 Rc/RefCell 复杂度直到真正需要

**偏差声明**：此设计偏离蓝图 §4.1 的 `Box<dyn Socket>` 存储，但满足蓝图的核心目标（统一 Socket API、非阻塞 IO、poll 多路复用）。偏差原因已记录，后续如需多态可引入 trait 对象。

### 2. Simplicity First — 句柄类型，非自包含对象

**决策**：TcpStream / TcpListener / UdpSocket 是 SocketId（usize）的 newtype，零成本句柄。

```rust
pub struct TcpStream(SocketId);
pub struct TcpListener(SocketId);
pub struct UdpSocket(SocketId);
```

**理由**：
- 实际数据在 SocketManager 内部
- 句柄可以 Copy（无需生命周期）
- 操作通过 `mgr.read(stream.id(), buf)` 完成
- 不需要 Rc/RefCell/Box

### 3. Surgical Changes — 不修改 v0.27.0/v0.28.0 源文件

**决策**：仅修改 lib.rs 和 Cargo.toml，不修改现有 13 个源文件。
- v0.27.0: error.rs / eth_frame.rs / dma_ring.rs / phy.rs / mac.rs / mock.rs（6 文件）
- v0.28.0: tcpip/mod.rs / addr.rs / error.rs / device.rs / interface.rs / socket.rs / dhcp.rs（7 文件）
- socket/ 是全新的独立子模块，通过 `pub use crate::tcpip::*` 引用 v0.28.0 类型

### 4. Goal-Driven Execution — 测试策略

**验证标准**：
1. `cargo test -p eneros-net` 通过（v0.27.0 的 130 + v0.28.0 的 127 + v0.29.0 新增 70+ = 327+ tests）
2. `cargo build -p eneros-net --target aarch64-unknown-none` 交叉编译通过
3. `cargo deny check` 通过
4. `cargo fmt` / `cargo clippy` 无 warning
5. workspace 回归测试 PASS
6. `cargo run -p eneros-ci` Overall: PASS

**测试范围**：
- SocketManager 创建、socket 创建/关闭
- TcpStream connect/listen/read/write（用 MockNetDevice，不需真实网络）
- UdpSocket bind/send_to/recv_from
- Poll 注册/注销/poll_once（非阻塞）
- 非阻塞 IO 行为（WouldBlock 错误）
- SocketError 转换
- **不测试 smoltcp 协议实现**（v0.28.0 已有测试）

## ADDED Requirements

### Requirement: SocketManager 集中管理

系统 SHALL 提供 `SocketManager<D: NetDevice>` 集中管理 NetworkInterface 和所有 Socket。

```rust
pub struct SocketManager<D: NetDevice> {
    iface: NetworkInterface<D>,
    sockets: BTreeMap<SocketId, SocketEntry>,
    next_id: SocketId,
    poll: Poll,
}

struct SocketEntry {
    handle: SocketHandle,      // smoltcp socket handle
    kind: SocketKind,          // TcpStream / TcpListener / Udp
    nonblocking: bool,
    local_addr: Option<SocketAddr>,
    remote_addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    TcpStream,
    TcpListener,
    Udp,
}

pub type SocketId = usize;
```

#### Scenario: 创建 TCP 连接
- **WHEN** 调用 `mgr.tcp_connect(remote, local_port)`
- **THEN** 创建 smoltcp TcpSocket + 加入 SocketSet + 注册到 sockets map + 返回 SocketId

#### Scenario: 关闭 Socket
- **WHEN** 调用 `mgr.close(id)`
- **THEN** 从 SocketSet 移除 + 从 sockets map 移除 + 从 poll registry 注销

### Requirement: Socket 句柄类型

系统 SHALL 提供 TcpStream / TcpListener / UdpSocket 作为 SocketId 的零成本 newtype 句柄。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpStream(SocketId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpListener(SocketId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpSocket(SocketId);
```

#### Scenario: 句柄操作
- **WHEN** 持有 TcpStream 句柄并调用 `mgr.read(stream.id(), buf)`
- **THEN** 通过 SocketId 查找 SocketEntry → 获取 SocketHandle → 访问 smoltcp TcpSocket → 读取数据

### Requirement: Socket 统一接口（trait）

系统 SHALL 定义 `Socket` trait 作为统一接口规范（用于文档和未来扩展）。

```rust
pub trait Socket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SocketError>;
    fn write(&mut self, buf: &[u8]) -> Result<usize, SocketError>;
    fn close(&mut self) -> Result<(), SocketError>;
    fn set_nonblocking(&mut self, nonblocking: bool);
    fn is_readable(&self) -> bool;
    fn is_writable(&self) -> bool;
    fn local_addr(&self) -> Result<SocketAddr, SocketError>;
    fn remote_addr(&self) -> Result<SocketAddr, SocketError>;
}
```

**注**：v0.29.0 不为 smoltcp 后端实现此 trait（需 Rc<RefCell<>>，违反 Simplicity First）。SocketManager 方法 API 提供等价功能。trait 定义保留用于：
- Mock 测试（测试用的 MockSocket 可实现 Socket trait）
- 未来扩展（如非 smoltcp 后端）

### Requirement: SocketManager API

系统 SHALL 提供以下 SocketManager 方法：

```rust
impl<D: NetDevice> SocketManager<D> {
    // 生命周期
    pub fn new(device: D, config: InterfaceConfig) -> Self;
    pub fn tcp_connect(&mut self, remote: SocketAddr, local_port: u16) -> Result<TcpStream, SocketError>;
    pub fn tcp_listen(&mut self, port: u16) -> Result<TcpListener, SocketError>;
    pub fn tcp_accept(&mut self, listener: TcpListener) -> Result<(TcpStream, SocketAddr), SocketError>;
    pub fn udp_bind(&mut self, local: SocketAddr) -> Result<UdpSocket, SocketError>;
    pub fn close(&mut self, id: SocketId) -> Result<(), SocketError>;

    // IO 操作
    pub fn read(&mut self, id: SocketId, buf: &mut [u8]) -> Result<usize, SocketError>;
    pub fn write(&mut self, id: SocketId, buf: &[u8]) -> Result<usize, SocketError>;
    pub fn send_to(&mut self, id: SocketId, buf: &[u8], dst: SocketAddr) -> Result<usize, SocketError>;
    pub fn recv_from(&mut self, id: SocketId, buf: &mut [u8]) -> Result<(usize, SocketAddr), SocketError>;

    // 状态查询
    pub fn is_readable(&self, id: SocketId) -> bool;
    pub fn is_writable(&self, id: SocketId) -> bool;
    pub fn local_addr(&self, id: SocketId) -> Result<SocketAddr, SocketError>;
    pub fn remote_addr(&self, id: SocketId) -> Result<SocketAddr, SocketError>;
    pub fn set_nonblocking(&mut self, id: SocketId, on: bool);
    pub fn socket_kind(&self, id: SocketId) -> Option<SocketKind>;

    // 网络接口
    pub fn poll_interface(&mut self, timestamp_ms: u64) -> Result<(), SocketError>;
    pub fn poll_at(&self, timestamp_ms: u64) -> Option<u64>;
    pub fn ipv4_addr(&self) -> Option<Ipv4Addr>;

    // Poll 多路复用
    pub fn register(&mut self, id: SocketId, interest: Interest) -> Result<(), SocketError>;
    pub fn deregister(&mut self, id: SocketId);
    pub fn modify_interest(&mut self, id: SocketId, interest: Interest);
    pub fn poll_once(&mut self) -> Vec<Event>;
}
```

#### Scenario: 非阻塞 read 返回 WouldBlock
- **WHEN** socket 设置为 nonblocking 且无数据可读
- **THEN** `mgr.read(id, buf)` 返回 `Err(SocketError::WouldBlock)`

#### Scenario: 阻塞 read 有数据
- **WHEN** socket 为阻塞模式且有数据可读
- **THEN** `mgr.read(id, buf)` 返回 `Ok(n)` 读取的字节数

### Requirement: Poll 多路复用

系统 SHALL 提供基于注册/轮询的 Poll 多路复用机制。

```rust
pub struct Poll {
    registry: BTreeMap<SocketId, Interest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interest {
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub socket_id: SocketId,
    pub readiness: Readiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readiness(u8);

impl Readiness {
    pub const READABLE: Readiness = Readiness(0x01);
    pub const WRITABLE: Readiness = Readiness(0x02);
    pub const ERROR: Readiness = Readiness(0x04);
    pub const EMPTY: Readiness = Readiness(0x00);

    pub fn empty() -> Self { Self::EMPTY }
    pub fn is_empty(self) -> bool { self.0 == 0 }
    pub fn contains(self, other: Readiness) -> bool { self.0 & other.0 == other.0 }
    pub fn insert(&mut self, other: Readiness) { self.0 |= other.0; }
    pub fn remove(&mut self, other: Readiness) { self.0 &= !other.0; }
}
```

#### Scenario: 注册 interest 并 poll
- **WHEN** 注册 SocketId=1 with Interest{readable: true}，且 socket 1 可读
- **THEN** `poll_once()` 返回 `vec![Event{socket_id: 1, readiness: Readiness::READABLE}]`

#### Scenario: 注销后不触发事件
- **WHEN** 注销 SocketId=1 后调用 `poll_once()`
- **THEN** 返回的 Event 列表不包含 socket_id: 1

### Requirement: SocketError 错误类型

系统 SHALL 提供 SocketError 枚举（11 变体）。

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketError {
    NotConnected,
    ConnectionRefused,
    ConnectionReset,
    WouldBlock,
    TimedOut,
    BrokenPipe,
    AddrInUse,
    AddrNotAvailable,
    InvalidArgument,
    Closed,
    IoError(alloc::string::String),
}
```

#### Scenario: 从 TcpIpError 转换
- **WHEN** v0.28.0 的 TcpIpError 传入 SocketManager 操作
- **THEN** 自动转换为对应的 SocketError（如 WouldBlock/TimedOut/ConnectionRefused 等）

### Requirement: 非阻塞 IO

系统 SHALL 支持非阻塞 IO 模式。

#### Scenario: 默认阻塞模式
- **WHEN** 创建新 socket（未设置 nonblocking）
- **THEN** read/write 操作在无数据/不可写时返回 `Err(SocketError::WouldBlock)`（smoltcp 本身是非阻塞的，"阻塞"由应用层循环实现）

#### Scenario: 设置非阻塞
- **WHEN** 调用 `mgr.set_nonblocking(id, true)`
- **THEN** 后续 read/write 在无数据时立即返回 `Err(SocketError::WouldBlock)`

**注**：smoltcp 本身是非阻塞的（所有操作立即返回）。v0.29.0 的 nonblocking 标志主要用于语义标注和未来阻塞模拟（应用层 poll 循环）。当前实现中，无论 nonblocking 标志如何，read/write 都可能返回 WouldBlock。

## 性能目标（蓝图 §6.3）

| 指标 | 目标 | 验证方式 |
|------|------|---------|
| poll 延迟 | < 100μs | QEMU/实机验证 |
| 并发连接 | ≥ 64 | Mock 测试 + 实机验证 |

**注**：性能目标需真实硬件/QEMU 验证，v0.29.0 仅交付软件实现 + mock 测试，性能验证延后。

## 依赖

无新增外部依赖。复用 v0.28.0 的 smoltcp v0.13.1 和 v0.27.0 的 NetDevice trait。

## 文件布局

```
crates/drivers/net/
├── Cargo.toml              # version → "0.29.0"
└── src/
    ├── lib.rs              # 添加 pub mod socket + re-exports + VERSION="0.29.0"
    ├── error.rs            # 不修改（v0.27.0）
    ├── eth_frame.rs        # 不修改（v0.27.0）
    ├── dma_ring.rs         # 不修改（v0.27.0）
    ├── phy.rs              # 不修改（v0.27.0）
    ├── mac.rs              # 不修改（v0.27.0）
    ├── mock.rs             # 不修改（v0.27.0）
    ├── tcpip/              # 不修改（v0.28.0）
    │   ├── mod.rs
    │   ├── addr.rs
    │   ├── error.rs
    │   ├── device.rs
    │   ├── interface.rs
    │   ├── socket.rs
    │   └── dhcp.rs
    └── socket/             # ★ v0.29.0 新增子模块
        ├── mod.rs          # 模块声明 + re-exports + crate 文档
        ├── api.rs          # Socket trait + SocketId + SocketKind + SocketError + TcpStream/TcpListener/UdpSocket
        ├── manager.rs      # SocketManager<D: NetDevice>
        ├── poll.rs         # Poll + Interest + Readiness
        └── event.rs        # Event 类型
```

## no_std 合规

- 所有 socket/* 文件遵循 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::collections::BTreeMap`（不用 HashMap，避免 hashbrown 依赖）
- 使用 `alloc::vec::Vec` / `alloc::string::String`
- 无 `use std::*`
- Readiness 使用手动位运算（不引入 bitflags 依赖）

## 内存预算声明（§5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| SocketManager | ~4 KB | BTreeMap + SocketEntry × 64 连接 |
| 单个 TCP Socket | ~128 KB | rx_buffer 64KB + tx_buffer 64KB（可配置） |
| 单个 UDP Socket | ~8 KB | rx_buffer 4KB + tx_buffer 4KB |
| 64 并发连接 | ~8 MB | 64 × 128KB（TCP 默认） |
| Poll registry | ~1 KB | BTreeMap<SocketId, Interest> × 64 |
| **总计** | **≤ 10 MB** | 可通过减小缓冲区降低 |

**OOM 策略**：关闭非关键 Socket、缩减缓冲区、降级到 L1（Solver-only 路径）。
