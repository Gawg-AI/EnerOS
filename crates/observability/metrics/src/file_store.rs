//! 指标 JSONL 文件导出后端（33 OB-07：std feature 门控的仓内可运行导出链）.
//!
//! 指标不再只有内存环形缓冲——`FileMetricsStore` 以追加式 JSONL 落盘
//! （每条数据点一行），支持最新值读取与时间窗查询，构成
//! record → flush → JSONL 可读回 的 host 端到端商用验收证据链。
//!
//! no_std 合规：本模块整体 `#[cfg(feature = "std")]` 门控，默认（无 std
//! feature）构建不进入编译，交叉编译 `aarch64-unknown-none` 不受影响。

#![cfg(feature = "std")]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

use heapless::String as HString;

use crate::metrics::{Metric, MetricType, MetricValue, MetricsError};
use crate::store::MetricsStore;

/// 追加型 JSONL 指标存储：每条数据点一行，进程重启不丢（33 OB-07）.
pub struct FileMetricsStore {
    /// 文件路径.
    path: String,
}

impl FileMetricsStore {
    /// 打开（不存在则创建）JSONL 追加文件；路径不可用返回
    /// [`MetricsError::IoFailed`].
    pub fn open(path: &str) -> Result<Self, MetricsError> {
        // 先验证路径可写（空文件预创建，append 语义在 append() 中维持）
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|_| MetricsError::IoFailed)?;
        Ok(Self {
            path: path.to_string(),
        })
    }

    /// 从文件读回全部数据点（JSONL 反序列化；空行跳过）.
    pub fn read_all(&self) -> Result<Vec<Metric>, MetricsError> {
        read_all(&self.path)
    }
}

/// 从 JSONL 文件读回全部数据点.
fn read_all(path: &str) -> Result<Vec<Metric>, MetricsError> {
    let f = File::open(path).map_err(|_| MetricsError::IoFailed)?;
    let reader = BufReader::new(f);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|_| MetricsError::IoFailed)?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(parse_metric_json(&line)?);
    }
    Ok(out)
}

impl MetricsStore for FileMetricsStore {
    fn append(&mut self, m: &Metric) -> Result<(), MetricsError> {
        let line = metric_to_json(m);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|_| MetricsError::IoFailed)?;
        f.write_all(line.as_bytes())
            .and_then(|()| f.write_all(b"\n"))
            .map_err(|_| MetricsError::IoFailed)
    }

    fn get_latest(&self, name: &str) -> Option<u64> {
        let all = read_all(&self.path).ok()?;
        all.into_iter()
            .rfind(|m| m.name.as_str() == name)
            .map(|m| match m.value {
                MetricValue::U64(v) => v,
                MetricValue::I64(v) => v as u64,
                MetricValue::F64(v) => v as u64,
            })
    }

    fn query_range(
        &self,
        name: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<Vec<Metric>, MetricsError> {
        if start_ms > end_ms {
            return Err(MetricsError::QueryRangeInvalid);
        }
        let all = read_all(&self.path)?;
        let mut out: Vec<Metric> = all
            .into_iter()
            .filter(|m| {
                m.name.as_str() == name && m.timestamp_ms >= start_ms && m.timestamp_ms <= end_ms
            })
            .collect();
        // 按时间戳升序（与 MemoryMetricsStore 语义一致）
        out.sort_by_key(|m| m.timestamp_ms);
        Ok(out)
    }
}

// ===== JSON 序列化 / 反序列化（本 crate 自定义的固定结构） =====

/// JSON 字符串转义：`"` 与 `\` 转义，控制字符转 `\u00XX`.
fn escape_json_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// 指标类型标签（与 `MetricType` 一一对应）.
fn kind_label(t: MetricType) -> &'static str {
    match t {
        MetricType::Counter => "COUNTER",
        MetricType::Gauge => "GAUGE",
        MetricType::Histogram => "HISTOGRAM",
        MetricType::Summary => "SUMMARY",
    }
}

/// 序列化指标数据点为一行 JSON.
fn metric_to_json(m: &Metric) -> String {
    let mut json = String::new();
    json.push_str("{\"name\":\"");
    escape_json_into(&mut json, m.name.as_str());
    json.push_str("\",\"kind\":\"");
    json.push_str(kind_label(m.metric_type));
    json.push_str("\",\"value\":{");
    match &m.value {
        MetricValue::U64(v) => {
            json.push_str("\"u\":");
            let _ = write!(json, "{}", v);
        }
        MetricValue::I64(v) => {
            json.push_str("\"i\":");
            let _ = write!(json, "{}", v);
        }
        MetricValue::F64(v) => {
            json.push_str("\"f\":");
            let _ = write!(json, "{}", v);
        }
    }
    json.push_str("},\"ts\":");
    let _ = write!(json, "{}", m.timestamp_ms);
    json.push_str(",\"labels\":[");
    for (i, l) in m.labels.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"k\":\"");
        escape_json_into(&mut json, l.key.as_str());
        json.push_str("\",\"v\":\"");
        escape_json_into(&mut json, l.value.as_str());
        json.push_str("\"}");
    }
    json.push_str("]}");
    json
}

