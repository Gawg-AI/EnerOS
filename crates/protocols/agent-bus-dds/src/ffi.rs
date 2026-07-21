//! Cyclone DDS C 库 FFI 绑定（D3 / D10：feature-gated）.
//!
//! 仅当启用 `cyclone-dds` feature 且链接 Cyclone DDS C 库（`libddsc.so`）时编译。
//! 所有声明使用 `core::ffi::*` 类型，确保 no_std 兼容。
//!
//! # SAFETY
//!
//! 所有 extern "C" 函数调用均为 `unsafe`，调用方需确保：
//! - 传入的实体句柄有效（由 `dds_create_*` 返回且未被 `dds_delete` 释放）
//! - 指针参数指向合法内存，且调用期间不会被释放
//! - 返回值正确处理（<0 表示错误）

#![cfg(feature = "cyclone-dds")]
// FFI 绑定镜像 Cyclone DDS C 库 API，使用 C 风格命名（snake_case 类型名）。
#![allow(non_camel_case_types)]

use core::ffi::{c_int, c_void};

/// DDS 实体句柄类型（Cyclone DDS C 库，>0 成功，<0 失败）.
pub type dds_entity_t = c_int;

/// DDS 返回码类型（>=0 成功，<0 失败）.
pub type dds_return_t = c_int;

// SAFETY: 以下 extern "C" 声明对应 Cyclone DDS C 库（libddsc）的导出符号。
// 启用 `cyclone-dds` feature 时需链接 `libddsc.so`。
extern "C" {
    /// 创建 DDS participant.
    ///
    /// 返回 participant 实体句柄（>0 成功，<0 失败）。
    /// 返回的句柄由 `CycloneDdsNode` 持有，`Drop` 时调用 `dds_delete` 释放。
    ///
    /// # SAFETY
    /// - `qos` / `listener` 传 `null` 表示使用默认值
    /// - `domain_id` 为合法的 DDS 域 ID
    pub fn dds_create_participant(
        domain_id: c_int,
        qos: *const c_void,
        listener: *const c_void,
    ) -> dds_entity_t;

    /// 创建 DDS writer.
    ///
    /// 返回 writer 实体句柄（>0 成功，<0 失败）。
    ///
    /// # SAFETY
    /// - `participant` 为有效的 participant 句柄
    /// - `topic` 为有效的 topic 句柄
    /// - `qos` / `listener` 传 `null` 表示使用默认值
    pub fn dds_create_writer(
        participant: dds_entity_t,
        topic: dds_entity_t,
        qos: *const c_void,
        listener: *const c_void,
    ) -> dds_entity_t;

    /// 创建 DDS reader.
    ///
    /// 返回 reader 实体句柄（>0 成功，<0 失败）。
    ///
    /// # SAFETY
    /// - `participant` 为有效的 participant 句柄
    /// - `topic` 为有效的 topic 句柄
    /// - `qos` / `listener` 传 `null` 表示使用默认值
    pub fn dds_create_reader(
        participant: dds_entity_t,
        topic: dds_entity_t,
        qos: *const c_void,
        listener: *const c_void,
    ) -> dds_entity_t;

    /// 写入样本.
    ///
    /// 返回写入的样本数（>=0 成功，<0 失败）。
    ///
    /// # SAFETY
    /// - `writer` 为有效的 writer 句柄
    /// - `data` 指向合法的序列化样本数据，调用期间不会被释放
    pub fn dds_write(writer: dds_entity_t, data: *const c_void) -> dds_return_t;

    /// 读取样本（不清空 reader 缓存）.
    ///
    /// 返回读取的样本数（>=0 成功，<0 失败）。
    ///
    /// # SAFETY
    /// - `reader` 为有效的 reader 句柄
    /// - `buf` 指向合法的指针数组，容量 >= `max_samples`
    pub fn dds_read(
        reader: dds_entity_t,
        buf: *mut *mut c_void,
        buf_size: usize,
        max_samples: usize,
    ) -> dds_return_t;

    /// 取走样本（清空 reader 缓存中被取走的样本）.
    ///
    /// 返回取走的样本数（>=0 成功，<0 失败）。
    ///
    /// # SAFETY
    /// - `reader` 为有效的 reader 句柄
    /// - `buf` 指向合法的指针数组，容量 >= `max_samples`
    pub fn dds_take(
        reader: dds_entity_t,
        buf: *mut *mut c_void,
        buf_size: usize,
        max_samples: usize,
    ) -> dds_return_t;

    /// 删除实体（释放资源）.
    ///
    /// 删除 participant 会级联删除其下所有 reader/writer。
    ///
    /// # SAFETY
    /// - `entity` 为有效的实体句柄，且尚未被释放
    /// - 释放后不可再使用该句柄
    pub fn dds_delete(entity: dds_entity_t) -> dds_return_t;
}
