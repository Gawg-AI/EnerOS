//! InfluxDB v2 时序数据后端（T029-20）
//!
//! 通过 InfluxDB v2 HTTP API（`/api/v2/write`、`/api/v2/query`、`/api/v2/delete`）
//! 实现 [`TimeSeriesStorage`] trait，提供工业级时序数据持久化能力。
//!
//! # 数据模型
//!
//! - **measurement**: `measurements`
//! - **tags**: `element_id`（元素 ID）、`parameter`（参数名）
//! - **fields**: `value`（浮点值）、`quality`（质量码字符串）
//! - **timestamp**: 纳秒精度
//!
//! Line Protocol 示例：
//! ```text
//! measurements,element_id=1,parameter=voltage value=220.5,quality="good" 1640995200000000000
//! ```
//!
//! # HTTP 客户端
//!
//! 使用 `std::net::TcpStream` 实现纯标准库 HTTP/1.1 客户端，避免 `reqwest`/
//! `libc` 的原生依赖编译问题（与 TDengine 后端保持一致）。支持 Token 认证、
//! Content-Length 和 chunked 传输编码。通过 `std::thread::scope` 在独立 OS
//! 线程执行，避免阻塞 tokio 运行时。

use eneros_core::ElementId;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::engine::{DataPoint, DataQuality};
use crate::storage::TimeSeriesStorage;

/// InfluxDB measurement 名称（固定为 `measurements`）
const MEASUREMENT: &str = "measurements";

/// InfluxDB 配置
#[derive(Debug, Clone)]
pub struct InfluxdbConfig {
    /// InfluxDB 服务器地址，例如 `http://localhost:8086`
    pub url: String,
    /// 组织名（org）
    pub org: String,
    /// bucket 名
    pub bucket: String,
    /// API token（认证用）
    pub token: String,
    /// HTTP 请求超时
    pub timeout: Duration,
}

impl Default for InfluxdbConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8086".to_string(),
            org: "eneros".to_string(),
            bucket: "eneros".to_string(),
            token: String::new(),
            timeout: Duration::from_secs(30),
        }
    }
}

/// InfluxDB v2 时序存储后端
///
/// 通过 HTTP API 与 InfluxDB v2 服务器交互，实现 [`TimeSeriesStorage`] trait。
/// 所有写入使用 Line Protocol，所有查询使用 Flux 查询语言。
pub struct InfluxdbBackend {
    config: InfluxdbConfig,
}

impl InfluxdbBackend {
    /// 创建新的 InfluxDB 后端
    pub fn new(config: InfluxdbConfig) -> Result<Self, String> {
        // 验证 URL 格式
        parse_url(&config.url)?;
        Ok(Self { config })
    }

    /// 从配置创建后端并验证连接（ping 服务器）
    pub fn connect(config: InfluxdbConfig) -> Result<Self, String> {
        let backend = Self::new(config)?;
        backend.ping()?;
        Ok(backend)
    }

    /// 验证 InfluxDB 服务器可达性（GET `/health`）
    pub fn ping(&self) -> Result<(), String> {
        let path = "/health".to_string();
        run_blocking(|| {
            let resp = http_request(
                "GET",
                &self.config.url,
                &path,
                &[],
                "",
                self.config.timeout,
            )?;
            if resp.status_code >= 200 && resp.status_code < 300 {
                Ok(())
            } else {
                Err(format!(
                    "InfluxDB 健康检查失败 (HTTP {}): {}",
                    resp.status_code,
                    resp.body.chars().take(200).collect::<String>()
                ))
            }
        })
    }

