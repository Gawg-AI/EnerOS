# EnerOS Socket 抽象层设计文档 (v0.29.0)

> **范围**：在 v0.28.0 TCP/IP 协议栈之上构建统一 Socket API 抽象层，通过
> SocketManager 集中管理 NetworkInterface 和所有 Socket，提供标准化的
> read/write/close 接口、非阻塞 IO、poll 多路复用，使上层应用无需关心底层
> 协议细节。
>
> **Crate**：`eneros-net` (`crates/drivers/net/src/socket/`)
> **版本**：v0.29.0（Phase 1 Layer 6 基础服务）
> **状态**：已实现 — 主机测试通过（370 个单元测试，含 v0.27.0 的 130 个 +
> v0.28.0 的 127 个 + v0.29.0 新增 113 个）。

---

## 1. 概述

`eneros-net::socket` 模块在 v0.28.0 TCP/IP 协议栈之上构建统一 Socket API。
v0.28.0 提供了底层 `TcpSocket` / `UdpSocket` / `IcmpSocket` 包装，但每个操作
都需要传入 `&mut NetworkInterface`，对上层应用（Modbus TCP、IEC 104、Agent
通信）使用不便。v0.29.0 通过 `SocketManager` 集中拥有 `NetworkInterface` 和所有
Socket，应用层只需持有轻量级 `SocketId` 句柄即可完成全部操作。

### 设计决策（Karpathy 四原则）

- **Think Before Coding**：SocketManager 集中管理（非 `Box<dyn Socket>`）。
  smoltcp 的 SocketSet 架构要求集中式 socket 所有权；实现 Socket trait 需
  `Rc<RefCell<>>`（no_std 不友好）或 unsafe 全局指针，违背 Simplicity First。
- **Simplicity First**：`TcpStream` / `TcpListener` / `UdpSocket` 是
  `SocketId`（`usize`）的零成本 newtype 句柄；操作通过 `mgr.read(stream.id(), buf)`
  完成，无 `Rc`/`RefCell`/`Box`。`Readiness` 使用手动位运算（不引入 bitflags 依赖）。
  `Poll` 使用 `BTreeMap`（不引入 hashbrown 依赖）。
- **Surgical Changes**：不修改 v0.27.0（6 文件）和 v0.28.0（7 文件）源文件，
  仅在 `lib.rs` 添加 `pub mod socket;`。
- **Goal-Driven Execution**：测试覆盖 SocketManager 生命周期 + IO + Poll +
  错误转换，使用 MockNetDevice 不依赖真实网络。

### v0.29.0 交付物

| 组件 | 文件 | 说明 |
|------|------|------|
| API 类型 | `socket/api.rs` | SocketId / SocketKind / SocketError（11 变体）+ Socket trait + 句柄 newtype |
| 事件类型 | `socket/event.rs` | Event（socket_id + readiness） |
| Poll 多路复用 | `socket/poll.rs` | Readiness(u8) + Interest + Poll（注册/注销/check_readiness） |
| Socket 管理器 | `socket/manager.rs` | SocketManager\<D: NetDevice\>（18 个方法 + 35+ 测试） |
| 模块入口 | `socket/mod.rs` | 模块声明 + re-exports + 架构文档注释 |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────────────────┐
│  Caller (v0.46.0 Modbus TCP / v0.48.0 IEC 104 / Phase 2) │
└─────────────┬────────────────────────────────────────────┘
              │  SocketManager API (tcp_connect/read/write/...)
┌─────────────▼────────────────────────────────────────────┐
│  socket::SocketManager<D: NetDevice>  (this module)       │
│  ┌──────────────────────────────────────────────────┐    │
│  │  sockets: BTreeMap<SocketId, SocketEntry>         │    │
│  │  poll:    Poll (registry: BTreeMap<Id, Interest>) │    │
│  │  iface:   NetworkInterface<D> (owns smoltcp)      │    │
│  └──────────────────────────────────────────────────┘    │
└─────────────┬────────────────────────────────────────────┘
              │  SocketHandle -> smoltcp tcp/udp Socket
