# Checklist

> **D26 偏差声明**：以下 checklist 项（C11 / C15 / C18 / C20 等）中引用的 `impl Fn()` /
> `Vec<Duration>` / `Duration` 字段类型为 spec 原始设计。实际实现按 **D26 偏差**：
> 闭包参数改为 `impl FnMut() -> Result<(), ()>`（`TsnDriver::send` 要求 `&mut self`，
> 捕获 `&mut T` 的闭包只能实现 `FnMut`），`measure_round_trip` 改为
> `send: &mut impl FnMut()` 以允许同一闭包在 `run_burst` 多轮循环中被反复调用；
> `DelayStats` 字段类型为 `u64`（与 `clock_fn: fn() -> u64` 返回的纳秒时间戳对齐）；
> `LatencyProbe.results: Vec<u64>`（同上）。所有偏差已在
> [lib.rs D20~D26 表](../../../crates/protocols/tsn-time/src/lib.rs) 与
> [tsn-determinism-report.md §12](../../../docs/protocols/tsn-determinism-report.md)
> 中正式登记。

## Task 1: crate 版本号升级
- [x] C1: `crates/protocols/tsn-time/Cargo.toml` 中 `version = "0.81.0"`
- [x] C2: `crates/protocols/tsn-time/Cargo.toml` 中 `description` 更新为 v0.81.0 描述（含 "端到端时延探针" 字样）

