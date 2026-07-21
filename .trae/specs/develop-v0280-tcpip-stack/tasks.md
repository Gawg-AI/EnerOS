# Tasks — v0.28.0 TCP/IP 协议栈集成

- [x] Task 1: 添加 smoltcp 依赖 + 模块骨架
  - [x] SubTask 1.1: 修改 `crates/drivers/net/Cargo.toml`：添加 smoltcp v0.13 依赖（features: alloc/medium-ethernet/proto-ipv4/socket-tcp/socket-udp/socket-icmp/socket-dhcpv4），version 改为 "0.28.0"
  - [x] SubTask 1.2: 创建 `crates/drivers/net/src/tcpip/mod.rs`：模块声明（pub mod device/interface/socket/dhcp/addr/error）+ re-exports
  - [x] SubTask 1.3: 修改 `crates/drivers/net/src/lib.rs`：添加 `pub mod tcpip;` + tcpip re-exports + VERSION 改为 "0.28.0"
  - [x] SubTask 1.4: 修改根 `Cargo.toml`：workspace.package.version 改为 "0.28.0"
  - [x] 验证: `cargo metadata --format-version 1 > /dev/null` 成功 + `cargo build -p eneros-net` 编译成功

- [x] Task 2: 实现 tcpip/addr.rs — 地址类型别名
  - [x] SubTask 2.1: 定义类型别名（复用 smoltcp::wire 类型）：
    - `Ipv4Addr = smoltcp::wire::Ipv4Address`
    - `Ipv4Cidr = smoltcp::wire::Ipv4Cidr`
    - `HardwareAddress = smoltcp::wire::HardwareAddress`
    - `SocketAddr = smoltcp::wire::IpEndpoint`
    - `SocketHandle = smoltcp::socket::SocketHandle`
  - [x] SubTask 2.2: 提供 helper 函数：`ipv4_addr(a,b,c,d) -> Ipv4Addr`、`ipv4_cidr(addr, prefix) -> Ipv4Cidr`
  - [x] 验证: 类型别名编译通过 + helper 函数测试 (5+ tests)

- [x] Task 3: 实现 tcpip/error.rs — 错误转换
  - [x] SubTask 3.1: 扩展 `NetError`（在 v0.27.0 的 error.rs 中添加变体，或创建 tcpip 专用错误类型）
    - **决策**：创建 `TcpIpError` 枚举，避免修改 v0.27.0 的 error.rs（Surgical Changes）
    - 变体：DmaError/NoRoute/ArpResolutionFailed/ConnectionRefused/ConnectionReset/NotConnected/WouldBlock/TimedOut/AddrInUse/AddrNotAvailable/InvalidArgument/DhcpFailed/SocketNotFound/Unreachable/PacketTooLarge
  - [x] SubTask 3.2: 实现 `From<smoltcp::Error> for TcpIpError`
  - [x] SubTask 3.3: 实现 `From<TcpIpError> for NetError`（统一错误转换）
  - [x] SubTask 3.4: 实现 `is_retriable()` 方法（WouldBlock/TimedOut/ArpResolutionFailed 可重试）
  - [x] 验证: 错误转换测试 (10+ tests)

- [x] Task 4: 实现 tcpip/device.rs — SmolcpDevice 适配器
  - [x] SubTask 4.1: 定义 `SmolcpDevice<D: NetDevice>` 结构体（device: D, mtu: usize, rx_queue: VecDeque<Vec<u8>>）
  - [x] SubTask 4.2: 实现 `SmolcpDevice::new(device: D) -> Self`（从 NetDevice 获取 MTU）
  - [x] SubTask 4.3: 实现 `SmolcpDevice::recv_frame(&mut self)` — 从 NetDevice::recv() 读取帧放入 rx_queue
  - [x] SubTask 4.4: 定义 `RxToken` 和 `TxToken` 结构体（实现 smoltcp::phy::RxToken / TxToken trait）
    - RxToken: 持有 Vec<u8>（帧数据），`consume()` 回调处理帧
    - TxToken: 持有 &mut D（NetDevice 引用）+ Vec<u8>（发送缓冲），`consume()` 调用 NetDevice::send()
  - [x] SubTask 4.5: 实现 `smoltcp::phy::Device` trait for `SmolcpDevice<D: NetDevice>`
    - `receive()`: 从 rx_queue 取帧，返回 (RxToken, TxToken)
    - `transmit()`: 返回 TxToken
    - `capabilities()`: 返回 DeviceCapabilities（mtu, medium=Ethernet, 无 checksum offload）
  - [x] 验证: 适配器测试（用 MockNetDevice 模拟帧收发）(15+ tests)

