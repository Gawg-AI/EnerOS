//! 严格解码面：canonical payload → [`CatalogBundle`]（fail-closed）。
//!
//! 解码纪律（与 `points` 解码同一代际）：未知字段、重复字段、截断、
//! count 越界 → 错误；逐条记录交叉引用校验（点→资产/schema、控制点
//! 回读、反馈槽 5 槽、拓扑边→节点、孤岛静态可达、单位换算量纲兼容、
//! 换算因子有限正数、资产 kind/节点 kind/边 kind/合同 class/约束 kind
//! 闭枚举）——任何歧义拒绝装载，不静默默认。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_config_bundle::payload::{decode_fields, field};
use eneros_config_bundle::schema::catalog;
pub use eneros_config_bundle::schema::catalog::{
    ASSET_FLAG_TOMBSTONE, CONTRACT_CLASS_SETPOINT, CONTRACT_CLASS_SWITCH, FEEDBACK_SLOT_DC_BUS_V,
    FEEDBACK_SLOT_GRID_UP, FEEDBACK_SLOT_OVERCURRENT, FEEDBACK_SLOT_OVERTEMP, FEEDBACK_SLOT_SOC,
    ROLE_CONTROL, ROLE_MEASUREMENT, ROLE_SWITCH_STATE, SAFETY_KIND_OVERCURRENT,
    SAFETY_KIND_OVERTEMP,
};
use eneros_config_bundle::schema::catalog_kinds;
use eneros_telemetry_model::unit_catalog;
use eneros_telemetry_model::PhysicalUnit;

use crate::model::*;
use crate::{CatalogBundle, CatalogError, CatalogErrorKind, VersionSet};

