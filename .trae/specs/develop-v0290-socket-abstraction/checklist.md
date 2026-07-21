# Checklist — v0.29.0 Socket 抽象层

> 验证清单：所有检查项必须通过才能标记版本完成。

## 一、目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：v0.29.0 不新增 crate，复用 `crates/drivers/net/`（v0.27.0 已就位）
- [x] **C2 workspace members**：根 `Cargo.toml` 已包含 `crates/drivers/net`（无需修改）
- [x] **C3 跨 crate path 引用**：v0.29.0 不新增跨 crate 引用，无新增外部依赖
- [x] **C4 文档分类**：新文档 `docs/drivers/socket-abstraction-design.md` 在 `docs/drivers/` 下，未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录下无新增 Rust crate 文件夹

## 二、源代码模块校验

- [x] **C6 socket/ 子模块**：`crates/drivers/net/src/socket/` 创建，包含 5 个文件（mod.rs/api.rs/manager.rs/poll.rs/event.rs）
- [x] **C7 mod.rs 模块声明**：`pub mod api/manager/poll/event` + re-exports
- [x] **C8 lib.rs 修改**：添加 `pub mod socket;` + socket re-exports + VERSION = "0.29.0"
- [x] **C9 v0.27.0 源文件未修改**：error.rs / eth_frame.rs / dma_ring.rs / phy.rs / mac.rs / mock.rs 保持不变（Surgical Changes）
- [x] **C10 v0.28.0 源文件未修改**：tcpip/mod.rs / addr.rs / error.rs / device.rs / interface.rs / socket.rs / dhcp.rs 保持不变（Surgical Changes）
- [x] **C11 no_std 合规**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，无 `use std::*`，用 `alloc::collections::BTreeMap`

## 三、功能实现校验

### api.rs
- [x] **C12 SocketId 类型别名**：`pub type SocketId = usize`
- [x] **C13 SocketKind 枚举**：TcpStream / TcpListener / Udp
- [x] **C14 SocketError 枚举**：11 变体（NotConnected/ConnectionRefused/ConnectionReset/WouldBlock/TimedOut/BrokenPipe/AddrInUse/AddrNotAvailable/InvalidArgument/Closed/IoError(String)）
- [x] **C15 From<TcpIpError> for SocketError**：实现 v0.28.0 错误转换
- [x] **C16 From<NetError> for SocketError**：实现 v0.27.0 错误转换
- [x] **C17 Socket trait**：定义 read/write/close/set_nonblocking/is_readable/is_writable/local_addr/remote_addr
- [x] **C18 句柄类型**：TcpStream / TcpListener / UdpSocket（SocketId newtype + id() 方法）

### event.rs
- [x] **C19 Event 结构体**：socket_id: SocketId, readiness: Readiness
- [x] **C20 Event::new()**：构造函数

### poll.rs
- [x] **C21 Readiness newtype**：u8 + 常量（READABLE/WRITABLE/ERROR/EMPTY）+ 方法（empty/is_empty/contains/insert/remove）
- [x] **C22 Interest 结构体**：readable/writable/error: bool + Default + helper constructors
- [x] **C23 Poll 结构体**：registry: BTreeMap<SocketId, Interest>
- [x] **C24 Poll::new() / register() / deregister() / modify()**：注册管理方法
- [x] **C25 Poll::check_readiness()**：根据 Interest 和 socket 状态返回 Readiness

### manager.rs
- [x] **C26 SocketEntry 结构体**：handle / kind / nonblocking / local_addr / remote_addr
- [x] **C27 SocketManager<D: NetDevice>**：iface / sockets / next_id / poll
- [x] **C28 SocketManager::new()**：创建 NetworkInterface + 空 sockets map + Poll
- [x] **C29 tcp_connect()**：创建 smoltcp TcpSocket + connect + 返回 TcpStream
- [x] **C30 tcp_listen()**：创建 smoltcp TcpSocket + listen + 返回 TcpListener
- [x] **C31 tcp_accept()**：检查 listener 状态，Established 返回 (TcpStream, addr)，否则 WouldBlock
- [x] **C32 udp_bind()**：创建 smoltcp UdpSocket + bind + 返回 UdpSocket
- [x] **C33 close()**：从 sockets map 移除 + 从 SocketSet 移除 + 从 poll 注销
- [x] **C34 read() / write()**：TCP IO 操作（委托 smoltcp TcpSocket）
- [x] **C35 send_to() / recv_from()**：UDP IO 操作（委托 smoltcp UdpSocket）
- [x] **C36 is_readable() / is_writable()**：查询 smoltcp socket 的 can_recv / can_send
- [x] **C37 local_addr() / remote_addr() / set_nonblocking() / socket_kind()**：状态查询
- [x] **C38 poll_interface() / poll_at() / ipv4_addr()**：委托 NetworkInterface
- [x] **C39 register() / deregister() / modify_interest()**：委托内部 Poll
- [x] **C40 poll_once()**：遍历 registry，返回就绪 Event 列表

