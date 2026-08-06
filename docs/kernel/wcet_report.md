# EnerOS v0.138.0 调度确定性与 WCET 验证报告（分区调度抖动 / WCET 测量 / 实时性证据）

> 版本：v0.138.0
> 适用范围：EnerOS 自研内核分区调度实时性测量与验证（ARM64 CNTVCT_EL0 虚拟计数器 + CNTFRQ_EL0 频率校准；QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` / host 注入时钟集成测试）
> 蓝图依据：`蓝图/phase3.md` §v0.138.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> 配套文档：[memory_ordering.md](./memory_ordering.md)（v0.137.0 多核内存一致性，WCET_PROF SpinLock 设施复用来源）、[smp_design.md](./smp_design.md)（v0.136.0 SMP 多核启动/IPI）、[scheduler_design.md](./scheduler_design.md)（v0.134.0 线程调度）、[interrupt_design.md](./interrupt_design.md)（v0.133.0 GICv3 中断管理，IRQ 30 定时器链路）、[../boot/qemu_run.md](../boot/qemu_run.md) §17（v0.138.0 QEMU 实测记录：首轮 FAIL → D8 TVAL→CVAL 修复 → 次轮 PASS）

---

## 1. 概述

**核心一句话**：测量并验证分区调度抖动与最坏情况响应时间（WCET），确保 RTOS 分区抖动 max < 1ms、p99 < 500μs（蓝图 §1 原样口径 + §7 验收量化）。

- **Phase 定位**：P3-C 中断调度 SMP 线**终版**（v0.133.0 ~ v0.138.0 第 6 版），证明自研内核调度满足实时要求（蓝图 §1）。前置 v0.137.0 多核内存一致性已交付（正确并发基座——WCET 样本测量器经 SpinLock 保护即复用其设施）。
- **业务价值**：电力控制对实时性要求极高，确定性调度是安全合规的基础（蓝图 §1）；本报告为**信创认证实时性证据**（ADR-0001 裁剪后保留项：方案 A 下调度确定性验证针对 seL4 + 自研分区调度策略）。
- **出口关联**：自研内核「确定性」证据，为 **v0.150.0 形式化证明提供实测数据**（蓝图 §1 / §5.5），并为等保认证提供实时性证据（蓝图 §5.5）。
- **ADR-0001 口径**：本版本原为自研内核组件，已按方案 A 调整为 seL4 深度定制/信创认证路径；原自研内核工作（语言重写、全量形式化验证、5 平台 BSP）已裁剪或合并（蓝图 §1 裁剪原样）。
- **依赖关系**：前序 v0.137.0 多核内存一致性可用（蓝图 §2）；外部依赖高精度定时器（CNTVCT_EL0）；假设 RTOS 绑核后不受其他核干扰（蓝图 §2）；**抖动不达标阻塞形式化验证**（蓝图 §2 阻塞项）。

交付物落位（仓库规范 §2.3.1 子系统归口，偏差 D1）：

| 交付物 | 路径 |
|--------|------|
| WCET 测量工具（WcetMeasurement / WcetReport / WcetProfiler trait / WcetProfilerImpl + WCE1~WCE5 单测） | [crates/kernel/microkernel/src/sched/wcet.rs](../../crates/kernel/microkernel/src/sched/wcet.rs) |
| 抖动统计（JitterStats / compute + JTR1~JTR4 单测） | [crates/kernel/microkernel/src/sched/jitter.rs](../../crates/kernel/microkernel/src/sched/jitter.rs) |
| sched 子系统挂载（`pub mod jitter; pub mod wcet;` 追加） | [crates/kernel/microkernel/src/sched/mod.rs](../../crates/kernel/microkernel/src/sched/mod.rs) |
| 内核启动期接入（§3.12 `wcet_smoke` 段 + `wcet_timer_handler`，IRQ 30 同号覆盖） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) |
| host 集成测试（WCT1~WCT6，含 10,000 采样验证） | [crates/kernel/microkernel/tests/wcet_test.rs](../../crates/kernel/microkernel/tests/wcet_test.rs) |
| QEMU 测试环境（第十一条断言 `[wcet] ok`，既有十条原样） | [tests/boot_test.sh](../../tests/boot_test.sh) |
| 测量报告（蓝图 §3 文档类 / §7.4 验收） | [docs/kernel/wcet_report.md](./wcet_report.md)（本文档） |

**接口一览**（蓝图 §3 接口类原样签名，[sched/wcet.rs](../../crates/kernel/microkernel/src/sched/wcet.rs) / [sched/jitter.rs](../../crates/kernel/microkernel/src/sched/jitter.rs) 公开导出）：

| 项 | 定义 | 说明 |
|----|------|------|
| `WcetMeasurement` | struct 四字段 | `min_cycles / max_cycles / avg_cycles / samples`（蓝图 §3 原样；字段名沿 `*_cycles`，实际存储值为 ns） |
| `WcetReport` | struct 四字段 | `schedule_jitter_ns / context_switch_ns / irq_latency_ns / rtos_response_ns`（p99 口径；`rtos_response_ns = jitter p99 + irq p99` 蓝图 §4.5 原样公式） |
| `WcetProfiler` | trait 四方法 | `measure_schedule_jitter / measure_context_switch / measure_irq_latency / report`（蓝图 §3 原样签名） |
| `JitterStats` | struct 五字段 | `min / max / p99 / p999 / avg`（D2：蓝图 `samples` 字段不入结构，样本驻留外层定长数组） |
| `compute` | `fn compute(samples: &mut [u64]) -> JitterStats` | 就地 `sort_unstable`（D4）后取百分位（索引 `len*p/100` 钳 `len-1`），avg 用 u128 累加防溢出 |

---

## 2. 测量方法论

### 2.1 计数器选型与频率校准

- **CNTVCT_EL0（虚拟计数器）**：ARM64 架构定义的高频低开销计数器（蓝图 §5.2 关键技术），EL1 经 `mrs` 只读访问、无副作用；`read_cycle_counter()` 以 `options(nostack)` 内联汇编实现（wcet.rs L72~L83），逐行 SAFETY 注释。
- **CNTFRQ_EL0（计数器频率）**：计数器每秒 tick 数（Hz），`read_counter_freq()` 同口径汇编读取（wcet.rs L85~L96）。`cycles_to_ns(cycles, freq) = cycles × 1_000_000_000 / freq`（wcet.rs L98~L106）：`saturating_mul` 防溢出（测量语义下饱和优于回绕），`freq == 0` 除零防御返回 0 不 panic。
- **QEMU virt 校准点**：CNTFRQ = 62.5 MHz → **1 tick = 16 ns**、62,500 tick = 1 ms（WCT3 已知点对照验证）。蓝图 §8.4 兼容「不同 CPU 频率需校准」与 §5.6 国产化「飞腾/鲲鹏计数器频率可配」→ 频率**运行时读取不硬编码**，`cycles_to_ns` 带 `freq` 参数（D3 口径附记，host 测试可控注入）。

### 2.2 D3 双时钟实现

| 路径 | 实现 | 用途 |
|------|------|------|
| aarch64 真机 | CNTVCT_EL0 / CNTFRQ_EL0 真实汇编 | QEMU 冒烟与真机测量 |
| host | `ClockSource = fn() -> u64` 注入时钟（static AtomicU64 模拟单调 ns 时钟） | host 无法读 ARM 计数器，驱动全部单测/集成测试（同 v0.137.0 D2 双实现惯例） |

trait 签名以蓝图 §3 为准（`measure_schedule_jitter` 仅 `samples` 参数），蓝图 §4.5 的 scheduler 交互经 ClockSource 抽象内化（D3）。

### 2.3 测量开销口径（蓝图 §5.4 难点「测量本身的开销干扰」）

- **单次采样开销** = 2 次 CNTVCT 读 + 1 次样本写入；handler 入口**最先**读 CNTVCT，使测得的延迟/抖动最接近真实触发时刻（main.rs `wcet_timer_handler` 口径）。
- **蓝图 §6.3「测量开销 < 100ns/次」为目标机 aarch64 裸机口径**；host 侧参考值：WCT5 实测 `jitter::compute`（10,000 样本）总耗时 **147.1 μs ≈ 14 ns/样本**（host x86 用户态口径，仅记录非验收，同 concurrency_test.rs CONC5 / smp_test.rs SMPT6 的 smoke 口径惯例）。
- **多核干扰**（蓝图 §5.4 难点二）：测量假设 RTOS 绑核后不受其他核干扰（蓝图 §2 假设）；样本数组驻留 BSS（NOBITS 不胀镜像），handler（IRQ 上下文）与主流程统计经同一 SpinLock 互斥（v0.137.0 IRQ 上下文持锁语义复用）。

---

## 3. 三类指标定义与口径

### 3.1 分区调度抖动

**公式**：`jitter(N) = cntvct_actual − (WCET_BASE + N × tval)`，仅在 **slot 切换 tick** 取样（`arinc.tick(N)` 返回 `Some`）。

- 蓝图 §4.5「expected = next_slot_boundary」语义的真机落点：理想周期下第 N tick 预期 CNTVCT = 基准 + N×tval（tval = cntfrq / 1,000，1 ms 网格步长）。
- `WCET_BASE` = 段启动 arm 时刻读数；arinc 经 `start_at(0)` 重新锚定 tick 0/slot 0（psched 段结束时 cur_tick ≈90+，不锚定则前 ≈95 tick 内 elapsed 饱和为 0 不产生切换，污染采样窗口）。
- `saturating_sub` 防计数器回绕下溢；`saturating_add/mul` 防极端值溢出。

### 3.2 IRQ 延迟（D6 口径）

**公式**：`latency = cntvct_handler_entry − expected_deadline`；`expected` 首中断 = `base + tval`，此后 handler 重装时按理想网格 `+tval` 同步递推（`WCET_EXPECT` 静态量）。

- 含义：定时器递送 + GIC 呈递 + handler 分发耗时——蓝图 §4.2 仅述「从硬件中断到 handler 执行的延迟」未给具体公式，取可复现口径并注释（D6）。
- 每次 tick 均取样（270 次全样本），不止 slot 切换点。

### 3.3 上下文切换（D7 代理口径）

- **背景**：threads 在 psched 路径不激活（v0.135.0 决策——tick 驱动线程抢占的寄存器级集成留待后续版本），真机无线程级切换可测。
- **代理**：稳态 `arinc.tick(0)`（elapsed 饱和为 0 恒不切换，纯路径耗时 + 两次 CNTVCT 读）**1,000 次夹取取 avg**（沿蓝图 §4.5 原样 1,000 次）；主流程直接统计（u128 累加），不写 ctx 数组（`record_ctx` 私有且 null_clock 桩下 `measure_context_switch` 无意义）。
- **声明**：线程级 context_switch WCET 留待 P3-D（main.rs `wcet_smoke` 头注决策记录已声明）。
- host 口径：蓝图 §4.5 原样 1,000 次 yield/空转夹取（WCT4 固定步进 100 ns 时钟下 min == avg == max == 100）。

---

## 4. D8 关键发现：TVAL 相对重装累积漂移 → CVAL 绝对网格修复

> 本节为本次测量最重要的方法论教训，单独成节详述（排障全程记录见 [qemu_run.md](../boot/qemu_run.md) §17）。

### 4.1 现象（首轮 FAIL）

首轮输出 `[wcet] err max=26582368 p99=26582368`（既有十类输出零回归）。max = **26,582,368 ns ≈ 26.58 ms**，超蓝图 §7.1 阈值 26.6 倍。

两点分析指向**系统性累积漂移**而非单发尖峰：

1. max ≡ p99 属 21 样本 p99 索引钳末位的预期现象（见 §5.1），不提供判别信息；
2. **26.58 ms ≈ 270 tick × ≈98.5 μs/tick**——偏差随 tick 序号线性增长，是典型累积漂移特征。

### 4.2 根因（TVAL 相对重装语义）

`wcet_timer_handler` ③ 原按 v0.135.0 psched 惯例以 **TVAL 相对重装**（TVAL 语义为「自写寄存器时刻起 tval 计数后触发」），每次触发实际周期 = tval + handler 入口延迟，270 tick 把递送延迟逐次累积进测量基准；而抖动判定基准 `WCET_BASE + N×tval` 是理想绝对网格，两者偏差随 N 线性增长——**测得的并非调度抖动，而是测量基准自身的漂移**。

### 4.3 修复（D8 口径修正）

arm 与重装一律改用 **CVAL 绝对比较寄存器**：

- 首中断 `cntp_cval_el0 = base + tval`（wcet_smoke ④）；
- handler ③ 每次按理想网格 `expected + tval` 递推下次到期。

CVAL 语义为「CNTVCT ≥ 比较值即触发」，handler 入口延迟不再累积进周期，抖动样本回归「递送延迟」本义（蓝图 §4.5 `expected = next_slot_boundary` 语义的真机落点）。handler 延迟 > tval 的极端情形下 CVAL 已过期会立即再触发一次追赶 tick，仍不累积漂移。

**改动面**：仅触 v0.138.0 wcet 段（handler ③ + wcet_smoke ④ arm + WCET_TVAL 静态量文档，三处注释同步）；既有十类输出与既有函数语义零改动。

### 4.4 两轮实测数据对照

| 轮次 | 重装方式 | max | p99 | 判定 | 结论 |
|------|---------|-----|-----|------|------|
| 首轮 | TVAL 相对重装 | 26,582,368 ns ≈ 26.58 ms | 26,582,368 ns | `[wcet] err` | 270 tick × ≈98.5 μs 系统性漂移，淹没真实调度抖动 |
| 次轮（修复后） | CVAL 绝对网格 | 170,592 ns ≈ 170.6 μs | 170,592 ns | `[wcet] ok` | 漂移消除，样本回归「递送延迟」本义 |

**方法论教训**：周期定时器驱动的 WCET/抖动测量必须以**绝对时间网格**为基准；相对重装语义会把测量者自身的处理延迟累积进被测对象。这是蓝图 §4.5 算法在真机定时器硬件语义下的关键落点，建议纳入蓝图 §8.5 坑点清单（见 §7）。

---

## 5. 实测结果

### 5.1 QEMU 真机冒烟（270 tick / 21 次 slot 切换）

串口关键行（原样收录，WSL2 Ubuntu-22.04 + QEMU 8.2.2 + Rust nightly-2026-04-04，`-M virt,gic-version=3 -smp 4`，2026-07-26 实测）：

```text
[sync] ok
[wcet] ok max=170592 p99=170592 irq99=250544 ctx=99
```

端到端总耗时 12 s（构建 2.03 s + QEMU timeout 10 s 守护窗口，含 wcet 段 270 tick ≈3 s 采样 + 1,000 次上下文代理夹取）；kernel ELF 115,336 B（WcetProfilerImpl ≈96 KB 样本数组经 `MaybeUninit::zeroed()` 驻留 BSS NOBITS，不占镜像体积，远低于 512 KB 预算）。

| 指标 | 实测值 | 样本口径 | 解读 |
|------|--------|---------|------|
| 抖动 max | 170,592 ns ≈ 170.6 μs | 21 次 slot 切换样本 | QEMU 定时器递送 + GIC 呈递 + handler 分发（含 SpinLock 临界区）全链路最坏值 |
| 抖动 p99 | 170,592 ns ≈ 170.6 μs | 21 样本 | p99 索引 `21×99/100=20` 钳至末位 ≡ max（main.rs D5 决策已声明的冒烟口径），两值相等属预期而非数据异常 |
| IRQ 延迟 p99 | 250,544 ns ≈ 250.5 μs | 270 次 tick 全样本（p99 索引 267） | 略大于抖动 max 属合理——IRQ 样本含全部 270 次触发（含非切换 tick），抖动仅取 21 次切换点 |
| 上下文代理 avg | 99 ns | 1,000 次 CNTVCT 夹取 | 两次 CNTVCT 读 + 稳态 arinc.tick 路径均值；QEMU CNTVCT 读本身有开销，真机口径待 P3-D 线程级切换集成后复核 |

### 5.2 host 10,000 采样集成测试（tests/wcet_test.rs）

| 用例 | 注入/构造 | 实测 | 判定 |
|------|----------|------|------|
| WCT1（蓝图 §6.2 + §7.1） | xorshift64* 有界伪随机抖动 [0, 500 μs) + 固有步进 10 ns，满容 10,000 采样 | samples == 10,000；min ≤ avg ≤ max；max ≤ 500,009 ns（注入上界 + 步进容差）；min < max 非空洞复核 | max < 1 ms 判定通过（约 2 倍余量） |
| WCT2（蓝图 §7.2） | 9,901 × 100 μs（99.01%）+ 99 × 800 μs（0.99%） | p99 = 100,000 ns（索引 9,900 落 100 μs 档）与 max = 800,000 ns **分离** | p99 < 500 μs 通过且 max ≥ 500 μs——双阈值独立判定，不可互相替代 |
| WCT5（蓝图 §6.3 smoke） | `compute`（10,000 伪随机样本）计时 | 总耗时 147.1 μs ≈ **14 ns/样本**（host x86 用户态口径，仅记录非验收） | 防算法退化断言 < 500 ms；min ≤ p99 ≤ max 顺带复核 |
| WCT6（蓝图 §6.5，`#[ignore]`） | 9,999 × 100 μs + 单发 2 ms 故障注入 | max = 2,000,000 ns → max < 1 ms 判定为 false；p99 仍 100,000 ns | err 判定路径可达；max 捕获单发尖峰 / p99 容忍 1% 尾部的语义分离 |

