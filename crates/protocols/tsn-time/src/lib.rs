//! EnerOS v0.79.0 gPTP（IEEE 802.1AS）时间同步层 — 类型与算法（无真实网络 I/O）.
//!
//! Phase 2 多机联邦协议栈的精密时间同步基础。提供 [`clock::ClockIdentity`] /
//! [`clock::MacAddr`] / [`clock::PtpTime`]（时间类型）、[`port::Port`] /
//! [`port::PortRole`] / [`port::PortState`]（端口模型）、[`bmca::AnnounceMessage`] /
//! [`bmca::BmcaResult`] / [`bmca::compare_priority`]（最佳主时钟算法）、
//! [`gptp::SyncMessage`] / [`gptp::FollowUpMessage`] / [`gptp::GptpConfig`] /
//! [`gptp::GptpClock`]（gPTP 时钟状态机：BMCA 选举 + 偏移低通滤波 + 时钟修正）。
//! 为后续 v0.80.0 时间敏感网络调度、v0.92.0 VPP 聚合、v0.158.0 硬件时间戳加速
//! 提供统一的时间同步原语。
//!
//! # v0.80.0 扩展：TSN 802.1Qbv 时间感知整形（TAS）
//!
//! v0.80.0 在 gPTP 时间同步之上扩展 [`tas`] 模块，为 Agent 控制命令、GOOSE
//! 跳闸、SV 采样等关键流量预留确定性时隙。新增 [`tas::TrafficClass`]（8 变体
//! 流量分类）/ [`tas::Packet`]（数据包描述符）/ [`tas::GateState`] /
//! [`tas::GateControlList`] / [`tas::TasConfig`] / [`tas::TasScheduler`]（调度器：
//! 闭合性校验 + 流量分类 + 下一窗口计算 + NIC 下发）/ [`tas::NicApplier`] +
//! [`tas::MockNicApplier`]（NIC 下发抽象，无真实 netlink/taprio）。
//! [`stream`] 模块提供 [`stream::StreamId`] / [`stream::StreamFilter`] 最小骨架
//! （无真实 802.1Qci 过滤逻辑）。[`config_loader`] 模块提供
//! [`config_loader::build_tas_config`] 纯 Rust 构造器（无 TOML 解析，依赖
//! eneros-config v0.26.0 上层加载）。本版本交付纯算法骨架，真实网卡下发延后
//! 到 v0.81.0 端到端时延验证。
//!
//! # v0.81.0 扩展：TSN 网络驱动抽象 + 端到端时延探针
//!
//! v0.81.0 在 TAS 配置面之上扩展 [`driver_glue`] 与 [`latency_probe`] 两个模块，
//! 建立 TSN 数据面抽象与端到端时延测量能力。新增 [`driver_glue::TsnDriver`] trait
//! （数据面抽象：按 [`tas::TrafficClass`] 发送 / 接收下一帧）+ [`driver_glue::MockTsnDriver`]
//! （测试用 Mock：记录 `sent` 队列 + `recv_queue` LIFO 弹出）+ [`driver_glue::TsnError`]
//! （错误枚举：SendFailed / RecvFailed / NotInitialized）+ [`driver_glue::driver_send_closure`]
//! 适配器（将 `TsnDriver::send` 包装为 `impl FnMut() -> Result<(), ()>` 闭包，桥接到
//! [`latency_probe::LatencyProbe`] 的 `send` 参数，无真实 netlink/socket — D25）。
//!
//! [`latency_probe`] 模块提供 [`latency_probe::DelayStats`]（7 字段统计结果：
//! min/max/mean/p99/p999/jitter/samples）与 [`latency_probe::LatencyProbe`]（基于
//! closure 注入的多场景时延探针）。`LatencyProbe` 通过 `clock_fn: fn() -> u64` +
//! `sleep_fn: fn(Duration)` 字段注入时间源与睡眠函数（D24：蓝图 `eneros_time::Instant::now()`
//! / `eneros_time::delay()` 不存在），通过 `send: impl FnMut() -> Result<(), ()>` 闭包
//! 注入发送动作（D26：`TsnDriver::send` 要求 `&mut self`，闭包只能实现 `FnMut` 而非 `Fn`）。
//! 关键方法：`measure_round_trip` / `run_burst` / `run` / `compute_stats` /
//! `measure_e2e` / `measure_under_load`，覆盖单次往返、突发测量、持续测量、
//! 端到端便捷测量、负载下测量五类场景，输出 `DelayStats` 统计结果.
//!
//! 性能基准（TC3 p99 < 2ms / p999 < 5ms / 抖动 < 1ms）标注为"硬件集成阶段验收"，
//! 本版本仅交付算法与统计骨架（D10 延续），真实 TSN 网卡 I/O 集成延后到 v0.82.0+
//! Agent 使用阶段.
//!
//! # 核心类型
//!
//! - [`clock::ClockIdentity`] — EUI-64 时钟标识（D13：定长 8 字节，无 `uuid`）
//! - [`clock::MacAddr`] — MAC 地址（D14：定长 6 字节）
//! - [`clock::PtpTime`] — PTP 时间戳（秒 + 纳秒），支持 `to_ns` / `add_ns` / `diff_ns`
//! - [`port::PortRole`] / [`port::PortState`] / [`port::Port`] — 端口角色/状态/描述符
//! - [`bmca::AnnounceMessage`] — Announce 报文（8 字段，BMCA 候选者）
//! - [`bmca::BmcaResult`] — BMCA 选举结果（ElectedAsMaster / FollowMaster）
//! - [`bmca::compare_priority`] — 候选者优先级比较（值小者优）
//! - [`gptp::SyncMessage`] / [`gptp::FollowUpMessage`] — 时间同步报文
//! - [`gptp::GptpConfig`] — 时钟配置（priority1/priority2/ports）
//! - [`gptp::GptpClock`] — gPTP 时钟状态机（13 + 4 字段：BMCA + 滤波 + FollowUp 配对）
//!
//! # 偏差声明（D1~D14）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 新建 crate `eneros-tsn-time` 置于 `crates/protocols/tsn-time/`（项目规则 §2.3.1，gPTP 属协议层） |
//! | **D2** | 文档位于 `docs/protocols/gptp-time-sync-design.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/`） |
//! | **D3** | 配置位于 `configs/gptp.toml`（项目规则 §2.3，非蓝图 `config/`） |
//! | **D4** | 测试内嵌 `src/lib.rs` T1~T25（沿用 v0.75.0~v0.78.0 模式，非蓝图 `tests/gptp_*.rs`） |
//! | **D5** | `GptpConfig.ports: alloc::vec::Vec<Port>` 替代固定数组（no_std 合规，使用 `alloc`） |
//! | **D6** | 无 `warn!()` 宏（无 `log` crate 依赖），用 `last_jump_ns: Option<i64>` 字段记录大跳变 |
//! | **D7** | `GptpClock::new(identity, initial_time, config)` 显式注入 `initial_time`，无 `PtpTime::now()`（no_std 无系统时钟） |
//! | **D8** | `SyncMessage` / `FollowUpMessage` 为纯数据结构，无网络 I/O（no_std 无 socket，真实收发延后到硬件驱动） |
//! | **D9** | `handle_sync(sync, rx_ts, delay_ns)` 显式接受 `delay_ns` 参数（修正蓝图 bug：`delay_to` + `diff_ns` 同对相消） |
//! | **D10** | `GptpClock` 方法无 `Send + Sync` bound（no_std 单线程，沿用 v0.59.0~v0.78.0 先例） |
//! | **D11** | 复用 `crate::clock::MacAddr`（不在 `port` 模块重复定义 MAC 类型） |
//! | **D12** | 不实现真实 P2P 延迟测量与硬件时间戳 I/O，仅保留算法与类型；性能基准延后到 v0.158.0 硬件加速 |
//! | **D13** | `ClockIdentity(pub [u8; 8])` EUI-64 定长数组（无 `uuid` crate 依赖，Karpathy 简化） |
//! | **D14** | `MacAddr(pub [u8; 6])` 定长数组（与 `eneros-hal` MAC 类型解耦，gPTP 协议层自包含） |
//!
//! # v0.80.0 偏差声明（D15~D19）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D15** | 配置构造器 `build_tas_config` 为纯 Rust 函数，无 `toml` / `serde` 依赖（TOML 解析由 eneros-config v0.26.0 上层处理） |
//! | **D16** | 使用 `core::time::Duration`（no_std 可用，`Sum` trait 在 `core` 中定义） |
//! | **D17** | `Packet` 为最小数据集（含 `ethertype` / `dscp` / `pcp` 三字段，无真实抓包，D5 延续） |
//! | **D18** | `apply_to_nic` 通过 `NicApplier` trait + `MockNicApplier` 抽象，无真实 netlink/taprio 下发（D6 延续）；不修改 v0.79.0 `PtpTime`，使用 `PtpTime::new(base_time_s, 0)`（D7 延续） |
//! | **D19** | `TrafficClass` 使用 `Be = 0` 命名变体而非蓝图 `Be(0)` tuple 变体（Rust 惯例 + `#[repr(u8)]` 直接对应）；`TasPort` 简化为 `port_id + applied`（消除蓝图 `gate_states` + `GateControlList.entries` 冗余，D10 延续）；`StreamFilter` 为纯数据类型无 802.1Qci 过滤逻辑（D14 延续） |
//!
//! # v0.81.0 偏差声明（D20~D26）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D20** | 复用 `crates/protocols/tsn-time` crate（项目规则 §2.3.1，TSN 属协议层），不新建 `crates/tsn_time/` |
//! | **D21** | 文档位于 `docs/protocols/tsn-determinism-report.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/`） |
//! | **D22** | 配置位于 `configs/latency_probe.toml`（项目规则 §2.3，非蓝图 `config/`） |
//! | **D23** | 测试内嵌 `src/lib.rs` T56~T84（沿用 v0.79.0~v0.80.0 模式，非蓝图 `tests/e2e_latency.rs` / `tests/jitter.rs`） |
//! | **D24** | `LatencyProbe` 通过 `clock_fn: fn() -> u64` + `sleep_fn: fn(Duration)` 字段注入时间源与睡眠函数（蓝图 `eneros_time::Instant::now()` / `eneros_time::delay()` 不存在，无系统时钟依赖） |
//! | **D25** | `TsnDriver` trait + `MockTsnDriver` + `driver_send_closure` 适配器抽象数据面，无真实 netlink/socket（D8 延续）；真实网卡数据面集成延后到 v0.82.0+ Agent 使用阶段 |
//! | **D26** | 闭包参数从蓝图 `impl Fn() -> Result<(), ()>` 改为 `impl FnMut() -> Result<(), ()>`（`TsnDriver::send` 要求 `&mut self`，捕获 `&mut T` 的闭包只能实现 `FnMut`；`measure_round_trip` 改为 `send: &mut impl FnMut()` 以允许同一闭包在 `run_burst` 多轮循环中被反复调用） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `core::*` 与 `alloc::*`，无任何外部依赖；不调用 `panic!` / `todo!` /
//! `unimplemented!`，不含 `unsafe` 块，不引入 `log` / `uuid` / `serde`。
//!
//! # 示例
//!
//! ```
//! use eneros_tsn_time::{ClockIdentity, MacAddr};
//!
//! let id = ClockIdentity::new([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);
//! assert_eq!(format!("{}", id), "01:23:45:67:89:AB:CD:EF");
//!
//! let mac = MacAddr::new([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB]);
//! assert_eq!(format!("{}", mac), "01:23:45:67:89:AB");
//! ```

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]
// v0.81.0：`LatencyProbe::measure_round_trip` 返回 `Result<Duration, ()>` 与
// `send: impl FnMut() -> Result<(), ()>` 闭包参数为 spec 批准的设计选择（D26：
// 仅传递成败，错误细节丢弃，由调用者通过 `driver_send_failure` 等场景区分），
// 非缺乏错误类型. 静默 clippy 风格提示.
#![allow(clippy::result_unit_err)]

