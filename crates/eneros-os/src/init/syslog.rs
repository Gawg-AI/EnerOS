//! 结构化日志系统（JSON 格式 + 轮转 + 压缩 + 远程转发）
//!
//! 提供电力 OS 的日志基础设施：
//! - 结构化 JSON 日志格式（tracing-subscriber 兼容）
//! - 日志轮转（按大小 100MB + 按天）
//! - 7 天保留 + gzip 压缩归档
//! - 日志分类：系统/Agent/协议/安全/审计
//! - 动态日志级别调整
//! - RFC 5424 远程转发（TCP/TLS + 多目标 + 本地缓存重传）

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{BufWriter, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 日志错误
#[derive(Debug, thiserror::Error)]
pub enum SyslogError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("forward error: {0}")]
    Forward(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }

    /// RFC 5424 severity 数值
    pub fn rfc5424_severity(&self) -> u8 {
        match self {
            LogLevel::Trace => 7, // Debug
            LogLevel::Debug => 7, // Debug
            LogLevel::Info => 6,  // Informational
            LogLevel::Warn => 4,  // Warning
            LogLevel::Error => 3, // Error
        }
    }

    pub fn parse_level(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "trace" => Some(LogLevel::Trace),
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" | "warning" => Some(LogLevel::Warn),
            "error" | "err" => Some(LogLevel::Error),
            _ => None,
        }
    }
}

/// 日志分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogCategory {
    /// 系统日志（init/服务管理/硬件）
    System,
    /// Agent 日志（Agent 运行时/调度）
    Agent,
    /// 协议日志（IEC 104/61850/Modbus）
    Protocol,
    /// 安全日志（认证/授权/防火墙）
    Security,
    /// 审计日志（操作审计/配置变更）
    Audit,
}

impl LogCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogCategory::System => "system",
            LogCategory::Agent => "agent",
            LogCategory::Protocol => "protocol",
            LogCategory::Security => "security",
            LogCategory::Audit => "audit",
        }
    }

    /// RFC 5424 facility 数值
    pub fn rfc5424_facility(&self) -> u8 {
        match self {
            LogCategory::System => 3,    // daemon
            LogCategory::Agent => 16,    // local0
            LogCategory::Protocol => 17, // local1
            LogCategory::Security => 4,  // auth
            LogCategory::Audit => 10,    // authpriv
        }
    }
}

/// 轮转策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RotatePolicy {
    /// 按大小轮转（字节）
    Size(u64),
    /// 按天轮转
    Daily,
    /// 按大小或按天（先到先轮转）
    Both(u64),
}

impl Default for RotatePolicy {
    fn default() -> Self {
        Self::Both(100 * 1024 * 1024) // 100MB
    }
}

/// 日志轮转配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateConfig {
    pub policy: RotatePolicy,
    /// 最大保留文件数（每个分类）
    #[serde(default = "default_max_files")]
    pub max_files: u32,
    /// 是否 gzip 压缩归档
    #[serde(default = "default_true")]
    pub compress: bool,
    /// 保留天数
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

fn default_max_files() -> u32 {
    7
}
fn default_true() -> bool {
    true
}
fn default_retention_days() -> u32 {
    7
}

impl Default for RotateConfig {
    fn default() -> Self {
        Self {
            policy: RotatePolicy::default(),
            max_files: 7,
            compress: true,
            retention_days: 7,
        }
    }
}

/// 远程转发目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardTarget {
    /// 目标地址（host:port）
    pub addr: String,
    /// 传输协议
    #[serde(default)]
    pub transport: Transport,
    /// 最小转发级别（低于此级别不转发）
    #[serde(default = "default_forward_level")]
    pub min_level: LogLevel,
}

fn default_forward_level() -> LogLevel {
    LogLevel::Info
}

/// 传输协议
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// UDP（RFC 5424 默认）
    Udp,
    /// TCP
    #[default]
    Tcp,
    /// TLS over TCP
    Tls,
}

/// 远程转发配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardConfig {
    /// 转发目标列表（主备日志服务器）
    #[serde(default)]
    pub targets: Vec<ForwardTarget>,
    /// 本地缓存大小（网络中断时缓存的日志条数）
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,
    /// 重传间隔（秒）
    #[serde(default = "default_retry_interval")]
    pub retry_interval_secs: u64,
}

fn default_cache_size() -> usize {
    10000
}
fn default_retry_interval() -> u64 {
    30
}

impl Default for ForwardConfig {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            cache_size: 10000,
            retry_interval_secs: 30,
        }
    }
}

/// syslog 配置（对应 /etc/eneros/syslog.toml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyslogConfig {
    /// 日志目录
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
    /// 全局最小日志级别
    #[serde(default = "default_global_level")]
    pub global_level: LogLevel,
    /// 轮转配置
    #[serde(default)]
    pub rotate: RotateConfig,
    /// 远程转发配置
    #[serde(default)]
    pub forward: ForwardConfig,
    /// 每个分类的日志级别覆盖
    #[serde(default)]
    pub category_levels: std::collections::HashMap<String, LogLevel>,
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("/var/log/eneros")
}
fn default_global_level() -> LogLevel {
    LogLevel::Info
}