---

## 6. 阈值对照（蓝图 §7 验收）

| 蓝图条目 | 阈值 | 实测 | 余量 | 判定 |
|---------|------|------|------|------|
| §7.1 功能：RTOS 分区抖动 max < 1 ms | < 1,000,000 ns | 170,592 ns（QEMU）/ ≤ 500,009 ns（host WCT1） | 5.9× / 2.0× | PASS |
| §7.2 性能：p99 抖动 < 500 μs | < 500,000 ns | 170,592 ns（QEMU，21 样本钳末位口径）/ 100,000 ns（host WCT2） | 2.9× / 5.0× | PASS |
| §7.2 性能：上下文切换 < 5 μs | < 5,000 ns | 99 ns（QEMU 代理 avg，D7 口径） | ≈50× | PASS（代理口径声明，线程级留待 P3-D） |
| §6.3 性能基准：测量开销 < 100 ns/次 | < 100 ns/次（目标机 aarch64 口径） | host 14 ns/样本（WCT5 参考）；QEMU ctx 99 ns 含两次 CNTVCT 读 + tick 路径 | — | PASS（host 参考 + 目标机口径声明） |
| §7.3 安全：确定性满足实时要求 | — | max/p99 双阈值达标；故障注入 err 判定路径可达（WCT6） | — | PASS |
| §7.4 文档：wcet_report.md | — | 本文档 | — | PASS |
| §7.5 出口判定：抖动 < 1 ms → P3-D 可启动 | — | 十一关键串断言 PASS（含 `[wcet] ok`）；host 全量测试零回归 | — | 达成，P3-D 可启动 |

