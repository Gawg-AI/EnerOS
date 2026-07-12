# PL031 RTC 驱动设计

> 版本：v0.12.0
> 适用范围：EnerOS Time 服务 PL031 实时时钟驱动
> 蓝图依据：`蓝图/phase0.md` §v0.12.0
> crate：eneros-time（`time/src/rtc.rs`）
> 硬件参考：ARM PrimeCell Real Time Clock (PL031) Technical Reference Manual（ARM DDI 0224）
> 接口规范：`docs/hal-interface-spec.md` HalClock

---

## 1. 概述

PL031 是 ARM PrimeCell 系列的实时时钟（Real-Time Clock）IP，属于 AMBA APB 外设，提供一个 32 位秒计数器并由外部电池备份，掉电后仍可维持计时。EnerOS 在 v0.12.0 引入 PL031 驱动，为系统提供墙钟（wall-clock）时间来源，用于日志时间戳、定时调度基准、RTC 校准等场景。

### 1.1 选型理由

| 原因 | 说明 |
|------|------|
| QEMU virt 默认 RTC | `qemu-system-aarch64 -M virt` 内置 PL031 @ `0x0901_0000`，免配置即可使用 |
| 电池备份 | 32 位秒计数器由外部电池供电，主掉电后计时不丢失 |
| 接口简单 | 仅需 MMIO 读写即可获取秒级时间，无需复杂初始化序列 |
| 国产平台兼容 | 飞腾/鲲鹏等 SoC 普遍集成 PL031 兼容 RTC，驱动可复用 |

### 1.2 在 EnerOS 中的位置

```
┌─────────────────────────────────────────────┐
│  上层：api.rs（get_time / rtc_read / ...）  │
└──────────────────┬──────────────────────────┘
                   │ Pl031Rtc::read() / read_secs()
┌──────────────────▼──────────────────────────┐
│  time crate — Pl031Rtc 驱动                 │
│  time/src/rtc.rs                            │
└──────────────────┬──────────────────────────┘
                   │ MMIO（read_volatile / write_volatile）
┌──────────────────▼──────────────────────────┐
│  PL031 硬件 @ 0x0901_0000（QEMU virt）      │
└─────────────────────────────────────────────┘
```

### 1.3 关键特性

| 特性 | 说明 |
|------|------|
| 计数器宽度 | 32 位秒计数器（约 136 年范围，自 Unix epoch 起） |
| 计时粒度 | 秒级（1 Hz） |
| 电池备份 | 外部 VBAT 供电，主电源关闭后持续计时 |
| 访问方式 | 32 位 MMIO 读写 |
| 中断支持 | 支持匹配中断（本驱动未启用，v0.12.0 仅轮询读取） |

---

## 2. PL031 寄存器映射

PL031 寄存器相对于基地址偏移如下。QEMU virt 平台基地址为 `0x0901_0000`。

| 偏移 | 名称 | 读写 | 说明 | 驱动使用 |
|------|------|------|------|----------|
| `0x00` | RTCDR | RO | Data Register，读取当前秒计数（Unix epoch 秒） | 是（`read_secs`） |
| `0x04` | RTCMR | RW | Match Register，匹配值，相等时触发中断 | 否 |
| `0x08` | RTCLR | RW | Load Register（部分文档记此偏移），加载计数器 | 否 |
| `0x0C` | RTCIMSC | RW | Interrupt Mask Set/Clear，写 1 使能中断，写 0 禁用 | 否 |
| `0x18` | RTCRIS | RO | Raw Interrupt Status，原始中断状态（未经过掩码） | 否 |
| `0x1C` | RTCMIS | RO | Masked Interrupt Status，掩码后的中断状态 | 否 |
| `0x20` | RTCLOAD | WO | Load Register，写入以设置当前秒计数 | 是（`write_secs`） |
| `0x24` | RTCICR | WO | Interrupt Clear Register，清除中断标志 | 否 |
| `0x2C` | RTCCR | RW | Control Register，bit 0 为 RTC 使能位 | 是（`enable`） |

