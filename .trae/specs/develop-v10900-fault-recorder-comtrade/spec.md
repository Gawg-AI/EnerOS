# v0.109.0 故障录波 COMTRADE Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.109.0（P2-H 第 1 版，9 节齐全）。新建 crate `crates/protocols/fault-recorder/`（eneros-fault-recorder，零第三方依赖）。蓝图检索确认无 v0.109.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

故障录波（Fault Recording）是电力事故追溯的法定数据源：故障时刻前后的电压/电流/开关量波形必须以 IEEE C37.111 COMTRADE 标准格式落盘，供标准分析工具（如 Sigra、Wavewin）解析。v0.55.0 已落地高频采样，v0.24.0 已落地文件系统，v0.108.0 已提供安全 SV 采样源。本版实现环形采样缓冲 + 7 类故障触发条件 + COMTRADE .cfg/.dat 文件生成导出，打通「采样 → 触发 → 录波 → 导出」链路，为 v0.110.0 云边同步提供可上传的录波文件。

## What Changes

- **新建** `crates/protocols/fault-recorder/`（`eneros-fault-recorder`，no_std + alloc，零第三方依赖）：
  - `src/ring_buffer.rs`：`RingSampleBuffer<T: Copy>`（固定容量环形缓冲，溢出覆盖最旧，`get_recent(n)` 按时间序取最近 n 个，蓝图 §4.5 基型，D5）
  - `src/trigger.rs`：`TriggerType`（7 变体）/ `TriggerCondition` / 内部 `TriggerEngine`（持续超阈值帧计数、变化率相邻差分、数字量上升沿、配置序优先级，D12）
  - `src/comtrade_writer.rs`：`Phase` / `ChannelConfig` / `ComtradeConfig` / `ComtradeFormat` / `SampleRecord` / `ComtradeWriter`（`write_cfg` → String、`write_dat` → Vec<u8>，C37.111-2013 合规修复 D6~D9）
  - `src/lib.rs`：`FaultRecorder` / `RecorderConfig` / `RecorderState`（Idle→Recording→Ready 状态机）/ `RecorderError`（4 变体，D12）/ `FileSink` trait + `MockSink`（D4）+ 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/fault-recorder.toml`：`[recorder]` 采样率/前后窗/缓冲配置 + `[[triggers]]` 触发条件模板 + 中文注释 ≥7 点
- **新增** `docs/protocols/fault-recorder-comtrade-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 31 个单元测试**（src 内嵌 `#[cfg(test)]`：RB×6 + TG×7 + CW×9 + FR×8 + PERF×1）
- 根 `Cargo.toml`：members 追加 `"crates/protocols/fault-recorder"` + version 0.108.0 → 0.109.0；`Makefile`（VERSION + L3 头部注释）/ `ci.yml` L3 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v10900-fault-recorder-comtrade（新建）
- Affected code：`crates/protocols/fault-recorder/`（新建）、`configs/`、`docs/protocols/`、根 4 文件版本号
- 上游：v0.55.0 高频采样（采样数据源）、v0.24.0 文件系统（集成层落盘，经 FileSink 抽象 D4）、v0.108.0 SV 安全采样
- 下游：v0.110.0 云边同步（录波文件上传）

## ADDED Requirements

### Requirement: 环形采样缓冲（ring_buffer.rs）

The system SHALL provide `RingSampleBuffer<T: Copy + Default>`：`new(capacity)` 分配固定容量缓冲；`push(value)` 写入并将 write_pos 前移，满则覆盖最旧；`push_slice(slice)` 逐元素写入；`get_recent(n)` 返回最近 min(n, 已写入数) 个元素且按时间旧→新排序；`len()` 返回 min(samples_written, capacity)；`capacity()` 返回容量。

#### Scenario: 溢出覆盖保序（蓝图 §4.4）
- **WHEN** capacity=4，连续 push 1,2,3,4,5,6
- **THEN** `get_recent(4)` 返回 [3,4,5,6]（最旧 2 个被覆盖，保序）

#### Scenario: 未写满读取
- **WHEN** capacity=10，仅 push 3 个元素
- **THEN** `get_recent(10)` 返回 3 个元素（旧→新），`len()` == 3

### Requirement: 故障触发引擎（trigger.rs）