/// 解码 `catalog` 配置 payload（svc-config ConfigGet("catalog") 响应体）。
pub fn decode_catalog(payload: &[u8]) -> Result<CatalogBundle, CatalogError> {
    if !crate::sanity_len(payload.len()) {
        return Err(CatalogError::new(
            CatalogErrorKind::Decode,
            alloc::format!("payload-too-large:{}", payload.len()),
        ));
    }
    let cfg = eneros_config_bundle::ConfigId::from_static(catalog::CONFIG_ID);
    let fields = decode_fields(payload, &cfg, catalog::MAX_FIELD)?;

    // ---- 头部（必填）----
    let hdr = field(&fields, catalog::FIELD_HEADER)
        .ok_or_else(|| CatalogError::new(CatalogErrorKind::Decode, "missing-header".into()))?;
    if hdr.len() != catalog::HEADER_LEN {
        return Err(CatalogError::new(
            CatalogErrorKind::Decode,
            alloc::format!("header-len:{}", hdr.len()),
        ));
    }
    let mut pos = 0usize;
    let next_u64 = |pos: &mut usize| {
        let v = u64::from_le_bytes(hdr[*pos..*pos + 8].try_into().unwrap());
        *pos += 8;
        v
    };
    let generation = next_u64(&mut pos);
    let valid_from_ns = next_u64(&mut pos);
    let valid_until_ns = next_u64(&mut pos);
    let mapping_version = next_u64(&mut pos);
    let topology_version = next_u64(&mut pos);
    let asset_version = next_u64(&mut pos);
    let contract_version = next_u64(&mut pos);
    let safety_version = next_u64(&mut pos);
    let header = CatalogHeader {
        versions: VersionSet {
            generation,
            mapping_version,
            topology_version,
            asset_version,
            contract_version,
            safety_version,
        },
        valid_from_ns,
        valid_until_ns,
    };

    let mut out = CatalogBundle {
        header: Some(header),
        ..CatalogBundle::default()
    };

    // ---- schema 注册表 ----
    if let Some(v) = field(&fields, catalog::FIELD_SCHEMAS) {
        let mut r = TableReader::new(v, catalog::FIELD_SCHEMAS, catalog::SCHEMA_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::SCHEMA_REC_LEN {
                return Err(r.bad_len());
            }
            let id = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            if id == 0 {
                return Err(r.bad_value("schema-zero-id"));
            }
            if out.schemas.iter().any(|s: &SchemaEntry| s.id == id) {
                return Err(r.bad_value("schema-duplicate"));
            }
            out.schemas.push(SchemaEntry {
                id,
                version: u16::from_le_bytes(rec[4..6].try_into().unwrap()),
                flags: u16::from_le_bytes(rec[6..8].try_into().unwrap()),
            });
        }
    }

    // ---- 单位换算图 ----
    if let Some(v) = field(&fields, catalog::FIELD_UNIT_CONVERSIONS) {
        let mut r = TableReader::new(
            v,
            catalog::FIELD_UNIT_CONVERSIONS,
            catalog::UNIT_CONV_REC_LEN,
        )?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::UNIT_CONV_REC_LEN {
                return Err(r.bad_len());
            }
            let from = u16::from_le_bytes(rec[0..2].try_into().unwrap());
            let to = u16::from_le_bytes(rec[2..4].try_into().unwrap());
            let factor = f64::from_le_bytes(rec[4..12].try_into().unwrap());
            let from_u = unit_catalog::from_code(from).ok_or_else(|| {
                CatalogError::new(CatalogErrorKind::UnknownUnit, alloc::format!("from:{from}"))
            })?;
            let to_u = unit_catalog::from_code(to).ok_or_else(|| {
                CatalogError::new(CatalogErrorKind::UnknownUnit, alloc::format!("to:{to}"))
            })?;
            if from == to || !(factor.is_finite() && factor > 0.0) {
                return Err(CatalogError::new(
                    CatalogErrorKind::Decode,
                    alloc::format!("conversion-invalid:({from},{to},{factor})"),
                ));
            }
            if !from_u.compatible_with(to_u) {
                return Err(CatalogError::new(
                    CatalogErrorKind::NoConversion,
                    alloc::format!(
                        "dimension-mismatch:{:?}->{:?}",
                        from_u.dimension(),
                        to_u.dimension()
                    ),
                ));
            }
            out.conversions.push(UnitConversion { from, to, factor });
        }
    }

    // ---- 资产目录 ----
    if let Some(v) = field(&fields, catalog::FIELD_ASSETS) {
        let mut r = TableReader::new(v, catalog::FIELD_ASSETS, catalog::ASSET_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::ASSET_REC_LEN {
                return Err(r.bad_len());
            }
            let asset_id = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            if asset_id == 0 {
                return Err(r.bad_value("asset-zero-id"));
            }
            if out
                .assets
                .iter()
                .any(|a: &AssetDescriptor| a.asset_id == asset_id)
            {
                return Err(r.bad_value("asset-duplicate"));
            }
            let kind = u16::from_le_bytes(rec[16..18].try_into().unwrap());
            if kind == 0 || kind > 11 {
                return Err(r.bad_value("asset-kind-out-of-range"));
            }
            out.assets.push(AssetDescriptor {
                asset_id,
                site_id: u32::from_le_bytes(rec[8..12].try_into().unwrap()),
                tenant_id: u32::from_le_bytes(rec[12..16].try_into().unwrap()),
                kind,
                device_id: u16::from_le_bytes(rec[18..20].try_into().unwrap()),
                topology_node_id: u64::from_le_bytes(rec[20..28].try_into().unwrap()),
                flags: u16::from_le_bytes(rec[28..30].try_into().unwrap()),
            });
        }
    }

    // ---- 拓扑节点 ----
    if let Some(v) = field(&fields, catalog::FIELD_TOPO_NODES) {
        let mut r = TableReader::new(v, catalog::FIELD_TOPO_NODES, catalog::TOPO_NODE_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::TOPO_NODE_REC_LEN {
                return Err(r.bad_len());
            }
            let id = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            if id == 0 || out.nodes.iter().any(|n: &TopologyNode| n.id == id) {
                return Err(r.bad_value("node-duplicate-or-zero"));
            }
            let kind = u16::from_le_bytes(rec[8..10].try_into().unwrap());
            // 节点 kind 闭枚举（[`catalog_kinds::NODE_*`] 1..=NODE_GRID）。
            if kind == 0 || kind > catalog_kinds::NODE_GRID {
                return Err(r.bad_value("node-kind-out-of-range"));
            }
            out.nodes.push(TopologyNode {
                id,
                kind,
                flags: u16::from_le_bytes(rec[10..12].try_into().unwrap()),
            });
        }
    }

    // ---- 拓扑边 ----
    if let Some(v) = field(&fields, catalog::FIELD_TOPO_EDGES) {
        let mut r = TableReader::new(v, catalog::FIELD_TOPO_EDGES, catalog::TOPO_EDGE_REC_LEN)?;
        let nodes: BTreeMap<u64, TopologyNode> = out.nodes.iter().map(|n| (n.id, *n)).collect();
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::TOPO_EDGE_REC_LEN {
                return Err(r.bad_len());
            }
            let from = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            let to = u64::from_le_bytes(rec[8..16].try_into().unwrap());
            let kind = u16::from_le_bytes(rec[16..18].try_into().unwrap());
            // 边 kind 闭枚举（1=switch 2=branch；未知 kind 不得静默按
            // branch 导通）。
            if kind != catalog::EDGE_KIND_SWITCH && kind != catalog::EDGE_KIND_BRANCH {
                return Err(r.bad_value("edge-kind-out-of-range"));
            }
            let switch_point_id = u32::from_le_bytes(rec[18..22].try_into().unwrap());
            let capacity_kw = f64::from_le_bytes(rec[22..30].try_into().unwrap());
            let loss = f64::from_le_bytes(rec[30..38].try_into().unwrap());
            let valid_from_ns = u64::from_le_bytes(rec[38..46].try_into().unwrap());
            let valid_until_ns = u64::from_le_bytes(rec[46..54].try_into().unwrap());
            if from == 0 || to == 0 || from == to {
                return Err(r.bad_value("edge-node-invalid"));
            }
            if !nodes.contains_key(&from) || !nodes.contains_key(&to) {
                return Err(r.bad_value("edge-dangling-node"));
            }
            if !(capacity_kw.is_finite() && capacity_kw >= 0.0 && loss.is_finite() && loss >= 0.0) {
                return Err(r.bad_value("edge-non-finite"));
            }
            out.edges.push(TopologyEdge {
                from,
                to,
                kind,
                switch_point_id,
                capacity_kw,
                loss,
                valid_from_ns,
                valid_until_ns,
                flags: u16::from_le_bytes(rec[54..56].try_into().unwrap()),
            });
        }
        // 编译期弱检查（可能是不同编译代际）也在此复验：switch 边必须带
        // 观测点；自环已被上面拒绝。
        for e in out.edges.iter() {
            if e.kind == 1 && e.switch_point_id == 0 {
                return Err(CatalogError::new(
                    CatalogErrorKind::InvalidTopology,
                    "switch-edge-without-point".into(),
                ));
            }
        }
    }

    // ---- 点语义目录 ----
    if let Some(v) = field(&fields, catalog::FIELD_POINT_SEMANTICS) {
        let mut r = TableReader::new(
            v,
            catalog::FIELD_POINT_SEMANTICS,
            catalog::POINT_SEM_REC_LEN,
        )?;
        let assets: BTreeMap<u64, ()> = out.assets.iter().map(|a| (a.asset_id, ())).collect();
        let schemas: BTreeMap<u32, ()> = out.schemas.iter().map(|s| (s.id, ())).collect();
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::POINT_SEM_REC_LEN {
                return Err(r.bad_len());
            }
            let point_id = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            let asset_id = u64::from_le_bytes(rec[4..12].try_into().unwrap());
            let schema_id = u32::from_le_bytes(rec[12..16].try_into().unwrap());
            let schema_version = u16::from_le_bytes(rec[16..18].try_into().unwrap());
            let unit_code = u16::from_le_bytes(rec[18..20].try_into().unwrap());
            let role = rec[20];
            let readback_point_id = u32::from_le_bytes(rec[21..25].try_into().unwrap());
            let flags = rec[25];
            if point_id == 0 {
                return Err(r.bad_value("point-zero-id"));
            }
            if out
                .points
                .iter()
                .any(|p: &PointSemantics| p.point_id == point_id)
            {
                return Err(r.bad_value("point-duplicate"));
            }
            if !assets.contains_key(&asset_id) {
                return Err(r.bad_value("point-unknown-asset"));
            }
            if schema_id != 0 && !schemas.contains_key(&schema_id) {
                return Err(r.bad_value("point-unknown-schema"));
            }
            if unit_code == 0 || unit_catalog::from_code(unit_code).is_none() {
                return Err(CatalogError::new(
                    CatalogErrorKind::UnknownUnit,
                    alloc::format!("point {point_id} unit {unit_code}"),
                ));
            }
            let role_ok = matches!(role, ROLE_MEASUREMENT | ROLE_CONTROL | ROLE_SWITCH_STATE);
            if !role_ok {
                return Err(r.bad_value("point-unknown-role"));
            }
            if role == ROLE_CONTROL && readback_point_id == 0 {
                return Err(r.bad_value("control-without-readback"));
            }
            out.points.push(PointSemantics {
                point_id,
                asset_id,
                schema_id,
                schema_version,
                unit_code,
                role,
                readback_point_id,
                flags,
            });
        }
    }

    // ---- 反馈槽（5 槽码齐全；点必须存在） ----
    if let Some(v) = field(&fields, catalog::FIELD_FEEDBACK_SLOTS) {
        let mut r = TableReader::new(
            v,
            catalog::FIELD_FEEDBACK_SLOTS,
            catalog::FEEDBACK_SLOT_REC_LEN,
        )?;
        let mut slots: BTreeMap<u8, u32> = BTreeMap::new();
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::FEEDBACK_SLOT_REC_LEN {
                return Err(r.bad_len());
            }
            let slot = rec[0];
            let point_id = u32::from_le_bytes(rec[1..5].try_into().unwrap());
            if !matches!(
                slot,
                FEEDBACK_SLOT_GRID_UP
                    | FEEDBACK_SLOT_SOC
                    | FEEDBACK_SLOT_DC_BUS_V
                    | FEEDBACK_SLOT_OVERCURRENT
                    | FEEDBACK_SLOT_OVERTEMP
            ) || slots.insert(slot, point_id).is_some()
            {
                return Err(r.bad_value("slot-invalid-or-duplicate"));
            }
        }
        if slots.len() != catalog::FEEDBACK_SLOT_COUNT as usize {
            return Err(r.bad_value("slot-count-not-five"));
        }
        for (_, pid) in slots.iter() {
            if !out.points.iter().any(|p| p.point_id == *pid) {
                return Err(r.bad_value("slot-point-not-in-catalog"));
            }
        }
        out.feedback = slots.into_iter().collect();
    }

    // ---- 安全覆盖 ----
    if let Some(v) = field(&fields, catalog::FIELD_COVERAGE) {
        let mut r = TableReader::new(v, catalog::FIELD_COVERAGE, catalog::COVERAGE_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::COVERAGE_REC_LEN {
                return Err(r.bad_len());
            }
            let point_id = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            let min = f64::from_le_bytes(rec[4..12].try_into().unwrap());
            let max = f64::from_le_bytes(rec[12..20].try_into().unwrap());
            let period_ms = u32::from_le_bytes(rec[20..24].try_into().unwrap());
            let unit_code = u16::from_le_bytes(rec[24..26].try_into().unwrap());
            if !out.points.iter().any(|p| p.point_id == point_id) {
                return Err(r.bad_value("coverage-point-not-in-catalog"));
            }
            if !(min.is_finite() && max.is_finite()) || min > max {
                return Err(r.bad_value("coverage-range-invalid"));
            }
            if unit_catalog::from_code(unit_code).is_none() {
                return Err(r.bad_value("coverage-unit-unknown"));
            }
            out.coverage.push(CoverageEntry {
                point_id,
                min,
                max,
                period_ms,
                unit_code,
                flags: u16::from_le_bytes(rec[26..28].try_into().unwrap()),
            });
        }
    }

    // ---- 控制合同 ----
    if let Some(v) = field(&fields, catalog::FIELD_CONTROL_CONTRACTS) {
        let mut r = TableReader::new(
            v,
            catalog::FIELD_CONTROL_CONTRACTS,
            catalog::CONTROL_CONTRACT_REC_LEN,
        )?;
        let assets: BTreeMap<u64, ()> = out.assets.iter().map(|a| (a.asset_id, ())).collect();
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::CONTROL_CONTRACT_REC_LEN {
                return Err(r.bad_len());
            }
            let contract_id = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            let asset_id = u64::from_le_bytes(rec[4..12].try_into().unwrap());
            let point_id = u32::from_le_bytes(rec[12..16].try_into().unwrap());
            let unit_code = u16::from_le_bytes(rec[16..18].try_into().unwrap());
            let class = u16::from_le_bytes(rec[18..20].try_into().unwrap());
            let min = f64::from_le_bytes(rec[20..28].try_into().unwrap());
            let max = f64::from_le_bytes(rec[28..36].try_into().unwrap());
            let ramp_kw_per_s = f64::from_le_bytes(rec[36..44].try_into().unwrap());
            let settle_ms = u32::from_le_bytes(rec[44..48].try_into().unwrap());
            let min_on_ms = u64::from_le_bytes(rec[48..56].try_into().unwrap());
            let min_off_ms = u64::from_le_bytes(rec[56..64].try_into().unwrap());
            let max_starts = u16::from_le_bytes(rec[64..66].try_into().unwrap());
            let readback_point_id = u32::from_le_bytes(rec[66..70].try_into().unwrap());
            let tolerance_kw = f64::from_le_bytes(rec[70..78].try_into().unwrap());
            let reversible = rec[78];
            let compensation = rec[79];
            let priority = rec[80];
            let safe_state = rec[81];
            let lost_link = rec[82];
            if contract_id == 0
                || out
                    .contracts
                    .iter()
                    .any(|c: &ControlContract| c.contract_id == contract_id)
            {
                return Err(r.bad_value("contract-duplicate-or-zero"));
            }
            // class 闭枚举（1=switch 2=setpoint）。
            if !matches!(class, CONTRACT_CLASS_SWITCH | CONTRACT_CLASS_SETPOINT) {
                return Err(r.bad_value("contract-class-out-of-range"));
            }
            if !assets.contains_key(&asset_id) {
                return Err(r.bad_value("contract-unknown-asset"));
            }
            if !out.points.iter().any(|p| p.point_id == point_id) {
                return Err(r.bad_value("contract-point-not-in-catalog"));
            }
            if unit_catalog::from_code(unit_code).is_none() {
                return Err(r.bad_value("contract-unit-unknown"));
            }
            if !(min.is_finite() && max.is_finite()) || min > max {
                return Err(r.bad_value("contract-range-invalid"));
            }
            if !(ramp_kw_per_s.is_finite()
                && ramp_kw_per_s >= 0.0
                && tolerance_kw.is_finite()
                && tolerance_kw >= 0.0)
            {
                return Err(r.bad_value("contract-non-finite"));
            }
            if readback_point_id == 0 {
                return Err(r.bad_value("contract-without-readback"));
            }
            out.contracts.push(ControlContract {
                contract_id,
                asset_id,
                point_id,
                unit_code,
                class,
                min,
                max,
                ramp_kw_per_s,
                settle_ms,
                min_on_ms,
                min_off_ms,
                max_starts,
                readback_point_id,
                tolerance_kw,
                reversible,
                compensation,
                priority,
                safe_state,
                lost_link,
                flags: rec[83],
            });
        }
    }

    // ---- 安全约束 ----
    if let Some(v) = field(&fields, catalog::FIELD_SAFETY_CONSTRAINTS) {
        let mut r = TableReader::new(
            v,
            catalog::FIELD_SAFETY_CONSTRAINTS,
            catalog::SAFETY_CONSTRAINT_REC_LEN,
        )?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::SAFETY_CONSTRAINT_REC_LEN {
                return Err(r.bad_len());
            }
            let constraint_id = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            let asset_id = u64::from_le_bytes(rec[4..12].try_into().unwrap());
            let kind = u16::from_le_bytes(rec[12..14].try_into().unwrap());
            let point_id = u32::from_le_bytes(rec[14..18].try_into().unwrap());
            let limit = f64::from_le_bytes(rec[18..26].try_into().unwrap());
            // id 唯一非零（与控制合同同纪律）；kind 闭枚举。
            if constraint_id == 0
                || out
                    .safety
                    .iter()
                    .any(|s: &SafetyConstraint| s.constraint_id == constraint_id)
            {
                return Err(r.bad_value("constraint-duplicate-or-zero"));
            }
            if !matches!(kind, SAFETY_KIND_OVERCURRENT | SAFETY_KIND_OVERTEMP) {
                return Err(r.bad_value("safety-kind-out-of-range"));
            }
            if !limit.is_finite() {
                return Err(r.bad_value("constraint-limit-non-finite"));
            }
            out.safety.push(SafetyConstraint {
                constraint_id,
                asset_id,
                kind,
                point_id,
                limit,
                flags: u16::from_le_bytes(rec[26..28].try_into().unwrap()),
            });
        }
    }

    // ---- 别名 ----
    if let Some(v) = field(&fields, catalog::FIELD_ALIASES) {
        let mut r = TableReader::new(v, catalog::FIELD_ALIASES, catalog::ALIAS_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::ALIAS_REC_LEN {
                return Err(r.bad_len());
            }
            let kind = rec[0];
            let legacy_id = u64::from_le_bytes(rec[1..9].try_into().unwrap());
            let asset_id = u64::from_le_bytes(rec[9..17].try_into().unwrap());
            out.aliases.push(AliasEntry {
                kind,
                legacy_id,
                asset_id,
            });
        }
    }

    // ---- tombstone ----
    if let Some(v) = field(&fields, catalog::FIELD_TOMBSTONES) {
        let mut r = TableReader::new(v, catalog::FIELD_TOMBSTONES, catalog::TOMBSTONE_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::TOMBSTONE_REC_LEN {
                return Err(r.bad_len());
            }
            let asset_id = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            if asset_id == 0 || out.tombstones.contains(&asset_id) {
                return Err(r.bad_value("tombstone-zero-or-duplicate"));
            }
            // 活资产与墓碑互斥。
            if out.assets.iter().any(|a| a.asset_id == asset_id) {
                return Err(r.bad_value("tombstone-asset-alive"));
            }
            out.tombstones.push(asset_id);
        }
    }

    // ---- 物理参数铭牌（蓝图 70.8 WP-70.8.3）----
    if let Some(v) = field(&fields, catalog::FIELD_NAMEPLATES) {
        let mut r = TableReader::new(v, catalog::FIELD_NAMEPLATES, catalog::NAMEPLATE_REC_LEN)?;
        while let Some(rec) = r.next()? {
            if rec.len() != catalog::NAMEPLATE_REC_LEN {
                return Err(r.bad_len());
            }
            let f =
                |i: usize| -> f64 { f64::from_le_bytes(rec[i * 8..i * 8 + 8].try_into().unwrap()) };
            let asset_id = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            let np = Nameplate {
                asset_id,
                capacity_kwh: f(1),
                eta_c: f(2),
                eta_d: f(3),
                energy_max_kwh: f(4),
                p_min_kw: f(5),
                p_max_kw: f(6),
                q_min_kvar: f(7),
                q_max_kvar: f(8),
                ramp_up_kw_s: f(9),
                ramp_down_kw_s: f(10),
                soc_min: f(11),
                soc_max: f(12),
            };
            if asset_id == 0 {
                return Err(r.bad_value("nameplate-zero-asset"));
            }
            if out.nameplates.iter().any(|n| n.asset_id == asset_id) {
                return Err(r.bad_value("nameplate-duplicate"));
            }
            if !out.assets.iter().any(|a| a.asset_id == asset_id) {
                return Err(r.bad_value("nameplate-unknown-asset"));
            }
            if out.tombstones.contains(&asset_id) {
                return Err(r.bad_value("nameplate-asset-tombstoned"));
            }
            // 物理参数必须有限且自洽（不猜、不默认）。
            let base = [np.capacity_kwh, np.eta_c, np.eta_d, np.energy_max_kwh];
            if base.iter().any(|v| !v.is_finite())
                || np.eta_c <= 0.0
                || np.eta_c > 1.0
                || np.eta_d <= 0.0
                || np.eta_d > 1.0
                || np.capacity_kwh <= 0.0
                || np.energy_max_kwh <= 0.0
            {
                return Err(r.bad_value("nameplate-invalid-energy-params"));
            }
            // 可用 SOC 窗（0..1 分数；0 ≤ min < max ≤ 1）——量程 coverage
            // 不是运行安全窗，故本窗必须由铭牌显式给出。
            if !np.soc_min.is_finite()
                || !np.soc_max.is_finite()
                || np.soc_min < 0.0
                || np.soc_max > 1.0
                || np.soc_min >= np.soc_max
            {
                return Err(r.bad_value("nameplate-invalid-soc-window"));
            }
            if !np.p_min_kw.is_finite()
                || !np.p_max_kw.is_finite()
                || !np.q_min_kvar.is_finite()
                || !np.q_max_kvar.is_finite()
                || !np.ramp_up_kw_s.is_finite()
                || !np.ramp_down_kw_s.is_finite()
                || np.p_min_kw > np.p_max_kw
                || np.q_min_kvar > np.q_max_kvar
                || np.ramp_up_kw_s < 0.0
                || np.ramp_down_kw_s < 0.0
            {
                return Err(r.bad_value("nameplate-invalid-power-params"));
            }
            out.nameplates.push(np);
        }
    }

    // ---- 静态可达性（孤岛规则）----
    validate_reachability(&out)?;

    Ok(out)
}