> **说明**：v0.12.0 驱动仅使用 RTCDR / RTCLOAD / RTCCR 三个寄存器完成时间读取、校准与使能。中断相关寄存器（RTCIMSC / RTCRIS / RTCMIS / RTCICR）属于硬件完整映射，预留给后续版本（如 RTC 闹钟中断）使用，当前未实现。

代码中定义的常量：

```rust
const RTCDR: u64 = 0x00;   // Data Register — reads current seconds count
const RTCLOAD: u64 = 0x20; // Load Register — writes to set the current seconds count
const RTCCR: u64 = 0x2c;   // Control Register — bit 0 enables the RTC
```

---

## 3. 驱动结构体与方法

### 3.1 Pl031Rtc 结构体

`Pl031Rtc` 仅持有一个 MMIO 基地址，结构极简：

```rust
pub struct Pl031Rtc {
    base: u64,
}
```

### 3.2 方法说明

| 方法 | 签名 | 说明 |
|------|------|------|
| `new` | `const fn new(base: u64) -> Self` | 创建驱动实例，绑定 MMIO 基地址。`const fn` 支持 `static` 初始化 |
| `read_secs` | `fn read_secs(&self) -> u64` | 读取 RTCDR，返回当前 Unix epoch 秒（32 位寄存器零扩展为 u64） |
| `write_secs` | `fn write_secs(&self, secs: u64)` | 写入 RTCLOAD，校准秒计数（截断为 u32） |
| `read` | `fn read(&self) -> RtcTime` | 读取当前时间并转换为 `RtcTime`；电池失效时返回 Unix epoch |
| `write` | `fn write(&self, t: RtcTime)` | 将 `RtcTime` 转换为秒后写入 RTC，用于校准 |
| `enable` | `fn enable(&self)` | 置 RTCCR bit 0 为 1，使能 RTC |

### 3.3 MMIO 操作方式

所有寄存器访问使用 `core::ptr::read_volatile` / `write_volatile`，确保编译器不会优化掉硬件访问。寄存器宽度为 32 位（`*const u32` / `*mut u32`）：

```rust
use core::ptr::{read_volatile, write_volatile};

pub fn read_secs(&self) -> u64 {
    let ptr = (self.base + RTCDR) as *const u32;
    // SAFETY: reading a 32-bit MMIO register at the configured base offset.
    unsafe { read_volatile(ptr) as u64 }
}

pub fn write_secs(&self, secs: u64) {
    let ptr = (self.base + RTCLOAD) as *mut u32;
    unsafe { write_volatile(ptr, secs as u32) };
}

pub fn enable(&self) {
    let ptr = (self.base + RTCCR) as *mut u32;
    unsafe { write_volatile(ptr, 0x1) };
}
```

> **安全性**：调用方需保证 `base` 指向有效的 PL031 设备地址。在 QEMU virt 平台上该地址由设备树保证有效。

---

## 4. 时间数据结构

### 4.1 RtcTime

`RtcTime` 是人类可读的日历时间表示：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcTime {
    pub year: u16,   // e.g. 2026
    pub month: u8,   // 1-12
    pub day: u8,     // 1-31
    pub hour: u8,    // 0-23
    pub minute: u8,  // 0-59
    pub second: u8,  // 0-59
    pub weekday: u8, // 0=Sunday, 1=Monday, ..., 6=Saturday
}
```

| 字段 | 类型 | 范围 | 说明 |
|------|------|------|------|
| `year` | `u16` | 如 2026 | 公历年份 |
| `month` | `u8` | 1–12 | 月份，1 = 一月 |
| `day` | `u8` | 1–31 | 日 |
| `hour` | `u8` | 0–23 | 小时（24 小时制） |
| `minute` | `u8` | 0–59 | 分钟 |
| `second` | `u8` | 0–59 | 秒 |
| `weekday` | `u8` | 0–6 | 星期，0 = Sunday ... 6 = Saturday |

### 4.2 TimeStamp

`TimeStamp` 是以纳秒为单位的 Unix epoch 时间戳，是一个新型别（newtype）封装：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeStamp(pub u64);
```