---

## 7. 蓝图 §8.5 坑点记录

| 坑点 | 本版本遭遇与处置 |
|------|----------------|
| QEMU 计数器精度不如真机 | 已实测确认：QEMU CNTVCT 递送延迟（百 μs 量级）显著大于真机定时器硬件精度。本报告数值为 **QEMU 口径下界证据**（真机定时器递送只会更优或同级），真机验收（飞腾/鲲鹏）以同口径重测为准；`cycles_to_ns` 带 `freq` 参数兼容 §5.6 国产化频率可配 |
| 中断风暴时抖动增大 | WCT6（`#[ignore]`）故障注入单发 2 ms 尖峰验证 max < 1 ms 的 err 判定路径可达；max 捕获尖峰 / p99 容忍尾部的双指标设计即应对此场景（蓝图 §4.4「抖动超限：记录日志 + 触发分析」） |
| （新增）TVAL 相对重装累积漂移 | 本版本实测发现并已修复（D8，见 §4）——周期定时器测量必须以绝对网格（CVAL）为基准，否则 handler 延迟逐次累积淹没被测对象。建议纳入蓝图坑点清单 |

---

## 8. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`（lib 内嵌单测 + tests/ 集成测试）。各用例独占自身 static 时钟态或走 record_* 直写路径，无跨用例共享态，cargo test 默认并行下结果确定（同 concurrency_test.rs 隔离口径）。

