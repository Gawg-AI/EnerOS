//! # EnerOS Driver SDK v1.0
//!
//! 第三方硬件驱动插件开发框架。
//!
//! ## 快速开始
//!
//! ```ignore
//! use eneros_driver_sdk::{
//!     register_driver, DeviceClass, DeviceInfo, DriverConfig, DriverError,
//!     DriverPlugin, ResourceRange,
//! };
//!
//! struct MyDriver;
//!
//! impl MyDriver {
//!     fn new() -> Self { Self }
//! }
//!
//! impl DriverPlugin for MyDriver {
//!     fn name(&self) -> &str { "my-driver" }
//!     fn version(&self) -> &str { "1.0.0" }
//!     fn probe(&mut self, info: &DeviceInfo) -> Result<bool, DriverError> {
//!         Ok(info.vendor_id == 0x1234)
//!     }
//!     fn init(&mut self, _config: &DriverConfig) -> Result<(), DriverError> { Ok(()) }
//!     fn read(&mut self, _reg: u32, buf: &mut [u8]) -> Result<(), DriverError> {
//!         buf.fill(0);
//!         Ok(())
//!     }
//!     fn write(&mut self, _reg: u32, _data: &[u8]) -> Result<(), DriverError> { Ok(()) }
//!     fn deinit(&mut self) -> Result<(), DriverError> { Ok(()) }
//! }
//!
//! register_driver!(MyDriver);
//! ```
//!
//! ## no_std 合规
//! 本 crate 完全 no_std，仅依赖 `alloc` 和 `heapless`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

use heapless::String as HString;

/// Driver SDK 版本号。
pub const SDK_VERSION: &str = "1.0.0";

/// SDK ABI 版本（插件兼容性检查）。
pub const ABI_VERSION: u32 = 1;

// ============================================================================
// 错误类型
// ============================================================================

/// 驱动错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// 设备不存在
    DeviceNotFound(HString<64>),
    /// 初始化失败
    InitFailed(HString<128>),
    /// 读取失败
    ReadFailed { reg: u32 },
    /// 写入失败
    WriteFailed { reg: u32 },
    /// 设备忙
    DeviceBusy,
    /// 不支持的操作
    NotSupported,
    /// 超时
    Timeout,
    /// 硬件故障
    HardwareFault(HString<128>),
    /// 未初始化
    NotInitialized,
}

impl core::fmt::Display for DriverError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DriverError::DeviceNotFound(name) => write!(f, "device not found: {}", name),
            DriverError::InitFailed(msg) => write!(f, "init failed: {}", msg),
            DriverError::ReadFailed { reg } => write!(f, "read failed at reg 0x{:08x}", reg),
            DriverError::WriteFailed { reg } => write!(f, "write failed at reg 0x{:08x}", reg),
            DriverError::DeviceBusy => write!(f, "device busy"),
            DriverError::NotSupported => write!(f, "not supported"),
            DriverError::Timeout => write!(f, "timeout"),
            DriverError::HardwareFault(msg) => write!(f, "hardware fault: {}", msg),
            DriverError::NotInitialized => write!(f, "not initialized"),
        }
    }
}

// ============================================================================
// 数据结构
// ============================================================================

/// 设备类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    /// 网络设备
    Network,
    /// 存储设备
    Storage,
    /// 串口设备
    Serial,
    /// GPIO
    Gpio,
    /// I2C
    I2c,
    /// SPI
    Spi,
    /// CAN
    Can,
    /// 定时器
    Timer,
    /// ADC/DAC
    Analog,
    /// 电源管理
    Power,
    /// 传感器
    Sensor,
    /// 自定义
    Custom,
}

/// 资源范围（MMIO / IO 端口 / 中断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRange {
    /// 起始地址
    pub start: u64,
    /// 长度（字节）
    pub len: u64,
    /// 资源类型（0=MMIO, 1=IO, 2=IRQ）
    pub resource_type: u8,
}

