//! EnerOS v0.87.0 设备能力模型与设备池管理.
//!
//! 定义多设备调度的基础数据类型：运行模式、设备能力、设备池。
//! 设备池使用 `BTreeMap` 保证设备迭代有序性，LP 变量列映射确定性（D3）。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D3** | `BTreeMap<u64, DeviceCapability>` 替代 `HashMap` — no_std 合规且迭代有序，LP 列映射确定性 |
//! | **D4** | `u64` 替代 `String` 设备 ID — no_std 无堆 String，保持 Copy 语义 |
//! | **D7** | `DeviceMode` 枚举定义（蓝图引用但未定义类型）；MVP 最小变体集 |

use alloc::collections::BTreeMap;

/// 设备运行模式.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceMode {
    /// 自动调度（默认）.
    #[default]
    Auto,
    /// 人工设定点.
    Manual,
}

/// 设备能力参数.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DeviceCapability {
    /// 最小功率（MW，负值表示充电）.
    pub p_min: f32,
    /// 最大功率（MW）.
    pub p_max: f32,
    /// 爬坡速率（MW·min⁻¹）.
    pub ramp_rate: f32,
    /// 转换效率（0~1）.
    pub efficiency: f32,
}

/// 设备池（有序映射；PartialEq 供 v0.96.0 云端汇聚 `DomainData` 比较）.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DevicePool {
    /// 设备 ID → 能力参数.
    pub devices: BTreeMap<u64, DeviceCapability>,
}

impl DevicePool {
    /// 创建空设备池.
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    /// 添加/更新设备（同 id 覆盖）.
    pub fn add_device(&mut self, id: u64, cap: DeviceCapability) {
        self.devices.insert(id, cap);
    }

    /// 移除设备；存在返回 true，不存在返回 false.
    pub fn remove_device(&mut self, id: u64) -> bool {
        self.devices.remove(&id).is_some()
    }

    /// 获取设备能力.
    pub fn get(&self, id: u64) -> Option<&DeviceCapability> {
        self.devices.get(&id)
    }

    /// 设备数量.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn t81_device_mode_default_is_auto() {
        assert_eq!(DeviceMode::default(), DeviceMode::Auto);
        let _ = format!("{:?}", DeviceMode::Auto);
        let _ = format!("{:?}", DeviceMode::Manual);
    }

    #[test]
    fn t82_device_capability_default_all_zero() {
        let c = DeviceCapability::default();
        assert_eq!(c.p_min, 0.0);
        assert_eq!(c.p_max, 0.0);
        assert_eq!(c.ramp_rate, 0.0);
        assert_eq!(c.efficiency, 0.0);
    }

    #[test]
    fn t83_device_capability_explicit_fields() {
        let c = DeviceCapability {
            p_min: 0.0,
            p_max: 5.0,
            ramp_rate: 1.0,
            efficiency: 0.9,
        };
        assert_eq!(c.p_min, 0.0);
        assert_eq!(c.p_max, 5.0);
        assert_eq!(c.ramp_rate, 1.0);
        assert_eq!(c.efficiency, 0.9);
    }

    #[test]
    fn t84_device_capability_copy() {
        let c = DeviceCapability {
            p_min: -2.0,
            p_max: 3.0,
            ramp_rate: 0.5,
            efficiency: 0.85,
        };
        let c2 = c;
        assert_eq!(c, c2);
    }

    #[test]
    fn t85_device_pool_new_empty() {
        let p = DevicePool::new();
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
        assert!(p.devices.is_empty());
    }

    #[test]
    fn t86_device_pool_add_one() {
        let mut p = DevicePool::new();
        let cap = DeviceCapability {
            p_min: 0.0,
            p_max: 5.0,
            ramp_rate: 1.0,
            efficiency: 0.9,
        };
        p.add_device(1, cap);
        assert_eq!(p.len(), 1);
        assert_eq!(p.get(1), Some(&cap));
        assert!(!p.is_empty());
    }

    #[test]
    fn t87_device_pool_add_overwrite() {
        let mut p = DevicePool::new();
        let cap1 = DeviceCapability {
            p_min: 0.0,
            p_max: 5.0,
            ramp_rate: 1.0,
            efficiency: 0.9,
        };
        let cap2 = DeviceCapability {
            p_min: 0.0,
            p_max: 10.0,
            ramp_rate: 2.0,
            efficiency: 0.8,
        };
        p.add_device(1, cap1);
        p.add_device(1, cap2);
        assert_eq!(p.len(), 1);
        assert_eq!(p.get(1), Some(&cap2));
    }

    #[test]
    fn t88_device_pool_remove_existing() {
        let mut p = DevicePool::new();
        p.add_device(1, DeviceCapability::default());
        assert!(p.remove_device(1));
        assert_eq!(p.get(1), None);
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn t89_device_pool_remove_nonexistent() {
        let mut p = DevicePool::new();
        p.add_device(1, DeviceCapability::default());
        assert!(!p.remove_device(99));
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn t90_device_pool_ordered_keys() {
        let mut p = DevicePool::new();
        p.add_device(
            30,
            DeviceCapability {
                p_min: 0.0,
                p_max: 3.0,
                ramp_rate: 0.5,
                efficiency: 0.9,
            },
        );
        p.add_device(
            10,
            DeviceCapability {
                p_min: 0.0,
                p_max: 2.0,
                ramp_rate: 0.4,
                efficiency: 0.85,
            },
        );
        p.add_device(
            20,
            DeviceCapability {
                p_min: 0.0,
                p_max: 4.0,
                ramp_rate: 0.6,
                efficiency: 0.8,
            },
        );
        let keys: Vec<u64> = p.devices.keys().copied().collect();
        assert_eq!(keys, vec![10, 20, 30]);
    }

    #[test]
    fn t91_device_pool_default_eq_new_and_clone() {
        let a = DevicePool::default();
        let b = DevicePool::new();
        assert_eq!(a.len(), b.len());
        let mut c = DevicePool::new();
        c.add_device(
            1,
            DeviceCapability {
                p_min: 0.0,
                p_max: 5.0,
                ramp_rate: 1.0,
                efficiency: 0.9,
            },
        );
        let d = c.clone();
        assert_eq!(c.get(1), d.get(1));
    }

    #[test]
    fn t92_device_pool_len_after_add_remove() {
        let mut p = DevicePool::new();
        p.add_device(1, DeviceCapability::default());
        p.add_device(2, DeviceCapability::default());
        p.add_device(3, DeviceCapability::default());
        assert_eq!(p.len(), 3);
        p.remove_device(2);
        assert_eq!(p.len(), 2);
    }
}
