# Tasks — v0.30.0 + v0.30.1 + v0.30.2 网络安全 + 蜂窝通信 + 双网冗余

## v0.30.0 — 网络栈安全与性能（eneros-net 扩展）

- [x] Task 1: v0.30.0 模块骨架 + 版本标识
  - [ ] SubTask 1.1: 修改 `crates/drivers/net/Cargo.toml`：version 改为 "0.30.0"（无新增依赖）
  - [ ] SubTask 1.2: 创建 `crates/drivers/net/src/security/mod.rs`：模块声明 + re-exports 占位
  - [ ] SubTask 1.3: 创建 `crates/drivers/net/src/perf/mod.rs`：模块声明占位
  - [ ] SubTask 1.4: 修改 `crates/drivers/net/src/lib.rs`：添加 `pub mod security; pub mod perf;` + VERSION 改为 "0.30.0"
  - [ ] SubTask 1.5: 修改根 `Cargo.toml`：workspace.package.version 改为 "0.30.0"
  - [ ] 验证: `cargo build -p eneros-net` 编译成功

- [x] Task 2: security/firewall.rs — 防火墙规则引擎
  - [ ] SubTask 2.1: 定义 `IpProtocol` 类型别名（`pub type IpProtocol = smoltcp::wire::IpProtocol;`）
  - [ ] SubTask 2.2: 定义 `FirewallAction` 枚举（Allow / Drop / Reject）
  - [ ] SubTask 2.3: 定义 `FirewallPolicy` 枚举（AllowAll / DropAll）
  - [ ] SubTask 2.4: 定义 `FirewallRule` 结构体（action / src_ip: Option<Ipv4Cidr> / dst_port: Option<u16> / protocol: Option<IpProtocol>）
  - [ ] SubTask 2.5: 定义 `Firewall` 结构体（rules: Vec<FirewallRule> / default_policy / conn_tracker: ConnectionTracker）
  - [ ] SubTask 2.6: 实现 `Firewall::new(default: FirewallPolicy, conn_tracker: ConnectionTracker) -> Self`
  - [ ] SubTask 2.7: 实现 `Firewall::add_rule(&mut self, rule: FirewallRule)`
  - [ ] SubTask 2.8: 实现 `Firewall::check_connection(&mut self, src: Ipv4Addr, now: u64) -> FirewallAction`（规则匹配 → 连接数检查 → 默认策略）
  - [ ] SubTask 2.9: 实现 `Firewall::match_rule(rule: &FirewallRule, src: Ipv4Addr) -> bool`（CIDR contains 检查）
  - [ ] SubTask 2.10: 实现 `Firewall::remove_rule(&mut self, index: usize)` + `Firewall::rules(&self) -> &[FirewallRule]`
  - [ ] 验证: 防火墙规则匹配测试 (15+ tests)

- [x] Task 3: security/rate_limit.rs — 连接跟踪与速率限制
  - [ ] SubTask 3.1: 定义 `ConnInfo` 结构体（count / last_connect / rate_window / rate_count）
  - [ ] SubTask 3.2: 定义 `ConnectionTracker` 结构体（connections: BTreeMap<Ipv4Addr, ConnInfo> / max_per_ip / max_total / total）
  - [ ] SubTask 3.3: 定义 `RateLimit` 结构体（max_connections / max_rate_per_sec）
  - [ ] SubTask 3.4: 实现 `ConnectionTracker::new(max_per_ip: u32, max_total: u32) -> Self`
  - [ ] SubTask 3.5: 实现 `ConnectionTracker::try_connect(&mut self, ip: Ipv4Addr, now: u64) -> bool`（检查 per_ip 和 total 限制）
  - [ ] SubTask 3.6: 实现 `ConnectionTracker::disconnect(&mut self, ip: Ipv4Addr)`（递减计数，0 则移除）
  - [ ] SubTask 3.7: 实现 `ConnectionTracker::is_rate_limited(&self, ip: Ipv4Addr) -> bool`（检查 rate_window 内的 rate_count）
  - [ ] SubTask 3.8: 实现 `ConnectionTracker::count_for(&self, ip: Ipv4Addr) -> u32` + `total(&self) -> u32`
  - [ ] 验证: 连接数限制 + 速率限制测试 (15+ tests)

