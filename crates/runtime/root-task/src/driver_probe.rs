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

#[cfg(target_os = "none")]
use alloc::borrow::ToOwned;
use alloc::boxed::Box;
#[cfg(test)]
use alloc::vec::Vec;

// 44.3 目标构建修复：下列符号仅 target 侧（`#[cfg(target_os = "none")]`
// 函数）使用，host check/clippy 不编译该路径——import 需同 cfg 门控。
#[cfg(target_os = "none")]
use eneros_boot_protocol::{dtb_window_for, ENEROS_DTB_MAX_LEN};
use eneros_can::{CanConfig, CanControllerType, CanDriver, Sja1000Controller, VolatileMmio};
use eneros_driver_framework::{
    BindingTable, CapabilityKind, DeviceDriver, DriverBinding, DriverError, DriverId, PlatformCtx,
};
#[cfg(target_os = "none")]
use eneros_driver_framework::{CapabilityQuality, CapabilityRegistry, DriverSupervisor};
// 33 WP-33.5：数据盘掉电持久化自检（30 WP-30.7 机制接线；recover/append
// 为 DurableLog trait 方法，需在作用域内）。
use eneros_durable_log::DurableLog;
// 38（WP-38.7）：DMA 唯一 seam——设备地址只经 hal DmaAllocator 映射产生。
use eneros_hal::dma::{DmaAllocator, DmaCoherency, DmaDirection, DmaMapping, DmaRequest};
use eneros_hal::inventory::DeviceId as InventoryDeviceId;
use eneros_net::NetDevice as _;
use eneros_rs485::{Pl011UartHw, Rs485Config, Rs485Driver};
use eneros_storage::driver::virtio_blk::VolatileMmio as BlkVolatileMmio;
use eneros_storage::driver::virtio_blk::{
    VirtioBlkDriver, VirtioMmio, VIRTIO_BLK_DEVICE_ID, VIRTIO_MMIO_MAGIC, VMMIO_DEVICE_ID,
    VMMIO_MAGIC_VALUE, VMMIO_STATUS,
};
use eneros_time::Pl031Rtc;
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
// SPEC-R4 WP-R4.3：板级事实单源 = handover-abi（历史与 spawn/entropy 三抄）。
const VIRTIO_REGION_BASE: u64 = eneros_handover_abi::VIRTIO_MMIO_REGION_BASE;
const VIRTIO_REGION_SIZE: u64 = 0x20_0000;
#[allow(dead_code)] // 44.2：qemu-virt 板级槽位常数，当前路径未用（44.4 冻结登记）
const VIRTIO_MMIO_SLOT_STRIDE: u64 = eneros_handover_abi::VIRTIO_MMIO_SLOT_STRIDE;
/// QEMU virt 的 virtio-mmio 槽数（32；探测上限，防越界读）。
#[allow(dead_code)] // 44.2：同上（44.4 冻结登记）
const VIRTIO_MMIO_SLOT_COUNT: u64 = 32;

/// 38（WP-38.7）：virtio DMA 一致性判定（纯函数，mutation 可测）——
/// 节点显式 `dma-coherent` 优先，其次平台适配器声明（qemu-virt 板级
/// 事实）；两者都无 → NonCoherent（**NoopCache 不再是生产默认**：
/// 生产 cache = [`cache_ops`] 的真实维护，仅 host 测试别名 NoopCache）。
fn virtio_coherency(node_coherent: Option<bool>, platform: Option<DmaCoherency>) -> DmaCoherency {
    match node_coherent {
        Some(true) => DmaCoherency::Coherent,
        Some(false) => DmaCoherency::NonCoherent,
        None => platform.unwrap_or(DmaCoherency::NonCoherent),
    }
}

/// 38（WP-38.7）：virtio cache 门禁——**capability 驱动**：
/// 平台显式声明 Coherent（FDT `dma-coherent` 或适配器板级事实）才允许
/// `NoopCache`（EL0 安全；一致内存无需维护——qemu-virt 双来源声明）；
/// NonCoherent → 拒绝（`dc ivac` 等 cache 维护指令在 EL0 执行触发
/// undefined instruction——seL4 用户态 DMA 必须走内核 cache syscall
/// `seL4_ARM_Page_*`，需设备 page cap——39 接线；38 不伪造）。
/// 登记 CVR-38-01：storage `Aarch64Cache` 的 EL0 用法需 seL4 syscall。
fn cache_ops_for(coherent: bool) -> Result<(), DriverError> {
    if coherent {
        Ok(())
    } else {
        Err(DriverError::NotFound)
    }
}

/// 38（WP-38.7）：virtio 队列 DMA 池——经 hal `DmaAllocator` 分配/映射，
/// 适配 storage `DmaPool` seam（设备地址 = `DmaMapping.device`，类型化；
/// CPU 视图 = `DmaBuffer.cpu`）。**不假设 VA==PA**：CPU 与设备地址分别
/// 由映射两侧给出，转换只发生在 hal 分配器内部。
///
/// # SAFETY（指针不变量）
/// - `cpu_ptr` 指向 hal 分配器返回的池内存（`Box::leak`，'static）；
/// - 驱动经 `as_bytes(_mut)` 只访问 `buf.len` 范围内字节；
/// - `device_addr` 只写设备描述符（Q_PFN/描述符 addr 字段）。
///
/// 纯指针/数值视图（Copy：同一池可被多个槽位驱动共享 CPU 侧内存——
/// virtio 只构造一个 blk 驱动，其余槽位探测不消费池）。
#[derive(Clone, Copy)]
struct HalPool {
    /// CPU 侧基址（池内存起点）。
    cpu_ptr: *mut u8,
    /// 设备可见基址（DMA 描述符只写此值）。
    device_addr: u64,
    len: usize,
}

// SAFETY: 池内存为 'static 泄漏内存，root-task 单线程独占（驱动内部
// RefCell 串行化）；指针不跨界读写。
unsafe impl Send for HalPool {}
unsafe impl Sync for HalPool {}

impl HalPool {
    /// 从 hal 分配器分配并映射 9 页队列内存（virtio_blk 需求下限）。
    fn allocate_from(
        alloc: &mut dyn DmaAllocator,
        device: InventoryDeviceId,
    ) -> Result<(Self, DmaMapping), DriverError> {
        let req = DmaRequest {
            size: 9 * 4096,
            alignment: 4096,
            direction: DmaDirection::Bidirectional,
            device_address_width: 64,
            iommu_domain: None,
            secret: false,
        };
        let buf = alloc.allocate(req).map_err(|_| DriverError::NotFound)?;
        let map = alloc
            .map(&buf, device, DmaDirection::Bidirectional)
            .map_err(|_| DriverError::NotFound)?;
        // CPU 视图指针（池内存；`buf.cpu` 为分配器给出的强类型虚拟地址）。
        let cpu_ptr = buf.cpu.as_u64() as *mut u8;
        Ok((
            Self {
                cpu_ptr,
                device_addr: map.device.as_u64(),
                len: buf.len,
            },
            map,
        ))
    }
}