impl Default for SyslogConfig {
    fn default() -> Self {
        Self {
            log_dir: default_log_dir(),
            global_level: LogLevel::Info,
            rotate: RotateConfig::default(),
            forward: ForwardConfig::default(),
            category_levels: std::collections::HashMap::new(),
        }
    }
}

/// 结构化日志条目（JSON 格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// 时间戳（ISO 8601 UTC）
    pub timestamp: DateTime<Utc>,
    /// 日志级别
    pub level: LogLevel,
    /// 日志分类
    pub category: LogCategory,
    /// 来源（Agent ID 或模块名）
    pub source: String,
    /// 日志消息
    pub message: String,
    /// 附加字段（JSON object）
    #[serde(default)]
    pub fields: serde_json::Value,
}

impl LogEntry {
    /// 创建一条日志
    pub fn new(
        level: LogLevel,
        category: LogCategory,
        source: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            category,
            source: source.into(),
            message: message.into(),
            fields: serde_json::Value::Null,
        }
    }

    /// 序列化为 JSON 行（JSONL 格式）
    pub fn to_jsonl(&self) -> Result<String, SyslogError> {
        serde_json::to_string(self).map_err(|e| SyslogError::Config(e.to_string()))
    }

    /// 转为 RFC 5424 格式字符串
    ///
    /// 格式: `<priority>version timestamp hostname app-name procid msgid structured-data msg`
    ///
    /// APP-NAME 放 source，PROCID 用进程 PID。
    pub fn to_rfc5424(&self, hostname: &str) -> String {
        let priority = self.category.rfc5424_facility() * 8 + self.level.rfc5424_severity();
        let ts = self.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        // RFC 5424 SD-PARAM 值需转义 " \ ]
        let structured_data = format!(
            r#"[eneros category="{}" source="{}"]"#,
            escape_sd_value(self.category.as_str()),
            escape_sd_value(&self.source),
        );
        // 消息中的换行符替换为空格，避免 TCP non-transparent 帧拆分
        let msg = self.message.replace('\n', " ");
        format!(
            "<{}>1 {} {} {} {} {} {} {}\n",
            priority,
            ts,
            hostname,
            self.source,
            std::process::id(),
            "-",
            structured_data,
            msg
        )
    }
}

/// RFC 5424 SD-PARAM 值转义（反斜杠、双引号、右方括号）
fn escape_sd_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(']', "\\]")
}

/// 跨平台 sync_data：Linux 用 fdatasync，非 Linux 回退到 sync_all
fn sync_file_data(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        file.sync_data()
    }
    #[cfg(not(target_os = "linux"))]
    {
        file.sync_all()
    }
}

/// 校验 category_levels 的 key 是否合法
fn is_valid_category_key(key: &str) -> bool {
    matches!(
        key,
        "system" | "agent" | "protocol" | "security" | "audit"
    )
}

// ---------------------------------------------------------------------------
// LogWriter — 日志文件管理器（写入 + 轮转 + 压缩 + 保留）
// ---------------------------------------------------------------------------

/// LogWriter 内部可变状态（由 Mutex 保护）
struct LogWriterInner {
    config: SyslogConfig,
    /// 每个分类的当前日期（用于按天轮转独立检测）
    current_dates: HashMap<LogCategory, String>,
    /// 每个分类的常驻 BufWriter 句柄（不再每次 open/close）
    handles: HashMap<LogCategory, BufWriter<std::fs::File>>,
    /// 上次 cleanup 时间（限流：每小时最多一次）
    last_cleanup: Option<Instant>,
    /// 写入计数器（非关键日志每 100 条 flush 一次）
    write_counter: u64,
    /// 上次 flush 时间（非关键日志每 5 秒 flush 一次）
    last_flush: Instant,
}

/// 日志文件管理器（写入 + 轮转 + 压缩 + 保留）
///
/// 内部使用 `parking_lot::Mutex` 保护可变状态，所有方法均为 `&self`，
/// 可安全跨线程共享。
pub struct LogWriter {
    inner: Mutex<LogWriterInner>,
}

impl LogWriter {
    pub fn new(config: SyslogConfig) -> Self {
        let now = Instant::now();
        Self {
            inner: Mutex::new(LogWriterInner {
                config,
                current_dates: HashMap::new(),
                handles: HashMap::new(),
                last_cleanup: None,
                write_counter: 0,
                last_flush: now,
            }),
        }
    }

    /// 返回当前配置的快照（克隆）
    pub fn config(&self) -> SyslogConfig {
        self.inner.lock().config.clone()
    }

    /// 写入一条日志（写入文件 + 检查轮转 + fsync 策略）
    pub fn write(&self, entry: &LogEntry) -> Result<(), SyslogError> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;

        // 级别过滤
        if !inner.should_log(entry) {
            return Ok(());
        }

        let jsonl = entry.to_jsonl()?;
        let line = format!("{}\n", jsonl);

        let log_file = inner
            .config
            .log_dir
            .join(format!("{}.log", entry.category.as_str()));

