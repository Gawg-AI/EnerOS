//! 蓝图 61 v0.314.33 —— SemanticCatalog 生产装载窄口（control-services）。
//!
//! svc-telemetry / svc-control-dispatch / svc-device-agent 共用：经
//! svc-config `ConfigGet("catalog")` → `semantic-catalog::decode` →
//! 目录仓库；再由仓库派生生产需要的**映射事实**：
//!
//! - [`coverage_config`]：安全覆盖（替代手写 SAFETY_COVERAGE_POINTS 装配）；
//! - [`feedback_mapping`]：反馈槽位（替代手写 PROD_FEEDBACK_MAPPING 装配）；
//! - [`control_contract_point`]：显式控制绑定点（替代「首个 Control 点」）。
//!
//! 目录不可得 → 敷生 fail-closed（None）：装配方不得自造语义。判定行
//! 词形（E4 grep 锚点）：`CATALOG-ACTIVE gen=<n> assets=<k> digest=<..>` /
//! `CATALOG-COVERAGE points=<n>` / `CATALOG-FEEDBACK grid=<g> soc=<s> v=<v>
//! oc=<o> ot=<t>` / `CATALOG-BINDING point=<p> device=<d>`。

use alloc::vec::Vec;

use eneros_control_dispatch::{FeedbackPointMapping, PROD_FEEDBACK_MAPPING};
use eneros_safety_validator::{PointUnit, RequiredPoint, RequiredPointCoverage};
use eneros_semantic_catalog::{decode::ROLE_CONTROL, CatalogRepository, SemanticCatalog};

/// 装载结果（generation = 目录头版本组 generation；digest = 目录摘要）。
#[derive(Debug, Clone)]
pub struct LoadedCatalog {
    pub repo: CatalogRepository,
    pub generation: u64,
}

/// svc-config `ConfigGet("catalog")` → 解码（失败 → None；词形由调用方）。
///
/// 62 v0.315.0 WP-62.1.4：传输面泛型化到 [`eneros_agent_sdk::config_gateway::
/// ConfigEntrySource`]（target = `Sel4IpcClient`，host 测试 = 内存假件——
/// 装配拉取计数与同代断言可测）；wire/解码/词形零变更（config_get 单源
/// 不动）。cfg 放宽到 `any(test, feature = "sel4")` 与 agent-sdk 同口径。
#[cfg(any(test, feature = "sel4"))]
pub fn fetch_catalog<T: eneros_agent_sdk::config_gateway::ConfigEntrySource>(
    config: &mut T,
) -> Option<LoadedCatalog> {
    use eneros_agent_sdk::config_gateway::{config_get, ConfigStep};
    let got =
        config_get(
            config,
            "catalog",
            |body| match eneros_semantic_catalog::decode::decode_catalog(body)
                .and_then(eneros_semantic_catalog::CatalogRepository::build)
            {
                Ok(repo) => Ok((repo.version_set().generation, repo)),
                Err(e) => {
                    eneros_runtime::println!("[catalog-gateway] catalog decode FAIL: {e}");
                    Err(ConfigStep::DecodeFailed)
                }
            },
        );
    match got {
        Ok((generation, repo)) => Some(LoadedCatalog { repo, generation }),
        Err(ConfigStep::Unavailable) => {
            eneros_runtime::println!("[catalog-gateway] ConfigGet(catalog) EIPC fail: Transport");
            None
        }
        Err(ConfigStep::TooShort) => {
            eneros_runtime::println!("[catalog-gateway] catalog response malformed: TooShort");
            None
        }
        Err(ConfigStep::BadPrefix) => {
            eneros_runtime::println!("[catalog-gateway] catalog response malformed: LengthPrefix");
            None
        }
        Err(ConfigStep::DecodeFailed) => None,
    }
}

/// 目录安全覆盖 → 评估器覆盖声明（unit 码映射到 PointUnit；未知单位 →
/// 该点跳过——覆盖不齐时评估器 fail-closed 拒绝）。
pub fn coverage_config(repo: &CatalogRepository) -> RequiredPointCoverage {
    let mut points: Vec<RequiredPoint> = Vec::new();
    for c in repo.coverage() {
        let Some(unit) = point_unit_of(c.unit_code) else {
            continue;
        };
        points.push(RequiredPoint {
            point: c.point_id,
            min: c.min,
            max: c.max,
            max_age_ms: u64::from(c.period_ms),
            unit,
        });
    }
    RequiredPointCoverage { points }
}

