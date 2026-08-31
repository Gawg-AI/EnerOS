//! HiGHS C API FFI 绑定（feature = "highs-ffi"）.
//!
//! 直接绑定 HiGHS C API（`src/interfaces/highs_c_api.h`），无中间包装。
//! 所有 `unsafe` 块在 `highs.rs` 中调用，此处仅声明 extern "C" 签名。
//!
//! # Safety
//!
//! HiGHS 句柄（`*mut c_void`）由 `HighsSolver` 通过 RAII 管理：
//! - `Highs_create` 返回的指针由 `HighsSolver` 持有
//! - `Drop` 调用 `Highs_destroy` 释放
//! - 其他函数接受非 null 的 `highs` 指针

#![cfg(feature = "highs-ffi")]

use core::ffi::c_void;

/// HiGHS 整数类型（C 侧为 `HighsInt`，通常是 `int` = i32）。
pub type HighsInt = i32;

// --- 状态常量（来自 highs_c_api.h）---

/// 矩阵格式：列压缩（CSC）。
pub const MATRIX_FORMAT_COLWISE: HighsInt = 1;
/// 矩阵格式：行压缩（CSR）。
pub const MATRIX_FORMAT_ROWWISE: HighsInt = 2;

/// 目标方向：最小化。
pub const OBJ_SENSE_MINIMIZE: HighsInt = 1;
/// 目标方向：最大化。
pub const OBJ_SENSE_MAXIMIZE: HighsInt = -1;

/// 模型状态：最优解。
pub const MODEL_STATUS_OPTIMAL: HighsInt = 7;
/// 模型状态：不可行。
pub const MODEL_STATUS_INFEASIBLE: HighsInt = 8;
/// 模型状态：无界或不可行。
pub const MODEL_STATUS_UNBOUNDED_OR_INFEASIBLE: HighsInt = 9;
/// 模型状态：无界。
pub const MODEL_STATUS_UNBOUNDED: HighsInt = 10;

/// 变量类型：连续。
pub const VAR_TYPE_CONTINUOUS: HighsInt = 0;
/// 变量类型：整数。
pub const VAR_TYPE_INTEGER: HighsInt = 1;

// --- C API 函数声明 ---

extern "C" {
    /// 创建 HiGHS 实例。返回不透明指针，失败返回 null。
    pub fn Highs_create() -> *mut c_void;

    /// 销毁 HiGHS 实例并释放所有关联内存。
    pub fn Highs_destroy(highs: *mut c_void);

    /// 传入 LP 问题（连续）。返回 0=成功，<0=错误。
    #[allow(clippy::too_many_arguments)]
    pub fn Highs_passLp(
        highs: *mut c_void,
        num_col: HighsInt,
        num_row: HighsInt,
        num_nz: HighsInt,
        a_format: HighsInt,
        sense: HighsInt,
        offset: f64,
        col_cost: *const f64,
        col_lower: *const f64,
        col_upper: *const f64,
        row_lower: *const f64,
        row_upper: *const f64,
        a_start: *const HighsInt,
        a_index: *const HighsInt,
        a_value: *const f64,
    ) -> HighsInt;

    /// 传入 MILP 问题（含整数约束）。返回 0=成功，<0=错误。
    #[allow(clippy::too_many_arguments)]
    pub fn Highs_passMip(
        highs: *mut c_void,
        num_col: HighsInt,
        num_row: HighsInt,
        num_nz: HighsInt,
        a_format: HighsInt,
        sense: HighsInt,
        offset: f64,
        col_cost: *const f64,
        col_lower: *const f64,
        col_upper: *const f64,
        row_lower: *const f64,
        row_upper: *const f64,
        a_start: *const HighsInt,
        a_index: *const HighsInt,
        a_value: *const f64,
        integrality: *const HighsInt,
    ) -> HighsInt;

    /// 求解已传入的问题。返回 0=成功，<0=错误。
    pub fn Highs_run(highs: *mut c_void) -> HighsInt;

    /// 获取解向量（col_value, col_dual, row_value, row_dual）。
    /// 调用者需预先分配足够大小的数组。
    pub fn Highs_getSolution(
        highs: *const c_void,
        col_value: *mut f64,
        col_dual: *mut f64,
        row_value: *mut f64,
        row_dual: *mut f64,
    ) -> HighsInt;

    /// 获取目标函数值。
    pub fn Highs_getObjectiveValue(highs: *const c_void) -> f64;

    /// 获取模型状态（kHighsModelStatus* 常量）。
    pub fn Highs_getModelStatus(highs: *const c_void) -> HighsInt;

    /// 读取 double 型 info 字段（如 "mip_gap"；蓝图61 [09] §3.5）。
    /// HiGHS 1.7.2 C API 实名 `Highs_getDoubleInfoValue`；返回
    /// kHighsStatus（0=Ok，1=Warning，<0=Error）。
    pub fn Highs_getDoubleInfoValue(
        highs: *const c_void,
        info: *const u8,
        value: *mut f64,
    ) -> HighsInt;

    /// 获取列数。
    pub fn Highs_getNumCol(highs: *const c_void) -> HighsInt;

    /// 获取行数。
    pub fn Highs_getNumRow(highs: *const c_void) -> HighsInt;

    /// 设置布尔选项（如 "presolve", "output_flag"）。
    pub fn Highs_setBoolOptionValue(
        highs: *mut c_void,
        option: *const u8,
        value: HighsInt,
    ) -> HighsInt;

    /// 设置整型选项（如 "random_seed"）（v0.202.0 P0-20）。
    pub fn Highs_setIntOptionValue(
        highs: *mut c_void,
        option: *const u8,
        value: HighsInt,
    ) -> HighsInt;

    /// 设置双精度选项（如 "time_limit", "mip_rel_gap"）（v0.202.0 P0-20）。
    pub fn Highs_setDoubleOptionValue(
        highs: *mut c_void,
        option: *const u8,
        value: f64,
    ) -> HighsInt;

    /// 设置字符串选项（如 "solver"）（v0.202.0 P0-20）。
    pub fn Highs_setStringOptionValue(
        highs: *mut c_void,
        option: *const u8,
        value: *const u8,
    ) -> HighsInt;

    /// 注入求解器初始解（热启动，v0.204.0 P0-19）。
    ///
    /// `col_value` 为列（变量）值数组（长度 = 列数）；`row_value`/
    /// `col_dual`/`row_dual` 可为 NULL（本封装只注入原始解）。
    pub fn Highs_setSolution(
        highs: *mut c_void,
        col_value: *const f64,
        row_value: *const f64,
        col_dual: *const f64,
        row_dual: *const f64,
    ) -> HighsInt;

    /// 清空模型但不销毁实例（用于复用）。
    pub fn Highs_clearModel(highs: *mut c_void) -> HighsInt;
}
