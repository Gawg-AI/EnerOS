# v0.28.0 TCP/IP 协议栈集成 Spec

## Why

Edge Box 需要通过 TCP/IP 与云端通信、接入 IEC 104 / Modbus TCP 设备。v0.28.0 集成 smoltcp（no_std TCP/IP 栈），实现 ARP/IPv4/TCP/UDP/ICMP/DHCP 全协议支持，为 v0.29.0 Socket 抽象层及所有上层网络协议提供基础。

**关键决策 — 集成 smoltcp 而非自研**（Karpathy: Think Before Coding）：
- 蓝图 [Blueprint.md §3644](file:///e:/eneros/蓝图/Power_Native_Agent_OS_Blueprint.md): "**固定 smoltcp**，禁止自研"
- 蓝图 [appendix.md §774](file:///e:/eneros/蓝图/appendix.md): "**优先集成 smoltcp，避免自研**"
- 蓝图 [§5.5 默认集成清单](file:///e:/eneros/.trae/rules/记忆.md): "TCP/IP 栈 | **smoltcp**（no_std） | 无 | 唯一成熟 no_std 网络栈"
- 蓝图 phase1.md §5.1 选型表: "smoltcp（Rust no_std） | ✅ Phase 1-2"
- 自研 TCP/IP 栈工作量极大（TCP 状态机 + 拥塞控制 + 重传），违反 §5.5 "禁止重复造轮子"

蓝图中的自研 TCP 代码仅为**参考设计**，展示协议逻辑，实际实现使用 smoltcp 的成熟实现。

## What Changes

- **新增 smoltcp 依赖**：在 `crates/drivers/net/Cargo.toml` 添加 smoltcp v0.13（0BSD 许可证）
- **新增 tcpip/ 子模块**（7 个源文件，添加到 eneros-net crate）：
  - `tcpip/mod.rs` — 模块声明 + re-exports
  - `tcpip/device.rs` — `SmolcpDevice<D: NetDevice>` 适配器（实现 `smoltcp::phy::Device` trait）
  - `tcpip/interface.rs` — `NetworkInterface<D>` 包装 `smoltcp::iface::Interface`
  - `tcpip/socket.rs` — `TcpSocket` / `UdpSocket` / `IcmpSocket` 包装 smoltcp sockets
  - `tcpip/dhcp.rs` — `DhcpClient` 包装 `smoltcp::socket::dhcpv4`
  - `tcpip/addr.rs` — 类型别名（Ipv4Addr / Ipv4Cidr / SocketAddr，复用 smoltcp::wire 类型）
  - `tcpip/error.rs` — smoltcp::Error → NetError 转换
- **修改 lib.rs**：添加 `pub mod tcpip;` + re-exports + VERSION 升级至 "0.28.0"
- **新增文档**：`docs/drivers/tcpip-stack-design.md`
- **新增配置**：`configs/tcpip.toml`
- **版本标识更新**：根 `Cargo.toml`、`Makefile`、`ci.yml`、`gate.rs`
- **BREAKING**：无（纯新增模块，不修改 v0.27.0 现有源文件）

## Impact

- **Affected specs**: 解锁 v0.29.0 Socket 抽象层、v0.46.0 Modbus TCP、v0.48.0 IEC 104
- **Affected code**:
  - 修改：`crates/drivers/net/Cargo.toml`（添加 smoltcp 依赖）、`crates/drivers/net/src/lib.rs`（添加 tcpip 模块 + 版本号）
  - 新增：`crates/drivers/net/src/tcpip/` 整个子模块（7 文件）
  - **不修改**：v0.27.0 的 error.rs / eth_frame.rs / dma_ring.rs / phy.rs / mac.rs / mock.rs
  - 修改：根 `Cargo.toml`（version）、`Makefile`、`ci.yml`、`gate.rs`

## ADDED Requirements

### Requirement: smoltcp Device 适配器

系统 SHALL 提供 `SmolcpDevice<D: NetDevice>` 适配器，将 v0.27.0 的 `NetDevice` trait 桥接到 smoltcp 的 `phy::Device` trait。

```rust
pub struct SmolcpDevice<D: NetDevice> {
    device: D,
    mtu: usize,
    tx_buffer: Vec<u8>,  // 临时发送缓冲
    rx_queue: VecDeque<Vec<u8>>,  // 接收帧队列
}

impl<D: NetDevice> smoltcp::phy::Device for SmolcpDevice<D> {
    type RxToken<'a> = RxToken<'a>;
    type TxToken<'a> = TxToken<'a>;
    fn receive(&mut self) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)>;
    fn transmit(&mut self) -> Option<Self::TxToken<'_>>;
    fn capabilities(&self) -> smoltcp::phy::DeviceCapabilities;
}
```

#### Scenario: 接收帧
- **WHEN** smoltcp 调用 `receive()` 且 rx_queue 有帧
- **THEN** 返回 RxToken（从队列取出帧）+ TxToken（用于回复）

#### Scenario: 发送帧
- **WHEN** smoltcp 通过 TxToken 写入帧数据并 consume
- **THEN** 调用 `NetDevice::send()` 发送帧

#### Scenario: 设备能力
- **WHEN** smoltcp 查询 `capabilities()`
- **THEN** 返回正确的 MTU、介质类型（Ethernet）、无 checksum offload

### Requirement: NetworkInterface 网络接口

系统 SHALL 提供 `NetworkInterface<D: NetDevice>` 包装 smoltcp::iface::Interface，提供主循环轮询。

```rust
pub struct NetworkInterface<D: NetDevice> {
    iface: smoltcp::iface::Interface,
    device: SmolcpDevice<D>,
    sockets: SocketSet,
}

impl<D: NetDevice> NetworkInterface<D> {
    pub fn new(device: D, config: InterfaceConfig) -> Self;
    pub fn poll(&mut self, timestamp_ms: u64) -> Result<(), NetError>;
    pub fn poll_at(&self) -> Option<u64>;
    pub fn add_ipv4_addr(&mut self, addr: Ipv4Cidr);
    pub fn ipv4_addr(&self) -> Option<Ipv4Addr>;
    pub fn gateway(&self) -> Option<Ipv4Addr>;
    pub fn set_dhcp(&mut self, on: bool);
}
```

#### Scenario: 轮询
- **WHEN** 调用 `poll(timestamp_ms)`
- **THEN** 从 NetDevice 接收帧 → 交给 smoltcp 处理 → 处理超时/DHCP → 返回 Ok

#### Scenario: 添加 IP 地址
- **WHEN** 调用 `add_ipv4_addr(cidr)`
- **THEN** smoltcp 接口添加该 IP 地址

### Requirement: TCP Socket

系统 SHALL 提供 `TcpSocket` 包装 smoltcp::socket::tcp::Socket，提供 EnerOS 风格的 TCP API。

```rust
pub struct TcpSocket {
    handle: SocketHandle,
    // 内部通过 SocketSet 访问 smoltcp socket
}

impl TcpSocket {
    pub fn new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>) -> Self;
    pub fn listen(&mut self, iface: &mut NetworkInterface<impl NetDevice>, port: u16) -> Result<(), NetError>;
    pub fn connect(&mut self, iface: &mut NetworkInterface<impl NetDevice>, remote: SocketAddr) -> Result<(), NetError>;
    pub fn send(&mut self, iface: &mut NetworkInterface<impl NetDevice>, data: &[u8]) -> Result<usize, NetError>;
    pub fn recv(&mut self, iface: &mut NetworkInterface<impl NetDevice>, buf: &mut [u8]) -> Result<usize, NetError>;
    pub fn close(&mut self, iface: &mut NetworkInterface<impl NetDevice>);
    pub fn state(&self, iface: &NetworkInterface<impl NetDevice>) -> TcpState;
}
```

#### Scenario: TCP 连接建立
- **WHEN** 调用 `connect(remote)` 且远端响应 SYN+ACK
- **THEN** 经过 poll() 后 socket 状态变为 Established

#### Scenario: TCP 数据发送
- **WHEN** 调用 `send(data)` 且连接已建立
- **THEN** 数据写入 smoltcp TCP 发送缓冲，返回写入字节数

#### Scenario: TCP 数据接收
- **WHEN** 调用 `recv(buf)` 且有数据到达
- **THEN** 从 smoltcp TCP 接收缓冲读取数据，返回读取字节数

### Requirement: UDP Socket

系统 SHALL 提供 `UdpSocket` 包装 smoltcp::socket::udp::Socket。

```rust
pub struct UdpSocket {
    handle: SocketHandle,
}

impl UdpSocket {
    pub fn new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>) -> Self;
    pub fn bind(&mut self, iface: &mut NetworkInterface<impl NetDevice>, port: u16) -> Result<(), NetError>;
    pub fn send_to(&mut self, iface: &mut NetworkInterface<impl NetDevice>, data: &[u8], dst: SocketAddr) -> Result<usize, NetError>;
    pub fn recv_from(&mut self, iface: &mut NetworkInterface<impl NetDevice>, buf: &mut [u8]) -> Result<(usize, SocketAddr), NetError>;
}
```

### Requirement: ICMP Socket（ping）

系统 SHALL 提供 `IcmpSocket` 包装 smoltcp::socket::icmp::Socket，支持 ping。

```rust
pub struct IcmpSocket {
    handle: SocketHandle,
}

impl IcmpSocket {
    pub fn new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>) -> Self;
    pub fn send_ping(&mut self, iface: &mut NetworkInterface<impl NetDevice>, dst: Ipv4Addr, seq: u16) -> Result<(), NetError>;
    pub fn recv_pong(&mut self, iface: &mut NetworkInterface<impl NetDevice>) -> Result<(Ipv4Addr, u16), NetError>;
}
```

### Requirement: DHCP 客户端

系统 SHALL 提供 `DhcpClient` 包装 smoltcp::socket::dhcpv4::Socket，自动获取 IP 地址。

```rust
pub struct DhcpClient {
    handle: SocketHandle,
}

impl DhcpClient {
    pub fn new() -> Self;
    pub fn start(&mut self, iface: &mut NetworkInterface<impl NetDevice>) -> Result<(), NetError>;
    pub fn poll(&mut self, iface: &mut NetworkInterface<impl NetDevice>) -> Result<DhcpState, NetError>;
    pub fn lease(&self, iface: &NetworkInterface<impl NetDevice>) -> Option<DhcpLease>;
}
```

#### Scenario: DHCP 获取 IP
- **WHEN** 调用 `start()` 后经过多次 `poll()`
- **THEN** 状态从 Init → Selecting → Requesting → Bound，获取 IP/网关/DNS

### Requirement: Socket 集合管理

系统 SHALL 提供 `SocketSet` 包装 smoltcp::socket::SocketSet，管理所有 socket 的生命周期。

```rust
pub struct SocketSet {
    set: smoltcp::socket::SocketSet<'static>,
}

impl SocketSet {
    pub fn new() -> Self;
    pub fn add(&mut self, socket: SocketEntry) -> SocketHandle;
    pub fn remove(&mut self, handle: SocketHandle);
    pub fn get(&self, handle: SocketHandle) -> Option<&SocketEntry>;
}
```

### Requirement: 地址类型

系统 SHALL 提供地址类型别名（复用 smoltcp::wire 类型，不重新定义）。

```rust
// 类型别名，复用 smoltcp 的成熟实现
pub type Ipv4Addr = smoltcp::wire::Ipv4Address;
pub type Ipv4Cidr = smoltcp::wire::Ipv4Cidr;
pub type HardwareAddress = smoltcp::wire::HardwareAddress;
pub type SocketAddr = smoltcp::wire::IpEndpoint;
pub type SocketHandle = smoltcp::socket::SocketHandle;
```

### Requirement: 错误转换

系统 SHALL 提供 smoltcp::Error → NetError 的自动转换。

```rust
impl From<smoltcp::Error> for NetError {
    fn from(e: smoltcp::Error) -> Self {
        match e {
            smoltcp::Error::Exhausted => NetError::NoBuffer,
            smoltcp::Error::Truncated => NetError::FrameTooSmall,
            smoltcp::Error::Checksum => NetError::CrcError,
            smoltcp::Error::Unrecognized => NetError::InvalidArgument,
            _ => NetError::DmaError(0), // 其他错误
        }
    }
}
```

## 设计决策（Karpathy 原则应用）

### 1. Think Before Coding — 集成 smoltcp 而非自研

**决策**：集成 smoltcp v0.13，不自研 TCP/IP 栈。
**理由**：
- 蓝图 ADR 明确要求："**固定 smoltcp**，禁止自研"（Blueprint.md §3644）
- §5.5 默认集成清单：smoltcp 是"唯一成熟 no_std 网络栈"
- 自研 TCP/IP 栈工作量极大（TCP 状态机 11 状态 + Reno 拥塞控制 + 重传 + ARP + DHCP），且易出错
- smoltcp 已实现蓝图要求的所有协议：ARP、IPv4、TCP（含状态机+拥塞控制）、UDP、ICMP、DHCP
- smoltcp 0BSD 许可证已在 deny.toml 允许列表
- 蓝图中的自研代码仅为参考设计，不是实现要求

### 2. Simplicity First — 薄包装层，不重新实现协议

**决策**：创建薄包装层，直接使用 smoltcp 的协议实现。
**理由**：
- smoltcp 已有成熟的 ARP 缓存、TCP 状态机、拥塞控制、重传机制
- 重新实现这些是"重复造轮子"（违反 §5.5）
- 包装层职责：适配 NetDevice → smoltcp::phy::Device、提供 EnerOS 风格 API、错误转换
- 不创建独立的 Ipv4Addr/Ipv4Cidr 类型，直接复用 smoltcp::wire 类型

### 3. Surgical Changes — 不修改 v0.27.0 现有源文件

**决策**：仅修改 lib.rs 和 Cargo.toml，不修改 v0.27.0 的 6 个源文件。
**理由**：
- v0.27.0 的 error.rs / eth_frame.rs / dma_ring.rs / phy.rs / mac.rs / mock.rs 已稳定
- v0.28.0 新增 tcpip/ 子模块，与 v0.27.0 代码完全解耦
- Cargo.toml 仅添加 smoltcp 依赖，不修改现有依赖
- lib.rs 仅添加 `pub mod tcpip;` 和 re-exports，不修改现有导出

### 4. Goal-Driven Execution — 测试策略

**验证标准**：
1. `cargo test -p eneros-net` 通过（含 v0.27.0 的 130 测试 + v0.28.0 新增测试）
2. `cargo build -p eneros-net --target aarch64-unknown-none` 交叉编译通过
3. `cargo deny check advisories licenses bans sources` 通过（smoltcp 0BSD 已允许）
4. `cargo fmt` / `cargo clippy` 无 warning
5. `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 回归测试通过
6. `cargo run -p eneros-ci` Overall: PASS

**测试范围**：
- `SmolcpDevice` 适配器测试（用 MockNetDevice 模拟帧收发）
- `NetworkInterface` poll 测试（验证帧处理流程）
- TCP/UDP/ICMP Socket 包装测试（验证 API 正确性）
- DHCP 客户端测试（状态机转换）
- 错误转换测试
- **不测试 smoltcp 自身的协议实现**（smoltcp 已有自己的测试套件）

### 5. smoltcp Feature 配置

```toml
[dependencies]
smoltcp = { version = "0.13", default-features = false, features = [
    "alloc",            # 使用堆分配
    "medium-ethernet",  # 以太网介质（v0.27.0 MAC 驱动）
    "proto-ipv4",       # IPv4 协议
    "socket-tcp",       # TCP sockets
    "socket-udp",       # UDP sockets
    "socket-icmp",      # ICMP sockets（ping）
    "socket-dhcpv4",    # DHCP 客户端
] }
```

不启用：`proto-ipv6`（v0.28.0 仅 IPv4）、`medium-ip`（用 Ethernet）、`async`（用轮询模型）、`socket-raw`（暂不需要）、`socket-dns`（v0.30.0 网络安全）

### 6. no_std 合规

- smoltcp 本身是 no_std（`#![no_std]`），无需特殊处理
- `alloc` feature 启用堆分配（v0.11.0 用户堆已就绪）
- 包装层代码遵循 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 无 `use std::*`

### 7. 轮询模型（非 async）

**决策**：使用 smoltcp 的轮询模型（`Interface::poll()`），不启用 async feature。
**理由**：
- EnerOS 是 RTOS，主循环定期调用 `poll()`
- async 模型需要 async runtime，增加复杂度
- 轮询模型更简单，更符合嵌入式场景

## 性能目标（蓝图 §6.3）

| 指标 | 目标 | 验证方式 |
|------|------|---------|
| TCP 吞吐 | ≥ 50 Mbps | QEMU/实机验证 |
| UDP 吞吐 | ≥ 100 Mbps | QEMU/实机验证 |
| ping 延迟 | < 1ms | QEMU/实机验证 |
| TCP 连接建立 | < 50ms | QEMU/实机验证 |
| DHCP 获取 IP | < 5s | QEMU/实机验证 |

**注**：性能目标需真实硬件/QEMU 验证，v0.28.0 仅交付软件实现 + mock 测试，性能验证延后。

## 依赖

```toml
[dependencies]
# v0.27.0 现有依赖（无变化）

# v0.28.0 新增
smoltcp = { version = "0.13", default-features = false, features = [
    "alloc", "medium-ethernet", "proto-ipv4",
    "socket-tcp", "socket-udp", "socket-icmp", "socket-dhcpv4",
] }
```

## 文件布局

```
crates/drivers/net/
├── Cargo.toml              # 添加 smoltcp 依赖
└── src/
    ├── lib.rs              # 添加 pub mod tcpip + re-exports + VERSION="0.28.0"
    ├── error.rs            # 不修改（v0.27.0）
    ├── eth_frame.rs        # 不修改（v0.27.0）
    ├── dma_ring.rs         # 不修改（v0.27.0）
    ├── phy.rs              # 不修改（v0.27.0）
    ├── mac.rs              # 不修改（v0.27.0）
    ├── mock.rs             # 不修改（v0.27.0）
    └── tcpip/              # ★ v0.28.0 新增子模块
        ├── mod.rs          # 模块声明 + re-exports
        ├── device.rs       # SmolcpDevice<D: NetDevice> 适配器
        ├── interface.rs    # NetworkInterface<D> 包装
        ├── socket.rs       # TcpSocket / UdpSocket / IcmpSocket
        ├── dhcp.rs         # DhcpClient 包装
        ├── addr.rs         # 类型别名（Ipv4Addr / Ipv4Cidr / SocketAddr）
        └── error.rs        # smoltcp::Error → NetError 转换 + 扩展错误类型
```
