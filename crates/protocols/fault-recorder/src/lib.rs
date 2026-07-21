//! EnerOS v0.109.0 故障录波 COMTRADE（P2-H 第 1 版）.
//!
//! 故障录波是电力事故追溯的法定数据源：故障时刻前后的电压/电流/开关量波形以
//! IEEE C37.111 COMTRADE 标准格式落盘，供标准分析工具解析。本 crate 在 v0.55.0
//! 高频采样、v0.24.0 文件系统、v0.108.0 安全 SV 采样源基座上，实现环形采样缓冲 +
//! 7 类故障触发条件 + COMTRADE .cfg/.dat 文件生成导出，打通「采样 → 触发 → 录波 →
//! 导出」链路，为 v0.110.0 云边同步提供可上传的录波文件。
//!
//! # 核心类型
//!
//! - [`FaultRecorder`] — 故障录波器（Idle→Recording→Ready 状态机，D10 交错双缓冲）
//! - [`RecorderConfig`] / [`RecorderState`] — 录波配置 / 状态机
//! - [`RecorderError`] — 错误枚举（IoError / InvalidConfig / NotReady / ChannelMismatch，D12）
//! - [`FileSink`] / [`MockSink`] — 文件落盘抽象 + mock 实现（D4）
//! - [`ring_buffer::RingSampleBuffer`] — 固定容量环形采样缓冲（溢出覆盖最旧，D5）
//! - [`trigger::TriggerType`] / [`trigger::TriggerCondition`] — 7 类触发条件（D12）
//! - [`comtrade_writer::ComtradeWriter`] — COMTRADE .cfg/.dat 生成（纯函数，D6~D9）
//! - [`comtrade_writer::ChannelConfig`] / [`comtrade_writer::ComtradeConfig`] /
//!   [`comtrade_writer::ComtradeFormat`] / [`comtrade_writer::SampleRecord`] / [`comtrade_writer::Phase`]
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/fault_recorder/` → `crates/protocols/fault-recorder/`（eneros-fault-recorder） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；录波为设备协议族基础服务，与 soe-engine（事件触发引擎）同 protocols 子系统先例 |
//! | **D2** | 蓝图 `docs/phase2/comtrade.md` → `docs/protocols/fault-recorder-comtrade-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/comtrade_parse.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.108.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 删除蓝图 `fs::write(path, ...)` 直接文件调用；新增 `FileSink` trait（`write_file(path, data)`）+ `MockSink`（置于 lib.rs，记录写入路径与字节）；真实 littlefs2 接线在集成层 | no_std 无 `std::fs`；主机可测；与 v0.106.0 D4 MmsTransport / v0.107.0 D4 L2Transport 同先例；`ComtradeWriter` 改为返回 String/Vec<u8> 纯函数 |
//! | **D5** | 蓝图 `RingSampleBuffer { data: Box<[T]> }` → `Vec<T>` 固定容量 | no_std 下 `Vec::with_capacity` 更直观（v0.108.0 D6 同先例） |
//! | **D6** | 蓝图 bug 修复①：cfg 第 2 行 `{n},{nA},{nD}` 缺 A/D 后缀 → 补 C37.111 合规 `TT,nA,nD` 格式 | C37.111-2013 §5.4 要求通道计数行带 A/D 后缀（如 `3,2A,1D`）；缺后缀标准工具解析失败 |
//! | **D7** | 蓝图 bug 修复②：cfg 结构缺行/错序——补线路频率行（50）；档数行为采样率档数（1）而非采样率值；档行 `samp_rate,total_samples`（蓝图写成 `total_samples,sample_rate` 错序且把采样率当档数）；时间戳行补 `dd/mm/yyyy,hh:mm:ss.ssssss`（蓝图仅 `hh:mm:ss` 无日期，不合规）；删除蓝图无意义的 `1,1s,1` 行 | C37.111-2013 §5.5/§5.6 强制行序；蓝图各行自相矛盾，录波文件无法被标准工具解析 |
//! | **D8** | 蓝图 bug 修复③：模拟量通道行补全 13 字段含 a=scale_factor/b=offset（蓝图 ChannelConfig 定义了缩放却未写出）；数字量行 `Dn,ch_id,,,0` | C37.111-2013 §5.4.2 模拟量行 13 字段（An,ch_id,ph,ccbm,uu,a,b,skew,min,max,primary,secondary,PS）；缺 a/b 则二进制量化值无法还原工程量 |
//! | **D9** | 蓝图 bug 修复④：BINARY32 语义修正为 i32 整数 LE（蓝图误写 f32 LE，f32 对应 2013 REAL32 格式）；BINARY/BINARY32/ASCII 模拟量统一按 `raw=round((v-b)/a)` 逆变换量化并钳位（蓝图 `v as i16` 截断且忽略缩放）；数字量按 16 位字打包（蓝图按 8 位字节打包不合规）；`write_dat` 增加 `channels` 参数承载 a/b（蓝图签名缺失） | C37.111-2013 §6/附录：BINARY=i16、BINARY32=i32、数字量 16 位字；cfg 的 a/b 与 dat 量化值必须互逆，否则分析工具还原值错误 |
//! | **D10** | 环形缓冲多通道承载：蓝图 `analog_buf: RingSampleBuffer<f32>` 单缓冲无法区分通道 → 按帧交错存储（帧×通道数+通道索引），数字量同构，时间戳独立 `RingSampleBuffer<u64>`；对外帧级 API `push_sample`/`get_recent_frames` | 蓝图数据结构自相矛盾（多通道采样压入单 f32 流无法回放）；交错存储零额外分配、保持 T: Copy |
//! | **D11** | 时间注入：`push_sample(timestamp_us)` 由调用方携带时间戳；`export_comtrade(time_str)` 的 cfg 时间戳行由调用方传预格式化字符串（集成层 RTC 格式化，v0.12.0）；蓝图 `fs::write` 内写死 `00:00:00.000000` 占位 | no_std 无系统时间/日历转换（v0.107.0 D6 / v0.108.0 D9 注入先例）；录波时间戳必须来自真实 RTC 才有事故追溯价值 |
//! | **D12** | 错误模型 `RecorderError` = IoError / InvalidConfig / NotReady / ChannelMismatch（4 变体）；触发语义补全：duration_ms 折算连续帧数、RateOfChange 相邻差分、DigitalEvent 上升沿、Manual 仅 `start_recording()`、冲突按配置序优先级；`check_triggers()` 返回锁存触发引用（蓝图 `&self` 签名保留）；性能 <1ms 落地为 cfg(test) Instant 断言（主机口径，真实硬件为实验室项） | 蓝图 TriggerCondition 有 duration_ms 字段但零语义定义（无法落地）；错误变体覆盖各失败面（对齐 v0.107.0/v0.108.0 D10 精简风格） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零第三方依赖，零 unsafe，零 extern "C"，
//! 不调用 `panic!` / `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod comtrade_writer;
pub mod ring_buffer;
pub mod trigger;