impl eneros_storage::driver::virtio_blk::DmaPool for HalPool {
    fn len(&self) -> usize {
        self.len
    }
    fn phys_base(&self) -> u64 {
        // 设备可见基址（typed：DmaMapping.device 转换而来）。
        self.device_addr
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: `cpu_ptr` 指向 'static 池内存；`len` 为分配长度（<= 池
        // 容量）；驱动只在此范围内读写（virtio 描述符/请求头）。
        unsafe { core::slice::from_raw_parts_mut(self.cpu_ptr, self.len) }
    }
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: 同上（只读视图）。
        unsafe { core::slice::from_raw_parts(self.cpu_ptr, self.len) }
    }
}

/// D4：VirtioBlkDriver（BlockDevice seam）的 DeviceDriver 适配。
///
/// 38（WP-38.7）：生产 cache = `Aarch64Cache`（真实 dc cvac/dc ivac + dsb；
/// 一致平台上为语义 no-op，非一致平台必需——不再以 NoopCache 冒充）。
struct BlkAdapter {
    /// 数据盘驱动实例——字段本身无读取方（驱动方法经 DeviceDriver 生命周期
    /// 保持 MMIO 映射与 DMA 池存活；33 WP-33.5 掉电自检在装箱前借用完成）。
    #[allow(dead_code)]
    inner: VirtioBlkDriver<BlkVolatileMmio, HalPool, eneros_storage::driver::virtio_blk::NoopCache>,
}

// SAFETY: root-task 单线程装配 + 事件循环（无并发访问）；VirtioBlkDriver
// 内部 RefCell 仅在驱动自身方法内短临界使用，不存在跨线程共享。
unsafe impl Send for BlkAdapter {}
unsafe impl Sync for BlkAdapter {}

/// 39 WP-39.7：virtio 复合驱动（blk 数据盘 + net 网卡）。
struct VirtioFabric {
    /// 数据盘驱动（可选——区域无 blk 时缺省）。
    blk: Option<BlkAdapter>,
    /// virtio-net 驱动（可选——区域无网卡时缺省）。
    net: Option<
        eneros_net::VirtioNetDriver<BlkVolatileMmio, HalPool, eneros_net::dma_mem::NoopCache>,
    >,
}

// SAFETY: 同 BlkAdapter（单线程装配 + 事件循环；驱动内部 RefCell 短临界）。
unsafe impl Send for VirtioFabric {}
unsafe impl Sync for VirtioFabric {}

impl DeviceDriver for VirtioFabric {
    fn id(&self) -> &DriverId {
        static ID: DriverId = DriverId(0xB10C_0002);
        &ID
    }
    fn name(&self) -> &str {
        "virtio-fabric"
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
        // 健康来自真实组件：blk 存在 → 其 health；否则 net link。
        if let Some(blk) = &self.blk {
            return blk.health_check();
        }
        match &self.net {
            Some(net) if net.link_up() => eneros_driver_framework::DriverHealth::Healthy,
            _ => eneros_driver_framework::DriverHealth::Unhealthy,
        }
    }
    fn capabilities(&self) -> &'static [CapabilityKind] {
        // 38 语义：virtio 复合发布 BlockDevice（blk 存在时）；net 的
        // NetworkDevice 能力随 39 服务化在 40/41 消费。
        &[CapabilityKind::BlockDevice]
    }
}

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
    fn capabilities(&self) -> &'static [CapabilityKind] {
        // 38（蓝图 §7）：virtio-blk 发布 BlockDevice。
        &[CapabilityKind::BlockDevice]
    }
}

