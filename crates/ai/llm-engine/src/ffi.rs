//! llama.cpp C API FFI 绑定（feature = "llama-cpp"）.
//!
//! 直接绑定 llama.cpp C API（`include/llama.h`），b4000 版本。
//! 仅声明 extern "C" 签名和 `#[repr(C)]` 结构体，unsafe 调用在 `llama_cpp.rs` 中。
//!
//! # 引用
//!
//! - llama.cpp b4000 (commit 在 third_party/llama.cpp/.git/HEAD)
//! - License: MIT

#![cfg(feature = "llama-cpp")]

use core::ffi::c_void;

/// llama.cpp token 类型（int32_t）。
pub type LlamaToken = i32;
/// llama.cpp 位置类型（int32_t）。
pub type LlamaPos = i32;
/// llama.cpp 序列 ID 类型（int32_t）。
pub type LlamaSeqId = i32;

/// llama.cpp 模型参数（对应 C 侧 `struct llama_model_params`）.
///
/// 字段顺序与 C 头文件一致，`#[repr(C)]` 保证 ABI 兼容。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LlamaModelParams {
    pub n_gpu_layers: i32,
    pub split_mode: i32,
    pub main_gpu: i32,
    pub tensor_split: *const f32,
    pub rpc_servers: *const u8,
    pub progress_callback: *const c_void,
    pub progress_callback_user_data: *mut c_void,
    pub kv_overrides: *const c_void,
    pub vocab_only: bool,
    pub use_mmap: bool,
    pub use_mlock: bool,
    pub check_tensors: bool,
}

/// llama.cpp 上下文参数（对应 C 侧 `struct llama_context_params`）.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LlamaContextParams {
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub n_seq_max: u32,
    pub n_threads: i32,
    pub n_threads_batch: i32,
    pub rope_scaling_type: i32,
    pub pooling_type: i32,
    pub attention_type: i32,
    pub rope_freq_base: f32,
    pub rope_freq_scale: f32,
    pub yarn_ext_factor: f32,
    pub yarn_attn_factor: f32,
    pub yarn_beta_fast: f32,
    pub yarn_beta_slow: f32,
    pub yarn_orig_ctx: u32,
    pub defrag_thold: f32,
    pub cb_eval: *const c_void,
    pub cb_eval_user_data: *mut c_void,
    pub type_k: i32,
    pub type_v: i32,
    pub logits_all: bool,
    pub embeddings: bool,
    pub offload_kqv: bool,
    pub flash_attn: bool,
    pub no_perf: bool,
    pub abort_callback: *const c_void,
    pub abort_callback_data: *mut c_void,
}

/// sampler chain 参数（对应 C 侧 `struct llama_sampler_chain_params`）.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LlamaSamplerChainParams {
    pub no_perf: bool,
}

/// batch 结构体（对应 C 侧 `struct llama_batch`）.
///
/// 派生 `Clone + Copy`：所有字段是 Copy 类型（i32/指针），C API 按 value 传递 batch。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LlamaBatch {
    pub n_tokens: i32,
    pub token: *mut LlamaToken,
    pub embd: *mut f32,
    pub pos: *mut LlamaPos,
    pub n_seq_id: *mut i32,
    pub seq_id: *mut *mut LlamaSeqId,
    pub logits: *mut i8,
}

extern "C" {
    // --- 后端初始化/清理 ---
    pub fn llama_backend_init();
    pub fn llama_backend_free();

    // --- 模型参数默认值 ---
    pub fn llama_model_default_params() -> LlamaModelParams;
    pub fn llama_context_default_params() -> LlamaContextParams;
    pub fn llama_sampler_chain_default_params() -> LlamaSamplerChainParams;

    // --- 模型加载/释放 ---
    pub fn llama_load_model_from_file(
        path_model: *const u8,
        params: LlamaModelParams,
    ) -> *mut c_void;
    pub fn llama_free_model(model: *mut c_void);

    // --- 上下文创建/释放 ---
    pub fn llama_new_context_with_model(
        model: *mut c_void,
        params: LlamaContextParams,
    ) -> *mut c_void;
    pub fn llama_free(ctx: *mut c_void);

    // --- 查询 ---
    pub fn llama_n_ctx(ctx: *const c_void) -> u32;
    pub fn llama_n_vocab(model: *const c_void) -> i32;
    pub fn llama_n_ctx_train(model: *const c_void) -> i32;
    pub fn llama_n_embd(model: *const c_void) -> i32;
    pub fn llama_model_size(model: *const c_void) -> u64;
    pub fn llama_model_n_params(model: *const c_void) -> u64;

    // --- 分词 ---
    pub fn llama_tokenize(
        model: *const c_void,
        text: *const u8,
        text_len: i32,
        tokens: *mut LlamaToken,
        n_tokens_max: i32,
        add_special: bool,
        parse_special: bool,
    ) -> i32;

    // --- token 转文本 ---
    // b4000 签名有 6 个参数（含 special: bool），缺少会导致 UB。
    pub fn llama_token_to_piece(
        model: *const c_void,
        token: LlamaToken,
        buf: *mut u8,
        length: i32,
        lstrip: i32,
        special: bool,
    ) -> i32;

    // --- 特殊 token ---
    pub fn llama_token_bos(model: *const c_void) -> LlamaToken;
    pub fn llama_token_eos(model: *const c_void) -> LlamaToken;
    pub fn llama_token_is_eog(model: *const c_void, token: LlamaToken) -> bool;

    // --- batch ---
    pub fn llama_batch_get_one(tokens: *mut LlamaToken, n_tokens: i32) -> LlamaBatch;
    pub fn llama_batch_init(n_tokens_alloc: i32, embd: i32, n_seq_max: i32) -> LlamaBatch;
    pub fn llama_batch_free(batch: LlamaBatch);

    // --- 解码 ---
    pub fn llama_decode(ctx: *mut c_void, batch: LlamaBatch) -> i32;
    pub fn llama_synchronize(ctx: *mut c_void);

    // --- logits ---
    pub fn llama_get_logits(ctx: *mut c_void) -> *mut f32;
    pub fn llama_get_logits_ith(ctx: *mut c_void, i: i32) -> *mut f32;

    // --- sampler ---
    pub fn llama_sampler_chain_init(params: LlamaSamplerChainParams) -> *mut c_void;
    pub fn llama_sampler_chain_add(chain: *mut c_void, sampler: *mut c_void);
    pub fn llama_sampler_free(smpl: *mut c_void);
    pub fn llama_sampler_init_temp(t: f32) -> *mut c_void;
    pub fn llama_sampler_init_top_k(k: i32) -> *mut c_void;
    pub fn llama_sampler_init_top_p(p: f32, min_keep: usize) -> *mut c_void;
    pub fn llama_sampler_sample(smpl: *mut c_void, ctx: *mut c_void, idx: i32) -> LlamaToken;

    // --- GPU 支持 ---
    pub fn llama_supports_gpu_offload() -> bool;
}