- [x] Task 4: security/ddos.rs — DDoS 防护
  - [ ] SubTask 4.1: 定义 `SynInfo` 结构体（syn_count / window_start）
  - [ ] SubTask 4.2: 定义 `DdosProtector` 结构体（syn_tracker: BTreeMap<Ipv4Addr, SynInfo> / syn_rate_threshold / window_ms）
  - [ ] SubTask 4.3: 定义 `SecurityError` 枚举（BlockedByFirewall / RateLimited / ConnectionLimitExceeded / SuspiciousActivity）
  - [ ] SubTask 4.4: 实现 `DdosProtector::new(syn_rate_threshold: u32, window_ms: u64) -> Self`
  - [ ] SubTask 4.5: 实现 `DdosProtector::check_syn(&mut self, src: Ipv4Addr, now: u64) -> bool`（窗口内 SYN 计数，超阈值返回 false）
  - [ ] SubTask 4.6: 实现 `DdosProtector::is_under_attack(&self) -> bool`（检查是否有 IP 超阈值）
  - [ ] SubTask 4.7: 实现 `DdosProtector::reset(&mut self)`（清空追踪器）
  - [ ] 验证: SYN Flood 检测测试 (10+ tests)

- [x] Task 5: security/mod.rs — 模块导出
  - [ ] SubTask 5.1: 添加 `pub mod firewall; pub mod rate_limit; pub mod ddos;`
  - [ ] SubTask 5.2: 添加 `pub use` 导出所有公共类型
  - [ ] 验证: 模块编译通过

- [x] Task 6: perf/benchmark.rs + perf/mod.rs — 性能基准测试
  - [ ] SubTask 6.1: 定义 `BenchmarkResult` 结构体（throughput_kbps / latency_us / packets_per_sec）
  - [ ] SubTask 6.2: 定义 `BenchmarkSuite` 结构体（results: Vec<(String, BenchmarkResult)>）
  - [ ] SubTask 6.3: 实现 `BenchmarkSuite::new() -> Self`
  - [ ] SubTask 6.4: 实现 `BenchmarkSuite::run_firewall_benchmark(&mut self, iterations: u32) -> BenchmarkResult`（创建 Firewall + 规则，循环检查延迟）
  - [ ] SubTask 6.5: 实现 `BenchmarkSuite::run_connection_benchmark(&mut self, iterations: u32) -> BenchmarkResult`（创建 ConnectionTracker，循环 try_connect）
  - [ ] SubTask 6.6: 实现 `BenchmarkSuite::results(&self) -> &[(String, BenchmarkResult)]`
  - [ ] SubTask 6.7: perf/mod.rs 添加 `pub mod benchmark; pub use benchmark::*;`
  - [ ] 验证: 基准测试运行 + 结果记录测试 (8+ tests)

## v0.30.1 — 蜂窝通信模块（新 crate eneros-cellular）