/// 拓扑静态可达性：每个节点必须能从 grid 节点（kind=10）经边双向往返
/// 到达；无 grid 节点 → 拒绝。开关状态导致的动态孤岛由
/// `topology::energized_view` 处理（此处只管结构）。
fn validate_reachability(out: &CatalogBundle) -> Result<(), CatalogError> {
    let grid: Vec<u64> = out
        .nodes
        .iter()
        .filter(|n| n.kind == eneros_config_bundle::schema::catalog_kinds::NODE_GRID)
        .map(|n| n.id)
        .collect();
    if grid.is_empty() {
        return Err(CatalogError::new(
            CatalogErrorKind::InvalidTopology,
            "no-grid-node".into(),
        ));
    }
    let mut adj: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for e in out.edges.iter() {
        adj.entry(e.from).or_default().push(e.to);
        adj.entry(e.to).or_default().push(e.from);
    }
    let mut reachable: BTreeMap<u64, ()> = BTreeMap::new();
    let mut stack: Vec<u64> = grid.clone();
    while let Some(n) = stack.pop() {
        if reachable.contains_key(&n) {
            continue;
        }
        reachable.insert(n, ());
        if let Some(ns) = adj.get(&n) {
            for m in ns {
                if !reachable.contains_key(m) {
                    stack.push(*m);
                }
            }
        }
    }
    for n in out.nodes.iter() {
        if !reachable.contains_key(&n.id) {
            return Err(CatalogError::new(
                CatalogErrorKind::InvalidTopology,
                alloc::format!("island-node:{}", n.id),
            ));
        }
    }
    Ok(())
}

