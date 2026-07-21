# Checklist — v0.30.0 + v0.30.1 + v0.30.2 网络安全 + 蜂窝通信 + 双网冗余

> 验证清单：所有检查项必须通过才能标记版本完成。

## 一、目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：v0.30.1 新增 `crates/drivers/cellular/` 在 `crates/drivers/` 下，未放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 members 添加 `"crates/drivers/cellular"`
- [x] **C3 跨 crate path 引用**：`crates/drivers/cellular/Cargo.toml` 的 eneros-hal/smoltcp 依赖使用正确的相对路径
- [x] **C4 文档分类**：3 个新文档在 `docs/drivers/` 下，未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录下无新增 Rust crate 文件夹

## 二、v0.30.0 源代码模块校验（eneros-net security + perf）

- [x] **C6 security/ 子模块**：`crates/drivers/net/src/security/` 创建，包含 4 文件（mod.rs/firewall.rs/rate_limit.rs/ddos.rs）
- [x] **C7 perf/ 子模块**：`crates/drivers/net/src/perf/` 创建，包含 2 文件（mod.rs/benchmark.rs）
- [x] **C8 lib.rs 修改**：添加 `pub mod security; pub mod perf;` + VERSION = "0.30.0"
- [x] **C9 v0.27.0~v0.29.0 源文件未修改**：18 个现有源文件保持不变（Surgical Changes）— lib.rs 和 Cargo.toml 修改为 spec 允许，未修改 v0.27.0~v0.29.0 的其他 18 个源文件
- [x] **C10 no_std 合规**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，无 `use std::*`，用 `BTreeMap`

### firewall.rs
- [x] **C11 IpProtocol 类型别名**：`pub type IpProtocol = smoltcp::wire::IpProtocol`
- [x] **C12 FirewallAction 枚举**：Allow / Drop / Reject
- [x] **C13 FirewallPolicy 枚举**：AllowAll / DropAll
- [x] **C14 FirewallRule 结构体**：action / src_ip / dst_port / protocol
- [x] **C15 Firewall 结构体**：rules: Vec / default_policy / conn_tracker
- [x] **C16 Firewall::new()**：创建防火墙实例
- [x] **C17 Firewall::add_rule()**：添加规则
- [x] **C18 Firewall::check_connection()**：规则匹配 → 连接数检查 → 默认策略
- [x] **C19 Firewall::match_rule()**：CIDR contains 检查
- [x] **C20 Firewall::remove_rule() / rules()**：规则管理

### rate_limit.rs
- [x] **C21 ConnInfo 结构体**：count / last_connect / rate_window / rate_count
- [x] **C22 ConnectionTracker 结构体**：BTreeMap<Ipv4Addr, ConnInfo> + max_per_ip + max_total
- [x] **C23 RateLimit 结构体**：max_connections / max_rate_per_sec
- [x] **C24 ConnectionTracker::new()**：创建连接跟踪器
- [x] **C25 ConnectionTracker::try_connect()**：检查 per_ip 和 total 限制
- [x] **C26 ConnectionTracker::disconnect()**：递减计数，0 则移除
- [x] **C27 ConnectionTracker::is_rate_limited()**：检查速率窗口内计数
- [x] **C28 ConnectionTracker::count_for() / total()**：状态查询

### ddos.rs
- [x] **C29 SynInfo 结构体**：syn_count / window_start
- [x] **C30 DdosProtector 结构体**：BTreeMap<Ipv4Addr, SynInfo> + syn_rate_threshold + window_ms
- [x] **C31 SecurityError 枚举**：BlockedByFirewall / RateLimited / ConnectionLimitExceeded / SuspiciousActivity
- [x] **C32 DdosProtector::new()**：创建 DDoS 防护器
- [x] **C33 DdosProtector::check_syn()**：窗口内 SYN 计数检查
- [x] **C34 DdosProtector::is_under_attack()**：攻击状态查询
- [x] **C35 DdosProtector::reset()**：清空追踪器

