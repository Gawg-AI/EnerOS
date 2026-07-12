# 北斗 GNSS 授时设计

> 版本：v0.12.1
> 适用范围：EnerOS Time 服务北斗 GNSS 授时模块
> 蓝图依据：`蓝图/phase0.md` §v0.12.1
> crate：eneros-time（`crates/drivers/time/src/beidou/`）
> 接口规范：`docs/hal-interface-spec.md` HalClock
> 前置版本：v0.12.0（RTC + 单调时钟）
> 配置参考：`configs/time/beidou.toml`

---

## 1. 概述

EnerOS v0.12.1 引入北斗 GNSS 授时模块，通过集成北斗卫星接收模块，利用 1PPS（秒脉冲）信号与 NMEA 0183 报文配对，将系统时钟同步至北斗 BDT（BeiDou Navigation Satellite System Time），同步精度优于 100ns。

### 1.1 选型理由

| 原因 | 说明 |
|------|------|
| 自主可控 | 北斗系统由中国自主研发，符合信创与能源行业准入要求（ADR-0003 合规闸门） |
| 高精度授时 | 1PPS 信号边沿精度优于 50ns，配合 NMEA 报文可实现 < 100ns 同步 |
| 不依赖 GPS | 仅使用北斗 BDI/BD2/BD3 信号，避免 GPS 受制于人的风险 |
| 与 RTC 互补 | RTC 提供掉电保持的粗粒度墙钟，北斗提供高精度实时校准 |

### 1.2 在 EnerOS 中的位置

```
┌──────────────────────────────────────────────────────────┐
│  上层：api.rs（get_time / rtc_read / ...）               │
└──────────────────────┬───────────────────────────────────┘
                       │ beidou_sync() / discipline_clock()
┌──────────────────────▼───────────────────────────────────┐
│  time crate — beidou 子模块                              │
│  beidou/mod.rs    — 入口、TimeStamp、BeidouState         │
│  beidou/nmea.rs   — NMEA 0183 解析                       │
│  beidou/pps.rs    — 1PPS 中断处理 + PI 控制器            │
└──────────────────────┬───────────────────────────────────┘
                       │
         ┌─────────────┴─────────────┐
         │                           │
┌────────▼────────┐         ┌────────▼────────┐
│  UART RX         │         │  GPIO IRQ       │
│  ($GNZDA/$GPRMC)│         │  (1PPS rising)  │
└─────────────────┘         └─────────────────┘
```

### 1.3 关键特性

| 特性 | 说明 |
|------|------|
| 同步精度 | < 100ns（1PPS 边沿 + PI 控制器微调） |
| 时间基准 | BDT（北斗时），epoch = 2006-01-01 00:00:00 UTC |
| 闰秒处理 | 内置闰秒表，自动计算 BDT-UTC 偏移 |
| 时钟单调性 | PI 输出限幅 ±50µs，保证 disciplined clock 不回退 |
| NMEA 报文 | 支持 $GNZDA（时间日期）与 $GPRMC（定位状态） |
| 无动态分配 | 全部使用 `&[u8]` 切片 + 固定容量数组，no_std 兼容 |

---

## 2. BDT 时间系统

### 2.1 BDT 基础

BDT（BeiDou Navigation Satellite System Time）是北斗卫星导航系统的连续时间基准，具有以下特征：

| 属性 | 值 |
|------|-----|
| Epoch | 2006-01-01 00:00:00 UTC |
| Unix 秒 | 1,136,073,600 |
| 连续性 | 无闰秒（连续时间尺度） |
| 与 TAI 关系 | BDT = TAI − 33s |
| 与 UTC 关系 | BDT = UTC + (TAI−UTC) − 33 = UTC + 4s（当前） |

BDT 是连续时间尺度，不引入闰秒。而 UTC 会定期插入闰秒，因此 BDT 与 UTC 的偏移随闰秒插入而变化。模块通过 `leap_seconds` 字段记录当前 BDT-UTC 偏移。

### 2.2 闰秒表

模块内置闰秒插入表（`LEAP_SECOND_TABLE`），记录每次闰秒插入后的 BDT-UTC 偏移：

```rust
const LEAP_SECOND_TABLE: [(u16, u8, u8, i32); 4] = [
    (2009, 1, 1, 1),  // inserted 2008-12-31
    (2012, 7, 1, 2),  // inserted 2012-06-30
    (2015, 7, 1, 3),  // inserted 2015-06-30
    (2017, 1, 1, 4),  // inserted 2016-12-31
];
```

