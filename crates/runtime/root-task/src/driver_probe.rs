//! WP-27.2（蓝图/27 §6）：root-task 侧驱动装配——绑定表 + ProbeReport 日志。
//!
//! # 装配链
//! 1. boot-manager 把运行时 DTB 拷贝到 [`ENEROS_DTB_PHYS`]（内核 BOOT-007
//!    映射窗口内，1:1 物理==虚拟）；
//! 2. root-task 从固定物理地址读 FDT 头（magic+totalsize 自校验）→
//!    [`parse_fdt`] → [`DeviceInventory`]；
//! 3. [`BindingTable`]（静态，编译期可审计）`probe_all` → [`ProbeReport`]；
//! 4. 报告以 `ENEROS stage=probe-report` 结构化日志行输出（指标载体）。
//!
//! # 不变量（蓝图/27 §6）
//! - 新增硬件 = 只增不改：在本文件绑定表里加一条 `DriverBinding`；
//! - 生产驱动必须实现 `DeviceDriver` seam（can/rs485 已实现，本 WP 迁入）；
//! - MMIO 映射走 FIX-BUGS-0810 两件套（`SpawnCtx::map_device_mmio`：
//!   `alloc_device_at` 物理校验 + `node_offset=slot`）。

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_boot_protocol::boot_info::{
    dtb_window_for, ENEROS_DTB_MAX_LEN, ENEROS_INIT_BUNDLE_PHYS,
};
use eneros_can::{
    CanConfig, CanControllerType, CanDriver, Sja1000Controller, VolatileMmio,
};
use eneros_driver_framework::{
    BindingTable, DeviceDriver, DriverBinding, DriverError, DriverId, PlatformCtx,
};
use eneros_rs485::{Pl011UartHw, Rs485Config, Rs485Driver};
// 33 WP-33.5：数据盘掉电持久化自检（30 WP-30.7 机制接线；recover/append
// 为 DurableLog trait 方法，需在作用域内）。
use eneros_durable_log::DurableLog;
use eneros_storage::driver::virtio_blk::{
    DmaPool, NoopCache, VirtioBlkDriver, VirtioMmio, VIRTIO_BLK_DEVICE_ID, VIRTIO_MMIO_MAGIC,
    VMMIO_DEVICE_ID, VMMIO_MAGIC_VALUE, VMMIO_STATUS,
};
use eneros_storage::driver::virtio_blk::VolatileMmio as BlkVolatileMmio;
use eneros_storage::StorageConfig;
use eneros_uart::{Pl011Mmio, UartDriver};

/// 绑定表：全部已接入 seam 的生产驱动（编译期可审计）。
///
/// 登记：
/// - D1（WP-27.3）UART：`arm,pl011`（PL011 标准 compatible，QEMU 可得）
/// - D4（WP-27.3）块设备：`virtio,mmio`（QEMU virtio-blk；factory 按
///   device-id 探测分流，D2 virtio-net 同 compatible 后续并入）
/// - CAN：`nxp,sja1000`（SJA1000 控制器标准 compatible；真机 dts 填空项引用）
/// - RS485：`eneros,rs485-uart`（自定义 compatible；后端为 PL011 硬件，
///   语义是 RS485 半双工端口；真机 dts 填空项引用）
///
/// QEMU virt 的 DTB 无 CAN/RS485 节点 → 活体报告中如实列为 unbound（真机
/// 填空项，蓝图/27 §16）；pl011 节点绑定为 UART。
pub fn binding_table() -> BindingTable {
    const ENTRIES: &[DriverBinding] = &[
        DriverBinding {
            compatible: "arm,pl011",
            factory: uart_factory,
        },
        DriverBinding {
            compatible: "virtio,mmio",
            factory: virtio_factory,
        },
        DriverBinding {
            compatible: "nxp,sja1000",
            factory: can_factory,
        },
        DriverBinding {
            compatible: "eneros,rs485-uart",
            factory: rs485_factory,
        },
        // 31 D6：SP805 看门狗（基址经 FDT reg 注入——装配层 0x0905_0000
        // 字面量退役）。
        DriverBinding {
            compatible: "arm,sp805",
            factory: wdt_factory,
        },
        // 31 D5：PL031 RTC（基址经 FDT reg 注入——time_init 占位 0 退役）。
        DriverBinding {
            compatible: "arm,pl031",
            factory: rtc_factory,
        },
    ];
    BindingTable::new(ENTRIES)
}

/// QEMU virt virtio-mmio 区域（32 个槽 × 0x200；blk/net 在低槽，其余空槽）。
const VIRTIO_REGION_BASE: u64 = 0x0a00_0000;
const VIRTIO_REGION_SIZE: u64 = 0x20_0000;
const VIRTIO_MMIO_SLOT_STRIDE: u64 = 0x200;
/// QEMU virt 的 virtio-mmio 槽数（32；探测上限，防越界读）。
const VIRTIO_MMIO_SLOT_COUNT: u64 = 32;

