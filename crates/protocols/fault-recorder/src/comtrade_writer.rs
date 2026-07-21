//! COMTRADE（IEEE C37.111-2013）`.cfg` / `.dat` 文件生成（无状态纯函数，D4/D6~D9）。
//!
//! - `write_cfg`：ASCII 配置（通道计数行 `TT,nA,nD` 带 A/D 后缀 D6；线路频率行 +
//!   档数行 + `samp_rate,total_samples` 档行 + 双时间戳行行序 D7；模拟量行 13 字段
//!   含 a=scale_factor/b=offset D8）
//! - `write_dat`：ASCII / BINARY（i16 LE）/ BINARY32（i32 LE，D9）三种格式；
//!   模拟量一律按 `raw = round((v - b) / a)` 逆变换量化并钳位目标位宽（a==0 按 1 处理）；
//!   数字量按 16 位字打包（位 i = 通道 i，1 = true；不足 16 位写完整字）

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 相别（`None` 表示数字量通道，蓝图约定）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// A 相。
    A,
    /// B 相。
    B,
    /// C 相。
    C,
    /// 中性线。
    N,
    /// 无相别（数字量通道）。
    None,
}

/// 通道配置（模拟量 `phase != None`；数字量 `phase == None`）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelConfig {
    /// 通道 ID（COMTRADE ch_id，触发条件按此匹配）。
    pub channel_id: String,
    /// 通道名称。
    pub channel_name: String,
    /// 相别。
    pub phase: Phase,
    /// 单位（如 "A"/"V"）。
    pub unit: String,
    /// 缩放系数 a（工程量 = a × raw + b）。
    pub scale_factor: f32,
    /// 偏移 b。
    pub offset: f32,
}

impl ChannelConfig {
    /// COMTRADE 相别字段（A/B/C/N；数字量为空串）。
    pub fn phase_str(&self) -> &'static str {
        match self.phase {
            Phase::A => "A",
            Phase::B => "B",
            Phase::C => "C",
            Phase::N => "N",
            Phase::None => "",
        }
    }

    /// 是否模拟量通道（`phase != None`）。
    pub fn is_analog(&self) -> bool {
        self.phase != Phase::None
    }
}

/// COMTRADE 站点配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ComtradeConfig {
    /// 厂站名称。
    pub station_name: String,
    /// 装置 ID。
    pub device_id: String,
    /// 标准修订年（如 2013）。
    pub revision_year: u16,
    /// 数据文件格式。
    pub file_format: ComtradeFormat,
}

/// COMTRADE 数据文件格式（BINARY32 为 32 位整数 LE，D9）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComtradeFormat {
    /// ASCII 文本。
    Ascii,
    /// 二进制（模拟量 i16 LE）。
    Binary,
    /// 二进制（模拟量 i32 LE）。
    Binary32,
}

/// 单帧采样记录（导出窗口重建结果）。
#[derive(Debug, Clone, PartialEq)]
pub struct SampleRecord {
    /// 采样序号（从 1 起）。
    pub sample_num: u32,
    /// 相对首帧时间戳（微秒）。
    pub timestamp_us: u32,
    /// 模拟量通道值（与 `channels` 中模拟量子序列一一对应）。
    pub analog: Vec<f32>,
    /// 数字量通道值。
    pub digital: Vec<bool>,
}

/// 无状态 COMTRADE 写盘器（纯函数，D4）。
pub struct ComtradeWriter;