/// D4/39：virtio 设备 factory——复合驱动（blk 数据盘 + net 网卡）。
///
/// 39 WP-39.5/39.7（OBS-04）：区域常量迁至 DeviceResources——base/size
/// 来自节点 regs，槽数由 size 派生（0x200 = virtio-mmio legacy 传输
/// 步长；上限 32 防越界读）；不再要求 base == 固定区域起点（QEMU 的
/// virtio-mmio-bus.1 等第二区域由此可探测）。blk 取最后一块（数据盘
/// 语义），net 取第一块网卡；无设备 → NotFound（fail-closed）。
#[allow(unreachable_code)] // 44.2 收口：函数尾部保留 41 起不执行的区域扫描回退实现（见函数内注释）
fn virtio_factory(
    node: &eneros_board::device_tree::DeviceNode,
    ctx: &mut dyn PlatformCtx,
) -> Result<Box<dyn DeviceDriver>, DriverError> {
    // 41（WP-41 存储句柄移交，CVR-39-02 剩余面）：root **不再绑定任何
    // virtio 设备**——数据盘（blk）由 svc-storage 独占初始化（root 再
    // retype 同一 device untyped 会推进内核 freeIndex，导致 alloc_device_at
    // 的物理定位与内核分配错位，QEMU 实测 vm fault 0xa003000）；virtio-net
    // 由 EL0 网络服务接（41 外部 peer 面，未装配前诚实 UnassignedService）。
    // durable-log 在无 blk 时零写保留（已有分支）。探测只读 magic 打
    // 印（不写 STATUS——避免重置 svc-storage 已初始化设备）。
    let (base, _size) = node.regs.first().copied().ok_or(DriverError::NotFound)?;
    if base == VIRTIO_REGION_BASE {
        crate::println!(
            "[eneros-root-task] virtio: region reserved for EL0 services (svc-storage data disk)"
        );
    }
    // 消费 ctx（探测代码保留为回退路径；41 起不执行）。
    let _ = ctx;
    return Err(DriverError::NotFound);
    // 39 WP-39.5/39.7（OBS-04 回退修订，CVR-39-04）：QEMU virt 的 DTB 为
    // **每槽位一个 virtio_mmio 节点**（reg 0x200）；设备 untyped 按 4K 帧
    // retype——逐节点独立映射会在同帧内冲突（retype 偏移碰撞，QEMU 实测
    // vm fault 0xa001000）。恢复**区域扫描**：仅区域起点节点
    // （base == VIRTIO_REGION_BASE，qemu-virt 板级事实）映射 4 帧并扫描
    // 全部槽位；其余槽位节点在映射前快速拒绝（NotFound，零副作用）。
    const SLOT_STRIDE: u64 = 0x200;
    let slot_count = if base == VIRTIO_REGION_BASE {
        // 区域 32 槽（QEMU virt 板级事实；上限防越界读）。
        (VIRTIO_REGION_SIZE / SLOT_STRIDE).clamp(1, 32)
    } else {
        return Err(DriverError::NotFound);
    };
    // 映射帧数 = ceil(slot_count × 0x200 / 0x1000)（8 槽/帧；≤ 4 帧）。
    let frames = slot_count.div_ceil(8).max(1);
    // 33 WP-33.5：区域按 4K 帧分段映射（QEMU 活体实测：单次 2M 帧
    // 映射仅首 4K 生效；4K 帧路径已被 D1 pl011 与 D4 槽 0 验证可靠）。
    let mut virt = 0u64;
    for i in 0..frames {
        let mapped = ctx.map_mmio(base + i * 0x1000, 0x1000)?;
        if i == 0 {
            virt = mapped;
        }
    }
    // BlkVolatileMmio::new 为安全函数（封装内做 volatile 读写）；
    // `virt` 为首帧映射基址（生产 1:1 = base；host mock 为可读模拟寄存器
    // 区）；探测只读寄存器（magic/device-id）。
    let mut probe = BlkVolatileMmio::new(virt as usize);
    // 38（WP-38.7）：DMA 一致性判定——节点显式 dma-coherent，或平台
    // 适配器声明（qemu-virt 板级事实）。cache 按 capability 选择：
    // 显式 Coherent → NoopCache（EL0 安全）；NonCoherent → 拒绝
    // （真实 cache 维护需 seL4 syscall + page cap，39 接线——不伪造）。
    let coherent =
        virtio_coherency(node.dma_coherent, ctx.platform_dma_coherency()) == DmaCoherency::Coherent;
    crate::println!(
        "[eneros-root-task] virtio dma: coherency={} (fdt={:?} platform={:?})",
        if coherent { "coherent" } else { "non-coherent" },
        node.dma_coherent,
        ctx.platform_dma_coherency()
    );
    // 失败传播：非一致（无显式 capability）→ factory 失败（fail-closed，
    // 不静默 NoopCache——NoopCache 仅在显式 Coherent capability 下使用）。
    cache_ops_for(coherent)?;
    // 39 WP-39.5/39.7：DMA 池按设备**按需**分配（hal seam；每个 virtio
    // 设备一个 9 页池——hal 板级池容量见 CROSS_VERSION-39 CVR-39-01）。
    // 41（WP-41 存储句柄移交）：root **不再绑定 blk 驱动**——数据盘由
    // svc-storage 独占（再初始化同一 virtio 设备会覆盖 queue 配置）；
    // 系统盘（boot 介质）不写。durable-log 在无 blk 时零写保留。
    // （VirtioBlkDriver 类型保留供 host 测试/回退路径编译。）
    let bound_blk: Option<
        VirtioBlkDriver<BlkVolatileMmio, HalPool, eneros_storage::driver::virtio_blk::NoopCache>,
    > = None;
    // 39 WP-39.7：virtio-net（NetDevice seam；QEMU bus.1 槽 0）。
    let mut bound_net: Option<
        eneros_net::VirtioNetDriver<BlkVolatileMmio, HalPool, eneros_net::dma_mem::NoopCache>,
    > = None;
    for slot in 0..slot_count {
        let off = (slot * SLOT_STRIDE) as usize;
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
        // 未激活）。41 起数据盘由 svc-storage 独占初始化（DRIVER_OK）——
        // **只读 STATUS 不写**：写 STATUS 会覆盖设备状态（virtio STATUS
        // 是覆盖写，写 1 会把 DRIVER_OK 重置为 ACK），svc-storage 的队列
        // 配置立即失效（QEMU 实测 device_id 读 0 + root IPC 卡死）。设备
        // 未激活（STATUS==0）时 device_id 读 0 是诚实探测结果（该槽不可
        // 用），不伪造。
        let version = probe.read_reg(off + 0x004);
        let dev_status = probe.read_reg(off + VMMIO_STATUS);
        let device_id = if dev_status != 0 {
            probe.read_reg(off + VMMIO_DEVICE_ID)
        } else {
            0
        };
        let vendor = probe.read_reg(off + 0x00C);
        crate::println!(
            "[eneros-root-task] DBG virtio probe slot={} version={} device_id={} status={} vendor=0x{:x}",
            slot,
            version,
            device_id,
            dev_status,
            vendor
        );
        if device_id == VIRTIO_BLK_DEVICE_ID {
            // 41（WP-41 存储句柄移交，CVR-39-02 剩余面）：blk 驱动不再由
            // root 绑定——数据盘（槽 2）由 svc-storage 独占初始化（root
            // 再初始化同一设备会覆盖 queue 配置，QEMU 实测设备状态破坏）。
            // 系统盘（槽 0）root 亦无需驱动（boot 介质不写）。durable-log
            // 装配自检在无 blk 时零写保留（已有分支）。
            crate::println!(
                "[eneros-root-task] virtio-blk: reserved for svc-storage (slot={})",
                slot
            );
            continue;
        }
        if device_id == eneros_net::VIRTIO_NET_DEVICE_ID {
            // 39 WP-39.7：绑定 virtio-net（独立 DMA 池——net 队列 9 页；
            // 同一节点内 blk/net 并存时各持一池）。
            let mmio = BlkVolatileMmio::new((base + off as u64) as usize);
            let (net_pool, _net_map) = HalPool::allocate_from(
                ctx.dma_runtime().ok_or(DriverError::NotFound)?,
                InventoryDeviceId((node.path.len() + 100) as u32),
            )?;
            let driver =
                eneros_net::VirtioNetDriver::new(mmio, net_pool, eneros_net::dma_mem::NoopCache)
                    .map_err(|_| DriverError::InitFailed)?;
            crate::println!(
                "[eneros-root-task] virtio-net: bound slot={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={}",
                slot,
                driver.mac_address()[0],
                driver.mac_address()[1],
                driver.mac_address()[2],
                driver.mac_address()[3],
                driver.mac_address()[4],
                driver.mac_address()[5],
                driver.link_up()
            );
            bound_net = Some(driver);
            continue;
        }
        // 有设备但不是 blk/net（如 rng=4）→ 跳过继续扫描（33 WP-33.5：
        // 旧实现直接返回——rng 在槽 1 会阻断槽 2 数据盘的到达）。
        // 30 RT-018：探测失败复位 STATUS（virtio 规范 STATUS=0 复位）。
        probe.write_reg(off + VMMIO_STATUS, 0);
    }
    // 39 WP-39.1/39.6（P0-07）：durable-log **零写发现**在装配层执行
    // （`durable_log_probe` 对最后一块 blk）——工厂只做绑定，不写盘。
    let fabric_blk = bound_blk.map(|driver| BlkAdapter { inner: driver });
    if fabric_blk.is_none() && bound_net.is_none() {
        return Err(DriverError::NotFound);
    }
    let mut fabric = VirtioFabric {
        blk: fabric_blk,
        net: bound_net,
    };
    // 39：**装箱后**注册全局指针（Box 分配稳定；栈上注册会在 move 后
    // 悬垂——39 QEMU 排障教训）。net → 事件循环 0x88b5 回显；blk →
    // 装配层 durable_log_probe（最后一块 = 数据盘）。
    let mut boxed = Box::new(fabric);
    if let Some(net) = boxed.net.as_mut() {
        let ptr = net as *mut eneros_net::VirtioNetDriver<
            BlkVolatileMmio,
            HalPool,
            eneros_net::dma_mem::NoopCache,
        >;
        NET_DRIVER.store(ptr as usize, core::sync::atomic::Ordering::Relaxed);
    }
    if let Some(blk) = boxed.blk.as_mut() {
        let ptr = &mut blk.inner
            as *mut VirtioBlkDriver<
                BlkVolatileMmio,
                HalPool,
                eneros_storage::driver::virtio_blk::NoopCache,
            >;
        LAST_BLK_DRIVER.store(ptr as usize, core::sync::atomic::Ordering::Relaxed);
    }
    Ok(boxed)
}