        // 确保目录存在
        if let Some(parent) = log_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // 懒打开 BufWriter 句柄（常驻不关闭）
        if let std::collections::hash_map::Entry::Vacant(e) = inner.handles.entry(entry.category) {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file)?;
            e.insert(BufWriter::new(file));
        }

        // 确定 fsync 策略（先更新计数器，避免与 handle 借用冲突）
        let need_sync = if entry.level == LogLevel::Error {
            // ERROR 级别：立即 sync_data
            true
        } else if entry.category == LogCategory::Audit {
            // Audit 类日志：立即 sync_all
            true
        } else {
            // 其他级别：每 100 条或每 5 秒 flush
            inner.write_counter += 1;
            let now = Instant::now();
            if inner.write_counter >= 100
                || now.duration_since(inner.last_flush) >= Duration::from_secs(5)
            {
                inner.write_counter = 0;
                inner.last_flush = now;
                true
            } else {
                false
            }
        };

        // 写入 + flush/sync
        {
            let handle = inner.handles.get_mut(&entry.category).unwrap();
            handle.write_all(line.as_bytes())?;
            if need_sync {
                handle.flush()?;
                if entry.category == LogCategory::Audit {
                    handle.get_ref().sync_all()?;
                } else if entry.level == LogLevel::Error {
                    sync_file_data(handle.get_ref())?;
                }
            }
        }

        // 检查轮转
        inner.maybe_rotate(entry.category)?;

        Ok(())
    }

    /// 判断该条日志是否应记录（级别过滤）
    pub fn should_log(&self, entry: &LogEntry) -> bool {
        self.inner.lock().should_log(entry)
    }

    /// Flush 所有 BufWriter 句柄到磁盘（用于优雅关停或测试验证）
    pub fn flush(&self) -> Result<(), SyslogError> {
        let mut inner = self.inner.lock();
        for handle in inner.handles.values_mut() {
            handle.flush()?;
        }
        Ok(())
    }

    /// 动态调整全局日志级别
    pub fn set_global_level(&self, level: LogLevel) {
        self.inner.lock().config.global_level = level;
    }

    /// 动态调整分类日志级别
    pub fn set_category_level(&self, category: LogCategory, level: LogLevel) {
        self.inner
            .lock()
            .config
            .category_levels
            .insert(category.as_str().to_string(), level);
    }

    /// 热重载配置（句柄由 maybe_rotate 在目录变更时自动重建）
    fn update_config(&self, config: SyslogConfig) {
        let mut inner = self.inner.lock();
        inner.config = config;
        // 目录可能变更，关闭旧句柄让下次 write 重新打开
        inner.handles.clear();
        inner.current_dates.clear();
    }

    #[cfg(test)]
    fn set_category_date_for_test(&self, category: LogCategory, date: &str) {
        let mut inner = self.inner.lock();
        inner.current_dates.insert(category, date.to_string());
    }
}

impl LogWriterInner {
    /// 判断该条日志是否应记录（级别过滤）
    fn should_log(&self, entry: &LogEntry) -> bool {
        // 分类级别覆盖优先
        if let Some(level) = self.config.category_levels.get(entry.category.as_str()) {
            return entry.level >= *level;
        }
        entry.level >= self.config.global_level
    }

    /// 检查并执行轮转（按分类独立检查日期变更）
    fn maybe_rotate(&mut self, category: LogCategory) -> Result<(), SyslogError> {
        let log_file = self
            .config
            .log_dir
            .join(format!("{}.log", category.as_str()));

        // 用 metadata 获取真实文件大小
        let file_size = std::fs::metadata(&log_file).map(|m| m.len()).unwrap_or(0);

        // 按分类独立检查日期变更（首次写入只记录日期，不触发轮转）
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let date_changed = match self.current_dates.get(&category) {
            None => {
                // 首次写入该分类 — 记录日期，不轮转
                self.current_dates.insert(category, today);
                false
            }
            Some(prev) if prev != &today => {
                // 日期变更 — 更新并触发轮转
                self.current_dates.insert(category, today);
                true
            }
            Some(_) => false, // 同一天，不轮转
        };

        let need_rotate = match &self.config.rotate.policy {
            RotatePolicy::Size(max) => file_size >= *max,
            RotatePolicy::Daily => date_changed,
            RotatePolicy::Both(max) => file_size >= *max || date_changed,
        };

        if need_rotate && log_file.exists() {
            // 轮转前先 flush 并关闭该分类的句柄
            if let Some(handle) = self.handles.remove(&category) {
                let mut handle = handle;
                handle.flush()?;
                // handle drop 关闭文件描述符
            }
            self.do_rotate(&log_file)?;
        }

        // 限流 cleanup：每小时最多一次
        let should_cleanup = self
            .last_cleanup
            .map(|t| t.elapsed() > Duration::from_secs(3600))
            .unwrap_or(true);
        if should_cleanup {
            self.cleanup_old_files(&log_file)?;
            self.last_cleanup = Some(Instant::now());
        }

        Ok(())
    }

    /// 执行轮转：重命名 + 压缩
    fn do_rotate(&self, log_file: &Path) -> Result<(), SyslogError> {
        let ts = Utc::now().format("%Y%m%d_%H%M%S");
        let rotated = log_file.with_extension(format!("log.{}", ts));

        std::fs::rename(log_file, &rotated)?;

        // gzip 压缩（失败时保留原文件，不吞掉错误）
        if self.config.rotate.compress {
            match std::process::Command::new("gzip").arg(&rotated).status() {
                Ok(status) if status.success() => {
                    // gzip 成功，原文件被 gzip 删除
                }
                _ => {
                    tracing::warn!("gzip 压缩失败: {}, 保留原文件", rotated.display());
                }
            }
        }

        Ok(())
    }

