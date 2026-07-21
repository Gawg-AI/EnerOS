//! v0.80.0 TAS（Time-Aware Shaper, IEEE 802.1Qbv）核心类型与调度算法.
//!
//! 在 v0.79.0 gPTP 时间同步之上建立时间感知整形调度层，为 Agent 控制命令、
//! GOOSE 跳闸、SV 采样等关键流量预留确定性时隙。本模块交付纯 Rust 类型与
//! 算法骨架（无真实 netlink/taprio 下发），通过 [`NicApplier`] trait +
//! [`MockNicApplier`] 注入的方式验证门控列表闭合性、流量分类、下一窗口计算。
//!
//! # 核心类型
//!
//! - [`TrafficClass`] — 8 变体流量分类（对应 802.1Q PCP 等级 0~7）
//! - [`Packet`] — 数据包描述符（最小数据集：ethertype / dscp / pcp，D5：无真实抓包）
//! - [`GateState`] / [`GateControlList`] — 门控状态与门控列表
//! - [`TasScheduleEntry`] / [`TasConfig`] — 调度表条目与 TAS 配置
//! - [`TasPort`] — TAS 端口（D10：简化为 `port_id + applied`）
//! - [`TasError`] — TAS 错误枚举
//! - [`NicApplier`] / [`MockNicApplier`] — NIC 下发抽象（D6：无真实 netlink/taprio）
//! - [`TasScheduler`] — TAS 调度器（new / validate_schedule / classify_packet /
//!   next_gate_window / apply_to_nic）

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;

use crate::clock::PtpTime;

/// 802.1Q 流量分类（PCP 等级 0~7）.
///
/// D18：使用 `Be = 0` 命名变体而非蓝图 `Be(0)` tuple 变体（Rust 惯例 +
/// `#[repr(u8)]` 直接对应）；外部使用 `TrafficClass::Be` 不带数字.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrafficClass {
    /// Best Effort（PCP 0）.
    Be = 0,
    /// Background（PCP 1）.
    BK = 1,
    /// Energy Efficiency — Agent 状态（PCP 2）.
    EE = 2,
    /// Critically Auth — Agent 命令（PCP 3）.
    CA = 3,
    /// Voice — GOOSE（PCP 4）.
    VO = 4,
    /// Video — SV（PCP 5）.
    VI = 5,
    /// Network Control — gPTP（PCP 6）.
    NC = 6,
    /// Strategic — 保留（PCP 7）.
    ST = 7,
}

impl TrafficClass {
    /// 返回变体对应的数值编码（0~7）.
    pub fn code(&self) -> u8 {
        *self as u8
    }

    /// 由数值编码反解为 `TrafficClass`；范围 0~7 返回 `Some`，其余返回 `None`.
    pub fn from_code(c: u8) -> Option<Self> {
        match c {
            0 => Some(TrafficClass::Be),
            1 => Some(TrafficClass::BK),
            2 => Some(TrafficClass::EE),
            3 => Some(TrafficClass::CA),
            4 => Some(TrafficClass::VO),
            5 => Some(TrafficClass::VI),
            6 => Some(TrafficClass::NC),
            7 => Some(TrafficClass::ST),
            _ => None,
        }
    }
}

/// 数据包描述符（D5：最小数据集，无真实抓包）.
///
/// 含 `ethertype` / `dscp` / `pcp` 三字段，用于 [`TasScheduler::classify_packet`]
/// 的流量分类决策。真实抓包与硬件卸载延后到 v0.81.0 网络驱动集成.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    /// EtherType（如 0x88F7 = PTP、0x88B8 = GOOSE、0x88BA = SV）.
    pub ethertype: u16,
    /// Differentiated Services Code Point（DSCP）.
    pub dscp: u8,
    /// Priority Code Point（PCP，802.1Q 优先级）.
    pub pcp: u8,
}

impl Packet {
    /// 是否为 PTP 报文（EtherType == 0x88F7）.
    pub fn is_ptp(&self) -> bool {
        self.ethertype == 0x88F7
    }

    /// 是否为 GOOSE 报文（EtherType == 0x88B8）.
    pub fn is_goose(&self) -> bool {
        self.ethertype == 0x88B8
    }

    /// 是否为 SV 报文（EtherType == 0x88BA）.
    pub fn is_sv(&self) -> bool {
        self.ethertype == 0x88BA
    }
}

/// 门控状态：单条 GCL 条目（`gates` 第 i 位 = 1 表示 TCi 开放）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateState {
    /// 该条目持续时间.
    pub duration: Duration,
    /// 8 位门控掩码（bit i = TCi 的门状态：1=开放，0=关闭）.
    pub gates: u8,
}

