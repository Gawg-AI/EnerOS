//! EnerOS 驱动框架 — 用户态设备驱动统一抽象（v0.43.0）.
//!
//! 定义设备驱动的核心类型与统一接口，为所有用户态设备驱动提供：
//! - [`DeviceDriver`] trait — 驱动统一接口（id/name/type/state/init/start/stop/deinit/irq/health）
//! - [`DriverId`] — 驱动唯一标识（u64）
//! - [`DriverType`] — 驱动类型枚举（串口/网卡/CAN/存储/GPIO/I2C/SPI/Custom）
//! - [`DriverState`] — 驱动状态机（6 态）
//! - [`DriverHealth`] — 驱动健康状态（4 级）
//! - [`DriverError`] — 驱动框架错误类型
//!
//! 子模块（由后续任务实现）：
//! - `registry` — 驱动注册表
//! - `handle` — 驱动句柄与能力
//! - `mock` — 测试用 mock 驱动
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零外部依赖。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod handle;
pub mod mock;
pub mod registry;

use core::fmt;

/// 驱动唯一标识
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DriverId(pub u64);

/// 驱动类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DriverType {
    /// 串口（RS232/RS485/RS422）
    Serial,
    /// 网卡
    Network,
    /// CAN 总线
    Can,
    /// 存储设备
    Storage,
    /// GPIO
    Gpio,
    /// I2C
    I2c,
    /// SPI
    Spi,
    /// 自定义类型（扩展用）
    Custom(u16),
}

/// 驱动状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverState {
    /// 未初始化
    Uninitialized,
    /// 就绪（已初始化，未运行）
    Ready,
    /// 运行中
    Running,
    /// 已停止
    Stopped,
    /// 错误状态
    Error,
    /// 已销毁（不可恢复）
    Dead,
}

/// 驱动健康状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverHealth {
    /// 健康
    Healthy,
    /// 降级（可运行但功能受限）
    Degraded,
    /// 不健康（需干预）
    Unhealthy,
    /// 未知（未检测或无法检测）
    Unknown,
}

/// 设备驱动统一接口
///
/// 所有用户态设备驱动必须实现此 trait。生命周期方法遵循
/// `Uninitialized -> Ready -> Running -> Stopped -> Dead` 状态机，
/// 错误时进入 `Error` 态。
pub trait DeviceDriver: Send + Sync {
    /// 返回驱动唯一标识的引用
    fn id(&self) -> &DriverId;
    /// 返回驱动名称
    fn name(&self) -> &str;
    /// 返回驱动类型
    fn driver_type(&self) -> DriverType;
    /// 返回当前驱动状态
    fn state(&self) -> DriverState;
    /// 初始化驱动（Uninitialized -> Ready）
    fn init(&mut self) -> Result<(), DriverError>;
    /// 启动驱动（Ready/Stopped -> Running）
    fn start(&mut self) -> Result<(), DriverError>;
    /// 停止驱动（Running -> Stopped）
    fn stop(&mut self) -> Result<(), DriverError>;
    /// 反初始化驱动（Stopped/Ready -> Dead）
    fn deinit(&mut self) -> Result<(), DriverError>;
    /// 处理中断
    fn handle_irq(&mut self, irq_id: u32);
    /// 健康检查
    fn health_check(&self) -> DriverHealth;
}

/// 驱动框架错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// 驱动已注册
    AlreadyRegistered,
    /// 驱动未找到
    NotFound,
    /// 权限不足
    PermissionDenied,
    /// 当前状态不允许此操作
    InvalidState,
    /// 初始化失败
    InitFailed,
    /// 启动失败
    StartFailed,
    /// 停止失败
    StopFailed,
    /// 反初始化失败
    DeinitFailed,
    /// 驱动未注册
    NotRegistered,
    /// 操作超时
    Timeout,
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriverError::AlreadyRegistered => write!(f, "driver already registered"),
            DriverError::NotFound => write!(f, "driver not found"),
            DriverError::PermissionDenied => write!(f, "permission denied"),
            DriverError::InvalidState => write!(f, "invalid state for this operation"),
            DriverError::InitFailed => write!(f, "driver init failed"),
            DriverError::StartFailed => write!(f, "driver start failed"),
            DriverError::StopFailed => write!(f, "driver stop failed"),
            DriverError::DeinitFailed => write!(f, "driver deinit failed"),
            DriverError::NotRegistered => write!(f, "driver not registered"),
            DriverError::Timeout => write!(f, "operation timed out"),
        }
    }
}

impl core::error::Error for DriverError {}