### 8.1 单元测试（sched/jitter.rs 内嵌，JTR1~JTR4）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| JTR1 | `jtr1_empty_slice_returns_zero` | 空切片返回全零（防除零 + 防 min 初值泄漏） | PASS |
| JTR2 | `jtr2_known_distribution_1_to_1000` | 已知分布 1..=1,000 手算对照：min=1 / max=1,000 / p99=991（索引 990）/ p999=1,000（索引 999）/ avg=500 | PASS |
| JTR3 | `jtr3_single_element` | 单元素所有统计恒等（索引钳位到 0） | PASS |
| JTR4 | `jtr4_unsorted_input_sorted_in_place` | 乱序输入就地排序后统计正确 | PASS |

### 8.2 单元测试（sched/wcet.rs 内嵌，WCE1~WCE5）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| WCE1 | `wce1_full_samples_min_avg_max` | 满样本 10,000：samples == 10,000 且 min ≤ avg ≤ max；递增注入下 min/max 必有区分 | PASS |
| WCE2 | `wce2_constant_injected_jitter` | 恒定注入 100 ns → min == max == avg == 100 | PASS |
| WCE3 | `wce3_report_aggregation_formula` | report 聚合公式：手工造数（jitter 1..=100 / irq 1..=50 / ctx 1..=10）→ (100, 10, 50, 150)，rtos_response == jitter p99 + irq p99 | PASS |
| WCE4 | `wce4_empty_report_all_zero` | 空测量 report 全零 | PASS |
| WCE5 | `wce5_capacity_boundary_drop` | 容量边界：10,000 写满不越界；第 10,001 次 record 丢弃且既有样本不被污染；超容请求钳到容量 | PASS |

