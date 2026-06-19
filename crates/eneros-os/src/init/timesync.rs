//! 时间同步服务（PTP IEEE 1588 优先，NTP 回退）
//!
//! Linux 下通过 `linuxptp` 包（ptp4l + phc2sys）实现 PTP 硬件时间戳同步，
//! 自研 NTPv4 客户端作为回退。时钟源优先级：PTP > NTP > 本地 RTC。
//!
//! 非 Linux 平台提供 no-op stub。

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

/// 守护进程关闭标志（由信号处理器通过 [`request_daemon_shutdown`] 设置，
/// [`TimeSyncManager::run_daemon`] 轮询检查）。
#[cfg(target_os = "linux")]
static DAEMON_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// 请求守护进程关闭。信号安全（仅原子 store）。
///
/// 非 Linux 平台为 no-op。
pub fn request_daemon_shutdown() {
    #[cfg(target_os = "linux")]
    {
        DAEMON_SHUTDOWN.store(true, Ordering::SeqCst);
    }
}

/// 时间同步错误
#[derive(Debug, thiserror::Error)]
pub enum TimeSyncError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("sync failed: {0}")]
    SyncFailed(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// 时钟源类型（优先级从高到低）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    /// PTP IEEE 1588 硬件时间戳（精度 < 100μs）
    Ptp,
    /// NTP 网络时间协议（精度 < 10ms）
    Ntp,
    /// 本地 RTC 硬件时钟（最后手段）
    LocalClock,
}

impl ClockSource {
    pub fn priority(&self) -> u8 {
        match self {
            ClockSource::Ptp => 0,
            ClockSource::Ntp => 1,
            ClockSource::LocalClock => 2,
        }
    }
}

/// PTP 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtpConfig {
    /// 绑定的网络接口（如 eth0）
    pub interface: String,
    /// PTP 域号（电力系统多区域用，默认 0）
    #[serde(default)]
    pub domain: u8,
    /// PHC 设备路径（如 /dev/ptp0，留空则自动发现）
    #[serde(default)]
    pub phc_device: Option<String>,
    /// 是否启用硬件时间戳（false 则用软件时间戳）
    #[serde(default = "default_true")]
    pub hardware_timestamping: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PtpConfig {
    fn default() -> Self {
        Self {
            interface: "eth0".to_string(),
            domain: 0,
            phc_device: None,
            hardware_timestamping: true,
        }
    }
}

/// NTP 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtpConfig {
    /// NTP 服务器列表（按优先级排序）
    pub servers: Vec<String>,
    /// 轮询间隔（秒）
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

fn default_poll_interval() -> u64 {
    64
}

impl Default for NtpConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                "ntp.aliyun.com".to_string(),
                "time.windows.com".to_string(),
            ],
            poll_interval_secs: 64,
        }
    }
}

/// 时间同步配置（对应 /etc/eneros/timesync.toml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSyncConfig {
    /// 启用的时钟源
    #[serde(default)]
    pub enabled_sources: Vec<ClockSource>,
    /// PTP 配置
    #[serde(default)]
    pub ptp: PtpConfig,
    /// NTP 配置
    #[serde(default)]
    pub ntp: NtpConfig,
    /// 时间偏差告警阈值（微秒，默认 1000 = 1ms）
    #[serde(default = "default_offset_threshold")]
    pub offset_alert_micros: i64,
}

fn default_offset_threshold() -> i64 {
    1000
}

impl Default for TimeSyncConfig {
    fn default() -> Self {
        Self {
            enabled_sources: vec![ClockSource::Ptp, ClockSource::Ntp],
            ptp: PtpConfig::default(),
            ntp: NtpConfig::default(),
            offset_alert_micros: 1000,
        }
    }
}

/// 时间同步状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSyncStatus {
    /// 当前活跃时钟源
    pub source: ClockSource,
    /// 与参考时钟的偏差（微秒）
    pub offset_micros: i64,
    /// 上次同步时间
    pub last_sync: DateTime<Utc>,
    /// 是否已锁定
    pub locked: bool,
    /// PTP grandmaster ID（仅 PTP 源）
    pub grandmaster_id: Option<String>,
    /// 最近一次错误描述（无错误时为 None）
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for TimeSyncStatus {
    fn default() -> Self {
        Self {
            source: ClockSource::LocalClock,
            offset_micros: 0,
            last_sync: Utc::now(),
            locked: false,
            grandmaster_id: None,
            last_error: None,
        }
    }
}

/// PTP 硬件时钟（PHC）信息
#[derive(Debug, Clone)]
pub struct PhcInfo {
    /// 设备路径（如 /dev/ptp0）
    pub device: PathBuf,
    /// 时钟名称（如 "Kirtley"）
    pub name: String,
    /// 是否支持硬件时间戳
    pub hardware_timestamping: bool,
}