/// 表格子格式读取器：`count u32 ‖ 记录 × n`；记录定长。
struct TableReader<'a> {
    pos: usize,
    buf: &'a [u8],
    field_id: u8,
    rec_len: usize,
    remaining: u32,
}

impl<'a> TableReader<'a> {
    fn new(buf: &'a [u8], field_id: u8, rec_len: usize) -> Result<Self, CatalogError> {
        if buf.len() < 4 {
            return Err(CatalogError::new(
                CatalogErrorKind::Decode,
                alloc::format!("field-{field_id}:truncated-count"),
            ));
        }
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let body = buf.len() - 4;
        let recs = body / rec_len;
        if recs < count as usize || body % rec_len != 0 {
            return Err(CatalogError::new(
                CatalogErrorKind::Decode,
                alloc::format!(
                    "field-{field_id}:len-mismatch(count={count},body={body},rec={rec_len})"
                ),
            ));
        }
        // 防御性上限（封闭 payload 无界拒绝）。
        let max = max_records(field_id);
        if count > max {
            return Err(CatalogError::new(
                CatalogErrorKind::Decode,
                alloc::format!("field-{field_id}:too-many({count} > {max})"),
            ));
        }
        Ok(Self {
            pos: 4,
            buf,
            field_id,
            rec_len,
            remaining: count,
        })
    }

