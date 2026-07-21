//! 线性表达式与运算符重载（D1/D2/D12）.

use alloc::collections::BTreeMap;
use core::ops::{Add, Mul, Sub};

use crate::variable::Variable;

/// 线性表达式：c1*x1 + c2*x2 + ... + constant.
#[derive(Debug, Clone, Default)]
pub struct LinearExpr {
    /// 系数-变量对（变量索引 → 系数，D2：BTreeMap 替代 HashMap）.
    pub terms: BTreeMap<usize, f64>,
    /// 常数项.
    pub constant: f64,
}

impl LinearExpr {
    /// 空表达式.
    pub fn new() -> Self {
        Self::default()
    }

    /// 从变量创建表达式（系数 1.0，需 var.index = Some）.
    pub fn from_var(var: &Variable) -> Self {
        let mut expr = Self::new();
        if let Some(idx) = var.index {
            expr.terms.insert(idx, 1.0);
        }
        expr
    }

    /// 添加项（系数累加；系数为 0 时自动移除）.
    pub fn add_term(&mut self, var_idx: usize, coeff: f64) -> &mut Self {
        let entry = self.terms.entry(var_idx).or_insert(0.0);
        *entry += coeff;
        if entry.abs() < 1e-12 {
            self.terms.remove(&var_idx);
        }
        self
    }

    /// 标量乘法.
    pub fn scale(&self, factor: f64) -> Self {
        Self {
            terms: self.terms.iter().map(|(&k, &v)| (k, v * factor)).collect(),
            constant: self.constant * factor,
        }
    }

    /// 加法.
    pub fn add(&self, other: &LinearExpr) -> Self {
        let mut result = self.clone();
        for (&idx, &coeff) in &other.terms {
            let entry = result.terms.entry(idx).or_insert(0.0);
            *entry += coeff;
        }
        result.constant += other.constant;
        result
    }

    /// 减法.
    pub fn sub(&self, other: &LinearExpr) -> Self {
        self.add(&other.scale(-1.0))
    }
}

// 运算符重载（D1：core::ops，非 std::ops）
impl Add<LinearExpr> for LinearExpr {
    type Output = LinearExpr;
    fn add(self, rhs: LinearExpr) -> Self::Output {
        LinearExpr::add(&self, &rhs)
    }
}

impl Sub<LinearExpr> for LinearExpr {
    type Output = LinearExpr;
    fn sub(self, rhs: LinearExpr) -> Self::Output {
        LinearExpr::sub(&self, &rhs)
    }
}

impl Mul<f64> for LinearExpr {
    type Output = LinearExpr;
    fn mul(self, rhs: f64) -> Self::Output {
        self.scale(rhs)
    }
}
