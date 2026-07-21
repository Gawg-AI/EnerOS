//! ONNX Runtime C API FFI 绑定与 `OnnxEngine` 安全封装（D6/D7）.
//!
//! 仅在 `onnx-ffi` feature 启用时编译。extern 声明与蓝图 §4.5 签名一致；
//! `OnnxEngine` RAII 管理 session 生命周期（NonNull + Drop）.

use alloc::vec;
use alloc::vec::Vec;
use core::ffi::{c_float, c_int, c_void};
use core::ptr::NonNull;

use crate::heuristic_net::{InferEngine, WarmError};

extern "C" {
    /// 创建 ONNX Runtime session（蓝图 §4.5）.
    pub fn ort_create_session(path: *const u8, len: c_int) -> *mut c_void;
    /// 运行 ONNX Runtime session（蓝图 §4.5）.
    pub fn ort_run_session(
        session: *mut c_void,
        input: *const c_float,
        input_len: c_int,
        output: *mut c_float,
        output_len: c_int,
    ) -> c_int;
    /// 释放 ONNX Runtime session（蓝图 §4.5）.
    pub fn ort_free_session(session: *mut c_void);
}

/// ONNX Runtime 推理引擎（feature = "onnx-ffi"，D6）.
///
/// RAII 管理 session：`load` 调 `ort_create_session`，`Drop` 调 `ort_free_session`。
/// `input_dim`/`output_dim` 为构造参数（消除蓝图 72/96 硬编码，D7）。
/// 所有 FFI 调用包裹在 `unsafe` 块中并附 SAFETY 注释.
pub struct OnnxEngine {
    /// ONNX session 句柄（NonNull 保证非空）.
    session: NonNull<c_void>,
    /// 输入维度（构造参数，D7）.
    pub input_dim: usize,
    /// 输出维度（构造参数，D7）.
    pub output_dim: usize,
}

impl OnnxEngine {
    /// 加载 ONNX 模型创建推理引擎.
    ///
    /// `path` 为模型文件路径；`input_dim`/`output_dim` 为模型输入/输出维度。
    /// 创建失败（空指针）返回 `WarmError::ModelLoadFailed`.
    pub fn load(path: &str, input_dim: usize, output_dim: usize) -> Result<Self, WarmError> {
        let bytes = path.as_bytes();
        // SAFETY: `bytes` 指向有效 UTF-8 字节串，长度 == bytes.len()，
        // 其生命周期覆盖本次 FFI 调用；`ort_create_session` 返回有效指针或空指针.
        let raw = unsafe { ort_create_session(bytes.as_ptr(), bytes.len() as c_int) };
        let session = NonNull::new(raw).ok_or(WarmError::ModelLoadFailed)?;
        Ok(Self {
            session,
            input_dim,
            output_dim,
        })
    }
}

impl InferEngine for OnnxEngine {
    fn infer(&self, input: &[f32]) -> Result<Vec<f32>, WarmError> {
        let mut output = vec![0.0f32; self.output_dim];
        // SAFETY: `session` 有效（`load` 已做空指针检查）；`input` 指针指向有效
        // slice 内存，`output` 缓冲区长度 == output_dim，二者生命周期均覆盖本次
        // FFI 调用；返回码非 0 视为推理失败.
        let ret = unsafe {
            ort_run_session(
                self.session.as_ptr(),
                input.as_ptr(),
                input.len() as c_int,
                output.as_mut_ptr(),
                output.len() as c_int,
            )
        };
        if ret != 0 {
            return Err(WarmError::InferenceFailed(ret));
        }
        Ok(output)
    }

    fn input_dim(&self) -> usize {
        self.input_dim
    }

    fn output_dim(&self) -> usize {
        self.output_dim
    }
}

impl Drop for OnnxEngine {
    fn drop(&mut self) {
        // SAFETY: `session` 是有效的 ONNX session 指针，由 `load` 创建；
        // `ort_free_session` 释放资源，调用后不再使用 session.
        unsafe {
            ort_free_session(self.session.as_ptr());
        }
    }
}
