//! 决策变量与链式构建器（D3/D4/D6）.

use alloc::string::String;

use eneros_solver_core::problem::VarType;

/// HiGHS 认可的边界上限（`kHighsInf = 1e30`，P0-21，v0.201.0）。
///
/// HiGHS C API 要求 `|bound| ≤ 1e30`；`f64::INFINITY` 会被
/// `Highs_passLp/passMip` 拒绝。所有 DSL 生成的变量界与约束 rhs 一律
/// 钳制到此值（或以下）。
pub const K_HIGHS_INF: f64 = 1e30;

/// 决策变量.
#[derive(Debug, Clone)]
pub struct Variable {
    /// 变量名.
    pub name: String,
    /// 下界（默认 0.0）.
    pub lower_bound: f64,
    /// 上界（默认 `K_HIGHS_INF` = 1e30，P0-21）.
    pub upper_bound: f64,
    /// 变量类型（复用 v0.64.0 `VarType`，D3）.
    pub var_type: VarType,
    /// 变量索引（编译前 None，编译后 Some(idx)，D4）.
    pub index: Option<usize>,
}

/// 变量链式构建器（D6：Builder 模式）.
pub struct VarBuilder {
    name: String,
    lower: f64,
    upper: f64,
    var_type: VarType,
}

impl VarBuilder {
    /// 创建变量构建器，默认 lower=0.0, upper=K_HIGHS_INF（1e30，P0-21）。
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            lower: 0.0,
            upper: K_HIGHS_INF,
            var_type: VarType::Continuous,
        }
    }

    /// 设置下界（超界字面量 → panic，带调用点定位）。
    #[track_caller]
    pub fn lower(mut self, v: f64) -> Self {
        assert!(
            (-K_HIGHS_INF..=K_HIGHS_INF).contains(&v),
            "VarBuilder::lower 越界: {v} 超出 ±kHighsInf(1e30)（P0-21：DSL 边界钳制）"
        );
        self.lower = v;
        self
    }

    /// 设置上界（超界字面量 → panic，带调用点定位）。
    #[track_caller]
    pub fn upper(mut self, v: f64) -> Self {
        assert!(
            (-K_HIGHS_INF..=K_HIGHS_INF).contains(&v),
            "VarBuilder::upper 越界: {v} 超出 ±kHighsInf(1e30)（P0-21：DSL 边界钳制）"
        );
        self.upper = v;
        self
    }

    /// 设置范围 [lo, hi]（超界字面量 → panic）。
    #[track_caller]
    pub fn range(mut self, lo: f64, hi: f64) -> Self {
        assert!(
            lo >= -K_HIGHS_INF && hi <= K_HIGHS_INF,
            "VarBuilder::range 越界: [{lo}, {hi}] 超出 ±kHighsInf(1e30)（P0-21）"
        );
        self.lower = lo;
        self.upper = hi;
        self
    }

    /// 等价于 `lower(0.0)`.
    pub fn non_negative(self) -> Self {
        self.lower(0.0)
    }

    /// 设置为整数变量.
    pub fn integer(mut self) -> Self {
        self.var_type = VarType::Integer;
        self
    }

    /// 设置为 0-1 变量（同时 range(0.0, 1.0)）.
    pub fn binary(mut self) -> Self {
        self.var_type = VarType::Binary;
        self.lower = 0.0;
        self.upper = 1.0;
        self
    }

    /// 构建变量（index = None）.
    pub fn build(self) -> Variable {
        Variable {
            name: self.name,
            lower_bound: self.lower,
            upper_bound: self.upper,
            var_type: self.var_type,
            index: None,
        }
    }
}
