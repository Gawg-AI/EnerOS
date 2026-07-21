# EnerOS 网络安全设计文档 (v0.30.0)

> **范围**：网络栈安全与性能子系统——防火墙规则引擎、连接跟踪与速率限制、
> DDoS（SYN Flood）防护、网络性能基准测试框架。
>
> **Crate**：`eneros-net` (`crates/drivers/net/src/security/`、`crates/drivers/net/src/perf/`)
> **版本**：v0.30.0（Phase 1 Layer 6 收尾 — 网络栈安全与性能）
> **状态**：已实现 — 主机测试通过（含 firewall / rate_limit / ddos / benchmark 单元测试），aarch64 交叉编译验证通过。

---

## 1. 概述

`eneros-net::security` 与 `eneros-net::perf` 模块构成 EnerOS Edge Box 的网络栈安全
与性能基线。储能场景下 Edge Box 部署在工业现场，需抵御网络扫描、SYN Flood 等攻击，
并保证协议栈通信性能可观测。本版本交付以下能力：

| 能力 | 模块 | 说明 |
|------|------|------|
| 防火墙规则引擎 | `security/firewall.rs` | 首匹配优先的源 CIDR 规则 + 默认策略（AllowAll / DropAll） |
| 连接跟踪 + 速率限制 | `security/rate_limit.rs` | per-IP 与全局连接数上限 + 1 秒滑动窗口速率计数 |
| DDoS 防护 | `security/ddos.rs` | 基于 SYN 速率阈值的固定窗口 SYN Flood 检测 |
| 性能基准 | `perf/benchmark.rs` | 防火墙 / 连接跟踪吞吐与延迟基准测试框架 |

### 设计原则

- **no_std 合规**（蓝图 §43.1）：`alloc::collections::BTreeMap` 替代 `std::collections::HashMap`，`alloc::vec::Vec` 替代 `std::vec::Vec`。
- **类型复用**：`IpProtocol` 直接别名 `smoltcp::wire::IpProtocol`；`Ipv4Addr` / `Ipv4Cidr` 复用 v0.28.0 `tcpip/addr.rs` 类型别名（蓝图 §5.5 "禁止重复造轮子"）。
- **零运行时分配热路径**：规则匹配路径仅做读取与 BTreeMap 查询，避免在每次入网包时分配新内存。

### v0.30.0 交付物

| 组件 | 文件 | 说明 |
|------|------|------|
| 防火墙 | `security/firewall.rs` | Firewall + FirewallRule + FirewallAction + FirewallPolicy |
| 连接跟踪 | `security/rate_limit.rs` | ConnectionTracker + ConnInfo + RateLimit 配置记录 |
| DDoS 防护 | `security/ddos.rs` | DdosProtector + SynInfo + SecurityError |
| 性能基准 | `perf/benchmark.rs` | BenchmarkSuite + BenchmarkResult |
| 模块入口 | `security/mod.rs` / `perf/mod.rs` | 模块声明 + re-exports |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────────────┐
│  Caller (smoltcp interface / socket layer)           │
└─────────────────┬────────────────────────────────────┘
                  │  check_connection(src, now)
┌─────────────────▼────────────────────────────────────┐
│  eneros_net::security::Firewall                      │
│  ┌────────────────────────────────────────────────┐  │
│  │  rules: Vec<FirewallRule>  (顺序匹配)          │  │
│  │  default_policy: FirewallPolicy                │  │
│  │  conn_tracker: ConnectionTracker               │  │
│  └──────────────────┬─────────────────────────────┘  │
└─────────────────────┼────────────────────────────────┘
                      │
        ┌─────────────┴─────────────┐
        ▼                           ▼
┌────────────────────┐    ┌──────────────────────────┐
│  Rule match path   │    │  Default policy path     │
│  (first match wins)│    │  AllowAll → try_connect  │
│  → return action   │    │  DropAll  → Drop         │
└────────────────────┘    └──────────┬───────────────┘
                                     │
                          ┌──────────▼────────────────┐
                          │  ConnectionTracker        │
                          │  BTreeMap<Ipv4Addr,       │
                          │            ConnInfo>      │
                          │  per-IP / total caps +    │
                          │  1 s rate window          │
                          └───────────────────────────┘

