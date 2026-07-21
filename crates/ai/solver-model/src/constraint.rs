//! 约束类型枚举.

use crate::expr::LinearExpr;

/// 约束类型.
#[derive(Debug, Clone)]
pub enum Constraint {
    /// expr <= rhs.
    Le(LinearExpr, f64),
    /// expr >= rhs.
    Ge(LinearExpr, f64),
    /// expr == rhs.
    Eq(LinearExpr, f64),
    /// lo <= expr <= hi.
    Range(LinearExpr, f64, f64),
}
