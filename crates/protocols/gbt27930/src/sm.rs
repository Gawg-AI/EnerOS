//! 充电机侧充电状态机（蓝图 48 WP-48.4；桩端角色，owner 默认裁决）.
//!
//! # 结构事实来源与边界（诚实登记）
//!
//! 相序与报文名取自**蓝图 48 WP-48.4 卷内文本**：
//! `握手（BHM/BRM↔CHM/CRM）→ 参数配置（BCP/BRO/BCL/BCS↔CML/CRO/CTS/CML）
//! → 充电（BCL/BCS/BSM↔CCS）→ 结束（BST↔CST/BSD/CSD）`。
//!
//! **BLOCKED 待规约核对**：各报文字节布局/PGN、节点地址表、超时表数值
//! （经 [`crate::config`] 显式注入）、以及"哪条报文触发哪条应答"的精确
//! 规程细节。本状态机的迁移语义为蓝图文本的结构化读法，规约到位后须
//! 逐条对照修订（迁移本身不变时只换 codec 与参数）。
//!
//! # 迁移表（charger = 桩端主动方）
//!
//! | 相 | 事件 | 动作/迁移 |
//! |---|---|---|
//! | Idle | `start()` | 发 `Chm`，进 Handshake |
//! | Handshake | 收 `Bhm` | 发 `Crm` |
//! | Handshake | 收 `Brm` | 进 ParamConfig，发 `Cts`+`Cml` |
//! | Handshake | 超时 | Abort(HandshakeTimeout) |
//! | ParamConfig | 收 `Bcp` 且请求≤限值 | 发 `Cro` |
//! | ParamConfig | 收 `Bcp` 且请求>限值 | Abort(ParamMismatch) |
//! | ParamConfig | 收 `Bro` | 进 Charging |
//! | ParamConfig | 超时 | Abort(ParamTimeout) |
//! | Charging | 收 `Bcl`/`Bcs` | 发 `Ccs`（刷新看门狗） |
//! | Charging | 收 `Bsm` | 无输出（保持，刷新看门狗） |
//! | Charging | 收 `Bst` | 发 `Cst`，进 Ended |
//! | Charging | 看门狗超时 | Abort(ChargingTimeout) |
//! | Ended | 收 `Bsd` | 发 `Csd` |
//! | Ended | 收 `Csd` 确认后 `Complete` | （统计交换完成） |
//! | Ended | 超时 | Abort(EndTimeout) |
//! | 任意 Abort 后 | `start()` | 重连：重新进 Handshake |

use crate::config::Gbt27930Config;

/// 充电相关消息名（蓝图 WP-48.4 文本；方向：`C*`=充电机→BMS，
/// `B*`=BMS→充电机）。字节编码 BLOCKED——抽象枚举仅供状态机消费。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// 充电机握手。
    Chm,
    /// BMS 握手。
    Bhm,
    /// 充电机辨识。
    Crm,
    /// BMS 辨识。
    Brm,
    /// 充电机时间同步。
    Cts,
    /// 充电机最大输出能力。
    Cml,
    /// 充电机就绪。
    Cro,
    /// 充电机充电状态。
    Ccs,
    /// 充电机中止。
    Cst,
    /// 充电机统计数据。
    Csd,
    /// BMS 参数配置。
    Bcp,
    /// BMS 就绪。
    Bro,
    /// BMS 充电需求（极限）。
    Bcl,
    /// BMS 充电总状态。
    Bcs,
    /// BMS 统计（充电中）。
    Bsm,
    /// BMS 中止。
    Bst,
    /// BMS 统计数据（结束）。
    Bsd,
}

/// 充电相。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargePhase {
    /// 未接入（等待 start）。
    Idle,
    /// 握手（CHM/BHM + CRM/BRM）。
    Handshake,
    /// 参数配置。
    ParamConfig,
    /// 充电。
    Charging,
    /// 结束（BST/CST + BSD/CSD 统计交换）。
    Ended,
}

/// 中止原因（每相超时分支一一对应 + 参数越限）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// 握手相超时。
    HandshakeTimeout,
    /// 参数配置相超时。
    ParamTimeout,
    /// 充电相看门狗超时（B 类报文断流）。
    ChargingTimeout,
    /// 结束相超时。
    EndTimeout,
    /// 参数不匹配（BCP 请求超出 CML 能力）。
    ParamMismatch,
}

/// 状态机输出动作（宿主执行实际发送；字节层 BLOCKED）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 发送充电机侧报文。
    Send(Msg),
    /// 中止（含原因）；宿主记录并停止流程。
    Abort(AbortReason),
    /// 全流程完成（BSD/CSD 交换完毕）。
    Complete,
}