┌──────────────────────────────────────────────────────┐
│  eneros_net::security::DdosProtector (旁路)          │
│  check_syn(src, now) → bool                          │
│  BTreeMap<Ipv4Addr, SynInfo> + 固定窗口              │
└──────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────┐
│  eneros_net::perf::BenchmarkSuite                    │
│  run_firewall_benchmark / run_connection_benchmark   │
└──────────────────────────────────────────────────────┘
```

### 2.1 防火墙规则匹配流程

`Firewall::check_connection(src, now)` 按以下顺序解析：

1. 顺序扫描 `rules`，调用 `Firewall::match_rule(rule, src)`：
   - `rule.src_ip == None` → 匹配任意源（match-all）。
   - `rule.src_ip == Some(cidr)` → 当 `cidr.contains_addr(&src)` 时匹配。
   - `dst_port` 与 `protocol` 字段在本版本**不参与匹配**（保留供后续版本使用）。
2. 命中规则时**立即返回**该规则的 `FirewallAction`（首匹配优先），连接跟踪器**不被调用**。
3. 无规则命中时进入默认策略：
   - `AllowAll` → 调用 `conn_tracker.try_connect(src, now)`；返回 `Allow` 当且仅当未超过 per-IP / total 上限，否则返回 `Drop`。
   - `DropAll` → 直接返回 `Drop`，不触及跟踪器。

### 2.2 连接跟踪与速率限制

`ConnectionTracker` 维护 `BTreeMap<Ipv4Addr, ConnInfo>`：

- **per-IP 上限**（`max_per_ip`）：单个源 IP 同时活跃连接数。
- **total 上限**（`max_total`）：全局同时活跃连接数。
- **速率窗口**（`RATE_WINDOW_MS = 1000`）：每 IP 维护一个 1 秒滑动窗口，记录 `rate_count`。窗口期满后下一次 `try_connect` 重置 `rate_count` 与 `rate_window`。
- `disconnect(ip)` 递减 per-IP 与 total 计数；当某 IP 计数归零时整条 entry 被移除，避免 BTreeMap 无限增长。
- `is_rate_limited(ip, max_per_sec)` 仅查询，不修改状态，供上层独立判断。

### 2.3 DDoS 防护（SYN Flood 检测）

`DdosProtector` 独立于 `Firewall`，由调用方在 SYN 入口处旁路调用 `check_syn(src, now)`：

- `BTreeMap<Ipv4Addr, SynInfo>` 记录每 IP 的 `syn_count` 与 `window_start`。
- **固定窗口**：当 `now - window_start >= window_ms` 时计数清零、窗口起点前移。这是嵌入式场景下最简单、最可预测的方案。
- `check_syn` 返回 `true` 表示放行；`syn_count > syn_rate_threshold` 时返回 `false`（疑似攻击，丢弃）。
- `is_under_attack()` 扫描全部跟踪 IP，返回是否有任一 IP 当前超阈值——作为全局"被攻击"指示器。
- `Ipv4Addr` 实现了 `Ord`，可直接作为 `BTreeMap` 键，无需包装。

### 2.4 性能基准框架

`BenchmarkSuite` 提供两个开箱即用的基准测试：

- `run_firewall_benchmark(iterations)`：构造包含 1 条 Allow + 1 条 Drop 规则的防火墙，循环 `check_connection`，隔离规则扫描成本（不进入连接跟踪）。
- `run_connection_benchmark(iterations)`：构造 `max_per_ip=100, max_total=10000` 的连接跟踪器，循环对 `10.0.x.y` 调用 `try_connect`。

**延迟占位说明**：`no_std` 环境无 `std::time::Instant`，当前 `LATENCY_US = 1` 为占位常量。真实测量需在硬件上接入 `HalClock::now_ns()` 并除以迭代次数。

---

## 3. 关键类型签名

```rust
// security/firewall.rs
pub type IpProtocol = smoltcp::wire::IpProtocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallAction { Allow, Drop, Reject }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallPolicy { AllowAll, DropAll }

