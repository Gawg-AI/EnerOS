//! TDengine 时序存储后端（v0.29.0 — T029-19）
//!
//! 通过 HTTP REST API 与 TDengine 时序数据库交互。TDengine 是专为物联网和
//! 工业数据设计的时序数据库，非常适合电力系统场景。
//!
//! # 架构
//!
//! - 超级表 `measurements`：所有测点数据的统一 schema
//!   - 列：`ts TIMESTAMP, value DOUBLE, quality INT`
//!   - 标签：`element_id BIGINT, parameter NCHAR(128)`
//! - 子表：每个 `(element_id, parameter)` 组合对应一个子表 `d_{element_id}_{param}`
//! - 数据保留：`KEEP 3650`（10 年），`DURATION 10`（每 10 天一个数据文件）
//!
//! # HTTP 客户端
//!
//! 使用 `std::net::TcpStream` 实现纯标准库 HTTP/1.1 客户端，避免 `reqwest`/
//! `libc` 的原生依赖编译问题。支持 Basic Auth、Content-Length 和 chunked
//! 传输编码。通过 `std::thread::scope` 在独立 OS 线程执行，避免阻塞 tokio
//! 运行时（`TimeSeriesStorage` trait 是同步的，但可能从 async 上下文调用）。

use eneros_core::ElementId;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::engine::{DataPoint, DataQuality};
use crate::storage::TimeSeriesStorage;

/// TDengine 连接配置
#[derive(Debug, Clone)]
pub struct TDengineConfig {
    /// TDengine REST API 地址（如 `http://localhost:6041`）
    pub url: String,
    /// 数据库名（如 `eneros`）
    pub database: String,
    /// 用户名
    pub username: String,
    /// 密码
    pub password: String,
    /// HTTP 请求超时（秒）
    pub timeout_secs: u64,
}

impl Default for TDengineConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:6041".to_string(),
            database: "eneros".to_string(),
            username: "root".to_string(),
            password: "taosdata".to_string(),
            timeout_secs: 10,
        }
    }
}

/// TDengine REST API 响应
#[derive(Debug, serde::Deserialize)]
struct TDengineResponse {
    /// 响应状态："succ" 或 "error"
    status: String,
    /// 查询结果数据（每行一个 JSON 数组）
    #[serde(default)]
    data: Vec<Vec<serde_json::Value>>,
    /// 返回行数
    #[serde(default)]
    rows: i64,
    /// 错误码（status 为 "error" 时）
    #[serde(default)]
    code: i64,
    /// 错误描述（status 为 "error" 时）
    #[serde(default)]
    desc: String,
}

/// TDengine 时序存储后端
///
/// 通过 HTTP REST API 与 TDengine 交互，实现 [`TimeSeriesStorage`] trait。
///
/// # 示例
///
/// ```no_run
/// use eneros_timeseries::tdengine_backend::{TDengineBackend, TDengineConfig};
/// use std::sync::Arc;
///
/// let config = TDengineConfig::default();
/// let backend = TDengineBackend::new(config).unwrap();
/// // 可通过 TimeSeriesEngine::with_persistent_storage 挂载到引擎
/// ```
pub struct TDengineBackend {
    config: TDengineConfig,
}

impl TDengineBackend {
    /// 创建并初始化 TDengine 后端
    ///
    /// 连接 TDengine 服务器，创建数据库和超级表（如不存在）。
    pub fn new(config: TDengineConfig) -> Result<Self, String> {
        let backend = Self { config };
        backend.init_schema()?;
        Ok(backend)
    }

    /// 初始化数据库 schema：创建数据库 + 超级表
    fn init_schema(&self) -> Result<(), String> {
        // 创建数据库（10 年数据保留，每 10 天一个数据文件，6 个数据块）
        let create_db = build_create_database_sql(&self.config.database);
        self.execute_sql_on_server(&create_db)?;

        // 创建超级表
        let create_stable = build_create_stable_sql();
        self.execute_sql(&create_stable)?;
        Ok(())
    }

    /// 在 TDengine 服务器上执行 SQL（不指定数据库上下文，用于 CREATE DATABASE）
    fn execute_sql_on_server(&self, sql: &str) -> Result<TDengineResponse, String> {
        self.execute_sql_internal(sql, false)
    }

    /// 在配置的数据库上下文中执行 SQL
    fn execute_sql(&self, sql: &str) -> Result<TDengineResponse, String> {
        self.execute_sql_internal(sql, true)
    }