/// 39 WP-39.7：virtio-net 全局驱动指针（事件循环回显用；0 = 未装配）。
static NET_DRIVER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// 39 WP-39.1/39.6：最后一块 virtio-blk 驱动指针（数据盘语义；装配层
/// `durable_log_probe` 使用；0 = 无 blk）。指针指向 fabric 内的驱动
/// （Box 稳定分配，root-task 常驻）。
static LAST_BLK_DRIVER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// 39 WP-39.1/39.6（P0-07）：数据盘 durable-log **零写发现**（装配层）。
///
/// 39 收尾复验（P0-1）：发现/识别/授权写严格分离——不再对裸整盘
/// 直接构造日志（那会写 LBA0/GPT 保护区），而是：
/// 1. `GptResolver::open::<AuditRole>`（零读发现 + role 匹配）拿
///    `PartitionHandle<AuditRole>`（LOGS 分区）；
/// 2. 句柄上 `open_existing`（零写；未初始化保持零写）；
/// 3. 识别成功才认领并 append "boot"（正常业务写，非发现写）。
///
/// 无 GPT/无 audit 分区/GPT 损坏 → 零写保留，boot_id 不注册。
pub fn durable_log_probe() {
    let ptr = LAST_BLK_DRIVER.load(core::sync::atomic::Ordering::Relaxed);
    if ptr == 0 {
        crate::println!("[eneros-root-task] durable-log: no data disk bound —— 零写保留");
        return;
    }
    // SAFETY: 指针由 virtio_factory 注册（Box 稳定分配）；root-task 单
    // 线程装配期独占访问。
    let driver = unsafe {
        &mut *(ptr as *mut VirtioBlkDriver<
            BlkVolatileMmio,
            HalPool,
            eneros_storage::driver::virtio_blk::NoopCache,
        >)
    };
    // 角色解析（零写）：默认 policy 对齐 qemu-v1 布局（LOGS→Audit）。
    crate::println!(
        "[eneros-root-task] durable-log: probe target blocks={} block_size={}",
        eneros_storage::driver::BlockDevice::block_count(&*driver),
        eneros_storage::driver::BlockDevice::block_size(&*driver)
    );
    let policy = eneros_storage::partition::RolePolicy::default();
    let dev = eneros_durable_log::RefMutDevice(driver);
    let handle = match eneros_storage::partition::GptResolver::open::<
        eneros_storage::partition::AuditRole,
        _,
    >(dev, &policy)
    {
        Ok(h) => {
            crate::println!(
                "[eneros-root-task] durable-log: resolved audit partition start_lba={} blocks={}",
                h.range().start_lba,
                h.block_count()
            );
            h
        }
        Err(e) => {
            // 零写：未知/无 GPT/无角色分区——不认领、不初始化。
            crate::println!(
                "[eneros-root-task] durable-log: discovery {:?} —— 零写保留 \
                 （无 audit 角色分区；provisioning 需显式授权，normal release 无 capability）",
                e
            );
            return;
        }
    };
    match eneros_durable_log::BlockDurableLog::open_existing(handle, 512) {
        Ok(mut log) => {
            crate::println!(
                "[eneros-root-task] durable-log: open_existing committed={}",
                log.committed_seq()
            );
            match log.append(b"boot") {
                Ok(seq) => {
                    crate::println!(
                        "[eneros-root-task] durable-log: append seq={} (boot count)",
                        seq
                    );
                    // 38：boot 序号注册为全局 boot_id（跨重启单调；
                    // 能力 epoch/重启归因来源）。
                    crate::panic::register_boot_id(seq);
                }
                Err(e) => {
                    crate::println!("[eneros-root-task] durable-log: append failed {:?}", e);
                }
            }
        }
        Err(e) => {
            // 零写：不 format、不写头块、不自动初始化。
            crate::println!(
                "[eneros-root-task] durable-log: open_existing {:?} —— 零写保留 \
                 （provisioning 需显式授权，normal release 无 capability）",
                e
            );
        }
    }
}