#[derive(Debug, Clone)]
pub struct FirewallRule {
    pub action: FirewallAction,
    pub src_ip: Option<Ipv4Cidr>,
    pub dst_port: Option<u16>,   // 本版本未参与匹配
    pub protocol: Option<IpProtocol>, // 本版本未参与匹配
}

pub struct Firewall {
    rules: Vec<FirewallRule>,
    default_policy: FirewallPolicy,
    conn_tracker: ConnectionTracker,
}

// security/rate_limit.rs
#[derive(Debug, Clone)]
pub struct ConnInfo {
    pub count: u32,
    pub last_connect: u64,
    pub rate_window: u64,
    pub rate_count: u32,
}

pub struct ConnectionTracker {
    connections: BTreeMap<Ipv4Addr, ConnInfo>,
    max_per_ip: u32,
    max_total: u32,
    total: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    pub max_connections: u32,
    pub max_rate_per_sec: u32,
}

// security/ddos.rs
#[derive(Debug, Clone, Copy)]
pub struct SynInfo {
    pub syn_count: u32,
    pub window_start: u64,
}

pub struct DdosProtector {
    syn_tracker: BTreeMap<Ipv4Addr, SynInfo>,
    syn_rate_threshold: u32,
    window_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityError {
    BlockedByFirewall,
    RateLimited,
    ConnectionLimitExceeded,
    SuspiciousActivity,
}

// perf/benchmark.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchmarkResult {
    pub throughput_kbps: u32,
    pub latency_us: u32,
    pub packets_per_sec: u32,
}

#[derive(Default)]
pub struct BenchmarkSuite {
    results: Vec<(String, BenchmarkResult)>,
}
```

---

## 4. 类型复用

遵循蓝图 §5.5 "禁止重复造轮子"，本模块复用既有类型而非重定义：

| 本模块类型 | 来源 | 说明 |
|-----------|------|------|
| `IpProtocol` | `smoltcp::wire::IpProtocol` | `pub type` 直接别名，避免重复定义枚举 |
| `Ipv4Addr` | `tcpip/addr.rs` → `smoltcp::wire::Ipv4Address` | 等价于 `core::net::Ipv4Addr`，实现 `Ord` 可作 BTreeMap 键 |
| `Ipv4Cidr` | `tcpip/addr.rs` → `smoltcp::wire::Ipv4Cidr` | 提供 `contains_addr(&Ipv4Addr) -> bool` 用于源 CIDR 匹配 |
| `ipv4_addr` / `ipv4_cidr` | `tcpip/addr.rs` 构造辅助函数 | 测试与基准中统一使用 |

---

## 5. no_std 合规（蓝图 §43.1）

| 标准库用法 | 本模块替代 | 出现位置 |
|-----------|-----------|---------|
| `std::collections::HashMap` | `alloc::collections::BTreeMap` | ConnectionTracker、DdosProtector |
| `std::vec::Vec` | `alloc::vec::Vec` | Firewall 规则表、BenchmarkSuite |
| `std::string::String` | `alloc::string::String` | BenchmarkSuite 结果名 |
| `std::time::Instant` | `HalClock::now_ns()`（硬件侧） | 延迟测量当前为占位常量 |

模块顶层 `#![no_std]` 通过 crate `eneros-net` 的 `lib.rs` 继承，所有 `use` 均限定在 `alloc::` / `core::` / `smoltcp::` 命名空间内。

---

## 6. 内存预算声明（蓝图 §5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| Firewall 规则表 | ~4 KB | 64 条规则 × 64 字节（`Vec<FirewallRule>`） |
| ConnectionTracker | ~8 KB | 64 连接 × 128 字节（`BTreeMap<Ipv4Addr, ConnInfo>` + 元数据） |
| DdosProtector | ~4 KB | 64 IP × 64 字节（`BTreeMap<Ipv4Addr, SynInfo>` + 元数据） |
| BenchmarkSuite | ~1 KB | 累积结果列表，仅在测试 / 诊断时实例化 |
| **运行时总计** | **≤ 16 KB** | 不含 TCP 缓冲与 smoltcp 接口自身内存 |