impl ComtradeWriter {
    /// 生成 `.cfg` 配置内容（每行 `\n` 结尾，行序严格按 C37.111-2013 §5.4~5.6）。
    pub fn write_cfg(
        config: &ComtradeConfig,
        channels: &[ChannelConfig],
        total_samples: usize,
        sample_rate: u32,
        time_str: &str,
    ) -> String {
        let n_analog = channels.iter().filter(|c| c.is_analog()).count();
        let n_digital = channels.len() - n_analog;
        let mut s = String::new();
        s.push_str(&format!(
            "{},{},{}\n",
            config.station_name, config.device_id, config.revision_year
        ));
        let total = n_analog + n_digital;
        s.push_str(&format!("{total},{n_analog}A,{n_digital}D\n"));
        let mut an = 0usize;
        let mut dn = 0usize;
        for ch in channels {
            if ch.is_analog() {
                an += 1;
                s.push_str(&format!(
                    "A{an},{},{},,{},{},{},0,-32767,32767,1,1,P\n",
                    ch.channel_id,
                    ch.phase_str(),
                    ch.unit,
                    ch.scale_factor,
                    ch.offset
                ));
            } else {
                dn += 1;
                s.push_str(&format!("D{dn},{},,,0\n", ch.channel_id));
            }
        }
        s.push_str("50\n");
        s.push_str("1\n");
        s.push_str(&format!("{sample_rate},{total_samples}\n"));
        s.push_str(time_str);
        s.push('\n');
        s.push_str(time_str);
        s.push('\n');
        s.push_str(match config.file_format {
            ComtradeFormat::Ascii => "ASCII",
            ComtradeFormat::Binary => "BINARY",
            ComtradeFormat::Binary32 => "BINARY32",
        });
        s.push('\n');
        s.push_str("1\n");
        s
    }

    /// 生成 `.dat` 数据内容（ASCII 文本或二进制字节流）。
    pub fn write_dat(
        records: &[SampleRecord],
        channels: &[ChannelConfig],
        format: ComtradeFormat,
    ) -> Vec<u8> {
        match format {
            ComtradeFormat::Ascii => Self::write_dat_ascii(records, channels),
            ComtradeFormat::Binary => Self::write_dat_binary(records, channels, false),
            ComtradeFormat::Binary32 => Self::write_dat_binary(records, channels, true),
        }
    }

    /// ASCII 数据行：`sample_num,timestamp_us,raw…,dig…`（`\n` 结尾）。
    fn write_dat_ascii(records: &[SampleRecord], channels: &[ChannelConfig]) -> Vec<u8> {
        let analog_chs: Vec<&ChannelConfig> = channels.iter().filter(|c| c.is_analog()).collect();
        let mut out = String::new();
        for rec in records {
            out.push_str(&format!("{},{}", rec.sample_num, rec.timestamp_us));
            for (i, ch) in analog_chs.iter().enumerate() {
                let v = rec.analog.get(i).copied().unwrap_or(0.0);
                let raw = quantize(v, ch.scale_factor, ch.offset) as i64;
                out.push_str(&format!(",{raw}"));
            }
            for &d in &rec.digital {
                out.push_str(if d { ",1" } else { ",0" });
            }
            out.push('\n');
        }
        out.into_bytes()
    }

