# Checklist — v0.28.0 TCP/IP 协议栈集成

> 验证清单：所有检查项必须通过才能标记版本完成。
> Task 11 相关的检查项（C45/C48/C50）需运行交叉编译/cargo-deny/eneros-ci 后再勾选。

## 一、目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：v0.28.0 不新增 crate，复用 `crates/drivers/net/`（v0.27.0 已就位）
- [x] **C2 workspace members**：根 `Cargo.toml` 已包含 `crates/drivers/net`（v0.27.0 已添加，v0.28.0 无需修改）
- [x] **C3 跨 crate path 引用**：v0.28.0 不新增跨 crate 引用，仅添加 smoltcp 外部依赖
- [x] **C4 文档分类**：新文档 `docs/drivers/tcpip-stack-design.md` 在 `docs/drivers/` 下，未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录下无新增 Rust crate 文件夹

## 二、smoltcp 集成校验

- [x] **C6 smoltcp 依赖**：`crates/drivers/net/Cargo.toml` 添加 `smoltcp = { version = "0.13", default-features = false, features = [...] }`
- [x] **C7 smoltcp features**：启用 `alloc` / `medium-ethernet` / `proto-ipv4` / `socket-tcp` / `socket-udp` / `socket-icmp` / `socket-dhcpv4`，未启用 `proto-ipv6` / `medium-ip` / `async` / `socket-raw` / `socket-dns`
- [x] **C8 smoltcp 许可证**：0BSD 已在 `deny.toml` 允许列表（无需修改 deny.toml）
- [x] **C9 默认集成合规**：遵循 §5.5 默认集成清单（固定 smoltcp，禁止自研 TCP/IP 栈）

## 三、源代码模块校验

- [x] **C10 tcpip/ 子模块**：`crates/drivers/net/src/tcpip/` 创建，包含 7 个文件（mod.rs/device.rs/interface.rs/socket.rs/dhcp.rs/addr.rs/error.rs）
- [x] **C11 mod.rs 模块声明**：`pub mod device/interface/socket/dhcp/addr/error` + re-exports
- [x] **C12 lib.rs 修改**：添加 `pub mod tcpip;` + tcpip re-exports + VERSION = "0.28.0"
- [x] **C13 v0.27.0 源文件未修改**：error.rs / eth_frame.rs / dma_ring.rs / phy.rs / mac.rs / mock.rs 保持不变（Surgical Changes）
- [x] **C14 no_std 合规**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，无 `use std::*`，用 `alloc::collections::VecDeque`

## 四、功能实现校验

### 地址类型（addr.rs）
- [x] **C15 Ipv4Addr 类型别名**：`pub type Ipv4Addr = smoltcp::wire::Ipv4Address`
- [x] **C16 Ipv4Cidr 类型别名**：`pub type Ipv4Cidr = smoltcp::wire::Ipv4Cidr`
- [x] **C17 SocketAddr 类型别名**：`pub type SocketAddr = smoltcp::wire::IpEndpoint`
- [x] **C18 SocketHandle 类型别名**：`pub type SocketHandle = smoltcp::socket::SocketHandle`
- [x] **C19 helper 函数**：`ipv4_addr(a,b,c,d)` / `ipv4_cidr(addr, prefix)` 可用

### 错误转换（error.rs）
- [x] **C20 TcpIpError 枚举**：包含 15+ 变体（DmaError/NoRoute/ArpResolutionFailed/ConnectionRefused/ConnectionReset/NotConnected/WouldBlock/TimedOut/AddrInUse/AddrNotAvailable/InvalidArgument/DhcpFailed/SocketNotFound/Unreachable/PacketTooLarge）
- [x] **C21 From<smoltcp::Error>**：实现 `From<smoltcp::Error> for TcpIpError`
- [x] **C22 From<TcpIpError> for NetError**：实现统一错误转换
- [x] **C23 is_retriable()**：WouldBlock/TimedOut/ArpResolutionFailed 返回 true

### 设备适配器（device.rs）
- [x] **C24 SmolcpDevice<D: NetDevice>**：结构体含 device / mtu / rx_queue: VecDeque<Vec<u8>>
- [x] **C25 RxToken**：实现 smoltcp::phy::RxToken trait，consume() 回调处理帧
- [x] **C26 TxToken**：实现 smoltcp::phy::TxToken trait，consume() 调用 NetDevice::send()
- [x] **C27 smoltcp::phy::Device impl**：实现 receive() / transmit() / capabilities()
- [x] **C28 capabilities()**：返回正确的 MTU、medium=Ethernet、无 checksum offload

### 网络接口（interface.rs）
- [x] **C29 InterfaceConfig**：含 mac_addr / ipv4_addr / gateway / dhcp 字段
- [x] **C30 NetworkInterface::new()**：创建 SmolcpDevice + smoltcp::iface::Interface + 配置 IP/网关 + 可选 DHCP
- [x] **C31 poll(timestamp_ms)**：先从 NetDevice 读取帧 → 调用 iface.poll() → 处理 DHCP 租约
- [x] **C32 poll_at()**：返回下次需要 poll 的最早时间
- [x] **C33 add_ipv4_addr / ipv4_addr / gateway / set_dhcp**：IP 管理接口可用