`bdt_utc_offset(year, month, day)` 函数查找此表，返回给定日期适用的偏移量。当 NMEA 报文中的 `second == 60`（闰秒插入时刻），模块返回 `SyncError::LeapSecondAmbiguous`，由上层决定如何处理。

### 2.3 BDT 纳秒计算

```rust
fn compute_bdt_nanos(year, month, day, hour, minute, second, centisecond) -> u64 {
    let unix_secs = rtc_to_secs(&RtcTime { year, month, day, hour, minute, second, ... });
    let bdt_secs = unix_secs.saturating_sub(BDT_EPOCH_UNIX_SECS);
    bdt_secs * 1_000_000_000 + centisecond * 10_000_000
}
```

复用 `rtc.rs` 中的 Howard Hinnant 日历转换算法（`rtc_to_secs`），将 UTC 日历字段转为 Unix 秒，再减去 BDT epoch 得到 BDT 秒，最后转换为纳秒并加上厘秒子秒分量。

---

## 3. NMEA 0183 报文解析

### 3.1 支持的报文类型

| 报文 | 用途 | 关键字段 |
|------|------|----------|
| `$xxZDA` | UTC 时间与日期 | hhmmss.ss, dd, mm, yyyy |
| `$xxRMC` | 推荐最小定位信息 | hhmmss.ss, A/V（定位有效性） |
| 其他 | 忽略，返回 `Unknown` | — |

模块支持任何 talker 前缀（`GN`、`BD`、`GP` 等），仅按最后 3 个字符（`ZDA`/`RMC`）匹配句子类型。

### 3.2 零分配解析

NMEA 解析全部基于 `&[u8]` 切片操作，不使用 `alloc::string::String` 或 `Vec`：

- **`FieldSplitter`**：逗号分隔字段迭代器，逐字段返回 `&[u8]` 切片
- **`parse_time`**：解析 `hhmmss.ss`，支持 0/1/2 位厘秒
- **`digit_val` / `hex_val`**：ASCII 字符转数值
- **`parse_u8` / `parse_u16`**：十进制字段解析，带溢出检查

### 3.3 校验和验证

NMEA 句子格式：`$<content>*<checksum><CR><LF>`

校验和 = `$` 与 `*` 之间所有字节的 XOR。模块验证校验和，不匹配时返回 `Err(SyncError::ParseError)`。支持大小写十六进制（`A-F` / `a-f`）。

### 3.4 异常处理

所有异常情况返回 `Err(SyncError::ParseError)`，绝不 panic：

| 异常 | 处理 |
|------|------|
| 校验和错误 | `ParseError` |
| 句子截断（过短） | `ParseError` |
| 缺少 `$` 或 `*` | `ParseError` |
| 字段非数字 | `ParseError` |
| 小时 > 23 / 分钟 > 59 / 秒 > 60 | `ParseError` |
| 日期 = 0 / 月份 > 12 | `ParseError` |
| 年份 < 2006（北斗启用前） | `ParseError` |
| 秒 == 60（闰秒） | 解析成功，由 `beidou_sync` 标记 `LeapSecondAmbiguous` |

---

## 4. 1PPS + NMEA 配对授时

### 4.1 配对原理

1PPS 与 NMEA 是两条独立的物理通道：

- **1PPS**：GPIO 中断，上升沿标记整秒边界，硬件捕获本地单调时钟纳秒
- **NMEA**：UART 报文，在 1PPS 后 ~200-400ms 到达，包含该秒的 BDT 时间

配对逻辑：

```
1PPS IRQ ──► 捕获 local_ns ──► 存入 LAST_PPS_NS
                                         │
NMEA RX  ──► feed_nmea() ──► 存入 LAST_NMEA
                                         │
                    beidou_sync() ◄──────┘
                         │
                    parse_nmea(LAST_NMEA)
                         │
                    compute BDT ns
                         │
                    pair with LAST_PPS_NS
                         │
                    discipline_clock()
```

### 4.2 beidou_sync() 流程

1. 从 `LAST_NMEA` 读取最近一条 NMEA 报文到栈缓冲区
2. 调用 `parse_nmea()` 解析，失败返回 `ParseError`
3. 从 `LAST_PPS_NS` 读取最近一次 1PPS 捕获，无则返回 `PpsTimeout`
4. 调用 `sync_from_message()` 计算 BDT 时间戳
5. 更新 `BeidouState.last_fix`，返回 `TimeStamp`

### 4.3 on_pps_pulse() 流程

1. 记录硬件捕获的 `local_ns` 到 `LAST_PPS_NS`
2. 将 `local_ns` 推入 PPS 历史环形缓冲区（4 个槽位）
3. 计算抖动（相邻间隔与平均间隔的最大偏差）
4. 更新 `BeidouState.pps_jitter_ns` 与 `disciplined = true`