    /// 写入 Line Protocol 数据
    fn write_line_protocol(&self, line: &str) -> Result<(), String> {
        let path = format!(
            "/api/v2/write?org={}&bucket={}&precision=ns",
            urlencode(&self.config.org),
            urlencode(&self.config.bucket),
        );
        let token = self.config.token.clone();
        let body = line.to_string();
        let url = self.config.url.clone();
        let timeout = self.config.timeout;

        run_blocking(move || {
            let headers = vec![
                ("Authorization".to_string(), format!("Token {}", token)),
                ("Content-Type".to_string(), "text/plain; charset=utf-8".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ];
            let resp = http_request("POST", &url, &path, &headers, &body, timeout)?;
            if resp.status_code >= 200 && resp.status_code < 300 {
                Ok(())
            } else {
                Err(format!(
                    "InfluxDB 写入失败 (HTTP {}): {}",
                    resp.status_code,
                    resp.body.chars().take(500).collect::<String>()
                ))
            }
        })
    }

    /// 执行 Flux 查询并返回 CSV 响应体
    fn query_flux(&self, flux: &str) -> Result<String, String> {
        let path = format!("/api/v2/query?org={}", urlencode(&self.config.org));
        let token = self.config.token.clone();
        let body = format!(r#"{{"query":{},"type":"flux"}}"#, serde_json::to_string(flux).unwrap_or_default());
        let url = self.config.url.clone();
        let timeout = self.config.timeout;

        run_blocking(move || {
            let headers = vec![
                ("Authorization".to_string(), format!("Token {}", token)),
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Accept".to_string(), "application/csv".to_string()),
            ];
            let resp = http_request("POST", &url, &path, &headers, &body, timeout)?;
            if resp.status_code >= 200 && resp.status_code < 300 {
                Ok(resp.body)
            } else {
                Err(format!(
                    "InfluxDB 查询失败 (HTTP {}): {}",
                    resp.status_code,
                    resp.body.chars().take(500).collect::<String>()
                ))
            }
        })
    }

    /// 执行删除操作（POST `/api/v2/delete`）
    fn delete_range(&self, start_rfc3339: &str, stop_rfc3339: &str) -> Result<(), String> {
        let path = format!(
            "/api/v2/delete?org={}&bucket={}",
            urlencode(&self.config.org),
            urlencode(&self.config.bucket),
        );
        let token = self.config.token.clone();
        let predicate = format!("_measurement=\"{}\"", MEASUREMENT);
        let body = format!(
            r#"{{"start":{},"stop":{},"predicate":{}}}"#,
            serde_json::to_string(start_rfc3339).unwrap_or_default(),
            serde_json::to_string(stop_rfc3339).unwrap_or_default(),
            serde_json::to_string(&predicate).unwrap_or_default(),
        );
        let url = self.config.url.clone();
        let timeout = self.config.timeout;

        run_blocking(move || {
            let headers = vec![
                ("Authorization".to_string(), format!("Token {}", token)),
                ("Content-Type".to_string(), "application/json".to_string()),
            ];
            let resp = http_request("POST", &url, &path, &headers, &body, timeout)?;
            if resp.status_code >= 200 && resp.status_code < 300 {
                Ok(())
            } else {
                Err(format!(
                    "InfluxDB 删除失败 (HTTP {}): {}",
                    resp.status_code,
                    resp.body.chars().take(500).collect::<String>()
                ))
            }
        })
    }
}

impl TimeSeriesStorage for InfluxdbBackend {
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String> {
        let line = encode_line_protocol(element_id, parameter, &point);
        self.write_line_protocol(&line)
    }

    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String> {
        let start_dt = chrono::DateTime::from_timestamp_millis(start)
            .ok_or_else(|| format!("无效的 start 时间戳: {}", start))?;
        let end_dt = chrono::DateTime::from_timestamp_millis(end)
            .ok_or_else(|| format!("无效的 end 时间戳: {}", end))?;

        let flux = build_retrieve_flux(
            &self.config.bucket,
            element_id,
            parameter,
            start_dt,
            end_dt,
        );
        let csv = self.query_flux(&flux)?;
        Ok(parse_query_csv(&csv))
    }

    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String> {
        let now = chrono::Utc::now();
        let start = now - chrono::Duration::days(30);
        let flux = build_latest_flux(&self.config.bucket, element_id, parameter, start);
        let csv = self.query_flux(&flux)?;
        let mut points = parse_query_csv(&csv);
        Ok(points.pop())
    }

    fn cleanup(&self, before: i64) -> Result<usize, String> {
        let before_dt = chrono::DateTime::from_timestamp_millis(before)
            .ok_or_else(|| format!("无效的 before 时间戳: {}", before))?;

        let count_flux = build_count_flux(&self.config.bucket, before_dt);
        let csv = self.query_flux(&count_flux)?;
        let count = parse_count_csv(&csv);

        let start_rfc3339 = "1970-01-01T00:00:00Z";
        let stop_rfc3339 = before_dt.to_rfc3339();
        self.delete_range(start_rfc3339, &stop_rfc3339)?;

        Ok(count)
    }
}

// =====================================================================
// Line Protocol 编码
// =====================================================================

/// 将数据点编码为 InfluxDB Line Protocol 行
///
/// 格式：`<measurement>,<tags> <fields> <timestamp_ns>`
pub(crate) fn encode_line_protocol(
    element_id: ElementId,
    parameter: &str,
    point: &DataPoint,
) -> String {
    let mut line = String::with_capacity(128);

    line.push_str(MEASUREMENT);

    line.push(',');
    line.push_str("element_id=");
    line.push_str(&element_id.to_string());
    line.push(',');
    line.push_str("parameter=");
    line.push_str(&escape_tag_value(parameter));

    line.push(' ');

    line.push_str("value=");
    line.push_str(&format_float(point.value));
    line.push(',');
    line.push_str("quality=");
    line.push('"');
    line.push_str(&escape_string_field(&quality_to_str(&point.quality)));
    line.push('"');

    line.push(' ');

    let ts_nanos = point
        .timestamp
        .timestamp_nanos_opt()
        .unwrap_or_else(|| point.timestamp.timestamp_millis() * 1_000_000);
    line.push_str(&ts_nanos.to_string());

    line
}

/// 转义 tag value 中的特殊字符（逗号、等号、空格）
fn escape_tag_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ',' | '=' | ' ' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// 转义 string field 中的双引号和反斜杠
fn escape_string_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// 格式化浮点数为 Line Protocol 字段值
///
/// InfluxDB 要求浮点字段必须包含小数点或 `e`/`E`，否则会被当作整数。
fn format_float(v: f64) -> String {
    let s = format!("{}", v);
    if s.contains('.') || s.contains('e') || s.contains('E') || s == "inf" || s == "-inf"
        || s == "NaN"
    {
        s
    } else {
        format!("{}.0", s)
    }
}

/// 质量码 → 字符串
fn quality_to_str(q: &DataQuality) -> String {
    match q {
        DataQuality::Good => "good".to_string(),
        DataQuality::Uncertain => "uncertain".to_string(),
        DataQuality::Bad => "bad".to_string(),
    }
}

/// 字符串 → 质量码
fn str_to_quality(s: &str) -> DataQuality {
    match s {
        "uncertain" => DataQuality::Uncertain,
        "bad" => DataQuality::Bad,
        _ => DataQuality::Good,
    }
}

// =====================================================================
// Flux 查询构建
// =====================================================================

/// 构建 retrieve 查询的 Flux 脚本
pub(crate) fn build_retrieve_flux(
    bucket: &str,
    element_id: ElementId,
    parameter: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> String {
    format!(
        r#"from(bucket: "{}")
  |> range(start: {}, stop: {})
  |> filter(fn: (r) => r._measurement == "{}")
  |> filter(fn: (r) => r.element_id == "{}")
  |> filter(fn: (r) => r.parameter == "{}")
  |> pivot(rowKey: ["_time"], columnKey: ["_field"], valueColumn: "_value")
  |> sort(columns: ["_time"])"#,
        bucket,
        start.to_rfc3339(),
        end.to_rfc3339(),
        MEASUREMENT,
        element_id,
        escape_flux_string(parameter),
    )
}

/// 构建 latest 查询的 Flux 脚本
pub(crate) fn build_latest_flux(
    bucket: &str,
    element_id: ElementId,
    parameter: &str,
    start: chrono::DateTime<chrono::Utc>,
) -> String {
    format!(
        r#"from(bucket: "{}")
  |> range(start: {})
  |> filter(fn: (r) => r._measurement == "{}")
  |> filter(fn: (r) => r.element_id == "{}")
  |> filter(fn: (r) => r.parameter == "{}")
  |> pivot(rowKey: ["_time"], columnKey: ["_field"], valueColumn: "_value")
  |> sort(columns: ["_time"], desc: true)
  |> limit(n: 1)"#,
        bucket,
        start.to_rfc3339(),
        MEASUREMENT,
        element_id,
        escape_flux_string(parameter),
    )
}

/// 构建 cleanup 计数查询的 Flux 脚本
pub(crate) fn build_count_flux(
    bucket: &str,
    before: chrono::DateTime<chrono::Utc>,
) -> String {
    format!(
        r#"from(bucket: "{}")
  |> range(start: 1970-01-01T00:00:00Z, stop: {})
  |> filter(fn: (r) => r._measurement == "{}")
  |> count(column: "_value")
  |> sum(column: "_value")"#,
        bucket,
        before.to_rfc3339(),
        MEASUREMENT,
    )
}

/// 转义 Flux 字符串字面量中的双引号和反斜杠
fn escape_flux_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

// =====================================================================
// CSV 响应解析
// =====================================================================

/// 解析 InfluxDB 查询返回的 CSV（pivot 后的格式）为 DataPoint 列表
pub(crate) fn parse_query_csv(csv: &str) -> Vec<DataPoint> {
    let mut points = Vec::new();
    let mut header: Option<Vec<String>> = None;

    for line in csv.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        let cols = parse_csv_row(line);

        if header.is_none() {
            header = Some(cols);
            continue;
        }

        let header = header.as_ref().unwrap();
        let time_idx = header.iter().position(|h| h == "_time");
        let value_idx = header.iter().position(|h| h == "value");
        let quality_idx = header.iter().position(|h| h == "quality");

        let (Some(ti), Some(vi), Some(qi)) = (time_idx, value_idx, quality_idx) else {
            continue;
        };

        let time_str = cols.get(ti).map(|s| s.as_str()).unwrap_or("");
        let value_str = cols.get(vi).map(|s| s.as_str()).unwrap_or("");
        let quality_str = cols.get(qi).map(|s| s.as_str()).unwrap_or("");

        if time_str.is_empty() {
            continue;
        }

        let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(time_str) else {
            continue;
        };
        let Ok(value) = value_str.parse::<f64>() else {
            continue;
        };

        points.push(DataPoint {
            timestamp: timestamp.to_utc(),
            value,
            quality: str_to_quality(quality_str),
        });
    }

    points
}

/// 解析 InfluxDB count 查询返回的 CSV，返回总计数
pub(crate) fn parse_count_csv(csv: &str) -> usize {
    let mut header: Option<Vec<String>> = None;

    for line in csv.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        let cols = parse_csv_row(line);

        if header.is_none() {
            header = Some(cols);
            continue;
        }

        let header = header.as_ref().unwrap();
        let value_idx = header.iter().position(|h| h == "_value");
        if let Some(vi) = value_idx {
            if let Some(val) = cols.get(vi) {
                if let Ok(n) = val.parse::<f64>() {
                    return n as usize;
                }
            }
        }
    }

    0
}

/// 解析单行 CSV（处理引号包裹的字段和转义双引号）
fn parse_csv_row(line: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            cols.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    cols.push(current);
    cols
}

// =====================================================================
// 纯标准库 HTTP 客户端
// =====================================================================

/// HTTP 响应
struct HttpResponse {
    status_code: u16,
    body: String,
}

/// 发送 HTTP 请求（支持自定义 header）
///
/// 使用 `std::net::TcpStream` 实现纯标准库 HTTP/1.1 客户端。
/// 支持 Content-Length 和 chunked 传输编码。
/// 仅支持 HTTP（HTTPS 需通过反向代理实现）。
fn http_request(
    method: &str,
    base_url: &str,
    path: &str,
    headers: &[(String, String)],
    body: &str,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let (host, port) = parse_url(base_url)?;

    // 构建请求行 + 头部
    let mut request = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        method, path, host, port
    );