/// 将超长 `&str` 截断转换为定容 HString.
fn hstr<const N: usize>(s: &str) -> HString<N> {
    match HString::try_from(s) {
        Ok(h) => h,
        Err(_) => {
            let mut h = HString::new();
            for ch in s.chars() {
                if h.push(ch).is_err() {
                    break;
                }
            }
            h
        }
    }
}

/// 单遍 JSON 扫描器（UTF-8 安全；字符串值处理 `\"` `\\` `\u00XX` 转义）.
struct Scan<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Scan<'a> {
    fn skip_ws(&mut self) {
        while let Some(ch) = self.s[self.i..].chars().next() {
            if ch.is_whitespace() {
                self.i += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += ch.len_utf8();
        Some(ch)
    }

    fn expect(&mut self, c: char) -> Result<(), MetricsError> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += c.len_utf8();
            Ok(())
        } else {
            Err(MetricsError::InvalidValue)
        }
    }

    fn read_string(&mut self) -> Result<String, MetricsError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let ch = self.bump().ok_or(MetricsError::InvalidValue)?;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self.bump().ok_or(MetricsError::InvalidValue)?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'u' => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                let d = self.bump().ok_or(MetricsError::InvalidValue)?;
                                code = code * 16 + d.to_digit(16).ok_or(MetricsError::InvalidValue)?;
                            }
                            out.push(char::from_u32(code).ok_or(MetricsError::InvalidValue)?);
                        }
                        _ => return Err(MetricsError::InvalidValue),
                    }
                }
                c => out.push(c),
            }
        }
    }

    /// 读取数值原文本（直到逗号/收尾括号；u64/f64 由调用方解析）.
    fn read_number_text(&mut self) -> Result<String, MetricsError> {
        self.skip_ws();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == ',' || c == '}' {
                break;
            }
            self.i += c.len_utf8();
        }
        if self.i == start {
            return Err(MetricsError::InvalidValue);
        }
        Ok(self.s[start..self.i].to_string())
    }

    /// 读取非负整数.
    fn read_u64(&mut self) -> Result<u64, MetricsError> {
        let t = self.read_number_text()?;
        t.parse().map_err(|_| MetricsError::InvalidValue)
    }

    /// 读取浮点.
    fn read_f64(&mut self) -> Result<f64, MetricsError> {
        let t = self.read_number_text()?;
        t.parse().map_err(|_| MetricsError::InvalidValue)
    }

    /// 读取 labels 数组（[{"k":"..","v":".."}, ...]）.
    fn read_labels(&mut self) -> Result<heapless::Vec<crate::metrics::Label, 8>, MetricsError> {
        self.expect('[')?;
        let mut labels = heapless::Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(']') {
                break;
            }
            self.expect('{')?;
            let mut k = HString::<32>::new();
            let mut v = HString::<64>::new();
            loop {
                self.skip_ws();
                if self.peek() == Some('}') {
                    break;
                }
                let key = self.read_string()?;
                self.expect(':')?;
                let val = self.read_string()?;
                match key.as_str() {
                    "k" => k = hstr::<32>(&val),
                    "v" => v = hstr::<64>(&val),
                    _ => {}
                }
                self.skip_ws();
                if self.peek() == Some('}') {
                    break;
                }
                self.expect(',')?;
            }
            // 消费子对象收尾 '}'
            self.expect('}')?;
            let _ = labels.push(crate::metrics::Label { key: k, value: v });
            self.skip_ws();
            if self.peek() == Some(']') {
                break;
            }
            self.expect(',')?;
        }
        // 消费数组收尾 ']'
        self.expect(']')?;
        Ok(labels)
    }

    /// 跳过未知键的值.
    fn skip_value(&mut self) -> Result<(), MetricsError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => {
                self.read_string()?;
                Ok(())
            }
            Some('{') | Some('[') => {
                let (open, close) = if self.peek() == Some('{') { ('{', '}') } else { ('[', ']') };
                let mut depth = 0usize;
                loop {
                    let ch = self.bump().ok_or(MetricsError::InvalidValue)?;
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if ch == '"' {
                        loop {
                            let c2 = self.bump().ok_or(MetricsError::InvalidValue)?;
                            if c2 == '\\' {
                                let _ = self.bump().ok_or(MetricsError::InvalidValue)?;
                            } else if c2 == '"' {
                                break;
                            }
                        }
                    }
                }
                Ok(())
            }
            _ => {
                let _ = self.read_number_text()?;
                Ok(())
            }
        }
    }
}

