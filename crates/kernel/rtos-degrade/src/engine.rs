//! 降级引擎 — 规则评估 + 模式切换 + 动作下发.
//!
//! [`DegradeEngine`] 是降级规则引擎的核心：按优先级遍历规则，计算目标模式，
//! 在模式切换时执行下发动作（StopCharge→0.0 / SafeDefault→安全值 / EmergencyStop→Bool(true)）。
//!
//! # D8：插入时排序
//!
//! `add_rule` 在插入时按 `priority` 降序排列，`evaluate` 无需排序，性能更优。
//!
//! # D11：EmergencyStop 锁定
//!
//! 一旦进入 `EmergencyStop`，`evaluate` 不会自动回切；调用方需通过
//! [`DegradeEngine::force_mode`] 显式恢复。

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_protocol_abstract::PointAccess;
use eneros_rtos_cmd_exec::device_map::DevicePointMap;
use eneros_upa_model::PointValue;

use crate::context::DegradeContext;
use crate::mode::DegradeMode;
use crate::rule::DegradeRule;
use crate::safe_defaults::SafeDefaults;
use crate::stats::{DegradeReport, DegradeStats};

/// 降级规则引擎（泛型 `<P: PointAccess>`，D6）.
pub struct DegradeEngine<P: PointAccess> {
    /// 已注册规则（按 priority 降序，D8）。
    rules: Vec<Box<dyn DegradeRule>>,
    /// 当前模式。
    current_mode: DegradeMode,
    /// 前一次模式。
    previous_mode: DegradeMode,
    /// 安全默认值表。
    safe_defaults: SafeDefaults,
    /// 设备→点映射（D3：复用 v0.56.0 DevicePointMap）。
    device_map: DevicePointMap,
    /// 协议访问层（D2：复用 v0.51.0 PointAccess 下发降级动作）。
    protocol: P,
    /// 累计统计（D7：普通 u64）。
    stats: DegradeStats,
}

impl<P: PointAccess> DegradeEngine<P> {
    /// 创建降级引擎.
    ///
    /// - `protocol`: 协议访问层（实现 PointAccess）
    /// - `device_map`: 设备→点映射
    /// - `safe_defaults`: 安全默认值表
    pub fn new(protocol: P, device_map: DevicePointMap, safe_defaults: SafeDefaults) -> Self {
        Self {
            rules: Vec::new(),
            current_mode: DegradeMode::Normal,
            previous_mode: DegradeMode::Normal,
            safe_defaults,
            device_map,
            protocol,
            stats: DegradeStats::default(),
        }
    }

    /// 添加规则（D8：按 priority 降序插入）.
    ///
    /// 高优先级规则插入在前，`evaluate` 遍历时首个 `Some(mode)` 即为最高优先级触发。
    pub fn add_rule(&mut self, rule: Box<dyn DegradeRule>) {
        let priority = rule.priority();
        let pos = self
            .rules
            .iter()
            .position(|r| r.priority() < priority)
            .unwrap_or(self.rules.len());
        self.rules.insert(pos, rule);
    }

    /// 评估当前上下文，返回评估报告.
    ///
    /// 按优先级降序遍历规则（rules 已在 `add_rule` 时排序，D8），
    /// 首个返回 `Some(mode)` 的规则决定新模式；若全部返回 `None`，模式为 `Normal`。
    ///
    /// **D11**：若当前处于 `EmergencyStop`，不会自动回切（锁定），需调用方通过
    /// [`force_mode`](Self::force_mode) 显式恢复。
    pub fn evaluate(&mut self, ctx: &DegradeContext, now_ns: u64) -> DegradeReport {
        self.stats.evaluations_count += 1;

        // 按优先级降序遍历（rules 已在 add_rule 时排序）
        let computed = self
            .rules
            .iter()
            .find_map(|r| r.evaluate(ctx))
            .unwrap_or(DegradeMode::Normal);

        // D11: EmergencyStop 锁定 — 不自动回切
        let new_mode = if self.current_mode == DegradeMode::EmergencyStop
            && computed != DegradeMode::EmergencyStop
        {
            self.current_mode
        } else {
            computed
        };

        if new_mode != self.current_mode {
            let action_taken = self.on_mode_change(self.current_mode, new_mode);
            self.previous_mode = self.current_mode;
            self.current_mode = new_mode;
            self.stats.mode_switch_count += 1;
            self.stats.last_mode = new_mode;
            self.stats.last_mode_switch_ns = now_ns;
            DegradeReport {
                new_mode,
                mode_changed: true,
                action_taken,
            }
        } else {
            DegradeReport {
                new_mode,
                mode_changed: false,
                action_taken: false,
            }
        }
    }