- [x] Task 7: cellular crate 骨架
  - [ ] SubTask 7.1: 创建 `crates/drivers/cellular/Cargo.toml`（name="eneros-cellular", version="0.30.0", 依赖 eneros-hal + smoltcp + alloc）
  - [ ] SubTask 7.2: 创建 `crates/drivers/cellular/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + VERSION="0.30.0" + 模块声明）
  - [ ] SubTask 7.3: 创建 `crates/drivers/cellular/src/error.rs`（CellularError 5 变体）
  - [ ] SubTask 7.4: 修改根 `Cargo.toml`：members 添加 `"crates/drivers/cellular"`
  - [ ] 验证: `cargo build -p eneros-cellular` 编译成功

- [x] Task 8: at_command.rs — AT 命令封装
  - [ ] SubTask 8.1: 定义 `AtCommand` 结构体（cmd: String / args: Vec<String> / timeout_ms: u32）
  - [ ] SubTask 8.2: 定义 `AtResponse` 枚举（Ok(String) / Error(String) / Timeout）
  - [ ] SubTask 8.3: 定义 `AtParser` 结构体
  - [ ] SubTask 8.4: 实现 `AtParser::encode(cmd: &AtCommand) -> String`（格式化为 "AT+CMD=arg1,arg2\r\n"）
  - [ ] SubTask 8.5: 实现 `AtParser::parse_response(raw: &str) -> Result<AtResponse, CellularError>`（解析 OK/ERROR/+CMD: 响应）
  - [ ] SubTask 8.6: 定义 `SignalStrength` 结构体（rssi: i8 / ber: u8 / network_type: NetworkType）
  - [ ] SubTask 8.7: 定义 `NetworkType` 枚举（Unknown / Gsm / Wcdma / Lte / Nr5g）
  - [ ] SubTask 8.8: 实现 `AtParser::parse_signal(raw: &str) -> Result<SignalStrength, CellularError>`（解析 "+CSQ: rssi,ber"）
  - [ ] SubTask 8.9: 实现 `AtParser::parse_operator(raw: &str) -> Result<String, CellularError>`（解析 "+COPS:" 响应）
  - [ ] 验证: AT 命令编码/解码 + 信号解析测试 (15+ tests)

- [x] Task 9: ppp.rs — PPP 拨号协议
  - [ ] SubTask 9.1: 定义 `PppState` 枚举（Closed / Establishing / Authenticating / Networking / Connected / Terminating）
  - [ ] SubTask 9.2: 定义 `PppFrame` 结构体（protocol: u16 / data: Vec<u8>）
  - [ ] SubTask 9.3: 定义 `PppStateMachine` 结构体（state / retry_count / max_retries / assigned_ip: Option<Ipv4Addr>）
  - [ ] SubTask 9.4: 实现 `PppStateMachine::new(max_retries: u32) -> Self`
  - [ ] SubTask 9.5: 实现 `PppStateMachine::state(&self) -> PppState`
  - [ ] SubTask 9.6: 实现 `PppStateMachine::on_lcp_config_ack(&mut self) -> Result<(), CellularError>`（Establishing → Authenticating）
  - [ ] SubTask 9.7: 实现 `PppStateMachine::on_auth_success(&mut self) -> Result<(), CellularError>`（Authenticating → Networking）
  - [ ] SubTask 9.8: 实现 `PppStateMachine::on_ipcp_config_ack(&mut self, ip: Ipv4Addr) -> Result<(), CellularError>`（Networking → Connected）
  - [ ] SubTask 9.9: 实现 `PppStateMachine::on_error(&mut self) -> Result<(), CellularError>`（重试或终止）
  - [ ] SubTask 9.10: 实现 `PppStateMachine::terminate(&mut self)`（→ Terminating → Closed）
  - [ ] SubTask 9.11: 实现 `PppStateMachine::assigned_ip(&self) -> Option<Ipv4Addr>`
  - [ ] SubTask 9.12: 实现 `PppFrame::encode(&self) -> Vec<u8>`（HDLC 帧：Flag + Protocol + Data + FCS + Flag）
  - [ ] SubTask 9.13: 实现 `PppFrame::decode(raw: &[u8]) -> Result<PppFrame, CellularError>`（解析 HDLC 帧）
  - [ ] 验证: PPP 状态机迁移 + 帧编解码测试 (15+ tests)

- [x] Task 10: modem.rs — CellularModem 驱动
  - [ ] SubTask 10.1: 定义 `CellularDriver` trait（send_at / dial / hang_up / signal）
  - [ ] SubTask 10.2: 定义 `RetryConfig` 结构体（max_retries / retry_interval_ms）
  - [ ] SubTask 10.3: 定义 `CellularModem<S: HalSerial>` 结构体（serial / at_parser / ppp / apn / retry_config）
  - [ ] SubTask 10.4: 实现 `CellularModem::new(serial: S, apn: &str, retry_config: RetryConfig) -> Self`
  - [ ] SubTask 10.5: 实现 `CellularModem::send_at(&mut self, cmd: &AtCommand) -> Result<AtResponse, CellularError>`（编码 → serial.write → 等待响应 → 解析）
  - [ ] SubTask 10.6: 实现 `CellularModem::check_signal(&mut self) -> Result<SignalStrength, CellularError>`（发送 AT+CSQ）
  - [ ] SubTask 10.7: 实现 `CellularModem::check_sim(&mut self) -> Result<bool, CellularError>`（发送 AT+CCID 检查 SIM）
  - [ ] SubTask 10.8: 实现 `CellularModem::dial(&mut self, apn: &str) -> Result<Ipv4Addr, CellularError>`（ATD*99# → PPP 协商 → 返回 IP）
  - [ ] SubTask 10.9: 实现 `CellularModem::hang_up(&mut self) -> Result<(), CellularError>`（PPP 终止 + ATH）
  - [ ] SubTask 10.10: 实现 `CellularDriver` trait for `CellularModem<S: HalSerial>`
  - [ ] 验证: Modem 驱动测试（用 MockSerial 实现 HalSerial）(15+ tests)

## v0.30.2 — 双网冗余与切换（cellular crate 扩展）

- [x] Task 11: heartbeat.rs — 心跳监测
  - [ ] SubTask 11.1: 定义 `HeartbeatMonitor` 结构体（interval_ms / timeout_ms / last_heartbeat / missed_count / max_missed）
  - [ ] SubTask 11.2: 实现 `HeartbeatMonitor::new(interval_ms: u64, timeout_ms: u64, max_missed: u32) -> Self`
  - [ ] SubTask 11.3: 实现 `HeartbeatMonitor::on_heartbeat(&mut self, now: u64)`（重置 missed_count，更新 last_heartbeat）
  - [ ] SubTask 11.4: 实现 `HeartbeatMonitor::check_timeout(&mut self, now: u64) -> bool`（检查是否超时，递增 missed_count，超 max_missed 返回 true）
  - [ ] SubTask 11.5: 实现 `HeartbeatMonitor::is_alive(&self) -> bool`（missed_count < max_missed）
  - [ ] SubTask 11.6: 实现 `HeartbeatMonitor::missed_count(&self) -> u32` + `last_heartbeat(&self) -> u64`
  - [ ] 验证: 心跳超时判定测试 (10+ tests)

- [x] Task 12: failover.rs — 故障切换管理
  - [ ] SubTask 12.1: 定义 `LinkType` 枚举（Ethernet / Cellular）
  - [ ] SubTask 12.2: 定义 `FailoverState` 枚举（PrimaryActive / BackupActive / Switching / Recovering）
  - [ ] SubTask 12.3: 定义 `FailoverEvent` 枚举（PrimaryDown / PrimaryUp / SwitchCompleted / RecoveryCompleted）
  - [ ] SubTask 12.4: 定义 `FailoverError` 枚举（NoBackupAvailable / SwitchInProgress / HeartbeatTimeout / InvalidState）
  - [ ] SubTask 12.5: 定义 `FailoverManager` 结构体（state / active / heartbeat_primary / heartbeat_backup / failover_count / recovery_delay_ms / last_failover_time / callback）
  - [ ] SubTask 12.6: 实现 `FailoverManager::new(recovery_delay_ms: u64) -> Self`（初始 PrimaryActive）
  - [ ] SubTask 12.7: 实现 `FailoverManager::on_event(&mut self, event: FailoverEvent, now: u64) -> Result<LinkType, FailoverError>`（状态机迁移）
  - [ ] SubTask 12.8: 实现 `FailoverManager::current_active(&self) -> LinkType` + `state(&self) -> FailoverState`
  - [ ] SubTask 12.9: 实现 `FailoverManager::force_switch(&mut self, target: LinkType, now: u64) -> Result<(), FailoverError>`（手动切换）
  - [ ] SubTask 12.10: 实现 `FailoverManager::register_callback(&mut self, cb: fn(FailoverEvent))`
  - [ ] SubTask 12.11: 实现 `FailoverManager::check_heartbeats(&mut self, now: u64) -> Option<FailoverEvent>`（检查心跳，返回可能的 FailoverEvent）
  - [ ] SubTask 12.12: 实现 `FailoverManager::failover_count(&self) -> u32`
  - [ ] 验证: 切换状态机 + 防抖回切 + 手动切换测试 (20+ tests)

- [x] Task 13: redundancy.rs — 双网冗余管理器
  - [ ] SubTask 13.1: 定义 `LinkState` 结构体（link_type: LinkType / is_up: bool / ipv4_addr: Option<Ipv4Addr>）
  - [ ] SubTask 13.2: 定义 `RedundancyManager` 结构体（primary_link / backup_link / active / failover_mgr）
  - [ ] SubTask 13.3: 实现 `RedundancyManager::new() -> Self`（初始 primary=Ethernet up, backup=Cellular down）
  - [ ] SubTask 13.4: 实现 `RedundancyManager::set_primary_status(&mut self, up: bool, now: u64)`（更新状态 + 触发 failover 事件）
  - [ ] SubTask 13.5: 实现 `RedundancyManager::set_backup_status(&mut self, up: bool, now: u64)`
  - [ ] SubTask 13.6: 实现 `RedundancyManager::set_primary_addr(&mut self, addr: Ipv4Addr)` + `set_backup_addr(&mut self, addr: Ipv4Addr)`
  - [ ] SubTask 13.7: 实现 `RedundancyManager::current_active(&self) -> LinkType` + `failover_count(&self) -> u32`
  - [ ] SubTask 13.8: 实现 `RedundancyManager::check_heartbeats(&mut self, now: u64) -> Option<FailoverEvent>`
  - [ ] SubTask 13.9: 实现 `RedundancyManager::primary_link(&self) -> &LinkState` + `backup_link(&self) -> &LinkState`
  - [ ] 验证: 双网冗余管理测试 (15+ tests)

- [x] Task 14: cellular/src/lib.rs 模块完善
  - [ ] SubTask 14.1: lib.rs 添加 `pub mod at_command; pub mod ppp; pub mod modem; pub mod heartbeat; pub mod failover; pub mod redundancy;`
  - [ ] SubTask 14.2: lib.rs 添加 `pub use` 导出所有公共类型
  - [ ] SubTask 14.3: lib.rs 添加 crate 文档注释（架构 + 使用示例 + 偏差声明）
  - [ ] 验证: `cargo doc -p eneros-cellular --no-deps` 生成文档无警告

## 共通任务

- [x] Task 15: 文档创建
  - [ ] SubTask 15.1: 创建 `docs/drivers/net-security-design.md`（v0.30.0: 防火墙 + 连接限制 + DDoS + 基准 + 内存预算 + OOM 策略）
  - [ ] SubTask 15.2: 创建 `docs/drivers/cellular-modem-design.md`（v0.30.1: AT 命令 + PPP + modem 驱动 + 偏差声明）
  - [ ] SubTask 15.3: 创建 `docs/drivers/dual-network-redundancy.md`（v0.30.2: 心跳 + 故障切换 + 防抖 + 状态机图）
  - [ ] 验证: 文档位于 `docs/drivers/`（§2.3.3 文档分类）

- [x] Task 16: 配置文件创建
  - [ ] SubTask 16.1: 创建 `configs/net-security.toml`（[firewall] default_policy/max_rules，[connection] max_per_ip/max_total，[ddos] syn_rate_threshold/window_ms）
  - [ ] SubTask 16.2: 创建 `configs/cellular.toml`（[modem] apn/baud_rate/max_retries/retry_interval_ms，[at] default_timeout_ms）
  - [ ] SubTask 16.3: 创建 `configs/failover.toml`（[heartbeat] interval_ms/timeout_ms/max_missed，[failover] recovery_delay_ms）
  - [ ] 验证: 配置文件格式正确

- [x] Task 17: 版本标识更新
  - [ ] SubTask 17.1: 根 `Cargo.toml` workspace.package.version = "0.30.0"（Task 1 已改）
  - [ ] SubTask 17.2: `Makefile` VERSION := 0.30.0 + 添加 cellular-build/cellular-test 目标
  - [ ] SubTask 17.3: `.github/workflows/ci.yml` Version: v0.30.0 + 添加 eneros-cellular 到构建步骤
  - [ ] SubTask 17.4: `ci/src/gate.rs` 注释添加 v0.30.0/v0.30.1/v0.30.2 说明 + eneros-cellular crate 配置
  - [ ] 验证: 版本号一致性

- [x] Task 18: 构建与质量验证
  - [x] SubTask 18.1: `cargo fmt --all -- --check` 通过（首次运行发现 modem.rs/redundancy.rs 格式问题，已 `cargo fmt --all` 自动修复后通过）
  - [x] SubTask 18.2: `cargo clippy -p eneros-net -p eneros-cellular --all-targets -- -D warnings` 通过（0 warnings）
  - [x] SubTask 18.3: `cargo test -p eneros-net` 通过（435 tests + 4 doc-tests passed，含 v0.30.0 新增 ~65 tests）
  - [x] SubTask 18.4: `cargo test -p eneros-cellular` 通过（139 tests passed）
  - [x] SubTask 18.5: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归测试全绿）
  - [x] SubTask 18.6: `cargo run -p eneros-ci` — fmt ✓ / clippy ✓ / test ✓，audit ❌（GitHub 网络不可达，环境问题，非代码问题；licenses/bans/sources 单独运行全 OK）
  - [x] SubTask 18.7: aarch64 交叉编译通过（WSL2 Ubuntu-22.04，3.13s 完成，smoltcp v0.13.1 + eneros-cellular v0.30.0）
  - [x] SubTask 18.8: `cargo deny check licenses bans sources` 通过；advisories 因 GitHub 网络不可达跳过（与 v0.29.0 相同环境问题，非代码问题）
  - [x] 验证: 所有代码相关检查项 PASS；audit/advisories 失败为环境问题（GitHub 网络不可达），与 v0.29.0 相同

# Task Dependencies

## v0.30.0 依赖链
- Task 1 (骨架) 无依赖
- Task 2 (firewall) 依赖 Task 1
- Task 3 (rate_limit) 依赖 Task 1（firewall 引用 ConnectionTracker，但可先定义后引用）
- Task 4 (ddos) 依赖 Task 1
- Task 5 (security/mod.rs) 依赖 Task 2, 3, 4
- Task 6 (perf) 依赖 Task 2, 3（benchmark 引用 Firewall + ConnectionTracker）

## v0.30.1 依赖链
- Task 7 (cellular 骨架) 依赖 Task 1（版本号一致）
- Task 8 (at_command) 依赖 Task 7
- Task 9 (ppp) 依赖 Task 7
- Task 10 (modem) 依赖 Task 8, 9

## v0.30.2 依赖链
- Task 11 (heartbeat) 依赖 Task 7
- Task 12 (failover) 依赖 Task 11
- Task 13 (redundancy) 依赖 Task 12
- Task 14 (lib.rs 完善) 依赖 Task 8-13

## 共通任务依赖
- Task 15 (文档) 依赖 Task 6, 14
- Task 16 (配置) 依赖 Task 7
- Task 17 (版本标识) 可与 Task 2-14 并行
- Task 18 (验证) 依赖 Task 14, 15, 16, 17 全部完成

# 并行化建议

- **Wave 1**: Task 1（v0.30.0 骨架）+ Task 7（cellular 骨架）
- **Wave 2（并行）**: Task 2 (firewall)、Task 3 (rate_limit)、Task 4 (ddos)、Task 8 (at_command)、Task 9 (ppp)、Task 11 (heartbeat)
- **Wave 3**: Task 5 (security/mod.rs)、Task 6 (perf)、Task 10 (modem)、Task 12 (failover)
- **Wave 4**: Task 13 (redundancy)、Task 14 (cellular lib.rs)
- **Wave 5（并行）**: Task 15 (文档)、Task 16 (配置)、Task 17 (版本标识)
- **Wave 6**: Task 18 (验证)

# 关键技术要点

## v0.30.0 防火墙与 smoltcp 类型复用

```rust
// 使用 smoltcp 的类型，不引入新依赖
pub type IpProtocol = smoltcp::wire::IpProtocol;
use crate::tcpip::addr::{Ipv4Addr, Ipv4Cidr};

