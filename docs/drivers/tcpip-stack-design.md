# EnerOS TCP/IP 协议栈集成设计文档 (v0.28.0)

> **范围**：集成 smoltcp v0.13 作为 EnerOS 的 TCP/IP 协议栈，提供 TCP/UDP/ICMP
> Socket 抽象与 DHCP 客户端，为上层应用（Agent Runtime、IEC 104/Modbus TCP
> 协议栈）提供标准网络通信能力。
>
> **Crate**：`eneros-net` (`crates/drivers/net/src/tcpip/`)
> **版本**：v0.28.0（Phase 1 Layer 6 基础服务）
> **状态**：已实现 — 主机测试通过（257 个单元测试，含 v0.27.0 的 130 个）。

---

## 1. 概述

`eneros-net::tcpip` 模块集成 [smoltcp](https://github.com/smoltcp-rs/smoltcp) v0.13
作为 EnerOS 的默认 TCP/IP 协议栈。smoltcp 是一个 `no_std` 的 TCP/IP 栈，适合
嵌入式和实时系统。本模块提供 smoltcp 与 v0.27.0 以太网驱动之间的适配层，以及
TCP/UDP/ICMP Socket 和 DHCP 客户端的高层封装。

### 设计决策

- **不自研协议栈**：遵循蓝图 §5.5（默认集成清单）"固定 smoltcp，禁止自研"
- **适配而非重写**：包装 smoltcp 类型，不重新实现协议逻辑
- **分离错误类型**：创建 `TcpIpError` 而非修改 v0.27.0 的 `NetError`（Surgical Changes）

### v0.28.0 交付物

| 组件 | 文件 | 说明 |
|------|------|------|
| 地址类型 | `tcpip/addr.rs` | Ipv4Addr/Ipv4Cidr/SocketAddr/SocketHandle 类型别名 + helper 函数 |
| 错误类型 | `tcpip/error.rs` | TcpIpError（15 变体）+ From 转换 + is_retriable() |
| 设备适配器 | `tcpip/device.rs` | SmolcpDevice<D: NetDevice> — 桥接 NetDevice ↔ smoltcp::phy::Device |
| 网络接口 | `tcpip/interface.rs` | NetworkInterface — 包装 smoltcp::iface::Interface + poll 管理 |
| Socket 封装 | `tcpip/socket.rs` | SocketSet + TcpSocket/UdpSocket/IcmpSocket + TcpState |
| DHCP 客户端 | `tcpip/dhcp.rs` | DhcpClient + DhcpState + DhcpLease（事件驱动模型） |
| 模块入口 | `tcpip/mod.rs` | 模块声明 + re-exports + 架构文档注释 |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────┐
│  Caller (Agent Runtime / IEC 104 / Modbus)   │
└─────────────┬────────────────────────────────┘
              │  TcpSocket / UdpSocket / IcmpSocket / DhcpClient
┌─────────────▼────────────────────────────────┐
│  eneros-net::tcpip (this module)             │
│  ┌────────────────────────────────────────┐  │
│  │  NetworkInterface (wraps smoltcp::iface)│  │
│  │  SocketSet (wraps smoltcp::iface)       │  │
│  │  SmolcpDevice (adapts NetDevice → phy)  │  │
│  └────────────────────────────────────────┘  │
└─────────────┬────────────────────────────────┘
              │  smoltcp::phy::Device trait (RxToken / TxToken)
┌─────────────▼────────────────────────────────┐
│  smoltcp v0.13 (protocol stack, 0BSD)        │
└─────────────┬────────────────────────────────┘
              │  NetDevice trait (send / recv)
┌─────────────▼────────────────────────────────┐
│  eneros-net::MacController (v0.27.0 driver)  │
└──────────────────────────────────────────────┘
```

---

## 3. SmolcpDevice 适配器

smoltcp 的 `phy::Device` trait 使用零拷贝 Token 模式（RxToken/TxToken），而
EnerOS 的 `NetDevice` trait 使用拷贝式接口（send/recv 字节切片）。适配器通过
内部 RX 队列（`VecDeque<Vec<u8>>`）桥接两种模型：

### 数据流

```text
NetDevice::recv() ──► drain_rx() ──► rx_queue ──► receive() ──► RxToken::consume()
                                                                     │
TxToken::consume() ──► NetDevice::send() ◄─────────────────────────┘
```

### 关键设计

- **RX 队列**：`drain_rx()` 从 `NetDevice::recv()` 读取所有可用帧放入 `VecDeque`
- **RxToken**：持有 `Vec<u8>` 帧数据，`consume()` 回调处理帧
- **TxToken**：持有 `&mut D` 引用，`consume()` 调用 `NetDevice::send()` 发送帧
- **capabilities**：返回 MTU、Medium=Ethernet、无 checksum offload

---

## 4. NetworkInterface

`NetworkInterface<D: NetDevice>` 是协议栈的核心管理结构：

### 职责

1. 持有 smoltcp `Interface`（管理 ARP、路由、IP 层）
2. 持有 `SmolcpDevice<D>`（设备适配器）
3. 持有 `SocketSet`（所有 TCP/UDP/ICMP/DHCP Socket）
4. 提供 `poll(timestamp_ms)` 推进协议栈
5. 管理 IP 地址和网关配置

### InterfaceConfig 构建器

```rust
let config = InterfaceConfig::new(mac_addr)
    .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
    .with_gateway(ipv4_addr(192, 168, 1, 1))
    .with_dhcp(false);
```

### Poll 机制

smoltcp 是事件驱动的，需要外部定期调用 `poll()`：
1. `poll(timestamp_ms)` — 先调用 `device.drain_rx()` 读取帧，再调用 `iface.poll()`
2. `poll_at(timestamp_ms)` — 返回下次需要 poll 的时间戳
3. `poll_delay(timestamp_ms)` — 返回距离下次 poll 的延迟（毫秒）

### smoltcp 0.13 API 适配

smoltcp 0.13 的 `Instant::from_millis<T: Into<i64>>` 要求 `T` 可转换为 `i64`，
但 `u64` 不实现 `Into<i64>`（溢出安全）。因此 `poll`/`poll_at`/`poll_delay` 中
将 `timestamp_ms as i64` 后传入。

`poll_at` 和 `poll_delay` 在 smoltcp 0.13 中需要 `&mut self`（非 `&self`），
因为内部可能更新邻居缓存等状态。

---

## 5. Socket API

### SocketSet

`SocketSet` 是 `smoltcp::iface::SocketSet<'static>` 的包装，提供便捷的 Socket
创建方法：

```rust
let tcp_handle = iface.sockets.add_tcp(65535, 65535);  // rx_size, tx_size
let udp_handle = iface.sockets.add_udp(1024, 1024);
let icmp_handle = iface.sockets.add_icmp(1024, 1024);
```

### TCP Socket

- `listen(iface, port)` — 监听端口
- `connect(iface, remote, local_port)` — 主动连接（需指定本地端口）
- `send(iface, data)` / `recv(iface, buf)` — 收发数据
- `close(iface)` / `abort(iface)` — 优雅关闭 / 强制中止
- `state(iface)` — 查询 TCP 状态（11 种状态映射 RFC 793）

**设计偏差**：smoltcp 0.13 的 `tcp::Socket::connect()` 拒绝 port=0（不同于旧版
的 `None` 表示自动选择），因此 `connect` 方法增加了 `local_port` 参数。

### UDP Socket

- `bind(iface, port)` — 绑定端口
- `send_to(iface, data, dst)` / `recv_from(iface, buf)` — 收发数据报
- `endpoint(iface)` — 查询本地绑定端点

### ICMP Socket

- `send_ping(iface, dst, seq)` — 发送 Echo Request
- `recv_pong(iface)` — 接收 Echo Reply，返回 (源地址, 序列号)

### smoltcp 0.13 缓冲区类型

| Socket 类型 | 缓冲区类型 | 说明 |
|-------------|-----------|------|
| TCP | `RingBuffer<u8>` | 流式数据，使用 `RingBuffer::new(vec![0u8; size])` |
| UDP | `PacketBuffer<UdpMetadata>` | 数据报，每个缓冲区可持有多包 |
| ICMP | `PacketBuffer<IpAddress>` | 数据报，2 个参数（rx + tx） |

---

## 6. DHCP 客户端

### smoltcp 0.13 事件驱动模型

smoltcp 0.13 的 `dhcpv4::Socket` 使用事件驱动模型：
- `socket.poll()` 返回 `Option<Event>`（`Deconfigured` 或 `Configured`）
- **没有**公共 `state()` 方法查询内部状态机
- 调用者必须通过处理事件来跟踪状态

### DhcpClient 封装

`DhcpClient` 内部跟踪状态和租约，每次 `poll()` 时处理所有待处理事件：

```rust
let mut dhcp = DhcpClient::new();
dhcp.start(&mut iface).unwrap();

// 主循环中：
iface.poll(timestamp).unwrap();
let state = dhcp.poll(&mut iface).unwrap();
if state.is_bound() {
    if let Some(lease) = dhcp.lease(&iface) {
        // 使用 lease.addr, lease.gateway 等
    }
}
```

### DhcpLease

`DhcpLease` 从 `dhcpv4::Config` 提取数据创建。由于 `Config<'a>` 含生命周期
参数（无法直接存储），`DhcpLease` 将所需字段拷贝到 owned 类型。

**注意**：smoltcp 0.13 的 `Config` 不暴露 `lease_duration`，该字段设为 0。

---

## 7. 错误处理

### TcpIpError（15 变体）

```
DmaError / NoRoute / ArpResolutionFailed / ConnectionRefused /
ConnectionReset / NotConnected / WouldBlock / TimedOut /
AddrInUse / AddrNotAvailable / InvalidArgument / DhcpFailed /
SocketNotFound / Unreachable / PacketTooLarge
```

### 错误转换

- `From<NetError> for TcpIpError` — v0.27.0 设备错误 → 协议栈错误
- `From<TcpIpError> for NetError` — 协议栈错误 → 设备错误（统一错误处理）
- `From<tcp::ListenError> / ConnectError / SendError / RecvError`
- `From<udp::BindError / SendError / RecvError>`
- `From<icmp::SendError / RecvError>`

### is_retriable()

`WouldBlock` / `TimedOut` / `ArpResolutionFailed` 标记为可重试，其余不可重试。

---

## 8. 测试策略

### 测试覆盖（127 个新测试）

| 模块 | 测试数 | 覆盖内容 |
|------|--------|---------|
| addr.rs | 8 | 类型别名、helper 函数、CIDR |
| error.rs | 20+ | 所有变体、From 转换、is_retriable |
| device.rs | 15+ | SmolcpDevice 适配器、RxToken/TxToken、capabilities |
| interface.rs | 15+ | InterfaceConfig 构建器、NetworkInterface 创建/poll |
| socket.rs | 28+ | TcpState 映射、SocketSet、TCP/UDP/ICMP Socket API |
| dhcp.rs | 22+ | DhcpState/DhcpLease/DhcpClient、事件驱动状态跟踪 |

### 测试方法

- 全部使用 Mock 设备（实现 `NetDevice` trait），不需要真实网络
- 不测试 smoltcp 协议实现本身（smoltcp 有自己的测试套件）
- 测试适配器层是否正确转发帧和 API 调用

---

## 9. no_std 合规

- smoltcp 本身是 `#![no_std]`，启用 `alloc` feature 使用堆分配
- 包装层遵循 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 测试时启用 std（`#[cfg(test)]`）
- 无 `use std::*`，使用 `alloc::collections::VecDeque`、`alloc::vec::Vec`

---

## 10. 依赖

| 依赖 | 版本 | 许可证 | 用途 |
|------|------|--------|------|
| smoltcp | 0.13.1 | 0BSD | TCP/IP 协议栈 |

### smoltcp features

```toml
[dependencies.smoltcp]
version = "0.13"
default-features = false
features = [
    "alloc",           # 使用堆分配
    "medium-ethernet", # 以太网介质
    "proto-ipv4",      # IPv4 协议
    "socket-tcp",      # TCP Socket
    "socket-udp",      # UDP Socket
    "socket-icmp",     # ICMP Socket
    "socket-dhcpv4",   # DHCPv4 Socket
]
```

---

## 11. 后续版本

- **v0.29.0**：TLS/SSL 支持（可能集成 rustls 或 mbedTLS）
- **v0.30.0**：DNS 解析器
- **v0.30.1**：HTTP 客户端
- 后续 IEC 104 / Modbus TCP 协议栈将基于本模块的 TCP Socket 构建

---

## 12. 参考

- [smoltcp 文档](https://docs.rs/smoltcp/0.13/)
- [smoltcp 源码](https://github.com/smoltcp-rs/smoltcp)
- 蓝图 §5.5（默认集成清单）
- 蓝图 §43.1（no_std 合规要求）
- `docs/drivers/net-driver-design.md`（v0.27.0 以太网驱动设计）