use alloc::string::String;
use alloc::vec::Vec;

pub use comtrade_writer::{
    ChannelConfig, ComtradeConfig, ComtradeFormat, ComtradeWriter, Phase, SampleRecord,
};
pub use ring_buffer::RingSampleBuffer;
use trigger::TriggerEngine;
pub use trigger::{TriggerCondition, TriggerType};

/// 录波错误（D12：4 变体覆盖各失败面）。
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderError {
    /// 文件落盘 I/O 错误（由 `FileSink` 上报）。
    IoError,
    /// 配置无效（channels 为空 / sample_rate 为 0 / buffer_frames < pre+post）。
    InvalidConfig,
    /// 未就绪（非 Ready 态调用 `export_comtrade`）。
    NotReady,
    /// 采样通道数与配置不匹配（analog/digital 切片长度错误）。
    ChannelMismatch,
}

/// 文件落盘抽象（D4：真实 littlefs2 接线在集成层）。
pub trait FileSink {
    /// 将 `data` 写入 `path`。
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), RecorderError>;
}

/// Mock 文件落盘（测试/集成占位，D4）：记录全部写入，可注入一次性写错误。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MockSink {
    written: Vec<(String, Vec<u8>)>,
    inject_error: bool,
}

impl MockSink {
    /// 创建空 mock。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入一次性写错误：下一次 `write_file` 返回 `Err(IoError)`。
    pub fn inject_write_error_once(&mut self) {
        self.inject_error = true;
    }