> 上述预算对应储能场景 Edge Box 的典型配置（max_total ≤ 64，max_per_ip ≤ 5，tracked IP ≤ 64）。实际占用随 `max_total` 与攻击流量上限线性增长。

---

## 7. OOM 策略

当 `ConnectionTracker` / `DdosProtector` 因攻击流量导致 BTreeMap 接近预算上限时，按以下优先级降级（蓝图 §5.6 OOM 阈值 90% 触发）：

1. **缩减规则表大小**：动态移除防火墙中优先级最低的规则（如 `src_ip == None` 的 match-all 规则），降低 `Firewall::rules` 内存。
2. **关闭非关键连接**：调用 `ConnectionTracker::disconnect(ip)` 主动断开非关键 Agent 的长连接，回收 `ConnInfo` entry。
3. **激进驱逐攻击 IP**：在 `DdosProtector` 中对超阈值的 IP 调用 `reset()` 清空其 `SynInfo`，或直接清空整个 `syn_tracker`。
4. **降级到 L1 路径**：若网络栈不可用，Agent Runtime 切换到 Solver-only 路径（蓝图 L1 主路径），暂停 LLM 增强路径与远程通信，仅保留本地实时控制。
5. **冻结非关键 Agent**：触发 OOM handler（蓝图 §43.6），冻结非关键 Agent 释放其堆配额。

---

## 8. 性能目标

| 指标 | 目标 | 验证方式 |
|------|------|---------|
| 防火墙单次检查延迟 | < 1 μs（64 条规则下） | **需实机验证**：基于 `HalClock::now_ns()` 测量，主机侧 `BenchmarkSuite` 当前为占位常量 |
| 连接跟踪单次 `try_connect` 延迟 | < 2 μs | 同上 |
| 同时跟踪连接数 | ≥ 64 | 由 `ConnectionTracker::max_total` 配置，测试覆盖至 10000 |
| 同时跟踪 DDoS IP 数 | ≥ 64 | 由 `DdosProtector` BTreeMap 容量决定，无硬上限 |
| SYN 速率阈值精度 | 1 秒窗口内 ±1 | `rate_window_reset_allows_again` / `check_syn_window_reset_allows_again` 测试覆盖 |

> 主机测试已覆盖功能正确性（首匹配优先、连接上限、窗口重置、多 IP 独立等）。**真实延迟数据需在飞腾 / 鲲鹏 / QEMU 实机上接入硬件时钟后复测**。

---

## 9. 偏差声明

| 偏差项 | 蓝图原计划 | 实际实现 | 原因 |
|--------|-----------|---------|------|
| 容器选型 | `HashMap`（隐含） | `BTreeMap` | no_std 合规（蓝图 §43.1）—— `std::collections::HashMap` 依赖 `std`，`alloc::collections::BTreeMap` 提供 `Ord`-based 等价语义且无需哈希随机化 |
| 规则匹配字段 | 源 IP / 目的端口 / 协议三元组 | 仅源 IP CIDR | `dst_port` 与 `protocol` 字段已定义于 `FirewallRule` 但本版本 `match_rule` 不评估——避免在 smoltcp 未提供解析 helper 前做出错误匹配决策，留作后续版本扩展 |
| 延迟基准 | 真实 μs 级测量 | 占位常量 `LATENCY_US = 1` | `no_std` 无 `std::time::Instant`；硬件侧需接入 `HalClock::now_ns()`，列入 v0.30.x 后续子版本任务 |
| `RateLimit` 配置 | 跟踪器内部持有 | 仅作 POD 配置记录 | 跟踪器通过 `try_connect` 参数化接受阈值，避免在跟踪器实例化时硬编码速率策略，便于运行时调参 |
