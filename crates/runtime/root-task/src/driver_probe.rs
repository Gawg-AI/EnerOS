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
use eneros_uart::{Pl011Mmio, UartDriver};

/// 绑定表：全部已接入 seam 的生产驱动（编译期可审计）。
///
/// 登记：
/// - D1（WP-27.3）UART：`arm,pl011`（PL011 标准 compatible，QEMU 可得）
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
            compatible: "nxp,sja1000",
            factory: can_factory,
        },
        DriverBinding {
            compatible: "eneros,rs485-uart",
            factory: rs485_factory,
        },
    ];
    BindingTable::new(ENTRIES)
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
        }
    }
}

/// size_bytes → seL4 arm64 帧 size_bits（**合法值 12/21/30**——arm64 无
/// 64K 用户帧，64K 用 2 MiB 块覆盖；WP-27.3 D1 QEMU 实测 size_bits=16
/// 在 Page_Map 中与页表槽粒度不匹配）。超出 1 GiB 拒绝。
#[cfg(target_os = "none")]
fn frame_size_bits(size_bytes: u64) -> Result<u64, DriverError> {
    let size = size_bytes.max(0x1000);
    for (limit, bits) in [(0x20_0000u64, 21u64), (0x4000_0000, 30)] {
        if size <= limit {
            return Ok(bits);
        }
    }
    Err(DriverError::NotFound)
}

#[cfg(target_os = "none")]
impl PlatformCtx for RootPlatformCtx<'_> {
    fn map_mmio(&mut self, phys: u64, size_bytes: u64) -> Result<u64, DriverError> {
        let size_bits = frame_size_bits(size_bytes)?;
        crate::spawn::map_device_mmio(self.spawn, phys, size_bits)
            .map_err(|_| DriverError::NotFound)?;
        // 1:1 映射（arm_page_map 以 paddr 为虚拟地址），虚拟基址 = 物理基址。
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
        Ok(badge)
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
fn runtime_dtb_slice(bundle_len: u64) -> Option<(&'static [u8], u64)> {
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
    // 组装 IRQ 接线：bound 驱动顺序与 ctx.bindings 顺序对齐（每个 factory
    // 至多 bind 一次；未 bind 的驱动无 IRQ 条目 → 轮询模式）。
    let mut irq_drivers = alloc::vec::Vec::new();
    for (bound, (irq, badge)) in report.into_bound().iter_mut().zip(ctx.bindings.iter()) {
        let Some(binding) = irq_table.binding((*badge - 1) as usize) else {
            continue;
        };
        crate::println!(
            "[eneros-root-task]   probe irq-wired driver={} irq={} badge={}",
            bound.driver.name(),
            irq,
            badge
        );
        // 取出驱动所有权（BoundEntry 其余字段丢弃）。
        let driver = core::mem::replace(&mut bound.driver, Box::new(NoopDriver));
        irq_drivers.push(IrqDriver {
            irq: *irq,
            binding,
            driver,
        });
    }
    irq_drivers
}

/// 占位驱动（BoundEntry 消费时的替身；永不使用——BoundEntry 随即丢弃）。
#[cfg(target_os = "none")]
struct NoopDriver;

#[cfg(target_os = "none")]
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
            Ok(phys)
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