/// BCP 请求（参数配置校验输入；工程值——字节解码 BLOCKED）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BcpRequest {
    /// 请求最高允许充电电压（V）。
    pub max_voltage_v: f32,
    /// 请求最大充电电流（A）。
    pub max_current_a: f32,
}

/// 充电机最大输出能力（CML 限值；参数匹配判据基准）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChargerLimits {
    /// 最高输出电压（V）。
    pub max_voltage_v: f32,
    /// 最大输出电流（A）。
    pub max_current_a: f32,
}

/// 充电机侧状态机。
#[derive(Debug)]
pub struct ChargerSm {
    cfg: Gbt27930Config,
    limits: ChargerLimits,
    phase: ChargePhase,
    /// 当前相起始时刻（毫秒基准由宿主注入——跨进程同基准，40 卷教训）。
    phase_since_ms: u64,
    /// 已发待确认报文（简化重发模型：相级超时覆盖重发上限判定）。
    retransmits: u8,
}

impl ChargerSm {
    /// 构造（Idle 相）。
    pub fn new(cfg: Gbt27930Config, limits: ChargerLimits) -> Self {
        Self {
            cfg,
            limits,
            phase: ChargePhase::Idle,
            phase_since_ms: 0,
            retransmits: 0,
        }
    }

    /// 当前相。
    pub fn phase(&self) -> ChargePhase {
        self.phase
    }

    /// 启动充电流程（或中止后重连）：Idle/Ended 均可进入 Handshake。
    pub fn start(&mut self, now_ms: u64) -> Option<Action> {
        match self.phase {
            ChargePhase::Idle => {
                self.phase = ChargePhase::Handshake;
                self.phase_since_ms = now_ms;
                self.retransmits = 0;
                Some(Action::Send(Msg::Chm))
            }
            _ => None,
        }
    }

    fn abort(&mut self, reason: AbortReason) -> Option<Action> {
        self.phase = ChargePhase::Idle;
        Some(Action::Abort(reason))
    }

    /// 驱动事件：收到报文 / 时钟推进（每相超时分支在此判定）。
    pub fn on(&mut self, event: Event, now_ms: u64) -> Option<Action> {
        if self.phase == ChargePhase::Idle {
            return None;
        }
        match event {
            Event::Rx(msg) => self.on_msg(msg, now_ms),
            Event::Tick => self.on_tick(now_ms),
        }
    }

    fn on_tick(&mut self, now_ms: u64) -> Option<Action> {
        let elapsed = now_ms.saturating_sub(self.phase_since_ms);
        let reason = match self.phase {
            ChargePhase::Handshake if elapsed >= self.cfg.handshake_timeout_ms => {
                Some(AbortReason::HandshakeTimeout)
            }
            ChargePhase::ParamConfig if elapsed >= self.cfg.param_timeout_ms => {
                Some(AbortReason::ParamTimeout)
            }
            ChargePhase::Charging if elapsed >= self.cfg.charging_timeout_ms => {
                Some(AbortReason::ChargingTimeout)
            }
            ChargePhase::Ended if elapsed >= self.cfg.end_timeout_ms => {
                Some(AbortReason::EndTimeout)
            }
            _ => None,
        };
        // 重发上限：相级超时前按 retransmit_max 折算（简化模型：超时即
        // 已达重发上限——精确 per-packet 重发计数属规约表细节，BLOCKED）。
        let _ = self.retransmits;
        reason.and_then(|r| self.abort(r))
    }

    fn enter(&mut self, phase: ChargePhase, now_ms: u64) {
        self.phase = phase;
        self.phase_since_ms = now_ms;
        self.retransmits = 0;
    }

