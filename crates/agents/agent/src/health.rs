//! Agent 健康检查 — HealthStatus 枚举与 HealthCheck trait
//!
//! # 设计
//! - `HealthStatus` 表示 Agent 的健康状态（4 级：Healthy/Degraded/Unhealthy/Dead）
//! - `HealthCheck` 是 object-safe trait，Agent 可实现自定义健康检查
//! - v0.37.0 仅定义 trait，不主动调用（蓝图 §9.7 可扩展）
//!
//! # no_std 合规
//! 仅使用 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

/// Agent 健康状态（4 级）.
///
/// 状态演进：Healthy → Degraded（1+ 次心跳缺失）→ Unhealthy（达阈值）→ Dead（v0.38.0 设置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    /// 健康（最近周期内有心跳）
    Healthy,
    /// 降级（1+ 次心跳缺失，但未达阈值）
    Degraded,
    /// 不健康（心跳缺失达阈值）
    Unhealthy,
    /// 已死亡（由 v0.38.0 崩溃恢复设置，v0.37.0 不设置）
    Dead,
}

/// Agent 自定义健康检查 trait（object-safe）.
///
/// Agent 可实现此 trait 提供自定义健康检查逻辑（蓝图 §9.7）。
/// v0.37.0 仅定义 trait，不主动调用。
pub trait HealthCheck {
    /// 检查 Agent 健康状态.
    fn check_health(&self) -> HealthStatus;
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    #[test]
    #[allow(clippy::clone_on_copy)] // 显式调用 .clone() 以验证 Clone trait 实现
    fn test_health_status_clone_copy() {
        let s1 = HealthStatus::Healthy;
        let s2 = s1;
        let s3 = s1.clone();
        assert_eq!(s1, s2);
        assert_eq!(s1, s3);
    }

    #[test]
    fn test_health_status_debug() {
        assert!(format!("{:?}", HealthStatus::Healthy).contains("Healthy"));
        assert!(format!("{:?}", HealthStatus::Degraded).contains("Degraded"));
        assert!(format!("{:?}", HealthStatus::Unhealthy).contains("Unhealthy"));
        assert!(format!("{:?}", HealthStatus::Dead).contains("Dead"));
    }

    #[test]
    fn test_health_status_eq() {
        assert_eq!(HealthStatus::Healthy, HealthStatus::Healthy);
        assert_eq!(HealthStatus::Degraded, HealthStatus::Degraded);
        assert_eq!(HealthStatus::Unhealthy, HealthStatus::Unhealthy);
        assert_eq!(HealthStatus::Dead, HealthStatus::Dead);

        assert_ne!(HealthStatus::Healthy, HealthStatus::Degraded);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Unhealthy);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Dead);
        assert_ne!(HealthStatus::Degraded, HealthStatus::Unhealthy);
        assert_ne!(HealthStatus::Degraded, HealthStatus::Dead);
        assert_ne!(HealthStatus::Unhealthy, HealthStatus::Dead);
    }

    #[test]
    fn test_health_status_ordering() {
        // HealthStatus 仅 derive Clone/Copy/Debug/PartialEq/Eq（无 Hash/Ord），
        // 故使用 Vec + 逐对比较验证 4 个 variant 互不相同（等价于 HashSet.len()==4）。
        let variants: Vec<HealthStatus> = vec![
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Unhealthy,
            HealthStatus::Dead,
        ];
        assert_eq!(variants.len(), 4);

        for i in 0..variants.len() {
            for j in (i + 1)..variants.len() {
                assert_ne!(variants[i], variants[j], "duplicate variant found");
            }
        }
    }

    #[test]
    fn test_health_check_object_safe() {
        struct AlwaysHealthy;

        impl HealthCheck for AlwaysHealthy {
            fn check_health(&self) -> HealthStatus {
                HealthStatus::Healthy
            }
        }

        let checker: Box<dyn HealthCheck> = Box::new(AlwaysHealthy);
        assert_eq!(checker.check_health(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_check_custom_impl() {
        struct CustomChecker {
            status: HealthStatus,
        }

        impl HealthCheck for CustomChecker {
            fn check_health(&self) -> HealthStatus {
                self.status
            }
        }

        let variants = [
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Unhealthy,
            HealthStatus::Dead,
        ];
        for status in variants {
            let checker = CustomChecker { status };
            assert_eq!(checker.check_health(), status);
        }
    }
}
