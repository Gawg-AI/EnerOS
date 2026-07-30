//! 内核热迁移实现（v0.163.0 蓝图 §4.3）。
//!
//! 提供内核热迁移的状态机模型与平台抽象层（PAL）接口：
//! - [`MigrateState`]：迁移状态机（Idle → Loading → MigratingThreads →
//!   MigratingMemory → MigratingDevices → Switching → Complete / Failed）
//! - [`MigrateContext`]：迁移上下文，持有各阶段快照
//! - [`PlatformAbstraction`]：平台适配层 trait，实现者提供真实硬件操作
//!
//! # 设计决策（蓝图 §5 技术交底）
//! 内核热迁移复杂度极高，本模块提供：
//! 1. 完整状态机骨架（可编译、可测试）
//! 2. 平台抽象层接口（真实迁移逻辑由 BSP 实现）
//! 3. 默认 `NullPlatform` 实现（用于测试/模拟）
//!
//! # no_std 合规
//! 本模块不使用 `std::*`；所有动态内存经 `alloc`。

use alloc::vec::Vec;

use crate::kernel_ota::{KernelImage, MigrateError};

/// 热迁移状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateState {
    /// 空闲，未开始迁移
    Idle,
    /// 正在加载新内核到内存
    Loading,
    /// 正在迁移线程状态
    MigratingThreads,
    /// 正在迁移内存映射
    MigratingMemory,
    /// 正在迁移设备状态
    MigratingDevices,
    /// 正在切换执行流
    Switching,
    /// 迁移完成
    Complete,
    /// 迁移失败
    Failed,
}

/// 线程状态快照（平台无关表示）。
#[derive(Debug, Clone)]
pub struct ThreadSnapshot {
    /// 线程 ID
    pub thread_id: u32,
    /// 程序计数器
    pub pc: u64,
    /// 栈指针
    pub sp: u64,
    /// 通用寄存器（x0-x28, x29/fp, x30/lr）
    pub regs: [u64; 31],
    /// 线程优先级
    pub priority: u8,
    /// 线程状态（0=就绪, 1=运行, 2=阻塞, 3=挂起）
    pub state: u8,
}

/// 内存映射快照。
#[derive(Debug, Clone)]
pub struct MemoryMapSnapshot {
    /// 虚拟地址起始
    pub vaddr_start: u64,
    /// 物理地址起始
    pub paddr_start: u64,
    /// 映射大小（字节）
    pub size: u64,
    /// 页表属性（ARM64 MAIR 索引 + 访问权限）
    pub attributes: u64,
}

/// 设备状态快照。
#[derive(Debug, Clone)]
pub struct DeviceSnapshot {
    /// 设备 ID
    pub device_id: u32,
    /// 设备类型（0=UART, 1=GPIO, 2=TIMER, 3=DMA, 4=NET）
    pub device_type: u8,
    /// 寄存器快照（≤64 个寄存器）
    pub registers: Vec<u32>,
    /// 中断号
    pub irq: u32,
    /// 是否已初始化
    pub initialized: bool,
}

/// 迁移上下文：持有各阶段快照与状态。
#[derive(Debug, Clone)]
pub struct MigrateContext {
    /// 当前迁移状态
    pub state: MigrateState,
    /// 新内核版本
    pub target_version: heapless::String<32>,
    /// 线程快照列表
    pub thread_snapshots: Vec<ThreadSnapshot>,
    /// 内存映射快照列表
    pub memory_snapshots: Vec<MemoryMapSnapshot>,
    /// 设备状态快照列表
    pub device_snapshots: Vec<DeviceSnapshot>,
    /// 迁移开始时间（毫秒）
    pub start_time_ms: u64,
    /// 迁移完成时间（毫秒）
    pub end_time_ms: u64,
    /// 错误信息（失败时）
    pub error: Option<MigrateError>,
}