    /// 已写入文件记录（路径 + 字节，按写入顺序）。
    pub fn written(&self) -> &[(String, Vec<u8>)] {
        &self.written
    }

    /// 按路径取已写入内容。
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.written
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, d)| d.as_slice())
    }

    /// 已写入文件数。
    pub fn len(&self) -> usize {
        self.written.len()
    }

    /// 是否无写入。
    pub fn is_empty(&self) -> bool {
        self.written.is_empty()
    }
}

impl FileSink for MockSink {
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), RecorderError> {
        if self.inject_error {
            self.inject_error = false;
            return Err(RecorderError::IoError);
        }
        self.written.push((String::from(path), Vec::from(data)));
        Ok(())
    }
}

/// 录波配置。
#[derive(Debug, Clone, PartialEq)]
pub struct RecorderConfig {
    /// 通道配置（模拟量 `phase != None` 在前序语义下按出现顺序编号；数字量同）。
    pub channels: Vec<ChannelConfig>,
    /// 触发条件列表（同帧多命中按配置序取首个）。
    pub triggers: Vec<TriggerCondition>,
    /// COMTRADE 站点配置。
    pub comtrade: ComtradeConfig,
    /// 故障前采样帧数（导出窗口前半）。
    pub pre_fault_samples: usize,
    /// 故障后采样帧数（导出窗口后半；触发后再录该数量帧转 Ready）。
    pub post_fault_samples: usize,
    /// 采样率（Hz），用于 duration_ms 折算帧数与 cfg 档行。
    pub sample_rate: u32,
    /// 环形缓冲帧容量（必须 ≥ pre_fault_samples + post_fault_samples，D10）。
    pub buffer_frames: usize,
}

/// 录波状态机。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecorderState {
    /// 空闲（评估触发条件）。
    Idle,
    /// 录波中（触发后记录 post_fault_samples 帧）。
    Recording,
    /// 录波完成（可导出）。
    Ready,
}

/// 通道名 → `(is_digital, idx)` 查询（先模拟通道命名空间，后数字通道）。
fn channel_lookup(channels: &[ChannelConfig], name: &str) -> Option<(bool, usize)> {
    let mut ai = 0usize;
    for ch in channels {
        if ch.is_analog() {
            if ch.channel_id == name {
                return Some((false, ai));
            }
            ai += 1;
        }
    }
    let mut di = 0usize;
    for ch in channels {
        if !ch.is_analog() {
            if ch.channel_id == name {
                return Some((true, di));
            }
            di += 1;
        }
    }
    None
}

/// 故障录波器（D10 交错双缓冲 + 时间戳缓冲 + 触发引擎 + 状态机）。
///
/// 通道顺序约定：前 `n_analog` 个 analog 值对应 `channels` 中模拟量子序列
/// （按出现顺序），digital 值对应数字量子序列；`push_sample` 切片必须按此约定。
pub struct FaultRecorder {
    analog: RingSampleBuffer<f32>,
    digital: RingSampleBuffer<bool>,
    timestamps: RingSampleBuffer<u64>,
    n_analog: usize,
    n_digital: usize,
    channels: Vec<ChannelConfig>,
    engine: TriggerEngine,
    comtrade: ComtradeConfig,
    pre_fault_samples: usize,
    post_fault_samples: usize,
    sample_rate: u32,
    state: RecorderState,
    latched_trigger: Option<usize>,
    record_count: usize,
    remaining: usize,
    frame_count: u64,
    prev_analog: Vec<f32>,
    prev_digital: Vec<bool>,
}

