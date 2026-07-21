# Tasks — v0.29.0 Socket 抽象层

- [x] Task 1: 模块骨架 + 版本标识
  - [x] SubTask 1.1: 修改 `crates/drivers/net/Cargo.toml`：version 改为 "0.29.0"（无新增依赖）
  - [x] SubTask 1.2: 创建 `crates/drivers/net/src/socket/mod.rs`：模块声明（pub mod api/manager/poll/event）+ re-exports + crate 文档注释
  - [x] SubTask 1.3: 修改 `crates/drivers/net/src/lib.rs`：添加 `pub mod socket;` + socket re-exports + VERSION 改为 "0.29.0"
  - [x] SubTask 1.4: 修改根 `Cargo.toml`：workspace.package.version 改为 "0.29.0"
  - [x] 验证: `cargo metadata --format-version 1 > /dev/null` 成功 + `cargo build -p eneros-net` 编译成功

- [x] Task 2: 实现 socket/api.rs — Socket trait + 类型定义
  - [x] SubTask 2.1: 定义 `SocketId = usize` 类型别名
  - [x] SubTask 2.2: 定义 `SocketKind` 枚举（TcpStream / TcpListener / Udp）
  - [x] SubTask 2.3: 定义 `SocketError` 枚举（11 变体：NotConnected/ConnectionRefused/ConnectionReset/WouldBlock/TimedOut/BrokenPipe/AddrInUse/AddrNotAvailable/InvalidArgument/Closed/IoError(String)）
  - [x] SubTask 2.4: 实现 `From<TcpIpError> for SocketError`（映射 v0.28.0 错误）
  - [x] SubTask 2.5: 实现 `From<NetError> for SocketError`（映射 v0.27.0 错误）
  - [x] SubTask 2.6: 定义 `Socket` trait（read/write/close/set_nonblocking/is_readable/is_writable/local_addr/remote_addr）
  - [x] SubTask 2.7: 定义句柄类型 `TcpStream(SocketId)` / `TcpListener(SocketId)` / `UdpSocket(SocketId)` + id() 方法 + new() (pub(crate))
  - [x] 验证: 类型编译通过 + 错误转换测试 (10+ tests)

- [x] Task 3: 实现 socket/event.rs — Event 类型
  - [x] SubTask 3.1: 定义 `Event` 结构体（socket_id: SocketId, readiness: Readiness）
  - [x] SubTask 3.2: 实现 `Event::new(socket_id, readiness)` 构造函数
  - [x] 验证: Event 编译通过 + 基本测试 (3+ tests)

- [x] Task 4: 实现 socket/poll.rs — Poll 多路复用
  - [x] SubTask 4.1: 定义 `Readiness(u8)` newtype + 常量（READABLE=0x01/WRITABLE=0x02/ERROR=0x04/EMPTY=0x00）+ 方法（empty/is_empty/contains/insert/remove）
  - [x] SubTask 4.2: 定义 `Interest` 结构体（readable: bool, writable: bool, error: bool）+ Default impl + helper constructors（all_readable/all_writable/none）
  - [x] SubTask 4.3: 定义 `Poll` 结构体（registry: BTreeMap<SocketId, Interest>）
  - [x] SubTask 4.4: 实现 `Poll::new() -> Self`
  - [x] SubTask 4.5: 实现 `Poll::register(&mut self, id: SocketId, interest: Interest)`
  - [x] SubTask 4.6: 实现 `Poll::deregister(&mut self, id: SocketId)`
  - [x] SubTask 4.7: 实现 `Poll::modify(&mut self, id: SocketId, interest: Interest)`
  - [x] SubTask 4.8: 实现 `Poll::check readiness(&self, id: SocketId, is_readable: bool, is_writable: bool) -> Readiness`（根据注册的 Interest 和 socket 状态返回 Readiness）
  - [x] 验证: Poll 注册/注销/modify/检查测试 (15+ tests)