    for (key, value) in headers {
        request.push_str(&format!("{}: {}\r\n", key, value));
    }

    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }

    request.push_str("\r\n");
    if !body.is_empty() {
        request.push_str(body);
    }

    // 解析服务器地址并连接
    let addr_str = format!("{}:{}", host, port);
    let addr = addr_str
        .to_socket_addrs()
        .map_err(|e| format!("地址解析失败 '{}': {}", addr_str, e))?
        .next()
        .ok_or_else(|| format!("无法解析地址: {}", addr_str))?;

    let stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("TCP 连接失败 {}:{}: {}", host, port, e))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("设置读超时失败: {}", e))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("设置写超时失败: {}", e))?;

    let mut reader = BufReader::new(stream);

    // 发送请求
    reader
        .get_mut()
        .write_all(request.as_bytes())
        .map_err(|e| format!("发送请求失败: {}", e))?;

    // 读取状态行
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("读取状态行失败: {}", e))?;
    let status_code = parse_http_status(&status_line)?;

    // 读取头部
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("读取头部失败: {}", e))?;
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            content_length = val.trim().parse().ok();
        } else if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }

    // 读取响应体
    let body = if chunked {
        read_chunked_body(&mut reader)?
    } else if let Some(len) = content_length {
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| format!("读取响应体失败: {}", e))?;
        String::from_utf8_lossy(&buf).to_string()
    } else {
        // 无 Content-Length 且非 chunked，读到连接关闭
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取响应体失败: {}", e))?;
        String::from_utf8_lossy(&buf).to_string()
    };

    Ok(HttpResponse { status_code, body })
}