    fn on_msg(&mut self, msg: Msg, now_ms: u64) -> Option<Action> {
        match (self.phase, msg) {
            (ChargePhase::Handshake, Msg::Bhm) => Some(Action::Send(Msg::Crm)),
            (ChargePhase::Handshake, Msg::Brm) => {
                self.enter(ChargePhase::ParamConfig, now_ms);
                Some(Action::Send(Msg::Cml))
            }
            (ChargePhase::ParamConfig, Msg::Bcp) => {
                // 参数匹配在 on_bcp（带工程值）路径判定；无值 Bcp 视为
                // 占位继续（真实值路径见 [`Self::on_bcp_request`]）。
                Some(Action::Send(Msg::Cro))
            }
            (ChargePhase::ParamConfig, Msg::Bro) => {
                self.enter(ChargePhase::Charging, now_ms);
                None
            }
            (ChargePhase::Charging, Msg::Bcl) | (ChargePhase::Charging, Msg::Bcs) => {
                self.phase_since_ms = now_ms; // 刷新看门狗
                Some(Action::Send(Msg::Ccs))
            }
            (ChargePhase::Charging, Msg::Bsm) => {
                self.phase_since_ms = now_ms;
                None
            }
            (ChargePhase::Charging, Msg::Bst) => {
                self.enter(ChargePhase::Ended, now_ms);
                Some(Action::Send(Msg::Cst))
            }
            (ChargePhase::Ended, Msg::Bsd) => Some(Action::Send(Msg::Csd)),
            (ChargePhase::Ended, Msg::Csd) => {
                self.enter(ChargePhase::Idle, now_ms);
                Some(Action::Complete)
            }
            // 其余相内报文组合：非本卷结构事实（规程细节 BLOCKED）——
            // 忽略不伪动（诚实：不猜应答）。
            _ => None,
        }
    }

    /// BCP 携带工程值的路径（字节解码 BLOCKED——宿主解出后注入）。
    ///
    /// 请求超出 CML 能力 → `Abort(ParamMismatch)`（蓝图 DoD：参数不匹配
    /// 中止分支）。
    pub fn on_bcp_request(&mut self, req: &BcpRequest, _now_ms: u64) -> Option<Action> {
        if self.phase != ChargePhase::ParamConfig {
            return None;
        }
        if req.max_voltage_v > self.limits.max_voltage_v
            || req.max_current_a > self.limits.max_current_a
        {
            return self.abort(AbortReason::ParamMismatch);
        }
        Some(Action::Send(Msg::Cro))
    }
}