    /// 清理超过保留天数的日志文件 + 按 max_files 保留最新 N 个
    fn cleanup_old_files(&self, log_file: &Path) -> Result<(), SyslogError> {
        let stem = log_file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let parent = log_file.parent().unwrap_or(Path::new("."));
        let retention = self.config.rotate.retention_days;
        let max_files = self.config.rotate.max_files as usize;

        // 收集通过保留天数检查的轮转文件（name, timestamp）
        let mut surviving: Vec<(String, chrono::NaiveDateTime)> = Vec::new();

        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            // 匹配 {stem}.log.YYYYMMDD_HHMMSS[.gz]
            if !name.starts_with(&format!("{}.log.", stem)) {
                continue;
            }

            // 提取时间戳部分
            let ts_part = name
                .strip_prefix(&format!("{}.log.", stem))
                .unwrap_or("")
                .trim_end_matches(".gz");

            if let Ok(file_time) = chrono::NaiveDateTime::parse_from_str(ts_part, "%Y%m%d_%H%M%S")
            {
                let file_age = Utc::now().naive_utc() - file_time;
                if file_age.num_days() >= retention as i64 {
                    let _ = std::fs::remove_file(entry.path());
                } else {
                    surviving.push((name, file_time));
                }
            }
        }

        // 按时间戳降序排列（最新在前）
        surviving.sort_by_key(|(_, time)| std::cmp::Reverse(*time));