impl MigrateContext {
    /// 创建新的迁移上下文。
    pub fn new(target_version: heapless::String<32>, now_ms: u64) -> Self {
        Self {
            state: MigrateState::Idle,
            target_version,
            thread_snapshots: Vec::new(),
            memory_snapshots: Vec::new(),
            device_snapshots: Vec::new(),
            start_time_ms: now_ms,
            end_time_ms: 0,
            error: None,
        }
    }

    /// 获取迁移耗时（毫秒）。
    pub fn duration_ms(&self) -> u64 {
        self.end_time_ms.saturating_sub(self.start_time_ms)
    }
}

/// 平台抽象层 trait（BSP 实现）。
///
/// 真实硬件操作由板级支持包实现，测试使用 [`NullPlatform`]。
pub trait PlatformAbstraction {
    /// 暂停所有线程，返回线程快照。
    fn suspend_threads(&mut self) -> Result<Vec<ThreadSnapshot>, MigrateError>;

    /// 恢复线程（从快照）。
    fn resume_threads(&mut self, snapshots: &[ThreadSnapshot]) -> Result<(), MigrateError>;

    /// 获取当前内存映射。
    fn get_memory_map(&mut self) -> Result<Vec<MemoryMapSnapshot>, MigrateError>;

    /// 应用新内存映射。
    fn apply_memory_map(&mut self, maps: &[MemoryMapSnapshot]) -> Result<(), MigrateError>;

    /// 获取设备状态。
    fn get_device_states(&mut self) -> Result<Vec<DeviceSnapshot>, MigrateError>;

    /// 恢复设备状态。
    fn restore_devices(&mut self, devices: &[DeviceSnapshot]) -> Result<(), MigrateError>;

    /// 加载新内核到指定地址。
    fn load_kernel(&mut self, image: &[u8], load_addr: u64) -> Result<(), MigrateError>;

    /// 切换执行流到新内核入口。
    fn switch_execution(&mut self, entry_addr: u64) -> Result<(), MigrateError>;
}

/// 空平台实现（测试/模拟用）。
///
/// 所有操作返回成功，不执行真实硬件操作。
pub struct NullPlatform {
    /// 模拟线程数
    pub thread_count: u32,
    /// 模拟设备数
    pub device_count: u32,
}

impl NullPlatform {
    /// 创建空平台。
    pub fn new(thread_count: u32, device_count: u32) -> Self {
        Self {
            thread_count,
            device_count,
        }
    }
}

impl PlatformAbstraction for NullPlatform {
    fn suspend_threads(&mut self) -> Result<Vec<ThreadSnapshot>, MigrateError> {
        let mut snapshots = Vec::new();
        for i in 0..self.thread_count {
            snapshots.push(ThreadSnapshot {
                thread_id: i,
                pc: 0x4900_0000 + (i as u64) * 0x1000,
                sp: 0x5000_0000 - (i as u64) * 0x4000,
                regs: [0; 31],
                priority: (i % 8) as u8,
                state: if i == 0 { 1 } else { 0 },
            });
        }
        Ok(snapshots)
    }

    fn resume_threads(&mut self, _snapshots: &[ThreadSnapshot]) -> Result<(), MigrateError> {
        Ok(())
    }

    fn get_memory_map(&mut self) -> Result<Vec<MemoryMapSnapshot>, MigrateError> {
        Ok(alloc::vec![
            MemoryMapSnapshot {
                vaddr_start: 0x4000_0000,
                paddr_start: 0x4000_0000,
                size: 0x1000_0000,
                attributes: 0x0000_0000_0000_0703, // Normal WB, EL1 RW
            },
            MemoryMapSnapshot {
                vaddr_start: 0x0900_0000,
                paddr_start: 0x0900_0000,
                size: 0x1000,
                attributes: 0x0000_0000_0000_0403, // Device-nGnRnE, EL1 RW
            },
        ])
    }

    fn apply_memory_map(&mut self, _maps: &[MemoryMapSnapshot]) -> Result<(), MigrateError> {
        Ok(())
    }

    fn get_device_states(&mut self) -> Result<Vec<DeviceSnapshot>, MigrateError> {
        let mut devices = Vec::new();
        for i in 0..self.device_count {
            devices.push(DeviceSnapshot {
                device_id: i,
                device_type: (i % 5) as u8,
                registers: alloc::vec![0; 16],
                irq: 32 + i,
                initialized: true,
            });
        }
        Ok(devices)
    }