- [x] Task 5: 实现 tcpip/interface.rs — NetworkInterface 包装
  - [x] SubTask 5.1: 定义 `InterfaceConfig` 结构体（mac_addr: [u8;6], ipv4_addr: Option<Ipv4Cidr>, gateway: Option<Ipv4Addr>, dhcp: bool）
  - [x] SubTask 5.2: 定义 `NetworkInterface<D: NetDevice>` 结构体（iface: smoltcp::iface::Interface, device: SmolcpDevice<D>, sockets: SocketSet, dhcp_handle: Option<SocketHandle>）
  - [x] SubTask 5.3: 实现 `NetworkInterface::new(device: D, config: InterfaceConfig) -> Self`
    - 创建 SmolcpDevice
    - 创建 smoltcp::iface::InterfaceConfig（hardware_addr, medium）
    - 创建 smoltcp::iface::Interface
    - 配置 IP 地址和网关
    - 若 dhcp=true，创建 DHCP socket
  - [x] SubTask 5.4: 实现 `poll(&mut self, timestamp_ms: u64) -> Result<(), TcpIpError>`
    - 先调用 device.recv_frame() 从 NetDevice 读取帧
    - 调用 iface.poll(timestamp) 推进协议栈
    - 若有 DHCP，检查租约状态
  - [x] SubTask 5.5: 实现 `poll_at(&self) -> Option<u64>` — 返回下次需要 poll 的时间
  - [x] SubTask 5.6: 实现 `add_ipv4_addr(&mut self, addr: Ipv4Cidr)` / `ipv4_addr(&self) -> Option<Ipv4Addr>` / `gateway(&self) -> Option<Ipv4Addr>` / `set_dhcp(&mut self, on: bool)`
  - [x] 验证: Interface 创建 + poll 测试 (15+ tests)

- [x] Task 6: 实现 tcpip/socket.rs — TCP/UDP/ICMP Socket 包装
  - [x] SubTask 6.1: 定义 `SocketSet` 包装 `smoltcp::socket::SocketSet`
    - `new() -> Self`
    - `add_tcp(&mut self, rx_size: usize, tx_size: usize) -> SocketHandle`
    - `add_udp(&mut self, rx_size: usize, tx_size: usize) -> SocketHandle`
    - `add_icmp(&mut self, rx_size: usize, tx_size: usize) -> SocketHandle`
    - `remove(&mut self, handle: SocketHandle)`
  - [x] SubTask 6.2: 实现 `TcpSocket` 包装
    - `new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>) -> Self`（创建 smoltcp TcpSocket + 加入 SocketSet）
    - `listen(&mut self, iface: &mut NetworkInterface<impl NetDevice>, port: u16) -> Result<(), TcpIpError>`
    - `connect(&mut self, iface: &mut NetworkInterface<impl NetDevice>, remote: SocketAddr) -> Result<(), TcpIpError>`
    - `send(&mut self, iface: &mut NetworkInterface<impl NetDevice>, data: &[u8]) -> Result<usize, TcpIpError>`
    - `recv(&mut self, iface: &mut NetworkInterface<impl NetDevice>, buf: &mut [u8]) -> Result<usize, TcpIpError>`
    - `close(&mut self, iface: &mut NetworkInterface<impl NetDevice>)`
    - `state(&self, iface: &NetworkInterface<impl NetDevice>) -> TcpState`（映射 smoltcp::socket::tcp::State）
  - [x] SubTask 6.3: 实现 `UdpSocket` 包装
    - `new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>) -> Self`
    - `bind(&mut self, iface: &mut NetworkInterface<impl NetDevice>, port: u16) -> Result<(), TcpIpError>`
    - `send_to(&mut self, iface: &mut NetworkInterface<impl NetDevice>, data: &[u8], dst: SocketAddr) -> Result<usize, TcpIpError>`
    - `recv_from(&mut self, iface: &mut NetworkInterface<impl NetDevice>, buf: &mut [u8]) -> Result<(usize, SocketAddr), TcpIpError>`
  - [x] SubTask 6.4: 实现 `IcmpSocket` 包装
    - `new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>) -> Self`
    - `send_ping(&mut self, iface: &mut NetworkInterface<impl NetDevice>, dst: Ipv4Addr, seq: u16) -> Result<(), TcpIpError>`
    - `recv_pong(&mut self, iface: &mut NetworkInterface<impl NetDevice>) -> Result<(Ipv4Addr, u16), TcpIpError>`
  - [x] SubTask 6.5: 定义 `TcpState` 枚举（映射 smoltcp::socket::tcp::State：Closed/Listen/SynSent/SynReceived/Established/FinWait1/FinWait2/CloseWait/Closing/LastAck/TimeWait）
  - [x] 验证: Socket 创建 + API 调用测试（用 MockNetDevice，不需要真实网络）(20+ tests)
  - **设计偏差记录**：smoltcp 0.13 的 `tcp::Socket::connect()` 拒绝 port 0（返回 `ConnectError::Unaddressable`），因此 `TcpSocket::connect()` 增加 `local_port: u16` 参数（非零本地端口）。