## 四、构建校验（§2.4.2）

- [x] **C41 cargo metadata**：`cargo metadata --format-version 1 > /dev/null` 成功
- [x] **C42 cargo build**：`cargo build -p eneros-net` 编译成功
- [x] **C43 cargo test**：`cargo test -p eneros-net` 通过（370 个单元测试 + 3 个 doc-tests，6 个 ignored；v0.27.0 的 130 + v0.28.0 的 127 + v0.29.0 新增 113 = 370 tests）
- [x] **C44 aarch64 交叉编译**：`cargo build -p eneros-net --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（WSL2 Ubuntu-22.04，2.83s，smoltcp v0.13.1 + eneros-net v0.29.0）
- [x] **C45 cargo fmt**：`cargo fmt -p eneros-net -- --check` 通过
- [x] **C46 cargo clippy**：`cargo clippy -p eneros-net --all-targets -- -D warnings` 无 warning
- [x] **C47 cargo deny check**：`cargo deny check licenses bans sources` 通过（advisories 因 GitHub 网络不可达无法拉取 RustSec 数据库，licenses/bans/sources 均通过；v0.28.0 验证记录显示网络可用时 advisories 通过）
- [x] **C48 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部 PASS
- [x] **C49 eneros-ci**：`cargo run -p eneros-ci` — fmt/clippy/test 通过；audit 因 GitHub 网络不可达失败（环境问题，非代码问题）

## 五、文档与规范校验（§2.4.3）

- [x] **C50 设计文档**：`docs/drivers/socket-abstraction-design.md` 已创建，包含 SocketManager 架构 + 句柄类型 + Poll 多路复用 + 非阻塞 IO + 错误处理 + 测试策略 + 性能基准 + 设计决策 + 偏差声明 + no_std 合规 + 内存预算 + 参考
- [x] **C51 配置模板**：`configs/socket.toml` 已创建，包含 [tcp] / [udp] / [manager] / [poll] 配置段
- [x] **C52 文档位置**：新文档在 `docs/drivers/` 下，未放 `docs/` 根
- [x] **C53 无垃圾文件**：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] **C54 .gitignore 覆盖**：无新产生的文件类型需要忽略

## 六、版本标识校验

- [x] **C55 根 Cargo.toml**：workspace.package.version = "0.29.0"
- [x] **C56 crates/drivers/net/Cargo.toml**：version = "0.29.0"
- [x] **C57 lib.rs VERSION**：`pub const VERSION: &str = "0.29.0"`
- [x] **C58 Makefile**：VERSION := 0.29.0 + 含 socket-build/socket-test 目标
- [x] **C59 ci.yml**：Version: v0.29.0
- [x] **C60 gate.rs**：注释含 v0.29.0 Socket 抽象层说明

## 七、设计原则合规

- [x] **C61 Karpathy Think Before Coding**：SocketManager 集中管理（非 Box<dyn Socket>），偏差已声明理由
- [x] **C62 Karpathy Simplicity First**：句柄类型（SocketId newtype）+ 手动位运算（无 bitflags）+ BTreeMap（无 hashbrown）
- [x] **C63 Karpathy Surgical Changes**：仅修改 lib.rs 和 Cargo.toml，v0.27.0/v0.28.0 共 13 个源文件未修改
- [x] **C64 Karpathy Goal-Driven Execution**：测试覆盖 SocketManager 生命周期 + IO + Poll + 错误转换
- [x] **C65 ADR 合规**：未引入自研组件，复用 smoltcp + v0.28.0 类型
- [x] **C66 偏差声明**：spec.md 明确记录 Socket trait 不为 smoltcp 后端实现的原因

## 八、内存预算声明（§5.6）

- [x] **C67 内存预算声明**：在 `docs/drivers/socket-abstraction-design.md` 声明 SocketManager + 64 连接内存占用（预估 ≤ 10 MB）
- [x] **C68 OOM 策略**：在文档中说明 OOM 时降级策略（关闭非关键 Socket、缩减缓冲区、降级到 L1）

## 九、后续版本解锁

- [x] **C69 解锁 v0.46.0**：Modbus TCP（依赖 v0.29.0 Socket API）
- [x] **C70 解锁 v0.48.0**：IEC 104（依赖 v0.29.0 Socket API）
- [x] **C71 解锁 Phase 2**：DDS / gRPC（依赖 v0.29.0 Socket API）