impl FaultRecorder {
    /// 创建录波器；校验配置（channels 非空、sample_rate > 0、buffer_frames ≥ pre+post）。
    pub fn new(config: RecorderConfig) -> Result<Self, RecorderError> {
        if config.channels.is_empty()
            || config.sample_rate == 0
            || config.buffer_frames < config.pre_fault_samples + config.post_fault_samples
        {
            return Err(RecorderError::InvalidConfig);
        }
        let n_analog = config.channels.iter().filter(|c| c.is_analog()).count();
        let n_digital = config.channels.len() - n_analog;
        let frames = config.buffer_frames;
        Ok(Self {
            analog: RingSampleBuffer::new(frames * n_analog),
            digital: RingSampleBuffer::new(frames * n_digital),
            timestamps: RingSampleBuffer::new(frames),
            n_analog,
            n_digital,
            engine: TriggerEngine::new(config.triggers, config.sample_rate),
            channels: config.channels,
            comtrade: config.comtrade,
            pre_fault_samples: config.pre_fault_samples,
            post_fault_samples: config.post_fault_samples,
            sample_rate: config.sample_rate,
            state: RecorderState::Idle,
            latched_trigger: None,
            record_count: 0,
            remaining: 0,
            frame_count: 0,
            prev_analog: alloc::vec![0.0; n_analog],
            prev_digital: alloc::vec![false; n_digital],
        })
    }