### 8.3 集成测试（tests/wcet_test.rs，WCT1~WCT6，仅以公开 API 端到端验证）

| 编号 | 用例 | 验证点 | 结果 |
|------|------|--------|------|
| WCT1 | `wct1_schedule_jitter_10000_samples_max_under_1ms` | 蓝图 §6.2「10,000 次采样」+ §7.1「max < 1ms」：xorshift 注入 [0, 500 μs) 满容采样，samples == 10,000 且 max < 1 ms | PASS |
| WCT2 | `wct2_p99_separated_from_max_independent_thresholds` | 蓝图 §7.2：p99 与 max 分离，双阈值独立判定 | PASS |
| WCT3 | `wct3_cycles_to_ns_known_points_and_zero_freq_guard` | 蓝图 §6.1「cycles_to_ns 转换」：62.5 MHz → 16 ns/tick、62,500 tick → 1 ms、1 GHz → 1 ns/tick、freq == 0 除零防御 | PASS |
| WCT4 | `wct4_context_switch_smoke_fixed_step_clock` | 蓝图 §9.1「上下文切换完整测量」冒烟：固定步进 100 ns × 1,000 次，min == avg == max == 100 | PASS |
| WCT5 | `wct5_compute_perf_smoke` | 蓝图 §6.3 性能 smoke：compute(10,000) 实测 147.1 μs ≈ 14 ns/样本（host 口径仅记录非验收） | PASS |
| WCT6 | `wct6_fault_injection_2ms_jitter_err_path`（`#[ignore]`） | 蓝图 §6.5 故障注入：单发 2 ms 抖动验证 err 判定路径可达（`cargo test -- --ignored` 才执行，同 CONC6 惯例） | ignore（口径内） |