- [x] Task 5: 实现 socket/manager.rs — SocketManager<D>
  - [x] SubTask 5.1: 定义 `SocketEntry` 结构体（handle: SocketHandle, kind: SocketKind, nonblocking: bool, local_addr: Option<SocketAddr>, remote_addr: Option<SocketAddr>）
  - [x] SubTask 5.2: 定义 `SocketManager<D: NetDevice>` 结构体（iface: NetworkInterface<D>, sockets: BTreeMap<SocketId, SocketEntry>, next_id: SocketId, poll: Poll）
  - [x] SubTask 5.3: 实现 `SocketManager::new(device: D, config: InterfaceConfig) -> Self`
  - [x] SubTask 5.4: 实现 `SocketManager::tcp_connect(&mut self, remote: SocketAddr, local_port: u16) -> Result<TcpStream, SocketError>`
    - 调用 `iface.sockets.add_tcp(rx_size, tx_size)` 创建 smoltcp TcpSocket
    - 通过 handle 获取 socket，调用 `socket.connect(IpEndpoint, local_port)`
    - 创建 SocketEntry，插入 sockets map
    - 返回 TcpStream(id)
  - [x] SubTask 5.5: 实现 `SocketManager::tcp_listen(&mut self, port: u16) -> Result<TcpListener, SocketError>`
    - 创建 smoltcp TcpSocket
    - 调用 `socket.listen(port)`
    - 返回 TcpListener(id)
  - [x] SubTask 5.6: 实现 `SocketManager::tcp_accept(&mut self, listener: TcpListener) -> Result<(TcpStream, SocketAddr), SocketError>`
    - 检查 listener socket 是否有 incoming connection
    - smoltcp 不支持 accept() 直接创建新 socket，需要应用层轮询
    - **决策**：tcp_accept 检查 listener 的 TcpState，若 Established 则返回该 socket 作为 stream
    - 若未就绪返回 `Err(SocketError::WouldBlock)`
  - [x] SubTask 5.7: 实现 `SocketManager::udp_bind(&mut self, local: SocketAddr) -> Result<UdpSocket, SocketError>`
  - [x] SubTask 5.8: 实现 `SocketManager::close(&mut self, id: SocketId) -> Result<(), SocketError>`
    - 从 sockets map 移除
    - 调用 `iface.sockets.inner.remove(handle)`
    - 从 poll registry 注销
  - [x] SubTask 5.9: 实现 `SocketManager::read(&mut self, id: SocketId, buf: &mut [u8]) -> Result<usize, SocketError>`
    - 查找 SocketEntry
    - 通过 handle 获取 smoltcp TcpSocket
    - 调用 `socket.recv_slice(buf)` → 返回字节数或 WouldBlock
  - [x] SubTask 5.10: 实现 `SocketManager::write(&mut self, id: SocketId, buf: &[u8]) -> Result<usize, SocketError>`
    - 调用 `socket.send_slice(buf)` → 返回字节数或 WouldBlock
  - [x] SubTask 5.11: 实现 `SocketManager::send_to` / `recv_from`（UDP 操作）
  - [x] SubTask 5.12: 实现 `SocketManager::is_readable` / `is_writable`（查询 smoltcp socket 的 can_recv/can_send）
  - [x] SubTask 5.13: 实现 `SocketManager::local_addr` / `remote_addr` / `set_nonblocking` / `socket_kind`
  - [x] SubTask 5.14: 实现 `SocketManager::poll_interface(timestamp_ms)` — 委托给 `iface.poll(timestamp_ms)`
  - [x] SubTask 5.15: 实现 `SocketManager::poll_at(timestamp_ms)` — 委托给 `iface.poll_at(timestamp_ms)`
  - [x] SubTask 5.16: 实现 `SocketManager::ipv4_addr()` — 委托给 `iface.ipv4_addr()`
  - [x] SubTask 5.17: 实现 `SocketManager::register` / `deregister` / `modify_interest` — 委托给内部 Poll
  - [x] SubTask 5.18: 实现 `SocketManager::poll_once() -> Vec<Event>`
    - 遍历 poll.registry
    - 对每个注册的 SocketId，检查 is_readable/is_writable
    - 若有就绪状态，创建 Event 加入返回列表
  - [x] 验证: SocketManager 创建 + socket 生命周期 + IO 操作测试 (30+ tests)