## Task 2: driver_glue.rs — TSN 驱动抽象
- [x] C3: `TsnError` 枚举含 3 变体 `SendFailed` / `RecvFailed` / `NotInitialized`
- [x] C4: `TsnError` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C5: `TsnDriver` trait 定义 `send(&mut self, tc: TrafficClass, payload: &[u8]) -> Result<(), TsnError>` 与 `recv(&mut self) -> Result<alloc::vec::Vec<u8>, TsnError>`
- [x] C6: `MockTsnDriver` 含字段 `sent: Vec<(TrafficClass, Vec<u8>)>` / `recv_queue: Vec<Vec<u8>>` / `fail_send: bool` / `fail_recv: bool`
- [x] C7: `MockTsnDriver::new()` 初始化 `sent=Vec::new()` / `recv_queue=Vec::new()` / `fail_send=false` / `fail_recv=false`
- [x] C8: `MockTsnDriver::push_recv(data: Vec<u8>)` 将数据推入 `recv_queue`
- [x] C9: `MockTsnDriver` 实现 `TsnDriver::send` — `fail_send=true` 时返回 `Err(SendFailed)`；否则 `sent.push((tc, payload.to_vec()))` 返回 `Ok(())`
- [x] C10: `MockTsnDriver` 实现 `TsnDriver::recv` — `fail_recv=true` 时返回 `Err(RecvFailed)`；否则 `recv_queue.pop()` 返回 `Ok(data)` 或空时 `Err(RecvFailed)`
- [x] C11: `driver_send_closure<'a>(driver, tc, payload) -> impl Fn() -> Result<(), ()> + 'a` 函数存在且签名正确
- [x] C12: `driver_send_closure` 内部调用 `driver.send(tc, payload)` 并将 `Result<(), TsnError>` 映射为 `Result<(), ()>`
- [x] C13: `driver_glue.rs` 使用 `use alloc::vec::Vec;` 与 `use crate::tas::TrafficClass;`（no_std 合规）
- [x] C14: `driver_glue.rs` 无 `use std::*` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!`

## Task 3: latency_probe.rs — 时延探针与统计
- [x] C15: `DelayStats` 结构体含 7 字段 `min / max / mean / p99 / p999 / jitter: Duration` + `samples: u64`
- [x] C16: `DelayStats` 派生 `Debug, Clone, PartialEq, Eq, Default`
- [x] C17: `DelayStats::default()` 返回所有 `Duration::ZERO` + `samples: 0`
- [x] C18: `LatencyProbe` 结构体含字段 `sample_count: u32` / `results: Vec<Duration>` / `clock_fn: fn() -> u64` / `sleep_fn: fn(Duration)`
- [x] C19: `LatencyProbe::new(clock_fn, sleep_fn) -> Self` 初始化 `sample_count=0` / `results=Vec::new()`
- [x] C20: `measure_round_trip(send: impl Fn() -> Result<(), ()>) -> Result<Duration, ()>` — 先 clock_fn 记 start_ns，调 send，失败立即返回 Err(())，成功再 clock_fn 记 end_ns，返回 `Duration::from_nanos(end_ns.saturating_sub(start_ns))`
- [x] C21: `run_burst(count, interval, send)` — 循环 count 次，每次调用 measure_round_trip，Ok 则 push + sample_count+=1，最后调用 sleep_fn（无论成败）
- [x] C22: `run_burst(count=0, ...)` 返回 `DelayStats::default()`，sample_count 不变
- [x] C23: `compute_stats(&self) -> DelayStats` — 空结果返回 default；否则 sort 后计算 min/max/mean/p99/p999/jitter/samples
- [x] C24: `compute_stats` 的 p99 索引 `((n as f64 * 0.99) as usize).min(n as usize - 1)`（n 为 usize）
- [x] C25: `compute_stats` 的 p999 索引 `((n as f64 * 0.999) as usize).min(n as usize - 1)`
- [x] C26: `compute_stats` 的 mean `sorted.iter().sum::<Duration>() / n as u32`
- [x] C27: `compute_stats` 的 jitter `max - min`
- [x] C28: `run(duration, send)` — clock_fn 记 start_ns，deadline = start_ns + duration.as_nanos() as u64，循环至 clock_fn() >= deadline，每次 measure_round_trip 成功则 push + sample_count+=1，不调用 sleep_fn
- [x] C29: `measure_e2e(samples, send)` 委托 `run_burst(samples, Duration::from_millis(1), send)`
- [x] C30: `measure_under_load(samples, interval, background_load, send)` — 每轮先调 background_load()，再 measure_round_trip，再 sleep_fn
- [x] C31: `latency_probe.rs` 使用 `use core::time::Duration;` + `use alloc::vec::Vec;`（no_std 合规）
- [x] C32: `latency_probe.rs` 无 `use std::*` / 无 `panic!` / 无 `unsafe` / 无 `todo!` / 无 `unimplemented!` / 无 `Instant` / 无 `eneros_time` 依赖

## Task 4: lib.rs — 模块声明 + 导出 + 测试 + 偏差
- [x] C33: `pub mod driver_glue;` 出现且按字母序位于 `config_loader` 与 `gptp` 之间
- [x] C34: `pub mod latency_probe;` 出现且按字母序位于 `gptp` 与 `port` 之间
- [x] C35: `pub use driver_glue::{driver_send_closure, MockTsnDriver, TsnDriver, TsnError};` 导出
- [x] C36: `pub use latency_probe::{DelayStats, LatencyProbe};` 导出
- [x] C37: 顶部模块文档注释追加 v0.81.0 扩展段落（描述 TSN 驱动抽象 + 时延探针，无真实网络 I/O）
- [x] C38: 追加 D20~D25 偏差声明段落（保留 v0.79.0 D1~D14 / v0.80.0 D15~D19 不变）
- [x] C39: v0.79.0 T1~T25 测试完全保留（Surgical Changes 验证：测试函数体未动）
- [x] C40: v0.80.0 T26~T55 测试完全保留（Surgical Changes 验证：测试函数体未动）
- [x] C41: 新增 T56 测试 — `DelayStats::default()` 返回全零
- [x] C42: 新增 T57 测试 — `DelayStats` 字段可读访问
- [x] C43: 新增 T58 测试 — `LatencyProbe::new` 初始化 sample_count=0 / results 空
- [x] C44: 新增 T59 测试 — `measure_round_trip` 成功返回 Duration（clock_fn 差值）
- [x] C45: 新增 T60 测试 — `measure_round_trip` send 失败返回 Err(())，未调第二次 clock_fn
- [x] C46: 新增 T61 测试 — `run_burst(0, ...)` 返回 default DelayStats
- [x] C47: 新增 T62 测试 — `run_burst(5, ...)` 成功采样 sample_count=5 / results.len()=5 / sleep_fn 调用 5 次
- [x] C48: 新增 T63 测试 — `run_burst` 部分 send 失败时 sample_count 与 results 长度匹配成功数，sleep_fn 仍调用 count 次
- [x] C49: 新增 T64 测试 — `compute_stats` 空结果返回 default
- [x] C50: 新增 T65 测试 — `compute_stats` 单样本 min=max=mean=p99=p999，jitter=0
- [x] C51: 新增 T66 测试 — `compute_stats` 多样本（5 个 100µs~500µs）min/max/mean/jitter 正确
- [x] C52: 新增 T67 测试 — `compute_stats` p99/p999 索引计算正确（5 样本时均为 sorted[4]）
- [x] C53: 新增 T68 测试 — `compute_stats` samples 字段等于 results.len()
- [x] C54: 新增 T69 测试 — `run(duration, ...)` 时长到期停止（clock_fn 控制循环次数）
- [x] C55: 新增 T70 测试 — `measure_e2e(3, ...)` 等价于 `run_burst(3, 1ms, ...)`
- [x] C56: 新增 T71 测试 — `measure_under_load` background_load 闭包被调用 N 次
- [x] C57: 新增 T72 测试 — `MockTsnDriver::new` 初始化 fail_send=false / fail_recv=false / 队列空
- [x] C58: 新增 T73 测试 — `MockTsnDriver::send` 记录 (tc, payload)，返回 Ok(())
- [x] C59: 新增 T74 测试 — `MockTsnDriver::recv` 返回 push_recv 的数据
- [x] C60: 新增 T75 测试 — `MockTsnDriver::send` fail_send=true 返回 Err(SendFailed)
- [x] C61: 新增 T76 测试 — `MockTsnDriver::recv` 空队列返回 Err(RecvFailed)
- [x] C62: 新增 T77 测试 — `driver_send_closure` 成功路径返回 Ok(())，driver.sent 有记录
- [x] C63: 新增 T78 测试 — `driver_send_closure` 失败路径返回 Err(())，driver.sent 无记录
- [x] C64: 新增 T79 测试 — 端到端集成：LatencyProbe + MockTsnDriver + driver_send_closure 完成 3 次采样
- [x] C65: 新增 T80 测试 — `measure_under_load` 下 send 闭包失败时样本不计入但 background_load 仍调用
- [x] C66: 新增 T81 测试 — `compute_stats` p99 边界（n=100 时索引为 99）
- [x] C67: 新增 T82 测试 — `compute_stats` p999 边界（n=1000 时索引为 999）
- [x] C68: 新增 T83 测试 — `LatencyProbe::run` duration=Duration::ZERO 立即返回（不进入循环）
- [x] C69: 新增 T84 测试 — `DelayStats::default() == DelayStats::default()`（PartialEq 一致性）
- [x] C70: 测试模块使用 `use core::sync::atomic::{AtomicU64, Ordering};` 静态计数器模拟 clock_fn
- [x] C71: 测试模块使用 `static mut SLEEP_COUNT: u32` 或 `AtomicU32` 计数 sleep_fn 调用次数
- [x] C72: lib.rs 无 `use std::*` / 无 `panic!` / 无 `unsafe`（除可能存在的 test 静态变量 `unsafe` 块，需有安全论证）

## Task 5: configs/latency_probe.toml
- [x] C73: 文件位于 `configs/latency_probe.toml`
- [x] C74: TOML 模板含 `sample_count: u32` / `interval_us: u64` / `burst_count: u32` / `duration_us: u64` 字段
- [x] C75: 含注释说明各字段用途（中文注释可接受，与 v0.79.0 gptp.toml / v0.80.0 tas.toml 风格一致）

## Task 6: docs/protocols/tsn-determinism-report.md
- [x] C76: 文件位于 `docs/protocols/tsn-determinism-report.md`（非 `docs/phase2/`，符合 D21）
- [x] C77: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
- [x] C78: 至少 1 个 Mermaid 图（时延测量 sequence diagram，蓝图 §4.3 风格）
- [x] C79: 至少 1 个 Mermaid 图（compute_stats 流程图或测量路径决策图）
- [x] C80: D20~D25 偏差声明表完整
- [x] C81: 引用 v0.79.0 gPTP 时间同步 + v0.80.0 TAS 调度作为前置依赖
- [x] C82: 包含性能目标说明（TC3 p99 < 2ms / p999 < 5ms / 抖动 < 1ms，但标注为"硬件集成阶段验收，本版本仅算法骨架"）

## Task 7: 版本同步根目录文件
- [x] C83: 根 `Cargo.toml` 顶层 `version = "0.81.0"`
- [x] C84: 根 `Cargo.toml` workspace members 列表未变化（`"crates/protocols/tsn-time"` 已存在）
- [x] C85: `Makefile` 中 `# Version: v0.81.0` 与 `VERSION := 0.81.0`
- [x] C86: `.github/workflows/ci.yml` 中 `# Version: v0.81.0`
- [x] C87: `ci/src/gate.rs` clippy 段注释含 `eneros-tsn-time v0.81.0` 与新增类型列表（`DelayStats` / `LatencyProbe` / `TsnDriver` / `MockTsnDriver` / `TsnError` / `driver_send_closure`）
- [x] C88: `ci/src/gate.rs` test 段注释同步更新类型列表