// Ipv4Cidr 含 contains 方法，可直接用于规则匹配
fn match_rule(rule: &FirewallRule, src: Ipv4Addr) -> bool {
    if let Some(ref cidr) = rule.src_ip {
        if !cidr.contains(&src) { return false; }
    }
    true
}
```

## v0.30.1 HalSerial 与 MockSerial

cellular crate 依赖 `eneros-hal` 的 `HalSerial` trait。测试时需创建 MockSerial：

```rust
pub struct MockSerial {
    tx_buf: alloc::vec::Vec<u8>,
    rx_buf: alloc::vec::Vec<u8>,
}

impl HalSerial for MockSerial {
    fn write(&self, data: &[u8]) -> Result<usize, HalError> { ... }
    fn read(&self, buf: &mut [u8]) -> Result<usize, HalError> { ... }
    fn flush(&self) -> Result<(), HalError> { Ok(()) }
}
```

## v0.30.2 状态机迁移图

```
PrimaryActive --PrimaryDown--> Switching --SwitchCompleted--> BackupActive
BackupActive --PrimaryUp--> Recovering --RecoveryCompleted--> PrimaryActive
任意状态 --force_switch--> Switching
```

## no_std 合规

- 所有新文件 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- `BTreeMap` 替代 `HashMap`
- `alloc::vec::Vec` / `alloc::string::String`
- cellular crate 依赖 eneros-hal + smoltcp，均为 no_std 兼容

## 测试策略

- **v0.30.0**: 纯软件测试（防火墙规则匹配、连接限制、DDoS 检测、基准测试）
- **v0.30.1**: Mock 测试（MockSerial 实现 HalSerial，测试 AT 命令 + PPP 状态机 + modem 驱动）
- **v0.30.2**: 纯软件测试（心跳超时、切换状态机、防抖逻辑、冗余管理器）
- **集成测试延后**: 真实 modem 拨号、拔网线切换 — 需硬件环境