    /// 执行 SQL 的内部实现
    ///
    /// 使用 `std::thread::scope` 在独立 OS 线程执行 HTTP 请求，
    /// 避免阻塞 tokio 运行时（`TimeSeriesStorage` 可能从 async 上下文调用）。
    fn execute_sql_internal(&self, sql: &str, use_db: bool) -> Result<TDengineResponse, String> {
        let path = if use_db {
            format!("/rest/sql/{}", self.config.database)
        } else {
            "/rest/sql".to_string()
        };

        let config = &self.config;
        let sql_owned = sql.to_string();
        let path_owned = path;

        let result = std::thread::scope(|s| {
            s.spawn(move || -> Result<TDengineResponse, String> {
                let body = http_post(
                    &config.url,
                    &path_owned,
                    &config.username,
                    &config.password,
                    &sql_owned,
                    Duration::from_secs(config.timeout_secs),
                )?;

                let response: TDengineResponse = serde_json::from_str(&body)
                    .map_err(|e| {
                        let preview = &body[..body.len().min(200)];
                        format!("解析响应 JSON 失败: {} (body: {})", e, preview)
                    })?;

                if response.status != "succ" {
                    return Err(format!(
                        "TDengine SQL 错误 (code={}): {}",
                        response.code, response.desc
                    ));
                }

                Ok(response)
            })
            .join()
        });

        match result {
            Ok(inner) => inner,
            Err(e) => Err(format!("HTTP 工作线程 panic: {:?}", e)),
        }
    }
}

// =====================================================================
// 纯标准库 HTTP/1.1 客户端
// =====================================================================

/// 发送 HTTP POST 请求并返回响应体
///
/// 使用 `std::net::TcpStream` 实现纯标准库 HTTP 客户端。
/// 支持 Basic Auth、Content-Length 和 chunked 传输编码。
fn http_post(
    base_url: &str,
    path: &str,
    username: &str,
    password: &str,
    body: &str,
    timeout: Duration,
) -> Result<String, String> {
    let (host, port) = parse_url(base_url)?;
    let auth = base64_encode(format!("{}:{}", username, password).as_bytes());

    // 构建 HTTP/1.1 POST 请求
    let request = format!(
        "POST {} HTTP/1.1\r\n\
         Host: {}:{}\r\n\
         Authorization: Basic {}\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        path,
        host,
        port,
        auth,
        body.len(),
        body
    );

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
    if status_code != 200 {
        // 读取剩余响应体用于错误信息
        let _ = read_http_body(&mut reader);
        return Err(format!("HTTP 错误 {}: {}", status_code, status_line.trim()));
    }

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
            break; // 头部结束
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
        read_http_body(&mut reader)?
    };

    Ok(body)
}

