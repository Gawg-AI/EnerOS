//! 数据品质标志（QualityFlag）.
//!
//! [`QualityFlag`] 为四遥数据提供统一品质语义，对应 IEC 104 QualityDescriptor
//! 与 Modbus 通信状态的归一化表示。

/// 数据品质标志（7 状态单选）。
///
/// 与 `eneros-upa-model::PointQuality`（7 位标志位组合）不同，本类型为互斥单选，
/// 适用于四遥数据点的"当前主导品质"语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityFlag {
    /// 数据有效。
    Good,
    /// 数据无效（设备故障/通信中断）。
    Invalid,
    /// 数据可疑（越限/异常）。
    Questionable,
    /// 替代值（人工置数）。
    Substituted,
    /// 闭锁。
    Blocked,
    /// 溢出。
    Overflow,
    /// 过时（长时间未更新）。
    Outdated,
}

impl QualityFlag {
    /// 返回品质是否有效（仅 `Good` 为 true）。
    pub fn is_valid(&self) -> bool {
        matches!(self, QualityFlag::Good)
    }

    /// 返回品质是否为错误态（`Invalid`/`Blocked`/`Overflow`/`Outdated` 为 true）。
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            QualityFlag::Invalid
                | QualityFlag::Blocked
                | QualityFlag::Overflow
                | QualityFlag::Outdated
        )
    }
}

impl QualityFlag {
    /// [`PointQuality`](eneros_upa_model::PointQuality)（7 位标志组合）→
    /// 主导品质单选（蓝图 56：ADR-0027 规范投影，全仓唯一实现）。
    ///
    /// 裁断规则：
    /// 1. `valid` 置位 → `Good`（有效优先——采集侧已判定可信）；
    /// 2. 否则按 invalid > questionable > substituted > blocked > overflow >
    ///    outdated 取主导标志；
    /// 3. 全空 → `Invalid`（不可信默认，fail-closed）。
    ///
    /// 历史背景：device-agent ingress 与 soe-engine trigger 曾各持一份语义
    /// 相反的手写映射（全空一判 Invalid 一判 Good），本函数收拢消除。
    pub fn from_point_quality(q: &eneros_upa_model::PointQuality) -> Self {
        if q.valid {
            QualityFlag::Good
        } else if q.invalid {
            QualityFlag::Invalid
        } else if q.questionable {
            QualityFlag::Questionable
        } else if q.substituted {
            QualityFlag::Substituted
        } else if q.blocked {
            QualityFlag::Blocked
        } else if q.overflow {
            QualityFlag::Overflow
        } else if q.outdated {
            QualityFlag::Outdated
        } else {
            // 全空 = 无任何采集侧品质背书 → 不可信（ADR-0027）。
            QualityFlag::Invalid
        }
    }
}

impl From<eneros_upa_model::PointQuality> for QualityFlag {
    fn from(q: eneros_upa_model::PointQuality) -> Self {
        Self::from_point_quality(&q)
    }
}

#[cfg(test)]
mod tests {
    use eneros_upa_model::PointQuality;

    use super::QualityFlag;

    #[test]
    fn seven_single_flags_project_to_dominant_variant() {
        let mut q = PointQuality::default();
        assert_eq!(
            QualityFlag::from_point_quality(&q),
            QualityFlag::Invalid,
            "全空 → Invalid（fail-closed，原 soe trigger 矛盾行为已修）"
        );
        q.invalid = true;
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Invalid);
        q = PointQuality::default();
        q.questionable = true;
        assert_eq!(
            QualityFlag::from_point_quality(&q),
            QualityFlag::Questionable
        );
        q = PointQuality::default();
        q.substituted = true;
        assert_eq!(
            QualityFlag::from_point_quality(&q),
            QualityFlag::Substituted
        );
        q = PointQuality::default();
        q.blocked = true;
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Blocked);
        q = PointQuality::default();
        q.overflow = true;
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Overflow);
        q = PointQuality::default();
        q.outdated = true;
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Outdated);
    }

    #[test]
    fn valid_flag_dominates_all_other_flags() {
        // {valid,invalid} 与 {valid,questionable} 为历史分歧输入——
        // 规范裁断：valid 先判 → Good。
        let mut q = PointQuality::good();
        q.invalid = true;
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Good);
        let mut q = PointQuality::good();
        q.questionable = true;
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Good);
    }

    #[test]
    fn precedence_order_without_valid() {
        // 同置两非 valid 标志 → 高优先级胜出（invalid > questionable > …）。
        let q = PointQuality {
            questionable: true,
            substituted: true,
            ..PointQuality::default()
        };
        assert_eq!(
            QualityFlag::from_point_quality(&q),
            QualityFlag::Questionable
        );
        let q = PointQuality {
            outdated: true,
            overflow: true,
            ..PointQuality::default()
        };
        assert_eq!(QualityFlag::from_point_quality(&q), QualityFlag::Overflow);
    }

    #[test]
    fn from_trait_delegates_to_projection() {
        let q = PointQuality::good();
        assert_eq!(QualityFlag::from(q), QualityFlag::Good);
        assert_eq!(
            QualityFlag::from(PointQuality::default()),
            QualityFlag::Invalid
        );
    }
}
