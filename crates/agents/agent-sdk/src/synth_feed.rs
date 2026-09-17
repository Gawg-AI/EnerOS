//! root 合成源 OBS 馈电（REG-70.8-SYNTH-NO-OBS 解封，2026-09-15）——
//! **测试设施**：仅 `qemu-test-synth` 装配消费（closure gate 强制该 feature
//! 不在 release 闭包 ⇒ 生产装配结构性不可达）。
//!
//! 合成馈电从"旧 TelemetrySample 兼容轨"升级为 **OBS 生产轨**：root 以
//! [`crate::obs_batch_wire`] 的 TEL_INGEST_OBS 发 source=1 `ObservationEvent`
//! 批次，使"观测 → 冻结快照 → 独立认证 → dispatch"全链在合成装配可证。
//!
//! 单一事实边界：馈电点的**授权**（来源行、点位范围）与**语义**
//!（unit/asset/schema）唯一事实 = 签名配置 `configs/points.toml`——本模块
//! 只镜像声明面供 root 构帧（root 不持配置），镜像一致性由 control-services
//! 的 mirror 锁测试逐点对照真实配置（漂移即红）。

use alloc::vec::Vec;

use eneros_telemetry_model::event::{
    event_flags, quality_bits, IntegrityTag, ObservationEvent, ObservationValue,
};

use crate::obs_batch_wire::{encode_obs_request_stream, ObsBatchError, FRAME_WIRE_LEN};

/// 合成来源 id（= `configs/points.toml` 的 source 1 行；签名配置分配）。
pub const SYNTH_SOURCE_ID: u32 = 1;
/// 合成馈电租户/站点/资产域（与点表声明面一致；mirror 锁测试校验）。
pub const SYNTH_TENANT_ID: u32 = 1;
/// 站点域。
pub const SYNTH_SITE_ID: u32 = 1;
/// 全局资产域（0 = 未绑定不得进控制快照）。
pub const SYNTH_ASSET_ID: u64 = 1;
/// 注册 schema。
pub const SYNTH_SCHEMA_ID: u32 = 1;
/// schema 版本。
pub const SYNTH_SCHEMA_VERSION: u16 = 1;
/// 点表/映射世代（装配期由配置赋值；合成馈电固定 1——与旧兼容轨同值）。
pub const SYNTH_MAPPING_VERSION: u64 = 1;

/// 合成馈电单点（声明面镜像；语义权威 = configs/points.toml）。
pub struct SynthPoint {
    /// 站内点位 id。
    pub point_id: u32,
    /// 站内设备短 id。
    pub device_id: u16,
    /// 全局资产 id（0 = 未绑定不得进控制快照）。
    pub asset_id: u64,
    /// `UnitCatalog` 封闭单位码（0 不得进控制快照）。
    pub unit_code: u16,
    /// 基准工程值（点 4 的 SOC 双值振荡见 [`synth_event`]）。
    pub value: f64,
}

use eneros_telemetry_model::unit_catalog;

const fn code(unit: eneros_telemetry_model::PhysicalUnit) -> u16 {
    unit_catalog::code_of(unit)
}

/// 馈电声明点全集（网关 1..7 + PCS-1 1001/1003/1004 + PCS-2 1005/1006；值取
/// 45/61 从站应答值经 scale×raw 的工程量，全部落各点声明值域内）。
// Keep the 12-frame transport bound: PCS-1 SOC is already supplied by point 4;
// omit its unused percentage display point 1002.
pub const SYNTH_FEED: [SynthPoint; 12] = [
    SynthPoint {
        point_id: 1,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Volts),
        value: 380.0,
    },
    SynthPoint {
        point_id: 2,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Amperes),
        value: 10.0,
    },
    SynthPoint {
        point_id: 3,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Hertz),
        value: 50.0,
    },
    // SPEC-E WP-E.3：SOC 确定性双值振荡（0.475↔0.525，振幅 0.05 跨出厂
    // 死区边界 0.5）——持续喂死区学习闭环；相位由批次号决定（偶 0.475）。
    SynthPoint {
        point_id: 4,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 0.475,
    },
    SynthPoint {
        point_id: 5,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 1.0,
    },
    SynthPoint {
        point_id: 6,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 0.0,
    },
    // 61 复查矫正：点 7 = 独立过温告警。
    SynthPoint {
        point_id: 7,
        asset_id: 1,
        device_id: 2,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 0.0,
    },
    // PCS-1（从站 2：0x0107=120×0.1、0x0108=5000×0.01、0x0109=400×0.1、
    // 0x010A 线圈=1）。
    SynthPoint {
        point_id: 1001,
        asset_id: 1,
        device_id: 1,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Kilowatts),
        value: 12.0,
    },
    SynthPoint {
        point_id: 1003,
        asset_id: 1,
        device_id: 1,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 40.0,
    },
    SynthPoint {
        point_id: 1004,
        asset_id: 1,
        device_id: 1,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 1.0,
    },
    // 蓝图 70 R10 多资产：PCS-2 功率回读（合同 2 的 readback 点——认证器
    // readback_ok 为 presence 判定，快照受理观测必须含 1005，含 asset 2
    // 分配的远程计划才能过独立认证）。
    SynthPoint {
        point_id: 1005,
        asset_id: 2,
        device_id: 8,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Kilowatts),
        value: 9.0,
    },
    SynthPoint {
        point_id: 1006,
        asset_id: 2,
        device_id: 8,
        unit_code: code(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        value: 0.30,
    },
];