/// 读取 HTTP 响应体（读到连接关闭）
fn read_http_body(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取响应体失败: {}", e))?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// 读取 chunked 传输编码的响应体
fn read_chunked_body(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut body = Vec::new();
    loop {
        // 读取 chunk 大小行（十六进制）
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(|e| format!("读取 chunk 大小失败: {}", e))?;
        let size_str = size_line.trim();
        let chunk_size = usize::from_str_radix(size_str, 16)
            .map_err(|e| format!("解析 chunk 大小失败 '{}': {}", size_str, e))?;
        if chunk_size == 0 {
            // 读取尾部（可能包含 trailer headers + 空行）
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok() {
                if line.trim().is_empty() {
                    break;
                }
                line.clear();
            }
            break;
        }
        // 读取 chunk 数据
        let mut chunk = vec![0u8; chunk_size];
        reader
            .read_exact(&mut chunk)
            .map_err(|e| format!("读取 chunk 数据失败: {}", e))?;
        body.extend_from_slice(&chunk);
        // 读取 chunk 后的 \r\n
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

    // 去除路径部分
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
        // 默认端口
        let default_port = if url.starts_with("https://") { 443 } else { 80 };
        Ok((host_port.to_string(), default_port))
    }
}

/// Base64 编码（用于 HTTP Basic Auth）
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

// =====================================================================
// SQL 生成函数（独立函数，便于单元测试）
// =====================================================================

/// 生成创建数据库的 SQL
fn build_create_database_sql(database: &str) -> String {
    // KEEP 3650：保留 10 年历史数据（电力系统长期趋势分析）
    // DURATION 10：每 10 天一个数据文件
    // BLOCKS 6：每个文件 6 个数据块
    format!(
        "CREATE DATABASE IF NOT EXISTS {} KEEP 3650 DURATION 10 BLOCKS 6",
        database
    )
}

/// 生成创建超级表的 SQL
fn build_create_stable_sql() -> String {
    // 超级表 measurements：
    // - ts：时间戳（毫秒精度）
    // - value：测量值（双精度浮点，覆盖电压/电流/功率等）
    // - quality：质量码（0=Good, 1=Uncertain, 2=Bad）
    // 标签：
    // - element_id：电网元件 ID（u64 → BIGINT）
    // - parameter：参数名（如 "voltage_a", "active_power"）
    "CREATE STABLE IF NOT EXISTS measurements \
     (ts TIMESTAMP, value DOUBLE, quality INT) \
     TAGS (element_id BIGINT, parameter NCHAR(128))"
        .to_string()
}

/// 生成写入数据的 SQL（自动建子表）
fn build_store_sql(element_id: ElementId, parameter: &str, point: &DataPoint) -> String {
    let table = subtable_name(element_id, parameter);
    let ts_ms = point.timestamp.timestamp_millis();
    let quality = quality_to_int(&point.quality);
    let param_escaped = escape_sql_string(parameter);
    format!(
        "INSERT INTO {} USING measurements TAGS ({}, '{}') VALUES ({}, {}, {})",
        table, element_id, param_escaped, ts_ms, point.value, quality
    )
}

/// 生成范围查询 SQL
fn build_retrieve_sql(element_id: ElementId, parameter: &str, start: i64, end: i64) -> String {
    let param_escaped = escape_sql_string(parameter);
    format!(
        "SELECT ts, value, quality FROM measurements \
         WHERE element_id = {} AND parameter = '{}' \
         AND ts >= {} AND ts <= {} \
         ORDER BY ts ASC",
        element_id, param_escaped, start, end
    )
}

/// 生成最新点查询 SQL
fn build_latest_sql(element_id: ElementId, parameter: &str) -> String {
    let param_escaped = escape_sql_string(parameter);
    format!(
        "SELECT ts, value, quality FROM measurements \
         WHERE element_id = {} AND parameter = '{}' \
         ORDER BY ts DESC LIMIT 1",
        element_id, param_escaped
    )
}

/// 生成删除前计数 SQL（用于 cleanup 返回删除行数）
fn build_cleanup_count_sql(before: i64) -> String {
    format!("SELECT COUNT(*) FROM measurements WHERE ts < {}", before)
}

/// 生成删除数据 SQL
fn build_cleanup_delete_sql(before: i64) -> String {
    format!("DELETE FROM measurements WHERE ts < {}", before)
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 质量码转换为 TDengine INT 值
fn quality_to_int(q: &DataQuality) -> i64 {
    match q {
        DataQuality::Good => 0,
        DataQuality::Uncertain => 1,
        DataQuality::Bad => 2,
    }
}

/// TDengine INT 值转换为质量码
fn int_to_quality(v: i64) -> DataQuality {
    match v {
        1 => DataQuality::Uncertain,
        2 => DataQuality::Bad,
        _ => DataQuality::Good,
    }
}

/// 生成子表名：`d_{element_id}_{sanitized_parameter}`
///
/// 将参数名中的非字母数字字符替换为下划线，确保是合法的 TDengine 表名。
/// 例如：`d_1_voltage_a`、`d_2_active_power`。
fn subtable_name(element_id: ElementId, parameter: &str) -> String {
    let sanitized: String = parameter
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("d_{}_{}", element_id, sanitized)
}

/// 转义 SQL 字符串字面量中的单引号（防止 SQL 注入）
fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "\\'")
}

/// 解析 TDengine 返回的时间戳值
///
/// TDengine REST API 可能返回：
/// - 字符串格式："2024-01-01 00:00:00.000" 或 "2024-01-01T00:00:00.000Z"
/// - Unix 毫秒时间戳（数字）
fn parse_timestamp(value: &serde_json::Value) -> Result<chrono::DateTime<chrono::Utc>, String> {
    match value {
        serde_json::Value::Number(n) => {
            let ms = n.as_i64().ok_or("时间戳不是整数")?;
            chrono::DateTime::from_timestamp_millis(ms)
                .ok_or_else(|| format!("无效的时间戳: {}", ms))
        }
        serde_json::Value::String(s) => {
            // 尝试多种时间格式（TDengine 可能使用不同格式返回时间戳）
            let formats = [
                "%Y-%m-%d %H:%M:%S%.3f",
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%dT%H:%M:%S%.3fZ",
                "%Y-%m-%dT%H:%M:%SZ",
            ];
            for fmt in &formats {
                if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
                    return Ok(naive.and_utc());
                }
            }
            Err(format!("无法解析时间戳字符串: {}", s))
        }
        _ => Err("时间戳类型无效".to_string()),
    }
}

