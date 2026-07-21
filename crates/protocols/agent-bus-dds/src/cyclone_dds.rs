//! CycloneDdsNode — Cyclone DDS C 库的 Rust 封装（D3 / D10：feature-gated）.
//!
//! 仅当启用 `cyclone-dds` feature 且链接 Cyclone DDS C 库（`libddsc.so`）时编译。
//! 所有 `unsafe` 块附 SAFETY 注释说明不变量（D10）。
//!
//! # 骨架说明
//!
//! 本模块为骨架实现，`create_reader`/`create_writer`/`read`/`take`/`write` 方法体
//! 返回 `Err(DdsError::Ffi(-1))` 占位。真实实现需：
//! 1. 创建 topic（`dds_create_topic`）
//! 2. 序列化 payload（Cyclone DDS CDR 序列化）
//! 3. 调用对应 C API
//!
//! CI 默认不启用 `cyclone-dds` feature，本模块不参与编译。

#![cfg(feature = "cyclone-dds")]

use alloc::vec::Vec;

use slotmap::SlotMap;

use crate::config::DdsConfig;
use crate::error::DdsError;
use crate::node::{DdsNode, DdsNodeConfig};
use crate::qos::QosPolicy;
use crate::types::{DdsSample, ParticipantId, ReaderId, WriterId};

/// Cyclone DDS 节点实现（feature-gated）.
///
/// 通过 FFI 调用 Cyclone DDS C 库执行真实发布/订阅。
/// participant 实体句柄由本结构体持有，`Drop` 时调用 `dds_delete` 释放（D10）。
///
/// 注意：启用此 feature 需链接 `libddsc.so`。
pub struct CycloneDdsNode {
    /// 节点配置.
    config: DdsConfig,
    /// participant 实体句柄表（Cyclone DDS C 库返回的 entity_t）.
    participants: SlotMap<ParticipantId, i32>,
    /// reader 实体句柄表.
    readers: SlotMap<ReaderId, i32>,
    /// writer 实体句柄表.
    writers: SlotMap<WriterId, i32>,
    /// 节点是否已关闭.
    shutdown: bool,
}

impl CycloneDdsNode {
    /// 创建 CycloneDdsNode.
    pub fn new(config: DdsConfig) -> Self {
        Self {
            config,
            participants: SlotMap::with_key(),
            readers: SlotMap::with_key(),
            writers: SlotMap::with_key(),
            shutdown: false,
        }
    }
}

impl DdsNodeConfig for CycloneDdsNode {
    fn config(&self) -> &DdsConfig {
        &self.config
    }
}

impl DdsNode for CycloneDdsNode {
    fn create_participant(&mut self) -> Result<ParticipantId, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        // SAFETY: dds_create_participant 是 C 库函数，domain_id 来自配置。
        // qos/listener 传 null 表示使用默认值。返回的句柄由 SlotMap 管理，
        // Drop 时调用 dds_delete 释放。
        let entity = unsafe {
            crate::ffi::dds_create_participant(
                self.config.domain_id as i32,
                core::ptr::null(),
                core::ptr::null(),
            )
        };
        if entity < 0 {
            return Err(DdsError::Ffi(entity));
        }
        Ok(self.participants.insert(entity))
    }

    fn create_reader(
        &mut self,
        participant: ParticipantId,
        _topic: &str,
        _qos: QosPolicy,
    ) -> Result<ReaderId, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let _p = self
            .participants
            .get(participant)
            .ok_or(DdsError::InvalidHandle)?;
        // 骨架实现：C 库未链接，返回 Ffi 错误.
        // 真实实现需创建 topic、配置 qos、调用 dds_create_reader.
        Err(DdsError::Ffi(-1))
    }

    fn create_writer(
        &mut self,
        participant: ParticipantId,
        _topic: &str,
        _qos: QosPolicy,
    ) -> Result<WriterId, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        let _p = self
            .participants
            .get(participant)
            .ok_or(DdsError::InvalidHandle)?;
        // 骨架实现：C 库未链接，返回 Ffi 错误.
        Err(DdsError::Ffi(-1))
    }

    fn read(&mut self, _reader: ReaderId, _max_samples: usize) -> Result<Vec<DdsSample>, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        // 骨架实现：C 库未链接.
        Err(DdsError::Ffi(-1))
    }

    fn take(&mut self, _reader: ReaderId, _max_samples: usize) -> Result<Vec<DdsSample>, DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        // 骨架实现：C 库未链接.
        Err(DdsError::Ffi(-1))
    }

    fn write(&mut self, _writer: WriterId, _data: &[u8]) -> Result<(), DdsError> {
        if self.shutdown {
            return Err(DdsError::Closed);
        }
        // 骨架实现：C 库未链接.
        Err(DdsError::Ffi(-1))
    }

    fn shutdown(&mut self) -> Result<(), DdsError> {
        if !self.shutdown {
            // 释放所有实体（writer → reader → participant 顺序，participant 级联释放）.
            for (_, &entity) in &self.writers {
                // SAFETY: entity 由 dds_create_writer 返回且尚未释放。
                unsafe {
                    crate::ffi::dds_delete(entity);
                }
            }
            for (_, &entity) in &self.readers {
                // SAFETY: entity 由 dds_create_reader 返回且尚未释放。
                unsafe {
                    crate::ffi::dds_delete(entity);
                }
            }
            for (_, &entity) in &self.participants {
                // SAFETY: entity 由 dds_create_participant 返回且尚未释放。
                // 删除 participant 会级联释放其下所有 reader/writer。
                unsafe {
                    crate::ffi::dds_delete(entity);
                }
            }
            self.shutdown = true;
        }
        Ok(())
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown
    }
}

impl Drop for CycloneDdsNode {
    fn drop(&mut self) {
        if !self.shutdown {
            let _ = self.shutdown();
        }
    }
}