### Socket（socket.rs）
- [x] **C34 SocketSet**：包装 smoltcp::socket::SocketSet，提供 add_tcp/add_udp/add_icmp/remove
- [x] **C35 TcpSocket**：new / listen / connect / send / recv / close / state 全部实现
- [x] **C36 UdpSocket**：new / bind / send_to / recv_from 全部实现
- [x] **C37 IcmpSocket**：new / send_ping / recv_pong 全部实现
- [x] **C38 TcpState 枚举**：映射 smoltcp::socket::tcp::State（11 个状态）

### DHCP（dhcp.rs）
- [x] **C39 DhcpState 枚举**：Init / Selecting / Requesting / Bound / Renewing / Rebinding
- [x] **C40 DhcpLease 结构体**：addr / gateway / dns_servers / lease_duration / server_id
- [x] **C41 DhcpClient**：new / start / poll / lease 全部实现

## 五、构建校验（§2.4.2）

- [x] **C42 cargo metadata**：`cargo metadata --format-version 1 > /dev/null` 成功
- [x] **C43 cargo build**：`cargo build -p eneros-net` 编译成功
- [x] **C44 cargo test**：`cargo test -p eneros-net` 通过（v0.27.0 的 130 测试 + v0.28.0 新增 127 测试 = 257 tests，超过 205+ 目标）
- [x] **C45 aarch64 交叉编译**：`cargo build -p eneros-net --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（WSL2 Ubuntu-22.04，smoltcp v0.13.1，3.81s）— Task 11 ✅
- [x] **C46 cargo fmt**：`cargo fmt -p eneros-net -- --check` 通过
- [x] **C47 cargo clippy**：`cargo clippy -p eneros-net --all-targets -- -D warnings` 无 warning
- [x] **C48 cargo deny check**：`cargo deny check advisories licenses bans sources` 通过（`advisories ok, bans ok, licenses ok, sources ok`，smoltcp 0BSD 已允许）— Task 11 ✅
- [x] **C49 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部 PASS
- [x] **C50 eneros-ci**：`cargo run -p eneros-ci` Overall: PASS（fmt/clippy/audit/test 全部 ✅）— Task 11 ✅

## 六、文档与规范校验（§2.4.3）

- [x] **C51 设计文档**：`docs/drivers/tcpip-stack-design.md` 已创建，包含 smoltcp 集成架构 + 适配器设计 + 接口管理 + Socket API + DHCP + 测试策略 + 性能基准 + 设计决策 + no_std 合规 + 参考
- [x] **C52 配置模板**：`configs/tcpip.toml` 已创建，包含 [interface] / [tcp] / [udp] / [dhcp] 配置段
- [x] **C53 文档位置**：新文档在 `docs/drivers/` 下，未放 `docs/` 根
- [x] **C54 无垃圾文件**：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] **C55 .gitignore 覆盖**：无新产生的文件类型需要忽略

## 七、版本标识校验

- [x] **C56 根 Cargo.toml**：workspace.package.version = "0.28.0"
- [x] **C57 crates/drivers/net/Cargo.toml**：version = "0.28.0"
- [x] **C58 lib.rs VERSION**：`pub const VERSION: &str = "0.28.0"`
- [x] **C59 Makefile**：VERSION := 0.28.0 + 含 tcpip-build/tcpip-test 目标
- [x] **C60 ci.yml**：Version: v0.28.0 + "Build net crate" 步骤名称显式包含 tcpip stack
- [x] **C61 gate.rs**：注释含 eneros-net tcpip（v0.28.0 TCP/IP 协议栈 — smoltcp integration）说明

## 八、设计原则合规

- [x] **C62 Karpathy Think Before Coding**：集成 smoltcp 而非自研（遵循 ADR §5.5 + Blueprint.md §3644）
- [x] **C63 Karpathy Simplicity First**：薄包装层，不重新实现协议（直接复用 smoltcp::wire 类型）
- [x] **C64 Karpathy Surgical Changes**：仅修改 lib.rs 和 Cargo.toml，v0.27.0 源文件未修改
- [x] **C65 Karpathy Goal-Driven Execution**：测试覆盖适配器/接口/Socket/DHCP/错误转换，不测试 smoltcp 自身
- [x] **C66 ADR-0001 合规**：未引入自研内核组件
- [x] **C67 ADR-0004 合规**：v0.28.0 属于 Phase 1 MVP 联邦基础组件，符合最小可商用集合定义

## 九、内存预算声明（§5.6）

- [x] **C68 smoltcp 内存占用声明**：在 `docs/drivers/tcpip-stack-design.md` 声明 smoltcp 协议栈内存占用（预估 ≤ 2MB，含 Socket 缓冲）
- [x] **C69 OOM 策略**：在文档中说明 OOM 时降级策略（关闭非关键 Socket、缩减缓冲区）

## 十、后续版本解锁

- [x] **C70 解锁 v0.29.0**：Socket 抽象层（v0.28.0 的 TcpSocket/UdpSocket/IcmpSocket 上层封装）
- [x] **C71 解锁 v0.46.0**：Modbus TCP（依赖 v0.28.0 TCP Socket）
- [x] **C72 解锁 v0.48.0**：IEC 104（依赖 v0.28.0 TCP Socket）

---

## 验证状态汇总

- **已通过**：69/72 检查项（C1~C44, C46~C47, C49, C51~C72）
- **待验证（Task 11）**：3 项（C45 aarch64 交叉编译 / C48 cargo deny / C50 eneros-ci）
- **测试统计**：257 个单元测试通过（v0.27.0 的 130 + v0.28.0 新增 127），超过 205+ 目标
