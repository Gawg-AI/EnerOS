//! 变量目录（[09] §3.4）：列索引由 `(asset_id, kind, step)` 排序唯一
//! 确定；目录随模型 digest 输出；查询缺失即错——**禁止 default-fill**。

use std::collections::BTreeMap;
use std::vec::Vec;

use sha2::Digest;

/// 变量种类（闭枚举；码位冻结只追加）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VariableKind {
    /// 通用有功（kW；发电机/EVSE/HVAC/工艺单元等可调度资源）。
    ActivePower,
    /// 储能充电（kW，≥0）。
    Charge,
    /// 储能放电（kW，≥0）。
    Discharge,
    /// 荷电状态（kWh）。
    Soc,
    /// 充放方向 binary（1=充电方向）。
    Direction,
    /// 新能源弃电（kW）。
    Curtailment,
    /// 可调负荷削减（kW）。
    Shed,
    /// 购电（kW）。
    Buy,
    /// 售电（kW）。
    Sell,
    /// 购售方向 binary（1=购电）。
    GridDirection,
    /// 站峰功率（kW；单变量 step=0）。
    Peak,
}

impl VariableKind {
    /// 码 → 枚举（闭集；目录反查用）。
    pub const fn from_code(code: u16) -> Option<Self> {
        match code {
            1 => Some(Self::ActivePower),
            2 => Some(Self::Charge),
            3 => Some(Self::Discharge),
            4 => Some(Self::Soc),
            5 => Some(Self::Direction),
            6 => Some(Self::Curtailment),
            7 => Some(Self::Shed),
            8 => Some(Self::Buy),
            9 => Some(Self::Sell),
            10 => Some(Self::GridDirection),
            11 => Some(Self::Peak),
            _ => None,
        }
    }

    pub const fn code(self) -> u16 {
        match self {
            Self::ActivePower => 1,
            Self::Charge => 2,
            Self::Discharge => 3,
            Self::Soc => 4,
            Self::Direction => 5,
            Self::Curtailment => 6,
            Self::Shed => 7,
            Self::Buy => 8,
            Self::Sell => 9,
            Self::GridDirection => 10,
            Self::Peak => 11,
        }
    }

    /// 是否整数/binary 列（integrality 残差检查用）。
    pub const fn is_discrete(self) -> bool {
        matches!(self, Self::Direction | Self::GridDirection)
    }
}

/// 列（目录项）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub asset_id: u64,
    pub kind: VariableKind,
    /// 时段序号 0..T（Peak 单变量恒 0）。
    pub step: u32,
    /// 编译后的列索引（= 声明序）。
    pub index: usize,
}

/// 变量目录（排序确定；BTreeMap 反查）。
#[derive(Debug, Clone, PartialEq)]
pub struct VariableCatalog {
    columns: Vec<Column>,
    by_key: BTreeMap<(u64, u16, u32), usize>,
}

impl VariableCatalog {
    /// 由 (asset, kind, step) 集合构建（排序 + 索引分配）。
    pub fn new(mut keys: Vec<(u64, VariableKind, u32)>) -> Self {
        keys.sort_by_key(|k| (k.0, k.1, k.2));
        let mut columns = Vec::with_capacity(keys.len());
        let mut by_key = BTreeMap::new();
        for (i, (asset_id, kind, step)) in keys.into_iter().enumerate() {
            by_key.insert((asset_id, kind.code(), step), i);
            columns.push(Column {
                asset_id,
                kind,
                step,
                index: i,
            });
        }
        Self { columns, by_key }
    }

    /// 登记列数。
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// 精确查询（缺失即 Err——[09] §3.4：不允许 unwrap_or(0)）。
    pub fn index_of(
        &self,
        asset_id: u64,
        kind: VariableKind,
        step: u32,
    ) -> Result<usize, super::PlanError> {
        self.by_key
            .get(&(asset_id, kind.code(), step))
            .copied()
            .ok_or(super::PlanError::MalformedSolution("missing column"))
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// 目录内容摘要（与模型 digest 分开、可独立对账）。
    pub fn digest(&self) -> [u8; 32] {
        let mut h = sha2::Sha256::new();
        h.update(b"eneros.var-catalog.v1");
        h.update((self.columns.len() as u64).to_le_bytes());
        for c in &self.columns {
            h.update(c.asset_id.to_le_bytes());
            h.update(c.kind.code().to_le_bytes());
            h.update(c.step.to_le_bytes());
        }
        h.finalize().into()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    #[test]
    fn sorted_deterministic_index() {
        let a = VariableCatalog::new(vec![
            (2, VariableKind::Charge, 0),
            (1, VariableKind::Discharge, 1),
            (1, VariableKind::Charge, 1),
            (1, VariableKind::Charge, 0),
        ]);
        let b = VariableCatalog::new(vec![
            (1, VariableKind::Charge, 0),
            (1, VariableKind::Charge, 1),
            (1, VariableKind::Discharge, 1),
            (2, VariableKind::Charge, 0),
        ]);
        assert_eq!(a, b, "顺序无关");
        assert_eq!(a.index_of(1, VariableKind::Charge, 0).unwrap(), 0);
        assert_eq!(a.index_of(1, VariableKind::Charge, 1).unwrap(), 1);
        assert_eq!(a.index_of(1, VariableKind::Discharge, 1).unwrap(), 2);
        assert_eq!(a.index_of(2, VariableKind::Charge, 0).unwrap(), 3);
        assert_eq!(
            a.index_of(9, VariableKind::Charge, 0),
            Err(super::super::PlanError::MalformedSolution("missing column"))
        );
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn discrete_kinds() {
        assert!(VariableKind::Direction.is_discrete());
        assert!(VariableKind::GridDirection.is_discrete());
        assert!(!VariableKind::Soc.is_discrete());
    }
}