/// root-task DMA 池（静态 9 页 = 36 KiB，virtio_blk 需求下限）。
///
/// 30 RT-019：`static mut` 数组收进私有模块内聚封装——外部只经
/// `RootDmaPool` 方法访问（消除散落的 static mut 引用面）。
mod dma_pool {
    /// root-task 镜像区域 1:1 映射（物理 == 虚拟），`phys_base` = 静态地址。
    ///
    /// # SAFETY（单线程独占不变量）
    /// root-task 单线程装配/事件循环，DmaPool 消费者（virtio_blk）以
    /// `&mut self` 方法独占访问（驱动内部 RefCell 串行化），无并发别名；
    /// SMP 启用前禁止多线程访问本池（与 IRQ_HANDLERS 同登记）。
    #[allow(static_mut_refs)]
    static mut BYTES: [u8; 9 * 4096] = [0u8; 9 * 4096];

    pub(super) const fn len() -> usize {
        9 * 4096
    }
    pub(super) fn phys_base() -> u64 {
        // SAFETY: 静态数组地址恒定；1:1 映射下即 guest 物理地址。
        unsafe { BYTES.as_ptr() as u64 }
    }
    pub(super) fn as_bytes() -> &'static [u8] {
        // SAFETY: 只读借用；单线程 + 驱动独占（&mut self 调用方保证）。
        unsafe { &BYTES }
    }
    pub(super) fn as_bytes_mut() -> &'static mut [u8] {
        // SAFETY: 调用方以 `&mut self` 独占；root-task 单线程无并发。
        unsafe { &mut BYTES }
    }
}

struct RootDmaPool;

impl DmaPool for RootDmaPool {
    fn len(&self) -> usize {
        dma_pool::len()
    }
    fn phys_base(&self) -> u64 {
        dma_pool::phys_base()
    }
    fn as_bytes(&self) -> &[u8] {
        dma_pool::as_bytes()
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        dma_pool::as_bytes_mut()
    }
}

/// D4：VirtioBlkDriver（BlockDevice seam）的 DeviceDriver 适配。
struct BlkAdapter {
    /// 数据盘驱动实例——字段本身无读取方（驱动方法经 DeviceDriver 生命周期
    /// 保持 MMIO 映射与 DMA 池存活；33 WP-33.5 掉电自检在装箱前借用完成）。
    #[allow(dead_code)]
    inner: VirtioBlkDriver<BlkVolatileMmio, RootDmaPool, NoopCache>,
}

// SAFETY: root-task 单线程装配 + 事件循环（无并发访问）；VirtioBlkDriver
// 内部 RefCell 仅在驱动自身方法内短临界使用，不存在跨线程共享。
unsafe impl Send for BlkAdapter {}
unsafe impl Sync for BlkAdapter {}