/// UnitCatalog 码 → 安全验证器 PointUnit（量纲等价映射；未知 → None）。
fn point_unit_of(unit_code: u16) -> Option<PointUnit> {
    use eneros_telemetry_model::unit_catalog;
    use eneros_telemetry_model::PhysicalUnit;
    let u = unit_catalog::from_code(unit_code)?;
    let pu = match u {
        PhysicalUnit::Volts | PhysicalUnit::Kilovolts => PointUnit::Volt,
        PhysicalUnit::Amperes | PhysicalUnit::Kiloamperes => PointUnit::Ampere,
        PhysicalUnit::Hertz => PointUnit::Hertz,
        PhysicalUnit::Percent | PhysicalUnit::Dimensionless => PointUnit::Ratio,
        PhysicalUnit::Kilowatts | PhysicalUnit::Megawatts | PhysicalUnit::Watts => {
            PointUnit::Kilowatt
        }
        _ => PointUnit::Other,
    };
    Some(pu)
}

/// 目录反馈槽 → 生产反馈映射（5 槽不齐 → None，fail-closed）。
pub fn feedback_mapping(repo: &CatalogRepository) -> Option<FeedbackPointMapping> {
    let slots = repo.feedback_slots()?;
    Some(FeedbackPointMapping {
        grid_up: slots.grid_up,
        soc: slots.soc,
        dc_bus_v: slots.dc_bus_v,
        overcurrent: slots.overcurrent,
        overtemp: slots.overtemp,
    })
}

/// 显式控制绑定点：目录中 role=control 且带控制合同的点**恰好一个**时
/// 返回该点（多于一个 = 装配歧义 → None fail-closed；零 → None）。
/// 设备短 ID 属协议映射（points.toml），由装配方从点表解析。
pub fn control_contract_point(repo: &CatalogRepository) -> Option<u32> {
    let mut found: Option<u32> = None;
    let vs = repo.version_set();
    for (point_id, sem) in repo.points_with_role(ROLE_CONTROL) {
        let asset_id = repo.asset_of_point(point_id)?;
        if repo
            .asset_control_contract(asset_id, vs.contract_version)
            .is_none()
        {
            continue;
        }
        let _ = sem;
        if found.is_some() {
            // 多控制点 = 装配歧义（本版本单资产生产；明确拒绝而非挑首个）。
            return None;
        }
        found = Some(point_id);
    }
    found
}

/// 生产回退：目录未装载时的手写映射（仅测试/旧 bundle 轨；生产路径
/// 一律经 [`feedback_mapping`]——退出 gate：生产无手写反馈点装配）。
pub fn prod_feedback_mapping_fallback() -> FeedbackPointMapping {
    PROD_FEEDBACK_MAPPING
}

#[cfg(test)]
mod tests {
    use eneros_semantic_catalog::CatalogSet;

    use super::*;