- [x] Task 6: socket/mod.rs 导出与文档注释
  - [x] SubTask 6.1: mod.rs 添加 pub use 导出（SocketManager/Socket/SocketId/SocketKind/SocketError/TcpStream/TcpListener/UdpSocket/Poll/Interest/Readiness/Event）
  - [x] SubTask 6.2: lib.rs 添加 socket 模块 re-exports
  - [x] SubTask 6.3: mod.rs 添加 crate 文档注释（架构图 + 使用示例 + SocketManager 说明 + 偏差声明）
  - [x] 验证: `cargo doc -p eneros-net --no-deps` 生成文档无警告

- [x] Task 7: 文档与配置
  - [x] SubTask 7.1: 创建 `docs/drivers/socket-abstraction-design.md`（设计文档：SocketManager 架构 + 句柄类型 + Poll 多路复用 + 非阻塞 IO + 错误处理 + 测试策略 + 性能基准 + 设计决策 + 偏差声明 + no_std 合规 + 内存预算 + 参考）
  - [x] SubTask 7.2: 创建 `configs/socket.toml`（默认 Socket 配置：[tcp] rx_buffer_size/tx_buffer_size，[udp] rx_buffer_size/tx_buffer_size，[manager] max_connections/default_nonblocking，[poll] default_timeout_ms）
  - [x] 验证: 文档位于 `docs/drivers/`（§2.3.3 文档分类）

- [x] Task 8: 版本标识更新
  - [x] SubTask 8.1: 根 `Cargo.toml` workspace.package.version = "0.29.0"（Task 1 已改）
  - [x] SubTask 8.2: `Makefile` VERSION := 0.29.0 + 添加 socket-build/socket-test 目标（作为 eneros-net 别名）
  - [x] SubTask 8.3: `.github/workflows/ci.yml` Version: v0.29.0 + 更新构建步骤说明
  - [x] SubTask 8.4: `ci/src/gate.rs` 注释添加 v0.29.0 Socket 抽象层说明
  - [x] 验证: 版本号一致性