┌─────────────▼────────────────────────────────────────────┐
│  tcpip::NetworkInterface<D> + SocketSet  (v0.28.0)        │
└─────────────┬────────────────────────────────────────────┘
              │  NetDevice trait
┌─────────────▼────────────────────────────────────────────┐
│  mac::MacController (v0.27.0 Ethernet driver)            │
└──────────────────────────────────────────────────────────┘
```

### 数据流

```text
Application                SocketManager                    smoltcp
    │                           │                              │
    │── tcp_connect(remote) ───►│                              │
    │                           │── sockets.add_tcp() ────────►│
    │                           │── socket.connect(cx, ...) ──►│
    │◄── TcpStream(id) ─────────│                              │
    │                           │                              │
    │── read(stream.id(), buf) ►│                              │
    │                           │── sockets.get(id) ──────────►│ (SocketHandle)
    │                           │── socket.recv_slice(buf) ───►│
    │◄── Ok(n) / Err ───────────│                              │
```

---

## 3. SocketManager 集中管理

### 3.1 为什么不用 Box\<dyn Socket\>

蓝图（phase1.md §4.1）设计了 `HashMap<SocketId, Box<dyn Socket>>` 模式，但
smoltcp 的架构不适合这种设计：

- smoltcp 的 Socket 数据存储在 `SocketSet`（在 `NetworkInterface` 内部），
  不在 Socket 对象本身
- 要实现 `Socket` trait 的 `fn read(&mut self, buf: &mut [u8])` 签名，
  `TcpStream` 必须能独立访问 `NetworkInterface`
- 这需要 `Rc<RefCell<>>`（no_std 不友好）或 unsafe 全局指针（复杂且不安全）

**决策**：SocketManager 集中拥有 `NetworkInterface`，所有操作通过
SocketManager 方法（传入 `SocketId`）完成。`Socket` trait 定义保留（用于文档
和未来扩展），但不为 smoltcp 后端实现。

### 3.2 SocketEntry 结构

```rust
struct SocketEntry {
    handle: SocketHandle,       // smoltcp socket handle
    kind: SocketKind,           // TcpStream / TcpListener / Udp
    nonblocking: bool,          // 语义标志（smoltcp 始终非阻塞）
    local_addr: Option<SocketAddr>,  // 缓存的本地地址
    remote_addr: Option<SocketAddr>, // 缓存的远程地址
}
```

`local_addr` 和 `remote_addr` 在 `tcp_connect` / `tcp_listen` / `udp_bind` /
`tcp_accept` 时缓存，在 `local_addr()` / `remote_addr()` 方法中作为 smoltcp
查询的回退值（smoltcp 的 `local_endpoint()` 对监听 socket 可能返回 `None`）。

### 3.3 Split-borrowing 模式

`tcp_connect` 需要同时访问 `self.iface.iface.context()` 和
`self.iface.sockets.inner.get_mut()`。通过 split-borrowing 模式绕过借用检查：

```rust
let handle = {
    let iface = &mut self.iface;
    let handle = iface.sockets.add_tcp(...);
    let cx = iface.iface.context();           // 借用 iface.iface
    let socket = iface.sockets.inner.get_mut::<tcp::Socket>(handle); // 借用 iface.sockets
    socket.connect(cx, remote, local_port)
};
```

编译器识别 `iface.iface` 和 `iface.sockets` 是不同字段，允许同时借用。

---

## 4. 句柄类型

`TcpStream` / `TcpListener` / `UdpSocket` 是 `SocketId`（`usize`）的零成本
newtype：

```rust
pub struct TcpStream(SocketId);
pub struct TcpListener(SocketId);
pub struct UdpSocket(SocketId);
```

- 句柄可以 `Copy`（无需生命周期参数）
- 操作通过 `mgr.read(stream.id(), buf)` 完成
- 不需要 `Rc`/`RefCell`/`Box`

---

## 5. Poll 多路复用

### 5.1 非阻塞模型

v0.29.0 使用非阻塞 poll（`poll_once()`），不实现带超时的阻塞 poll：

- `poll_once()` 遍历所有注册的 socket，返回就绪事件列表（可能为空）
- 应用层主循环负责调用 `poll_once()` + `poll_interface(timestamp_ms)` + sleep/yield
- 这符合 RTOS 的事件驱动模型（蓝图 §8.2）

### 5.2 Readiness 位运算

`Readiness(u8)` 使用手动位运算，不引入 bitflags 依赖：

```rust
pub struct Readiness(u8);

