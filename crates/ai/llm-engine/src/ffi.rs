//! llama.cpp C 库 FFI 绑定（D3 / D10：feature-gated）.
//!
//! 仅当启用 `llama-cpp` feature 且链接 llama.cpp C 库时编译。
//! 所有声明使用 `core::ffi::*` 类型，确保 no_std 兼容。

#![cfg(feature = "llama-cpp")]

use core::ffi::{c_char, c_float, c_int, c_uint, c_void};

extern "C" {
    /// 创建 llama.cpp 推理上下文.
    ///
    /// 返回上下文指针，由 `LlamaCppEngine` 持有，`Drop` 时调用 `llama_free` 释放。
    pub fn llama_init() -> *mut c_void;

    /// 加载模型文件到上下文.
    ///
    /// 返回 0 表示成功，非 0 表示失败。
    pub fn llama_load_model(ctx: *mut c_void, path: *const c_char) -> c_int;

    /// 执行推理.
    ///
    /// 返回生成的 C 字符串指针，调用方需通过 `llama_free_result` 释放。
    /// 返回 `null` 表示推理失败。
    pub fn llama_infer(
        ctx: *mut c_void,
        prompt: *const c_char,
        max_tokens: c_uint,
        temperature: c_float,
    ) -> *mut c_char;

    /// 释放 `llama_infer` 返回的字符串.
    pub fn llama_free_result(result: *mut c_char);

    /// 释放 `llama_init` 返回的上下文.
    pub fn llama_free(ctx: *mut c_void);

    /// 设置推理设备（0=CPU，1=CUDA，2=Metal，3=NPU）.
    ///
    /// 返回 0 表示成功，非 0 表示设备不可用。
    pub fn llama_set_device(ctx: *mut c_void, device: c_int) -> c_int;
}