### perf/benchmark.rs
- [x] **C36 BenchmarkResult 结构体**：throughput_kbps / latency_us / packets_per_sec
- [x] **C37 BenchmarkSuite 结构体**：results: Vec<(String, BenchmarkResult)>
- [x] **C38 BenchmarkSuite::new()**：创建基准套件
- [x] **C39 BenchmarkSuite::run_firewall_benchmark()**：防火墙基准测试
- [x] **C40 BenchmarkSuite::run_connection_benchmark()**：连接跟踪基准测试
- [x] **C41 BenchmarkSuite::results()**：结果查询

## 三、v0.30.1 源代码模块校验（eneros-cellular）

- [x] **C42 cellular crate 创建**：`crates/drivers/cellular/Cargo.toml` + `src/lib.rs` + `src/error.rs`
- [x] **C43 Cargo.toml 依赖**：eneros-hal + smoltcp + alloc，版本="0.30.0"
- [x] **C44 lib.rs no_std**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + VERSION="0.30.0"
- [x] **C45 error.rs**：CellularError 5 变体（NoSimCard/NoSignal/DialFailed/AtCommandTimeout/PppNegotiationFailed）

### at_command.rs
- [x] **C46 AtCommand 结构体**：cmd: String / args: Vec<String> / timeout_ms: u32
- [x] **C47 AtResponse 枚举**：Ok(String) / Error(String) / Timeout
- [x] **C48 AtParser 结构体**：AT 命令解析器
- [x] **C49 AtParser::encode()**：AT 命令编码为 "AT+CMD=arg1,arg2\r\n"
- [x] **C50 AtParser::parse_response()**：解析 OK/ERROR/+CMD: 响应
- [x] **C51 SignalStrength 结构体**：rssi: i8 / ber: u8 / network_type: NetworkType
- [x] **C52 NetworkType 枚举**：Unknown / Gsm / Wcdma / Lte / Nr5g
- [x] **C53 AtParser::parse_signal()**：解析 "+CSQ: rssi,ber"
- [x] **C54 AtParser::parse_operator()**：解析 "+COPS:" 响应

### ppp.rs
- [x] **C55 PppState 枚举**：Closed / Establishing / Authenticating / Networking / Connected / Terminating
- [x] **C56 PppFrame 结构体**：protocol: u16 / data: Vec<u8>
- [x] **C57 PppStateMachine 结构体**：state / retry_count / max_retries / assigned_ip
- [x] **C58 PppStateMachine::new()**：创建状态机
- [x] **C59 PppStateMachine 状态迁移方法**：on_lcp_config_ack / on_auth_success / on_ipcp_config_ack / on_error / terminate
- [x] **C60 PppStateMachine::assigned_ip()**：查询分配的 IP
- [x] **C61 PppFrame::encode()**：HDLC 帧编码（Flag + Protocol + Data + FCS + Flag）
- [x] **C62 PppFrame::decode()**：HDLC 帧解码

### modem.rs
- [x] **C63 CellularDriver trait**：send_at / dial / hang_up / signal
- [x] **C64 RetryConfig 结构体**：max_retries / retry_interval_ms
- [x] **C65 CellularModem<S: HalSerial> 结构体**：serial / at_parser / ppp / apn / retry_config — 实际实现中 AtParser 为单元结构（无状态），CellularModem 不持有 at_parser 字段，AtParser::encode 等以静态方法形式调用，功能等价
- [x] **C66 CellularModem::new()**：创建 modem 实例
- [x] **C67 CellularModem::send_at()**：AT 命令发送 + 响应接收
- [x] **C68 CellularModem::check_signal()**：AT+CSQ 信号查询
- [x] **C69 CellularModem::check_sim()**：AT+CCID SIM 卡检查
- [x] **C70 CellularModem::dial()**：ATD*99# → PPP 协商 → 返回 IP
- [x] **C71 CellularModem::hang_up()**：PPP 终止 + ATH
- [x] **C72 CellularDriver impl for CellularModem**：trait 实现

## 四、v0.30.2 源代码模块校验（双网冗余）