/// 构造单条合成观测事件（`validate()` 门对齐：epoch=0 必须 flags 含 TEST、
/// event_time≠0 不得含 TIME_MISSING、单位码必须在册、F64 必须 finite）。
///
/// 点 4 的值按批次号奇偶翻转（偶 0.475 / 奇 0.525）——学习闭环喂入语义与
/// 旧兼容轨逐值一致；其余点取基准值。
pub fn synth_event(
    point: &SynthPoint,
    batch: u64,
    idx_in_batch: usize,
    now_ns: u64,
) -> ObservationEvent {
    let value = if point.point_id == 4 && batch % 2 == 1 {
        0.525
    } else {
        point.value
    };
    ObservationEvent {
        tenant_id: SYNTH_TENANT_ID,
        site_id: SYNTH_SITE_ID,
        source_id: SYNTH_SOURCE_ID,
        source_epoch: 0,
        // 同 epoch 内严格单调：批次号 × 批容量 + 批内序。
        sequence: batch
            .saturating_mul(SYNTH_FEED.len() as u64)
            .saturating_add(idx_in_batch as u64)
            .saturating_add(1),
        event_time_ns: now_ns,
        ingest_time_ns: now_ns,
        asset_id: point.asset_id,
        device_id: point.device_id,
        unit_code: point.unit_code,
        point_id: point.point_id,
        schema_id: SYNTH_SCHEMA_ID,
        schema_version: SYNTH_SCHEMA_VERSION,
        quality_bits: quality_bits::VALID,
        mapping_version: SYNTH_MAPPING_VERSION,
        value: ObservationValue::F64(value),
        // 本地可信边（合成注入与 source 2 同信任级；Unverified 会被
        // require_integrity 来源隔离）。
        integrity: IntegrityTag::LocalTrustedEdge,
        clock_id: 0,
        flags: event_flags::SIMULATED | event_flags::TEST,
    }
}

/// 批次帧流（`SYNTH_FEED` 顺序；帧长恒 [`FRAME_WIRE_LEN`]）。
pub fn synth_frames(batch: u64, now_ns: u64) -> Vec<[u8; FRAME_WIRE_LEN]> {
    SYNTH_FEED
        .iter()
        .enumerate()
        .map(|(i, p)| synth_event(p, batch, i, now_ns).encode_v1())
        .collect()
}

/// TEL_INGEST_OBS 请求（一次 chunk 传输上限由
/// [`encode_obs_request_stream`] 结构性保证——超限即 Err，不静默截断）。
pub fn synth_obs_request(batch: u64, now_ns: u64) -> Result<Vec<u8>, ObsBatchError> {
    let flat: Vec<u8> = synth_frames(batch, now_ns).concat();
    encode_obs_request_stream(SYNTH_SOURCE_ID, &flat)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// 每帧必过 decode 门（epoch=0⇒TEST、单位在册、finite）。
    #[test]
    fn all_feed_frames_pass_decode_gate() {
        for (batch, parity) in [(0u64, 0.475), (1, 0.525), (2, 0.475)] {
            let frames = synth_frames(batch, 1_000_000_000);
            assert_eq!(frames.len(), SYNTH_FEED.len());
            for f in &frames {
                let ev = ObservationEvent::decode_v1(f).expect("decode");
                ev.validate().expect("validate");
                assert_eq!(ev.source_id, SYNTH_SOURCE_ID);
                assert_ne!(ev.flags & event_flags::TEST, 0);
            }
            let soc = ObservationEvent::decode_v1(&frames[3]).expect("decode soc");
            match soc.value {
                ObservationValue::F64(v) => assert_eq!(v, parity),
                _ => panic!("SOC 必须是 F64"),
            }
        }
    }

    /// 批间 sequence 严格单调（同 epoch 重放/乱序 = sink 拒绝面）。
    #[test]
    fn sequences_are_strictly_monotonic_across_batches() {
        let mut last = 0u64;
        for batch in 0..4u64 {
            for ev in synth_frames(batch, 1) {
                let e = ObservationEvent::decode_v1(&ev).expect("decode");
                assert!(e.sequence > last, "sequence 必须跨批严格单调");
                last = e.sequence;
            }
        }
    }

    /// 请求可被服务端 decode_obs_request 还原（布局闭环）。
    #[test]
    fn request_roundtrips_through_service_decoder() {
        let req = synth_obs_request(7, 1).expect("encode");
        let (source_id, frames) = crate::obs_batch_wire::decode_obs_request(&req).expect("decode");
        assert_eq!(source_id, SYNTH_SOURCE_ID);
        assert_eq!(frames.len(), SYNTH_FEED.len() * FRAME_WIRE_LEN);
    }
}