The system SHALL provide `TriggerCondition { trigger_type, threshold, duration_ms, channel }` 与内部 `TriggerEngine`：构造时按 `duration_ms × sample_rate / 1000`（最小 1 帧）折算每条件所需连续满足帧数；每帧评估语义——OverCurrent/OverVoltage/OverFrequency：`v > threshold`；UnderVoltage：`v < threshold`；RateOfChange：`|v - v_prev| > threshold`；DigitalEvent：数字量上升沿（false→true）；Manual：不自动触发（由 `start_recording()` 显式触发）；条件连续满足达所需帧数即触发，同帧多条件命中按配置顺序取首个（蓝图 §4.4 优先级）；触发后该条件计数复位。

#### Scenario: 持续帧数触发（duration_ms 语义，D12）
- **WHEN** 过流阈值 100A、duration_ms=10、sample_rate=4000Hz（需 40 帧），连续 39 帧超阈值后回落
- **THEN** 不触发；再连续 40 帧超阈值 → 触发

#### Scenario: 变化率触发
- **WHEN** RateOfChange 阈值 50，相邻帧模拟量从 10 跳变到 80
- **THEN** |80-10|=70 > 50，该帧计入连续计数

#### Scenario: 数字量上升沿触发
- **WHEN** DigitalEvent 条件绑定数字通道，该通道 false→true
- **THEN** 该帧命中；持续为 true 不再重复命中

### Requirement: COMTRADE 文件生成（comtrade_writer.rs）

The system SHALL provide 无状态 `ComtradeWriter`：`write_cfg(config, channels, total_samples, sample_rate, time_str) -> String` 生成 C37.111-2013 ASCII 配置——第 1 行 `station_name,device_id,rev_year`；第 2 行 `TT,nA,nD`（带 A/D 后缀，D6）；模拟量通道行 13 字段 `An,ch_id,ph,,uu,a,b,0,-32767,32767,1,1,P`（a=scale_factor、b=offset 必须写出，D8）；数字量通道行 `Dn,ch_id,,,0`；随后线路频率行（50）、采样率档数行（1）、`sample_rate,total_samples` 档行（D7）；两行时间戳 `dd/mm/yyyy,hh:mm:ss.ssssss`（由 time_str 承载，D7/D11）；文件类型行（ASCII/BINARY/BINARY32）；时标乘数行（1）。`write_dat(records, channels, format) -> Vec<u8>`：ASCII 格式逐行 `sample_num,timestamp_us,raw…,dig…`；BINARY 格式 sample_num u32 LE + timestamp u32 LE + 模拟量 i16 LE + 数字量 16 位字打包；BINARY32 模拟量 i32 LE（D9：BINARY32 为 32 位整数而非 f32）。所有格式模拟量一律按 `raw = round((v - b) / a)` 逆变换为整数量化值并钳位目标位宽（a==0 按 1 处理，D9）。

#### Scenario: .cfg 头部合规（C37.111-2013，D6/D7）
- **WHEN** 2 模拟量 + 1 数字量通道、4000Hz、800 采样生成 cfg
- **THEN** 第 2 行为 `3,2A,1D`；含线路频率行 `50`；档数行 `1`；档行 `4000,800`；时间戳行含日期（`dd/mm/yyyy,hh:mm:ss.ssssss`）

#### Scenario: 模拟量缩放写出（D8）
- **WHEN** ChannelConfig.scale_factor=0.1、offset=0.0
- **THEN** 模拟量通道行第 6/7 字段为 `0.1,0`

#### Scenario: BINARY 逆变换量化（D9）
- **WHEN** a=0.1、b=0、采样值 v=12.34，BINARY 格式
- **THEN** 写入 i16 LE = round(12.34/0.1)=123；v 超 i16 范围时钳位 ±32767

#### Scenario: 数字量 16 位打包（D9）
- **WHEN** 3 个数字通道值 [true,false,true]，BINARY 格式
- **THEN** 打包为 1 个 u16 LE 字 = 0b101 = 5（2 字节）

### Requirement: 故障录波器（lib.rs FaultRecorder）