/// 将 metric_to_json 生成的一行 JSON 反序列化为 `Metric`.
fn parse_metric_json(line: &str) -> Result<Metric, MetricsError> {
    let mut sc = Scan { s: line, i: 0 };
    let mut name = HString::<64>::new();
    let mut metric_type = MetricType::Gauge;
    let mut value = MetricValue::U64(0);
    let mut timestamp_ms = 0u64;
    let mut labels = heapless::Vec::new();
    sc.expect('{')?;
    loop {
        sc.skip_ws();
        if sc.peek() == Some('}') {
            break;
        }
        let key = sc.read_string()?;
        sc.expect(':')?;
        sc.skip_ws();
        match key.as_str() {
            "name" => name = hstr::<64>(&sc.read_string()?),
            "kind" => {
                let k = sc.read_string()?;
                metric_type = match k.as_str() {
                    "COUNTER" => MetricType::Counter,
                    "GAUGE" => MetricType::Gauge,
                    "HISTOGRAM" => MetricType::Histogram,
                    "SUMMARY" => MetricType::Summary,
                    _ => return Err(MetricsError::InvalidValue),
                };
            }
            "value" => {
                sc.expect('{')?;
                sc.skip_ws();
                let vk = sc.read_string()?;
                sc.expect(':')?;
                value = match vk.as_str() {
                    "u" => MetricValue::U64(sc.read_u64()?),
                    "i" => MetricValue::I64(sc.read_number_text()?.parse().map_err(|_| MetricsError::InvalidValue)?),
                    "f" => MetricValue::F64(sc.read_f64()?),
                    _ => return Err(MetricsError::InvalidValue),
                };
                sc.skip_ws();
                sc.expect('}')?;
            }
            "ts" => timestamp_ms = sc.read_u64()?,
            "labels" => labels = sc.read_labels()?,
            _ => sc.skip_value()?,
        }
        sc.skip_ws();
        if sc.peek() == Some('}') {
            break;
        }
        sc.expect(',')?;
    }
    Ok(Metric {
        name,
        metric_type,
        value,
        labels,
        timestamp_ms,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::metrics::Label;

    fn mk_label(k: &str, v: &str) -> Label {
        Label {
            key: HString::<32>::try_from(k).unwrap(),
            value: HString::<64>::try_from(v).unwrap(),
        }
    }

    fn mk_metric(name: &str, value: MetricValue, ts: u64) -> Metric {
        Metric {
            name: HString::<64>::try_from(name).unwrap(),
            metric_type: MetricType::Counter,
            value,
            labels: heapless::Vec::new(),
            timestamp_ms: ts,
        }
    }

    /// 33 OB-07 判据：record → flush → JSONL 读回（U64 值 + 标签往返）.
    #[test]
    fn jsonl_roundtrip_preserves_metric() {
        let dir = std::env::temp_dir().join(format!("eneros-metric-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.jsonl");
        let path_str = path.to_str().unwrap();

        let mut store = FileMetricsStore::open(path_str).unwrap();
        let mut m = mk_metric("agent.error_count", MetricValue::U64(3), 1000);
        m.labels.push(mk_label("agent_id", "7")).unwrap();
        store.append(&m).unwrap();
        store.append(&mk_metric("agent.error_count", MetricValue::U64(4), 2000)).unwrap();

        let read_back = store.read_all().unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].name.as_str(), "agent.error_count");
        assert_eq!(read_back[0].timestamp_ms, 1000);
        assert_eq!(read_back[0].labels.len(), 1);
        assert_eq!(read_back[0].labels[0].key.as_str(), "agent_id");
        assert_eq!(read_back[0].labels[0].value.as_str(), "7");
        match read_back[1].value {
            MetricValue::U64(v) => assert_eq!(v, 4),
            _ => panic!("期望 U64"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 33 OB-07：F64/I64 值与 query_range/get_latest 语义.
    #[test]
    fn jsonl_query_and_latest() {
        let dir = std::env::temp_dir().join(format!("eneros-metric-fs2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("q.jsonl");
        let path_str = path.to_str().unwrap();

        let mut store = FileMetricsStore::open(path_str).unwrap();
        store.append(&mk_metric("system.temperature", MetricValue::F64(36.5), 100)).unwrap();
        store.append(&mk_metric("system.temperature", MetricValue::F64(37.2), 200)).unwrap();
        store.append(&mk_metric("agent.error_count", MetricValue::I64(-2), 300)).unwrap();

        // 重启语义：新实例读回
        let reopened = FileMetricsStore::open(path_str).unwrap();
        let got = reopened.query_range("system.temperature", 100, 200).unwrap();
        assert_eq!(got.len(), 2);
        match got[1].value {
            MetricValue::F64(v) => assert_eq!(v, 37.2),
            _ => panic!("期望 F64"),
        }
        assert_eq!(reopened.get_latest("agent.error_count"), Some((-2i64) as u64));
        assert_eq!(reopened.get_latest("missing"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
