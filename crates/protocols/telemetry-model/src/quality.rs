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