/// 从 JSON 值中提取 f64
fn extract_f64(value: &serde_json::Value) -> Result<f64, String> {
    match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| format!("数值无法转为 f64: {}", n)),
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|e| format!("字符串转 f64 失败 '{}': {}", s, e)),
        _ => Err("数值类型无效".to_string()),
    }
}

/// 从 JSON 值中提取 i64
fn extract_i64(value: &serde_json::Value) -> Result<i64, String> {
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| format!("数值无法转为 i64: {}", n)),
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .map_err(|e| format!("字符串转 i64 失败 '{}': {}", s, e)),
        _ => Err("整数类型无效".to_string()),
    }
}

// =====================================================================
// TimeSeriesStorage trait 实现
// =====================================================================

impl TimeSeriesStorage for TDengineBackend {
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String> {
        let sql = build_store_sql(element_id, parameter, &point);
        self.execute_sql(&sql)?;
        Ok(())
    }

    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String> {
        let sql = build_retrieve_sql(element_id, parameter, start, end);
        let response = self.execute_sql(&sql)?;

        let mut results = Vec::with_capacity(response.rows.max(0) as usize);
        for row in response.data {
            if row.len() < 3 {
                continue;
            }
            let timestamp = parse_timestamp(&row[0])?;
            let value = extract_f64(&row[1])?;
            let quality_int = extract_i64(&row[2])?;
            results.push(DataPoint {
                timestamp,
                value,
                quality: int_to_quality(quality_int),
            });
        }

        Ok(results)
    }

    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String> {
        let sql = build_latest_sql(element_id, parameter);
        let response = self.execute_sql(&sql)?;

        if response.data.is_empty() {
            return Ok(None);
        }

        let row = &response.data[0];
        if row.len() < 3 {
            return Ok(None);
        }

        let timestamp = parse_timestamp(&row[0])?;
        let value = extract_f64(&row[1])?;
        let quality_int = extract_i64(&row[2])?;

        Ok(Some(DataPoint {
            timestamp,
            value,
            quality: int_to_quality(quality_int),
        }))
    }

    fn cleanup(&self, before: i64) -> Result<usize, String> {
        // 先查询将被删除的行数
        let count_sql = build_cleanup_count_sql(before);
        let count_response = self.execute_sql(&count_sql)?;
        let mut count = 0i64;
        if let Some(row) = count_response.data.first() {
            if let Some(val) = row.first() {
                count = extract_i64(val).unwrap_or(0);
            }
        }

        // 执行删除
        let delete_sql = build_cleanup_delete_sql(before);
        self.execute_sql(&delete_sql)?;

        Ok(count as usize)
    }
}