        // 超出 max_files 的删除最旧的
        if surviving.len() > max_files {
            for (name, _) in surviving.iter().skip(max_files) {
                let path = parent.join(name);
                let _ = std::fs::remove_file(&path);
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LogForwarder — 远程日志转发器（RFC 5424 over TCP/UDP）
// ---------------------------------------------------------------------------

/// LogForwarder 内部可变状态（由 Mutex 保护）
struct LogForwarderInner {
    targets: Vec<ForwardTarget>,
    cache_size: usize,
    #[allow(dead_code)]
    retry_interval_secs: u64,
    /// 本地缓存（网络中断时）: (entry, rfc5424_line, retry_count)
    cache: VecDeque<(LogEntry, String, u32)>,
    hostname: String,
}

/// 远程日志转发器（RFC 5424 over TCP/UDP）
///
/// 内部使用 `parking_lot::Mutex` 保护可变状态，所有方法均为 `&self`，
/// 可安全跨线程共享。TLS 在配置加载阶段被拒绝（fail-fast）。
pub struct LogForwarder {
    inner: Mutex<LogForwarderInner>,
}

impl LogForwarder {
    pub fn new(config: ForwardConfig, hostname: &str) -> Self {
        let cache = VecDeque::with_capacity(config.cache_size);
        Self {
            inner: Mutex::new(LogForwarderInner {
                targets: config.targets,
                cache_size: config.cache_size,
                retry_interval_secs: config.retry_interval_secs,
                cache,
                hostname: hostname.to_string(),
            }),
        }
    }

    /// 返回当前配置的快照（克隆）
    pub fn config(&self) -> ForwardConfig {
        let inner = self.inner.lock();
        ForwardConfig {
            targets: inner.targets.clone(),
            cache_size: inner.cache_size,
            retry_interval_secs: inner.retry_interval_secs,
        }
    }

    /// 转发一条日志（发送到所有目标，失败时缓存待重传）
    pub fn forward(&self, entry: &LogEntry) -> Result<(), SyslogError> {
        let line = {
            let inner = self.inner.lock();
            entry.to_rfc5424(&inner.hostname)
        };

        let mut failed_targets = Vec::new();
        {
            let inner = self.inner.lock();
            for target in &inner.targets {
                // 级别过滤
                if entry.level < target.min_level {
                    continue;
                }
                if let Err(e) = send_to_target(target, &line) {
                    // 记录失败目标，继续尝试剩余目标（主备日志服务器场景）
                    failed_targets.push(format!("{}: {}", target.addr, e));
                }
            }
        }

        if !failed_targets.is_empty() {
            // 仅缓存一次（重传时发往所有目标）
            self.cache_push(entry.clone(), line);
            return Err(SyslogError::Forward(format!(
                "发送失败的目标 [{}]，已缓存待重传",
                failed_targets.join(", ")
            )));
        }
        Ok(())
    }

    /// 缓存日志（网络中断时），加权保留高优先级
    fn cache_push(&self, entry: LogEntry, line: String) {
        let mut inner = self.inner.lock();
        if inner.cache.len() >= inner.cache_size {
            let is_high_priority = entry.level >= LogLevel::Error
                || entry.category == LogCategory::Security
                || entry.category == LogCategory::Audit;
            if is_high_priority {
                // 高优先级：丢弃最旧，腾出空间
                inner.cache.pop_front();
                inner.cache.push_back((entry, line, 0));
            } else {
                // 低优先级：丢弃新日志
                tracing::warn!(
                    "log cache full, dropping low-priority log: {}",
                    entry.message
                );
            }
        } else {
            inner.cache.push_back((entry, line, 0));
        }
    }

    /// 重传缓存的日志
    ///
    /// 失败条目移到队尾继续尝试后续条目；单条重试超过 5 次则丢弃。
    pub fn retry_cached(&self) -> Result<usize, SyslogError> {
        let mut inner = self.inner.lock();

        if inner.cache.is_empty() {
            return Ok(0);
        }

        let targets = inner.targets.clone();
        let mut sent = 0;
        let mut remaining: VecDeque<(LogEntry, String, u32)> = VecDeque::new();

        while let Some((entry, line, retries)) = inner.cache.pop_front() {
            // 无目标则保留
            if targets.is_empty() {
                remaining.push_back((entry, line, retries));
                continue;
            }

            let mut all_ok = true;
            for target in &targets {
                if entry.level < target.min_level {
                    continue;
                }
                if send_to_target(target, &line).is_err() {
                    all_ok = false;
                    break;
                }
            }

            if all_ok {
                sent += 1;
            } else {
                let new_retries = retries + 1;
                if new_retries >= 5 {
                    tracing::warn!(
                        "dropping log entry after 5 failed retries: {}",
                        entry.message
                    );
                } else {
                    // 失败条目移到队尾
                    remaining.push_back((entry, line, new_retries));
                }
            }
        }

        inner.cache = remaining;
        Ok(sent)
    }

    /// 缓存中待重传的日志数
    pub fn pending_count(&self) -> usize {
        self.inner.lock().cache.len()
    }

    /// 热重载配置（保留 cache 中的待重传日志）
    fn update_config(&self, config: &ForwardConfig) {
        let mut inner = self.inner.lock();
        inner.targets = config.targets.clone();
        inner.cache_size = config.cache_size;
        inner.retry_interval_secs = config.retry_interval_secs;
        // cache 保留
    }
}

/// 发送到单个目标（独立函数，避免与 LogForwarderInner 借用冲突）
fn send_to_target(target: &ForwardTarget, line: &str) -> Result<(), SyslogError> {
    match target.transport {
        Transport::Tcp => {
            let addr: std::net::SocketAddr = target
                .addr
                .parse()
                .map_err(|e: std::net::AddrParseError| {
                    SyslogError::Forward(format!("parse addr: {e}"))
                })?;
            let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
            let mut stream = stream;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            stream.write_all(line.as_bytes())?;
        }
        Transport::Tls => {
            // TLS 在配置加载阶段已被拒绝（fail-fast），此处为安全兜底
            return Err(SyslogError::Forward(
                "TLS transport not supported — rejected at config load".into(),
            ));
        }
        Transport::Udp => {
            let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
            sock.send_to(line.as_bytes(), &target.addr)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SyslogManager — 组合写入器 + 转发器
// ---------------------------------------------------------------------------

/// syslog 管理器（组合写入器 + 转发器）
///
/// writer 和 forwarder 内部已有 Mutex，无需外部加锁。
/// log()/retry_forward()/set_*_level() 均为 &self，可跨线程共享。
pub struct SyslogManager {
    writer: LogWriter,
    forwarder: Option<LogForwarder>,
}

impl SyslogManager {
    /// 从配置文件加载
    pub fn load(path: &Path) -> Result<Self, SyslogError> {
        let content = std::fs::read_to_string(path)?;
        let config: SyslogConfig =
            toml::from_str(&content).map_err(|e| SyslogError::Config(e.to_string()))?;
        Self::new(config)
    }

    /// 从配置创建（校验 TLS fail-fast + category_levels key）
    pub fn new(config: SyslogConfig) -> Result<Self, SyslogError> {
        // TLS fail-fast：配置加载阶段拒绝 TLS
        for target in &config.forward.targets {
            if target.transport == Transport::Tls {
                return Err(SyslogError::Config(
                    "TLS transport not yet supported, use Tcp or Udp".into(),
                ));
            }
        }

        // category_levels key 校验
        for key in config.category_levels.keys() {
            if !is_valid_category_key(key) {
                return Err(SyslogError::Config(format!("unknown category: {key}")));
            }
        }

        let forward_config = config.forward.clone();
        let writer = LogWriter::new(config);
        let forwarder = if forward_config.targets.is_empty() {
            None
        } else {
            Some(LogForwarder::new(
                forward_config,
                &std::env::var("HOSTNAME").unwrap_or_else(|_| "eneros".to_string()),
            ))
        };
        Ok(Self { writer, forwarder })
    }

    /// 返回当前配置的快照（克隆）
    pub fn config(&self) -> SyslogConfig {
        self.writer.config()
    }

    /// 写入并转发一条日志
    pub fn log(&self, entry: &LogEntry) -> Result<(), SyslogError> {
        // 写入本地文件
        self.writer.write(entry)?;
        // 远程转发（失败不影响本地写入）
        if let Some(forwarder) = &self.forwarder {
            let _ = forwarder.forward(entry);
        }
        Ok(())
    }

    /// Flush 所有 BufWriter 句柄到磁盘（用于优雅关停或测试验证）
    pub fn flush(&self) -> Result<(), SyslogError> {
        self.writer.flush()
    }

    /// 重传缓存的远程日志
    pub fn retry_forward(&self) -> Result<usize, SyslogError> {
        if let Some(forwarder) = &self.forwarder {
            forwarder.retry_cached()
        } else {
            Ok(0)
        }
    }

    /// 动态调整全局日志级别
    pub fn set_global_level(&self, level: LogLevel) {
        self.writer.set_global_level(level);
    }

    /// 动态调整分类日志级别
    pub fn set_category_level(&self, category: LogCategory, level: LogLevel) {
        self.writer.set_category_level(category, level);
    }

    /// 热重载配置（重新加载配置文件，更新 writer/forwarder，保留 cache）
    pub fn reload(&mut self, path: &Path) -> Result<(), SyslogError> {
        let content = std::fs::read_to_string(path)?;
        let config: SyslogConfig =
            toml::from_str(&content).map_err(|e| SyslogError::Config(e.to_string()))?;

        // TLS fail-fast
        for target in &config.forward.targets {
            if target.transport == Transport::Tls {
                return Err(SyslogError::Config(
                    "TLS transport not yet supported, use Tcp or Udp".into(),
                ));
            }
        }
        // category_levels key 校验
        for key in config.category_levels.keys() {
            if !is_valid_category_key(key) {
                return Err(SyslogError::Config(format!("unknown category: {key}")));
            }
        }

        // 更新 writer 配置（锁内）
        self.writer.update_config(config.clone());

        // 更新 forwarder 配置（保留 cache）
        if let Some(ref forwarder) = self.forwarder {
            forwarder.update_config(&config.forward);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error > LogLevel::Warn);
        assert!(LogLevel::Warn > LogLevel::Info);
        assert!(LogLevel::Info > LogLevel::Debug);
        assert!(LogLevel::Debug > LogLevel::Trace);
    }

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::parse_level("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse_level("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse_level("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse_level("invalid"), None);
    }

    #[test]
    fn test_log_level_rfc5424_severity() {
        assert_eq!(LogLevel::Info.rfc5424_severity(), 6);
        assert_eq!(LogLevel::Warn.rfc5424_severity(), 4);
        assert_eq!(LogLevel::Error.rfc5424_severity(), 3);
    }

    #[test]
    fn test_log_category_rfc5424_facility() {
        assert_eq!(LogCategory::System.rfc5424_facility(), 3);
        assert_eq!(LogCategory::Security.rfc5424_facility(), 4);
        assert_eq!(LogCategory::Audit.rfc5424_facility(), 10);
    }

    #[test]
    fn test_log_entry_jsonl() {
        let entry = LogEntry::new(
            LogLevel::Info,
            LogCategory::System,
            "init",
            "system started",
        );
        let jsonl = entry.to_jsonl().unwrap();
        assert!(jsonl.contains("\"level\":\"info\""));
        assert!(jsonl.contains("\"category\":\"system\""));
        assert!(jsonl.contains("\"source\":\"init\""));
        assert!(jsonl.contains("\"message\":\"system started\""));
    }

    #[test]
    fn test_log_entry_rfc5424() {
        let entry = LogEntry::new(
            LogLevel::Warn,
            LogCategory::Security,
            "auth",
            "login failed",
        );
        let rfc = entry.to_rfc5424("eneros-001");
        // priority = facility(4) * 8 + severity(4) = 36
        assert!(rfc.starts_with("<36>1 "));
        assert!(rfc.contains("eneros-001"));
        assert!(rfc.contains("category=\"security\""));
        assert!(rfc.contains("source=\"auth\""));
        assert!(rfc.ends_with("login failed\n"));
    }

    #[test]
    fn test_rfc5424_appname_is_source_procid_is_pid() {
        // APP-NAME 放 source，PROCID 用 std::process::id()
        let entry = LogEntry::new(
            LogLevel::Info,
            LogCategory::System,
            "my-module",
            "hello",
        );
        let rfc = entry.to_rfc5424("host1");
        // 格式: <pri>1 ts hostname app-name procid msgid sd msg
        let parts: Vec<&str> = rfc.split_whitespace().collect();
        // parts[0]=<pri>1, parts[1]=ts, parts[2]=hostname, parts[3]=app-name, parts[4]=procid
        assert_eq!(parts[2], "host1");
        assert_eq!(parts[3], "my-module");
        assert_eq!(parts[4], std::process::id().to_string());
    }

    #[test]
    fn test_escape_sd_value() {
        // 反斜杠、双引号、右方括号均需转义
        assert_eq!(escape_sd_value(r#"a\b"#), r#"a\\b"#);
        assert_eq!(escape_sd_value(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape_sd_value("a]b"), r#"a\]b"#);
    }

    #[test]
    fn test_config_default() {
        let config = SyslogConfig::default();
        assert_eq!(config.global_level, LogLevel::Info);
        assert_eq!(config.log_dir, PathBuf::from("/var/log/eneros"));
        assert_eq!(config.rotate.retention_days, 7);
        assert!(config.rotate.compress);
        assert!(config.forward.targets.is_empty());
    }

    #[test]
    fn test_config_parse() {
        let toml_str = r#"
log_dir = "/var/log/eneros"
global_level = "debug"

[rotate]
policy = "Daily"
max_files = 14
compress = true
retention_days = 14

[[forward.targets]]
addr = "192.168.1.100:514"
transport = "tcp"
min_level = "warn"

[[forward.targets]]
addr = "192.168.1.101:514"
transport = "udp"
min_level = "info"

[forward]
cache_size = 5000
retry_interval_secs = 60

[category_levels]
system = "info"
audit = "debug"
"#;
        let config: SyslogConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.global_level, LogLevel::Debug);
        assert_eq!(config.rotate.retention_days, 14);
        assert_eq!(config.forward.targets.len(), 2);
        assert_eq!(config.forward.targets[0].transport, Transport::Tcp);
        assert_eq!(config.forward.targets[1].min_level, LogLevel::Info);
        assert_eq!(
            config.category_levels.get("audit"),
            Some(&LogLevel::Debug)
        );
    }

    #[test]
    fn test_should_log_global_level() {
        let config = SyslogConfig::default(); // global = Info
        let writer = LogWriter::new(config);

        let debug_entry = LogEntry::new(LogLevel::Debug, LogCategory::System, "test", "debug");
        let info_entry = LogEntry::new(LogLevel::Info, LogCategory::System, "test", "info");

        assert!(!writer.should_log(&debug_entry)); // Debug < Info
        assert!(writer.should_log(&info_entry)); // Info >= Info
    }

    #[test]
    fn test_should_log_category_override() {
        let mut config = SyslogConfig::default(); // global = Info
        config
            .category_levels
            .insert("audit".to_string(), LogLevel::Debug);
        let writer = LogWriter::new(config);

        let audit_debug = LogEntry::new(LogLevel::Debug, LogCategory::Audit, "test", "debug");
        let system_debug = LogEntry::new(LogLevel::Debug, LogCategory::System, "test", "debug");

        assert!(writer.should_log(&audit_debug)); // Audit override = Debug
        assert!(!writer.should_log(&system_debug)); // System uses global = Info
    }

    #[test]
    fn test_set_global_level() {
        let config = SyslogConfig::default();
        let writer = LogWriter::new(config);
        assert_eq!(writer.config().global_level, LogLevel::Info);

        writer.set_global_level(LogLevel::Debug);
        assert_eq!(writer.config().global_level, LogLevel::Debug);
    }

    #[test]
    fn test_set_category_level() {
        let config = SyslogConfig::default();
        let writer = LogWriter::new(config);

        writer.set_category_level(LogCategory::Protocol, LogLevel::Trace);
        assert_eq!(
            writer.config().category_levels.get("protocol"),
            Some(&LogLevel::Trace)
        );
    }

    #[test]
    fn test_forwarder_cache() {
        let config = ForwardConfig {
            targets: vec![ForwardTarget {
                addr: "127.0.0.1:9999".to_string(),
                transport: Transport::Tcp,
                min_level: LogLevel::Info,
            }],
            cache_size: 100,
            retry_interval_secs: 30,
        };
        let forwarder = LogForwarder::new(config, "test-host");

        assert_eq!(forwarder.pending_count(), 0);

        // 手动缓存
        let entry = LogEntry::new(LogLevel::Info, LogCategory::System, "test", "msg");
        forwarder.cache_push(entry, "test line".to_string());
        assert_eq!(forwarder.pending_count(), 1);
    }

    #[test]
    fn test_forwarder_cache_overflow() {
        let config = ForwardConfig {
            targets: vec![],
            cache_size: 2,
            retry_interval_secs: 30,
        };
        let forwarder = LogForwarder::new(config, "test-host");

        for i in 0..5 {
            let entry = LogEntry::new(LogLevel::Info, LogCategory::System, "test", format!("msg{i}"));
            forwarder.cache_push(entry, format!("line{i}"));
        }

        // cache_size=2, 低优先级日志满时丢弃新日志，应只保留前 2 条
        assert_eq!(forwarder.pending_count(), 2);
    }

    #[test]
    fn test_forwarder_cache_high_priority_evicts_oldest() {
        let config = ForwardConfig {
            targets: vec![],
            cache_size: 2,
            retry_interval_secs: 30,
        };
        let forwarder = LogForwarder::new(config, "test-host");

        // 填满缓存（低优先级）
        for i in 0..2 {
            let entry = LogEntry::new(LogLevel::Info, LogCategory::System, "test", format!("msg{i}"));
            forwarder.cache_push(entry, format!("line{i}"));
        }
        assert_eq!(forwarder.pending_count(), 2);

        // 高优先级日志应驱逐最旧
        let entry = LogEntry::new(LogLevel::Error, LogCategory::System, "test", "critical");
        forwarder.cache_push(entry, "critical line".to_string());
        assert_eq!(forwarder.pending_count(), 2); // 仍然 2（驱逐最旧，加入新的）
    }

    #[test]
    fn test_rotate_policy_default() {
        match RotatePolicy::default() {
            RotatePolicy::Both(size) => assert_eq!(size, 100 * 1024 * 1024),
            _ => panic!("expected Both"),
        }
    }

    // -----------------------------------------------------------------------
    // 新增测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_write_safety() {
        // 多线程并发写入，验证无 panic（线程安全验证）
        let dir = std::env::temp_dir().join("eneros-syslog-concurrent-test");
        let _ = std::fs::remove_dir_all(&dir);

        let config = SyslogConfig {
            log_dir: dir.clone(),
            ..Default::default()
        };
        let writer = LogWriter::new(config);

        std::thread::scope(|s| {
            for i in 0..4 {
                let w = &writer;
                s.spawn(move || {
                    for j in 0..50 {
                        let entry = LogEntry::new(
                            LogLevel::Info,
                            LogCategory::System,
                            "test",
                            format!("msg {}-{}", i, j),
                        );
                        let _ = w.write(&entry);
                    }
                });
            }
        });

        // 验证文件已创建且包含内容
        let log_file = dir.join("system.log");
        assert!(log_file.exists());
        let content = std::fs::read_to_string(&log_file).unwrap();
        assert!(content.contains("msg"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tls_config_rejected() {
        // TLS 配置在 SyslogManager::new 阶段被拒绝
        let config = SyslogConfig {
            forward: ForwardConfig {
                targets: vec![ForwardTarget {
                    addr: "192.168.1.100:6514".to_string(),
                    transport: Transport::Tls,
                    min_level: LogLevel::Info,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let result = SyslogManager::new(config);
        assert!(matches!(result, Err(SyslogError::Config(_))));
    }

    #[test]
    fn test_category_levels_unknown_key() {
        // 未知 category key 报错
        let mut config = SyslogConfig::default();
        config
            .category_levels
            .insert("unknown".to_string(), LogLevel::Info);
        let result = SyslogManager::new(config);
        assert!(matches!(result, Err(SyslogError::Config(_))));
    }

    #[test]
    fn test_category_levels_valid_keys_accepted() {
        // 所有合法 category key 应被接受
        let mut config = SyslogConfig::default();
        for key in &["system", "agent", "protocol", "security", "audit"] {
            config
                .category_levels
                .insert(key.to_string(), LogLevel::Debug);
        }
        let result = SyslogManager::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_daily_rotation_per_category() {
        // 多分类按天轮转均独立触发
        let dir = std::env::temp_dir().join("eneros-syslog-rotation-per-cat-test");
        let _ = std::fs::remove_dir_all(&dir);

        let config = SyslogConfig {
            log_dir: dir.clone(),
            rotate: RotateConfig {
                policy: RotatePolicy::Daily,
                ..Default::default()
            },
            ..Default::default()
        };
        let writer = LogWriter::new(config);

        // 写入初始条目
        let entry1 = LogEntry::new(LogLevel::Info, LogCategory::System, "test", "msg1");
        writer.write(&entry1).unwrap();
        let entry2 = LogEntry::new(LogLevel::Info, LogCategory::Agent, "test", "msg2");
        writer.write(&entry2).unwrap();

        // 将 System 分类日期设为过去，触发轮转
        writer.set_category_date_for_test(LogCategory::System, "2020-01-01");

        // 写入 System — 应触发轮转
        let entry3 = LogEntry::new(LogLevel::Info, LogCategory::System, "test", "msg3");
        writer.write(&entry3).unwrap();

        // System 应有轮转文件
        let has_rotated_system = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("system.log.")
            });
        assert!(
            has_rotated_system,
            "System category should have rotated file"
        );

        // Agent 不应轮转（日期仍是今天）
        let has_rotated_agent = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("agent.log.")
            });
        assert!(
            !has_rotated_agent,
            "Agent category should not have rotated"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_max_files_enforced() {
        // max_files 限制轮转文件数
        let dir = std::env::temp_dir().join("eneros-syslog-max-files-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 手动创建 10 个轮转文件
        for i in 0..10 {
            let ts = format!("2024010{}_000000", i);
            let path = dir.join(format!("system.log.{}", ts));
            std::fs::write(&path, "old log").unwrap();
        }

        let config = SyslogConfig {
            log_dir: dir.clone(),
            rotate: RotateConfig {
                max_files: 3,
                retention_days: 365, // 高保留天数，避免按天数删除
                ..Default::default()
            },
            ..Default::default()
        };
        let writer = LogWriter::new(config);

        // 写入一条日志触发 cleanup（首次 cleanup 不受限流影响）
        let entry = LogEntry::new(LogLevel::Info, LogCategory::System, "test", "msg");
        let _ = writer.write(&entry);

        // 计算剩余轮转文件数
        let rotated_count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("system.log.")
            })
            .count();

        assert!(
            rotated_count <= 3,
            "should have at most 3 rotated files, got {}",
            rotated_count
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_syslog_manager_new_returns_result() {
        // SyslogManager::new 现在返回 Result
        let config = SyslogConfig::default();
        let manager = SyslogManager::new(config);
        assert!(manager.is_ok());
    }

    #[test]
    fn test_syslog_manager_log_no_forwarder() {
        // 无转发目标时 log() 仍可正常写入本地文件
        let dir = std::env::temp_dir().join("eneros-syslog-manager-log-test");
        let _ = std::fs::remove_dir_all(&dir);

        let config = SyslogConfig {
            log_dir: dir.clone(),
            ..Default::default()
        };
        let manager = SyslogManager::new(config).unwrap();

        let entry = LogEntry::new(LogLevel::Info, LogCategory::System, "test", "hello");
        manager.log(&entry).unwrap();
        // Info 级别不会立即 flush（仅 Error/Audit 或计数达阈值），需手动 flush 确保落盘
        manager.flush().unwrap();

        let log_file = dir.join("system.log");
        assert!(log_file.exists());
        let content = std::fs::read_to_string(&log_file).unwrap();
        assert!(content.contains("hello"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_syslog_manager_set_levels() {
        // set_global_level / set_category_level 为 &self
        let config = SyslogConfig::default();
        let manager = SyslogManager::new(config).unwrap();

        manager.set_global_level(LogLevel::Debug);
        assert_eq!(manager.config().global_level, LogLevel::Debug);

        manager.set_category_level(LogCategory::Audit, LogLevel::Trace);
        assert_eq!(
            manager.config().category_levels.get("audit"),
            Some(&LogLevel::Trace)
        );
    }
}