---

## 5. PI 时钟 disciplining 控制器

### 5.1 控制目标

通过微调本地单调时钟的推进速率，使其逐渐收敛至 BDT，而非直接跳变（跳变会破坏单调性，影响 SOE 时标与定时器）。

### 5.2 PI 算法

```
error     = BDT_ns − local_ns      (正: 本地落后, 需加速)
P_term    = Kp × error              = error / 2
I_term   += Ki × error              = error / 10  (带抗饱和限幅)
output    = clamp(P_term + I_term, ±MAX_CORRECTION)
```

参数值（对应 `configs/time/beidou.toml`）：

| 参数 | 值 | 说明 |
|------|-----|------|
| Kp | 0.5 (1/2) | 比例增益 |
| Ki | 0.1 (1/10) | 积分增益 |
| 积分限幅 | ±500,000 ns (500µs) | 抗饱和 |
| 输出限幅 | ±50,000 ns (50µs) | 单次最大校正量 |

### 5.3 单调性保证

`MAX_CORRECTION_NS = 50µs` 是单调性的关键保证：

- 每次 discipline 调用对应 1 个 PPS 周期（1 秒实时时间）
- 最坏情况：校正为 −50µs，时钟仍推进 `1,000,000,000 − 50,000 = 999,950,000 ns`
- 时钟永远向前推进，不回退

### 5.4 抗饱和

积分项累积可能因持续正误差而无限增长。`INTEGRAL_LIMIT_NS = 500µs` 将积分项钳位在 ±500µs，防止过冲后需要长时间恢复。

---

## 6. 数据结构

### 6.1 TimeStamp

```rust
pub struct TimeStamp {
    pub nanos_since_epoch: u64,  // BDT 纳秒（自 2006-01-01）
    pub leap_seconds: i32,        // 当前 BDT-UTC 偏移
    pub fix_quality: FixQuality,  // 定位质量
}
```

### 6.2 FixQuality

```rust
pub enum FixQuality {
    NoFix,                        // 无定位
    Fix2D,                        // 2D 定位
    Fix3D { satellites: u8 },     // 3D 定位 + 卫星数
    RtkFixed,                     // RTK 固定解（最高精度）
}
```

### 6.3 BeidouState

```rust
pub struct BeidouState {
    pub last_fix: Option<TimeStamp>,
    pub pps_jitter_ns: u32,       // 1PPS 抖动（ns）
    pub satellites_visible: u8,   // 可见卫星数
    pub disciplined: bool,        // 时钟是否已 discipline
}
```

### 6.4 SyncError

```rust
pub enum SyncError {
    NoSignal,             // 无 GNSS 信号
    ParseError,           // NMEA 解析失败
    PpsTimeout,           // 1PPS 超时
    LeapSecondAmbiguous,  // 闰秒插入中，时间模糊
}
```

---

## 7. 公共 API

| 函数 | 签名 | 说明 |
|------|------|------|
| `feed_nmea` | `fn(line: &[u8])` | 从 UART RX 喂入一条 NMEA 报文 |
| `on_pps_pulse` | `fn(ts: TimeStamp)` | 1PPS 中断回调，记录硬件时戳 |
| `beidou_sync` | `fn() -> Result<TimeStamp, SyncError>` | 配对 NMEA + PPS，返回 BDT 时戳 |
| `discipline_clock` | `fn(pps: &BeidouState) -> Result<Duration, SyncError>` | PI 控制器计算校正量 |

---

## 8. 降级策略

### 8.1 信号丢失

| 场景 | 处理 |
|------|------|
| NMEA 无报文 | `beidou_sync` 返回 `NoSignal` |
| 1PPS 超时 | `beidou_sync` 返回 `PpsTimeout`，由 v0.12.2 守时模块兜底 |
| 定位无效（RMC status=V） | `beidou_sync` 返回 `NoSignal` |
| 闰秒插入中 | 返回 `LeapSecondAmbiguous`，上层可选择保持上一秒或等待 |

### 8.2 与 v0.12.2 守时模块的衔接

v0.12.1 仅负责"有信号时的精确同步"。当北斗信号丢失时：

1. v0.12.2 守时模块接管，基于 OCXO 晶振推算时间
2. 时钟不回退（单调性保证）
3. 24 小时守时漂移 < 1ms
4. 北斗信号恢复后，平滑回切至北斗授时

### 8.3 与 RTC 的关系