impl Readiness {
    pub const READABLE: Readiness = Readiness(0x01);
    pub const WRITABLE: Readiness = Readiness(0x02);
    pub const ERROR:    Readiness = Readiness(0x04);
    pub const EMPTY:    Readiness = Readiness(0x00);
}
```

### 5.3 Interest 注册

```rust
pub struct Interest {
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
}
```

`Poll::check_readiness(id, is_readable, is_writable)` 根据 `Interest` 和当前
socket 状态计算 `Readiness`，只报告同时被注册且当前为真的就绪事件。

---

## 6. TCP accept 模式

smoltcp 不像 POSIX 提供 `accept()` 创建新 socket。smoltcp 的 listen 模式：

1. 创建 `TcpSocket`，调用 `socket.listen(port)`
2. 等待远端 SYN → smoltcp 自动完成握手
3. socket 状态变为 `Established`
4. 应用层检测到 `Established` 后，该 socket 即为"已接受的连接"

### v0.29.0 的 tcp_accept 实现

- `tcp_listen(port)` 返回 `TcpListener(id)`，底层是一个 listen 状态的 smoltcp TcpSocket
- `tcp_accept(listener)` 检查该 socket 的 `TcpState`：
  - 若 `Established` → 返回 `(TcpStream(listener_id), remote_addr)`（复用同一 socket）
  - 若未就绪 → 返回 `Err(SocketError::WouldBlock)`
- **限制**：每个 listener 同时只能有一个 pending connection

---

## 7. 错误处理

### SocketError（11 变体）

```rust
pub enum SocketError {
    NotConnected,        // socket 未连接
    ConnectionRefused,   // 连接被拒绝（RST）
    ConnectionReset,     // 连接被重置
    WouldBlock,          // 非阻塞操作无数据
    TimedOut,            // 操作超时
    BrokenPipe,          // 写入已关闭的 socket
    AddrInUse,           // 地址已占用
    AddrNotAvailable,    // 地址不可用
    InvalidArgument,     // 无效参数
    Closed,              // socket 已关闭或不存在
    IoError(String),     // 通用 IO 错误（带描述）
}
```

### 错误转换链

```text
smoltcp 错误 (ConnectError/SendError/RecvError/...)
    │
    ▼  TcpIpError::from(e)
TcpIpError (15 变体)
    │
    ▼  SocketError::from(TcpIpError::from(e))