The system SHALL provide `FaultRecorder`：`new(config: RecorderConfig)` 校验（buffer_frames ≥ pre+post、channels 非空、sample_rate>0，否则 `InvalidConfig`）；模拟量/数字量各一条环形缓冲，按帧交错存储（帧 i 的通道 c 位于 i×n_ch+c，D10），时间戳独立 u64 缓冲；`push_sample(analog, digital, timestamp_us)` 长度不匹配返回 `ChannelMismatch`，写缓冲后在 Idle 态评估触发引擎，命中则记录触发帧号并转 Recording（remaining=post_fault_samples），Recording 态每帧 remaining-1、归零转 Ready；`check_triggers()` 返回当前锁存触发的引用（蓝图接口保留，D12）；`start_recording()` 显式触发（Manual 入口）；`export_comtrade(sink, base_path, time_str)` 仅 Ready 态可用（否则 `NotReady`），截取触发点前 pre + 后 post 帧窗口重建 `Vec<SampleRecord>`（sample_num 从 1 起、timestamp_us 为相对首帧微秒 u32），经 sink 写 `{base_path}.cfg` 与 `{base_path}.dat`，record_count+1 并复位 Idle。

#### Scenario: 完整录波流程（蓝图 §4.3 流程图）
- **WHEN** 4000Hz、pre=40、post=40，推 100 帧正常数据后注入 40 帧过流，再推 40 帧
- **THEN** 状态 Idle→Recording→Ready；export 后 MockSink 含 `.cfg`+`.dat` 两文件，dat 含 80 条记录，record_count==1，状态回 Idle

#### Scenario: 未就绪拒绝导出
- **WHEN** Idle 或 Recording 态调用 export_comtrade
- **THEN** 返回 `Err(RecorderError::NotReady)`，sink 无写入

#### Scenario: 触发检测性能（蓝图 §6.3/§7.2）
- **WHEN** 4000Hz 帧率下连续 4000 次 push_sample（含触发评估，cfg(test) Instant 口径，D12）
- **THEN** 总耗时 < 1000ms（等效单帧 < 0.25ms，满足触发检测 < 1ms）

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/fault_recorder/` → `crates/protocols/fault-recorder/`（eneros-fault-recorder） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；录波为设备协议族基础服务，与 soe-engine（事件触发引擎）同 protocols 子系统先例 |
| **D2** | 蓝图 `docs/phase2/comtrade.md` → `docs/protocols/fault-recorder-comtrade-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/comtrade_parse.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.108.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 删除蓝图 `fs::write(path, ...)` 直接文件调用；新增 `FileSink` trait（`write_file(path, data)`）+ `MockSink`（置于 lib.rs，记录写入路径与字节）；真实 littlefs2 接线在集成层 | no_std 无 `std::fs`；主机可测；与 v0.106.0 D4 MmsTransport / v0.107.0 D4 L2Transport 同先例；`ComtradeWriter` 改为返回 String/Vec<u8> 纯函数 |
| **D5** | 蓝图 `RingSampleBuffer { data: Box<[T]> }` → `Vec<T>` 固定容量 | no_std 下 `Vec::with_capacity` 更直观（v0.108.0 D6 同先例） |
| **D6** | 蓝图 bug 修复①：cfg 第 2 行 `{n},{nA},{nD}` 缺 A/D 后缀 → 补 C37.111 合规 `TT,nA,nD` 格式 | C37.111-2013 §5.4 要求通道计数行带 A/D 后缀（如 `3,2A,1D`）；缺后缀标准工具解析失败 |
| **D7** | 蓝图 bug 修复②：cfg 结构缺行/错序——补线路频率行（50）；档数行为采样率档数（1）而非采样率值；档行 `samp_rate,total_samples`（蓝图写成 `total_samples,sample_rate` 错序且把采样率当档数）；时间戳行补 `dd/mm/yyyy,hh:mm:ss.ssssss`（蓝图仅 `hh:mm:ss` 无日期，不合规）；删除蓝图无意义的 `1,1s,1` 行 | C37.111-2013 §5.5/§5.6 强制行序；蓝图各行自相矛盾，录波文件无法被标准工具解析 |
| **D8** | 蓝图 bug 修复③：模拟量通道行补全 13 字段含 a=scale_factor/b=offset（蓝图 ChannelConfig 定义了缩放却未写出）；数字量行 `Dn,ch_id,,,0` | C37.111-2013 §5.4.2 模拟量行 13 字段（An,ch_id,ph,ccbm,uu,a,b,skew,min,max,primary,secondary,PS）；缺 a/b 则二进制量化值无法还原工程量 |
| **D9** | 蓝图 bug 修复④：BINARY32 语义修正为 i32 整数 LE（蓝图误写 f32 LE，f32 对应 2013 REAL32 格式）；BINARY/BINARY32/ASCII 模拟量统一按 `raw=round((v-b)/a)` 逆变换量化并钳位（蓝图 `v as i16` 截断且忽略缩放）；数字量按 16 位字打包（蓝图按 8 位字节打包不合规）；`write_dat` 增加 `channels` 参数承载 a/b（蓝图签名缺失） | C37.111-2013 §6/附录：BINARY=i16、BINARY32=i32、数字量 16 位字；cfg 的 a/b 与 dat 量化值必须互逆，否则分析工具还原值错误 |
| **D10** | 环形缓冲多通道承载：蓝图 `analog_buf: RingSampleBuffer<f32>` 单缓冲无法区分通道 → 按帧交错存储（帧×通道数+通道索引），数字量同构，时间戳独立 `RingSampleBuffer<u64>`；对外帧级 API `push_sample`/`get_recent_frames` | 蓝图数据结构自相矛盾（多通道采样压入单 f32 流无法回放）；交错存储零额外分配、保持 T: Copy |
| **D11** | 时间注入：`push_sample(timestamp_us)` 由调用方携带时间戳；`export_comtrade(time_str)` 的 cfg 时间戳行由调用方传预格式化字符串（集成层 RTC 格式化，v0.12.0）；蓝图 `fs::write` 内写死 `00:00:00.000000` 占位 | no_std 无系统时间/日历转换（v0.107.0 D6 / v0.108.0 D9 注入先例）；录波时间戳必须来自真实 RTC 才有事故追溯价值 |
| **D12** | 错误模型 `RecorderError` = IoError / InvalidConfig / NotReady / ChannelMismatch（4 变体）；触发语义补全：duration_ms 折算连续帧数、RateOfChange 相邻差分、DigitalEvent 上升沿、Manual 仅 `start_recording()`、冲突按配置序优先级；`check_triggers()` 返回锁存触发引用（蓝图 `&self` 签名保留）；性能 <1ms 落地为 cfg(test) Instant 断言（主机口径，真实硬件为实验室项） | 蓝图 TriggerCondition 有 duration_ms 字段但零语义定义（无法落地）；错误变体覆盖各失败面（对齐 v0.107.0/v0.108.0 D10 精简风格） |