/// 读取 chunked 传输编码的响应体
fn read_chunked_body(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(|e| format!("读取 chunk 大小失败: {}", e))?;
        let size_str = size_line.trim();
        let chunk_size = usize::from_str_radix(size_str, 16)
            .map_err(|e| format!("解析 chunk 大小失败 '{}': {}", size_str, e))?;
        if chunk_size == 0 {
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok() {
                if line.trim().is_empty() {
                    break;
                }
                line.clear();
            }
            break;
        }
        let mut chunk = vec![0u8; chunk_size];
        reader
            .read_exact(&mut chunk)
            .map_err(|e| format!("读取 chunk 数据失败: {}", e))?;
        body.extend_from_slice(&chunk);
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|e| format!("读取 chunk 尾部失败: {}", e))?;
    }
    Ok(String::from_utf8_lossy(&body).to_string())
}

/// 解析 HTTP 状态行，返回状态码
fn parse_http_status(status_line: &str) -> Result<u16, String> {
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("无效的 HTTP 状态行: {}", status_line.trim()));
    }
    parts[1]
        .parse::<u16>()
        .map_err(|e| format!("解析状态码失败 '{}': {}", parts[1], e))
}

/// 解析 URL，提取 host 和 port
///
/// 支持 `http://host:port` 或 `http://host`（默认端口 80）格式。
fn parse_url(url: &str) -> Result<(String, u16), String> {
    let url = url.trim();
    let after_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| format!("URL 必须以 http:// 开头: {}", url))?;

    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);

    if host_port.is_empty() {
        return Err(format!("URL 缺少主机名: {}", url));
    }

    if let Some(idx) = host_port.rfind(':') {
        let host = &host_port[..idx];
        let port: u16 = host_port[idx + 1..]
            .parse()
            .map_err(|e| format!("端口号无效 '{}': {}", &host_port[idx + 1..], e))?;
        Ok((host.to_string(), port))
    } else {
        let default_port = if url.starts_with("https://") { 443 } else { 80 };
        Ok((host_port.to_string(), default_port))
    }
}