    /// 用编译面（dev-dep `compile` feature）装载生产 catalog.toml。
    fn prod_repo() -> CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled =
            eneros_config_bundle::toml_compile::compile_catalog(toml).expect("compile catalog");
        let bundle = eneros_semantic_catalog::decode::decode_catalog(&compiled.payload).unwrap();
        CatalogRepository::build(bundle).unwrap()
    }

    #[test]
    fn coverage_matches_safety_coverage_points() {
        use eneros_control_dispatch::feedback::SAFETY_COVERAGE_POINTS;
        let repo = prod_repo();
        let cov = coverage_config(&repo);
        assert_eq!(cov.points.len(), 7);
        for (i, req) in cov.points.iter().enumerate() {
            let (point, min, max, max_age, unit) = SAFETY_COVERAGE_POINTS[i];
            assert_eq!(req.point, point, "覆盖点序与单源表一致（i={i}）");
            assert_eq!(req.min, min);
            assert_eq!(req.max, max);
            assert_eq!(req.max_age_ms, max_age);
            // 单位映射等价：V/A/Hz/Ratio 直映；SAFETY 的 Other(5-7 告警
            // 0/1) == 目录 ratio（无量纲）——范围与语义等价。
            let mapped = point_unit_of(repo.coverage().nth(i).unwrap().unit_code).unwrap();
            if unit != PointUnit::Other {
                assert_eq!(mapped, unit);
            } else {
                assert_eq!(mapped, PointUnit::Ratio);
            }
        }
    }

    #[test]
    fn feedback_matches_prod_mapping() {
        let repo = prod_repo();
        let fb = feedback_mapping(&repo).unwrap();
        let prod = PROD_FEEDBACK_MAPPING;
        assert_eq!(fb.grid_up, prod.grid_up);
        assert_eq!(fb.soc, prod.soc);
        assert_eq!(fb.dc_bus_v, prod.dc_bus_v);
        assert_eq!(fb.overcurrent, prod.overcurrent);
        assert_eq!(fb.overtemp, prod.overtemp);
        assert_eq!(fb.grid_up, 5);
        assert_eq!(fb.soc, 4);
        assert_eq!(fb.dc_bus_v, 1);
        assert_eq!(fb.overcurrent, 6);
        assert_eq!(fb.overtemp, 7);
    }

    #[test]
    fn control_binding_point_explicit() {
        let repo = prod_repo();
        // 目录显式控制绑定 = 点 10（唯一 role=control + 合同）。
        assert_eq!(control_contract_point(&repo), Some(10));
    }

    #[test]
    fn catalog_set_activate_and_rollback() {
        let mut set = CatalogSet::new();
        let toml = include_str!("../../../../configs/catalog.toml");
        let v1 = eneros_config_bundle::toml_compile::compile_catalog(toml).expect("compile v1");
        set.activate(&v1.payload).unwrap();
        assert_eq!(set.active().unwrap().version_set().generation, 1);
        // 换代（[08] §2.3 原子切换；generation 严格递增），回滚 → 上一代。
        let v2_toml = toml.replace("generation = 1", "generation = 2");
        let v2 = eneros_config_bundle::toml_compile::compile_catalog(&v2_toml).expect("compile v2");
        set.activate(&v2.payload).unwrap();
        assert_eq!(set.active().unwrap().version_set().generation, 2);
        assert!(set.rollback());
        assert_eq!(set.active().unwrap().version_set().generation, 1);
    }

    /// 蓝图 §2.6：catalog 切换使旧版本组失效（快照/计划绑定侧）。
    #[test]
    fn catalog_switch_invalidates_old_version_set() {
        let mut set = CatalogSet::new();
        let toml = include_str!("../../../../configs/catalog.toml");
        let v1 = eneros_config_bundle::toml_compile::compile_catalog(toml).unwrap();
        set.activate(&v1.payload).unwrap();
        let old_vs = set.active().unwrap().version_set();
        // 新一代（generation+1）。
        let v2_toml = toml.replace("generation = 1", "generation = 2");
        let v2 = eneros_config_bundle::toml_compile::compile_catalog(&v2_toml).unwrap();
        set.activate(&v2.payload).unwrap();
        let active = set.active().unwrap();
        // 旧版本组解析失败（VersionMismatch）；新版本组成功。
        let key = eneros_semantic_catalog::ObservationKey {
            tenant_id: 1,
            site_id: 1,
            source_id: 2,
            device_id: 1,
            point_id: 10,
            mapping_version: 1,
        };
        assert!(active.resolve_observation(&key, old_vs).is_err());
        assert!(active
            .resolve_observation(&key, active.version_set())
            .is_ok());
    }

    /// 点表 ↔ 目录一致：每个 points.toml 的 control-eligible 点（identity
    /// 齐备）都有目录语义；控制点唯一且与 Phase 0 合同一致。
    #[test]
    fn points_table_control_points_have_catalog_semantics() {
        let repo = prod_repo();
        let points_toml = include_str!("../../../../configs/points.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_points(points_toml).unwrap();
        let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
        let table = eneros_config_bundle::points::decode_points(&compiled.payload, &cfg_id)
            .expect("decode points");
        let vs = repo.version_set();
        for rec in table.points.iter() {
            let identity_ok = rec.identity.asset_id != 0 && rec.identity.unit_code != 0;
            if !identity_ok {
                // 未绑定点不得进控制——目录语义解析必须失败。
                let key = eneros_semantic_catalog::ObservationKey {
                    tenant_id: 1,
                    site_id: 1,
                    source_id: 1,
                    device_id: rec.device_id,
                    point_id: rec.point_id,
                    mapping_version: vs.mapping_version,
                };
                if repo.asset_of_point(rec.point_id).is_some() {
                    continue; // 目录可额外登记（如回读点 1001）
                }
                assert!(
                    repo.resolve_observation(&key, vs).is_err(),
                    "未绑定点 {} 不得解析成功",
                    rec.point_id
                );
            } else {
                let key = eneros_semantic_catalog::ObservationKey {
                    tenant_id: 1,
                    site_id: 1,
                    source_id: 1,
                    device_id: rec.device_id,
                    point_id: rec.point_id,
                    mapping_version: vs.mapping_version,
                };
                assert!(
                    repo.resolve_observation(&key, vs).is_ok(),
                    "绑定点 {} 必须有目录语义",
                    rec.point_id
                );
            }
        }
        // 控制点唯一：points.toml 中 data_type=control 的点数 == 目录
        // 控制合同点数 == 1（Phase 0 单 PCS 契约）。
        let control_pts = table
            .points
            .iter()
            .filter(|p| p.data_type == eneros_upa_model::PointType::Control)
            .count();
        assert_eq!(control_pts, 1);
        assert_eq!(control_contract_point(&repo), Some(10));
    }
}