## 接口契约

```rust
// ============ crates/protocols/fault-recorder/src/lib.rs ============
pub enum RecorderError {
    IoError, InvalidConfig, NotReady, ChannelMismatch,
}  // Debug/Clone/PartialEq（D12）

pub trait FileSink {                                     // D4
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), RecorderError>;
}
pub struct MockSink { /* Vec<(String, Vec<u8>)> 写入记录 + 错误注入（测试/集成占位，D4） */ }

pub struct RecorderConfig {
    pub channels: Vec<ChannelConfig>,
    pub triggers: Vec<TriggerCondition>,
    pub comtrade: ComtradeConfig,
    pub pre_fault_samples: usize,
    pub post_fault_samples: usize,
    pub sample_rate: u32,
    pub buffer_frames: usize,          // ≥ pre + post（D10）
}

pub enum RecorderState { Idle, Recording, Ready }        // Debug/Clone/Copy/PartialEq

pub struct FaultRecorder { /* 字段私有（D10 交错双缓冲 + 时间戳缓冲 + TriggerEngine + 状态机） */ }
impl FaultRecorder {
    pub fn new(config: RecorderConfig) -> Result<Self, RecorderError>;
    pub fn push_sample(&mut self, analog: &[f32], digital: &[bool], timestamp_us: u64)
        -> Result<(), RecorderError>;
    pub fn check_triggers(&self) -> Option<&TriggerCondition>;   // 锁存触发（D12）
    pub fn start_recording(&mut self);                            // Manual 入口
    pub fn export_comtrade<S: FileSink>(&mut self, sink: &mut S,
        base_path: &str, time_str: &str) -> Result<(), RecorderError>;   // D4/D11
    pub fn state(&self) -> RecorderState;
    pub fn record_count(&self) -> usize;
}

// ============ src/ring_buffer.rs ============
pub struct RingSampleBuffer<T: Copy> { /* data: Vec<T>, capacity, write_pos, samples_written（D5） */ }
impl<T: Copy + Default> RingSampleBuffer<T> {
    pub fn new(capacity: usize) -> Self;
    pub fn push(&mut self, value: T);
    pub fn push_slice(&mut self, slice: &[T]);
    pub fn get_recent(&self, n: usize) -> Vec<T>;   // 旧→新保序
    pub fn len(&self) -> usize;
    pub fn capacity(&self) -> usize;
}

// ============ src/trigger.rs ============
pub enum TriggerType {                                  // Debug/Clone/Copy/PartialEq
    OverCurrent, OverVoltage, UnderVoltage, OverFrequency, RateOfChange, DigitalEvent, Manual,
}
pub struct TriggerCondition {                           // Debug/Clone/PartialEq
    pub trigger_type: TriggerType,
    pub threshold: f32,
    pub duration_ms: u32,
    pub channel: String,
}
// TriggerEngine 为 pub(crate) 内部实现（D12 语义：连续帧计数/差分/上升沿/配置序优先级）

// ============ src/comtrade_writer.rs ============
pub enum Phase { A, B, C, N, None }                     // Debug/Clone/Copy/PartialEq；None=数字量（蓝图约定）
pub struct ChannelConfig {                              // Debug/Clone/PartialEq
    pub channel_id: String, pub channel_name: String, pub phase: Phase,
    pub unit: String, pub scale_factor: f32, pub offset: f32,
}
pub struct ComtradeConfig {                             // Debug/Clone/PartialEq
    pub station_name: String, pub device_id: String,
    pub revision_year: u16, pub file_format: ComtradeFormat,
}
pub enum ComtradeFormat { Ascii, Binary, Binary32 }     // Debug/Clone/Copy/PartialEq
pub struct SampleRecord {                               // Debug/Clone/PartialEq
    pub sample_num: u32, pub timestamp_us: u32,
    pub analog: Vec<f32>, pub digital: Vec<bool>,
}
pub struct ComtradeWriter;                              // 无状态（D4 纯函数）
impl ComtradeWriter {
    pub fn write_cfg(config: &ComtradeConfig, channels: &[ChannelConfig],
        total_samples: usize, sample_rate: u32, time_str: &str) -> String;   // D6/D7/D8
    pub fn write_dat(records: &[SampleRecord], channels: &[ChannelConfig],
        format: ComtradeFormat) -> Vec<u8>;                                   // D9
}
impl ChannelConfig {
    pub fn phase_str(&self) -> &'static str;
    pub fn is_analog(&self) -> bool;                    // phase != Phase::None
}
```