## Task 8: 构建校验（§2.4.2）
- [x] C89: `cargo metadata --format-version 1` 成功（无 workspace 解析错误）
- [x] C90: `cargo test -p eneros-tsn-time` 全部通过（T1~T84 = 84 tests + 1 doctest，0 failures）
- [x] C91: `cargo build -p eneros-tsn-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 退出码 0
- [x] C92: `cargo fmt -p eneros-tsn-time -- --check` 退出码 0
- [x] C93: `cargo clippy -p eneros-tsn-time --all-targets -- -D warnings` 无 warning，退出码 0
- [x] C94: `cargo deny check licenses bans sources` 通过（无新依赖引入，应继续 pass）
- [x] C95: 回归 — `cargo test -p eneros-agent-bus-dds` 仍通过 63 tests + 1 doctest（无回归）
- [x] C96: 回归 — v0.79.0/v0.80.0 既有 T1~T55 测试函数体未被修改（通过 git diff 检查）

## 总体校验
- [x] C97: 无根目录新 crate（除 `ci/` 外）
- [x] C98: 无 `docs/` 根目录平面化文档（新文档在 `docs/protocols/` 下）
- [x] C99: `.gitignore` 未需更新（无新文件类型）
- [x] C100: `git status` 无 `target/` / `*.elf` / `*.bin` / `*.dtb` / IDE 缓存被追踪
- [x] C101: 提交信息遵循 Conventional Commits（如 `feat(protocols/tsn-time): v0.81.0 实现 TSN 驱动抽象与端到端时延探针`）
- [x] C102: ADR 决策未被违反（未引入研究特性、未自研已有开源替代组件、未超出 v1.0.0 范围）
- [x] C103: no_std 合规性：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` 保留
- [x] C104: 内存预算：本 crate 为协议层纯算法骨架，无运行时分配增长（除 `Vec::push` 既有模式）
- [x] C105: SBOM 未变化（无新第三方依赖）
- [x] C106: 文档同步：v0.79.0 / v0.80.0 历史偏差声明保留，v0.81.0 新增 D20~D25 段落