    /// 二进制数据：sample_num u32 LE + timestamp u32 LE + 模拟量 i16/i32 LE
    /// + 数字量 16 位字打包（`wide = true` 时模拟量按 i32，D9）。
    fn write_dat_binary(
        records: &[SampleRecord],
        channels: &[ChannelConfig],
        wide: bool,
    ) -> Vec<u8> {
        let analog_chs: Vec<&ChannelConfig> = channels.iter().filter(|c| c.is_analog()).collect();
        let n_digital = channels.len() - analog_chs.len();
        let n_words = n_digital.div_ceil(16);
        let mut out = Vec::new();
        for rec in records {
            out.extend_from_slice(&rec.sample_num.to_le_bytes());
            out.extend_from_slice(&rec.timestamp_us.to_le_bytes());
            for (i, ch) in analog_chs.iter().enumerate() {
                let v = rec.analog.get(i).copied().unwrap_or(0.0);
                let raw = quantize(v, ch.scale_factor, ch.offset);
                if wide {
                    let clamped = raw.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
                    out.extend_from_slice(&clamped.to_le_bytes());
                } else {
                    let clamped = raw.clamp(-32767.0, 32767.0) as i16;
                    out.extend_from_slice(&clamped.to_le_bytes());
                }
            }
            for w in 0..n_words {
                let mut word: u16 = 0;
                for bit in 0..16usize {
                    let idx = w * 16 + bit;
                    if idx < rec.digital.len() && rec.digital[idx] {
                        word |= 1u16 << bit;
                    }
                }
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
        out
    }
}

/// 模拟量逆变换量化：`raw = round((v - b) / a)`；a == 0 按 1 处理（D9）。
///
/// no_std 下 `f64::round` 不可用（libm 方法），手动实现四舍五入：
/// 加 ±0.5 后向零截断取整，等价 half away from zero。
fn quantize(v: f32, a: f32, b: f32) -> f64 {
    let denom = if a == 0.0 { 1.0 } else { f64::from(a) };
    let x = (f64::from(v) - f64::from(b)) / denom;
    let adj = x + if x >= 0.0 { 0.5 } else { -0.5 };
    adj - (adj % 1.0)
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{
        ChannelConfig, ComtradeConfig, ComtradeFormat, ComtradeWriter, Phase, SampleRecord,
    };

    fn analog_ch(id: &str, a: f32, b: f32) -> ChannelConfig {
        ChannelConfig {
            channel_id: String::from(id),
            channel_name: String::from(id),
            phase: Phase::A,
            unit: String::from("A"),
            scale_factor: a,
            offset: b,
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

    fn cfg(fmt: ComtradeFormat) -> ComtradeConfig {
        ComtradeConfig {
            station_name: String::from("SubA"),
            device_id: String::from("FR01"),
            revision_year: 2013,
            file_format: fmt,
        }
    }

    const TS: &str = "20/07/2026,10:00:00.123456";

    fn three_channels() -> Vec<ChannelConfig> {
        vec![
            analog_ch("Ia", 0.1, 0.0),
            analog_ch("Ib", 1.0, 2.0),
            digital_ch("CB"),
        ]
    }

    fn cfg_lines() -> Vec<String> {
        let text = ComtradeWriter::write_cfg(
            &cfg(ComtradeFormat::Ascii),
            &three_channels(),
            800,
            4000,
            TS,
        );
        text.lines().map(String::from).collect()
    }

    #[test]
    fn cw14_cfg_header_lines() {
        let lines = cfg_lines();
        assert_eq!(lines[0], "SubA,FR01,2013");
        assert_eq!(lines[1], "3,2A,1D");
    }

    #[test]
    fn cw15_analog_line_13_fields_with_ab() {
        let lines = cfg_lines();
        let fields: Vec<&str> = lines[2].split(',').collect();
        assert_eq!(fields.len(), 13);
        assert_eq!(fields[0], "A1");
        assert_eq!(fields[1], "Ia");
        assert_eq!(fields[2], "A");
        assert_eq!(fields[3], "");
        assert_eq!(fields[4], "A");
        assert_eq!(fields[5], "0.1");
        assert_eq!(fields[6], "0");
        assert_eq!(fields[8], "-32767");
        assert_eq!(fields[9], "32767");
        assert_eq!(fields[12], "P");
    }

    #[test]
    fn cw16_digital_line_format() {
        let lines = cfg_lines();
        assert_eq!(lines[4], "D1,CB,,,0");
    }

    #[test]
    fn cw17_cfg_line_order_complete() {
        let lines = cfg_lines();
        assert_eq!(lines[5], "50");
        assert_eq!(lines[6], "1");
        assert_eq!(lines[7], "4000,800");
        assert_eq!(lines[8], TS);
        assert_eq!(lines[9], TS);
        assert_eq!(lines[10], "ASCII");
        assert_eq!(lines[11], "1");
        assert_eq!(lines.len(), 12);
    }

    #[test]
    fn cw18_ascii_dat_line_content() {
        let records = vec![SampleRecord {
            sample_num: 1,
            timestamp_us: 250,
            analog: vec![12.34, 5.0],
            digital: vec![true, false],
        }];
        let chs = vec![
            analog_ch("Ia", 0.1, 0.0),
            analog_ch("Ib", 1.0, 2.0),
            digital_ch("CB"),
            digital_ch("CB2"),
        ];
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Ascii);
        // Ia: round(12.34/0.1)=123；Ib: round((5-2)/1)=3；数字量 1,0
        assert_eq!(dat, b"1,250,123,3,1,0\n".to_vec());
    }

    #[test]
    fn cw19_binary_layout_fields() {
        let records = vec![SampleRecord {
            sample_num: 0x0102_0304,
            timestamp_us: 0x0506_0708,
            analog: vec![10.0, 20.0],
            digital: vec![true],
        }];
        let chs = vec![
            analog_ch("Ia", 1.0, 0.0),
            analog_ch("Ib", 1.0, 0.0),
            digital_ch("CB"),
        ];
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Binary);
        // 8 + 2×2 + 2 = 14 字节/记录
        assert_eq!(dat.len(), 14);
        assert_eq!(
            u32::from_le_bytes([dat[0], dat[1], dat[2], dat[3]]),
            0x0102_0304
        );
        assert_eq!(
            u32::from_le_bytes([dat[4], dat[5], dat[6], dat[7]]),
            0x0506_0708
        );
        assert_eq!(i16::from_le_bytes([dat[8], dat[9]]), 10);
        assert_eq!(i16::from_le_bytes([dat[10], dat[11]]), 20);
        assert_eq!(u16::from_le_bytes([dat[12], dat[13]]), 1);
    }

    #[test]
    fn cw20_binary_quantize_and_clamp() {
        let chs = vec![analog_ch("Ia", 0.1, 0.0), analog_ch("Ib", 1.0, 0.0)];
        let records = vec![SampleRecord {
            sample_num: 1,
            timestamp_us: 0,
            analog: vec![12.34, 1.0e6],
            digital: vec![],
        }];
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Binary);
        assert_eq!(i16::from_le_bytes([dat[8], dat[9]]), 123);
        assert_eq!(i16::from_le_bytes([dat[10], dat[11]]), 32767);
        let records = vec![SampleRecord {
            sample_num: 1,
            timestamp_us: 0,
            analog: vec![12.34, -1.0e6],
            digital: vec![],
        }];
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Binary);
        assert_eq!(i16::from_le_bytes([dat[10], dat[11]]), -32767);
    }