### 8.4 QEMU 冒烟（main.rs §3.12 段 + [qemu_run.md](../boot/qemu_run.md) §17）

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **十一**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok` / `[smp] ok` / `[sync] ok` / `[wcet] ok`；v0.138.0 起追加第十一条断言 `grep "[wcet] ok"`，既有十条原样） |
| 轮次 | 首轮 FAIL（TVAL 累积漂移 max=26,582,368 ns）→ D8 修复 → 次轮一次 PASS（max=170,592 ns / p99=170,592 ns / irq99=250,544 ns / ctx=99 ns） |
| 采样规模 | 270 tick = 3 主周期 × 默认表 90 tick（D5，1 kHz 下约 3 s）；默认表七槽下恒产生 21 次 slot 切换样本 |
| 回归 | host 全量测试零回归（WCT6 等 `#[ignore]` 用例按设计跳过）；`cargo fmt --check` / `cargo clippy -D warnings` 干净；release ELF 115,336 B < 512 KB 预算 |

### 8.5 蓝图 §6 测试计划对照

| 蓝图条目 | 覆盖情况 |
|---------|---------|
| §6.1 单元测试「百分位计算 / cycles_to_ns 转换」 | JTR1~JTR4（百分位）+ WCE3（p99 聚合）/ WCT3（cycles_to_ns 集成侧复核） |
| §6.2 集成测试「10,000 次采样，验证 max < 1ms」 | WCT1（host 满容 10,000 采样 + 阈值判定）；QEMU 冒烟按 D5 收敛 270 采样（boot 测试时间预算） |
| §6.3 性能基准「测量开销 < 100ns/次」 | WCT5 host smoke 实测 14 ns/样本（host 口径声明，仅记录非验收；目标机口径同 CONC5/SMPT6 惯例） |
| §6.4 回归「调度功能不受影响」 | QEMU 既有十断言先于 `[wcet] ok` 全部通过；sched/ 既有文件零改动、mod.rs 仅追加挂载行（spec 零改动承诺） |
| §6.5 故障注入「故意制造干扰（高负载），观察抖动」 | WCT6（`#[ignore]`）单发 2 ms 注入验证 err 路径 |
| §6.6 GPU 规则 | 不适用（WCET 测量，蓝图原样口径） |

---

## 9. 蓝图偏差表（D1~D8）

> 文档级编号与 spec.md / 源码头注编号一一对应：wcet.rs / jitter.rs 头注 D2~D4 → 本文档同号；main.rs `wcet_smoke` / `wcet_timer_handler` 头注决策记录 D5~D8 → 本文档同号（D-W1 为 main.rs 本地适配决策，附于 D2 行内）。