- `TimeStamp` 实现 `Ord`，可直接比较先后顺序。
- 由上层 `api.rs` 的 `get_time()` 返回，值为 `RTC_OFFSET_NS + monotonic_ns`。
- 纳秒精度，u64 范围约可表示 584 年。

---

## 5. Howard Hinnant 日历转换算法

### 5.1 算法概述

RTC 寄存器存储的是 Unix epoch 秒，而用户需要的是年月日时分秒。EnerOS 采用 Howard Hinnant 提出的日历转换算法（`days_from_civil` / `civil_from_days`），该算法以 proleptic Gregorian 历法为基础，通过整数运算完成 civil 日期与"自 1970-01-01 起的天数"之间的双向转换。

代码中实现了三个核心函数（均为私有 `fn`）：

| 函数 | 签名 | 说明 |
|------|------|------|
| `days_from_civil` | `fn(year: i64, month: i64, day: i64) -> i64` | (年, 月, 日) → 自 epoch 起天数 |
| `civil_from_days` | `fn(days: i64) -> (i64, i64, i64)` | 自 epoch 起天数 → (年, 月, 日) |
| `weekday_from_days` | `fn(days: i64) -> u8` | 自 epoch 起天数 → 星期（0=Sunday） |

### 5.2 days_from_civil 实现

```rust
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}
```

算法将年份按 400 年一个 era 划分（每个 era 恰好 146097 天，闰年规则封闭），再在 era 内部用 `yoe`（year-of-era）、`doy`（day-of-year）、`doe`（day-of-era）逐级换算。`719468` 是 1970-01-01 相对于 era 起点的天数偏移。

### 5.3 civil_from_days 实现

```rust
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
```

这是 `days_from_civil` 的逆运算，通过 `doe` 反推 `yoe`，再由 `doy` 反推月份与日。

### 5.4 weekday_from_days 实现

1970-01-01 是星期四（weekday = 4）。通过对天数取模 7 并加 4 偏移得到星期：

```rust
fn weekday_from_days(days: i64) -> u8 {
    let wd = (days % 7 + 4) % 7; // 1970-01-01 is Thursday (4)
    if wd < 0 {
        (wd + 7) as u8
    } else {
        wd as u8
    }
}
```

负数天数（epoch 之前）通过 `+7` 归一化到 `[0, 6]` 区间。

### 5.5 公共转换函数

基于上述算法，提供两个公共转换函数：

```rust
/// Unix epoch 秒 → RtcTime
pub fn secs_to_rtc(secs: u64) -> RtcTime {
    let days = (secs / 86400) as i64;
    let rem_secs = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    let weekday = weekday_from_days(days);
    RtcTime {
        year: year as u16,
        month: month as u8,
        day: day as u8,
        hour: (rem_secs / 3600) as u8,
        minute: ((rem_secs % 3600) / 60) as u8,
        second: (rem_secs % 60) as u8,
        weekday,
    }
}

/// RtcTime → Unix epoch 秒
pub fn rtc_to_secs(t: &RtcTime) -> u64 {
    let days = days_from_civil(t.year as i64, t.month as i64, t.day as i64);
    let secs_in_day = t.hour as u64 * 3600 + t.minute as u64 * 60 + t.second as u64;
    (days as u64) * 86400 + secs_in_day
}
```

---

## 6. RTC 电池失效处理

当 RTC 电池耗尽或首次上电未校准时，RTCDR 可能返回 0。`read()` 方法检测到 `secs == 0` 时，判定为电池失效，直接返回 Unix epoch（1970-01-01 00:00:00，星期四）：

