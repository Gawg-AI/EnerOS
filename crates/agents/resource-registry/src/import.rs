//! 迁移导入（[08] §4.6）：energy-market-agent `DevicePool`、coordinator
//! `VppResource`、当前 control profile → Registry 描述/基座。旧 f32 MW
//! 在边界转 f64 kW（换算有 UnitCatalog 证据）；旧结构在业务 crate 降为
//! 内部 import DTO（本模块只定义 DTO 面与转换——不依赖业务 crate）。

use crate::provider::NameplateBase;
use crate::{RegistryError, RegistryErrorKind, ResourceDescriptor};

/// 旧 DevicePool 设备能力 DTO（f32 MW 域；字段名与 v0.87.0 对齐）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarketDeviceDto {
    pub id: u64,
    pub p_min_mw: f32,
    pub p_max_mw: f32,
    /// MW·min⁻¹。
    pub ramp_rate_mw_per_min: f32,
    pub efficiency: f32,
    pub kind: u16,
}

/// 旧 VppResource DTO（coordinator v0.94.0 域）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VppResourceDto {
    pub resource_id: u64,
    pub capacity_mw: f32,
    pub available_mw: f32,
    pub ramp_rate_mw_per_min: f32,
    pub efficiency: f32,
    pub kind: u16,
    pub online: bool,
}

/// 单位换算证据：f32 MW → f64 kW（UnitCatalog：MW=1e6 W / kW=1e3 W；
/// 与 semantic-catalog `megawatts_to_kilowatts` 同源）。
fn mw_to_kw(mw: f32) -> Result<f64, RegistryError> {
    if !mw.is_finite() {
        return Err(RegistryError {
            kind: RegistryErrorKind::UnknownAsset,
            detail: "non-finite-mw".into(),
        });
    }
    Ok(f64::from(mw) * 1000.0)
}

/// 旧市场设备 → 描述 + 基座（f32 MW → f64 kW；ramp MW/min → kW/s）。
pub fn market_device_to_registry(
    dto: &MarketDeviceDto,
    site_id: u32,
    tenant_id: u32,
) -> Result<(ResourceDescriptor, NameplateBase), RegistryError> {
    let p_min = mw_to_kw(dto.p_min_mw)?;
    let p_max = mw_to_kw(dto.p_max_mw)?;
    if p_min > p_max {
        return Err(RegistryError {
            kind: RegistryErrorKind::UnknownAsset,
            detail: "p-range-inverted".into(),
        });
    }
    let ramp = mw_to_kw(dto.ramp_rate_mw_per_min)? / 60.0;
    // 效率字段在 import DTO 中保留但本版本基座不消费（聚合参数由
    // 组装方注入——避免自造默认值）。
    let _eff = if dto.efficiency.is_finite() && dto.efficiency > 0.0 {
        dto.efficiency.clamp(0.0, 1.0) as f64
    } else {
        1.0
    };
    Ok((
        ResourceDescriptor {
            asset_id: dto.id,
            site_id,
            resource_kind: dto.kind,
            control_contract_id: 0, // 合同由目录装配（本版本单资产生产）
            topology_node_id: 0,
            capability_model_id: 1,
            owner_tenant_id: tenant_id,
        },
        NameplateBase {
            p_min_kw: p_min,
            p_max_kw: p_max,
            q_min_kvar: 0.0,
            q_max_kvar: 0.0,
            energy_max_kwh: 0.0,
            ramp_up_kw_s: ramp,
            ramp_down_kw_s: ramp,
        },
    ))
}

