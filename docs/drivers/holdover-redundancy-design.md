# 守时与时钟冗余设计

> 版本：v0.12.2
> 适用范围：EnerOS Time 服务守时与时钟冗余模块
> 蓝图依据：`蓝图/phase0.md` §v0.12.2
> crate：eneros-time（`crates/drivers/time/src/holdover/` + `crates/drivers/time/src/redundancy.rs`）
> 前置版本：v0.12.1（北斗授时）、v0.12.0（RTC + 单调时钟）
> 配置参考：`configs/time/holdover.toml`

---

## 1. 概述

EnerOS v0.12.2 引入守时与时钟冗余模块，在北斗 GNSS 信号丢失时通过 OCXO（恒温晶振）+ RTC（实时时钟）维持系统时间，24h 漂移 < 1ms。模块实现北斗/OCXO/RTC 三源故障切换，切换瞬间平滑过渡不产生时钟跳变。

### 1.1 设计目标

| 指标 | 要求 | 实现方式 |
|------|------|---------|
| 24h 守时精度 | < 1ms | OCXO 频率补偿模型（1 ppb → 86.4µs/24h） |
| 故障切换 | 三源自动切换 | 健康度评分驱动，优先级 BeiDou > OCXO > RTC |
| 切换平滑性 | 无时钟跳变 | 残余偏移量逐步衰减（slew） |
| 降级单调性 | RTC-only 时标单调递增 | 有效时间 = 原始时间 + 衰减偏移 |
| 安全性 | 手动切换需授权 | 一次性授权令牌（authorize_switch） |

### 1.2 在 EnerOS 中的位置

```
┌──────────────────────────────────────────────────────────┐
│  上层：api.rs（get_time / get_monotonic_ns / ...）        │
└──────────────────────┬───────────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────────┐
│  redundancy.rs — 三源故障切换                             │
│  evaluate_sources() / switch_clock_source() /             │
│  auto_switch_if_needed()                                  │
└──────────────────────┬───────────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────────┐
│  holdover/mod.rs — 守时状态机                             │
│  HoldoverStatus / ClockSource / HoldoverQuality           │
│  holdover_quality() / authorize_switch() / sync_time()   │
│  current_time_ns() — 有效时间（含偏移校正）               │
└──────────────────────┬───────────────────────────────────┘
                       │
         ┌─────────────┴─────────────┐
         │                           │
┌────────▼────────┐         ┌────────▼────────┐
│  holdover/ocxo  │         │  beidou/        │
│  OcxoModel      │         │  TimeStamp      │
│  extrapolate_   │         │  beidou_sync()  │
│  time()         │         │                 │
└─────────────────┘         └─────────────────┘
```

---

## 2. 三源冗余架构

### 2.1 时钟源优先级

| 优先级 | 时钟源 | 精度 | 角色 |
|--------|--------|------|------|
| 1（主） | BeiDou GNSS | < 100ns | 主授时源，1PPS + NMEA |
| 2（备） | OCXO | < 1ms/24h | 守时振荡器，频率补偿 |
| 3（降级） | RTC | ~秒级 | 粗粒度兜底，电池保持 |

### 2.2 状态机

```
  Beidou (正常) ──北斗失锁──► Ocxo (守时) ──OCXO不健康──► Rtc (降级)
       ▲                                                         │
       └─────────── 北斗恢复（平滑回切） ─────────────────────────┘
```

状态转换条件：
- **BeiDou → Ocxo**：北斗健康度 < 50，OCXO 健康
- **Ocxo → Rtc**：OCXO 健康度 < 50，RTC 健康
- **任意 → BeiDou**：北斗恢复健康（自动回切）
- **无健康源**：保持当前源，记录告警

### 2.3 全局状态

守时状态存储在 `HOLDOVER_STATE: spin::Mutex<HoldoverInner>` 中，包含：

- `current_source`: 当前活跃时钟源
- `holdover_start_ns`: 守时开始时间（离开 BeiDou 时记录）
- `switch_offset_ns`: 切换残余偏移（逐步衰减）
- `authorized`: 一次性手动切换授权
- `beidou/ocxo/rtc_healthy` + `score`: 各源健康状态
- `ocxo_model`: OCXO 频率补偿模型参数
- `temperature_c`: 当前工作温度

---

## 3. OCXO 漂移模型

### 3.1 线性漂移 + 温度补偿

```
base_drift   = elapsed_ns × freq_offset_ppb / 1_000_000_000
temp_comp    = temperature_c × temp_coeff × elapsed_ns / 1_000_000_000
extrapolated = elapsed + base_drift + temp_comp
```

- `freq_offset_ppb`：频率偏差（十亿分之一），正=快，负=慢
- `temp_coeff`：温度系数（ppb/°C）
- 使用 `i128` 中间运算，避免大时间跨度溢出

### 3.2 24h 精度验证

典型 OCXO 频率稳定度 ≤ 1×10⁻⁹/日（1 ppb/日）：

```
24h = 86_400s = 86_400_000_000_000 ns
drift = 86_400_000_000_000 × 1 / 1_000_000_000 = 86_400 ns ≈ 86.4µs
```

86.4µs << 1ms，满足硬性验收标准。

| OCXO 稳定度 | 24h 漂移 | 质量等级 |
|-------------|---------|---------|
| 1 ppb | 86.4µs | Excellent（< 100µs） |
| 10 ppb | 864µs | Good（< 1ms） |
| 100 ppb | 8.64ms | Degraded（< 10ms） |
| > 100 ppb | > 10ms | Lost |

### 3.3 RTC 漂移

RTC（PL031）使用普通晶振，典型漂移 10-50 ppm。本模块采用保守值 20 ppm：