// =====================================================================
// 单元测试（不依赖 TDengine 服务器）
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

    // --- 配置测试 ---

    #[test]
    fn test_config_default() {
        let config = TDengineConfig::default();
        assert_eq!(config.url, "http://localhost:6041");
        assert_eq!(config.database, "eneros");
        assert_eq!(config.username, "root");
        assert_eq!(config.password, "taosdata");
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn test_config_clone() {
        let config = TDengineConfig {
            url: "http://tdengine:6041".to_string(),
            database: "power_db".to_string(),
            username: "admin".to_string(),
            password: "secret".to_string(),
            timeout_secs: 30,
        };
        let cloned = config.clone();
        assert_eq!(config.url, cloned.url);
        assert_eq!(config.database, cloned.database);
    }

    // --- SQL 生成测试 ---

    #[test]
    fn test_build_create_database_sql() {
        let sql = build_create_database_sql("eneros");
        assert!(sql.contains("CREATE DATABASE IF NOT EXISTS eneros"));
        assert!(sql.contains("KEEP 3650"));
        assert!(sql.contains("DURATION 10"));
        assert!(sql.contains("BLOCKS 6"));
    }

    #[test]
    fn test_build_create_stable_sql() {
        let sql = build_create_stable_sql();
        assert!(sql.contains("CREATE STABLE IF NOT EXISTS measurements"));
        assert!(sql.contains("ts TIMESTAMP"));
        assert!(sql.contains("value DOUBLE"));
        assert!(sql.contains("quality INT"));
        assert!(sql.contains("element_id BIGINT"));
        assert!(sql.contains("parameter NCHAR(128)"));
    }

    #[test]
    fn test_build_store_sql() {
        let point = make_point(1700000000, 220.5, DataQuality::Good);
        let sql = build_store_sql(1, "voltage_a", &point);

        assert!(sql.starts_with("INSERT INTO d_1_voltage_a USING measurements TAGS"));
        assert!(sql.contains("TAGS (1, 'voltage_a')"));
        let expected_ts = 1700000000_i64 * 1000;
        assert!(sql.contains(&format!("VALUES ({}, 220.5, 0)", expected_ts)));
    }

    #[test]
    fn test_build_store_sql_quality_codes() {
        let ts = 1700000000;
        let ts_ms = ts * 1000;

        let good = make_point(ts, 100.0, DataQuality::Good);
        let sql_good = build_store_sql(1, "p", &good);
        assert!(sql_good.contains(&format!("VALUES ({}, 100, 0)", ts_ms)));

        let uncertain = make_point(ts, 100.0, DataQuality::Uncertain);
        let sql_uncertain = build_store_sql(1, "p", &uncertain);
        assert!(sql_uncertain.contains(&format!("VALUES ({}, 100, 1)", ts_ms)));

        let bad = make_point(ts, 100.0, DataQuality::Bad);
        let sql_bad = build_store_sql(1, "p", &bad);
        assert!(sql_bad.contains(&format!("VALUES ({}, 100, 2)", ts_ms)));
    }

    #[test]
    fn test_build_store_sql_special_chars_in_parameter() {
        let point = make_point(1700000000, 1.0, DataQuality::Good);
        let sql = build_store_sql(1, "phase-a/b", &point);
        assert!(sql.contains("d_1_phase_a_b"));
        assert!(sql.contains("'phase-a/b'"));
    }

    #[test]
    fn test_build_store_sql_sql_injection_in_parameter() {
        let point = make_point(1700000000, 1.0, DataQuality::Good);
        let sql = build_store_sql(1, "param'; DROP TABLE", &point);
        assert!(sql.contains("\\'"));
        assert!(!sql.contains("DROP TABLE measurements"));
    }

    #[test]
    fn test_build_retrieve_sql() {
        let sql = build_retrieve_sql(42, "active_power", 1700000000000, 1700000100000);
        assert!(sql.contains("SELECT ts, value, quality FROM measurements"));
        assert!(sql.contains("element_id = 42"));
        assert!(sql.contains("parameter = 'active_power'"));
        assert!(sql.contains("ts >= 1700000000000"));
        assert!(sql.contains("ts <= 1700000100000"));
        assert!(sql.contains("ORDER BY ts ASC"));
    }

    #[test]
    fn test_build_latest_sql() {
        let sql = build_latest_sql(7, "frequency");
        assert!(sql.contains("SELECT ts, value, quality FROM measurements"));
        assert!(sql.contains("element_id = 7"));
        assert!(sql.contains("parameter = 'frequency'"));
        assert!(sql.contains("ORDER BY ts DESC LIMIT 1"));
    }

    #[test]
    fn test_build_cleanup_count_sql() {
        let sql = build_cleanup_count_sql(1700000000000);
        assert_eq!(
            sql,
            "SELECT COUNT(*) FROM measurements WHERE ts < 1700000000000"
        );
    }

    #[test]
    fn test_build_cleanup_delete_sql() {
        let sql = build_cleanup_delete_sql(1700000000000);
        assert_eq!(sql, "DELETE FROM measurements WHERE ts < 1700000000000");
    }

    // --- 辅助函数测试 ---

    #[test]
    fn test_quality_to_int() {
        assert_eq!(quality_to_int(&DataQuality::Good), 0);
        assert_eq!(quality_to_int(&DataQuality::Uncertain), 1);
        assert_eq!(quality_to_int(&DataQuality::Bad), 2);
    }

    #[test]
    fn test_int_to_quality() {
        assert_eq!(int_to_quality(0), DataQuality::Good);
        assert_eq!(int_to_quality(1), DataQuality::Uncertain);
        assert_eq!(int_to_quality(2), DataQuality::Bad);
        assert_eq!(int_to_quality(99), DataQuality::Good);
        assert_eq!(int_to_quality(-1), DataQuality::Good);
    }

    #[test]
    fn test_quality_round_trip() {
        for q in &[
            DataQuality::Good,
            DataQuality::Uncertain,
            DataQuality::Bad,
        ] {
            let i = quality_to_int(q);
            let q2 = int_to_quality(i);
            assert_eq!(*q, q2, "质量码往返转换不一致");
        }
    }

    #[test]
    fn test_subtable_name() {
        assert_eq!(subtable_name(1, "voltage"), "d_1_voltage");
        assert_eq!(subtable_name(42, "active_power"), "d_42_active_power");
        assert_eq!(subtable_name(1, "phase-a/b"), "d_1_phase_a_b");
        assert_eq!(subtable_name(1, "temp.1"), "d_1_temp_1");
        assert_eq!(subtable_name(1, ""), "d_1_");
        assert_eq!(subtable_name(u64::MAX, "p"), "d_18446744073709551615_p");
    }

    #[test]
    fn test_escape_sql_string() {
        assert_eq!(escape_sql_string("normal"), "normal");
        assert_eq!(escape_sql_string("it's"), "it\\'s");
        assert_eq!(escape_sql_string("a'b'c"), "a\\'b\\'c");
        assert_eq!(escape_sql_string(""), "");
    }

    // --- 时间戳解析测试 ---

    #[test]
    fn test_parse_timestamp_from_number() {
        let val = serde_json::json!(1700000000000_i64);
        let ts = parse_timestamp(&val).unwrap();
        assert_eq!(ts.timestamp_millis(), 1700000000000);
    }

    #[test]
    fn test_parse_timestamp_from_string_with_millis() {
        let val = serde_json::json!("2023-11-14 22:13:20.000");
        let ts = parse_timestamp(&val).unwrap();
        assert_eq!(ts.timestamp_millis(), 1700000000000);
    }

    #[test]
    fn test_parse_timestamp_from_string_without_millis() {
        let val = serde_json::json!("2023-11-14 22:13:20");
        let ts = parse_timestamp(&val).unwrap();
        assert_eq!(ts.timestamp(), 1700000000);
    }

    #[test]
    fn test_parse_timestamp_from_iso_string() {
        let val = serde_json::json!("2023-11-14T22:13:20.000Z");
        let ts = parse_timestamp(&val).unwrap();
        assert_eq!(ts.timestamp_millis(), 1700000000000);
    }

    #[test]
    fn test_parse_timestamp_invalid() {
        let val = serde_json::json!("not a timestamp");
        assert!(parse_timestamp(&val).is_err());

        let val = serde_json::json!(true);
        assert!(parse_timestamp(&val).is_err());
    }

    // --- 数值提取测试 ---

    #[test]
    fn test_extract_f64_from_number() {
        assert_eq!(extract_f64(&serde_json::json!(220.5)).unwrap(), 220.5);
        assert_eq!(extract_f64(&serde_json::json!(42)).unwrap(), 42.0);
    }

    #[test]
    fn test_extract_f64_from_string() {
        assert_eq!(extract_f64(&serde_json::json!("220.5")).unwrap(), 220.5);
        assert_eq!(extract_f64(&serde_json::json!("42")).unwrap(), 42.0);
    }

    #[test]
    fn test_extract_f64_invalid() {
        assert!(extract_f64(&serde_json::json!("abc")).is_err());
        assert!(extract_f64(&serde_json::json!(true)).is_err());
    }

    #[test]
    fn test_extract_i64_from_number() {
        assert_eq!(extract_i64(&serde_json::json!(42)).unwrap(), 42);
        assert_eq!(extract_i64(&serde_json::json!(0)).unwrap(), 0);
    }

    #[test]
    fn test_extract_i64_from_string() {
        assert_eq!(extract_i64(&serde_json::json!("42")).unwrap(), 42);
    }

    #[test]
    fn test_extract_i64_invalid() {
        assert!(extract_i64(&serde_json::json!(true)).is_err());
        assert!(extract_i64(&serde_json::json!("abc")).is_err());
    }

    // --- TDengineResponse 反序列化测试 ---

    #[test]
    fn test_deserialize_select_response() {
        let json = r#"{
            "status": "succ",
            "head": ["ts", "value", "quality"],
            "column_meta": [["ts", 9, 8], ["value", 6, 8], ["quality", 4, 4]],
            "data": [
                ["2023-11-14 22:13:20.000", 220.5, 0],
                ["2023-11-14 22:13:21.000", 221.0, 1]
            ],
            "rows": 2
        }"#;
        let resp: TDengineResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "succ");
        assert_eq!(resp.rows, 2);
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].len(), 3);
    }

    #[test]
    fn test_deserialize_insert_response() {
        let json = r#"{
            "status": "succ",
            "head": ["affected_rows"],
            "column_meta": [["affected_rows", 4, 4]],
            "data": [[1]],
            "rows": 1
        }"#;
        let resp: TDengineResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "succ");
        assert_eq!(resp.data.len(), 1);
    }

    #[test]
    fn test_deserialize_error_response() {
        let json = r#"{
            "status": "error",
            "code": 214,
            "desc": "syntax error near 'SELEC'"
        }"#;
        let resp: TDengineResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "error");
        assert_eq!(resp.code, 214);
        assert!(resp.desc.contains("syntax error"));
        assert!(resp.data.is_empty());
    }

    #[test]
    fn test_deserialize_empty_data_response() {
        let json = r#"{
            "status": "succ",
            "head": ["ts", "value", "quality"],
            "data": [],
            "rows": 0
        }"#;
        let resp: TDengineResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "succ");
        assert!(resp.data.is_empty());
        assert_eq!(resp.rows, 0);
    }

    // --- URL 解析测试 ---

    #[test]
    fn test_parse_url_with_port() {
        let (host, port) = parse_url("http://localhost:6041").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 6041);
    }

    #[test]
    fn test_parse_url_with_path() {
        let (host, port) = parse_url("http://localhost:6041/rest/sql").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 6041);
    }

    #[test]
    fn test_parse_url_default_port() {
        let (host, port) = parse_url("http://tdengine.example.com").unwrap();
        assert_eq!(host, "tdengine.example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_parse_url_invalid() {
        assert!(parse_url("ftp://localhost").is_err());
        assert!(parse_url("not a url").is_err());
    }

    // --- Base64 编码测试 ---

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_base64_encode_credentials() {
        let encoded = base64_encode(b"root:taosdata");
        assert_eq!(encoded, "cm9vdDp0YW9zZGF0YQ==");
    }

    // --- HTTP 状态行解析测试 ---

    #[test]
    fn test_parse_http_status() {
        assert_eq!(parse_http_status("HTTP/1.1 200 OK\r\n").unwrap(), 200);
        assert_eq!(parse_http_status("HTTP/1.1 404 Not Found\r\n").unwrap(), 404);
        assert_eq!(parse_http_status("HTTP/1.0 500 Internal Server Error\r\n").unwrap(), 500);
    }

    #[test]
    fn test_parse_http_status_invalid() {
        assert!(parse_http_status("not a status line").is_err());
        assert!(parse_http_status("HTTP/1.1").is_err());
    }

    // --- 端到端 SQL 生成验证 ---

    #[test]
    fn test_full_sql_generation_flow() {
        let element_id: ElementId = 10;
        let parameter = "voltage_a";
        let point = make_point(1700000000, 230.5, DataQuality::Uncertain);

        let store_sql = build_store_sql(element_id, parameter, &point);
        assert!(store_sql.contains("d_10_voltage_a"));
        assert!(store_sql.contains("USING measurements"));
        assert!(store_sql.contains("TAGS (10, 'voltage_a')"));
        assert!(store_sql.contains("1700000000000"));
        assert!(store_sql.contains("230.5"));
        assert!(store_sql.contains(", 1)"));

        let retrieve_sql = build_retrieve_sql(
            element_id,
            parameter,
            1700000000000,
            1700000100000,
        );
        assert!(retrieve_sql.contains("element_id = 10"));
        assert!(retrieve_sql.contains("'voltage_a'"));

        let latest_sql = build_latest_sql(element_id, parameter);
        assert!(latest_sql.contains("ORDER BY ts DESC LIMIT 1"));

        let count_sql = build_cleanup_count_sql(1700000000000);
        let delete_sql = build_cleanup_delete_sql(1700000000000);
        assert!(count_sql.contains("COUNT(*)"));
        assert!(delete_sql.contains("DELETE FROM measurements"));
    }
}

