//! HiGHS C API FFI 绑定（D2/D10）.
//!
//! 仅在 `highs-ffi` feature 启用时编译。对应 HiGHS `highs.h` 中的 C 接口.

use core::ffi::{c_char, c_int, c_void};

/// HiGHS 对象句柄.
pub type HighsPtr = *mut c_void;

extern "C" {
    /// 创建 HiGHS 对象。返回空指针表示失败.
    pub fn Highs_create() -> HighsPtr;
    /// 销毁 HiGHS 对象.
    pub fn Highs_destroy(highs: HighsPtr);
    /// 传递 LP 问题.
    pub fn Highs_passLp(
        highs: HighsPtr,
        num_col: c_int,
        num_row: c_int,
        num_nz: c_int,
        a_format: c_int,
        sense: c_int,
        offset: f64,
        col_cost: *const f64,
        col_lower: *const f64,
        col_upper: *const f64,
        row_lower: *const f64,
        row_upper: *const f64,
        a_start: *const c_int,
        a_index: *const c_int,
        a_value: *const f64,
    ) -> c_int;
    /// 传递 MIP 问题（v0.102.0 增量：同 `Highs_passLp`，尾部追加整数性数组）.
    ///
    /// `integrality` 长度 = num_col：0=连续，1=整数（Binary 以 [0,1] 界 + 1 表达）.
    pub fn Highs_passMip(
        highs: HighsPtr,
        num_col: c_int,
        num_row: c_int,
        num_nz: c_int,
        a_format: c_int,
        sense: c_int,
        offset: f64,
        col_cost: *const f64,
        col_lower: *const f64,
        col_upper: *const f64,
        row_lower: *const f64,
        row_upper: *const f64,
        a_start: *const c_int,
        a_index: *const c_int,
        a_value: *const f64,
        integrality: *const c_int,
    ) -> c_int;
    /// 运行求解.
    pub fn Highs_run(highs: HighsPtr) -> c_int;
    /// 获取求解状态.
    pub fn Highs_getModelStatus(highs: HighsPtr) -> c_int;
    /// 获取目标函数值.
    pub fn Highs_getObjectiveValue(highs: HighsPtr) -> f64;
    /// 获取变量解.
    pub fn Highs_getSolution(
        highs: HighsPtr,
        col_value: *mut f64,
        col_dual: *mut f64,
        row_value: *mut f64,
        row_dual: *mut f64,
    ) -> c_int;
    /// 设置字符串参数.
    pub fn Highs_setStringOptionValue(
        highs: HighsPtr,
        option: *const c_char,
        value: *const c_char,
    ) -> c_int;
    /// 设置双精度参数.
    pub fn Highs_setDoubleOptionValue(highs: HighsPtr, option: *const c_char, value: f64) -> c_int;
    /// 注入热启动初始解（v0.103.0 增量）.
    ///
    /// `col_value` 长度 = num_col，为完整解向量（连续/整数列已合并）.
    pub fn Highs_setSolution(highs: HighsPtr, col_value: *const f64) -> c_int;
}