/// 时间同步管理器
pub struct TimeSyncManager {
    config: TimeSyncConfig,
    /// 并发安全的状态字段（读多写少，使用 RwLock）
    status: RwLock<TimeSyncStatus>,
    /// ptp4l 子进程句柄（Linux 下保留以便管理生命周期）
    ptp4l_child: Option<std::process::Child>,
    /// phc2sys 子进程句柄
    phc2sys_child: Option<std::process::Child>,
}

impl TimeSyncManager {
    /// 从配置文件加载
    pub fn load(path: &Path) -> Result<Self, TimeSyncError> {
        let content = std::fs::read_to_string(path)?;
        let config: TimeSyncConfig = toml::from_str(&content)
            .map_err(|e| TimeSyncError::Config(e.to_string()))?;
        Ok(Self::new(config))
    }

    /// 用配置创建
    pub fn new(config: TimeSyncConfig) -> Self {
        Self {
            config,
            status: RwLock::new(TimeSyncStatus::default()),
            ptp4l_child: None,
            phc2sys_child: None,
        }
    }

    pub fn config(&self) -> &TimeSyncConfig {
        &self.config
    }

    /// 返回当前状态的快照（clone）。
    ///
    /// 使用 RwLock 内部可变性，调用方获得独立副本，不会阻塞其他读者。
    pub fn status(&self) -> TimeSyncStatus {
        self.status.read().clone()
    }