// Re-export submodule types for convenience.
// lib.rs 自身定义的类型（DeviceDriver/DriverId 等）已为 pub，无需再次 re-export。
pub use handle::{DriverCapability, DriverHandle, DriverPermission};
pub use mock::MockDriver;
pub use registry::{DriverRegistry, DriverStats};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_driver_id_construct_and_compare() {
        let id1 = DriverId(1);
        let id1_dup = DriverId(1);
        let id2 = DriverId(2);

        assert_eq!(id1, id1_dup);
        assert_ne!(id1, id2);
        assert!(id1 < id2);
        assert!(id2 > id1);
        assert_eq!(id1.0, 1u64);
        assert_eq!(id2.0, 2u64);
    }

    #[test]
    fn test_driver_type_variants() {
        let serial = DriverType::Serial;
        let network = DriverType::Network;
        let can = DriverType::Can;
        let storage = DriverType::Storage;
        let gpio = DriverType::Gpio;
        let i2c = DriverType::I2c;
        let spi = DriverType::Spi;
        let custom = DriverType::Custom(42);

        // 7 个具名变体两两不相等
        assert_ne!(serial, network);
        assert_ne!(serial, can);
        assert_ne!(serial, storage);
        assert_ne!(serial, gpio);
        assert_ne!(serial, i2c);
        assert_ne!(serial, spi);
        assert_ne!(serial, custom);

        // Custom 变体携带值
        assert_eq!(custom, DriverType::Custom(42));
        assert_ne!(custom, DriverType::Custom(43));
    }

    #[test]
    fn test_driver_state_all_variants() {
        let states = [
            DriverState::Uninitialized,
            DriverState::Ready,
            DriverState::Running,
            DriverState::Stopped,
            DriverState::Error,
            DriverState::Dead,
        ];

        // 全部 6 个状态自反相等
        for s in states.iter() {
            assert_eq!(*s, s.clone());
        }

        // 两两不相等（共 6 个，应有 15 对不同）
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(
                    states[i], states[j],
                    "states {:?} and {:?} should differ",
                    states[i], states[j]
                );
            }
        }
    }

    #[test]
    fn test_driver_health_all_variants() {
        let healths = [
            DriverHealth::Healthy,
            DriverHealth::Degraded,
            DriverHealth::Unhealthy,
            DriverHealth::Unknown,
        ];

        // 全部 4 个健康状态自反相等
        for h in healths.iter() {
            assert_eq!(*h, h.clone());
        }

        // 两两不相等
        for i in 0..healths.len() {
            for j in (i + 1)..healths.len() {
                assert_ne!(healths[i], healths[j]);
            }
        }
    }

    #[test]
    fn test_driver_error_display() {
        assert_eq!(
            format!("{}", DriverError::AlreadyRegistered),
            "driver already registered"
        );
        assert_eq!(format!("{}", DriverError::NotFound), "driver not found");
        assert_eq!(
            format!("{}", DriverError::PermissionDenied),
            "permission denied"
        );
        assert_eq!(
            format!("{}", DriverError::InvalidState),
            "invalid state for this operation"
        );
        assert_eq!(format!("{}", DriverError::InitFailed), "driver init failed");
        assert_eq!(
            format!("{}", DriverError::StartFailed),
            "driver start failed"
        );
        assert_eq!(format!("{}", DriverError::StopFailed), "driver stop failed");
        assert_eq!(
            format!("{}", DriverError::DeinitFailed),
            "driver deinit failed"
        );
        assert_eq!(
            format!("{}", DriverError::NotRegistered),
            "driver not registered"
        );
        assert_eq!(format!("{}", DriverError::Timeout), "operation timed out");
    }

    #[test]
    fn test_driver_error_eq() {
        // 自反相等
        assert_eq!(
            DriverError::AlreadyRegistered,
            DriverError::AlreadyRegistered
        );
        assert_eq!(DriverError::NotFound, DriverError::NotFound);

        // 不同变体不相等
        assert_ne!(DriverError::AlreadyRegistered, DriverError::NotFound);
        assert_ne!(DriverError::NotFound, DriverError::NotRegistered);
        assert_ne!(DriverError::PermissionDenied, DriverError::InvalidState);
        assert_ne!(DriverError::InitFailed, DriverError::StartFailed);
        assert_ne!(DriverError::StopFailed, DriverError::DeinitFailed);

        // clone 相等
        let e = DriverError::InvalidState;
        assert_eq!(e, e.clone());

        // Timeout 变体
        assert_ne!(DriverError::Timeout, DriverError::NotFound);
        assert_eq!(DriverError::Timeout, DriverError::Timeout);
    }
}