RTC（PL031）提供秒级墙钟，用于：
- 系统启动时北斗未锁定阶段的粗略时间
- 掉电后恢复时间基准
- 北斗与 RTC 偏差过大时的告警

北斗锁定后，`beidou_sync()` 返回的 BDT 时戳可用于校准 RTC（`rtc_write`）。

---

## 9. 设计决策

### 9.1 为什么仅北斗不依赖 GPS

| 决策点 | 说明 |
|--------|------|
| 自主可控 | 北斗系统由中国自主运营，不受外部控制 |
| 合规要求 | 能源行业准入要求使用国产授时源（ADR-0003） |
| 精度足够 | 北斗 1PPS 精度与 GPS 相当（均优于 50ns） |
| 多模兼容 | 解析器支持 `GN`（GNSS 联合）talker，未来可扩展 |

### 9.2 为什么用 PI 控制器而非直接跳变

| 决策点 | 说明 |
|--------|------|
| 单调性 | 直接跳变会导致 SOE 时标错乱、定时器失效 |
| 收敛性 | PI 控制器在 10-20 个 PPS 周期内收敛至 < 100ns |
| 抗噪声 | 积分项平滑短期抖动，比例项快速响应长期偏差 |
| 实现简单 | 纯整数运算，no_std 兼容，无浮点依赖 |

### 9.3 为什么 NMEA 解析用 `&[u8]` 而非 String

| 决策点 | 说明 |
|--------|------|
| no_std | 避免依赖 `alloc::string::String` |
| 零分配 | UART RX 缓冲区直接传入，无需拷贝 |
| 性能 | 切片操作比字符串操作更快 |
| 安全 | 固定容量缓冲区避免 OOM |

### 9.4 为什么 PPS 历史长度为 4

| 决策点 | 说明 |
|--------|------|
| 抖动估计 | 4 个样本足以计算合理的抖动估计 |
| 内存占用 | 4 × 8 = 32 字节，极小 |
| 响应速度 | 4 秒内更新抖动估计，对 1PPS 足够 |
| 环形缓冲 | 固定 4 槽位，无需动态分配 |

### 9.5 子模块不重复 `#![cfg_attr(not(test), no_std)]`

`lib.rs` 已声明 `#![cfg_attr(not(test), no_std)]`，子模块（`nmea.rs`、`pps.rs`）继承此属性，无需重复声明。

---

## 10. 测试覆盖

### 10.1 NMEA 解析测试（22 项）

| 类别 | 测试项 |
|------|--------|
| 正常解析 | ZDA 正常、BD talker、无厘秒、单厘秒位、RMC 有效/无效、GN talker |
| 校验和 | 错误校验和、大小写不敏感 |
| 截断 | 空输入、仅 `$`、无 `*`、无 `$`、校验和截断 |
| 非法字段 | 非法小时/日/月/秒、非数字字段、年份越界 |
| 闰秒 | 6 月闰秒、12 月闰秒、非法秒（61） |
| 其他 | 未知句子、空字段、字段分割器 |

### 10.2 PPS 与 PI 控制器测试（14 项）

| 类别 | 测试项 |
|------|--------|
| on_pps_pulse | 存储捕获、更新抖动、检测抖动 |
| discipline_clock | 无定位、无 PPS、正误差、负误差、限幅、积分累积、抗饱和、单调性 |
| PpsHistory | 零抖动、非零抖动、少样本、环形回绕 |

### 10.3 BDT 时间计算测试（7 项）

| 类别 | 测试项 |
|------|--------|
| beidou_sync | 完整同步、无 NMEA、无 PPS、错误校验和、闰秒 |
| BDT 计算 | 闰秒表查找、已知时间转换、含厘秒转换 |

---

## 11. 使用示例

```rust
use eneros_time::beidou::{beidou_sync, feed_nmea, on_pps_pulse, TimeStamp, FixQuality};

// 1. UART 接收到一条 NMEA 报文
feed_nmea(b"$GNZDA,123456.78,12,07,2026,,,*XX\r\n");

// 2. 1PPS 中断触发（local_ns 由硬件捕获）
on_pps_pulse(TimeStamp {
    nanos_since_epoch: 644_000_000_000,
    leap_seconds: 4,
    fix_quality: FixQuality::NoFix,
});

// 3. 执行同步
match beidou_sync() {
    Ok(ts) => {
        // ts.nanos_since_epoch = BDT 纳秒
        // ts.leap_seconds = 4 (当前 BDT-UTC 偏移)
        // ts.fix_quality = Fix3D { satellites: 8 }
    }
    Err(e) => {
        // 处理同步失败（信号丢失、解析错误等）
    }
}
```