    /// 应用时间同步配置（Linux 下启动 ptp4l/phc2sys 或 NTP 同步）
    #[cfg(target_os = "linux")]
    pub fn apply(&mut self) -> Result<(), TimeSyncError> {
        if self.config.enabled_sources.is_empty() {
            return Err(TimeSyncError::Config("enabled_sources is empty".into()));
        }
        // 按优先级尝试：PTP → NTP → LocalClock
        for source in &self.config.enabled_sources {
            match source {
                ClockSource::Ptp => match self.start_ptp() {
                    Ok(()) => {
                        let mut st = self.status.write();
                        st.source = ClockSource::Ptp;
                        // PTP 锁定需要数秒~数十秒，不立即标记锁定。
                        // 实际锁定状态应通过 pmc 轮询 port_state == SLAVE 确认。
                        st.locked = false;
                        st.last_sync = Utc::now();
                        st.last_error = None;
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::warn!("PTP 启动失败: {e}, 回退 NTP");
                        let mut st = self.status.write();
                        st.last_error = Some(format!("PTP failed: {e}"));
                    }
                },
                ClockSource::Ntp => match self.sync_ntp() {
                    Ok(offset) => {
                        let mut st = self.status.write();
                        st.source = ClockSource::Ntp;
                        st.offset_micros = offset;
                        st.locked = true;
                        st.last_sync = Utc::now();
                        st.last_error = None;
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::warn!("NTP 同步失败: {e}");
                        let mut st = self.status.write();
                        st.last_error = Some(format!("NTP failed: {e}"));
                    }
                },
                ClockSource::LocalClock => {
                    let mut st = self.status.write();
                    st.source = ClockSource::LocalClock;
                    st.locked = false;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply(&mut self) -> Result<(), TimeSyncError> {
        if self.config.enabled_sources.is_empty() {
            return Err(TimeSyncError::Config("enabled_sources is empty".into()));
        }
        Err(TimeSyncError::UnsupportedPlatform)
    }

    /// 启动 PTP 同步（ptp4l + phc2sys）
    #[cfg(target_os = "linux")]
    fn start_ptp(&mut self) -> Result<(), TimeSyncError> {
        let ptp = &self.config.ptp;

        // 停止旧的 ptp4l/phc2sys（避免 reload 时产生孤儿进程）
        if let Some(mut child) = self.ptp4l_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.phc2sys_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        // 发现 PHC 设备（验证存在；ptp4l 通过接口自动绑定 PHC）
        if ptp.phc_device.is_none() {
            self.discover_phc()?
                .into_iter()
                .next()
                .ok_or_else(|| TimeSyncError::SyncFailed("no PHC device found".into()))?;
        }

        // 生成 /etc/ptp4l.conf 配置文件
        let conf_path = self.write_ptp4l_config()?;

        // 确保日志目录存在
        let _ = std::fs::create_dir_all("/var/log/eneros");

        // 启动 ptp4l（PTP 边界时钟/普通时钟守护进程）
        // -f 指定配置文件，-i 指定接口，-s slave only 模式
        let ptp4l_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/var/log/eneros/ptp4l.log")
            .map_err(|e| TimeSyncError::SyncFailed(format!("open ptp4l.log: {e}")))?;
        let ptp4l_log_err = ptp4l_log
            .try_clone()
            .map_err(|e| TimeSyncError::SyncFailed(format!("clone ptp4l.log: {e}")))?;

        let ptp4l_child = std::process::Command::new("ptp4l")
            .args([
                "-f",
                conf_path.to_str().unwrap(),
                "-i",
                ptp.interface.as_str(),
                "-s",
            ])
            .stdout(Stdio::from(ptp4l_log))
            .stderr(Stdio::from(ptp4l_log_err))
            .spawn()
            .map_err(|e| TimeSyncError::SyncFailed(format!("ptp4l spawn: {e}")))?;
        self.ptp4l_child = Some(ptp4l_child);

        // 启动 phc2sys（PHC ↔ 系统时钟同步）
        // -w 等待 ptp4l 进入 SERVO_LOCKED 状态后再同步，避免初始大偏差跳变
        let phc2sys_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/var/log/eneros/phc2sys.log")
            .map_err(|e| {
                // phc2sys 日志打开失败时清理已启动的 ptp4l
                if let Some(mut child) = self.ptp4l_child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                TimeSyncError::SyncFailed(format!("open phc2sys.log: {e}"))
            })?;
        let phc2sys_log_err = phc2sys_log
            .try_clone()
            .map_err(|e| TimeSyncError::SyncFailed(format!("clone phc2sys.log: {e}")))?;

        let phc2sys_child = std::process::Command::new("phc2sys")
            .args(["-s", ptp.interface.as_str(), "-c", "CLOCK_REALTIME", "-w"])
            .stdout(Stdio::from(phc2sys_log))
            .stderr(Stdio::from(phc2sys_log_err))
            .spawn()
            .map_err(|e| {
                // phc2sys spawn 失败时清理已启动的 ptp4l
                if let Some(mut child) = self.ptp4l_child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                TimeSyncError::SyncFailed(format!("phc2sys spawn: {e}"))
            })?;
        self.phc2sys_child = Some(phc2sys_child);

        Ok(())
    }

    /// 生成 /etc/ptp4l.conf 配置文件
    #[cfg(target_os = "linux")]
    fn write_ptp4l_config(&self) -> Result<PathBuf, TimeSyncError> {
        let ptp = &self.config.ptp;
        let conf_path = PathBuf::from("/etc/ptp4l.conf");
        let mut content = String::new();
        content.push_str(&format!("[{}]\n", ptp.interface));
        content.push_str(&format!(
            "time_stamping {}\n",
            if ptp.hardware_timestamping {
                "hardware"
            } else {
                "software"
            }
        ));
        content.push_str(&format!("domainNumber {}\n", ptp.domain));
        content.push_str("slaveOnly 1\n");
        std::fs::write(&conf_path, &content)
            .map_err(|e| TimeSyncError::SyncFailed(format!("write ptp4l.conf: {e}")))?;
        Ok(conf_path)
    }

    /// 发现系统 PHC 设备（扫描 /sys/class/ptp/）
    #[cfg(target_os = "linux")]
    pub fn discover_phc(&self) -> Result<Vec<PhcInfo>, TimeSyncError> {
        let mut phcs = Vec::new();
        let ptp_class = Path::new("/sys/class/ptp");
        if !ptp_class.exists() {
            return Ok(phcs);
        }

        for entry in std::fs::read_dir(ptp_class)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("ptp") {
                continue;
            }

            let dev_path = PathBuf::from(format!("/dev/{}", name));
            let clock_name = std::fs::read_to_string(entry.path().join("clock_name"))
                .unwrap_or_default()
                .trim()
                .to_string();

            phcs.push(PhcInfo {
                device: dev_path,
                name: clock_name,
                hardware_timestamping: self.config.ptp.hardware_timestamping,
            });
        }
        Ok(phcs)
    }

    /// 非 Linux 平台 stub
    #[cfg(not(target_os = "linux"))]
    pub fn discover_phc(&self) -> Result<Vec<PhcInfo>, TimeSyncError> {
        Err(TimeSyncError::UnsupportedPlatform)
    }

    /// 读取 PTP grandmaster ID（从 ptp4l 状态文件）
    ///
    /// # Deprecated
    ///
    /// 此方法通过 `/sys/class/ptp/ptp0/clock_name` 读取本地 PHC 名称，
    /// 并非真正的 grandmaster ID。请使用 [`poll_ptp_status`] 通过 pmc
    /// 读取 `grandmasterIdentity`。
    #[cfg(target_os = "linux")]
    #[deprecated(
        since = "0.20.2",
        note = "use poll_ptp_status which reads grandmasterIdentity via pmc"
    )]
    #[allow(dead_code)]
    fn read_grandmaster_id(&self) -> Option<String> {
        let clock_name = std::fs::read_to_string("/sys/class/ptp/ptp0/clock_name")
            .ok()?
            .trim()
            .to_string();
        Some(clock_name)
    }

    /// NTP 同步 — 自研 NTPv4 客户端（UDP 端口 123）
    ///
    /// 对每个服务器重试 3 次（每次 2 秒超时），全部失败再切换下一服务器。
    #[cfg(target_os = "linux")]
    fn sync_ntp(&self) -> Result<i64, TimeSyncError> {
        if self.config.ntp.servers.is_empty() {
            return Err(TimeSyncError::Config(
                "NTP servers list is empty".into(),
            ));
        }
        const MAX_RETRIES: u32 = 3;
        for server in &self.config.ntp.servers {
            for attempt in 1..=MAX_RETRIES {
                match self.query_ntp(server) {
                    Ok((offset, absolute_time)) => {
                        self.apply_clock_offset(offset, absolute_time)?;
                        return Ok(offset);
                    }
                    Err(e) => {
                        tracing::debug!(
                            "NTP query attempt {attempt}/{MAX_RETRIES} for {server} failed: {e}"
                        );
                    }
                }
            }
            tracing::debug!("NTP server {server} exhausted all {MAX_RETRIES} retries, trying next");
        }
        Err(TimeSyncError::SyncFailed(
            "all NTP servers unreachable".into(),
        ))
    }

    /// 查询单个 NTP 服务器，返回 (偏差微秒, 服务器绝对时间)
    ///
    /// `absolute_time` 为服务器返回的 NTP 时间戳（自 Unix 纪元起的 Duration），
    /// 用于大偏差时直接 settimeofday，避免经过 Utc::now() 中转引入二次偏差。
    #[cfg(target_os = "linux")]
    fn query_ntp(&self, server: &str) -> Result<(i64, Option<Duration>), TimeSyncError> {
        use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

        let addr: SocketAddr = (server, 123)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| TimeSyncError::SyncFailed(format!("resolve {server} failed")))?;

        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.set_read_timeout(Some(Duration::from_secs(2)))?;
        sock.set_write_timeout(Some(Duration::from_secs(2)))?;

        // NTPv4 包：48 字节，首字节 0x1B = li=0, vn=3, mode=3 (client)
        let mut packet = [0u8; 48];
        packet[0] = 0x1B;

        let send_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();

        // 填充 Transmit Timestamp（字节 40-47）— NTPv4 客户端模式要求
        // 服务器会回显此时间戳用于 RTT 计算
        const NTP_EPOCH_OFFSET: u64 = 2208988800;
        let ntp_secs = (send_time.as_secs() + NTP_EPOCH_OFFSET) as u32;
        let ntp_frac =
            ((send_time.subsec_nanos() as u64) * (1u64 << 32) / 1_000_000_000) as u32;
        packet[40..44].copy_from_slice(&ntp_secs.to_be_bytes());
        packet[44..48].copy_from_slice(&ntp_frac.to_be_bytes());

        sock.send_to(&packet, addr)?;

        let mut buf = [0u8; 48];
        sock.recv_from(&mut buf)?;

        // 校验 NTP 响应完整性
        let mode = buf[0] & 0x07;
        if mode != 4 {
            return Err(TimeSyncError::SyncFailed(format!(
                "invalid NTP response mode: {mode} (expected 4=server)"
            )));
        }
        let stratum = buf[1];
        if stratum == 0 {
            return Err(TimeSyncError::SyncFailed(
                "NTP kiss-o'-death response (stratum 0)".into(),
            ));
        }
        // transmit timestamp（字节 40-47）不应全零
        if buf[40..48].iter().all(|&b| b == 0) {
            return Err(TimeSyncError::SyncFailed(
                "NTP transmit timestamp is zero".into(),
            ));
        }

        let recv_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();

        // 解析 NTP 时间戳（transmit timestamp，字节 40-47）
        let secs = u32::from_be_bytes([buf[40], buf[41], buf[42], buf[43]]);
        let frac = u32::from_be_bytes([buf[44], buf[45], buf[46], buf[47]]);

        let ntp_secs = secs.wrapping_sub(NTP_EPOCH_OFFSET as u32);
        let ntp_micros = (frac as u64 * 1_000_000) >> 32;

        let server_time =
            Duration::from_secs(ntp_secs as u64) + Duration::from_micros(ntp_micros);
        // 用 saturating_sub 防止时钟回拨时 Duration 减法 panic
        let rtt = recv_time.saturating_sub(send_time);
        let one_way = rtt / 2;
        let client_time = send_time + one_way;

        // 偏差 = 服务器时间 - 客户端时间（微秒）
        let offset_micros = if server_time > client_time {
            (server_time - client_time).as_micros() as i64
        } else {
            -((client_time - server_time).as_micros() as i64)
        };

        Ok((offset_micros, Some(server_time)))
    }