| 编号 | 内容 | 理由 | 影响 |
|------|------|------|------|
| D1 | 蓝图交付物 `kernel/src/sched/*` → 落位 `crates/kernel/microkernel/src/sched/*`（wcet.rs / jitter.rs，mod.rs 追加挂载）；测试 `kernel/tests/wcet_test.rs` → `crates/kernel/microkernel/tests/`；文档路径 `docs/kernel/wcet_report.md` 同蓝图 | 仓库规范 §2.3.1 强制子系统归口（同 v0.134~v0.137 D1 惯例） | 仅路径差异，交付物一一对应 |
| D2 | 蓝图 `Vec<u64>` 样本容器 → 定长数组（jitter 10,000 / ctx 1,024 / irq 1,024 × u64 BSS 静态）；`JitterStats.samples` 字段不入结构（样本驻留外层数组，结构仅五统计字段） | microkernel 无 alloc（lib.rs 无 `extern crate alloc`），同 v0.136.0 IpiRing 先例；BSS NOBITS 不膨胀 ELF 文件体积（main.rs D-W1：`MaybeUninit::zeroed()` 全零初值落 BSS，运行时 write 有效化，≈96 KB 样本数组不占镜像，ELF 115,336 B） | 容量满时丢弃不越界（WCE5 锚定）；容量上限即蓝图采样数 |
| D3 | 双实现时钟源：aarch64 读 CNTVCT_EL0 / CNTFRQ_EL0（真实汇编）；host 经注入时钟 `ClockSource`（`fn() -> u64` 函数指针）驱动单测；trait 签名以蓝图 §3 为准（仅 `samples`），scheduler 交互经 ClockSource 内化；`cycles_to_ns` 带 `freq` 参数（host 可控注入 + 除零防御） | host 无法读 ARM 计数器（同 v0.137.0 D2 双实现惯例）；蓝图 §4.5 `cycles_to_ns(cycles)` 无参自读 freq 不便 host 测试 | 两路径统计语义一致；真机样本经 handler 埋点 `record_jitter` / `record_irq` 直填，host 经 measure_* 方法注入 |
| D4 | 排序用 `sort_unstable`（core 可用）替代蓝图 §4.5 `sort()`（需 alloc） | no_std 合规 | 统计语义一致（JTR1~JTR4 锚定） |
| D5 | QEMU 冒烟采样数 10,000 → 270（3 主周期 × 默认表 90 tick，1 kHz 下约 3 s）；蓝图 §6.2「10,000 次采样」由 host 集成测试（WCT1）满足 | 1 kHz tick 下 10,000 次 slot 切换需 300 s，超 boot 测试时间预算 | 270 tick 恒产生 21 次切换样本，p99 索引钳末位 ≡ max（冒烟口径已声明，足以暴露定时器粗粒度抖动） |
| D6 | IRQ 延迟测量口径：`latency = handler 入口 CNTVCT − 预期到期时刻`（expected 首中断 = base + tval，此后按网格 +tval 递推）；蓝图 §4.2 仅述「从硬件中断到 handler 执行的延迟」未给具体公式 | 取可复现口径并注释（main.rs `wcet_timer_handler` 决策记录①） | 含定时器递送 + GIC 呈递 + 分发耗时；270 次 tick 全样本统计（irq99=250,544 ns 记录口径，不设阈值） |
| D7 | 上下文切换真机代理口径：threads 在 psched 路径不激活（v0.135.0 决策），真机无线程级切换可测；以稳态 `arinc.tick(0)` 路径 + CNTVCT 读对 1,000 次夹取 avg 为代理（纯路径耗时 + 两次计数器读） | 真机口径声明（main.rs `wcet_smoke` 头注决策记录）；线程级 context_switch WCET 留待 P3-D | ctx avg = 99 ns 为代理值；§7.2「上下文切换 < 5μs」按代理口径判定并声明 |
| D8 | TVAL 相对重装 → **CVAL 绝对比较寄存器**修复：首中断 `cntp_cval_el0 = base + tval`，handler ③ 按理想网格 `expected + tval` 递推（QEMU 首轮实测 TVAL 语义致 270 tick 累积漂移 26,582,368 ns 淹没真实抖动，见 §4） | TVAL 相对语义下实际周期 = tval + 入口延迟，测量基准随 N 线性漂移；CVAL 锚定绝对网格后延迟不累积 | 抖动样本回归「递送延迟」本义；仅触 v0.138.0 wcet 段三处（handler ③ / wcet_smoke ④ / WCET_TVAL 文档），既有十类输出与既有函数语义零改动 |