    fn next(&mut self) -> Result<Option<&'a [u8]>, CatalogError> {
        if self.remaining == 0 {
            // 尾随字节拒绝（count 与实际记录数不符——完整性规则，与
            // points 解码面 trailing-bytes 同纪律）。
            if self.pos != self.buf.len() {
                return Err(self.bad_value("trailing-bytes"));
            }
            return Ok(None);
        }
        let rec = &self.buf[self.pos..self.pos + self.rec_len];
        self.pos += self.rec_len;
        self.remaining -= 1;
        Ok(Some(rec))
    }

    fn bad_len(&self) -> CatalogError {
        CatalogError::new(
            CatalogErrorKind::Decode,
            alloc::format!("field-{}:record-len", self.field_id),
        )
    }

    fn bad_value(&self, what: &str) -> CatalogError {
        CatalogError::new(
            CatalogErrorKind::Decode,
            alloc::format!("field-{}:{what}", self.field_id),
        )
    }
}

fn max_records(field_id: u8) -> u32 {
    match field_id {
        catalog::FIELD_SCHEMAS => catalog::MAX_SCHEMAS,
        catalog::FIELD_UNIT_CONVERSIONS => catalog::MAX_UNIT_CONVERSIONS,
        catalog::FIELD_ASSETS => catalog::MAX_ASSETS,
        catalog::FIELD_TOPO_NODES => catalog::MAX_TOPO_NODES,
        catalog::FIELD_TOPO_EDGES => catalog::MAX_TOPO_EDGES,
        catalog::FIELD_POINT_SEMANTICS => catalog::MAX_POINT_SEMANTICS,
        catalog::FIELD_FEEDBACK_SLOTS => catalog::MAX_FEEDBACK_SLOTS,
        catalog::FIELD_COVERAGE => catalog::MAX_COVERAGE,
        catalog::FIELD_CONTROL_CONTRACTS => catalog::MAX_CONTROL_CONTRACTS,
        catalog::FIELD_SAFETY_CONSTRAINTS => catalog::MAX_SAFETY_CONSTRAINTS,
        catalog::FIELD_ALIASES => catalog::MAX_ALIASES,
        catalog::FIELD_TOMBSTONES => catalog::MAX_TOMBSTONES,
        catalog::FIELD_NAMEPLATES => catalog::MAX_NAMEPLATES,
        _ => u32::MAX,
    }
}