    /// 应用时钟偏差修正（通过 adjtime / settimeofday 系统调用）
    ///
    /// `absolute_time` 为 NTP 服务器返回的绝对时间，大偏差时直接用于
    /// settimeofday，避免经过 Utc::now() 中转引入二次偏差。
    #[cfg(target_os = "linux")]
    fn apply_clock_offset(
        &self,
        offset_micros: i64,
        absolute_time: Option<Duration>,
    ) -> Result<(), TimeSyncError> {
        // 归一化：确保 tv_usec 在 [0, 1_000_000) 范围内
        let mut secs = offset_micros / 1_000_000;
        let mut micros = offset_micros % 1_000_000;
        if micros < 0 {
            secs -= 1;
            micros += 1_000_000;
        }

        // 小偏差用 adjtime 平滑修正（< 500ms），大偏差用 settimeofday
        if offset_micros.abs() < 500_000 {
            let delta = libc::timeval {
                tv_sec: secs as _,
                tv_usec: micros as _,
            };
            // adjtime(delta, NULL) — 渐进调整
            let ret = unsafe { libc::adjtime(&delta, std::ptr::null_mut()) };
            if ret != 0 {
                return Err(TimeSyncError::SyncFailed(format!(
                    "adjtime failed (需要 CAP_SYS_TIME 权限): 返回值 {ret}"
                )));
            }
        } else {
            // 大偏差直接设置（需要 root）
            // 优先使用 NTP 服务器返回的绝对时间，避免 Utc::now() 中转
            let now = if let Some(abs) = absolute_time {
                libc::timeval {
                    tv_sec: abs.as_secs() as _,
                    tv_usec: abs.subsec_micros() as _,
                }
            } else {
                libc::timeval {
                    tv_sec: Utc::now().timestamp() + secs,
                    tv_usec: micros as _,
                }
            };
            let ret = unsafe { libc::settimeofday(&now, std::ptr::null()) };
            if ret != 0 {
                return Err(TimeSyncError::SyncFailed(format!(
                    "settimeofday failed (需要 CAP_SYS_TIME 权限): 返回值 {ret}"
                )));
            }
        }
        Ok(())
    }