```
drift_per_hour = 3.6×10¹² × 20000 / 10⁹ = 72ms/h
24h_drift = 72ms × 24 = 1.728s
```

RTC 模式质量等级为 `Lost`，表示时间仅作粗粒度参考。

---

## 4. 健康度评分

### 4.1 评分机制

每个时钟源拥有 0-100 的健康度评分：

| 评分范围 | 状态 | 说明 |
|---------|------|------|
| ≥ 50 | 健康（healthy=true） | 可作为切换目标 |
| < 50 | 不健康（healthy=false） | 触发自动切换 |

### 4.2 评估接口

```rust
pub fn evaluate_sources() -> [SourceHealth; 3]
```

返回三源健康度快照（BeiDou, OCXO, RTC 顺序），更新 `last_check_ns`。

### 4.3 健康度来源

- **BeiDou**：由 `beidou_sync()` 成功/失败驱动（fix_quality、pps_jitter）
- **OCXO**：由 OCXO 硬件状态/温度监控驱动
- **RTC**：由 RTC 电池电压/读数有效性驱动

当前版本通过 `set_source_health()` 由外部设置（后续版本自动采集）。

---

## 5. 平滑切换算法

### 5.1 设计原则

切换瞬间不产生时钟跳变——这是硬性要求，因为时钟跳变会导致：
- 实时控制周期紊乱
- 日志时间戳错乱
- 分布式系统时钟不同步

### 5.2 有效时间计算

```
effective_time = raw_monotonic_ns + switch_offset_ns
```

- `switch_offset_ns`：残余偏移量，有符号
- 切换时不改变 `switch_offset_ns`（保持 0），确保有效时间连续
- 偏移量通过 `decay_switch_offset()` 逐步衰减至 0

### 5.3 衰减算法

```rust
fn decay_switch_offset(state: &mut HoldoverInner) {
    if state.switch_offset_ns > 0 {
        state.switch_offset_ns = saturating_sub(SLEW_RATE_NS);  // 100µs/step
    } else if state.switch_offset_ns < 0 {
        state.switch_offset_ns = saturating_add(SLEW_RATE_NS);
    }
}
```

每次调用 `holdover_quality()` 时执行一次衰减。100µs/step 的速率确保：
- 1ms 偏移在 10 次评估内校正完毕
- 校正过程对 10ms 控制周期无感知

### 5.4 回切策略

北斗恢复时，自动切换回 BeiDou（`auto_switch_if_needed`）。此时：
- `holdover_start_ns` 清零（退出守时）
- `switch_offset_ns` 保持当前值（如有），继续衰减
- 有效时间连续，无跳变

---

## 6. 授权机制

### 6.1 手动切换授权

```rust
pub fn authorize_switch()           // 授予一次性授权
pub fn switch_clock_source(target)  // 消耗授权
```

- 授权为一次性令牌：无论切换成功或失败（AlreadyActive/SourceUnavailable），授权均被消耗
- 防止恶意代码触发降级攻击（强制切换至低精度源）

### 6.2 自动切换豁免

`auto_switch_if_needed()` 是安全机制，**不需要授权**。当当前源不健康时，系统必须立即切换至最佳可用源，不应等待授权。

---

## 7. 配置参数

参见 `configs/time/holdover.toml`：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `ocxo.freq_offset_ppb` | 1 | OCXO 频率偏差（ppb） |
| `ocxo.temp_coeff` | 0 | 温度系数（ppb/°C） |
| `rtc.drift_ppb` | 20000 | RTC 漂移（20 ppm） |
| `switching.health_threshold` | 50 | 健康度阈值 |
| `switching.slew_rate_ns` | 100000 | 偏移衰减速率（100µs/step） |
| `accuracy.max_24h_drift_ms` | 1 | 24h 漂移硬性阈值 |

---

## 8. 验收标准

| # | 验收项 | 测试方法 | 状态 |
|---|--------|---------|------|
| 1 | 24h 漂移 < 1ms | `test_extrapolate_24h_drift_under_1ms` | ✅ |
| 2 | 10ppb 下 24h 漂移 < 1ms | `test_extrapolate_24h_10ppb_under_1ms` | ✅ |
| 3 | 零漂移模型返回原时间 | `test_zero_drift_model` | ✅ |
| 4 | 温度补偿计算正确 | `test_temperature_compensation` | ✅ |
| 5 | holdover_quality 返回正确状态 | `test_holdover_quality_on_*` | ✅ |
| 6 | 手动切换授权/未授权路径 | `test_switch_*` | ✅ |
| 7 | 状态机转换 BeiDou→OCXO→RTC | `test_state_machine_beidou_to_ocxo_to_rtc` | ✅ |
| 8 | 北斗恢复平滑回切 | `test_state_machine_recovery_beidou_restored` | ✅ |
| 9 | evaluate_sources 返回三源健康度 | `test_evaluate_sources_*` | ✅ |
| 10 | auto_switch_if_needed 自动切换 | `test_auto_switch_*` | ✅ |
| 11 | 三源切换无时钟跳变 | `test_smooth_transition_*` | ✅ |
| 12 | RTC-only 时标单调递增 | `test_rtc_only_monotonic` | ✅ |

---

## 9. 模块文件清单

| 文件 | 说明 |
|------|------|
| `crates/drivers/time/src/holdover/mod.rs` | 守时状态机、类型定义、公共接口 |
| `crates/drivers/time/src/holdover/ocxo.rs` | OCXO 频率补偿模型与时间推算 |
| `crates/drivers/time/src/redundancy.rs` | 三源故障切换逻辑 |
| `configs/time/holdover.toml` | 守时与冗余配置模板 |
| `docs/drivers/holdover-redundancy-design.md` | 本设计文档 |