    #[test]
    fn cw21_binary32_i32_le() {
        let chs = vec![analog_ch("Ia", 0.1, 0.0)];
        let records = vec![SampleRecord {
            sample_num: 1,
            timestamp_us: 0,
            analog: vec![12.34],
            digital: vec![],
        }];
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Binary32);
        // 8 + 4 = 12 字节；模拟量 i32 LE
        assert_eq!(dat.len(), 12);
        assert_eq!(i32::from_le_bytes([dat[8], dat[9], dat[10], dat[11]]), 123);
    }

    #[test]
    fn cw22_digital_packed_u16_le() {
        let chs = vec![digital_ch("D0"), digital_ch("D1"), digital_ch("D2")];
        let records = vec![SampleRecord {
            sample_num: 1,
            timestamp_us: 0,
            analog: vec![],
            digital: vec![true, false, true],
        }];
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Binary);
        // [true,false,true] → 0b101 = 5，u16 LE
        assert_eq!(dat.len(), 10);
        assert_eq!(u16::from_le_bytes([dat[8], dat[9]]), 5);
    }

    #[test]
    fn cw23_quantize_rounds_half_up() {
        let chs = vec![analog_ch("Ia", 0.1, 0.0)];
        let records = vec![SampleRecord {
            sample_num: 1,
            timestamp_us: 0,
            analog: vec![12.36],
            digital: vec![],
        }];
        // 12.36/0.1=123.6，round=124（若为向零截断实现则为 123，可区分）
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Ascii);
        assert_eq!(dat, b"1,0,124\n".to_vec());
        let dat = ComtradeWriter::write_dat(&records, &chs, ComtradeFormat::Binary);
        assert_eq!(i16::from_le_bytes([dat[8], dat[9]]), 124);
    }
}