- [x] Task 7: 实现 tcpip/dhcp.rs — DHCP 客户端包装
  - [x] SubTask 7.1: 定义 `DhcpState` 枚举（Init/Selecting/Requesting/Bound/Renewing/Rebinding）
  - [x] SubTask 7.2: 定义 `DhcpLease` 结构体（addr: Ipv4Cidr, gateway: Option<Ipv4Addr>, dns_servers: Vec<Ipv4Addr>, lease_duration: u64, server_id: Ipv4Addr）
  - [x] SubTask 7.3: 实现 `DhcpClient` 包装
    - `new() -> Self`（创建 smoltcp dhcpv4 Socket + 配置）
    - `start(&mut self, iface: &mut NetworkInterface<impl NetDevice>) -> Result<(), TcpIpError>`
    - `poll(&mut self, iface: &mut NetworkInterface<impl NetDevice>) -> Result<DhcpState, TcpIpError>`（查询 smoltcp DHCP socket 状态）
    - `lease(&self, iface: &NetworkInterface<impl NetDevice>) -> Option<DhcpLease>`
  - [x] 验证: DHCP 状态机测试 (10+ tests)
  - **设计说明**：smoltcp 0.13 的 `dhcpv4::Socket` 使用事件驱动模型（`socket.poll()` 返回 `Option<Event>`），无公开 `state()` 方法。`DhcpClient` 通过处理事件本地跟踪状态与租约。

- [x] Task 8: tcpip/mod.rs 导出与文档注释
  - [x] SubTask 8.1: mod.rs 添加 pub use 导出（SmolcpDevice/NetworkInterface/InterfaceConfig/SocketSet/TcpSocket/UdpSocket/IcmpSocket/TcpState/DhcpClient/DhcpState/DhcpLease/Ipv4Addr/Ipv4Cidr/SocketAddr/SocketHandle/TcpIpError）
  - [x] SubTask 8.2: lib.rs 添加 tcpip 模块 re-exports
  - [x] SubTask 8.3: mod.rs 添加 crate 文档注释（架构图 + 使用示例 + smoltcp 集成说明）
  - [x] 验证: `cargo doc -p eneros-net --no-deps` 生成文档无警告

- [x] Task 9: 文档与配置
  - [x] SubTask 9.1: 创建 `docs/drivers/tcpip-stack-design.md`（设计文档：smoltcp 集成架构 + 适配器设计 + 接口管理 + Socket API + DHCP + 测试策略 + 性能基准 + 设计决策 + 后续版本 + no_std 合规 + 参考）
  - [x] SubTask 9.2: 创建 `configs/tcpip.toml`（默认 TCP/IP 配置：[interface] ipv4_addr/gateway/dhcp，[tcp] rx_buffer_size/tx_buffer_size，[udp] rx_buffer_size/tx_buffer_size，[dhcp] retry_count/timeout）
  - [x] 验证: 文档位于 `docs/drivers/`（§2.3.3 文档分类）

- [x] Task 10: 版本标识更新
  - [x] SubTask 10.1: 根 `Cargo.toml` workspace.package.version = "0.28.0"（Task 1 已改）
  - [x] SubTask 10.2: `Makefile` VERSION := 0.28.0 + 添加 tcpip-build/tcpip-test 目标
  - [x] SubTask 10.3: `.github/workflows/ci.yml` Version: v0.28.0 + 更新 "Build net crate" 步骤名称以显式包含 tcpip stack（tcpip 在 eneros-net 内，复用现有构建步骤避免冗余编译）
  - [x] SubTask 10.4: `ci/src/gate.rs` 注释添加 eneros-net tcpip（v0.28.0 TCP/IP 协议栈）说明
  - [x] SubTask 10.5: 更新 `deny.toml`（确认 smoltcp 0BSD 许可证已在 allow 列表，无需修改）
  - [x] 验证: 版本号一致性 + ci.yml 含 tcpip 构建步骤

- [x] Task 11: 构建与质量验证
  - [x] SubTask 11.1: `cargo fmt -p eneros-net -- --check` 通过
  - [x] SubTask 11.2: `cargo clippy -p eneros-net --all-targets -- -D warnings` 通过
  - [x] SubTask 11.3: `cargo test -p eneros-net` 通过（v0.27.0 的 130 测试 + v0.28.0 新增 127 测试 = 257 tests，超过 205+ 目标）
  - [x] SubTask 11.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归测试全部 PASS）
  - [x] SubTask 11.5: `cargo run -p eneros-ci` 通过（Overall: PASS，fmt/clippy/audit/test 全部 ✅）
  - [x] SubTask 11.6: aarch64 交叉编译通过（WSL2 Ubuntu-22.04，smoltcp v0.13.1，3.81s）
  - [x] SubTask 11.7: `cargo deny check advisories licenses bans sources` 通过（`advisories ok, bans ok, licenses ok, sources ok`，smoltcp 0BSD 已允许）
  - [x] 验证: 所有检查项 PASS