/// 单位名 → 封闭码（解码面复验用；与编译面 `unit_code_from_name` 同表）。
/// 供 host 测试与工具使用（target 由编译产物直读码值）。
#[allow(dead_code)]
pub(crate) fn unit_code_of_name(name: &str) -> Option<u16> {
    if matches!(name, "ratio" | "dimensionless") {
        return Some(unit_catalog::code_of(PhysicalUnit::Dimensionless));
    }
    const ALL: [PhysicalUnit; 18] = [
        PhysicalUnit::Volts,
        PhysicalUnit::Kilovolts,
        PhysicalUnit::Amperes,
        PhysicalUnit::Kiloamperes,
        PhysicalUnit::Watts,
        PhysicalUnit::Kilowatts,
        PhysicalUnit::Megawatts,
        PhysicalUnit::VoltAmpereReactive,
        PhysicalUnit::Kilovar,
        PhysicalUnit::Megavar,
        PhysicalUnit::Hertz,
        PhysicalUnit::Celsius,
        PhysicalUnit::Percent,
        PhysicalUnit::KilowattHour,
        PhysicalUnit::AmpereHour,
        PhysicalUnit::Seconds,
        PhysicalUnit::Rpm,
        PhysicalUnit::Dimensionless,
    ];
    ALL.iter()
        .find(|u| u.short_name() == name)
        .map(|&u| unit_catalog::code_of(u))
}

#[cfg(test)]
mod tests {
    use super::decode_catalog;

    /// 用编译面（config-bundle `compile` dev-dep）编码合法目录 → 解码往返。
    /// 生产 `configs/catalog.toml` 的装载门禁（先编译后解码，两侧布局一致）。
    #[test]
    fn decode_compiled_production_catalog() {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(toml).unwrap();
        let bundle = decode_catalog(&compiled.payload).unwrap();
        let hdr = bundle.header.unwrap();
        assert_eq!(hdr.versions.generation, 1);
        assert_eq!(bundle.assets.len(), 2);
        // 2026-09-14：PCS-1 的功率回读/遥测点进目录（+3 语义）；覆盖点数
        // +1（合同回读点 1001 必须进冻结快照的必收点集）。
        assert_eq!(bundle.points.len(), 12);
        assert_eq!(bundle.feedback.len(), 5);
        assert_eq!(bundle.coverage.len(), 8);
        assert_eq!(bundle.contracts.len(), 1);
    }
}
