//! LP 问题定义（D11）.

use alloc::string::String;
use alloc::vec::Vec;

/// 变量类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    /// 连续变量.
    Continuous,
    /// 整数变量.
    Integer,
    /// 0-1 变量.
    Binary,
}

/// 目标方向.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveSense {
    /// 最小化.
    Minimize,
    /// 最大化.
    Maximize,
}

/// CSR 格式约束矩阵（D11）.
///
/// 约束矩阵以行压缩稀疏格式存储：
/// - `row_start[i..i+1]` 给出第 i 行非零元素在 `col_index`/`values` 中的范围
/// - `row_start.len() == num_rows + 1`
/// - `col_index.len() == num_nz`
/// - `values.len() == num_nz`
#[derive(Debug, Clone)]
pub struct ConstraintMatrix {
    /// 约束行数.
    pub num_rows: usize,
    /// 非零元素数.
    pub num_nz: usize,
    /// 行起始索引（长度 = num_rows + 1）.
    pub row_start: Vec<i32>,
    /// 列索引（长度 = num_nz）.
    pub col_index: Vec<i32>,
    /// 非零值（长度 = num_nz）.
    pub values: Vec<f64>,
}

impl ConstraintMatrix {
    /// 创建 CSR 格式约束矩阵.
    pub fn new(
        num_rows: usize,
        num_nz: usize,
        row_start: Vec<i32>,
        col_index: Vec<i32>,
        values: Vec<f64>,
    ) -> Self {
        Self {
            num_rows,
            num_nz,
            row_start,
            col_index,
            values,
        }
    }
}

/// LP 问题定义.
///
/// v0.65.0 将扩展为完整 DSL；v0.64.0 仅定义矩阵格式.
#[derive(Debug, Clone)]
pub struct LpProblem {
    /// 变量名列表.
    pub variables: Vec<String>,
    /// 变量下界.
    pub lower_bounds: Vec<f64>,
    /// 变量上界.
    pub upper_bounds: Vec<f64>,
    /// 变量类型.
    pub var_types: Vec<VarType>,
    /// 目标函数系数.
    pub objective: Vec<f64>,
    /// 目标方向.
    pub sense: ObjectiveSense,
    /// 约束矩阵（CSR 格式）.
    pub constraints: ConstraintMatrix,
    /// 约束下界.
    pub rhs_lower: Vec<f64>,
    /// 约束上界.
    pub rhs_upper: Vec<f64>,
}