/// 门控列表（Gate Control List）：一个调度周期内的所有条目.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateControlList {
    /// GCL 条目序列.
    pub entries: Vec<GateState>,
    /// 周期计数（每次 `increment_cycle` 自增 1）.
    pub cycle_count: u32,
}

impl GateControlList {
    /// 以条目序列构造 GCL，`cycle_count` 初始为 0.
    pub fn new(entries: Vec<GateState>) -> Self {
        Self {
            entries,
            cycle_count: 0,
        }
    }

    /// 周期计数自增（饱和到 `u32::MAX`）.
    pub fn increment_cycle(&mut self) {
        self.cycle_count = self.cycle_count.saturating_add(1);
    }
}

/// TAS 调度表条目（用于 `TasConfig` 的 `schedule` 字段）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TasScheduleEntry {
    /// 条目持续时间（微秒）.
    pub duration_us: u64,
    /// 8 位门控掩码（bit i = TCi 的门状态）.
    pub gate_mask: u8,
}

/// TAS 配置：周期 + 基准时间 + 调度表 + 端口数.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TasConfig {
    /// 调度周期（微秒，默认 1_000_000 = 1ms）.
    pub cycle_us: u64,
    /// 周期基准时间（秒，PTP 时间戳的秒部分）.
    pub base_time_s: u64,
    /// 调度表条目序列.
    pub schedule: Vec<TasScheduleEntry>,
    /// 端口数量.
    pub port_count: u8,
}

impl Default for TasConfig {
    fn default() -> Self {
        Self {
            cycle_us: 1_000_000,
            base_time_s: 0,
            schedule: Vec::new(),
            port_count: 1,
        }
    }
}

/// TAS 端口（D10：消除冗余，仅 `port_id + applied`）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TasPort {
    /// 端口编号.
    pub port_id: u8,
    /// 是否已成功下发门控列表.
    pub applied: bool,
}

impl TasPort {
    /// 以端口编号构造，`applied` 初始为 `false`.
    pub fn new(port_id: u8) -> Self {
        Self {
            port_id,
            applied: false,
        }
    }
}

/// TAS 错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasError {
    /// 调度不闭合：条目总时长 != 周期时长.
    ScheduleGap {
        /// 期望时长（cycle_time）.
        expected: Duration,
        /// 实际时长（条目总和）.
        actual: Duration,
    },
    /// 单条目时长过短（< 5µs）.
    TooShort(Duration),
    /// NIC 下发失败.
    NicApplyFailed,
    /// 配置非法.
    InvalidConfig,
}

/// NIC 下发抽象（D6：无真实 netlink/taprio，通过 trait 注入）.
///
/// 真实网卡下发（Linux taprio / netlink）由实现此 trait 的具体类型提供；
/// 测试与算法验证使用 [`MockNicApplier`].
pub trait NicApplier {
    /// 将门控列表应用到指定网卡接口.
    fn apply(&mut self, iface: &str, config: &GateControlList) -> Result<(), TasError>;
}

/// Mock NIC 下发器（用于测试与算法验证）.
///
/// `fail = false` 时记录 `(iface, entry_count)` 到 `applied` 并返回 `Ok(())`；
/// `fail = true` 时返回 `Err(TasError::NicApplyFailed)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockNicApplier {
    /// 已下发的接口与条目数记录：`(iface, entry_count)`.
    pub applied: Vec<(String, u32)>,
    /// 是否模拟下发失败.
    pub fail: bool,
}

impl MockNicApplier {
    /// 构造空 Mock（`applied = Vec::new(), fail = false`）.
    pub fn new() -> Self {
        Self {
            applied: Vec::new(),
            fail: false,
        }
    }
}

impl Default for MockNicApplier {
    fn default() -> Self {
        Self::new()
    }
}

impl NicApplier for MockNicApplier {
    fn apply(&mut self, iface: &str, config: &GateControlList) -> Result<(), TasError> {
        if self.fail {
            return Err(TasError::NicApplyFailed);
        }
        self.applied
            .push((iface.to_string(), config.entries.len() as u32));
        Ok(())
    }
}