impl DeviceDriver for BlkAdapter {
    fn id(&self) -> &DriverId {
        static ID: DriverId = DriverId(0xB10C_0001);
        &ID
    }
    fn name(&self) -> &str {
        "virtio-blk0"
    }
    fn driver_type(&self) -> eneros_driver_framework::DriverType {
        eneros_driver_framework::DriverType::Storage
    }
    fn state(&self) -> eneros_driver_framework::DriverState {
        eneros_driver_framework::DriverState::Running
    }
    fn init(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn deinit(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn handle_irq(&mut self, _irq_id: u32) {}
    fn health_check(&self) -> eneros_driver_framework::DriverHealth {
        eneros_driver_framework::DriverHealth::Healthy
    }
}

/// D4：virtio 设备 factory——按 device-id 探测分流（当前：blk；net 留
/// D2）。只认区域起点节点（QEMU 32 个 virtio,mmio 节点中空槽无设备 →
/// NotFound 如实）；区域整体映射（分段 2 MiB 帧）后逐槽探测。
fn virtio_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    let (base, _size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    if base != VIRTIO_REGION_BASE {
        // 非区域起点槽：真实设备只挂在前几个槽；空槽无设备（fail-closed）。
        return Err(DriverError::NotFound);
    }
    // 33 WP-33.5：区域按 4 × 4K 帧分段映射（QEMU 活体实测：单次 2M 帧
    // 映射仅首 4K 生效——槽 8 起读 0xa001000 触发 translation fault；
    // 4K 帧路径已被 D1 pl011 与 D4 槽 0 验证可靠）。32 槽 × 0x200 =
    // 0x4000 恰 4 帧。
    let mut virt = 0u64;
    for i in 0..4u64 {
        let mapped = ctx.map_mmio(base + i * 0x1000, 0x1000)?;
        if i == 0 {
            virt = mapped;
        }
    }
    // BlkVolatileMmio::new 为安全函数（封装内做 volatile 读写）；
    // `virt` 为首帧映射基址（生产 1:1 = base；host mock 为可读模拟寄存器
    // 区）；探测只读寄存器（magic/device-id）。
    let mut probe = BlkVolatileMmio::new(virt as usize);
    // 33 WP-33.5：绑定**最后一块** virtio-blk 作为持久化数据盘（QEMU：
    // 槽 0 = 系统盘、槽 2 = 数据盘；系统盘由固件 BlockIO 处理，root-task
    // 不写系统盘——旧实现绑第一块 blk，DurableLog 写它会破坏 GPT）。
    let mut bound_blk: Option<VirtioBlkDriver<BlkVolatileMmio, RootDmaPool, NoopCache>> = None;
    for slot in 0..VIRTIO_MMIO_SLOT_COUNT {
        let off = (slot * VIRTIO_MMIO_SLOT_STRIDE) as usize;
        let magic = probe.read_reg(off + VMMIO_MAGIC_VALUE);
        crate::println!(
            "[eneros-root-task] DBG virtio probe slot={} magic=0x{:08x}",
            slot,
            magic
        );
        if magic != VIRTIO_MMIO_MAGIC {
            continue; // 空槽
        }
        // QEMU 9.2.4 virtio-mmio 实测：STATUS==0 时 DEVICE_ID 读 0（proxy
        // 未激活）；写 STATUS=ACKNOWLEDGE(1) 后 device_id 才有效（WP-27.3
        // D4 迭代定位）。
        let version = probe.read_reg(off + 0x004);
        probe.write_reg(off + VMMIO_STATUS, 1); // ACKNOWLEDGE
        let device_id = probe.read_reg(off + VMMIO_DEVICE_ID);
        let vendor = probe.read_reg(off + 0x00C);
        crate::println!(
            "[eneros-root-task] DBG virtio probe slot={} version={} device_id={} vendor=0x{:x}",
            slot,
            version,
            device_id,
            vendor
        );
        if device_id == VIRTIO_BLK_DEVICE_ID {
            // 槽内寄存器块（0x200）位于已映射区域内；驱动存续期由装配层
            // 保证。queue DMA 用 RootDmaPool（1:1 物理地址）。
            let mmio = BlkVolatileMmio::new((base + off as u64) as usize);
            let driver = VirtioBlkDriver::new(
                mmio,
                RootDmaPool,
                NoopCache,
                StorageConfig::default(),
            )
            .map_err(|_| DriverError::InitFailed)?;
            // 33 WP-33.5：记录最后一块 blk（数据盘语义；多块时取高槽位）
            bound_blk = Some(driver);
            continue;
        }
        // 有设备但不是 blk（如 rng=4/net=1）→ 跳过继续扫描（33 WP-33.5：
        // 旧实现直接返回——rng 在槽 1 会阻断槽 2 数据盘的到达）。
        // 30 RT-018：探测失败复位 STATUS（virtio 规范 STATUS=0 复位）。
        probe.write_reg(off + VMMIO_STATUS, 0);
    }
    // 33 WP-33.5：数据盘掉电持久化自检（30 WP-30.7 DurableLog 机制接线；
    // 判据行 = 每次掉电后下次启动的 recover 输出 + 启动计数续增）。
    // 借用式（RefMutDevice）——自检后驱动归还适配器。
    match bound_blk {
        Some(mut driver) => {
            match eneros_durable_log::BlockDurableLog::format(
                eneros_durable_log::RefMutDevice(&mut driver),
                512,
            ) {
                Ok(mut log) => match log.recover() {
                    Ok(committed) => {
                        crate::println!(
                            "[eneros-root-task] durable-log: recover committed={}",
                            committed
                        );
                        match log.append(b"boot") {
                            Ok(seq) => {
                                crate::println!(
                                    "[eneros-root-task] durable-log: append seq={} (boot count)",
                                    seq
                                );
                            }
                            Err(e) => {
                                crate::println!(
                                    "[eneros-root-task] durable-log: append failed {:?}",
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        crate::println!(
                            "[eneros-root-task] durable-log: recover failed {:?}",
                            e
                        );
                    }
                },
                Err(e) => {
                    crate::println!(
                        "[eneros-root-task] durable-log: format failed {:?}（判据行缺失，\
                         掉电证据受影响）",
                        e
                    );
                }
            }
            Ok(Box::new(BlkAdapter { inner: driver }))
        }
        None => Err(DriverError::NotFound),
    }
}

/// UART 波特率（D1：115200 8N1 默认；真机按协议层配置）。
const UART_BAUD: u32 = 115_200;
/// PL011 时钟（Hz）。qemu-virt.dts 的 `clk24mhz` 固定 24 MHz；真机按平台
/// dts `clocks` 填空（蓝图/27 §16）。
const PL011_CLOCK_HZ: u32 = 24_000_000;

/// UART 驱动 factory（D1）：`reg[0]` → MMIO 映射 → Pl011Mmio → UartDriver，
/// 并把节点 `interrupts` 首个 SPI 号转为 GIC 中断号绑定（QEMU pl011 =
/// SPI 1 → 中断 33）。
fn uart_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    let (base, size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    let virt = ctx.map_mmio(base, size)?;
    // SAFETY: `virt` 为 `PlatformCtx::map_mmio` 返回的已映射 MMIO 基址
    // （1:1，FIX-BUGS-0810 两件套保证映射成功 ⇔ 正确物理页）；`Pl011Mmio`
    // 生命周期由装配层（root-task 常驻）保证。
    let mmio = unsafe { Pl011Mmio::new(virt as *mut u8) };
    let mut driver = UartDriver::new(
        DriverId(0x0A11_0001),
        "pl011-uart0",
        Box::new(mmio),
        PL011_CLOCK_HZ,
        UART_BAUD,
    );
    // IRQ 接线：`interrupts` = <GIC_SPI 号 触发类型>（QEMU pl011: <0 1 4>）
    // → GIC SPI 全局中断号 = 32 + 号。
    if let Some(irq) = node
        .interrupts
        .chunks_exact(3)
        .find(|cell| cell[0] == 0)
        .map(|cell| 32 + cell[1])
    {
        // 绑定失败不阻断装配（驱动仍可轮询收发；IRQ 收包缺失如实记录）。
        if let Ok(badge) = ctx.bind_irq(irq) {
            driver.set_irq_badge(badge);
        }
    }
    Ok(Box::new(driver))
}

/// 31 D6：SP805 看门狗工厂——基址经 FDT reg 注入并注册到 panic 全局
///（软件模式语义不变，30 RT-001 守约）。
fn wdt_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    let (base, size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    let virt = ctx.map_mmio(base, size)?;
    // 31-review：注册**虚拟**地址（map_mmio 返回值，非物理 base——与 can_factory
    // 同模式；panic 路径经此 VSpace 地址写 SP805 寄存器）+ 武装 WDT_MMIO_MAPPED
    //（旧实现注册物理 base 且不置 MAPPED，致真机 panic 路径永不武装 HW 看门狗）。
    crate::panic::register_wdt_base(virt);
    crate::panic::WDT_MMIO_MAPPED.store(true, core::sync::atomic::Ordering::Relaxed);
    Ok(Box::new(NoopDriver)) // 事件循环不消费 WDT 中断——映射+武装即装配
}

/// 31 D5：PL031 RTC 工厂——基址经 FDT reg 注入并注册（time_init 装配）。
fn rtc_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    let (base, size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    let virt = ctx.map_mmio(base, size)?;
    // 31-review：注册虚拟地址（map_mmio 返回值，非物理 base）——
    // set_rtc_base 的 Pl031Rtc 经此 VSpace 地址读 RTC 秒计数。
    crate::panic::register_rtc_base(virt);
    Ok(Box::new(NoopDriver))
}

/// SJA1000 晶振频率（Hz）。真机填空项：应从 dts `clocks`/`clock-frequency`
/// 读取（蓝图/27 §16 平台 dts 节点补全）；仓库无解析前用典型 16 MHz。
const SJA1000_CLOCK_HZ: u32 = 16_000_000;

/// CAN 驱动 factory：`reg[0]` → MMIO 映射 → SJA1000 控制器 → CanDriver。
fn can_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    let (base, size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    let virt = ctx.map_mmio(base, size)?;
    // SAFETY: `virt` 为 `PlatformCtx::map_mmio` 返回的已映射 MMIO 基址
    // （1:1，FIX-BUGS-0810 两件套保证映射成功 ⇔ 正确物理页）；`VolatileMmio`
    // 仅做 volatile 读写，生命周期由装配层（root-task 常驻）保证。
    let mmio = unsafe { VolatileMmio::new(virt as *mut u8) };
    let controller = Sja1000Controller::new(mmio, SJA1000_CLOCK_HZ, eneros_time::get_monotonic_ns);
    let mut config = CanConfig::default();
    config.controller_type = CanControllerType::SJA1000;
    Ok(Box::new(CanDriver::new(
        DriverId(0xCA11_0001),
        config,
        Box::new(controller),
    )))
}

/// RS485 驱动 factory：`reg[0]` → MMIO 映射 → PL011 后端 → Rs485Driver。
fn rs485_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    let (base, size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    let virt = ctx.map_mmio(base, size)?;
    // SAFETY: `virt` 为 `PlatformCtx::map_mmio` 返回的已映射 MMIO 基址
    // （1:1，FIX-BUGS-0810 两件套保证映射成功 ⇔ 正确物理页）；`Pl011UartHw`
    // 生命周期由装配层（root-task 常驻）保证。
    let uart = unsafe { Pl011UartHw::new(virt, PL011_CLOCK_HZ, Box::new(eneros_time::get_monotonic_ns)) };
    Ok(Box::new(Rs485Driver::new(
        DriverId(0x4850_0001),
        Rs485Config::default(),
        Box::new(uart),
    )))
}

/// 一条已接线的驱动 IRQ：事件循环 poll ntfn → 驱动 `handle_irq` → ack。
#[cfg(target_os = "none")]
pub struct IrqDriver {
    /// 中断号（GIC 全局号）。
    pub irq: u32,
    /// IRQ 绑定链（ntfn_slot/handler_cap/badge）。
    pub binding: crate::irq::IrqBinding,
    /// 驱动（handle_irq 收包）。
    pub driver: Box<dyn DeviceDriver>,
}

/// root-task 装配平台资源：SpawnCtx（MMIO，FIX-BUGS-0810 两件套）+ IrqTable。
///
/// host 构建不可用（无 seL4）——host 测试用 mock ctx 直接调 factory。
#[cfg(target_os = "none")]
pub struct RootPlatformCtx<'a> {
    spawn: &'a mut crate::spawn::SpawnCtx,
    irq_table: &'a mut crate::irq::IrqTable,
    /// factory 经 `bind_irq` 建立的 (irq, badge) 记录（顺序与 bound 对齐）。
    bindings: alloc::vec::Vec<(u32, u32)>,
    /// 33 WP-33.5：最近一次 bind_irq 的 badge（probe_all 取走写入
    /// BoundEntry.irq_badge——索引配对在未绑 IRQ 的驱动存在时错位）。
    last_badge: Option<u32>,
}

#[cfg(target_os = "none")]
impl<'a> RootPlatformCtx<'a> {
    pub fn new(
        spawn: &'a mut crate::spawn::SpawnCtx,
        irq_table: &'a mut crate::irq::IrqTable,
    ) -> Self {
        Self {
            spawn,
            irq_table,
            bindings: alloc::vec::Vec::new(),
            last_badge: None,
        }
    }
}

/// size_bytes → seL4 arm64 帧 size_bits（**合法值 12/21/30**——arm64 无
/// 64K 用户帧，64K 用 2 MiB 块覆盖；WP-27.3 D1 QEMU 实测 size_bits=16
/// 在 Page_Map 中与页表槽粒度不匹配）。超出 1 GiB 拒绝。
#[cfg(target_os = "none")]
fn frame_size_bits(size_bytes: u64) -> Result<u64, DriverError> {
    // 30 RT-017：按实际尺寸选帧（4KiB 设备 → 12-bit 帧）——旧实现 ≤2MiB
    // 一律 21-bit，4KiB 设备基址恰 2M 对齐时过度映射（浪费 511/512 内存
    // 且扩大 MMIO 暴露面）。层级：4K → 2M → 1G（arm64 合法 size_bits）。
    let size = size_bytes.max(0x1000);
    for (limit, bits) in [(0x1000u64, 12u64), (0x20_0000, 21), (0x4000_0000, 30)] {
        if size <= limit {
            return Ok(bits);
        }
    }
    Err(DriverError::NotFound)
}

/// arm64 帧位宽层级（降级用）。
#[cfg(target_os = "none")]
const FRAME_BITS_4K: u64 = 12;
#[cfg(target_os = "none")]
const FRAME_BITS_2M: u64 = 21;
#[cfg(target_os = "none")]
const FRAME_BITS_1G: u64 = 30;

#[cfg(target_os = "none")]
impl PlatformCtx for RootPlatformCtx<'_> {
    fn map_mmio(&mut self, phys: u64, size_bytes: u64) -> Result<u64, DriverError> {
        // WP-27.3 D4：大区域（如 virtio-mmio 32 MiB）按合法帧分段映射
        //（1:1，虚拟连续）；帧位宽按 phys 对齐度降级（1GiB → 2MiB → 4K）。
        let mut remaining = size_bytes;
        let mut cursor = phys;
        while remaining > 0 {
            let mut bits = frame_size_bits(remaining)?;
            while bits > FRAME_BITS_4K
                && (cursor & ((1u64 << bits) - 1)) != 0
            {
                bits = if bits == FRAME_BITS_1G {
                    FRAME_BITS_2M
                } else {
                    FRAME_BITS_4K
                };
            }
            crate::spawn::map_device_mmio(self.spawn, cursor, bits)
                .map_err(|_| DriverError::NotFound)?;
            let mapped = 1u64 << bits;
            cursor = cursor
                .checked_add(mapped)
                .ok_or(DriverError::NotFound)?;
            remaining = remaining.saturating_sub(mapped);
        }
        Ok(phys)
    }

    fn bind_irq(&mut self, irq: u32) -> Result<u32, DriverError> {
        // IrqTable::bind 返回 badge_bit（= 绑定索引 + 1；0 保留未知）。
        let badge = self
            .irq_table
            .bind(self.spawn, irq)
            .map(|badge| badge as u32)
            .map_err(|_| DriverError::NotFound)?;
        self.bindings.push((irq, badge));
        // 33 WP-33.5：记录最近 badge（probe_all 经 take_last_badge 写入
        // BoundEntry.irq_badge）。
        self.last_badge = Some(badge);
        Ok(badge)
    }

    fn take_last_badge(&mut self) -> Option<u32> {
        self.last_badge.take()
    }
}

/// 从 BOOT-007 窗口取运行时 DTB 切片（**不拷贝**——root-task 堆仅 1 MiB，
/// QEMU virt 运行时 DTB ≈ 1 MiB + 4 KiB 且内容仅 ~7.5 KB（其余为对齐
/// 填充），整拷贝会 OOM（WP-27.2 QEMU 实测 layout=size:1052672））。
///
/// DTB 起点 = bundle 末端页对齐（`dtb_window_for(bundle_len)`，与
/// boot-manager 拷贝处同推导——boot-manager 已交叉校验 bundle header
/// `total_len == init.media_length`，故 root-task 用已解析的 bundle
/// `total_len` 推导不会漂移）。FDT 头 magic+totalsize 自校验，fail-closed。
///
/// 返回的切片指向 BOOT-007 常驻映射窗口（root-task 生命周期内有效），
/// 解析器（`parse_fdt`）直接在窗口内存上工作，仅节点/属性分配走堆。
#[cfg(target_os = "none")]
pub(crate) fn runtime_dtb_slice(bundle_len: u64) -> Option<(&'static [u8], u64)> {
    let (dtb_phys, window) = dtb_window_for(bundle_len)?;
    let header_ptr = dtb_phys as *const u8;
    let mut header = [0u8; 8];
    // SAFETY: `dtb_phys` 位于内核 BOOT-007 映射窗口（1:1 映射进初始线程
    // VSpace），8 字节头读取不越界（窗口 ≥ 8 由 dtb_window_for 保证）。
    unsafe {
        core::ptr::copy_nonoverlapping(header_ptr, header.as_mut_ptr(), 8);
    }
    if header[0] != 0xd0 || header[1] != 0x0d || header[2] != 0xfe || header[3] != 0xed {
        return None;
    }
    let total = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as u64;
    // 双边界：单 DTB 硬上限 + 窗口剩余空间。
    if total < 8 || total > ENEROS_DTB_MAX_LEN || total > window {
        return None;
    }
    // SAFETY: `dtb_phys` 为 BOOT-007 常驻映射窗口内地址（root-task 存续期
    // 有效）；`total` 受窗口与硬上限双重约束，切片不越出映射窗口。
    let blob = unsafe { core::slice::from_raw_parts(header_ptr, total as usize) };
    Some((blob, dtb_phys))
}

/// 30 RT-008：从运行时 DTB 提取 arch timer 虚拟中断号（零硬件依赖——
/// 原 main.rs 硬编码 27）。FDT `arm,armv8-timer` 节点 interrupts 属性按
/// arm 规范为 4 个三元组（phys-secure / **phys-nonsecure** / virt / hyp，
/// GIC 编码 [type, ppi, flags]）。
///
/// 33 WP-33.5：取 **phys-nonsecure（第 2 个三元组）** 而非 virt——seL4
/// 内核无 hypervisor 配置下 KERNEL_TIMER_IRQ=27（虚拟定时器）已被占用，
/// QEMU 活体绑定 27 返回 seL4_RevokeFirst（实测 `IRQ bind failed:
/// Sel4(Other(9))`）；phys PPI 14 → 30 空闲且 EL1 可驱动 CNTP。
pub fn timer_irq_from_dtb(dtb: &[u8]) -> Option<u32> {
    let inventory = eneros_board::device_tree::parse_fdt(dtb).ok()?;
    let timer = inventory.find_by_compatible("arm,armv8-timer")?;
    let triples: alloc::vec::Vec<&[u32]> = timer.interrupts.chunks_exact(3).collect();
    // 第 2 个三元组 = phys-nonsecure timer（QEMU virt/arm 规范顺序）；
    // PPI 编码 [1, ppi, flags]（type=1）。
    let phys = triples.get(1)?;
    if phys[0] == 1 {
        Some(phys[1].checked_add(16)?)
    } else {
        None
    }
}

/// 装配并打印 ProbeReport（root-task 启动日志；指标 = 结构化数值字段），
/// 返回已接线的驱动 IRQ 列表（事件循环 poll → handle_irq → ack）。
///
/// 任一步失败（无 DTB/解析失败）→ 打印 fail 行并继续（不挂起：驱动缺失
/// 不应阻断 EL0 服务 spawn；真机 bring-up 依赖报告定位缺口）。
#[cfg(target_os = "none")]
pub fn probe_and_report(
    spawn: &mut crate::spawn::SpawnCtx,
    irq_table: &mut crate::irq::IrqTable,
    bundle_len: u64,
) -> alloc::vec::Vec<IrqDriver> {
    use eneros_board::device_tree::parse_fdt;

    let Some((blob, dtb_phys)) = runtime_dtb_slice(bundle_len) else {
        crate::println!(
            "[eneros-root-task] ENEROS stage=probe-report error=no-runtime-dtb bundle_len={}",
            bundle_len
        );
        return alloc::vec::Vec::new();
    };
    let inventory = match parse_fdt(&blob) {
        Ok(inv) => inv,
        Err(e) => {
            crate::println!(
                "[eneros-root-task] ENEROS stage=probe-report error=fdt-parse {:?} bytes={}",
                e,
                blob.len()
            );
            return alloc::vec::Vec::new();
        }
    };
    crate::println!(
        "[eneros-root-task] ENEROS stage=probe-report dtb-phys=0x{:x} dtb-bytes={} nodes={} compatibles={:?}",
        dtb_phys,
        blob.len(),
        inventory.len(),
        binding_table().compatibles()
    );
    let mut ctx = RootPlatformCtx::new(spawn, irq_table);
    let report = binding_table().probe_all(&inventory, &mut ctx);
    for bound in report.bound() {
        crate::println!(
            "[eneros-root-task]   probe bound compatible={} node={} driver={}",
            bound.compatible,
            bound.node,
            bound.driver.name()
        );
    }
    for unbound in report.unbound() {
        crate::println!(
            "[eneros-root-task]   probe unbound compatible={} node={}",
            unbound.compatible,
            unbound.node
        );
    }
    for failed in report.failed() {
        crate::println!(
            "[eneros-root-task]   probe failed compatible={} node={} error={:?}",
            failed.compatible,
            failed.node,
            failed.error
        );
    }
    crate::println!(
        "[eneros-root-task] ENEROS stage=probe-report-done {}",
        report.metrics_line()
    );
    // 组装 IRQ 接线（30 RT-009 后 33 WP-33.5 修正：索引配对在"未绑 IRQ
    // 的驱动"存在时错位——QEMU 活体实测 pl011 的 IRQ 33 被接到
    // virtio-blk0（bound[0] 未绑 IRQ）。现按 badge 配对：factory 经
    // bind_irq 绑定的 badge 由 probe_all 写入 BoundEntry.irq_badge，
    // 与 ctx.bindings 的 (irq, badge) 精确匹配；匹配不到 = 装配异常
    // 打印判据行并跳过（fail-closed，不静默错位）。
    let mut irq_drivers = alloc::vec::Vec::new();
    let mut bound = report.into_bound();
    for (irq, badge) in ctx.bindings.iter().copied() {
        // badge 解码防御：badge = 绑定索引 + 1（0 保留未知）——回绕防护。
        let Some(binding) = badge_to_binding_index(badge)
            .and_then(|idx| irq_table.binding(idx))
        else {
            crate::println!(
                "[eneros-root-task]   probe irq-wire badge decode FAILED badge={} — 跳过",
                badge
            );
            continue;
        };
        // 按 badge 找持有该绑定的驱动（BoundEntry.irq_badge）。
        let Some(idx) = bound
            .iter()
            .position(|e| e.irq_badge == Some(badge))
        else {
            crate::println!(
                "[eneros-root-task]   probe irq-wire NO-DRIVER badge={} irq={} — 跳过",
                badge,
                irq
            );
            continue;
        };
        let entry = &mut bound[idx];
        crate::println!(
            "[eneros-root-task]   probe irq-wired driver={} irq={} badge={}",
            entry.driver.name(),
            irq,
            badge
        );
        // 取出驱动所有权（BoundEntry 其余字段丢弃）。
        let driver = core::mem::replace(&mut entry.driver, Box::new(NoopDriver));
        irq_drivers.push(IrqDriver {
            irq,
            binding,
            driver,
        });
    }
    irq_drivers
}

/// 30 RT-009：badge → 绑定索引解码（badge = 绑定索引 + 1，0 保留未知）。
/// `badge=0` 返回 None（防 u32 回绕成 u32::MAX 索引）。
fn badge_to_binding_index(badge: u32) -> Option<usize> {
    badge.checked_sub(1).map(|b| b as usize)
}

/// 占位驱动（BoundEntry 消费时的替身；永不使用——BoundEntry 随即丢弃）。
/// 31 D5/D6：WDT/RTC 工厂返回本占位（注册即装配）；host 编译亦需要。
struct NoopDriver;

impl DeviceDriver for NoopDriver {
    fn id(&self) -> &DriverId {
        static ID: DriverId = DriverId(0);
        &ID
    }
    fn name(&self) -> &str {
        "noop"
    }
    fn driver_type(&self) -> eneros_driver_framework::DriverType {
        eneros_driver_framework::DriverType::Custom(0)
    }
    fn state(&self) -> eneros_driver_framework::DriverState {
        eneros_driver_framework::DriverState::Dead
    }
    fn init(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn deinit(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn handle_irq(&mut self, _irq_id: u32) {}
    fn health_check(&self) -> eneros_driver_framework::DriverHealth {
        eneros_driver_framework::DriverHealth::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::badge_to_binding_index;

    /// 30 RT-009 判据：badge 解码（绑定索引+1 语义；0 保留未知必须拒绝——
    /// 旧实现 `badge - 1` 在 badge=0 时 u32 回绕成 u32::MAX 索引）。
    #[test]
    fn badge_to_index_decode() {
        assert_eq!(badge_to_binding_index(1), Some(0));
        assert_eq!(badge_to_binding_index(2), Some(1));
        assert_eq!(badge_to_binding_index(17), Some(16));
        assert_eq!(badge_to_binding_index(0), None, "badge=0（未知）必须拒绝");
    }

    use super::*;
    use eneros_board::device_tree::DeviceNode;

    /// 记录调用轨迹的 mock ctx（host 测试：factory 装配路径无 seL4）。
    #[derive(Default)]
    struct MockCtx {
        mapped: Vec<(u64, u64)>,
    }

    impl PlatformCtx for MockCtx {
        fn map_mmio(&mut self, phys: u64, size_bytes: u64) -> Result<u64, DriverError> {
            self.mapped.push((phys, size_bytes));
            // 返回**可读**模拟寄存器内存（全零 = 空槽 magic）：virtio factory
            // 探测会读 MMIO，返回 phys 会导致 host 测试 SIGSEGV（WP-27.3 D4）；
            // u64 数组保证 8 字节对齐（read_volatile u32 对齐要求）。
            static SIM_REGISTERS: [u64; 2048] = [0u64; 2048];
            Ok(SIM_REGISTERS.as_ptr() as u64)
        }
        fn bind_irq(&mut self, _irq: u32) -> Result<u32, DriverError> {
            Ok(1)
        }
    }

    fn node(name: &str, compat: &str, base: u64, size: u64) -> DeviceNode {
        DeviceNode {
            name: name.into(),
            compatible: compat.into(),
            regs: alloc::vec![(base, size)],
            interrupts: alloc::vec![],
        }
    }

    /// can/rs485 经绑定表实例化（host 构造级证据；QEMU 无节点 → 真机填空）。
    #[test]
    fn can_and_rs485_instantiate_via_binding_table() {
        let table = binding_table();
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![
                node("can@10000000", "nxp,sja1000", 0x1000_0000, 0x100),
                node("rs485@9000000", "eneros,rs485-uart", 0x0900_0000, 0x1000),
            ],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 2, "can + rs485 都应绑定");
        assert_eq!(report.unbound_count(), 0);
        assert_eq!(report.failed_count(), 0);
        assert_eq!(report.bound()[0].compatible, "nxp,sja1000");
        assert_eq!(report.bound()[0].driver.name(), "can-sja1000");
        assert_eq!(report.bound()[1].compatible, "eneros,rs485-uart");
        assert_eq!(report.bound()[1].driver.name(), "rs485-uart0");
        // MMIO 经 ctx 装配（1:1 虚拟基址）
        assert_eq!(ctx.mapped, alloc::vec![(0x1000_0000, 0x100), (0x0900_0000, 0x1000)]);
    }

    /// 无 reg 属性的节点 → failed（fail-closed，不静默构造）。
    #[test]
    fn missing_reg_fails_binding() {
        let table = binding_table();
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![DeviceNode {
                name: "can@0".into(),
                compatible: "nxp,sja1000".into(),
                regs: alloc::vec![],
                interrupts: alloc::vec![],
            }],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 0);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.failed()[0].error, DriverError::NotFound);
    }

    /// D4：virtio 非区域起点槽（空槽/后续槽）→ NotFound（fail-closed）。
    #[test]
    fn virtio_non_region_start_fails_closed() {
        let table = binding_table();
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![
                // QEMU 32 个 virtio,mmio 节点中的空槽（base ≠ 区域起点）
                node("virtio_mmio@a000200", "virtio,mmio", 0x0a00_0200, 0x200),
            ],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 0);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.failed()[0].error, DriverError::NotFound);
        // 区域起点在 mock（无真实设备）下同样 NotFound——不静默构造
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![node("virtio_mmio@a000000", "virtio,mmio", 0x0a00_0000, 0x200)],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 0);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.failed()[0].error, DriverError::NotFound);
    }

    /// 30 RT-008 判据：真实 qemu-virt.dtb → timer IRQ = 27（虚拟 timer
    /// PPI 11 + 16；与 WP-26.8 基线一致）；破坏 DTB → None（fallback 路径）。
    #[test]
    fn timer_irq_from_dtb_returns_qemu_phys_30() {
        const QEMU_VIRT_DTB: &[u8] =
            include_bytes!("../../../hal/board/tests/fixtures/qemu-virt.dtb");
        assert_eq!(
            timer_irq_from_dtb(QEMU_VIRT_DTB),
            Some(30),
            "QEMU virt phys-nonsecure timer = PPI 14 → IRQ 30（33 WP-33.5 判据；\
             虚拟 timer 27 被 seL4 内核占用）"
        );
        assert_eq!(timer_irq_from_dtb(b"not-a-dtb"), None);
        assert_eq!(timer_irq_from_dtb(&[]), None);
    }

    /// 真实 qemu-virt.dtb fixture：pl011 绑定为 UART（D1），其余如实 unbound。
    #[test]
    fn qemu_virt_dtb_probe_binds_pl011_and_reports_rest_unbound() {
        use eneros_board::device_tree::parse_fdt;
        const QEMU_VIRT_DTB: &[u8] =
            include_bytes!("../../../hal/board/tests/fixtures/qemu-virt.dtb");
        let inventory = parse_fdt(QEMU_VIRT_DTB).expect("fixture dtb 必须可解析");
        let mut ctx = MockCtx::default();
        let report = binding_table().probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 1, "pl011 绑定为 UART");
        assert_eq!(report.failed_count(), 0);
        assert_eq!(report.bound()[0].compatible, "arm,pl011");
        assert_eq!(report.bound()[0].driver.name(), "pl011-uart0");
        // qemu-virt.dts 的其余设备节点都应在 unbound 清单中（节点一致）
        let unbound: Vec<&str> = report.unbound().iter().map(|u| u.node.as_str()).collect();
        assert!(
            unbound.contains(&"uart@9000000") == false,
            "pl011 已绑定，不应在 unbound: {unbound:?}"
        );
        assert!(
            report
                .unbound()
                .iter()
                .any(|u| u.compatible == "arm,armv8-timer"),
            "timer 节点必须在报告中"
        );
        assert!(
            report
                .unbound()
                .iter()
                .any(|u| u.compatible == "arm,gic-v3"),
            "gic 节点必须在报告中"
        );
    }
}