    /// 推入一帧采样（analog/digital 长度必须与配置一致，否则 `ChannelMismatch`）。
    ///
    /// Idle 态评估触发引擎：命中则锁存触发条件并转 Recording
    /// （remaining = post_fault_samples）；Recording 态每帧 remaining-1，归零转 Ready。
    /// 首帧 prev 以当前帧自身初始化（避免 RateOfChange/DigitalEvent 边沿误判）。
    pub fn push_sample(
        &mut self,
        analog: &[f32],
        digital: &[bool],
        timestamp_us: u64,
    ) -> Result<(), RecorderError> {
        if analog.len() != self.n_analog || digital.len() != self.n_digital {
            return Err(RecorderError::ChannelMismatch);
        }
        self.analog.push_slice(analog);
        self.digital.push_slice(digital);
        self.timestamps.push(timestamp_us);
        if self.frame_count == 0 {
            self.prev_analog.copy_from_slice(analog);
            self.prev_digital.copy_from_slice(digital);
        }
        match self.state {
            RecorderState::Idle => {
                let channels = &self.channels;
                let hit = self.engine.evaluate(
                    |name| channel_lookup(channels, name),
                    analog,
                    &self.prev_analog,
                    digital,
                    &self.prev_digital,
                );
                if let Some(idx) = hit {
                    self.latched_trigger = Some(idx);
                    self.state = RecorderState::Recording;
                    self.remaining = self.post_fault_samples;
                }
            }
            RecorderState::Recording => {
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    self.state = RecorderState::Ready;
                }
            }
            RecorderState::Ready => {}
        }
        self.prev_analog.copy_from_slice(analog);
        self.prev_digital.copy_from_slice(digital);
        self.frame_count += 1;
        Ok(())
    }

    /// 当前锁存的触发条件（D12：蓝图 `&self` 签名保留）。
    pub fn check_triggers(&self) -> Option<&TriggerCondition> {
        self.latched_trigger
            .and_then(|i| self.engine.conditions().get(i))
    }

    /// 手动触发录波（Manual 入口；仅 Idle 态生效，无对应触发条件）。
    pub fn start_recording(&mut self) {
        if self.state == RecorderState::Idle {
            self.latched_trigger = None;
            self.state = RecorderState::Recording;
            self.remaining = self.post_fault_samples;
        }
    }

    /// 导出 COMTRADE 文件（仅 Ready 态；写 `{base_path}.cfg` 与 `{base_path}.dat`）。
    ///
    /// 窗口为最近 pre+post 帧：`sample_num` 从 1 起，`timestamp_us` 为相对首帧微秒。
    /// 任一写失败传播 `Err`（第一笔失败则第二笔不写）；成功后 record_count+1、
    /// 状态复位 Idle、锁存触发清空。
    pub fn export_comtrade<S: FileSink>(
        &mut self,
        sink: &mut S,
        base_path: &str,
        time_str: &str,
    ) -> Result<(), RecorderError> {
        if self.state != RecorderState::Ready {
            return Err(RecorderError::NotReady);
        }
        let window = self.pre_fault_samples + self.post_fault_samples;
        let ts = self.timestamps.get_recent(window);
        let frames = ts.len();
        let analog_vals = self.analog.get_recent(frames * self.n_analog);
        let digital_vals = self.digital.get_recent(frames * self.n_digital);
        let base_ts = ts.first().copied().unwrap_or(0);
        let mut records = Vec::with_capacity(frames);
        for i in 0..frames {
            records.push(SampleRecord {
                sample_num: i as u32 + 1,
                timestamp_us: ts[i].wrapping_sub(base_ts) as u32,
                analog: analog_vals[i * self.n_analog..(i + 1) * self.n_analog].to_vec(),
                digital: digital_vals[i * self.n_digital..(i + 1) * self.n_digital].to_vec(),
            });
        }
        let cfg_text = ComtradeWriter::write_cfg(
            &self.comtrade,
            &self.channels,
            records.len(),
            self.sample_rate,
            time_str,
        );
        let dat = ComtradeWriter::write_dat(&records, &self.channels, self.comtrade.file_format);
        sink.write_file(&alloc::format!("{base_path}.cfg"), cfg_text.as_bytes())?;
        sink.write_file(&alloc::format!("{base_path}.dat"), &dat)?;
        self.record_count += 1;
        self.state = RecorderState::Idle;
        self.latched_trigger = None;
        Ok(())
    }

    /// 当前状态。
    pub fn state(&self) -> RecorderState {
        self.state
    }

    /// 累计导出录波文件次数。
    pub fn record_count(&self) -> usize {
        self.record_count
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;

    const TS: &str = "20/07/2026,10:00:00.000000";

    fn analog_ch(id: &str) -> ChannelConfig {
        ChannelConfig {
            channel_id: String::from(id),
            channel_name: String::from(id),
            phase: Phase::A,
            unit: String::from("A"),
            scale_factor: 1.0,
            offset: 0.0,
        }
    }

    fn digital_ch(id: &str) -> ChannelConfig {
        ChannelConfig {
            channel_id: String::from(id),
            channel_name: String::from(id),
            phase: Phase::None,
            unit: String::new(),
            scale_factor: 1.0,
            offset: 0.0,
        }
    }

    fn overcurrent(threshold: f32) -> TriggerCondition {
        TriggerCondition {
            trigger_type: TriggerType::OverCurrent,
            threshold,
            duration_ms: 0,
            channel: String::from("Ia"),
        }
    }

    fn test_config(pre: usize, post: usize, rate: u32, buffer: usize) -> RecorderConfig {
        RecorderConfig {
            channels: vec![analog_ch("Ia"), digital_ch("CB")],
            triggers: vec![overcurrent(100.0)],
            comtrade: ComtradeConfig {
                station_name: String::from("SubA"),
                device_id: String::from("FR01"),
                revision_year: 2013,
                file_format: ComtradeFormat::Ascii,
            },
            pre_fault_samples: pre,
            post_fault_samples: post,
            sample_rate: rate,
            buffer_frames: buffer,
        }
    }

    #[test]
    fn fr23_new_rejects_invalid_config() {
        let mut cfg = test_config(40, 40, 1000, 50);
        assert_eq!(
            FaultRecorder::new(cfg.clone()).err(),
            Some(RecorderError::InvalidConfig)
        );
        cfg.buffer_frames = 80;
        cfg.sample_rate = 0;
        assert_eq!(
            FaultRecorder::new(cfg.clone()).err(),
            Some(RecorderError::InvalidConfig)
        );
        cfg.sample_rate = 1000;
        cfg.channels = Vec::new();
        assert_eq!(
            FaultRecorder::new(cfg).err(),
            Some(RecorderError::InvalidConfig)
        );
    }

    #[test]
    fn fr24_push_sample_channel_mismatch() {
        let mut rec = FaultRecorder::new(test_config(4, 4, 1000, 16)).unwrap();
        assert_eq!(
            rec.push_sample(&[1.0, 2.0], &[false], 0).err(),
            Some(RecorderError::ChannelMismatch)
        );
        assert_eq!(
            rec.push_sample(&[1.0], &[false, true], 0).err(),
            Some(RecorderError::ChannelMismatch)
        );
        assert!(rec.push_sample(&[1.0], &[false], 0).is_ok());
    }

    /// 推 4 正常帧 → 过流触发帧 → 4 故障后帧，走到 Ready。
    fn drive_to_ready(rec: &mut FaultRecorder) {
        for i in 0..4u64 {
            rec.push_sample(&[10.0], &[false], i * 250).unwrap();
        }
        assert_eq!(rec.state(), RecorderState::Idle);
        rec.push_sample(&[200.0], &[true], 1000).unwrap();
        assert_eq!(rec.state(), RecorderState::Recording);
        for i in 0..4u64 {
            rec.push_sample(&[10.0], &[false], 1250 + i * 250).unwrap();
        }
        assert_eq!(rec.state(), RecorderState::Ready);
    }

    #[test]
    fn fr25_full_flow_idle_recording_ready_export() {
        let mut rec = FaultRecorder::new(test_config(4, 4, 1000, 16)).unwrap();
        drive_to_ready(&mut rec);
        let trig = rec.check_triggers().unwrap();
        assert_eq!(trig.trigger_type, TriggerType::OverCurrent);
        let mut sink = MockSink::new();
        rec.export_comtrade(&mut sink, "rec1", TS).unwrap();
        assert_eq!(sink.len(), 2);
        assert!(sink.get("rec1.cfg").is_some());
        let dat = sink.get("rec1.dat").unwrap();
        let text = String::from_utf8(dat.to_vec()).unwrap();
        assert_eq!(text.lines().count(), 8);
        assert_eq!(rec.record_count(), 1);
        assert_eq!(rec.state(), RecorderState::Idle);
        assert!(rec.check_triggers().is_none());
    }

    #[test]
    fn fr26_export_dat_contains_pre_plus_post_records() {
        let mut rec = FaultRecorder::new(test_config(4, 4, 1000, 16)).unwrap();
        drive_to_ready(&mut rec);
        let mut sink = MockSink::new();
        rec.export_comtrade(&mut sink, "rec2", TS).unwrap();
        let cfg_text = String::from_utf8(sink.get("rec2.cfg").unwrap().to_vec()).unwrap();
        let lines: Vec<&str> = cfg_text.lines().collect();
        assert_eq!(lines[1], "2,1A,1D");
        assert_eq!(lines[6], "1000,8");
        let dat_text = String::from_utf8(sink.get("rec2.dat").unwrap().to_vec()).unwrap();
        assert_eq!(dat_text.lines().count(), 4 + 4);
    }

    #[test]
    fn fr27_export_not_ready_rejected_and_sink_stays_empty() {
        let mut rec = FaultRecorder::new(test_config(4, 4, 1000, 16)).unwrap();
        let mut sink = MockSink::new();
        assert_eq!(
            rec.export_comtrade(&mut sink, "x", TS).err(),
            Some(RecorderError::NotReady)
        );
        // Recording 态同样拒绝
        rec.push_sample(&[200.0], &[false], 0).unwrap();
        assert_eq!(rec.state(), RecorderState::Recording);
        assert_eq!(
            rec.export_comtrade(&mut sink, "x", TS).err(),
            Some(RecorderError::NotReady)
        );
        assert!(sink.is_empty());
    }

    #[test]
    fn fr28_record_count_increments_across_exports() {
        let mut rec = FaultRecorder::new(test_config(4, 4, 1000, 16)).unwrap();
        let mut sink = MockSink::new();
        drive_to_ready(&mut rec);
        rec.export_comtrade(&mut sink, "r1", TS).unwrap();
        assert_eq!(rec.record_count(), 1);
        // 第二次：手动触发再录
        rec.start_recording();
        for i in 0..4u64 {
            rec.push_sample(&[10.0], &[false], 3000 + i * 250).unwrap();
        }
        assert_eq!(rec.state(), RecorderState::Ready);
        rec.export_comtrade(&mut sink, "r2", TS).unwrap();
        assert_eq!(rec.record_count(), 2);
        assert_eq!(sink.len(), 4);
    }

    #[test]
    fn fr29_start_recording_manual_trigger() {
        let mut rec = FaultRecorder::new(test_config(2, 3, 1000, 8)).unwrap();
        rec.push_sample(&[1.0], &[false], 0).unwrap();
        rec.start_recording();
        assert_eq!(rec.state(), RecorderState::Recording);
        assert!(rec.check_triggers().is_none());
        for i in 0..3u64 {
            rec.push_sample(&[1.0], &[false], 250 + i * 250).unwrap();
        }
        assert_eq!(rec.state(), RecorderState::Ready);
        // 非 Idle 态调用不生效
        rec.start_recording();
        assert_eq!(rec.state(), RecorderState::Ready);
    }

    #[test]
    fn fr30_export_window_replays_trigger_context_values() {
        let mut cfg = test_config(2, 2, 1000, 8);
        cfg.channels = vec![analog_ch("Ia")];
        cfg.triggers = vec![overcurrent(50.0)];
        let mut rec = FaultRecorder::new(cfg).unwrap();
        rec.push_sample(&[1.0], &[], 0).unwrap();
        rec.push_sample(&[2.0], &[], 250).unwrap();
        rec.push_sample(&[100.0], &[], 500).unwrap(); // 触发帧
        rec.push_sample(&[4.0], &[], 750).unwrap();
        rec.push_sample(&[5.0], &[], 1000).unwrap();
        assert_eq!(rec.state(), RecorderState::Ready);
        let mut sink = MockSink::new();
        rec.export_comtrade(&mut sink, "w", TS).unwrap();
        let dat = String::from_utf8(sink.get("w.dat").unwrap().to_vec()).unwrap();
        // 窗口 = 最近 4 帧（触发前 1 帧 + 触发帧 + 触发后 2 帧）：2.0/100.0/4.0/5.0
        assert_eq!(dat, "1,0,2\n2,250,100\n3,500,4\n4,750,5\n");
    }

    #[test]
    fn perf31_trigger_eval_4000_frames_under_1000ms() {
        let mut cfg = test_config(100, 100, 4000, 5000);
        cfg.channels = vec![
            analog_ch("Ia"),
            analog_ch("Ua"),
            digital_ch("CB"),
            digital_ch("CB2"),
        ];
        cfg.triggers = vec![
            overcurrent(100.0),
            TriggerCondition {
                trigger_type: TriggerType::UnderVoltage,
                threshold: 50.0,
                duration_ms: 5,
                channel: String::from("Ua"),
            },
            TriggerCondition {
                trigger_type: TriggerType::RateOfChange,
                threshold: 500.0,
                duration_ms: 0,
                channel: String::from("Ia"),
            },
            TriggerCondition {
                trigger_type: TriggerType::DigitalEvent,
                threshold: 0.0,
                duration_ms: 0,
                channel: String::from("CB"),
            },
        ];
        let mut rec = FaultRecorder::new(cfg).unwrap();
        let start = std::time::Instant::now();
        for i in 0..4000u64 {
            let a = [10.0 + (i % 7) as f32, 220.0];
            let d = [false, true];
            rec.push_sample(&a, &d, i * 250).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 1000,
            "4000 push_sample took {elapsed:?}"
        );
    }
}