## 测试规划（31 个，src 内嵌 #[cfg(test)]）

| 组 | 编号 | 覆盖点 |
|----|------|--------|
| ring_buffer | RB1~RB6 | push+get_recent 基本序 / 溢出覆盖保序 / 未写满读取 / capacity=1 / push_slice / len+capacity |
| trigger | TG7~TG13 | 过流阈值+duration 帧数（39 帧不触发/40 帧触发）/ 低压 v<threshold / 变化率相邻差分 / 数字量上升沿（持续 true 不重复）/ Manual 不自动触发 / 同帧多条件配置序优先级 |
| comtrade_writer | CW14~CW22 | cfg 第 1/2 行（TT,nA,nD）/ 模拟量 13 字段含 a/b / 数字量行 / 频率+档数+档行+时间戳行序 / ASCII dat 行格式 / BINARY 布局（u32+u32+i16×n+16 位字）/ BINARY 逆变换量化+钳位 / BINARY32 i32 / 数字量 16 位打包 |
| recorder | FR23~FR30 | new 校验 InvalidConfig（buffer<pre+post）/ push_sample 长度 ChannelMismatch / 全流程 Idle→Recording→Ready→export→Idle / 导出文件内容（cfg+dat 记录数=pre+post）/ NotReady 拒绝 / record_count 递增 / 手动 start_recording / 窗口数据正确性（触发点前后值回放一致） |
| perf | PERF31 | 4000 次 push_sample（含触发评估）< 1000ms（cfg(test) Instant，D12） |