    /// 强制切换模式（D11：EmergencyStop 进入/恢复由调用方控制）.
    ///
    /// 绕过规则评估直接设置模式，执行与 `evaluate` 相同的模式切换动作。
    /// 用于外部触发 EmergencyStop 或从 EmergencyStop 恢复到 Normal。
    pub fn force_mode(&mut self, mode: DegradeMode, now_ns: u64) -> DegradeReport {
        if mode != self.current_mode {
            let action_taken = self.on_mode_change(self.current_mode, mode);
            self.previous_mode = self.current_mode;
            self.current_mode = mode;
            self.stats.mode_switch_count += 1;
            self.stats.last_mode = mode;
            self.stats.last_mode_switch_ns = now_ns;
            DegradeReport {
                new_mode: mode,
                mode_changed: true,
                action_taken,
            }
        } else {
            DegradeReport {
                new_mode: mode,
                mode_changed: false,
                action_taken: false,
            }
        }
    }

    /// 模式切换动作执行.
    ///
    /// - `Normal` / `HoldOutput`：无动作（Agent 接管 / 保持当前值）
    /// - `StopCharge`：向 device_map 中所有点下发 `Float(0.0)`
    /// - `SafeDefault`：向 protocol 下发 safe_defaults 中所有安全值
    /// - `EmergencyStop`：向 device_map 中所有点下发 `Bool(true)`
    fn on_mode_change(&mut self, _from: DegradeMode, to: DegradeMode) -> bool {
        match to {
            DegradeMode::Normal | DegradeMode::HoldOutput => {
                // Normal: Agent 接管，无动作
                // HoldOutput: 保持当前设定值，无动作
                false
            }
            DegradeMode::StopCharge => {
                // 向 device_map 中所有设备下发 0.0
                for (_device_id, point_id) in self.device_map.iter() {
                    let _ = self.protocol.write_point(point_id, PointValue::Float(0.0));
                }
                true
            }
            DegradeMode::SafeDefault => {
                // 遍历 safe_defaults 下发
                for (point_id, value) in self.safe_defaults.iter() {
                    let _ = self
                        .protocol
                        .write_point(point_id, PointValue::Float(value));
                }
                true
            }
            DegradeMode::EmergencyStop => {
                // 向 device_map 中所有设备下发 Bool(true)
                for (_device_id, point_id) in self.device_map.iter() {
                    let _ = self.protocol.write_point(point_id, PointValue::Bool(true));
                }
                true
            }
        }
    }

    /// 当前模式.
    pub fn current_mode(&self) -> DegradeMode {
        self.current_mode
    }

    /// 前一次模式.
    pub fn previous_mode(&self) -> DegradeMode {
        self.previous_mode
    }

    /// 累计统计引用.
    pub fn stats(&self) -> &DegradeStats {
        &self.stats
    }

    /// 协议访问层引用.
    pub fn protocol(&self) -> &P {
        &self.protocol
    }

    /// 协议访问层可变引用（v0.58.0 端到端降级流程所需）.
    ///
    /// 供 `WatchdogDegradeFlow` 在 Recovering 状态写入过渡插值结果。
    pub fn protocol_mut(&mut self) -> &mut P {
        &mut self.protocol
    }

    /// 已注册规则数.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}