    fn restore_devices(&mut self, _devices: &[DeviceSnapshot]) -> Result<(), MigrateError> {
        Ok(())
    }

    fn load_kernel(&mut self, _image: &[u8], _load_addr: u64) -> Result<(), MigrateError> {
        Ok(())
    }

    fn switch_execution(&mut self, _entry_addr: u64) -> Result<(), MigrateError> {
        Ok(())
    }
}

/// 热迁移引擎：协调平台抽象层完成迁移。
pub struct MigrateEngine<P: PlatformAbstraction> {
    /// 平台抽象层
    platform: P,
    /// 迁移上下文
    context: MigrateContext,
}

impl<P: PlatformAbstraction> MigrateEngine<P> {
    /// 创建迁移引擎。
    pub fn new(platform: P, target_version: heapless::String<32>, now_ms: u64) -> Self {
        Self {
            platform,
            context: MigrateContext::new(target_version, now_ms),
        }
    }

    /// 获取迁移上下文。
    pub fn context(&self) -> &MigrateContext {
        &self.context
    }

    /// 执行完整迁移流程。
    ///
    /// 返回迁移耗时（毫秒）。
    pub fn execute(
        &mut self,
        image: &KernelImage,
        load_addr: u64,
        entry_addr: u64,
        end_time_ms: u64,
    ) -> Result<u64, MigrateError> {
        // 1. 加载新内核
        self.context.state = MigrateState::Loading;
        self.platform
            .load_kernel(&image.data, load_addr)
            .map_err(|e| self.fail(e))?;

        // 2. 暂停并快照线程
        self.context.state = MigrateState::MigratingThreads;
        self.context.thread_snapshots =
            self.platform.suspend_threads().map_err(|e| self.fail(e))?;

        // 3. 快照并迁移内存映射
        self.context.state = MigrateState::MigratingMemory;
        self.context.memory_snapshots = self.platform.get_memory_map().map_err(|e| self.fail(e))?;
        self.platform
            .apply_memory_map(&self.context.memory_snapshots)
            .map_err(|e| self.fail(e))?;

        // 4. 快照并迁移设备状态
        self.context.state = MigrateState::MigratingDevices;
        self.context.device_snapshots = self
            .platform
            .get_device_states()
            .map_err(|e| self.fail(e))?;
        self.platform
            .restore_devices(&self.context.device_snapshots)
            .map_err(|e| self.fail(e))?;

        // 5. 切换执行流
        self.context.state = MigrateState::Switching;
        self.platform
            .switch_execution(entry_addr)
            .map_err(|e| self.fail(e))?;

        // 6. 恢复线程
        self.platform
            .resume_threads(&self.context.thread_snapshots)
            .map_err(|e| self.fail(e))?;

        // 7. 完成
        self.context.state = MigrateState::Complete;
        self.context.end_time_ms = end_time_ms;
        Ok(self.context.duration_ms())
    }

