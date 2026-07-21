//! GGUF 张量数据类型（D11）.
//!
//! `GgufDtype` 枚举 GGUF 规范定义的全部量化类型，并通过 `to_quantization`
//! 映射到 v0.59.0 的 [`eneros_llm_engine::model::Quantization`]（仅 4 种
//! 边缘推理推荐类型有映射，其余返回 `None`）。

use eneros_llm_engine::model::Quantization;

/// GGUF 张量数据类型（与 GGUF 规范的 dtype 值一一对应）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum GgufDtype {
    /// 32-bit 浮点.
    F32 = 0,
    /// 16-bit 浮点.
    F16 = 1,
    /// 4-bit 量化（基础版）.
    Q4_0 = 2,
    /// 4-bit 量化（版本 1）.
    Q4_1 = 3,
    /// 5-bit 量化（基础版）.
    Q5_0 = 6,
    /// 5-bit 量化（版本 1）.
    Q5_1 = 7,
    /// 8-bit 量化（基础版）.
    Q8_0 = 8,
    /// 8-bit 量化（版本 1）.
    Q8_1 = 9,
    /// k-quant 2-bit.
    Q2_K = 10,
    /// k-quant 3-bit.
    Q3_K = 11,
    /// k-quant 4-bit medium.
    Q4_K = 12,
    /// k-quant 5-bit.
    Q5_K = 13,
    /// k-quant 6-bit.
    Q6_K = 14,
    /// k-quant 8-bit.
    Q8_K = 15,
}

impl GgufDtype {
    /// 从 u32 原始值构造 `GgufDtype`，非法值返回 `None`.
    pub fn from_u32(value: u32) -> Option<GgufDtype> {
        match value {
            0 => Some(Self::F32),
            1 => Some(Self::F16),
            2 => Some(Self::Q4_0),
            3 => Some(Self::Q4_1),
            6 => Some(Self::Q5_0),
            7 => Some(Self::Q5_1),
            8 => Some(Self::Q8_0),
            9 => Some(Self::Q8_1),
            10 => Some(Self::Q2_K),
            11 => Some(Self::Q3_K),
            12 => Some(Self::Q4_K),
            13 => Some(Self::Q5_K),
            14 => Some(Self::Q6_K),
            15 => Some(Self::Q8_K),
            _ => None,
        }
    }

    /// 映射到 v0.59.0 的 `Quantization`（D11）.
    ///
    /// 仅 F16 / Q8_0 / Q4_0 / Q4_K 有映射；F32、Q4_1、Q5_0、Q5_1、Q8_1、
    /// Q2_K、Q3_K、Q5_K、Q6_K、Q8_K 在边缘推理中无对应推荐类型，返回 `None`。
    pub fn to_quantization(&self) -> Option<Quantization> {
        match self {
            Self::F16 => Some(Quantization::F16),
            Self::Q8_0 => Some(Quantization::Q8_0),
            Self::Q4_0 => Some(Quantization::Q4_0),
            Self::Q4_K => Some(Quantization::Q4_K_M),
            _ => None,
        }
    }
}