/// 设备信息（probe 时传入）。
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// 厂商 ID
    pub vendor_id: u16,
    /// 设备 ID
    pub device_id: u16,
    /// 设备类别
    pub class: DeviceClass,
    /// 资源列表
    pub resources: Vec<ResourceRange>,
    /// 设备名称（≤64 字节）
    pub name: HString<64>,
}

impl DeviceInfo {
    /// 创建最小设备信息。
    pub fn minimal(vendor_id: u16, device_id: u16, class: DeviceClass) -> Self {
        Self {
            vendor_id,
            device_id,
            class,
            resources: Vec::new(),
            name: HString::<64>::new(),
        }
    }

    /// 添加 MMIO 资源。
    pub fn with_mmio(mut self, start: u64, len: u64) -> Self {
        self.resources.push(ResourceRange {
            start,
            len,
            resource_type: 0,
        });
        self
    }

    /// 添加 IRQ 资源。
    pub fn with_irq(mut self, irq: u64) -> Self {
        self.resources.push(ResourceRange {
            start: irq,
            len: 1,
            resource_type: 2,
        });
        self
    }

    /// 设置名称。
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = HString::<64>::try_from(name).unwrap_or_default();
        self
    }
}

/// 驱动配置。
#[derive(Debug, Clone)]
pub struct DriverConfig {
    /// 设备名称（≤64 字节）
    pub device_name: HString<64>,
    /// MMIO 基址
    pub base_addr: u64,
    /// 中断号
    pub irq: u32,
    /// 是否启用 DMA
    pub dma_enabled: bool,
    /// 时钟频率（Hz）
    pub clock_hz: u32,
}

impl DriverConfig {
    /// 创建默认配置。
    pub fn new(device_name: &str, base_addr: u64) -> Self {
        Self {
            device_name: HString::<64>::try_from(device_name).unwrap_or_default(),
            base_addr,
            irq: 0,
            dma_enabled: false,
            clock_hz: 0,
        }
    }

    /// 设置中断号。
    pub fn with_irq(mut self, irq: u32) -> Self {
        self.irq = irq;
        self
    }

    /// 启用 DMA。
    pub fn with_dma(mut self) -> Self {
        self.dma_enabled = true;
        self
    }

    /// 设置时钟频率。
    pub fn with_clock(mut self, hz: u32) -> Self {
        self.clock_hz = hz;
        self
    }
}

// ============================================================================
// DriverPlugin trait
// ============================================================================

/// 驱动插件 trait（SDK v1.0 稳定 API）。
///
/// 第三方开发者实现此接口开发自定义硬件驱动插件。
///
/// # 生命周期
/// ```text
/// new() → probe → init → read/write (循环) → deinit
/// ```
pub trait DriverPlugin: Send + Sync {
    /// 驱动名称。
    fn name(&self) -> &str;

    /// 驱动版本。
    fn version(&self) -> &str;

    /// 探测设备（检查是否匹配）。
    ///
    /// 返回 `true` 表示驱动支持该设备。
    fn probe(&mut self, device_info: &DeviceInfo) -> Result<bool, DriverError>;

    /// 初始化驱动。
    fn init(&mut self, config: &DriverConfig) -> Result<(), DriverError>;

    /// 读取寄存器。
    fn read(&mut self, reg: u32, buf: &mut [u8]) -> Result<(), DriverError>;

    /// 写入寄存器。
    fn write(&mut self, reg: u32, data: &[u8]) -> Result<(), DriverError>;

    /// 去初始化。
    fn deinit(&mut self) -> Result<(), DriverError>;

    /// 中断处理（可选）。
    fn handle_irq(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    /// 获取驱动状态（可选）。
    fn status(&self) -> DriverStatus {
        DriverStatus::Unknown
    }
}

/// 驱动状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverStatus {
    /// 未知
    Unknown,
    /// 已探测
    Probed,
    /// 已初始化
    Initialized,
    /// 运行中
    Running,
    /// 错误
    Error,
    /// 已去初始化
    Deinitialized,
}