```rust
pub fn read(&self) -> RtcTime {
    let secs = self.read_secs();
    if secs == 0 {
        // RTC battery failure — return the Unix epoch.
        RtcTime {
            year: 1970, month: 1, day: 1,
            hour: 0, minute: 0, second: 0,
            weekday: 4, // 1970-01-01 was a Thursday
        }
    } else {
        secs_to_rtc(secs)
    }
}
```

这一设计的意义：

- 避免将 `secs == 0` 误判为合法的 1970 年时间（实际几乎不可能处于该时刻）。
- 提供可识别的兜底值，上层可据此触发 RTC 校准流程。
- 不 panic，保证系统在 RTC 异常时仍可启动。

---

## 7. 使用示例

### 7.1 基本读写

```rust
use eneros_time::{Pl031Rtc, RtcTime};

// QEMU virt 平台 PL031 基地址
let rtc = Pl031Rtc::new(0x0901_0000);

// 使能 RTC
rtc.enable();

// 读取当前时间
let t: RtcTime = rtc.read();
println!("{}-{:02}-{:02} {:02}:{:02}:{:02}",
    t.year, t.month, t.day, t.hour, t.minute, t.second);

// 校准 RTC（写入指定时间）
rtc.write(RtcTime {
    year: 2026, month: 7, day: 12,
    hour: 10, minute: 30, second: 0,
    weekday: 0, // weekday 会被 secs_to_rtc 重算，写入时忽略
});
```

### 7.2 通过上层 API 访问

通常不直接使用 `Pl031Rtc`，而是通过 `time_init` 注入后使用 `api.rs` 提供的统一接口：

```rust
use eneros_time::{time_init, rtc_read, rtc_write, get_time};

// 初始化时间服务（clock 为 Arm64Timer 实现的 HalClock）
time_init(clock, 0x0901_0000);

// 读取墙钟时间
let wall = get_time(); // TimeStamp（纳秒）

// 直接读写 RTC
let t = rtc_read();
rtc_write(RtcTime { year: 2026, month: 7, day: 12, ..t });
```

---

## 8. 设计决策

### 8.1 为什么用 Howard Hinnant 算法

| 决策点 | 说明 |
|--------|------|
| 无外部依赖 | 算法纯整数运算，无需 `chrono` / `time` 等 std 生态 crate，符合 no_std 约束 |
| i64 防溢出 | 全程使用 `i64` 运算，可表示约 ±2.9 亿年，彻底避免 32 位溢出 |
| 标准 days-since-epoch | 以"自 1970-01-01 起的天数"为中间量，与 Unix 时间戳自然衔接 |
| 闰年正确性 | 400 年 era 封闭闰年规则，自动处理格里高利历的所有闰年边界 |
| 双向可逆 | `days_from_civil` 与 `civil_from_days` 互为逆函数，round-trip 测试验证 |

### 8.2 为什么不使用中断

v0.12.0 仅需秒级墙钟读取，采用轮询方式（`read_secs`）足够。RTC 匹配中断（RTCMR + RTCIMSC）预留给后续需要闹钟功能的版本。引入中断会增加 GIC 注册与中断上下文管理复杂度，不符合 Phase 0 简化目标。

### 8.3 为什么 read_secs 零扩展为 u64

PL031 的 RTCDR 是 32 位寄存器，32 位秒计数自 1970 起可表示到 2106 年。驱动将其零扩展为 u64 返回，与上层 `TimeStamp(u64)` 纳秒表示兼容，同时为未来可能的 64 位 RTC 留出空间。

### 8.4 weekday 字段的冗余性

`RtcTime` 包含 `weekday` 字段，但该字段可由 `year/month/day` 通过 `weekday_from_days` 唯一确定。保留该字段的原因：

- `read()` 返回时已计算好，避免上层重复计算。
- `write()` 时上层可能未填写，`rtc_to_secs` 不使用 `weekday`（仅用 year/month/day/hour/minute/second），因此填写任意值不影响校准结果。