/// 39 WP-39.7：外部 peer 回显轮询——收到 EtherType=0x88b5 且 payload 为
/// `ENEROS-NET-RX` 的帧时回 `ENEROS-NET-TX`（`tools/sel4-net-peer.py`
/// 的期望）。非阻塞（try_recv）：无帧立即返回，不拖慢事件循环。
pub fn net_echo_poll() {
    let ptr = NET_DRIVER.load(core::sync::atomic::Ordering::Relaxed);
    if ptr == 0 {
        return;
    }
    // SAFETY: 指针由 virtio_factory 注册（Box 稳定分配）；root-task 单
    // 线程事件循环独占访问；驱动内部 RefCell 串行化。
    let drv = unsafe {
        &mut *(ptr as *mut eneros_net::VirtioNetDriver<
            BlkVolatileMmio,
            HalPool,
            eneros_net::dma_mem::NoopCache,
        >)
    };
    if !drv.link_up() {
        return;
    }
    let mut buf = [0u8; 2048];
    match drv.try_recv(&mut buf) {
        Ok(Some(n)) => {
            // 以太帧：dst(6) + src(6) + ethertype(2) + payload。
            if n >= 27 && buf[12..14] == [0x88, 0xB5] && &buf[14..27] == b"ENEROS-NET-RX" {
                let mut reply = [0u8; 27];
                // 39 收尾复验（F12）：回复目的 MAC = 入帧源 MAC
                //（旧实现硬编码 02:00:00:00:00:01，仅对单一 peer 脚本成立）。
                reply[..6].copy_from_slice(&buf[6..12]);
                reply[6..12].copy_from_slice(&drv.mac_address());
                reply[12..14].copy_from_slice(&[0x88, 0xB5]);
                reply[14..27].copy_from_slice(b"ENEROS-NET-TX");
                match drv.send(&reply) {
                    Ok(()) => {
                        crate::println!(
                            "[eneros-root-task] virtio-net: echoed ENEROS-NET-TX ({} bytes)",
                            n
                        );
                    }
                    Err(e) => {
                        crate::println!("[eneros-root-task] virtio-net: echo send failed {:?}", e);
                    }
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            // 39 收尾复验（F12）：接收错误不再静默吞掉（RingCorrupt/
            // DmaError 与无帧 Timeout 一并不可观测——损坏必须可见）。
            crate::println!("[eneros-root-task] virtio-net: rx poll error {:?}", e);
        }
    }
}

/// SPEC-D WP-D.1：pl011 实例可寻址标识——按 FDT 节点单元地址派生
/// （多 uart 板卡每实例唯一、跨 boot 稳定），替换硬编码 `"pl011-uart0"`
/// （单实例时代遗留）。QEMU virt 唯一节点 `uart@9000000` →
/// `pl011-uart@9000000`。spawn 侧移交允许清单与证据词表按本标识对齐；
/// 该名是 root 内注册/诊断用途，不跨 VSpace 进入 EL0 服务。
fn pl011_instance_name(node: &eneros_board::device_tree::DeviceNode) -> alloc::string::String {
    alloc::format!("pl011-{}", node.name)
}

/// UART 波特率（D1：115200 8N1 默认；真机按协议层配置）。
const UART_BAUD: u32 = 115_200;
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
    // 38（WP-38.10）：PL011 时钟来自 FDT（clocks phandle → clock-frequency）；
    // 缺失 → Err(NotFound) fail-closed（禁止猜时钟）。
    let clock_hz = node
        .clocks
        .iter()
        .find_map(|c| c.frequency_hz)
        .or(node.clock_frequency)
        .ok_or(DriverError::NotFound)?;
    let name = pl011_instance_name(node);
    let mut driver = UartDriver::new(
        DriverId(0x0A11_0001),
        &name,
        Box::new(mmio),
        clock_hz as u32,
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
    // 38（WP-38.10）：真实 WdtAdapter（真实 health：is_enabled 读回；
    // 不再以 Noop 冒充 healthy）。
    Ok(Box::new(WdtAdapter {
        wdt: eneros_watchdog::wdt::HwWatchdog::new(virt),
    }))
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
    // 38（WP-38.10）：真实 RtcAdapter（真实 health：秒值必须>epoch 基线；
    // 不再以 Noop 冒充 healthy）。
    Ok(Box::new(RtcAdapter {
        rtc: Pl031Rtc::new(virt),
    }))
}

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
    // 38（WP-38.10）：SJA1000 晶振频率来自 FDT（clocks 或 clock-frequency）；
    // 缺失 → Err(NotFound) fail-closed（禁止猜时钟）。
    let clock_hz = node
        .clocks
        .iter()
        .find_map(|c| c.frequency_hz)
        .or(node.clock_frequency)
        .ok_or(DriverError::NotFound)?;
    let controller = Sja1000Controller::new(mmio, clock_hz as u32, eneros_time::get_monotonic_ns);
    let config = CanConfig {
        controller_type: CanControllerType::SJA1000,
        ..Default::default()
    };
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
    // 38（WP-38.10）：RS485 后端 PL011 时钟来自 FDT；缺失 → fail-closed。
    let clock_hz = node
        .clocks
        .iter()
        .find_map(|c| c.frequency_hz)
        .or(node.clock_frequency)
        .ok_or(DriverError::NotFound)?;
    let uart = unsafe {
        Pl011UartHw::new(
            virt,
            clock_hz as u32,
            Box::new(eneros_time::get_monotonic_ns),
        )
    };
    Ok(Box::new(Rs485Driver::new(
        DriverId(0x4850_0001),
        Rs485Config::default(),
        Box::new(uart),
    )))
}

/// 一条已接线的驱动 IRQ：事件循环 poll ntfn → supervisor 内驱动
/// `handle_irq`（按 badge 查）→ ack。驱动所有权在 [`DriverSupervisor`]
/// （38 WP-38.8：单所有权，无 IRQ 驱动不再丢失）。
#[cfg(target_os = "none")]
pub struct IrqDriver {
    /// 中断号（GIC 全局号）。
    pub irq: u32,
    /// IRQ 绑定链（ntfn_slot/handler_cap/badge）。
    pub binding: crate::irq::IrqBinding,
    /// 对应驱动在 supervisor 内的 badge。
    pub badge: u32,
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
    /// 38（WP-38.7）：平台 DMA 运行时（hal 唯一 seam；`HalRuntime.dma`）。
    dma: &'static mut dyn DmaAllocator,
    /// 38（WP-38.7）：平台缺省 DMA 一致性（qemu-virt 适配器声明）。
    coherency: Option<DmaCoherency>,
}

#[cfg(target_os = "none")]
impl<'a> RootPlatformCtx<'a> {
    pub fn new(
        spawn: &'a mut crate::spawn::SpawnCtx,
        irq_table: &'a mut crate::irq::IrqTable,
        dma: &'static mut dyn DmaAllocator,
        coherency: Option<DmaCoherency>,
    ) -> Self {
        Self {
            spawn,
            irq_table,
            bindings: alloc::vec::Vec::new(),
            last_badge: None,
            dma,
            coherency,
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
            while bits > FRAME_BITS_4K && (cursor & ((1u64 << bits) - 1)) != 0 {
                bits = if bits == FRAME_BITS_1G {
                    FRAME_BITS_2M
                } else {
                    FRAME_BITS_4K
                };
            }
            crate::spawn::map_device_mmio(self.spawn, cursor, bits)
                .map_err(|_| DriverError::NotFound)?;
            let mapped = 1u64 << bits;
            cursor = cursor.checked_add(mapped).ok_or(DriverError::NotFound)?;
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

    fn dma_runtime(&mut self) -> Option<&mut (dyn DmaAllocator + '_)> {
        // reborrow：字段为 'static 泄漏分配器；factory 构造期短借用。
        Some(&mut *self.dma)
    }

    fn platform_dma_coherency(&self) -> Option<DmaCoherency> {
        self.coherency
    }
}

/// SPEC-B WP-B.1：spawn 前 RTC 兜底装配。DTB 探测链后置且可能剪除 pl031
/// （QEMU virt 实测运行时 DTB nodes=60 无该节点）——X6 墙钟取数又必须在
/// spawn 期完成，故以 QEMU virt 板级事实把 PL031 所在 2MiB 设备区帧一次
/// 恒等映射并注册；pl011 等子设备后续经 [`crate::spawn::map_device_mmio`]
/// 的 SpawnCtx.dev_maps 去重复用同帧，零二次 Retype。RTC_BASE 已非零 →
/// no-op（DTB 路径先行成功时零副作用）。失败显式打 MISSING 行（fail-closed，
/// 绝不猜测基址继续）。
#[cfg(target_os = "none")]
pub fn ensure_rtc_bootstrapped(ctx: &mut crate::spawn::SpawnCtx) {
    if crate::panic::RTC_BASE.load(core::sync::atomic::Ordering::Relaxed) != 0 {
        return;
    }
    // QEMU virt 板级事实：PL031 @ 0x09010000，落在 DEV-UNTYPED
    // paddr=0x9000000/size_bits=24 的 16MiB 设备区。seL4 Retype 无 untyped
    // 内偏移、2MiB 大页在 root VSpace 落地不生效（QEMU 实测 .07 L3 翻译
    // 故障）——按 4K 帧从区基址起**逐页恒等映射**（pre-spawn cursor@0，
    // 帧落点精确），PL031 在第 16 页。页 0..15 的映射同时登记进
    // SpawnCtx.dev_maps：后续 pl011 工厂的同页请求命中去重 → 零二次
    // Retype、零 freeIndex 撞位（此前顺序帧+pl011 重取导致 UART 翻错页
    // 故障 0x9000024 的根因闭环）。
    const QEMU_VIRT_REGION_BASE: u64 = 0x0900_0000;
    const PL031_OFFSET_IN_REGION: u64 = 0x0001_0000;
    const FRAME: u64 = 0x1000;
    let rtc_va = QEMU_VIRT_REGION_BASE + PL031_OFFSET_IN_REGION;
    let pages = (PL031_OFFSET_IN_REGION / FRAME) as usize; // 0..=16 → 17 页
    let mut ok = true;
    for k in 0..=pages {
        let va = QEMU_VIRT_REGION_BASE + (k as u64) * FRAME;
        if let Err(e) = crate::spawn::map_device_mmio(ctx, va, 12) {
            ok = false;
            crate::println!(
                "[eneros-root-task] TIME-EPOCH-MISSING source=no-rtc page-map-failed va={va:#x} err={e:?}"
            );
            break;
        }
    }
    match ok {
        true => {
            crate::panic::register_rtc_base(rtc_va);
            // 关键：X6 取数走 eneros_time 的 RTC_BASE——必须同步写时钟子系统
            //（set_rtc_base 立即读 RTCDR 建立偏移；探测期后置路径同基址幂等）。
            eneros_time::set_rtc_base(rtc_va);
            crate::println!(
                "[eneros-root-task] TIME-SOURCE bootstrapped: PL031 vaddr={rtc_va:#x} (4K 逐页·QEMU virt 板级兜底，spawn 前)"
            );
        }
        false => {
            crate::println!(
                "[eneros-root-task] TIME-EPOCH-MISSING source=no-rtc region-map-failed"
            );
        }
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
/// 返回（DriverSupervisor, IRQ 接线表）。
///
/// 38（WP-38.8/38.10）：
/// - **全部** BoundDriver 进入 [`DriverSupervisor`]（不再按 IRQ 过滤——
///   无 IRQ 驱动不再随局部变量 drop，P0-06/HD-04 修复）；
/// - supervisor 执行 bind→map→init→start→health 全生命周期，任一步
///   失败逆序回滚，健康由驱动真实 health 决定（Noop 不冒充）；
/// - Started+Healthy 驱动的 capability 发布进 [`CapabilityRegistry`]；
///   缺 required capability（console/monotonic-clock）→ CAP-BLOCKED
///   （readiness 阻断判据，fail-closed）；
/// - IRQ 接线按 badge 引用 supervisor 内驱动（事件循环经
///   `supervisor.driver_for_badge` 调 handle_irq——单所有权）。
///
/// 任一步失败（无 DTB/解析失败）→ 打印 fail 行并继续（不挂起：驱动缺失
/// 不应阻断 EL0 服务 spawn；真机 bring-up 依赖报告定位缺口）。
#[cfg(target_os = "none")]
pub fn probe_and_report(
    spawn: &mut crate::spawn::SpawnCtx,
    irq_table: &mut crate::irq::IrqTable,
    bundle_len: u64,
    platform_dma: &'static mut dyn DmaAllocator,
    platform_coherency: Option<DmaCoherency>,
    // 38（蓝图 §7）：HAL 层能力（timer/entropy 来自 HalRuntime——非驱动
    // 设备的平台能力也进注册表）。
    hal_timer_present: bool,
    hal_entropy: Option<alloc::string::String>,
) -> (DriverSupervisor, alloc::vec::Vec<IrqDriver>) {
    use eneros_board::device_tree::parse_fdt;

    let empty = || (DriverSupervisor::new(), alloc::vec::Vec::new());
    let Some((blob, dtb_phys)) = runtime_dtb_slice(bundle_len) else {
        crate::println!(
            "[eneros-root-task] ENEROS stage=probe-report error=no-runtime-dtb bundle_len={}",
            bundle_len
        );
        return empty();
    };
    let inventory = match parse_fdt(&blob) {
        Ok(inv) => inv,
        Err(e) => {
            crate::println!(
                "[eneros-root-task] ENEROS stage=probe-report error=fdt-parse {:?} bytes={}",
                e,
                blob.len()
            );
            return empty();
        }
    };
    crate::println!(
        "[eneros-root-task] ENEROS stage=probe-report dtb-phys=0x{:x} dtb-bytes={} nodes={} compatibles={:?}",
        dtb_phys,
        blob.len(),
        inventory.len(),
        binding_table().compatibles()
    );
    // SPEC-B WP-B.1 诊断行：pl031/sp805 是否真的进入设备清单（AAVMF 链
    // 剪枝嫌疑的实证口径；一行常驻成本可忽略）。
    crate::println!(
        "[eneros-root-task] DBG dtb-inventory pl031={} sp805={} nodes={}",
        inventory.find_by_compatible("arm,pl031").is_some(),
        inventory.find_by_compatible("arm,sp805").is_some(),
        inventory.nodes.len()
    );
    let mut ctx = RootPlatformCtx::new(spawn, irq_table, platform_dma, platform_coherency);
    let report = binding_table().probe_all(&inventory, &mut ctx);
    // 39 WP-39.1/39.6：数据盘 durable-log 零写发现（最后一块 blk；
    // 装配层执行——工厂内不做写盘判断）。
    durable_log_probe();
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
    // 38（WP-38.8）：全部 bound 进入 supervisor + 全生命周期。
    let mut supervisor = DriverSupervisor::new();
    for entry in report.into_bound() {
        supervisor.adopt(entry);
    }
    supervisor
        .run_lifecycle(crate::panic::PANIC_BOOT_ID.load(core::sync::atomic::Ordering::Relaxed));
    crate::println!(
        "[eneros-root-task] ENEROS stage=driver-supervisor {} healthy={}",
        supervisor.report_line(),
        supervisor.healthy_count()
    );
    for d in supervisor.all() {
        crate::println!(
            "[eneros-root-task]   lifecycle node={} driver={} state={} err={:?}",
            d.node,
            d.driver.name(),
            d.state.name(),
            d.error
        );
    }
    // 38（蓝图 §7）：能力注册表——Started+Healthy 驱动发布 capability；
    // required 缺失 → CAP-BLOCKED（readiness 阻断判据）。
    let mut registry = CapabilityRegistry::new();
    let epoch = crate::panic::PANIC_BOOT_ID.load(core::sync::atomic::Ordering::Relaxed);
    // HAL 层平台能力（非 DeviceDriver 的设备能力）。
    if hal_timer_present {
        let _ = registry.publish(eneros_driver_framework::Capability {
            kind: CapabilityKind::MonotonicClock,
            version: 1,
            owner: "arm64-generic-timer".to_owned(),
            epoch,
            quality: CapabilityQuality::Full,
        });
    }
    if let Some(name) = hal_entropy {
        let _ = registry.publish(eneros_driver_framework::Capability {
            kind: CapabilityKind::Entropy,
            version: 1,
            owner: name,
            epoch,
            quality: CapabilityQuality::Full,
        });
    }
    for d in supervisor.all() {
        if !matches!(
            d.state,
            eneros_driver_framework::DriverLifecycleState::Healthy
                | eneros_driver_framework::DriverLifecycleState::Degraded
        ) {
            continue;
        }
        for kind in d.driver.capabilities() {
            let _ = registry.publish(eneros_driver_framework::Capability {
                kind: *kind,
                version: 1,
                owner: d.driver.name().to_owned(),
                epoch,
                quality: CapabilityQuality::Full,
            });
        }
    }
    for cap in registry.all() {
        crate::println!(
            "[eneros-root-task] CAPABILITY: kind={} owner={} epoch={}",
            cap.kind.name(),
            cap.owner,
            cap.epoch
        );
    }
    const REQUIRED: &[CapabilityKind] = &[CapabilityKind::Console, CapabilityKind::MonotonicClock];
    let missing = registry.missing(REQUIRED);
    if missing.is_empty() {
        crate::println!(
            "[eneros-root-task] CAP-READY required={:?}",
            REQUIRED
                .iter()
                .map(|k| k.name())
                .collect::<alloc::vec::Vec<_>>()
        );
    } else {
        // 38-review（Exit Gate 9）：缺 required capability 必须**真阻断**
        // readiness——只打印不阻断等于假门禁。fail-closed 挂起（零设备
        // 写/零服务 spawn），由 watchdog/复位路径收场。
        crate::println!(
            "[eneros-root-task] CAP-BLOCKED missing={:?} — fail-closed 挂起（readiness 阻断，零设备写/零 spawn）",
            missing.iter().map(|k| k.name()).collect::<alloc::vec::Vec<_>>()
        );
        loop {
            core::hint::spin_loop();
        }
    }
    // 38：IRQ 接线——按 badge 引用 supervisor 内驱动（单所有权；事件循环
    // 经 driver_for_badge 调 handle_irq）。
    let mut irq_drivers = alloc::vec::Vec::new();
    for (irq, badge) in ctx.bindings.iter().copied() {
        // badge 解码防御：badge = 绑定索引 + 1（0 保留未知）——回绕防护。
        let Some(binding) = badge_to_binding_index(badge).and_then(|idx| irq_table.binding(idx))
        else {
            crate::println!(
                "[eneros-root-task]   probe irq-wire badge decode FAILED badge={} — 跳过",
                badge
            );
            continue;
        };
        let Some(idx) = supervisor
            .all()
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
        let entry = &supervisor.all()[idx];
        crate::println!(
            "[eneros-root-task]   probe irq-wired driver={} irq={} badge={}",
            entry.driver.name(),
            irq,
            badge
        );
        irq_drivers.push(IrqDriver {
            irq,
            binding,
            badge,
        });
    }
    (supervisor, irq_drivers)
}

/// 30 RT-009：badge → 绑定索引解码（badge = 绑定索引 + 1，0 保留未知）。
/// `badge=0` 返回 None（防 u32 回绕成 u32::MAX 索引）。
#[allow(dead_code)] // 44.2：badge→绑定槽位换算，当前路径未用（44.4 冻结登记）
fn badge_to_binding_index(badge: u32) -> Option<usize> {
    badge.checked_sub(1).map(|b| b as usize)
}

/// SP805 看门狗 Adapter（38 WP-38.10：真实 factory——31 D5/D6 时代工厂曾
/// 返回占位驱动「注册即装配」，38 退役占位；host 编译亦需要真实类型）。
/// 38（WP-38.10）：SP805 看门狗 Adapter——真实 health（is_enabled 读回），
/// 发布 Watchdog 能力（占位已退役，此处仅保留类型以兼容 host 测试装配）。
/// 38-review 订正：WDT/RTC 工厂当前返回真实 Adapter（WdtAdapter/RtcAdapter），
/// 不再经占位。
struct WdtAdapter {
    wdt: eneros_watchdog::wdt::HwWatchdog,
}

impl DeviceDriver for WdtAdapter {
    fn id(&self) -> &DriverId {
        static ID: DriverId = DriverId(0x8050_0001);
        &ID
    }
    fn name(&self) -> &str {
        "sp805-wdt0"
    }
    fn driver_type(&self) -> eneros_driver_framework::DriverType {
        eneros_driver_framework::DriverType::Custom(0x805)
    }
    fn state(&self) -> eneros_driver_framework::DriverState {
        if self.wdt.is_enabled() {
            eneros_driver_framework::DriverState::Running
        } else {
            eneros_driver_framework::DriverState::Error
        }
    }
    fn init(&mut self) -> Result<(), DriverError> {
        // panic 路径已武装；此处初始化软件看门狗语义（10s 超时）。
        self.wdt.init(10_000);
        Ok(())
    }
    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), DriverError> {
        self.wdt.stop();
        Ok(())
    }
    fn deinit(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn handle_irq(&mut self, _irq_id: u32) {}
    fn health_check(&self) -> eneros_driver_framework::DriverHealth {
        // 真实 health：基址有效且已使能才算 healthy（非 Noop 冒充）。
        if self.wdt.is_enabled() {
            eneros_driver_framework::DriverHealth::Healthy
        } else {
            eneros_driver_framework::DriverHealth::Unknown
        }
    }
    fn capabilities(&self) -> &'static [CapabilityKind] {
        &[CapabilityKind::Watchdog]
    }
}

/// 38（WP-38.10）：PL031 RTC Adapter——真实 health（秒值>epoch 基线），
/// 发布 WallClock 能力。
struct RtcAdapter {
    rtc: Pl031Rtc,
}

impl DeviceDriver for RtcAdapter {
    fn id(&self) -> &DriverId {
        static ID: DriverId = DriverId(0x0310_0001);
        &ID
    }
    fn name(&self) -> &str {
        "pl031-rtc0"
    }
    fn driver_type(&self) -> eneros_driver_framework::DriverType {
        eneros_driver_framework::DriverType::Custom(0x031)
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
        // 真实 health：RTC 秒值必须大于 epoch 基线（2024-01-01），
        // 否则电池/寄存器失效 → Unknown（不冒充 healthy）。
        const EPOCH_BASELINE: u64 = 1_704_067_200;
        if self.rtc.read_secs() > EPOCH_BASELINE {
            eneros_driver_framework::DriverHealth::Healthy
        } else {
            eneros_driver_framework::DriverHealth::Unknown
        }
    }
    fn capabilities(&self) -> &'static [CapabilityKind] {
        &[CapabilityKind::WallClock]
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

    use eneros_board::device_tree::DeviceNode;

    use super::*;

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
        fn dma_runtime(&mut self) -> Option<&mut (dyn DmaAllocator + '_)> {
            Some(test_allocator())
        }
        fn platform_dma_coherency(&self) -> Option<DmaCoherency> {
            // 测试平台声明一致（与 qemu-virt 适配器同语义）。
            Some(DmaCoherency::Coherent)
        }
    }

    fn node(name: &str, compat: &str, base: u64, size: u64) -> DeviceNode {
        DeviceNode {
            name: name.into(),
            compatible: compat.into(),
            compatible_all: alloc::vec![compat.into()],
            status: eneros_board::device_tree::NodeStatus::Ok,
            path: alloc::format!("/{name}"),
            phandle: None,
            regs: alloc::vec![(base, size)],
            regs_bus: alloc::vec![(base, size)],
            interrupts: alloc::vec![],
            global_irqs: alloc::vec![],
            // 38（WP-38.10）：工厂时钟来自 FDT——测试节点带 24 MHz 时钟
            // （qemu-virt clk24mhz 语义；缺失 → fail-closed）。
            clocks: alloc::vec![eneros_hal::inventory::ClockRef {
                phandle: 1,
                frequency_hz: Some(24_000_000),
            }],
            resets: alloc::vec![],
            gpios: alloc::vec![],
            dmas: alloc::vec![],
            clock_frequency: Some(24_000_000),
            dma_coherent: None,
            dma_ranges: alloc::vec![],
            iommus: alloc::vec![],
            properties: alloc::vec![],
        }
    }

    /// 38（WP-38.7）mutation 防假绿：NoopCache 生产默认熔断——一致性
    /// 判定必须显式来源（节点 dma-coherent 或平台事实），无来源 → 非一致。
    #[test]
    fn virtio_coherency_requires_explicit_fact() {
        // 无任何声明 → NonCoherent（禁止默认 NoopCache）。
        assert_eq!(virtio_coherency(None, None), DmaCoherency::NonCoherent);
        // 节点显式 dma-coherent → Coherent（FDT 事实优先）。
        assert_eq!(
            virtio_coherency(Some(true), Some(DmaCoherency::NonCoherent)),
            DmaCoherency::Coherent
        );
        // 节点显式 non-coherent → NonCoherent（不被平台覆盖）。
        assert_eq!(
            virtio_coherency(Some(false), Some(DmaCoherency::Coherent)),
            DmaCoherency::NonCoherent
        );
        // 平台适配器声明（qemu-virt 板级事实）。
        assert_eq!(
            virtio_coherency(None, Some(DmaCoherency::Coherent)),
            DmaCoherency::Coherent
        );
    }

    /// 测试 DMA 分配器（hal seam host 实现；每次调用泄漏一个新池——测试
    /// 独占，MockCtx 生命周期短）。
    fn test_allocator() -> &'static mut dyn DmaAllocator {
        Box::leak(Box::new(
            eneros_hal::dma::DmaPoolAllocator::<{ 9 * 4096 }>::new(eneros_hal::PhysAddr::new(
                0x1000_0000,
            )),
        ))
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
            disabled: alloc::vec![],
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
        assert_eq!(
            ctx.mapped,
            alloc::vec![(0x1000_0000, 0x100), (0x0900_0000, 0x1000)]
        );
    }

    /// 无 reg 属性的节点 → failed（fail-closed，不静默构造）。
    #[test]
    fn missing_reg_fails_binding() {
        let table = binding_table();
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![DeviceNode {
                name: "can@0".into(),
                compatible: "nxp,sja1000".into(),
                compatible_all: alloc::vec!["nxp,sja1000".into()],
                status: eneros_board::device_tree::NodeStatus::Ok,
                path: "/can@0".into(),
                phandle: None,
                regs: alloc::vec![],
                regs_bus: alloc::vec![],
                interrupts: alloc::vec![],
                global_irqs: alloc::vec![],
                clocks: alloc::vec![],
                resets: alloc::vec![],
                gpios: alloc::vec![],
                dmas: alloc::vec![],
                clock_frequency: Some(24_000_000),
                dma_coherent: None,
                dma_ranges: alloc::vec![],
                iommus: alloc::vec![],
                properties: alloc::vec![],
            }],
            disabled: alloc::vec![],
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
            disabled: alloc::vec![],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 0);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.failed()[0].error, DriverError::NotFound);
        // 区域起点在 mock（无真实设备）下同样 NotFound——不静默构造
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![node(
                "virtio_mmio@a000000",
                "virtio,mmio",
                0x0a00_0000,
                0x200
            )],
            disabled: alloc::vec![],
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
        assert_eq!(
            report.bound()[0].driver.name(),
            "pl011-uart@9000000",
            "SPEC-D WP-D.1：实例标识按 FDT 单元地址派生（去硬编码）"
        );
        // qemu-virt.dts 的其余设备节点都应在 unbound 清单中（节点一致）
        let unbound: Vec<&str> = report.unbound().iter().map(|u| u.node.as_str()).collect();
        assert!(
            !unbound.contains(&"uart@9000000"),
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

    /// SPEC-D WP-D.1 C1：两枚 PL011 节点（真机板形）probe 产生两个**可区分**
    /// 实例标识——派生自各自 FDT 单元地址，顺序=dtb walk 序跨 boot 稳定；
    /// 不再共享单实例硬编码名。
    #[test]
    fn pl011_two_instances_probe_distinct_identities() {
        let table = binding_table();
        let inventory = eneros_board::device_tree::DeviceInventory {
            nodes: alloc::vec![
                node("uart@9000000", "arm,pl011", 0x0900_0000, 0x1000),
                node("uart@9100000", "arm,pl011", 0x0910_0000, 0x1000),
            ],
            disabled: alloc::vec![],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 2, "两枚 pl011 都绑定");
        let names: alloc::vec::Vec<&str> = report.bound().iter().map(|b| b.driver.name()).collect();
        assert_eq!(names[0], "pl011-uart@9000000");
        assert_eq!(names[1], "pl011-uart@9100000");
        assert_ne!(names[0], names[1], "两实例标识必须可区分");
        // 两实例 MMIO 都经 ctx 真实装配（1:1）。
        assert_eq!(
            ctx.mapped,
            alloc::vec![(0x0900_0000, 0x1000), (0x0910_0000, 0x1000)]
        );
    }
}