/// TAS 调度器：门控列表 + 基准时间 + 周期 + 端口集合.
///
/// 提供 [`TasScheduler::new`] / [`TasScheduler::validate_schedule`] /
/// [`TasScheduler::classify_packet`] / [`TasScheduler::next_gate_window`] /
/// [`TasScheduler::apply_to_nic`] 方法.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TasScheduler {
    /// 端口集合（每个端口独立跟踪 `applied` 状态）.
    pub ports: Vec<TasPort>,
    /// 周期基准时间（D7：使用 `PtpTime::new(config.base_time_s, 0)`，不修改 v0.79.0 clock.rs）.
    pub base_time: PtpTime,
    /// 调度周期时长.
    pub cycle_time: Duration,
    /// 门控列表.
    pub config: GateControlList,
}

impl TasScheduler {
    /// 由 [`TasConfig`] 构造调度器.
    ///
    /// D7：使用 `PtpTime::new(config.base_time_s, 0)` 构造基准时间，
    /// 不修改 v0.79.0 的 `clock.rs`（无 `from_unix` 方法）.
    pub fn new(config: &TasConfig) -> Self {
        let cycle_time = Duration::from_micros(config.cycle_us);
        let entries: Vec<GateState> = config
            .schedule
            .iter()
            .map(|e| GateState {
                duration: Duration::from_micros(e.duration_us),
                gates: e.gate_mask,
            })
            .collect();
        let ports = (0..config.port_count).map(TasPort::new).collect();
        Self {
            ports,
            base_time: PtpTime::new(config.base_time_s, 0),
            cycle_time,
            config: GateControlList::new(entries),
        }
    }

    /// 校验调度闭合性：所有条目时长之和 == `cycle_time`，且每条 >= 5µs.
    ///
    /// - 不闭合 → `Err(TasError::ScheduleGap { expected, actual })`
    /// - 任一条目 < 5µs → `Err(TasError::TooShort(d))`
    /// - 通过 → `Ok(())`
    pub fn validate_schedule(&self) -> Result<(), TasError> {
        let total: Duration = self.config.entries.iter().map(|e| e.duration).sum();
        if total != self.cycle_time {
            return Err(TasError::ScheduleGap {
                expected: self.cycle_time,
                actual: total,
            });
        }
        for e in &self.config.entries {
            if e.duration.as_micros() < 5 {
                return Err(TasError::TooShort(e.duration));
            }
        }
        Ok(())
    }

    /// 数据包流量分类（D12：PTP/GOOSE/SV ethertype 优先，否则 DSCP 分段）.
    ///
    /// - PTP（0x88F7）→ `TrafficClass::NC`
    /// - GOOSE（0x88B8）→ `TrafficClass::VO`
    /// - SV（0x88BA）→ `TrafficClass::VI`
    /// - 否则按 DSCP：0-7 → Be、8-15 → BK、24-31 → EE、40-47 → CA、其他 → Be
    pub fn classify_packet(&self, pkt: &Packet) -> TrafficClass {
        if pkt.is_ptp() {
            return TrafficClass::NC;
        }
        if pkt.is_goose() {
            return TrafficClass::VO;
        }
        if pkt.is_sv() {
            return TrafficClass::VI;
        }
        match pkt.dscp {
            0..=7 => TrafficClass::Be,
            8..=15 => TrafficClass::BK,
            24..=31 => TrafficClass::EE,
            40..=47 => TrafficClass::CA,
            _ => TrafficClass::Be,
        }
    }

    /// 计算指定流量分类的下一窗口起始偏移.
    ///
    /// 遍历 GCL 找首个 `gates >> tc.code() & 1 == 1` 的条目，返回从周期起点
    /// 到该条目起点的累计 duration；若该 TC 在本周期永未开放，返回 `cycle_time`
    /// （表示需要等待整个周期）.
    pub fn next_gate_window(&self, tc: TrafficClass) -> Duration {
        let mut acc = Duration::ZERO;
        for e in &self.config.entries {
            if (e.gates >> tc.code()) & 1 == 1 {
                return acc;
            }
            acc += e.duration;
        }
        self.cycle_time
    }

    /// 将门控列表下发到 NIC（先 validate 再 apply）.
    ///
    /// 1. 调用 [`validate_schedule`](Self::validate_schedule)，调度非法则直接返回错误.
    /// 2. 调用 `applier.apply(iface, &self.config)` 下发到指定接口.
    /// 3. 下发成功后将所有端口的 `applied` 标记为 `true`.
    pub fn apply_to_nic(
        &mut self,
        applier: &mut dyn NicApplier,
        iface: &str,
    ) -> Result<(), TasError> {
        self.validate_schedule()?;
        applier.apply(iface, &self.config)?;
        for port in &mut self.ports {
            port.applied = true;
        }
        Ok(())
    }
}