extern crate alloc;

pub mod bmca;
pub mod clock;
pub mod config_loader;
pub mod driver_glue;
pub mod gptp;
pub mod latency_probe;
pub mod port;
pub mod stream;
pub mod tas;

pub use bmca::{compare_priority, AnnounceMessage, BmcaResult};
pub use clock::{ClockIdentity, MacAddr, PtpTime};
pub use config_loader::build_tas_config;
pub use driver_glue::{driver_send_closure, MockTsnDriver, TsnDriver, TsnError};
pub use gptp::{FollowUpMessage, GptpClock, GptpConfig, SyncMessage};
pub use latency_probe::{DelayStats, LatencyProbe};
pub use port::{Port, PortRole, PortState};
pub use stream::{StreamFilter, StreamId};
pub use tas::{
    GateControlList, GateState, MockNicApplier, NicApplier, Packet, TasConfig, TasError, TasPort,
    TasScheduleEntry, TasScheduler, TrafficClass,
};

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T25（覆盖 D1~D14 偏差声明与 spec 验收场景）.
    //!
    //! - T1~T7：时间类型（ClockIdentity / MacAddr / PtpTime）
    //! - T8~T11：端口与配置（PortRole / PortState / Port / GptpConfig）
    //! - T12~T17：GptpClock 基础（new / current_time / compute_offset / adjust_clock / to_announce）
    //! - T18~T21：BMCA（AnnounceMessage / compare_priority 三级比较）
    //! - T22~T24：run_bmca 选举（ElectedAsMaster / FollowMaster）
    //! - T25：handle_sync 偏移低通滤波

    use super::*;
    use crate::bmca::{compare_priority, AnnounceMessage, BmcaResult};
    use crate::clock::{ClockIdentity, MacAddr, PtpTime};
    use crate::gptp::{GptpClock, GptpConfig, SyncMessage};
    use crate::port::{Port, PortRole, PortState};

    // ===== T1：ClockIdentity Display 输出冒号分隔大写 hex =====
    #[test]
    fn test_t1_clock_identity_display() {
        let id = ClockIdentity::new([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);
        assert_eq!(alloc::format!("{}", id), "01:23:45:67:89:AB:CD:EF");
    }

    // ===== T2：MacAddr Display 输出冒号分隔大写 hex =====
    #[test]
    fn test_t2_mac_addr_display() {
        let mac = MacAddr::new([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB]);
        assert_eq!(alloc::format!("{}", mac), "01:23:45:67:89:AB");
    }

    // ===== T3：PtpTime::to_ns() 秒+纳秒合并为 i128 =====
    #[test]
    fn test_t3_ptp_time_to_ns() {
        assert_eq!(PtpTime::new(2, 500_000_000).to_ns(), 2_500_000_000i128);
    }

    // ===== T4：add_ns 正向进位（nanos >= 1e9 → seconds+1）=====
    #[test]
    fn test_t4_ptp_time_add_ns_positive_carry() {
        let mut t = PtpTime::new(1, 500_000_000);
        t.add_ns(500_000_000);
        assert_eq!(t, PtpTime::new(2, 0));
    }

    // ===== T5：add_ns 负向借位（nanos 变负 → seconds-1）=====
    #[test]
    fn test_t5_ptp_time_add_ns_negative_borrow() {
        let mut t = PtpTime::new(1, 0);
        t.add_ns(-500_000_000);
        assert_eq!(t, PtpTime::new(0, 500_000_000));
    }

    // ===== T6：diff_ns 正差值 =====
    #[test]
    fn test_t6_ptp_time_diff_ns_positive() {
        assert_eq!(
            PtpTime::new(2, 0).diff_ns(&PtpTime::new(1, 0)),
            1_000_000_000i64
        );
    }

    // ===== T7：diff_ns 负差值 =====
    #[test]
    fn test_t7_ptp_time_diff_ns_negative() {
        assert_eq!(
            PtpTime::new(1, 0).diff_ns(&PtpTime::new(2, 0)),
            -1_000_000_000i64
        );
    }

    // ===== T8：PortRole 各变体 Display 输出非空 =====
    #[test]
    fn test_t8_port_role_display_non_empty() {
        let roles = [
            PortRole::Master,
            PortRole::Slave,
            PortRole::Passive,
            PortRole::Disabled,
        ];
        for role in &roles {
            let s = alloc::format!("{}", role);
            assert!(!s.is_empty(), "PortRole Display 不应为空: {:?}", role);
        }
    }

    // ===== T9：PortState 各变体 Display 输出非空 =====
    #[test]
    fn test_t9_port_state_display_non_empty() {
        let states = [
            PortState::Initializing,
            PortState::Listening,
            PortState::Master,
            PortState::Slave,
            PortState::Passive,
        ];
        for state in &states {
            let s = alloc::format!("{}", state);
            assert!(!s.is_empty(), "PortState Display 不应为空: {:?}", state);
        }
    }

    // ===== T10：Port::new 初始化 role=Disabled / state=Initializing =====
    #[test]
    fn test_t10_port_new_defaults() {
        let port = Port::new(1, MacAddr::new([0; 6]), true);
        assert_eq!(port.port_id, 1);
        assert_eq!(port.role, PortRole::Disabled);
        assert_eq!(port.state, PortState::Initializing);
        assert!(port.hw_timestamping);
    }

    // ===== T11：GptpConfig::default() priority1=128 / priority2=0 / ports 空 =====
    #[test]
    fn test_t11_gptp_config_default() {
        let cfg = GptpConfig::default();
        assert_eq!(cfg.priority1, 128);
        assert_eq!(cfg.priority2, 0);
        assert!(cfg.ports.is_empty());
    }

    // ===== T12：GptpClock::new 字段初始化校验 =====
    #[test]
    fn test_t12_gptp_clock_new_fields() {
        let identity = ClockIdentity::new([1; 8]);
        let clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        assert_eq!(clk.steps_removed, 0);
        assert_eq!(clk.offset, 0);
        assert_eq!(clk.grandmaster_identity, identity);
        assert_eq!(clk.frequency_offset, 0);
        assert_eq!(clk.last_jump_ns, None);
        assert_eq!(clk.current_time, PtpTime::new(100, 0));
    }

    // ===== T13：current_time() offset=0 时返回原始时间 =====
    #[test]
    fn test_t13_gptp_clock_current_time_zero_offset() {
        let identity = ClockIdentity::new([1; 8]);
        let clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        assert_eq!(clk.current_time(), PtpTime::new(100, 0));
    }

    // ===== T14：compute_offset() 初始为 0 =====
    #[test]
    fn test_t14_gptp_clock_compute_offset_zero() {
        let identity = ClockIdentity::new([1; 8]);
        let clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        assert_eq!(clk.compute_offset(), 0);
    }

    // ===== T15：adjust_clock 小偏移（<1ms）→ frequency_offset，不改 current_time =====
    #[test]
    fn test_t15_adjust_clock_small_offset_frequency() {
        let identity = ClockIdentity::new([1; 8]);
        let mut clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        clk.adjust_clock(500_000);
        assert_eq!(clk.frequency_offset, 5_000);
        assert_eq!(clk.current_time, PtpTime::new(100, 0));
        assert_eq!(clk.last_jump_ns, None);
    }

    // ===== T16：adjust_clock 大偏移（≥1ms）→ 跳变 current_time + 记录 last_jump_ns =====
    #[test]
    fn test_t16_adjust_clock_large_offset_jump() {
        let identity = ClockIdentity::new([1; 8]);
        let mut clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        clk.adjust_clock(5_000_000);
        assert_eq!(clk.current_time, PtpTime::new(100, 5_000_000));
        assert_eq!(clk.last_jump_ns, Some(5_000_000));
    }

    // ===== T17：to_announce() 字段映射正确 =====
    #[test]
    fn test_t17_to_announce_fields() {
        let identity = ClockIdentity::new([1; 8]);
        let clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        let ann = clk.to_announce();
        assert_eq!(ann.grandmaster_identity, clk.identity);
        assert_eq!(ann.priority1, clk.priority1);
        assert_eq!(ann.steps_removed, clk.steps_removed);
    }

    // ===== T18：AnnounceMessage 构造与字段访问 =====
    #[test]
    fn test_t18_announce_message_construction() {
        let ann = AnnounceMessage {
            grandmaster_identity: ClockIdentity::new([2; 8]),
            priority1: 100,
            clock_class: 248,
            accuracy: 0xFE,
            priority2: 0,
            steps_removed: 5,
            source_port_id: 1,
            source_mac: MacAddr::new([1; 6]),
        };
        assert_eq!(ann.priority1, 100);
        assert_eq!(ann.clock_class, 248);
        assert_eq!(ann.steps_removed, 5);
        assert_eq!(ann.source_port_id, 1);
    }

    // ===== T19：compare_priority — priority1 升序，a(100) < b(200) → Less =====
    #[test]
    fn test_t19_compare_priority_priority1() {
        let a = sample_announce(ClockIdentity::new([0; 8]), 100, 248, 0xFE, 0);
        let b = AnnounceMessage {
            priority1: 200,
            ..a.clone()
        };
        assert_eq!(compare_priority(&a, &b), core::cmp::Ordering::Less);
    }

    // ===== T20：compare_priority — clock_class 升序，a(100) < b(200) → Less =====
    #[test]
    fn test_t20_compare_priority_clock_class() {
        let a = sample_announce(ClockIdentity::new([0; 8]), 128, 100, 0xFE, 0);
        let b = AnnounceMessage {
            clock_class: 200,
            ..a.clone()
        };
        assert_eq!(compare_priority(&a, &b), core::cmp::Ordering::Less);
    }

    // ===== T21：compare_priority — identity 字典序，[0;8] < [1;8] → Less =====
    #[test]
    fn test_t21_compare_priority_identity() {
        let a = sample_announce(ClockIdentity::new([0; 8]), 128, 248, 0xFE, 0);
        let b = AnnounceMessage {
            grandmaster_identity: ClockIdentity::new([1; 8]),
            ..a.clone()
        };
        assert_eq!(compare_priority(&a, &b), core::cmp::Ordering::Less);
    }

    // ===== T22：run_bmca 无远端 → ElectedAsMaster =====
    #[test]
    fn test_t22_run_bmca_elected_as_master_no_announces() {
        let identity = ClockIdentity::new([1; 8]);
        let mut clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        let r = clk.run_bmca(&[]);
        assert!(matches!(r, BmcaResult::ElectedAsMaster));
    }

    // ===== T23：run_bmca 远端 priority1=100 优于自身 128 → FollowMaster + steps_removed=1 =====
    #[test]
    fn test_t23_run_bmca_follow_master_remote_wins() {
        let self_id = ClockIdentity::new([1; 8]);
        let remote_id = ClockIdentity::new([2; 8]);
        let mut clk = GptpClock::new(self_id, PtpTime::new(100, 0), &GptpConfig::default());
        let remote_announce = sample_announce(remote_id, 100, 248, 0xFE, 0);
        let r = clk.run_bmca(&[remote_announce]);
        assert!(matches!(r, BmcaResult::FollowMaster(id) if id == remote_id));
        assert_eq!(clk.steps_removed, 1);
    }

    // ===== T24：run_bmca 远端 priority1=255 劣于自身 128 → ElectedAsMaster =====
    #[test]
    fn test_t24_run_bmca_self_wins_lower_priority1() {
        let self_id = ClockIdentity::new([1; 8]);
        let remote_id = ClockIdentity::new([2; 8]);
        let mut clk = GptpClock::new(self_id, PtpTime::new(100, 0), &GptpConfig::default());
        let remote_announce = sample_announce(remote_id, 255, 248, 0xFE, 0);
        let r = clk.run_bmca(&[remote_announce]);
        assert!(matches!(r, BmcaResult::ElectedAsMaster));
    }

    // ===== T25：handle_sync 偏移低通滤波 (0*7 + 900_000_000)/8 = 112_500_000 =====
    // clippy::erasing_op / identity_op：此处显式保留滤波公式 (offset*7 + new_offset)/8 的字面形态
    #[test]
    #[allow(clippy::erasing_op, clippy::identity_op)]
    fn test_t25_handle_sync_offset_filter() {
        let identity = ClockIdentity::new([1; 8]);
        let mut clk = GptpClock::new(identity, PtpTime::new(100, 0), &GptpConfig::default());
        let sync = SyncMessage {
            origin_timestamp: PtpTime::new(0, 0),
            sequence_id: 1,
            steps_removed: 0,
        };
        clk.handle_sync(&sync, PtpTime::new(1, 0), 100_000_000);
        // 低通滤波：offset 初始 0 → (0*7 + 900_000_000)/8 = 112_500_000
        assert_eq!(clk.offset, (0 * 7 + 900_000_000) / 8);
        assert_eq!(clk.offset, 112_500_000);
    }

    // ===== v0.80.0 TAS 测试（T26~T55）=====
    // 覆盖 D15~D19 偏差声明与 spec 验收场景：
    // - T26~T27：TrafficClass code / from_code
    // - T28~T29：Packet ethertype 识别与构造
    // - T30~T32：GateState / GateControlList
    // - T33~T34：TasConfig::default / TasPort::new
    // - T35：TasScheduler::new 字段验证
    // - T36~T38：validate_schedule（OK / ScheduleGap / TooShort）
    // - T39~T46：classify_packet（PTP/GOOSE/SV/DSCP 分段）
    // - T47~T49：next_gate_window（首窗口/第二窗口/永未开放）
    // - T50~T51：apply_to_nic（Mock 成功 / 调度非法时不下发）
    // - T52：StreamId Display
    // - T53~T55：build_tas_config / StreamFilter / MockNicApplier fail
    use core::time::Duration;

    use crate::config_loader::build_tas_config;
    use crate::stream::{StreamFilter, StreamId};
    use crate::tas::{
        GateControlList, GateState, MockNicApplier, Packet, TasConfig, TasError, TasPort,
        TasScheduleEntry, TasScheduler, TrafficClass,
    };

    // ===== T26：TrafficClass::code() — 8 变体返回 0~7 =====
    #[test]
    fn test_t26_traffic_class_code() {
        assert_eq!(TrafficClass::Be.code(), 0);
        assert_eq!(TrafficClass::BK.code(), 1);
        assert_eq!(TrafficClass::EE.code(), 2);
        assert_eq!(TrafficClass::CA.code(), 3);
        assert_eq!(TrafficClass::VO.code(), 4);
        assert_eq!(TrafficClass::VI.code(), 5);
        assert_eq!(TrafficClass::NC.code(), 6);
        assert_eq!(TrafficClass::ST.code(), 7);
    }

    // ===== T27：TrafficClass::from_code() — 0~7 → Some, 8+ → None =====
    #[test]
    fn test_t27_traffic_class_from_code() {
        assert_eq!(TrafficClass::from_code(0), Some(TrafficClass::Be));
        assert_eq!(TrafficClass::from_code(3), Some(TrafficClass::CA));
        assert_eq!(TrafficClass::from_code(7), Some(TrafficClass::ST));
        assert_eq!(TrafficClass::from_code(8), None);
        assert_eq!(TrafficClass::from_code(255), None);
    }

    // ===== T28：Packet::is_ptp() / is_goose() / is_sv() ethertype 识别 =====
    #[test]
    fn test_t28_packet_ethertype_detection() {
        let ptp = Packet {
            ethertype: 0x88F7,
            dscp: 0,
            pcp: 0,
        };
        assert!(ptp.is_ptp());
        assert!(!ptp.is_goose());
        assert!(!ptp.is_sv());
        let goose = Packet {
            ethertype: 0x88B8,
            dscp: 0,
            pcp: 0,
        };
        assert!(!goose.is_ptp());
        assert!(goose.is_goose());
        let sv = Packet {
            ethertype: 0x88BA,
            dscp: 0,
            pcp: 0,
        };
        assert!(sv.is_sv());
        let other = Packet {
            ethertype: 0x0800,
            dscp: 0,
            pcp: 0,
        };
        assert!(!other.is_ptp() && !other.is_goose() && !other.is_sv());
    }

    // ===== T29：Packet 构造与字段访问 =====
    #[test]
    fn test_t29_packet_construction() {
        let pkt = Packet {
            ethertype: 0x88F7,
            dscp: 46,
            pcp: 7,
        };
        assert_eq!(pkt.ethertype, 0x88F7);
        assert_eq!(pkt.dscp, 46);
        assert_eq!(pkt.pcp, 7);
    }

    // ===== T30：GateState 构造与字段访问 =====
    #[test]
    fn test_t30_gate_state_construction() {
        let gs = GateState {
            duration: Duration::from_micros(50),
            gates: 0b01000000,
        };
        assert_eq!(gs.duration, Duration::from_micros(50));
        assert_eq!(gs.gates, 0b01000000);
    }

    // ===== T31：GateControlList::new() cycle_count=0 初始 =====
    #[test]
    fn test_t31_gate_control_list_new() {
        let entries = vec![GateState {
            duration: Duration::from_micros(50),
            gates: 0x40,
        }];
        let gcl = GateControlList::new(entries);
        assert_eq!(gcl.cycle_count, 0);
        assert_eq!(gcl.entries.len(), 1);
    }

    // ===== T32：GateControlList::increment_cycle() 自增 =====
    #[test]
    fn test_t32_gate_control_list_increment_cycle() {
        let mut gcl = GateControlList::new(vec![]);
        gcl.increment_cycle();
        gcl.increment_cycle();
        gcl.increment_cycle();
        assert_eq!(gcl.cycle_count, 3);
    }

    // ===== T33：TasConfig::default() 字段验证 =====
    #[test]
    fn test_t33_tas_config_default() {
        let cfg = TasConfig::default();
        assert_eq!(cfg.cycle_us, 1_000_000);
        assert_eq!(cfg.base_time_s, 0);
        assert_eq!(cfg.port_count, 1);
        assert!(cfg.schedule.is_empty());
    }

    // ===== T34：TasPort::new() applied=false =====
    #[test]
    fn test_t34_tas_port_new() {
        let port = TasPort::new(2);
        assert_eq!(port.port_id, 2);
        assert!(!port.applied);
    }

    // ===== T35：TasScheduler::new() 字段验证 =====
    #[test]
    fn test_t35_tas_scheduler_new_fields() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 100,
            schedule: vec![TasScheduleEntry {
                duration_us: 1_000,
                gate_mask: 0xFF,
            }],
            port_count: 2,
        };
        let sched = TasScheduler::new(&cfg);
        assert_eq!(sched.cycle_time, Duration::from_micros(1_000));
        assert_eq!(sched.base_time, PtpTime::new(100, 0)); // D7
        assert_eq!(sched.ports.len(), 2);
        assert_eq!(sched.config.entries.len(), 1);
        assert_eq!(sched.config.entries[0].gates, 0xFF);
        assert_eq!(sched.config.cycle_count, 0);
    }

    // ===== T36：validate_schedule() OK（闭合 + 时长 >= 5µs）=====
    #[test]
    fn test_t36_validate_schedule_ok() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![
                TasScheduleEntry {
                    duration_us: 50,
                    gate_mask: 0x40,
                },
                TasScheduleEntry {
                    duration_us: 950,
                    gate_mask: 0x01,
                },
            ],
            port_count: 1,
        };
        let sched = TasScheduler::new(&cfg);
        assert!(sched.validate_schedule().is_ok());
    }

    // ===== T37：validate_schedule() ScheduleGap 错误（sum != cycle_time）=====
    #[test]
    fn test_t37_validate_schedule_gap() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![
                TasScheduleEntry {
                    duration_us: 300,
                    gate_mask: 0x40,
                },
                TasScheduleEntry {
                    duration_us: 500,
                    gate_mask: 0x01,
                }, // 总 800µs != 1000µs
            ],
            port_count: 1,
        };
        let sched = TasScheduler::new(&cfg);
        let err = sched.validate_schedule().unwrap_err();
        assert!(matches!(err, TasError::ScheduleGap { expected, actual }
            if expected == Duration::from_micros(1_000) && actual == Duration::from_micros(800)));
    }

    // ===== T38：validate_schedule() TooShort 错误（duration < 5µs）=====
    #[test]
    fn test_t38_validate_schedule_too_short() {
        let cfg = TasConfig {
            cycle_us: 3, // 3µs 总周期
            base_time_s: 0,
            schedule: vec![TasScheduleEntry {
                duration_us: 3,
                gate_mask: 0xFF,
            }], // 3µs < 5µs
            port_count: 1,
        };
        let sched = TasScheduler::new(&cfg);
        let err = sched.validate_schedule().unwrap_err();
        assert!(matches!(err, TasError::TooShort(d) if d == Duration::from_micros(3)));
    }

    // ===== T39：classify_packet() PTP ethertype → NC =====
    #[test]
    fn test_t39_classify_ptp() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x88F7,
            dscp: 0,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::NC);
    }

    // ===== T40：classify_packet() GOOSE ethertype → VO =====
    #[test]
    fn test_t40_classify_goose() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x88B8,
            dscp: 0,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::VO);
    }

    // ===== T41：classify_packet() SV ethertype → VI =====
    #[test]
    fn test_t41_classify_sv() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x88BA,
            dscp: 0,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::VI);
    }

    // ===== T42：classify_packet() DSCP 0-7 → BE =====
    #[test]
    fn test_t42_classify_dscp_be() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x0800,
            dscp: 5,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::Be);
    }

    // ===== T43：classify_packet() DSCP 8-15 → BK =====
    #[test]
    fn test_t43_classify_dscp_bk() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x0800,
            dscp: 10,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::BK);
    }

    // ===== T44：classify_packet() DSCP 24-31 → EE =====
    #[test]
    fn test_t44_classify_dscp_ee() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x0800,
            dscp: 28,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::EE);
    }

    // ===== T45：classify_packet() DSCP 40-47 → CA =====
    #[test]
    fn test_t45_classify_dscp_ca() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x0800,
            dscp: 46,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::CA);
    }

    // ===== T46：classify_packet() DSCP 48 → BE（默认分支）=====
    #[test]
    fn test_t46_classify_dscp_default_be() {
        let sched = TasScheduler::new(&TasConfig::default());
        let pkt = Packet {
            ethertype: 0x0800,
            dscp: 48,
            pcp: 0,
        };
        assert_eq!(sched.classify_packet(&pkt), TrafficClass::Be);
    }

    // ===== T47：next_gate_window() TC6 首窗口（0µs）=====
    #[test]
    fn test_t47_next_gate_window_nc_first() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![
                TasScheduleEntry {
                    duration_us: 50,
                    gate_mask: 0b01000000,
                }, // TC6
                TasScheduleEntry {
                    duration_us: 950,
                    gate_mask: 0b00001000,
                }, // TC3
            ],
            port_count: 1,
        };
        let sched = TasScheduler::new(&cfg);
        assert_eq!(sched.next_gate_window(TrafficClass::NC), Duration::ZERO);
    }

    // ===== T48：next_gate_window() TC3 第二窗口（50µs）=====
    #[test]
    fn test_t48_next_gate_window_ca_second() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![
                TasScheduleEntry {
                    duration_us: 50,
                    gate_mask: 0b01000000,
                }, // TC6
                TasScheduleEntry {
                    duration_us: 950,
                    gate_mask: 0b00001000,
                }, // TC3
            ],
            port_count: 1,
        };
        let sched = TasScheduler::new(&cfg);
        assert_eq!(
            sched.next_gate_window(TrafficClass::CA),
            Duration::from_micros(50)
        );
    }

    // ===== T49：next_gate_window() TC 永未开放 → cycle_time =====
    #[test]
    fn test_t49_next_gate_window_never_open() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![
                TasScheduleEntry {
                    duration_us: 500,
                    gate_mask: 0b01000000,
                }, // 仅 TC6
                TasScheduleEntry {
                    duration_us: 500,
                    gate_mask: 0b01000000,
                }, // 仅 TC6
            ],
            port_count: 1,
        };
        let sched = TasScheduler::new(&cfg);
        // TC3 (bit 3 = 0b00001000) 永未开放
        assert_eq!(
            sched.next_gate_window(TrafficClass::CA),
            Duration::from_micros(1_000)
        );
    }

    // ===== T50：apply_to_nic() Mock 成功 =====
    #[test]
    fn test_t50_apply_to_nic_mock_success() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![TasScheduleEntry {
                duration_us: 1_000,
                gate_mask: 0xFF,
            }],
            port_count: 2,
        };
        let mut sched = TasScheduler::new(&cfg);
        let mut mock = MockNicApplier::new();
        let result = sched.apply_to_nic(&mut mock, "eth0");
        assert!(result.is_ok());
        assert_eq!(mock.applied.len(), 1);
        assert_eq!(mock.applied[0].0, "eth0");
        assert_eq!(mock.applied[0].1, 1); // 1 entry
                                          // 所有端口 applied=true
        assert!(sched.ports.iter().all(|p| p.applied));
    }

    // ===== T51：apply_to_nic() 调度非法时不下发 =====
    #[test]
    fn test_t51_apply_to_nic_invalid_schedule() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![
                TasScheduleEntry {
                    duration_us: 300,
                    gate_mask: 0xFF,
                }, // 总 800µs
                TasScheduleEntry {
                    duration_us: 500,
                    gate_mask: 0xFF,
                }, // != cycle 1000µs
            ],
            port_count: 1,
        };
        let mut sched = TasScheduler::new(&cfg);
        let mut mock = MockNicApplier::new();
        let result = sched.apply_to_nic(&mut mock, "eth0");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TasError::ScheduleGap { .. }));
        assert!(mock.applied.is_empty()); // 未下发
        assert!(!sched.ports.iter().any(|p| p.applied)); // 端口未标记
    }

    // ===== T52：StreamId 构造 + Display 输出 "42" =====
    #[test]
    fn test_t52_stream_id_display() {
        let sid = StreamId::new(42);
        assert_eq!(alloc::format!("{}", sid), "42");
    }

    // ===== T53：build_tas_config 构造器（spec 要求）=====
    #[test]
    fn test_t53_build_tas_config() {
        let entries = vec![TasScheduleEntry {
            duration_us: 500,
            gate_mask: 0xFF,
        }];
        let cfg = build_tas_config(1_000, 100, entries, 3);
        assert_eq!(cfg.cycle_us, 1_000);
        assert_eq!(cfg.base_time_s, 100);
        assert_eq!(cfg.schedule.len(), 1);
        assert_eq!(cfg.port_count, 3);
    }

    // ===== T54：StreamFilter 构造 =====
    #[test]
    fn test_t54_stream_filter_construction() {
        let sf = StreamFilter::new(StreamId::new(1), 2, 7);
        assert_eq!(sf.stream_id, StreamId::new(1));
        assert_eq!(sf.gate_id, 2);
        assert_eq!(sf.priority, 7);
    }

    // ===== T55：MockNicApplier fail=true 返回 NicApplyFailed =====
    #[test]
    fn test_t55_mock_nic_applier_fail() {
        let cfg = TasConfig {
            cycle_us: 1_000,
            base_time_s: 0,
            schedule: vec![TasScheduleEntry {
                duration_us: 1_000,
                gate_mask: 0xFF,
            }],
            port_count: 1,
        };
        let mut sched = TasScheduler::new(&cfg);
        let mut mock = MockNicApplier {
            fail: true,
            ..MockNicApplier::new()
        };
        let result = sched.apply_to_nic(&mut mock, "eth0");
        assert!(matches!(result, Err(TasError::NicApplyFailed)));
        assert!(mock.applied.is_empty());
    }

    // ===== v0.81.0 driver_glue + latency_probe 测试（T56~T84）=====
    // 覆盖 D20~D26 偏差声明与 spec 验收场景：
    // - T56~T58：DelayStats（default / 构造 / derives）
    // - T59~T60：LatencyProbe::new（字段初始化 / clock_fn 存储）
    // - T61~T63：measure_round_trip（成功 / 失败 / driver_send_closure 集成）
    // - T64~T66：run_burst（全成功 / 全失败 / 混合）
    // - T67~T68：run（持续测量 / 零时长终止）
    // - T69~T71：compute_stats（空 / 单样本 / 多样本统计）
    // - T72~T73：measure_e2e（委托 / 集成）
    // - T74~T76：measure_under_load（背景调用 / 混合失败 / 全成功）
    // - T77~T78：TsnError（变体 / derives）
    // - T79~T84：MockTsnDriver（new / send / recv / 失败路径）
    use core::sync::atomic::{AtomicU64, Ordering};

    use crate::driver_glue::{driver_send_closure, MockTsnDriver, TsnDriver, TsnError};
    use crate::latency_probe::{DelayStats, LatencyProbe};

    // 静态时钟计数器（测试前重置）
    static CLOCK_NS: AtomicU64 = AtomicU64::new(0);
    static SLEEP_COUNT: AtomicU64 = AtomicU64::new(0);

    fn reset_statics() {
        CLOCK_NS.store(0, Ordering::SeqCst);
        SLEEP_COUNT.store(0, Ordering::SeqCst);
    }

    fn test_clock() -> u64 {
        CLOCK_NS.load(Ordering::SeqCst)
    }

    fn test_sleep(_: Duration) {
        SLEEP_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    // ===== T56：DelayStats::default() 全零 + samples=0 =====
    #[test]
    fn test_t56_delay_stats_default() {
        let stats = DelayStats::default();
        assert_eq!(stats.min, Duration::ZERO);
        assert_eq!(stats.max, Duration::ZERO);
        assert_eq!(stats.mean, Duration::ZERO);
        assert_eq!(stats.p99, Duration::ZERO);
        assert_eq!(stats.p999, Duration::ZERO);
        assert_eq!(stats.jitter, Duration::ZERO);
        assert_eq!(stats.samples, 0);
    }

    // ===== T57：DelayStats 构造与字段访问 =====
    #[test]
    fn test_t57_delay_stats_construction() {
        let stats = DelayStats {
            min: Duration::from_micros(100),
            max: Duration::from_micros(500),
            mean: Duration::from_micros(250),
            p99: Duration::from_micros(450),
            p999: Duration::from_micros(490),
            jitter: Duration::from_micros(400),
            samples: 100,
        };
        assert_eq!(stats.min, Duration::from_micros(100));
        assert_eq!(stats.max, Duration::from_micros(500));
        assert_eq!(stats.mean, Duration::from_micros(250));
        assert_eq!(stats.p99, Duration::from_micros(450));
        assert_eq!(stats.p999, Duration::from_micros(490));
        assert_eq!(stats.jitter, Duration::from_micros(400));
        assert_eq!(stats.samples, 100);
    }

    // ===== T58：DelayStats derives（Debug/Clone/PartialEq/Eq）=====
    #[test]
    fn test_t58_delay_stats_derives() {
        let stats = DelayStats {
            min: Duration::from_micros(10),
            max: Duration::from_micros(20),
            mean: Duration::from_micros(15),
            p99: Duration::from_micros(19),
            p999: Duration::from_micros(20),
            jitter: Duration::from_micros(10),
            samples: 5,
        };
        let cloned = stats.clone();
        assert_eq!(stats, cloned);
        // Debug 输出包含字段名
        let debug_str = alloc::format!("{:?}", stats);
        assert!(debug_str.contains("DelayStats"));
        assert!(debug_str.contains("samples"));
    }

    // ===== T59：LatencyProbe::new() 字段初始化 =====
    #[test]
    fn test_t59_latency_probe_new_fields() {
        reset_statics();
        let probe = LatencyProbe::new(test_clock, test_sleep);
        assert_eq!(probe.sample_count, 0);
        assert!(probe.results.is_empty());
        assert_eq!((probe.clock_fn)(), 0);
    }

    // ===== T60：LatencyProbe clock_fn 存储并返回静态值 =====
    #[test]
    fn test_t60_latency_probe_clock_fn_stored() {
        reset_statics();
        CLOCK_NS.store(42, Ordering::SeqCst);
        let probe = LatencyProbe::new(test_clock, test_sleep);
        assert_eq!((probe.clock_fn)(), 42);
    }

    // ===== T61：measure_round_trip 成功 — 返回 Ok(Duration) 等于时钟差 =====
    #[test]
    fn test_t61_measure_round_trip_success() {
        // 使用推进时钟：每次 clock_fn() 调用返回旧值并自增 100ns
        static ADV_CLOCK: AtomicU64 = AtomicU64::new(0);
        ADV_CLOCK.store(0, Ordering::SeqCst);
        fn advancing_clock() -> u64 {
            ADV_CLOCK.fetch_add(100, Ordering::SeqCst)
        }
        let mut probe = LatencyProbe::new(advancing_clock, test_sleep);
        // measure_round_trip 流程：
        //   start_ns = clock_fn() → 0 (clock 推进到 100)
        //   send() → Ok(()) (不改 clock)
        //   end_ns = clock_fn() → 100 (clock 推进到 200)
        //   返回 Ok(Duration::from_nanos(100 - 0)) = Ok(100ns)
        let mut send = || Ok::<(), ()>(());
        let result = probe.measure_round_trip(&mut send);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Duration::from_nanos(100));
        // measure_round_trip 不修改 sample_count 或 results（由调用者决定）
        assert_eq!(probe.sample_count, 0);
        assert!(probe.results.is_empty());
    }

    // ===== T62：measure_round_trip send 失败 — 返回 Err(()) =====
    #[test]
    fn test_t62_measure_round_trip_send_failure() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let mut send = || Err::<(), ()>(());
        let result = probe.measure_round_trip(&mut send);
        assert!(result.is_err());
        assert_eq!(probe.sample_count, 0);
        assert!(probe.results.is_empty());
    }

    // ===== T63：measure_round_trip 与 driver_send_closure 集成 =====
    #[test]
    fn test_t63_measure_round_trip_with_driver_send_closure() {
        // 使用推进时钟：每次调用返回旧值并自增 50ns
        static ADV_CLOCK: AtomicU64 = AtomicU64::new(0);
        ADV_CLOCK.store(0, Ordering::SeqCst);
        fn advancing_clock() -> u64 {
            ADV_CLOCK.fetch_add(50, Ordering::SeqCst)
        }
        let mut driver = MockTsnDriver::new();
        let mut probe = LatencyProbe::new(advancing_clock, test_sleep);
        // send 闭包作用域：driver 借给 send，measure_round_trip 调用一次后归还
        {
            let mut send = driver_send_closure(&mut driver, TrafficClass::CA, &[0x05]);
            // start=clock_fn()=0 (adv → 50), send() ok, end=clock_fn()=50 (adv → 100)
            // 返回 Ok(Duration::from_nanos(50))
            let result = probe.measure_round_trip(&mut send);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), Duration::from_nanos(50));
        } // send 在此 drop，driver 借用归还
          // 验证 driver 记录了 send 调用
        assert_eq!(driver.sent.len(), 1);
        assert_eq!(driver.sent[0].0, TrafficClass::CA);
        assert_eq!(driver.sent[0].1, vec![0x05]);
    }

    // ===== T64：run_burst 10 次全成功 — samples=10, sample_count=10 =====
    #[test]
    fn test_t64_run_burst_success() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let stats = probe.run_burst(10, Duration::from_millis(1), || Ok::<(), ()>(()));
        assert_eq!(stats.samples, 10);
        assert_eq!(probe.sample_count, 10);
        assert_eq!(probe.results.len(), 10);
        // sleep_fn 调用 10 次
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 10);
    }

    // ===== T65：run_burst 全失败 — samples=0, 返回 DelayStats::default() =====
    #[test]
    fn test_t65_run_burst_all_fail() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let stats = probe.run_burst(10, Duration::from_millis(1), || Err::<(), ()>(()));
        assert_eq!(stats.samples, 0);
        assert_eq!(stats, DelayStats::default());
        assert_eq!(probe.sample_count, 0);
        // sleep_fn 仍调用 10 次（即使失败）
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 10);
    }

    // ===== T66：run_burst 混合 — 5 成功 5 失败 — samples=5 =====
    #[test]
    fn test_t66_run_burst_mixed() {
        reset_statics();
        static CALL_COUNT: AtomicU64 = AtomicU64::new(0);
        CALL_COUNT.store(0, Ordering::SeqCst);
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let mut send = || {
            let n = CALL_COUNT.fetch_add(1, Ordering::SeqCst);
            if n < 5 {
                Ok::<(), ()>(())
            } else {
                Err::<(), ()>(())
            }
        };
        let stats = probe.run_burst(10, Duration::from_millis(1), &mut send);
        assert_eq!(stats.samples, 5);
        assert_eq!(probe.sample_count, 5);
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 10);
    }

    // ===== T67：run 持续测量到 deadline — 收集约 5 样本 =====
    #[test]
    fn test_t67_run_collects_until_deadline() {
        reset_statics();
        // 使用推进时钟：每次读返回旧值并自增 100µs
        static ADV_CLOCK: AtomicU64 = AtomicU64::new(0);
        ADV_CLOCK.store(0, Ordering::SeqCst);
        fn advancing_clock() -> u64 {
            ADV_CLOCK.fetch_add(100_000, Ordering::SeqCst)
        }
        let mut probe = LatencyProbe::new(advancing_clock, test_sleep);
        // run(duration=500µs)：clock_fn 起始 0，deadline = 0 + 500_000ns
        // 每轮：clock_fn() 读 0 (< 500_000) → measure (clock_fn 100_000 → 200_000, diff 100_000)
        //       clock_fn() 读 200_000 (< 500_000) → measure (300_000 → 400_000, diff 100_000)
        //       clock_fn() 读 400_000 (< 500_000) → measure (500_000 → 600_000, diff 100_000)
        //       clock_fn() 读 600_000 (>= 500_000) → 退出
        // 共 3 次成功采样（注意：每次 measure_round_trip 调用 clock_fn 2 次）
        let stats = probe.run(Duration::from_micros(500), || Ok::<(), ()>(()));
        assert!(stats.samples >= 1, "应至少采集 1 个样本");
        assert_eq!(probe.sample_count, stats.samples as u32);
        // run 不调用 sleep_fn
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 0);
    }

    // ===== T68：run 零时长 — 单次迭代后退出（clock_fn 不推进）=====
    #[test]
    fn test_t68_run_zero_duration_no_loop() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        // clock_fn 始终返回 0；duration=0 → deadline=0
        // 进入 while 时 clock_fn()=0 < 0 不成立 → 不进入循环
        // 验证不会无限循环（测试能完成即通过）
        let stats = probe.run(Duration::ZERO, || Ok::<(), ()>(()));
        assert_eq!(stats.samples, 0);
        assert_eq!(probe.sample_count, 0);
    }

    // ===== T69：compute_stats 空结果 — 返回 Default =====
    #[test]
    fn test_t69_compute_stats_empty() {
        reset_statics();
        let probe = LatencyProbe::new(test_clock, test_sleep);
        let stats = probe.compute_stats();
        assert_eq!(stats, DelayStats::default());
        assert_eq!(stats.samples, 0);
    }

    // ===== T70：compute_stats 单样本 — 所有统计等于该样本, jitter=0 =====
    #[test]
    fn test_t70_compute_stats_single_sample() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        probe.results.push(Duration::from_micros(500));
        let stats = probe.compute_stats();
        assert_eq!(stats.samples, 1);
        assert_eq!(stats.min, Duration::from_micros(500));
        assert_eq!(stats.max, Duration::from_micros(500));
        assert_eq!(stats.mean, Duration::from_micros(500));
        assert_eq!(stats.p99, Duration::from_micros(500));
        assert_eq!(stats.p999, Duration::from_micros(500));
        assert_eq!(stats.jitter, Duration::ZERO); // max - min = 0
    }

    // ===== T71：compute_stats 多样本 — 验证 min/max/mean/p99/p999/jitter =====
    #[test]
    fn test_t71_compute_stats_multiple_samples() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        // 推入 100µs, 200µs, 300µs, 400µs, 500µs
        for us in [100, 200, 300, 400, 500] {
            probe.results.push(Duration::from_micros(us));
        }
        let stats = probe.compute_stats();
        assert_eq!(stats.samples, 5);
        assert_eq!(stats.min, Duration::from_micros(100));
        assert_eq!(stats.max, Duration::from_micros(500));
        // mean = (100+200+300+400+500)/5 = 300µs
        assert_eq!(stats.mean, Duration::from_micros(300));
        // jitter = max - min = 400µs
        assert_eq!(stats.jitter, Duration::from_micros(400));
        // p99_idx = (5 * 0.99) as usize = 4 → sorted[4] = 500µs
        assert_eq!(stats.p99, Duration::from_micros(500));
        // p999_idx = (5 * 0.999) as usize = 4 → sorted[4] = 500µs
        assert_eq!(stats.p999, Duration::from_micros(500));
    }

    // ===== T72：measure_e2e 委托 run_burst — 100 样本 =====
    #[test]
    fn test_t72_measure_e2e_delegates_to_run_burst() {
        reset_statics();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let stats = probe.measure_e2e(100, || Ok::<(), ()>(()));
        assert_eq!(stats.samples, 100);
        assert_eq!(probe.sample_count, 100);
        // measure_e2e = run_burst(samples, 1ms, send)，sleep_fn 调用 100 次
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 100);
    }

    // ===== T73：measure_e2e 与 driver_send_closure 集成 =====
    #[test]
    fn test_t73_measure_e2e_with_driver_send_closure() {
        reset_statics();
        let mut driver = MockTsnDriver::new();
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        // send 闭包作用域：driver 借给 send，measure_e2e 调用 5 次后归还
        let stats = {
            let mut send = driver_send_closure(&mut driver, TrafficClass::NC, &[0xAA, 0xBB]);
            probe.measure_e2e(5, &mut send)
        }; // send 在此 drop，driver 借用归还
        assert_eq!(stats.samples, 5);
        assert_eq!(driver.sent.len(), 5);
        assert_eq!(driver.sent[0].0, TrafficClass::NC);
        assert_eq!(driver.sent[0].1, vec![0xAA, 0xBB]);
    }

    // ===== T74：measure_under_load background_load 在每轮调用 =====
    #[test]
    fn test_t74_measure_under_load_background_called() {
        reset_statics();
        static BG_COUNT: AtomicU64 = AtomicU64::new(0);
        BG_COUNT.store(0, Ordering::SeqCst);
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let bg = || {
            BG_COUNT.fetch_add(1, Ordering::SeqCst);
        };
        let stats = probe.measure_under_load(10, Duration::from_millis(1), bg, || Ok::<(), ()>(()));
        assert_eq!(stats.samples, 10);
        // background_load 应被调用 10 次（每轮一次）
        assert_eq!(BG_COUNT.load(Ordering::SeqCst), 10);
        // sleep_fn 调用 10 次
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 10);
    }

    // ===== T75：measure_under_load 混合失败 — 5 成功 5 失败, background 调用 10 次 =====
    #[test]
    fn test_t75_measure_under_load_mixed_failure() {
        reset_statics();
        static BG_COUNT: AtomicU64 = AtomicU64::new(0);
        static CALL_COUNT: AtomicU64 = AtomicU64::new(0);
        BG_COUNT.store(0, Ordering::SeqCst);
        CALL_COUNT.store(0, Ordering::SeqCst);
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let bg = || {
            BG_COUNT.fetch_add(1, Ordering::SeqCst);
        };
        let mut send = || {
            let n = CALL_COUNT.fetch_add(1, Ordering::SeqCst);
            if n < 5 {
                Ok::<(), ()>(())
            } else {
                Err::<(), ()>(())
            }
        };
        let stats = probe.measure_under_load(10, Duration::from_millis(1), bg, &mut send);
        assert_eq!(stats.samples, 5);
        assert_eq!(probe.sample_count, 5);
        // background_load 仍调用 10 次（每轮一次，不论成败）
        assert_eq!(BG_COUNT.load(Ordering::SeqCst), 10);
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 10);
    }

    // ===== T76：measure_under_load 全成功 — samples=10 =====
    #[test]
    fn test_t76_measure_under_load_full_success() {
        reset_statics();
        static BG_COUNT: AtomicU64 = AtomicU64::new(0);
        BG_COUNT.store(0, Ordering::SeqCst);
        let mut probe = LatencyProbe::new(test_clock, test_sleep);
        let bg = || {
            BG_COUNT.fetch_add(1, Ordering::SeqCst);
        };
        let stats = probe.measure_under_load(10, Duration::from_millis(1), bg, || Ok::<(), ()>(()));
        assert_eq!(stats.samples, 10);
        assert_eq!(probe.sample_count, 10);
        assert_eq!(probe.results.len(), 10);
        assert_eq!(BG_COUNT.load(Ordering::SeqCst), 10);
        assert_eq!(SLEEP_COUNT.load(Ordering::SeqCst), 10);
    }

    // ===== T77：TsnError 三变体 + Debug 输出 =====
    #[test]
    fn test_t77_tsn_error_variants() {
        let send_err = TsnError::SendFailed;
        let recv_err = TsnError::RecvFailed;
        let init_err = TsnError::NotInitialized;
        // Debug 输出包含变体名
        assert!(alloc::format!("{:?}", send_err).contains("SendFailed"));
        assert!(alloc::format!("{:?}", recv_err).contains("RecvFailed"));
        assert!(alloc::format!("{:?}", init_err).contains("NotInitialized"));
        // PartialEq 比较
        assert_eq!(send_err, TsnError::SendFailed);
        assert_ne!(send_err, recv_err);
        assert_ne!(recv_err, init_err);
    }

    // ===== T78：TsnError derives（Clone/Copy/PartialEq/Eq）=====
    #[test]
    fn test_t78_tsn_error_derives() {
        let err = TsnError::SendFailed;
        // Copy：直接赋值（无需 clone）
        let copied = err;
        assert_eq!(err, copied);
        // Eq：能放入 HashSet 等场景（间接验证）
        assert_eq!(TsnError::RecvFailed, TsnError::RecvFailed);
    }

    // ===== T79：MockTsnDriver::new() — 空队列，无 fail 标志 =====
    #[test]
    fn test_t79_mock_tsn_driver_new() {
        let driver = MockTsnDriver::new();
        assert!(driver.sent.is_empty());
        assert!(driver.recv_queue.is_empty());
        assert!(!driver.fail_send);
        assert!(!driver.fail_recv);
        // Default::default() 与 new() 等价（MockTsnDriver 未派生 PartialEq，
        // 逐字段比较）
        let def = MockTsnDriver::default();
        assert!(def.sent.is_empty());
        assert!(def.recv_queue.is_empty());
        assert!(!def.fail_send);
        assert!(!def.fail_recv);
    }

    // ===== T80：MockTsnDriver::send() 记录到 sent 队列 =====
    #[test]
    fn test_t80_mock_tsn_driver_send_records() {
        let mut driver = MockTsnDriver::new();
        assert!(driver.send(TrafficClass::CA, &[0x01, 0x02]).is_ok());
        assert!(driver.send(TrafficClass::NC, &[0xFF]).is_ok());
        assert_eq!(driver.sent.len(), 2);
        assert_eq!(driver.sent[0].0, TrafficClass::CA);
        assert_eq!(driver.sent[0].1, vec![0x01, 0x02]);
        assert_eq!(driver.sent[1].0, TrafficClass::NC);
        assert_eq!(driver.sent[1].1, vec![0xFF]);
    }

    // ===== T81：MockTsnDriver::recv() LIFO 弹出（push_recv 推入，recv 弹出最后）=====
    #[test]
    fn test_t81_mock_tsn_driver_recv_pops_lifo() {
        let mut driver = MockTsnDriver::new();
        driver.push_recv(vec![0x01]);
        driver.push_recv(vec![0x02]);
        driver.push_recv(vec![0x03]);
        // LIFO：最后推入的先弹出
        let first = driver.recv().unwrap();
        assert_eq!(first, vec![0x03]);
        let second = driver.recv().unwrap();
        assert_eq!(second, vec![0x02]);
        let third = driver.recv().unwrap();
        assert_eq!(third, vec![0x01]);
        // 队列空后返回 Err(RecvFailed)
        let fourth = driver.recv();
        assert!(matches!(fourth, Err(TsnError::RecvFailed)));
    }

    // ===== T82：MockTsnDriver::send() fail_send=true → Err(SendFailed)，不记录 =====
    #[test]
    fn test_t82_mock_tsn_driver_send_fail() {
        let mut driver = MockTsnDriver {
            fail_send: true,
            ..MockTsnDriver::new()
        };
        let result = driver.send(TrafficClass::CA, &[0x01]);
        assert!(matches!(result, Err(TsnError::SendFailed)));
        // 失败时不记录到 sent 队列
        assert!(driver.sent.is_empty());
    }

    // ===== T83：MockTsnDriver::recv() 空队列 → Err(RecvFailed) =====
    #[test]
    fn test_t83_mock_tsn_driver_recv_empty_queue() {
        let mut driver = MockTsnDriver::new();
        let result = driver.recv();
        assert!(matches!(result, Err(TsnError::RecvFailed)));
    }

    // ===== T84：MockTsnDriver::recv() fail_recv=true → Err(RecvFailed)，即使有数据 =====
    #[test]
    fn test_t84_mock_tsn_driver_recv_fail_flag() {
        let mut driver = MockTsnDriver {
            fail_recv: true,
            ..MockTsnDriver::new()
        };
        driver.push_recv(vec![0x01, 0x02]); // 即使有数据
        let result = driver.recv();
        assert!(matches!(result, Err(TsnError::RecvFailed)));
        // fail_recv 短路，不弹出数据
        // 关闭 fail_recv 后仍能 recv 到原数据
        driver.fail_recv = false;
        let data = driver.recv().unwrap();
        assert_eq!(data, vec![0x01, 0x02]);
    }

    /// 构造测试用 AnnounceMessage（默认 source_port_id=1, source_mac=[1;6]）.
    fn sample_announce(
        grandmaster_identity: ClockIdentity,
        priority1: u8,
        clock_class: u8,
        accuracy: u8,
        priority2: u8,
    ) -> AnnounceMessage {
        AnnounceMessage {
            grandmaster_identity,
            priority1,
            clock_class,
            accuracy,
            priority2,
            steps_removed: 0,
            source_port_id: 1,
            source_mac: MacAddr::new([1; 6]),
        }
    }
}