/// 旧 VPP 资源 → 描述 + 基座（可用容量收紧 at import——`available_mw`
/// 是 imported 快照时刻事实，登记为基座收紧）。
pub fn vpp_resource_to_registry(
    dto: &VppResourceDto,
    site_id: u32,
    tenant_id: u32,
) -> Result<(ResourceDescriptor, NameplateBase), RegistryError> {
    let capacity = mw_to_kw(dto.capacity_mw)?;
    let available = mw_to_kw(dto.available_mw)?;
    if capacity < 0.0 || available < 0.0 || available > capacity {
        return Err(RegistryError {
            kind: RegistryErrorKind::UnknownAsset,
            detail: "capacity-invalid".into(),
        });
    }
    if !dto.online {
        // 离线资源：基座 p 范围归零（状态保留——聚合/分配排除）。
        return Ok((
            ResourceDescriptor {
                asset_id: dto.resource_id,
                site_id,
                resource_kind: dto.kind,
                control_contract_id: 0,
                topology_node_id: 0,
                capability_model_id: 1,
                owner_tenant_id: tenant_id,
            },
            NameplateBase {
                p_min_kw: 0.0,
                p_max_kw: 0.0,
                q_min_kvar: 0.0,
                q_max_kvar: 0.0,
                energy_max_kwh: 0.0,
                ramp_up_kw_s: 0.0,
                ramp_down_kw_s: 0.0,
            },
        ));
    }
    let ramp = mw_to_kw(dto.ramp_rate_mw_per_min)? / 60.0;
    Ok((
        ResourceDescriptor {
            asset_id: dto.resource_id,
            site_id,
            resource_kind: dto.kind,
            control_contract_id: 0,
            topology_node_id: 0,
            capability_model_id: 1,
            owner_tenant_id: tenant_id,
        },
        NameplateBase {
            p_min_kw: -available,
            p_max_kw: available,
            q_min_kvar: 0.0,
            q_max_kvar: 0.0,
            energy_max_kwh: 0.0,
            ramp_up_kw_s: if ramp.is_finite() && ramp >= 0.0 {
                ramp
            } else {
                0.0
            },
            ramp_down_kw_s: if ramp.is_finite() && ramp >= 0.0 {
                ramp
            } else {
                0.0
            },
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_device_mw_to_kw_boundary() {
        let (desc, base) = market_device_to_registry(
            &MarketDeviceDto {
                id: 42,
                p_min_mw: -0.5,
                p_max_mw: 1.0,
                ramp_rate_mw_per_min: 0.6,
                efficiency: 0.9,
                kind: crate::kinds::BATTERY,
            },
            1,
            1,
        )
        .unwrap();
        assert_eq!(desc.asset_id, 42);
        assert!((base.p_min_kw - (-500.0)).abs() < 1e-6);
        assert!((base.p_max_kw - 1000.0).abs() < 1e-6);
        // 0.6 MW/min = 600 kW/min = 10 kW/s。
        assert!((base.ramp_up_kw_s - 10.0).abs() < 1e-6);
    }

    #[test]
    fn vpp_offline_resource_zeroed_not_dropped() {
        let (desc, base) = vpp_resource_to_registry(
            &VppResourceDto {
                resource_id: 7,
                capacity_mw: 2.0,
                available_mw: 1.5,
                ramp_rate_mw_per_min: 0.3,
                efficiency: 1.0,
                kind: crate::kinds::PV,
                online: false,
            },
            1,
            1,
        )
        .unwrap();
        assert_eq!(desc.asset_id, 7);
        // 离线资源状态保留：id 仍在册，基座归零。
        assert_eq!(base.p_max_kw, 0.0);
        assert_eq!(base.p_min_kw, 0.0);
    }

    #[test]
    fn vpp_online_available_bounds() {
        let (_, base) = vpp_resource_to_registry(
            &VppResourceDto {
                resource_id: 8,
                capacity_mw: 2.0,
                available_mw: 1.5,
                ramp_rate_mw_per_min: 0.3,
                efficiency: 1.0,
                kind: crate::kinds::PV,
                online: true,
            },
            1,
            1,
        )
        .unwrap();
        assert!((base.p_max_kw - 1500.0).abs() < 1e-3);
        assert!((base.p_min_kw + 1500.0).abs() < 1e-3);
    }

    #[test]
    fn nan_mw_rejected() {
        assert!(market_device_to_registry(
            &MarketDeviceDto {
                id: 1,
                p_min_mw: f32::NAN,
                p_max_mw: 1.0,
                ramp_rate_mw_per_min: 0.1,
                efficiency: 1.0,
                kind: crate::kinds::BATTERY,
            },
            1,
            1,
        )
        .is_err());
    }
}