- [x] Task 9: 构建与质量验证
  - [x] SubTask 9.1: `cargo fmt -p eneros-net -- --check` 通过
  - [x] SubTask 9.2: `cargo clippy -p eneros-net --all-targets -- -D warnings` 通过
  - [x] SubTask 9.3: `cargo test -p eneros-net` 通过（v0.27.0 的 130 + v0.28.0 的 127 + v0.29.0 新增 70+ = 327+ tests）
  - [x] SubTask 9.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归测试全部 PASS）
  - [x] SubTask 9.5: `cargo run -p eneros-ci` 通过（Overall: PASS）
  - [x] SubTask 9.6: aarch64 交叉编译通过（WSL2 Ubuntu-22.04，`cargo build -p eneros-net --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）
  - [x] SubTask 9.7: `cargo deny check advisories licenses bans sources` 通过
  - [x] 验证: 所有检查项 PASS

# Task Dependencies

- Task 1 (骨架) 无依赖，先开始
- Task 2 (api) 依赖 Task 1
- Task 3 (event) 依赖 Task 1, 2
- Task 4 (poll) 依赖 Task 1, 2, 3
- Task 5 (manager) 依赖 Task 2, 3, 4
- Task 6 (mod.rs) 依赖 Task 2-5
- Task 7 (文档) 依赖 Task 5
- Task 8 (版本) 可与 Task 2-7 并行
- Task 9 (验证) 依赖 Task 6,7,8 全部完成

# 并行化建议

- **Wave 1**: Task 1（骨架 + 版本标识）
- **Wave 2**: Task 2（api）
- **Wave 3（并行）**: Task 3（event）、Task 4（poll）
- **Wave 4**: Task 5（manager）
- **Wave 5（并行）**: Task 6（mod.rs）、Task 7（文档）、Task 8（版本标识）
- **Wave 6**: Task 9（验证）

# 关键技术要点

## SocketManager 与 smoltcp 的桥接

smoltcp 的 socket 存储在 `SocketSet`（在 `NetworkInterface` 内部），通过 `SocketHandle` 访问。SocketManager 的职责：

1. **拥有 NetworkInterface**（包括 SocketSet）
2. **维护 SocketId ↔ SocketHandle 映射**（BTreeMap<SocketId, SocketEntry>）
3. **所有操作通过 SocketId 查找 SocketHandle**，再通过 `iface.sockets.inner.get(handle)` 访问 smoltcp socket

```rust
// 示例：read 操作
fn read(&mut self, id: SocketId, buf: &mut [u8]) -> Result<usize, SocketError> {
    let entry = self.sockets.get(&id).ok_or(SocketError::Closed)?;
    let handle = entry.handle;
    let socket = self.iface.sockets.inner.get(handle);
    let socket = socket.as_tcp().ok_or(SocketError::InvalidArgument)?;
    socket.recv_slice(buf).map_err(SocketError::from)
}
```

## smoltcp TcpSocket 的 accept 模式

smoltcp 不像 POSIX 提供 `accept()` 创建新 socket。smoltcp 的 listen 模式是：
1. 创建 TcpSocket，调用 `socket.listen(port)`
2. 等待远端 SYN → smoltcp 自动完成握手
3. socket 状态变为 Established
4. 应用层检测到 Established 后，该 socket 即为"已接受的连接"

**v0.29.0 的 tcp_accept 实现策略**：
- `tcp_listen(port)` 返回 TcpListener(id)，底层是一个 listen 状态的 smoltcp TcpSocket
- `tcp_accept(listener)` 检查该 socket 的 TcpState：
  - 若 Established → 返回 (TcpStream(listener_id), remote_addr)（复用同一 socket）
  - 若未就绪 → 返回 Err(SocketError::WouldBlock)
- **限制**：每个 listener 同时只能有一个 pending connection。多客户端需创建多个 listener socket 或应用层管理 socket 池。

这是 smoltcp 架构的固有限制，不是 v0.29.0 的问题。未来如需多客户端并发 accept，需要：
1. 创建多个 TcpSocket 都 listen 同一端口（smoltcp 可能不支持）
2. 或在应用层维护 socket 池，轮询检查每个 socket 的状态

## Poll 非阻塞模型

v0.29.0 使用非阻塞 poll（`poll_once()`），不实现带超时的阻塞 poll：
- `poll_once()` 遍历所有注册的 socket，返回就绪事件列表（可能为空）
- 应用层主循环负责调用 `poll_once()` + `poll_interface(timestamp_ms)` + sleep/yield
- 这符合 RTOS 的事件驱动模型

蓝图 §8.2 提到 "非阻塞 IO 需配合调度器使用"，正符合此设计。

## no_std 合规

- 所有 socket/* 文件遵循 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::collections::BTreeMap`（不用 HashMap）
- Readiness 使用手动位运算（不引入 bitflags 依赖）
- 无 `use std::*`

## 测试策略

由于 smoltcp 的非阻塞特性，测试不需要真实网络：
1. **SocketManager 创建**：验证 new() + 配置
2. **Socket 生命周期**：tcp_connect/tcp_listen/udp_bind/close
3. **Poll 注册/注销**：register/deregister/modify/poll_once
4. **错误转换**：TcpIpError → SocketError、NetError → SocketError
5. **Readiness 位运算**：insert/remove/contains/is_empty
6. **Interest**：Default + helper constructors
7. **句柄类型**：TcpStream/TcpListener/UdpSocket 的 id() 方法

测试使用 MockNetDevice（v0.28.0 已有模式），不需要真实硬件。