// =====================================================================
// 集成测试（需要 TDengine 服务器，标记 #[ignore]）
// =====================================================================

#[cfg(test)]
mod integration_tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_database() -> String {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("eneros_test_{}", id)
    }

    fn create_test_backend() -> Option<TDengineBackend> {
        let config = TDengineConfig {
            database: unique_database(),
            ..TDengineConfig::default()
        };
        match TDengineBackend::new(config) {
            Ok(backend) => Some(backend),
            Err(e) => {
                eprintln!("跳过 TDengine 集成测试（无法连接）: {}", e);
                None
            }
        }
    }

    fn make_point(ts_secs: i64, value: f64, quality: DataQuality) -> DataPoint {
        DataPoint {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            value,
            quality,
        }
    }

    #[test]
    #[ignore = "需要 TDengine 服务器运行于 localhost:6041"]
    fn test_tdengine_store_and_retrieve() {
        let backend = match create_test_backend() {
            Some(b) => b,
            None => return,
        };

        let ts = Utc.timestamp_opt(1700000000, 0).unwrap();
        let point = make_point(1700000000, 220.5, DataQuality::Good);
        backend.store(1, "voltage", point).unwrap();

        let start = (ts - chrono::Duration::hours(1)).timestamp_millis();
        let end = (ts + chrono::Duration::hours(1)).timestamp_millis();
        let results = backend.retrieve(1, "voltage", start, end).unwrap();

        assert_eq!(results.len(), 1);
        assert!((results[0].value - 220.5).abs() < 0.001);
        assert_eq!(results[0].quality, DataQuality::Good);
    }

    #[test]
    #[ignore = "需要 TDengine 服务器运行于 localhost:6041"]
    fn test_tdengine_latest() {
        let backend = match create_test_backend() {
            Some(b) => b,
            None => return,
        };

        backend
            .store(1, "current", make_point(1700000000, 10.0, DataQuality::Good))
            .unwrap();
        backend
            .store(1, "current", make_point(1700001000, 20.0, DataQuality::Uncertain))
            .unwrap();

        let latest = backend.latest(1, "current").unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert!((latest.value - 20.0).abs() < 0.001);
        assert_eq!(latest.quality, DataQuality::Uncertain);
    }

    #[test]
    #[ignore = "需要 TDengine 服务器运行于 localhost:6041"]
    fn test_tdengine_cleanup() {
        let backend = match create_test_backend() {
            Some(b) => b,
            None => return,
        };

        let old_ts = 1700000000;
        let new_ts = 1800000000;

        backend
            .store(1, "power", make_point(old_ts, 100.0, DataQuality::Good))
            .unwrap();
        backend
            .store(1, "power", make_point(new_ts, 200.0, DataQuality::Good))
            .unwrap();

        let cutoff = (old_ts + 1) * 1000;
        let removed = backend.cleanup(cutoff).unwrap();
        assert_eq!(removed, 1);

        let latest = backend.latest(1, "power").unwrap();
        assert!(latest.is_some());
        assert!((latest.unwrap().value - 200.0).abs() < 0.001);
    }

    #[test]
    #[ignore = "需要 TDengine 服务器运行于 localhost:6041"]
    fn test_tdengine_batch_write_and_range_query() {
        let backend = match create_test_backend() {
            Some(b) => b,
            None => return,
        };

        for i in 0..100 {
            backend
                .store(1, "frequency", make_point(1700000000 + i, 50.0 + i as f64 * 0.01, DataQuality::Good))
                .unwrap();
        }

        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000099, 0).unwrap();
        let results = backend
            .retrieve(1, "frequency", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(results.len(), 100);

        for (i, p) in results.iter().enumerate() {
            assert!((p.value - (50.0 + i as f64 * 0.01)).abs() < 0.001, "值不匹配 @ {}", i);
        }
    }

    #[test]
    #[ignore = "需要 TDengine 服务器运行于 localhost:6041"]
    fn test_tdengine_multiple_elements_and_parameters() {
        let backend = match create_test_backend() {
            Some(b) => b,
            None => return,
        };

        for element_id in 1..=3 {
            for param in &["voltage", "current", "power"] {
                backend
                    .store(element_id, param, make_point(1700000000, element_id as f64, DataQuality::Good))
                    .unwrap();
            }
        }

        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();

        for element_id in 1..=3 {
            for param in &["voltage", "current", "power"] {
                let results = backend
                    .retrieve(element_id, param, start.timestamp_millis(), end.timestamp_millis())
                    .unwrap();
                assert_eq!(results.len(), 1, "element={}, param={}", element_id, param);
                assert!((results[0].value - element_id as f64).abs() < 0.001);
            }
        }
    }

    #[test]
    #[ignore = "需要 TDengine 服务器运行于 localhost:6041"]
    fn test_tdengine_quality_codes_round_trip() {
        let backend = match create_test_backend() {
            Some(b) => b,
            None => return,
        };

        let ts = 1700000000;
        backend
            .store(1, "quality_test", make_point(ts, 1.0, DataQuality::Good))
            .unwrap();
        backend
            .store(1, "quality_test", make_point(ts + 1, 2.0, DataQuality::Uncertain))
            .unwrap();
        backend
            .store(1, "quality_test", make_point(ts + 2, 3.0, DataQuality::Bad))
            .unwrap();

        let start = Utc.timestamp_opt(ts, 0).unwrap();
        let end = Utc.timestamp_opt(ts + 100, 0).unwrap();
        let results = backend
            .retrieve(1, "quality_test", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].quality, DataQuality::Good);
        assert_eq!(results[1].quality, DataQuality::Uncertain);
        assert_eq!(results[2].quality, DataQuality::Bad);
    }
}