# Task Dependencies

- Task 1 (骨架) 无依赖，先开始
- Task 2 (addr) 依赖 Task 1
- Task 3 (error) 依赖 Task 1
- Task 4 (device) 依赖 Task 1, 2, 3
- Task 5 (interface) 依赖 Task 4
- Task 6 (socket) 依赖 Task 5
- Task 7 (dhcp) 依赖 Task 5, 6
- Task 8 (mod.rs) 依赖 Task 2-7
- Task 9 (文档) 依赖 Task 7
- Task 10 (版本) 可与 Task 2-7 并行
- Task 11 (验证) 依赖 Task 8,9,10 全部完成

# 并行化建议

- **Wave 1**: Task 1（骨架 + smoltcp 依赖）
- **Wave 2（并行）**: Task 2（addr）、Task 3（error）
- **Wave 3**: Task 4（device 适配器）
- **Wave 4**: Task 5（interface）
- **Wave 5（并行）**: Task 6（socket）、Task 7（dhcp）
- **Wave 6（并行）**: Task 8（mod.rs）、Task 9（文档）、Task 10（版本标识）
- **Wave 7**: Task 11（验证）

# 关键技术要点

## smoltcp::phy::Device trait 适配

smoltcp 的 Device trait 使用 RxToken/TxToken 模式（零拷贝），而我们的 NetDevice 是拷贝式（send/recv 字节切片）。适配器需要：
1. `SmolcpDevice` 维护 `rx_queue: VecDeque<Vec<u8>>` 缓冲接收的帧
2. `receive()` 从 rx_queue 取帧，包装为 RxToken
3. `transmit()` 返回 TxToken，TxToken 持有 &mut D 引用
4. TxToken::consume() 将缓冲数据通过 NetDevice::send() 发送

## smoltcp Socket 生命周期

smoltcp 的 Socket 不直接持有数据，而是通过 SocketSet 管理：
1. 创建 socket（带 rx/tx 缓冲）
2. 加入 SocketSet，获得 SocketHandle
3. 通过 handle + SocketSet 引用访问 socket
4. 我们的包装层持有 handle，操作时需要传入 &mut NetworkInterface（包含 SocketSet）

## smoltcp Interface 轮询

smoltcp 是事件驱动的，需要外部定期调用 `Interface::poll(timestamp)`：
1. 主循环定期调用 `iface.poll(timestamp_ms)`
2. poll 前先从 NetDevice 读取帧放入 rx_queue
3. poll 内部处理接收帧、超时、DHCP 等
4. `poll_at()` 返回下次需要 poll 的最早时间

## no_std 合规

- smoltcp 本身是 `#![no_std]`，启用 `alloc` feature 使用堆分配
- 包装层代码遵循 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 测试时启用 std（`#[cfg(test)]`）
- 无 `use std::*`，用 `alloc::collections::VecDeque`

## 测试策略

由于 smoltcp 已有自己的测试套件，我们不测试 smoltcp 协议实现本身，而是测试：
1. **SmolcpDevice 适配器**：用 MockNetDevice（实现 NetDevice trait）模拟帧收发，验证帧正确传递
2. **NetworkInterface**：验证 poll() 调用流程、IP 配置
3. **Socket 包装**：验证 API 调用正确转发到 smoltcp socket
4. **错误转换**：验证 smoltcp::Error → TcpIpError → NetError 映射
5. **DHCP**：验证状态查询

测试不需要真实网络，全部用 mock。

## smoltcp 0.13 API 关键差异（实现中发现）

- `Interface::poll_at()` 和 `poll_delay()` 需要 `&mut self`（非 `&self`）
- `Instant::from_millis<T: Into<i64>>` — u64 不实现 `Into<i64>`，需 `as i64` 转换
- `Instant::millis()` 返回 `i64`；`Duration::total_millis()` 返回 `u64`（类型不同）
- `tcp::Socket::new` 接受 `RingBuffer`（非 `PacketBuffer`）
- `tcp::Socket::connect()` 拒绝 port 0，需非零本地端口
- `udp::Socket::recv_slice` 返回 `UdpMetadata`（非 `IpEndpoint`），需提取 `meta.endpoint`
- `icmp::Socket::new` 接受 2 个参数（rx + tx buffer），使用 `send_slice` 而非 `send`
- DHCP 使用事件驱动模型，无公开 `state()` 方法
- `Routes::get_default_ipv4_route()` 返回 `Option<Route>`，`Route` 是结构体（非枚举）