SocketError (11 变体)
```

直接语义匹配的错误保持一致（如 `WouldBlock` → `WouldBlock`），
无直接匹配的错误折叠为 `IoError(String)`（如 `DmaError` → `IoError("DMA/hardware error")`）。

---

## 8. no_std 合规

- crate 根（`lib.rs`）声明 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，
  覆盖所有子模块
- 使用 `alloc::collections::BTreeMap`（不用 `HashMap`，避免 hashbrown 依赖）
- 使用 `alloc::vec::Vec` / `alloc::string::String`
- `Readiness` 使用手动位运算（不引入 bitflags 依赖）
- 无 `use std::*`

---

## 9. 测试策略

由于 smoltcp 的非阻塞特性，测试不需要真实网络：

1. **SocketManager 创建**：验证 `new()` + 配置 + `ipv4_addr()`
2. **Socket 生命周期**：`tcp_connect` / `tcp_listen` / `udp_bind` / `close`
3. **TCP IO**：`read` / `write` 在 `SynSent` 状态返回 `NotConnected`
4. **UDP IO**：`recv_from` 空缓冲区返回 `WouldBlock`
5. **Poll 注册/注销**：`register` / `deregister` / `modify_interest` / `poll_once`
6. **错误转换**：`TcpIpError` → `SocketError`、`NetError` → `SocketError`
7. **Readiness 位运算**：`insert` / `remove` / `contains` / `is_empty`
8. **Interest**：`Default` + helper constructors + builder
9. **句柄类型**：`TcpStream` / `TcpListener` / `UdpSocket` 的 `id()` 方法
10. **多 socket 场景**：混合 TCP/UDP + 独立关闭

测试使用 `MockNetDevice`（`send` 为 no-op，`recv` 返回 `NoBuffer`），
与 v0.28.0 测试模式一致。

---

## 10. 内存预算声明（蓝图 §5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| SocketManager | ~4 KB | BTreeMap + SocketEntry × 64 连接 |
| 单个 TCP Socket | ~128 KB | rx_buffer 64KB + tx_buffer 64KB（可配置） |
| 单个 UDP Socket | ~8 KB | rx_buffer 4KB + tx_buffer 4KB |
| 64 并发连接 | ~8 MB | 64 × 128KB（TCP 默认） |
| Poll registry | ~1 KB | BTreeMap<SocketId, Interest> × 64 |
| **总计** | **≤ 10 MB** | 可通过减小缓冲区降低 |

### OOM 策略

当总用量 > 90%（蓝图 §43.6 OOM 阈值）时：
1. 关闭非关键 Socket（保留控制大区 L1 路径所需连接）
2. 缩减缓冲区大小（TCP 64KB → 16KB，UDP 4KB → 1KB）
3. 降级到 L1（Solver-only 路径，不依赖网络通信）

---

## 11. 偏差声明

### Socket trait 不为 smoltcp 后端实现

`Socket` trait 定义保留（用于文档和未来扩展），但不为 smoltcp 后端实现。原因：
smoltcp 的 socket 数据存储在 `SocketSet`（在 `NetworkInterface` 内部），要实现
`Socket` trait 的 `fn read(&mut self, buf: &mut [u8])` 签名，`TcpStream` 必须能
独立访问 `NetworkInterface` — 这需要 `Rc<RefCell<>>`（no_std 不友好）或 unsafe
全局指针（复杂且不安全）。SocketManager 方法 API 提供等价功能，代码更简单。

### poll_at 签名为 &mut self

spec 中 `poll_at` 原设计为 `&self` 签名，但 smoltcp 0.13 的
`Interface::poll_at` 需要 `&mut self`，因此 `SocketManager::poll_at` 签名改为
`&mut self`，委托给 `NetworkInterface::poll_at(timestamp_ms)`。

---

## 12. 后续版本解锁

| 版本 | 功能 | 依赖 |
|------|------|------|
| v0.46.0 | Modbus TCP | v0.29.0 Socket API（TcpStream + read/write） |
| v0.48.0 | IEC 104 | v0.29.0 Socket API（TcpStream + poll 多路复用） |
| Phase 2 | DDS / gRPC | v0.29.0 Socket API（UdpSocket + TCP） |

---

## 13. 参考

- [smoltcp v0.13 文档](https://docs.rs/smoltcp/0.13/)
- 蓝图 `phase1.md` §4.1 Socket 抽象层设计
- `docs/drivers/tcpip-stack-design.md` — v0.28.0 TCP/IP 协议栈集成
- `docs/drivers/net-driver-design.md` — v0.27.0 以太网驱动
- Karpathy 四原则：Think Before Coding / Simplicity First / Surgical Changes / Goal-Driven Execution