    /// 标记失败并返回错误。
    fn fail(&mut self, e: MigrateError) -> MigrateError {
        self.context.state = MigrateState::Failed;
        self.context.error = Some(e.clone());
        e
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_image() -> KernelImage {
        KernelImage {
            version: heapless::String::<32>::try_from("0.163.0").unwrap_or_default(),
            data: alloc::vec![0xAA; 4096],
            hash: [0; 32],
            signature: [0; 64],
        }
    }

    /// MIG1：NullPlatform 完整迁移流程。
    #[test]
    fn mig1_null_platform_full_migration() {
        let platform = NullPlatform::new(4, 3);
        let mut engine = MigrateEngine::new(
            platform,
            heapless::String::<32>::try_from("0.163.0").unwrap_or_default(),
            1_000,
        );
        let image = make_test_image();

        let duration = engine
            .execute(&image, 0x4900_0000, 0x4900_0000, 1_500)
            .expect("迁移应成功");

        assert_eq!(duration, 500);
        assert_eq!(engine.context().state, MigrateState::Complete);
        assert_eq!(engine.context().thread_snapshots.len(), 4);
        assert_eq!(engine.context().device_snapshots.len(), 3);
        assert!(engine.context().error.is_none());
    }

    /// MIG2：线程快照字段验证。
    #[test]
    fn mig2_thread_snapshot_fields() {
        let mut platform = NullPlatform::new(2, 0);
        let snapshots = platform.suspend_threads().expect("应成功");

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].thread_id, 0);
        assert_eq!(snapshots[0].state, 1); // 运行态
        assert_eq!(snapshots[1].thread_id, 1);
        assert_eq!(snapshots[1].state, 0); // 就绪态
    }

    /// MIG3：内存映射快照。
    #[test]
    fn mig3_memory_map_snapshot() {
        let mut platform = NullPlatform::new(0, 0);
        let maps = platform.get_memory_map().expect("应成功");

        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].vaddr_start, 0x4000_0000);
        assert_eq!(maps[0].size, 0x1000_0000);
        assert_eq!(maps[1].vaddr_start, 0x0900_0000); // UART
    }

    /// MIG4：设备状态快照。
    #[test]
    fn mig4_device_state_snapshot() {
        let mut platform = NullPlatform::new(0, 5);
        let devices = platform.get_device_states().expect("应成功");

        assert_eq!(devices.len(), 5);
        for (i, dev) in devices.iter().enumerate() {
            assert_eq!(dev.device_id, i as u32);
            assert_eq!(dev.device_type, (i % 5) as u8);
            assert!(dev.initialized);
        }
    }

    /// MIG5：迁移失败状态机。
    #[test]
    fn mig5_migration_failure_state() {
        struct FailPlatform;
        impl PlatformAbstraction for FailPlatform {
            fn suspend_threads(&mut self) -> Result<Vec<ThreadSnapshot>, MigrateError> {
                Err(MigrateError::ThreadMigrationFailed)
            }
            fn resume_threads(&mut self, _: &[ThreadSnapshot]) -> Result<(), MigrateError> {
                Ok(())
            }
            fn get_memory_map(&mut self) -> Result<Vec<MemoryMapSnapshot>, MigrateError> {
                Ok(Vec::new())
            }
            fn apply_memory_map(&mut self, _: &[MemoryMapSnapshot]) -> Result<(), MigrateError> {
                Ok(())
            }
            fn get_device_states(&mut self) -> Result<Vec<DeviceSnapshot>, MigrateError> {
                Ok(Vec::new())
            }
            fn restore_devices(&mut self, _: &[DeviceSnapshot]) -> Result<(), MigrateError> {
                Ok(())
            }
            fn load_kernel(&mut self, _: &[u8], _: u64) -> Result<(), MigrateError> {
                Ok(())
            }
            fn switch_execution(&mut self, _: u64) -> Result<(), MigrateError> {
                Ok(())
            }
        }

        let mut engine = MigrateEngine::new(
            FailPlatform,
            heapless::String::<32>::try_from("0.163.0").unwrap_or_default(),
            1_000,
        );
        let image = make_test_image();

        let err = engine
            .execute(&image, 0x4900_0000, 0x4900_0000, 1_500)
            .expect_err("应失败");

        assert_eq!(err, MigrateError::ThreadMigrationFailed);
        assert_eq!(engine.context().state, MigrateState::Failed);
        assert_eq!(
            engine.context().error,
            Some(MigrateError::ThreadMigrationFailed)
        );
    }

    /// MIG6：迁移上下文耗时计算。
    #[test]
    fn mig6_context_duration() {
        let ctx = MigrateContext::new(
            heapless::String::<32>::try_from("0.163.0").unwrap_or_default(),
            1_000,
        );
        assert_eq!(ctx.duration_ms(), 0); // 未完成

        let mut ctx2 = ctx.clone();
        ctx2.end_time_ms = 2_500;
        assert_eq!(ctx2.duration_ms(), 1_500);
    }
}