### heartbeat.rs
- [x] **C73 HeartbeatMonitor 结构体**：interval_ms / timeout_ms / last_heartbeat / missed_count / max_missed
- [x] **C74 HeartbeatMonitor::new()**：创建心跳监测器
- [x] **C75 HeartbeatMonitor::on_heartbeat()**：重置 missed_count
- [x] **C76 HeartbeatMonitor::check_timeout()**：检查超时 + 递增 missed_count
- [x] **C77 HeartbeatMonitor::is_alive()**：存活状态查询

### failover.rs
- [x] **C78 LinkType 枚举**：Ethernet / Cellular
- [x] **C79 FailoverState 枚举**：PrimaryActive / BackupActive / Switching / Recovering
- [x] **C80 FailoverEvent 枚举**：PrimaryDown / PrimaryUp / SwitchCompleted / RecoveryCompleted
- [x] **C81 FailoverError 枚举**：NoBackupAvailable / SwitchInProgress / HeartbeatTimeout / InvalidState
- [x] **C82 FailoverManager 结构体**：state / active / heartbeat_primary / heartbeat_backup / failover_count / recovery_delay_ms / callback（实际还包含 last_failover_time 字段，扩展不破坏 spec）
- [x] **C83 FailoverManager::new()**：创建故障切换管理器（初始 PrimaryActive）
- [x] **C84 FailoverManager::on_event()**：状态机迁移
- [x] **C85 FailoverManager::current_active() / state()**：状态查询
- [x] **C86 FailoverManager::force_switch()**：手动强制切换
- [x] **C87 FailoverManager::register_callback()**：注册事件回调
- [x] **C88 FailoverManager::check_heartbeats()**：心跳检查
- [x] **C89 FailoverManager::failover_count()**：切换计数

### redundancy.rs
- [x] **C90 LinkState 结构体**：link_type / is_up / ipv4_addr
- [x] **C91 RedundancyManager 结构体**：primary_link / backup_link / active / failover_mgr
- [x] **C92 RedundancyManager::new()**：创建冗余管理器
- [x] **C93 RedundancyManager::set_primary_status() / set_backup_status()**：链路状态更新
- [x] **C94 RedundancyManager::set_primary_addr() / set_backup_addr()**：IP 地址设置
- [x] **C95 RedundancyManager::current_active() / failover_count()**：状态查询
- [x] **C96 RedundancyManager::check_heartbeats()**：心跳检查
- [x] **C97 RedundancyManager::primary_link() / backup_link()**：链路状态查询

### cellular/src/lib.rs
- [x] **C98 lib.rs 模块声明**：所有 7 个子模块（error/at_command/ppp/modem/heartbeat/failover/redundancy）— checklist 中"8 个"为笔误，实际 7 个全部声明
- [x] **C99 lib.rs re-exports**：pub use 导出所有公共类型
- [x] **C100 lib.rs 文档注释**：架构 + 使用示例 + 偏差声明

## 五、构建校验（§2.4.2）