// =====================================================================
// 工具函数
// =====================================================================

/// URL 编码（百分号编码）
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

/// 在独立 OS 线程中执行 blocking 操作，避免在 tokio 运行时内 panic
fn run_blocking<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce() -> Result<R, String> + Send,
    R: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(_) => std::thread::scope(|s| {
            s.spawn(f)
                .join()
                .map_err(|e| format!("blocking 操作线程 panic: {:?}", e))?
        }),
        Err(_) => f(),
    }
}

// =====================================================================
// 单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_point(ts_secs: i64, value: f64, quality: DataQuality) -> DataPoint {
        DataPoint {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            value,
            quality,
        }
    }

    // ---------- Line Protocol 编码测试 ----------

    #[test]
    fn test_encode_line_protocol_basic() {
        let point = make_point(1640995200, 220.5, DataQuality::Good);
        let line = encode_line_protocol(1, "voltage", &point);
        assert_eq!(
            line,
            "measurements,element_id=1,parameter=voltage value=220.5,quality=\"good\" 1640995200000000000"
        );
    }

    #[test]
    fn test_encode_line_protocol_uncertain_quality() {
        let point = make_point(1640995200, 99.9, DataQuality::Uncertain);
        let line = encode_line_protocol(42, "current", &point);
        assert_eq!(
            line,
            "measurements,element_id=42,parameter=current value=99.9,quality=\"uncertain\" 1640995200000000000"
        );
    }

    #[test]
    fn test_encode_line_protocol_bad_quality() {
        let point = make_point(1640995200, -1.0, DataQuality::Bad);
        let line = encode_line_protocol(7, "status", &point);
        assert_eq!(
            line,
            "measurements,element_id=7,parameter=status value=-1.0,quality=\"bad\" 1640995200000000000"
        );
    }

    #[test]
    fn test_encode_line_protocol_integer_value_has_decimal() {
        let point = make_point(1640995200, 220.0, DataQuality::Good);
        let line = encode_line_protocol(1, "voltage", &point);
        assert!(
            line.contains("value=220.0"),
            "整数值应输出 220.0，实际: {}",
            line
        );
    }

    #[test]
    fn test_encode_line_protocol_tag_escaping() {
        let point = make_point(1640995200, 1.0, DataQuality::Good);
        let line = encode_line_protocol(1, "phase a,b=c", &point);
        assert!(
            line.contains("parameter=phase\\ a\\,b\\=c"),
            "tag value 转义错误: {}",
            line
        );
    }

    #[test]
    fn test_encode_line_protocol_nanosecond_timestamp() {
        let point = make_point(1640995200, 1.0, DataQuality::Good);
        let line = encode_line_protocol(1, "v", &point);
        assert!(line.ends_with(" 1640995200000000000"));
    }

    #[test]
    fn test_format_float() {
        assert_eq!(format_float(220.5), "220.5");
        assert_eq!(format_float(220.0), "220.0");
        assert_eq!(format_float(-1.0), "-1.0");
        assert_eq!(format_float(1.23e10), "12300000000.0");
    }

    #[test]
    fn test_escape_tag_value() {
        assert_eq!(escape_tag_value("voltage"), "voltage");
        assert_eq!(escape_tag_value("a b"), "a\\ b");
        assert_eq!(escape_tag_value("a,b"), "a\\,b");
        assert_eq!(escape_tag_value("a=b"), "a\\=b");
    }

    #[test]
    fn test_escape_string_field() {
        assert_eq!(escape_string_field("good"), "good");
        assert_eq!(escape_string_field("a\"b"), "a\\\"b");
        assert_eq!(escape_string_field("a\\b"), "a\\\\b");
    }

    // ---------- Flux 查询构建测试 ----------

    #[test]
    fn test_build_retrieve_flux() {
        let start = Utc.timestamp_opt(1640995200, 0).unwrap();
        let end = Utc.timestamp_opt(1641081600, 0).unwrap();
        let flux = build_retrieve_flux("eneros", 1, "voltage", start, end);

        assert!(flux.contains("from(bucket: \"eneros\")"));
        assert!(flux.contains("range(start: 2022-01-01T00:00:00+00:00"));
        assert!(flux.contains("r._measurement == \"measurements\""));
        assert!(flux.contains("r.element_id == \"1\""));
        assert!(flux.contains("r.parameter == \"voltage\""));
        assert!(flux.contains("pivot(rowKey: [\"_time\"]"));
        assert!(flux.contains("sort(columns: [\"_time\"])"));
    }

    #[test]
    fn test_build_retrieve_flux_parameter_escaping() {
        let start = Utc.timestamp_opt(1640995200, 0).unwrap();
        let end = Utc.timestamp_opt(1641081600, 0).unwrap();
        let flux = build_retrieve_flux("eneros", 1, "a\"b", start, end);
        assert!(flux.contains(r#"r.parameter == "a\"b""#));
    }

    #[test]
    fn test_build_latest_flux() {
        let start = Utc.timestamp_opt(1640995200, 0).unwrap();
        let flux = build_latest_flux("eneros", 42, "current", start);

        assert!(flux.contains("from(bucket: \"eneros\")"));
        assert!(flux.contains("r.element_id == \"42\""));
        assert!(flux.contains("r.parameter == \"current\""));
        assert!(flux.contains("sort(columns: [\"_time\"], desc: true)"));
        assert!(flux.contains("limit(n: 1)"));
    }

    #[test]
    fn test_build_count_flux() {
        let before = Utc.timestamp_opt(1640995200, 0).unwrap();
        let flux = build_count_flux("eneros", before);

        assert!(flux.contains("from(bucket: \"eneros\")"));
        assert!(flux.contains("range(start: 1970-01-01T00:00:00Z"));
        assert!(flux.contains("count(column: \"_value\")"));
        assert!(flux.contains("sum(column: \"_value\")"));
    }

    // ---------- CSV 解析测试 ----------

    #[test]
    fn test_parse_query_csv_basic() {
        let csv = "#group,false,false,true,true,false,false,false,false\n#datatype,string,long,dateTime:RFC3339,dateTime:RFC3339,dateTime:RFC3339,string,string,double,string\n#default,_result,,,,,,,,\n,result,table,_start,_stop,_time,element_id,parameter,value,quality\n,,0,2022-01-01T00:00:00Z,2022-01-02T00:00:00Z,2022-01-01T00:00:00Z,1,voltage,220.5,good\n,,0,2022-01-01T00:00:00Z,2022-01-02T00:00:00Z,2022-01-01T00:00:01Z,1,voltage,221.0,uncertain\n";
        let points = parse_query_csv(csv);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].value, 220.5);
        assert_eq!(points[0].quality, DataQuality::Good);
        assert_eq!(points[1].value, 221.0);
        assert_eq!(points[1].quality, DataQuality::Uncertain);
    }

    #[test]
    fn test_parse_query_csv_empty() {
        let csv = "#group,false,false\n#datatype,string,long\n#default,_result,,\n,result,table,_time,value,quality\n";
        let points = parse_query_csv(csv);
        assert!(points.is_empty());
    }

    #[test]
    fn test_parse_count_csv() {
        let csv = "#group,false,false\n#datatype,string,long\n#default,_result,,\n,result,table,_value\n,,0,1500\n";
        let count = parse_count_csv(csv);
        assert_eq!(count, 1500);
    }

    #[test]
    fn test_parse_count_csv_empty() {
        let csv = "";
        let count = parse_count_csv(csv);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_parse_csv_row_simple() {
        let cols = parse_csv_row("a,b,c");
        assert_eq!(cols, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_csv_row_quoted() {
        let cols = parse_csv_row("a,\"b,c\",d");
        assert_eq!(cols, vec!["a", "b,c", "d"]);
    }

    #[test]
    fn test_parse_csv_row_escaped_quote() {
        let cols = parse_csv_row("a,\"b\"\"c\",d");
        assert_eq!(cols, vec!["a", "b\"c", "d"]);
    }

    #[test]
    fn test_parse_csv_row_empty_fields() {
        let cols = parse_csv_row("a,,c");
        assert_eq!(cols, vec!["a", "", "c"]);
    }

    // ---------- 配置测试 ----------

    #[test]
    fn test_influxdb_config_default() {
        let config = InfluxdbConfig::default();
        assert_eq!(config.url, "http://localhost:8086");
        assert_eq!(config.org, "eneros");
        assert_eq!(config.bucket, "eneros");
        assert!(config.token.is_empty());
        assert_eq!(config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_influxdb_config_clone() {
        let config = InfluxdbConfig {
            url: "http://influx:8086".to_string(),
            org: "myorg".to_string(),
            bucket: "mybucket".to_string(),
            token: "secret-token".to_string(),
            timeout: Duration::from_secs(60),
        };
        let cloned = config.clone();
        assert_eq!(config.url, cloned.url);
        assert_eq!(config.org, cloned.org);
        assert_eq!(config.bucket, cloned.bucket);
        assert_eq!(config.token, cloned.token);
        assert_eq!(config.timeout, cloned.timeout);
    }

    // ---------- 工具函数测试 ----------

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("eneros"), "eneros");
        assert_eq!(urlencode("my org"), "my%20org");
        assert_eq!(urlencode("a+b"), "a%2Bb");
        assert_eq!(urlencode("a/b"), "a%2Fb");
    }

    #[test]
    fn test_quality_round_trip() {
        assert_eq!(quality_to_str(&DataQuality::Good), "good");
        assert_eq!(quality_to_str(&DataQuality::Uncertain), "uncertain");
        assert_eq!(quality_to_str(&DataQuality::Bad), "bad");

        assert_eq!(str_to_quality("good"), DataQuality::Good);
        assert_eq!(str_to_quality("uncertain"), DataQuality::Uncertain);
        assert_eq!(str_to_quality("bad"), DataQuality::Bad);
        assert_eq!(str_to_quality("unknown"), DataQuality::Good);
    }

    #[test]
    fn test_parse_url_http() {
        let (host, port) = parse_url("http://localhost:8086").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8086);
    }

    #[test]
    fn test_parse_url_default_port() {
        let (host, port) = parse_url("http://influx.example.com").unwrap();
        assert_eq!(host, "influx.example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_parse_url_with_path() {
        let (host, port) = parse_url("http://localhost:8086/health").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8086);
    }

    #[test]
    fn test_parse_url_invalid() {
        assert!(parse_url("ftp://localhost").is_err());
        assert!(parse_url("not-a-url").is_err());
    }

    #[test]
    fn test_parse_http_status() {
        assert_eq!(parse_http_status("HTTP/1.1 200 OK\r\n").unwrap(), 200);
        assert_eq!(parse_http_status("HTTP/1.1 204 No Content\r\n").unwrap(), 204);
        assert_eq!(parse_http_status("HTTP/1.1 404 Not Found\r\n").unwrap(), 404);
    }

    #[test]
    fn test_parse_http_status_invalid() {
        assert!(parse_http_status("invalid").is_err());
        assert!(parse_http_status("HTTP/1.1").is_err());
    }

    // ---------- 后端创建测试（不连接服务器） ----------

    #[test]
    fn test_influxdb_backend_new() {
        let config = InfluxdbConfig::default();
        let backend = InfluxdbBackend::new(config).expect("创建后端失败");
        assert_eq!(backend.config.url, "http://localhost:8086");
        assert_eq!(backend.config.org, "eneros");
    }

    #[test]
    fn test_influxdb_backend_new_with_custom_config() {
        let config = InfluxdbConfig {
            url: "http://influx.example.com:8086".to_string(),
            org: "production".to_string(),
            bucket: "power_data".to_string(),
            token: "my-token-123".to_string(),
            timeout: Duration::from_secs(10),
        };
        let backend = InfluxdbBackend::new(config).expect("创建后端失败");
        assert_eq!(backend.config.url, "http://influx.example.com:8086");
        assert_eq!(backend.config.org, "production");
        assert_eq!(backend.config.bucket, "power_data");
        assert_eq!(backend.config.token, "my-token-123");
        assert_eq!(backend.config.timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_influxdb_backend_new_invalid_url() {
        let config = InfluxdbConfig {
            url: "ftp://invalid".to_string(),
            ..Default::default()
        };
        assert!(InfluxdbBackend::new(config).is_err());
    }
}

// =====================================================================
// 集成测试（需要 InfluxDB 服务器，标记 #[ignore]）
// =====================================================================

#[cfg(test)]
mod integration_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn test_config() -> Option<InfluxdbConfig> {
        let url = std::env::var("INFLUXDB_URL").ok()?;
        let org = std::env::var("INFLUXDB_ORG").unwrap_or_else(|_| "eneros".to_string());
        let bucket = std::env::var("INFLUXDB_BUCKET").unwrap_or_else(|_| "eneros".to_string());
        let token = std::env::var("INFLUXDB_TOKEN").ok()?;
        Some(InfluxdbConfig {
            url,
            org,
            bucket,
            token,
            timeout: Duration::from_secs(30),
        })
    }

    fn make_point(ts_secs: i64, value: f64, quality: DataQuality) -> DataPoint {
        DataPoint {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            value,
            quality,
        }
    }

    #[test]
    #[ignore = "需要 InfluxDB 服务器，设置 INFLUXDB_URL/INFLUXDB_TOKEN 环境变量后运行"]
    fn test_influxdb_store_and_retrieve() {
        let config = test_config().expect("未配置 INFLUXDB_URL/INFLUXDB_TOKEN");
        let backend = InfluxdbBackend::connect(config).expect("连接 InfluxDB 失败");

        let ts = Utc::now();
        let point = DataPoint {
            timestamp: ts,
            value: 220.5,
            quality: DataQuality::Good,
        };

        backend
            .store(99999, "test_voltage", point.clone())
            .expect("写入失败");

        std::thread::sleep(Duration::from_secs(1));

        let start = (ts - chrono::Duration::minutes(1)).timestamp_millis();
        let end = (ts + chrono::Duration::minutes(1)).timestamp_millis();
        let results = backend
            .retrieve(99999, "test_voltage", start, end)
            .expect("查询失败");

        assert!(!results.is_empty(), "应至少返回 1 个点");
        let found = results.iter().find(|p| p.timestamp == ts);
        assert!(found.is_some(), "应包含刚写入的点");
        assert_eq!(found.unwrap().value, 220.5);
    }

    #[test]
    #[ignore = "需要 InfluxDB 服务器"]
    fn test_influxdb_latest() {
        let config = test_config().expect("未配置 INFLUXDB_URL/INFLUXDB_TOKEN");
        let backend = InfluxdbBackend::connect(config).expect("连接 InfluxDB 失败");

        let ts1 = Utc::now() - chrono::Duration::seconds(10);
        let ts2 = Utc::now();

        backend
            .store(99998, "test_current", make_point(ts1.timestamp(), 10.0, DataQuality::Good))
            .unwrap();
        backend
            .store(99998, "test_current", make_point(ts2.timestamp(), 20.0, DataQuality::Uncertain))
            .unwrap();

        std::thread::sleep(Duration::from_secs(1));

        let latest = backend.latest(99998, "test_current").expect("查询最新失败");
        assert!(latest.is_some(), "应返回最新点");
        assert_eq!(latest.unwrap().value, 20.0);
    }

    #[test]
    #[ignore = "需要 InfluxDB 服务器"]
    fn test_influxdb_cleanup() {
        let config = test_config().expect("未配置 INFLUXDB_URL/INFLUXDB_TOKEN");
        let backend = InfluxdbBackend::connect(config).expect("连接 InfluxDB 失败");

        let old_ts = Utc::now() - chrono::Duration::hours(2);
        let new_ts = Utc::now();

        backend
            .store(99997, "test_power", make_point(old_ts.timestamp(), 100.0, DataQuality::Good))
            .unwrap();
        backend
            .store(99997, "test_power", make_point(new_ts.timestamp(), 200.0, DataQuality::Good))
            .unwrap();

        std::thread::sleep(Duration::from_secs(1));

        let cutoff = (Utc::now() - chrono::Duration::hours(1)).timestamp_millis();
        let removed = backend.cleanup(cutoff).expect("清理失败");
        assert!(removed > 0, "应删除至少 1 个点");
    }

    #[test]
    #[ignore = "需要 InfluxDB 服务器"]
    fn test_influxdb_ping() {
        let config = test_config().expect("未配置 INFLUXDB_URL/INFLUXDB_TOKEN");
        let backend = InfluxdbBackend::new(config).expect("创建后端失败");
        backend.ping().expect("InfluxDB 服务器不可达");
    }
}