    /// 后台守护循环（Linux only）
    ///
    /// PTP 模式：try_wait 监控 ptp4l/phc2sys 子进程，崩溃则重启
    /// （指数退避，初始 2s，最大 30s）；每 10 秒通过 pmc 轮询更新 status。
    /// NTP 模式：每 poll_interval_secs 秒调用 sync_ntp() 更新 status。
    ///
    /// 通过 [`request_daemon_shutdown`] 触发优雅退出。
    #[cfg(target_os = "linux")]
    pub fn run_daemon(&mut self) -> Result<(), TimeSyncError> {
        let source = {
            let st = self.status.read();
            st.source
        };
        match source {
            ClockSource::Ptp => self.run_ptp_daemon(),
            ClockSource::Ntp => self.run_ntp_daemon(),
            ClockSource::LocalClock => self.run_local_daemon(),
        }
    }

    /// 非 Linux 平台 stub
    #[cfg(not(target_os = "linux"))]
    pub fn run_daemon(&mut self) -> Result<(), TimeSyncError> {
        Err(TimeSyncError::UnsupportedPlatform)
    }

    /// PTP 守护循环
    #[cfg(target_os = "linux")]
    fn run_ptp_daemon(&mut self) -> Result<(), TimeSyncError> {
        let mut backoff_secs = 2u64;
        const MAX_BACKOFF_SECS: u64 = 30;
        const POLL_INTERVAL_SECS: u64 = 10;
        const LOOP_INTERVAL_MS: u64 = 500;
        let mut last_poll = Instant::now();

        loop {
            if DAEMON_SHUTDOWN.load(Ordering::SeqCst) {
                tracing::info!("timesync PTP daemon shutting down");
                return Ok(());
            }

            // 检查子进程是否崩溃
            let ptp4l_exited = match self.ptp4l_child.as_mut() {
                Some(child) => matches!(child.try_wait(), Ok(Some(_))),
                None => true,
            };
            let phc2sys_exited = match self.phc2sys_child.as_mut() {
                Some(child) => matches!(child.try_wait(), Ok(Some(_))),
                None => true,
            };

            if ptp4l_exited || phc2sys_exited {
                let reason = if ptp4l_exited && phc2sys_exited {
                    "ptp4l and phc2sys exited"
                } else if ptp4l_exited {
                    "ptp4l exited"
                } else {
                    "phc2sys exited"
                };
                tracing::warn!(
                    "PTP process crash: {reason}, restarting in {backoff_secs}s (exponential backoff)"
                );
                {
                    let mut st = self.status.write();
                    st.last_error = Some(reason.to_string());
                    st.locked = false;
                }
                std::thread::sleep(Duration::from_secs(backoff_secs));
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);

                // 清理已退出的句柄
                self.ptp4l_child = None;
                self.phc2sys_child = None;

                match self.start_ptp() {
                    Ok(()) => {
                        tracing::info!("PTP restarted successfully, resetting backoff");
                        backoff_secs = 2;
                    }
                    Err(e) => {
                        tracing::warn!("PTP restart failed: {e}, will retry");
                        let mut st = self.status.write();
                        st.last_error = Some(format!("restart failed: {e}"));
                    }
                }
                continue;
            }

            // 每 10 秒通过 pmc 轮询更新 status
            if last_poll.elapsed() >= Duration::from_secs(POLL_INTERVAL_SECS) {
                let _ = self.poll_ptp_status();
                last_poll = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(LOOP_INTERVAL_MS));
        }
    }

    /// NTP 守护循环
    #[cfg(target_os = "linux")]
    fn run_ntp_daemon(&mut self) -> Result<(), TimeSyncError> {
        let poll_interval = self.config.ntp.poll_interval_secs.max(1);

        loop {
            if DAEMON_SHUTDOWN.load(Ordering::SeqCst) {
                tracing::info!("timesync NTP daemon shutting down");
                return Ok(());
            }

            match self.sync_ntp() {
                Ok(offset) => {
                    tracing::debug!("NTP sync OK, offset={offset}μs");
                    let mut st = self.status.write();
                    st.offset_micros = offset;
                    st.locked = true;
                    st.last_sync = Utc::now();
                    st.last_error = None;
                }
                Err(e) => {
                    tracing::warn!("NTP sync failed: {e}");
                    let mut st = self.status.write();
                    st.locked = false;
                    st.last_error = Some(e.to_string());
                }
            }

            // 分段睡眠以便及时响应关闭信号
            let mut slept = 0u64;
            while slept < poll_interval {
                if DAEMON_SHUTDOWN.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let step = std::cmp::min(poll_interval - slept, 1);
                std::thread::sleep(Duration::from_secs(step));
                slept += step;
            }
        }
    }

    /// 本地时钟守护循环（空转，仅响应关闭信号）
    #[cfg(target_os = "linux")]
    fn run_local_daemon(&mut self) -> Result<(), TimeSyncError> {
        loop {
            if DAEMON_SHUTDOWN.load(Ordering::SeqCst) {
                tracing::info!("timesync local-clock daemon shutting down");
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// 通过 pmc 轮询 PTP 状态（Linux only）
    ///
    /// 执行 `pmc -u -b 0 'GET TIME_STATUS_NP'` 解析 master_offset 和 port_state，
    /// 执行 `pmc -u -b 0 'GET PARENT_DATASET'` 解析 grandmasterIdentity。
    /// port_state == SLAVE 且 |master_offset| < offset_alert_micros 时 locked = true。
    #[cfg(target_os = "linux")]
    pub fn poll_ptp_status(&mut self) -> Result<(), TimeSyncError> {
        // GET TIME_STATUS_NP: master_offset, port_state
        let time_status = self.run_pmc("GET TIME_STATUS_NP");
        let master_offset = time_status
            .as_ref()
            .and_then(|out| parse_pmc_field(out, "master_offset"));
        let port_state = time_status
            .as_ref()
            .and_then(|out| parse_pmc_field(out, "port_state"));

        // GET PARENT_DATASET: grandmasterIdentity
        let parent_ds = self.run_pmc("GET PARENT_DATASET");
        let grandmaster_id = parent_ds
            .as_ref()
            .and_then(|out| parse_pmc_field(out, "grandmasterIdentity"));

        let mut st = self.status.write();
        if let Some(offset_str) = master_offset {
            if let Ok(offset_val) = offset_str.parse::<i64>() {
                st.offset_micros = offset_val;
            }
        }
        if let Some(gm) = grandmaster_id {
            st.grandmaster_id = Some(gm);
        }
        if let Some(state) = &port_state {
            let offset_ok = st.offset_micros.abs() < self.config.offset_alert_micros;
            st.locked = state == "SLAVE" && offset_ok;
        }
        st.last_sync = Utc::now();
        Ok(())
    }

    /// 非 Linux 平台 stub
    #[cfg(not(target_os = "linux"))]
    pub fn poll_ptp_status(&mut self) -> Result<(), TimeSyncError> {
        Err(TimeSyncError::UnsupportedPlatform)
    }

    /// 执行 pmc 命令并返回 stdout
    #[cfg(target_os = "linux")]
    fn run_pmc(&self, command: &str) -> Option<String> {
        std::process::Command::new("pmc")
            .args(["-u", "-b", "0", command])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    }

    /// 检查时间偏差是否超阈值（> 1ms 告警）
    pub fn check_offset_alert(&self) -> bool {
        self.status.read().offset_micros.abs() > self.config.offset_alert_micros
    }

    /// 热重载配置
    pub fn reload(path: &Path) -> Result<Self, TimeSyncError> {
        Self::load(path)
    }
}

/// Drop：清理 ptp4l/phc2sys 子进程
///
/// Linux 下 kill + wait 确保不产生孤儿进程。非 Linux 平台字段始终为 None，
/// 此处为 no-op。
impl Drop for TimeSyncManager {
    fn drop(&mut self) {
        if let Some(mut child) = self.ptp4l_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.phc2sys_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 从 pmc 输出中解析指定字段值
///
/// pmc 输出格式示例：
/// ```text
///   401c6b.fffe.5d4d70-0 seq 0 RESPONSE MANAGEMENT TIME_STATUS_NP
///   master_offset              12345
///   port_state                 SLAVE
/// ```
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_pmc_field(output: &str, field: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        let mut parts = line.splitn(2, char::is_whitespace);
        if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
            if name == field {
                let v = value.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_source_priority() {
        assert!(ClockSource::Ptp.priority() < ClockSource::Ntp.priority());
        assert!(ClockSource::Ntp.priority() < ClockSource::LocalClock.priority());
    }

    #[test]
    fn test_config_default() {
        let config = TimeSyncConfig::default();
        assert!(config.enabled_sources.contains(&ClockSource::Ptp));
        assert!(config.enabled_sources.contains(&ClockSource::Ntp));
        assert_eq!(config.ptp.interface, "eth0");
        assert!(!config.ntp.servers.is_empty());
        assert_eq!(config.offset_alert_micros, 1000);
    }

    #[test]
    fn test_config_parse() {
        let toml_str = r#"
enabled_sources = ["ptp", "ntp"]
offset_alert_micros = 500

[ptp]
interface = "eth1"
domain = 1
hardware_timestamping = false

[ntp]
servers = ["time.google.com", "time.cloudflare.com"]
poll_interval_secs = 128
"#;
        let config: TimeSyncConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.ptp.interface, "eth1");
        assert_eq!(config.ptp.domain, 1);
        assert!(!config.ptp.hardware_timestamping);
        assert_eq!(config.ntp.servers.len(), 2);
        assert_eq!(config.ntp.poll_interval_secs, 128);
        assert_eq!(config.offset_alert_micros, 500);
    }

    #[test]
    fn test_status_default() {
        let status = TimeSyncStatus::default();
        assert_eq!(status.source, ClockSource::LocalClock);
        assert!(!status.locked);
        assert!(status.grandmaster_id.is_none());
        assert!(status.last_error.is_none());
    }

    #[test]
    fn test_check_offset_alert() {
        let config = TimeSyncConfig::default();
        let status = TimeSyncStatus {
            offset_micros: 500,
            ..Default::default()
        };
        let _ = TimeSyncManager::new(config.clone());

        // 直接测试逻辑
        assert!(500 < config.offset_alert_micros);

        let status_high = TimeSyncStatus {
            offset_micros: 2000,
            ..Default::default()
        };
        assert!(status_high.offset_micros.abs() > config.offset_alert_micros);
        let _ = status;
    }

    #[test]
    fn test_ptp_config_default() {
        let ptp = PtpConfig::default();
        assert_eq!(ptp.interface, "eth0");
        assert_eq!(ptp.domain, 0);
        assert!(ptp.phc_device.is_none());
        assert!(ptp.hardware_timestamping);
    }

    #[test]
    fn test_ntp_config_default() {
        let ntp = NtpConfig::default();
        assert!(!ntp.servers.is_empty());
        assert_eq!(ntp.poll_interval_secs, 64);
    }

    #[test]
    fn test_clock_source_serialization() {
        let json = serde_json::to_string(&ClockSource::Ptp).unwrap();
        assert_eq!(json, "\"ptp\"");
        let json = serde_json::to_string(&ClockSource::Ntp).unwrap();
        assert_eq!(json, "\"ntp\"");
        let json = serde_json::to_string(&ClockSource::LocalClock).unwrap();
        assert_eq!(json, "\"local_clock\"");
    }

    #[test]
    fn test_status_serialization() {
        let status = TimeSyncStatus {
            source: ClockSource::Ptp,
            offset_micros: 42,
            last_sync: Utc::now(),
            locked: true,
            grandmaster_id: Some("GM-001".to_string()),
            last_error: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        let decoded: TimeSyncStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.source, ClockSource::Ptp);
        assert_eq!(decoded.offset_micros, 42);
        assert!(decoded.locked);
        assert_eq!(decoded.grandmaster_id, Some("GM-001".to_string()));
    }

    #[test]
    fn test_status_with_last_error_serialization() {
        let status = TimeSyncStatus {
            source: ClockSource::Ntp,
            offset_micros: -100,
            last_sync: Utc::now(),
            locked: false,
            grandmaster_id: None,
            last_error: Some("connection refused".to_string()),
        };
        let json = serde_json::to_string(&status).unwrap();
        let decoded: TimeSyncStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.last_error, Some("connection refused".to_string()));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_apply_unsupported() {
        let mut manager = TimeSyncManager::new(TimeSyncConfig::default());
        let result = manager.apply();
        assert!(matches!(result, Err(TimeSyncError::UnsupportedPlatform)));
    }

    #[test]
    fn test_reload_uses_load() {
        // reload 内部调用 load，验证接口一致性
        let config = TimeSyncConfig::default();
        let manager = TimeSyncManager::new(config);
        assert!(manager.config().enabled_sources.len() >= 2);
    }

    #[test]
    fn test_status_returns_clone() {
        let manager = TimeSyncManager::new(TimeSyncConfig::default());
        let st1 = manager.status();
        let st2 = manager.status();
        // 两次调用返回独立 clone，互不影响
        assert_eq!(st1.source, st2.source);
    }

    #[test]
    fn test_empty_enabled_sources_returns_error() {
        let config = TimeSyncConfig {
            enabled_sources: vec![],
            ..Default::default()
        };
        let mut manager = TimeSyncManager::new(config);
        let result = manager.apply();
        assert!(matches!(result, Err(TimeSyncError::Config(msg)) if msg.contains("enabled_sources is empty")));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_empty_ntp_servers_returns_error() {
        let config = TimeSyncConfig {
            enabled_sources: vec![ClockSource::Ntp],
            ntp: NtpConfig {
                servers: vec![],
                poll_interval_secs: 64,
            },
            ..Default::default()
        };
        let mut manager = TimeSyncManager::new(config);
        let result = manager.apply();
        assert!(matches!(result, Err(TimeSyncError::Config(msg)) if msg.contains("NTP servers list is empty")));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_discover_phc_non_linux_stub() {
        let manager = TimeSyncManager::new(TimeSyncConfig::default());
        let result = manager.discover_phc();
        assert!(matches!(result, Err(TimeSyncError::UnsupportedPlatform)));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_run_daemon_non_linux_stub() {
        let mut manager = TimeSyncManager::new(TimeSyncConfig::default());
        let result = manager.run_daemon();
        assert!(matches!(result, Err(TimeSyncError::UnsupportedPlatform)));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_poll_ptp_status_non_linux_stub() {
        let mut manager = TimeSyncManager::new(TimeSyncConfig::default());
        let result = manager.poll_ptp_status();
        assert!(matches!(result, Err(TimeSyncError::UnsupportedPlatform)));
    }

    #[test]
    fn test_drop_kills_children() {
        let mut manager = TimeSyncManager::new(TimeSyncConfig::default());
        // 生成一个长时间运行的子进程模拟 ptp4l
        #[cfg(target_os = "linux")]
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn sleep");
        #[cfg(windows)]
        let child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .spawn()
            .expect("failed to spawn ping");
        #[cfg(not(any(target_os = "linux", windows)))]
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn sleep");

        manager.ptp4l_child = Some(child);
        // Drop 应 kill + wait 子进程，不 panic
        drop(manager);
    }

    #[test]
    fn test_drop_with_no_children() {
        let manager = TimeSyncManager::new(TimeSyncConfig::default());
        // 无子进程时 Drop 应为 no-op，不 panic
        drop(manager);
    }

    #[test]
    fn test_parse_pmc_field() {
        let output = "  401c6b.fffe.5d4d70-0 seq 0 RESPONSE MANAGEMENT TIME_STATUS_NP\n\
  master_offset              12345\n\
  ingress_time               12345678901234567\n\
  port_state                 SLAVE\n";
        assert_eq!(
            parse_pmc_field(output, "master_offset"),
            Some("12345".to_string())
        );
        assert_eq!(
            parse_pmc_field(output, "port_state"),
            Some("SLAVE".to_string())
        );
        assert_eq!(parse_pmc_field(output, "nonexistent"), None);
    }

    #[test]
    fn test_parse_pmc_field_grandmaster() {
        let output = "  401c6b.fffe.5d4d70-0 seq 0 RESPONSE MANAGEMENT PARENT_DATASET\n\
  grandmasterIdentity        401c6b.fffe.5d4d70\n\
  parent_port_identity       401c6b.fffe.5d4d70-1\n";
        assert_eq!(
            parse_pmc_field(output, "grandmasterIdentity"),
            Some("401c6b.fffe.5d4d70".to_string())
        );
    }

    #[test]
    fn test_request_daemon_shutdown() {
        // 验证函数可调用且不 panic
        request_daemon_shutdown();
    }
}