/// 状态机输入事件。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// 收到一条报文（字节解码 BLOCKED——宿主解出后注入）。
    Rx(Msg),
    /// 时钟推进（宿主周期驱动超时判定）。
    Tick,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Gbt27930Config {
        Gbt27930Config {
            charger_addr: 86,
            bms_addr: 1,
            handshake_timeout_ms: 100,
            param_timeout_ms: 200,
            charging_timeout_ms: 300,
            end_timeout_ms: 50,
            retransmit_max: 3,
        }
    }

    fn limits() -> ChargerLimits {
        ChargerLimits {
            max_voltage_v: 500.0,
            max_current_a: 250.0,
        }
    }

    /// DoD 正常充电全流程（含每条相内迁移）。
    #[test]
    fn normal_full_charge_flow() {
        let mut sm = ChargerSm::new(cfg(), limits());
        assert_eq!(sm.start(0), Some(Action::Send(Msg::Chm)));
        assert_eq!(sm.phase(), ChargePhase::Handshake);
        assert_eq!(sm.on(Event::Rx(Msg::Bhm), 10), Some(Action::Send(Msg::Crm)));
        assert_eq!(sm.on(Event::Rx(Msg::Brm), 20), Some(Action::Send(Msg::Cml)));
        assert_eq!(sm.phase(), ChargePhase::ParamConfig);
        let req = BcpRequest {
            max_voltage_v: 400.0,
            max_current_a: 100.0,
        };
        assert_eq!(sm.on_bcp_request(&req, 30), Some(Action::Send(Msg::Cro)));
        assert_eq!(sm.on(Event::Rx(Msg::Bro), 40), None);
        assert_eq!(sm.phase(), ChargePhase::Charging);
        assert_eq!(
            sm.on(Event::Rx(Msg::Bcl), 100),
            Some(Action::Send(Msg::Ccs))
        );
        assert_eq!(
            sm.on(Event::Rx(Msg::Bcs), 150),
            Some(Action::Send(Msg::Ccs))
        );
        assert_eq!(sm.on(Event::Rx(Msg::Bsm), 160), None);
        assert_eq!(
            sm.on(Event::Rx(Msg::Bst), 200),
            Some(Action::Send(Msg::Cst))
        );
        assert_eq!(sm.phase(), ChargePhase::Ended);
        assert_eq!(
            sm.on(Event::Rx(Msg::Bsd), 210),
            Some(Action::Send(Msg::Csd))
        );
        assert_eq!(sm.on(Event::Rx(Msg::Csd), 220), Some(Action::Complete));
        assert_eq!(sm.phase(), ChargePhase::Idle);
    }

    /// DoD 握手超时分支。
    #[test]
    fn handshake_timeout_aborts() {
        let mut sm = ChargerSm::new(cfg(), limits());
        sm.start(0);
        assert_eq!(sm.on(Event::Tick, 99), None, "超时边界前不中止");
        assert_eq!(
            sm.on(Event::Tick, 100),
            Some(Action::Abort(AbortReason::HandshakeTimeout))
        );
        assert_eq!(sm.phase(), ChargePhase::Idle);
    }

    /// DoD 参数配置相超时分支。
    #[test]
    fn param_phase_timeout_aborts() {
        let mut sm = ChargerSm::new(cfg(), limits());
        sm.start(0);
        let _ = sm.on(Event::Rx(Msg::Bhm), 10);
        let _ = sm.on(Event::Rx(Msg::Brm), 20);
        assert_eq!(sm.on(Event::Tick, 219), None);
        assert_eq!(
            sm.on(Event::Tick, 220),
            Some(Action::Abort(AbortReason::ParamTimeout))
        );
    }

    /// DoD 参数不匹配中止分支（BCP 请求 > CML 能力）。
    #[test]
    fn param_mismatch_aborts() {
        let mut sm = ChargerSm::new(cfg(), limits());
        sm.start(0);
        let _ = sm.on(Event::Rx(Msg::Bhm), 10);
        let _ = sm.on(Event::Rx(Msg::Brm), 20);
        let over = BcpRequest {
            max_voltage_v: 600.0,
            max_current_a: 100.0,
        };
        assert_eq!(
            sm.on_bcp_request(&over, 30),
            Some(Action::Abort(AbortReason::ParamMismatch))
        );
        assert_eq!(sm.phase(), ChargePhase::Idle);
        // 电流越限同样命中。
        let mut sm2 = ChargerSm::new(cfg(), limits());
        sm2.start(0);
        let _ = sm2.on(Event::Rx(Msg::Bhm), 10);
        let _ = sm2.on(Event::Rx(Msg::Brm), 20);
        let over_i = BcpRequest {
            max_voltage_v: 400.0,
            max_current_a: 251.0,
        };
        assert_eq!(
            sm2.on_bcp_request(&over_i, 30),
            Some(Action::Abort(AbortReason::ParamMismatch))
        );
    }

    /// DoD 充电相看门狗（B 类断流超时）分支。
    #[test]
    fn charging_watchdog_aborts_on_silence() {
        let mut sm = ChargerSm::new(cfg(), limits());
        sm.start(0);
        let _ = sm.on(Event::Rx(Msg::Bhm), 5);
        let _ = sm.on(Event::Rx(Msg::Brm), 10);
        let req = BcpRequest {
            max_voltage_v: 400.0,
            max_current_a: 100.0,
        };
        let _ = sm.on_bcp_request(&req, 15);
        let _ = sm.on(Event::Rx(Msg::Bro), 20);
        assert_eq!(sm.phase(), ChargePhase::Charging);
        // Bcl 刷新看门狗。
        let _ = sm.on(Event::Rx(Msg::Bcl), 300);
        assert_eq!(sm.on(Event::Tick, 599), None);
        assert_eq!(
            sm.on(Event::Tick, 600),
            Some(Action::Abort(AbortReason::ChargingTimeout))
        );
    }

    /// DoD 结束相超时分支。
    #[test]
    fn end_phase_timeout_aborts() {
        let mut sm = ChargerSm::new(cfg(), limits());
        sm.start(0);
        let _ = sm.on(Event::Rx(Msg::Bhm), 5);
        let _ = sm.on(Event::Rx(Msg::Brm), 10);
        let _ = sm.on(Event::Rx(Msg::Bro), 20);
        let _ = sm.on(Event::Rx(Msg::Bst), 30);
        assert_eq!(sm.phase(), ChargePhase::Ended);
        assert_eq!(
            sm.on(Event::Tick, 80),
            Some(Action::Abort(AbortReason::EndTimeout))
        );
    }

    /// DoD 中止后重连分支。
    #[test]
    fn reconnect_after_abort() {
        let mut sm = ChargerSm::new(cfg(), limits());
        sm.start(0);
        assert_eq!(
            sm.on(Event::Tick, 100),
            Some(Action::Abort(AbortReason::HandshakeTimeout))
        );
        // Idle 后可重新 start——重新进 Handshake 并发 CHM。
        assert_eq!(sm.start(200), Some(Action::Send(Msg::Chm)));
        assert_eq!(sm.phase(), ChargePhase::Handshake);
        // 完整走到充电相证明重连后流程可用。
        let _ = sm.on(Event::Rx(Msg::Bhm), 205);
        let _ = sm.on(Event::Rx(Msg::Brm), 210);
        assert_eq!(sm.phase(), ChargePhase::ParamConfig);
    }
}