- [x] **C101 cargo metadata**：`cargo metadata --format-version 1 > /dev/null` 成功（已运行确认 META_OK）
- [x] **C102 cargo build eneros-net**：`cargo build -p eneros-net` 编译成功（由 C104 测试通过隐含）
- [x] **C103 cargo build eneros-cellular**：`cargo build -p eneros-cellular` 编译成功（由 C105 测试通过隐含）
- [x] **C104 cargo test eneros-net**：`cargo test -p eneros-net` 通过（435 tests + 4 doc-tests）
- [x] **C105 cargo test eneros-cellular**：`cargo test -p eneros-cellular` 通过（139 tests）
- [x] **C106 aarch64 交叉编译**：`cargo build -p eneros-net -p eneros-cellular --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过（WSL2，3.13s，smoltcp v0.13.1）
- [x] **C107 cargo fmt**：`cargo fmt --all -- --check` 通过（modem.rs 和 redundancy.rs 格式问题已由 `cargo fmt --all` 自动修复）
- [x] **C108 cargo clippy**：`cargo clippy -p eneros-net -p eneros-cellular --all-targets -- -D warnings` 无 warning（0 warnings）
- [~] **C109 cargo deny check**：`cargo deny check advisories licenses bans sources` — advisories FAILED（GitHub 网络不可达，环境因素非代码问题）；licenses / bans / sources PASS
- [x] **C110 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部 PASS
- [~] **C111 eneros-ci**：`cargo run -p eneros-ci` Overall: fmt/clippy/test PASS；audit FAILED（GitHub 网络不可达，环境因素非代码问题）

## 六、文档与规范校验（§2.4.3）

- [x] **C112 net-security-design.md**：v0.30.0 设计文档已创建，含防火墙 + 连接限制 + DDoS + 基准 + 内存预算 + OOM 策略
- [x] **C113 cellular-modem-design.md**：v0.30.1 设计文档已创建，含 AT 命令 + PPP + modem 驱动 + 偏差声明
- [x] **C114 dual-network-redundancy.md**：v0.30.2 设计文档已创建，含心跳 + 故障切换 + 防抖 + 状态机图
- [x] **C115 配置模板**：3 个配置文件（net-security.toml / cellular.toml / failover.toml）已创建
- [x] **C116 文档位置**：3 个新文档在 `docs/drivers/` 下，未放 `docs/` 根
- [x] **C117 无垃圾文件**：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪（仅 ci.yml/.gitignore/Cargo.lock/Cargo.toml/Makefile/ci/src/gate.rs 修改 + 新 crate/docs/configs 未追踪）
- [x] **C118 .gitignore 覆盖**：无新产生的文件类型需要忽略

## 七、版本标识校验

- [x] **C119 根 Cargo.toml**：workspace.package.version = "0.30.0"
- [x] **C120 eneros-net Cargo.toml**：version = "0.30.0"
- [x] **C121 eneros-cellular Cargo.toml**：version = "0.30.0"
- [x] **C122 lib.rs VERSION**：eneros-net `pub const VERSION: &str = "0.30.0"` + eneros-cellular 同样
- [x] **C123 Makefile**：VERSION := 0.30.0 + 含 cellular-build/cellular-test 目标
- [x] **C124 ci.yml**：Version: v0.30.0 + eneros-cellular 在构建步骤中
- [x] **C125 gate.rs**：注释含 v0.30.0/v0.30.1/v0.30.2 说明 + eneros-cellular 配置

## 八、设计原则合规

- [x] **C126 Karpathy Think Before Coding**：硬件依赖与测试策略明确（软件可测试 + 集成测试延后）
- [x] **C127 Karpathy Simplicity First**：PacketInfo 简化 + BTreeMap 替代 HashMap + PPP 最小状态机 + 4 状态冗余机
- [x] **C128 Karpathy Surgical Changes**：v0.27.0~v0.29.0 共 18 个源文件未修改；v0.30.1 新建 crate；v0.30.2 仅在 cellular crate 添加文件
- [x] **C129 Karpathy Goal-Driven Execution**：测试覆盖防火墙 + 连接限制 + DDoS + AT 命令 + PPP 状态机 + 心跳 + 切换
- [x] **C130 ADR 合规**：未引入自研组件，复用 smoltcp + eneros-hal 类型
- [x] **C131 偏差声明**：spec.md 明确记录 HashMap→BTreeMap、PPP 最小实现、集成测试延后

## 九、内存预算声明（§5.6）

- [x] **C132 内存预算声明**：在 3 个设计文档中声明内存占用（总计 ≤ 20 KB，不含 TCP 缓冲）
- [x] **C133 OOM 策略**：在文档中说明 OOM 时降级策略（缩减规则表、关闭非关键连接、降级到 L1）

## 十、后续版本解锁

- [x] **C134 解锁 v0.31.0**：国密算法（网络栈安全基础已完成）
- [x] **C135 解锁 v0.57.0**：降级规则联动（双网故障触发本地降级）
- [x] **C136 解锁 Phase 2**：mTLS 通信安全（防火墙 + 蜂窝通道基础）