// ============================================================================
// 注册宏
// ============================================================================

/// 驱动插件注册宏（C ABI 导出）。
///
/// # 导出函数
/// - `eneros_driver_create` — 创建插件实例
/// - `eneros_driver_version` — 获取 ABI 版本
/// - `eneros_driver_destroy` — 销毁插件实例
#[macro_export]
macro_rules! register_driver {
    ($type:ty) => {
        /// 创建驱动插件实例（C ABI）。
        #[no_mangle]
        pub extern "C" fn eneros_driver_create() -> *mut dyn $crate::DriverPlugin {
            let d = <$type>::new();
            Box::into_raw(Box::new(d))
        }

        /// 获取 ABI 版本（C ABI）。
        #[no_mangle]
        pub extern "C" fn eneros_driver_version() -> u32 {
            $crate::ABI_VERSION
        }

        /// 销毁驱动插件实例（C ABI）。
        ///
        /// # Safety
        /// 调用者必须确保 `ptr` 是由 `eneros_driver_create` 返回的有效指针。
        #[no_mangle]
        pub unsafe extern "C" fn eneros_driver_destroy(ptr: *mut dyn $crate::DriverPlugin) {
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
    };
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock 驱动插件。
    struct MockDriver {
        initialized: bool,
        regs: [u8; 256],
    }

    impl MockDriver {
        fn new() -> Self {
            Self {
                initialized: false,
                regs: [0; 256],
            }
        }
    }

    impl DriverPlugin for MockDriver {
        fn name(&self) -> &str {
            "mock-driver"
        }
        fn version(&self) -> &str {
            "1.0.0"
        }
        fn probe(&mut self, info: &DeviceInfo) -> Result<bool, DriverError> {
            Ok(info.vendor_id == 0x1234 && info.device_id == 0x5678)
        }
        fn init(&mut self, _config: &DriverConfig) -> Result<(), DriverError> {
            self.initialized = true;
            Ok(())
        }
        fn read(&mut self, reg: u32, buf: &mut [u8]) -> Result<(), DriverError> {
            if !self.initialized {
                return Err(DriverError::NotInitialized);
            }
            let start = reg as usize;
            let end = start + buf.len();
            if end > self.regs.len() {
                return Err(DriverError::ReadFailed { reg });
            }
            buf.copy_from_slice(&self.regs[start..end]);
            Ok(())
        }
        fn write(&mut self, reg: u32, data: &[u8]) -> Result<(), DriverError> {
            if !self.initialized {
                return Err(DriverError::NotInitialized);
            }
            let start = reg as usize;
            let end = start + data.len();
            if end > self.regs.len() {
                return Err(DriverError::WriteFailed { reg });
            }
            self.regs[start..end].copy_from_slice(data);
            Ok(())
        }
        fn deinit(&mut self) -> Result<(), DriverError> {
            self.initialized = false;
            Ok(())
        }
        fn status(&self) -> DriverStatus {
            if self.initialized {
                DriverStatus::Running
            } else {
                DriverStatus::Deinitialized
            }
        }
    }

    /// DRV1：完整生命周期。
    #[test]
    fn drv1_full_lifecycle() {
        let mut drv = MockDriver::new();
        let info = DeviceInfo::minimal(0x1234, 0x5678, DeviceClass::Serial)
            .with_mmio(0x0900_0000, 0x1000)
            .with_irq(33)
            .with_name("uart0");

        // probe
        assert!(drv.probe(&info).expect("probe 应成功"));

        // init
        let config = DriverConfig::new("uart0", 0x0900_0000)
            .with_irq(33)
            .with_clock(24_000_000);
        drv.init(&config).expect("init 应成功");
        assert_eq!(drv.status(), DriverStatus::Running);

        // write + read
        drv.write(0x10, &[0xAB, 0xCD]).expect("write 应成功");
        let mut buf = [0u8; 2];
        drv.read(0x10, &mut buf).expect("read 应成功");
        assert_eq!(buf, [0xAB, 0xCD]);

        // deinit
        drv.deinit().expect("deinit 应成功");
        assert_eq!(drv.status(), DriverStatus::Deinitialized);
    }

    /// DRV2：probe 不匹配。
    #[test]
    fn drv2_probe_mismatch() {
        let mut drv = MockDriver::new();
        let info = DeviceInfo::minimal(0xAAAA, 0xBBBB, DeviceClass::Network);
        assert!(!drv.probe(&info).expect("probe 应成功"));
    }

    /// DRV3：未初始化读写拒绝。
    #[test]
    fn drv3_uninitialized_access() {
        let mut drv = MockDriver::new();
        let mut buf = [0u8; 4];
        assert_eq!(
            drv.read(0, &mut buf).expect_err("应拒绝"),
            DriverError::NotInitialized
        );
        assert_eq!(
            drv.write(0, &[1, 2, 3]).expect_err("应拒绝"),
            DriverError::NotInitialized
        );
    }

    /// DRV4：越界访问。
    #[test]
    fn drv4_out_of_bounds() {
        let mut drv = MockDriver::new();
        let config = DriverConfig::new("test", 0);
        drv.init(&config).expect("init 应成功");

        let mut buf = [0u8; 4];
        assert_eq!(
            drv.read(255, &mut buf).expect_err("应拒绝"),
            DriverError::ReadFailed { reg: 255 }
        );
    }

    /// DRV5：设备信息构建器。
    #[test]
    fn drv5_device_info_builder() {
        let info = DeviceInfo::minimal(0x1234, 0x5678, DeviceClass::Gpio)
            .with_mmio(0x4000_0000, 0x1000)
            .with_irq(42)
            .with_name("gpio0");

        assert_eq!(info.vendor_id, 0x1234);
        assert_eq!(info.device_id, 0x5678);
        assert_eq!(info.class, DeviceClass::Gpio);
        assert_eq!(info.resources.len(), 2);
        assert_eq!(info.resources[0].resource_type, 0); // MMIO
        assert_eq!(info.resources[1].resource_type, 2); // IRQ
        assert_eq!(info.name.as_str(), "gpio0");
    }

    /// DRV6：驱动配置构建器。
    #[test]
    fn drv6_driver_config_builder() {
        let config = DriverConfig::new("spi0", 0x4001_0000)
            .with_irq(50)
            .with_dma()
            .with_clock(100_000_000);

        assert_eq!(config.device_name.as_str(), "spi0");
        assert_eq!(config.base_addr, 0x4001_0000);
        assert_eq!(config.irq, 50);
        assert!(config.dma_enabled);
        assert_eq!(config.clock_hz, 100_000_000);
    }

    /// DRV7：错误类型 Display。
    #[test]
    fn drv7_error_display() {
        let e = DriverError::DeviceNotFound(HString::<64>::try_from("uart1").unwrap_or_default());
        assert_eq!(format!("{}", e), "device not found: uart1");
        assert_eq!(format!("{}", DriverError::DeviceBusy), "device busy");
        assert_eq!(
            format!("{}", DriverError::NotInitialized),
            "not initialized"
        );
        assert_eq!(
            format!("{}", DriverError::ReadFailed { reg: 0x10 }),
            "read failed at reg 0x00000010"
        );
    }

    /// DRV8：trait object safety。
    #[test]
    fn drv8_object_safe() {
        let drv: Box<dyn DriverPlugin> = Box::new(MockDriver::new());
        assert_eq!(drv.name(), "mock-driver");
        assert_eq!(drv.version(), "1.0.0");
    }

    /// DRV9：SDK 版本常量。
    #[test]
    fn drv9_sdk_version() {
        assert_eq!(SDK_VERSION, "1.0.0");
        assert_eq!(ABI_VERSION, 1);
    }
}