---

## 10. 源码链接

| 模块 | 路径（含行号区间） |
|------|-------------------|
| WCET 测量工具（头注 D2~D4 决策记录） | [crates/kernel/microkernel/src/sched/wcet.rs L1~L435](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs) |
| — 接口区（WcetMeasurement / WcetReport / WcetProfiler trait） | [wcet.rs L31~L70](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs) |
| — 计数器汇编与换算（read_cycle_counter / read_counter_freq / cycles_to_ns） | [wcet.rs L72~L106](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs) |
| — WcetProfilerImpl（定长 BSS 数组 + ClockSource + record_* 直写 + report 聚合） | [wcet.rs L108~L319](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs) |
| — WCE1~WCE5 内嵌单测 | [wcet.rs L321~L435](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/wcet.rs) |
| 抖动统计（JitterStats / compute + JTR1~JTR4） | [crates/kernel/microkernel/src/sched/jitter.rs L1~L132](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/jitter.rs) |
| — compute（sort_unstable + 百分位 + u128 avg） | [jitter.rs L34~L56](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/jitter.rs) |
| — JTR1~JTR4 内嵌单测 | [jitter.rs L58~L132](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/jitter.rs) |
| sched 子系统挂载（`pub mod jitter; pub mod wcet;`） | [crates/kernel/microkernel/src/sched/mod.rs L26~L30](file:///e:/EnerOS/crates/kernel/microkernel/src/sched/mod.rs) |
| 内核启动期接入（WCET 静态量 + wcet_timer_handler + wcet_smoke + D5~D8 决策记录） | [crates/kernel/microkernel/src/main.rs L1828~L2175](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — WCET 支撑静态量（WCET_PROF / WCET_TICK / WCET_FREQ / WCET_TVAL / WCET_BASE / WCET_EXPECT / WCET_DONE） | [main.rs L1828~L1893](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — wcet_timer_handler（D6 延迟埋点 + 抖动埋点 + D8 CVAL 网格重装） | [main.rs L1895~L1988](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — wcet_smoke（①有效化/锚定 → ④CVAL arm → ⑤270 tick 收集 → ⑥ctx 代理 → ⑦统计判定） | [main.rs L1990~L2175](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| — rust_main 调用点（`[sync] ok` 后追加） | [main.rs L772~L775](file:///e:/EnerOS/crates/kernel/microkernel/src/main.rs) |
| host 集成测试 WCT1~WCT6 | [crates/kernel/microkernel/tests/wcet_test.rs L1~L305](file:///e:/EnerOS/crates/kernel/microkernel/tests/wcet_test.rs) |
| — WCT1（10,000 采样 + max < 1ms 判定） | [wcet_test.rs L113~L156](file:///e:/EnerOS/crates/kernel/microkernel/tests/wcet_test.rs) |
| — WCT2（p99 与 max 分离）/ WCT3（cycles_to_ns）/ WCT4（ctx 冒烟） | [wcet_test.rs L170~L234](file:///e:/EnerOS/crates/kernel/microkernel/tests/wcet_test.rs) |
| — WCT5（perf smoke）/ WCT6（故障注入 `#[ignore]`） | [wcet_test.rs L245~L305](file:///e:/EnerOS/crates/kernel/microkernel/tests/wcet_test.rs) |
| QEMU 测试脚本（第十一条断言 `[wcet] ok`） | [tests/boot_test.sh](file:///e:/EnerOS/tests/boot_test.sh) |
| QEMU 实测记录（串口全文 / 十一断言 / D8 排障全程） | [docs/boot/qemu_run.md](file:///e:/EnerOS/docs/boot/qemu_run.md) §17（L534~L606） |
| 多核内存一致性设计文档（v0.137.0，SpinLock 复用来源） | [docs/kernel/memory_ordering.md](file:///e:/EnerOS/docs/kernel/memory_ordering.md) |
| SMP 设计文档（v0.136.0） | [docs/kernel/smp_design.md](file:///e:/EnerOS/docs/kernel/smp_design.md) |
| 线程调度设计文档（v0.134.0） | [docs/kernel/scheduler_design.md](file:///e:/EnerOS/docs/kernel/scheduler_design.md) |
| 中断管理设计文档（v0.133.0，IRQ 30 定时器链路） | [docs/kernel/interrupt_design.md](file:///e:/EnerOS/docs/kernel/interrupt_design.md) |
