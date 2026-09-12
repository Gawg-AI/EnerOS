//! svc-storage 真块后端（蓝图 41：存储句柄移交——数据盘 virtio-blk 由
//! svc-storage 独占持有，root 不再绑定）。
//!
//! 39 WP-39.12 的 [`StorageService`] 需要 `BlockBackend`；本模块提供
//! **生产后端**：[`VirtioBlkBackend`] 包装 storage crate 的
//! `VirtioBlkDriver`（真实 virtqueue + DMA，QEMU 实测），并定义服务侧
//! DMA 池 [`SvcDmaPool`]（root 移交的连续物理帧——cpu 虚拟地址 + 物理
//! 基址，Q_PFN 写物理地址）。
//!
//! 设备移交契约（CVR-39-02 剩余面，41 实施）：
//! - root spawn svc-storage 时映射 virtio-blk 数据盘 MMIO 页（固定虚拟
//!   地址 `SVC_STORAGE_MMIO_VADDR`）+ 9 页连续 DMA 帧（固定虚拟地址
//!   `SVC_STORAGE_DMA_VADDR`）进服务 VSpace；
//! - 服务 `_start` 收到 X3=mmio_vaddr, X4=dma_vaddr, X5=dma_phys；
//!   X3==0 → 设备未移交 → `StorageService` 无后端 → EXECUTE 显式
//!   ST_UNAVAILABLE（fail-closed，不伪造 I/O）；
//! - 缓存一致性：QEMU virt 声明 dma-coherent（FDT/平台双来源）→
//!   NoopCache 合法（38 门禁：coherent → NoopCache；非一致拒绝——EL0
//!   无 cache 维护指令权限，CVR-38-01）。

extern crate alloc;

use core::cell::RefCell;

// SPEC-B/C（2026-08-28）：移交 vaddr 常量单一事实源 = abi/handover-abi
//（历史与 spawn.rs 同值不同名双源已收编；此处仅别名 re-use，无值定义）。
pub use eneros_handover_abi::{
    STORAGE_DMA_PAGES as SVC_STORAGE_DMA_PAGES, STORAGE_DMA_VADDR as SVC_STORAGE_DMA_VADDR,
    STORAGE_MMIO_VADDR as SVC_STORAGE_MMIO_VADDR,
};
use eneros_storage::driver::virtio_blk::{DmaPool, VirtioBlkDriver, VolatileMmio};
use eneros_storage::driver::BlockDevice;

use crate::block_cmd::BlockBackend;

/// 服务侧 DMA 池（root 移交的连续物理帧；cpu 视图 = 服务 VSpace 映射
/// 虚拟地址，设备视图 = 物理基址）。
#[derive(Clone, Copy)]
pub struct SvcDmaPool {
    /// CPU 视图基址（服务 VSpace 已映射虚拟地址）。
    cpu_ptr: *mut u8,
    /// 设备可见物理基址（Q_PFN/描述符 addr 写此值）。
    phys: u64,
    /// 池长度（字节）。
    len: usize,
}

// SAFETY: 池内存为 root 移交的静态帧映射，服务单线程独占（驱动内部
// RefCell 串行化）；指针不跨界读写。
unsafe impl Send for SvcDmaPool {}
unsafe impl Sync for SvcDmaPool {}

impl SvcDmaPool {
    /// 从 root 移交参数构造（cpu_vaddr / dma_phys / 页数）。
    ///
    /// # SAFETY
    /// `cpu_vaddr` 必须是 root 映射到服务 VSpace 的 DMA 帧虚拟地址，
    /// 且覆盖 `pages * 4096` 字节；`phys` 为同一批帧的物理基址。
    pub const unsafe fn new(cpu_vaddr: u64, phys: u64, pages: usize) -> Self {
        Self {
            cpu_ptr: cpu_vaddr as *mut u8,
            phys,
            len: pages * 4096,
        }
    }

    pub fn phys_base(&self) -> u64 {
        self.phys
    }
}

impl DmaPool for SvcDmaPool {
    fn len(&self) -> usize {
        self.len
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn phys_base(&self) -> u64 {
        self.phys
    }
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: `cpu_ptr` 指向 root 移交的 'static 帧映射；`len` 为
        // 移交长度（<= 池容量）；驱动只在此范围内读写（virtio 描述符/
        // 请求头/数据槽）。
        unsafe { core::slice::from_raw_parts_mut(self.cpu_ptr, self.len) }
    }
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: 同上（只读视图）。
        unsafe { core::slice::from_raw_parts(self.cpu_ptr, self.len) }
    }
}

/// 生产块后端：VirtioBlkDriver 适配 `BlockBackend`（lba = 字节偏移，
/// 与 39 命令面 RamBackend 测试语义一致；按 512B 块 RMW 落盘）。
pub struct VirtioBlkBackend<M, P, C>
where
    M: eneros_storage::driver::virtio_blk::VirtioMmio,
    P: DmaPool,
    C: eneros_storage::driver::virtio_blk::ArmCache,
{
    driver: RefCell<VirtioBlkDriver<M, P, C>>,
    block_size: usize,
}

impl VirtioBlkBackend<VolatileMmio, SvcDmaPool, eneros_storage::driver::virtio_blk::NoopCache> {
    /// 初始化 virtio-blk 数据盘驱动并构造后端。
    ///
    /// `mmio_vaddr`/`dma_vaddr`/`dma_phys` 来自 root spawn 移交（服务
    /// `_start` X3/X4/X5）；任一为 0 → Err（调用方保持无后端 → 显式
    /// Unavailable，不伪造）。
    pub fn try_new(mmio_vaddr: u64, dma_vaddr: u64, dma_phys: u64) -> Result<Self, &'static str> {
        if mmio_vaddr == 0 || dma_vaddr == 0 || dma_phys == 0 {
            return Err("device-handover-missing");
        }
        // SAFETY: root 已把 DMA 帧映射到 dma_vaddr（长度 = 页数×4K）。
        let pool = unsafe { SvcDmaPool::new(dma_vaddr, dma_phys, SVC_STORAGE_DMA_PAGES) };
        let mmio = VolatileMmio::new(mmio_vaddr as usize);
        // NoopCache：QEMU virt 声明 dma-coherent → 38 门禁允许（非一致
        // 平台必须拒绝——此处不伪造 cache 维护）。
        let driver = VirtioBlkDriver::new(
            mmio,
            pool,
            eneros_storage::driver::virtio_blk::NoopCache,
            eneros_storage::StorageConfig::default(),
        )
        .map_err(|_| "virtio-blk-init-failed")?;
        let block_size = driver.block_size();
        Ok(Self {
            driver: RefCell::new(driver),
            block_size,
        })
    }
}

impl<M, P, C> BlockBackend for VirtioBlkBackend<M, P, C>
where
    M: eneros_storage::driver::virtio_blk::VirtioMmio,
    P: DmaPool,
    C: eneros_storage::driver::virtio_blk::ArmCache,
{
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        let bs = self.block_size as u64;
        if buf.is_empty() {
            return Ok(());
        }
        // 可能跨块：逐块读（lba = 字节偏移；块号 = lba / bs）。
        let mut off = lba;
        let mut out = buf;
        while !out.is_empty() {
            let block = off / bs;
            let within = (off % bs) as usize;
            let take = core::cmp::min(out.len(), bs as usize - within);
            let mut tmp = [0u8; 512];
            if bs as usize > tmp.len() {
                return Err(());
            }
            match self
                .driver
                .borrow()
                .read_block(block, &mut tmp[..bs as usize])
            {
                Ok(()) => {}
                Err(_e) => {
                    // 41 诊断：target 侧打印失败细节（fail-closed 不伪造）。
                    #[cfg(target_os = "none")]
                    eneros_runtime::println!(
                        "[svc-storage] blk read FAILED lba={} block={} err={:?}",
                        lba,
                        block,
                        _e
                    );
                    return Err(());
                }
            }
            out[..take].copy_from_slice(&tmp[within..within + take]);
            out = &mut out[take..];
            off += take as u64;
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
        let bs = self.block_size as u64;
        if data.is_empty() {
            return Ok(());
        }
        let mut off = lba;
        let mut src = data;
        while !src.is_empty() {
            let block = off / bs;
            let within = (off % bs) as usize;
            let take = core::cmp::min(src.len(), bs as usize - within);
            // RMW：读整块（未对齐写须保留块内其余字节）。
            let mut tmp = [0u8; 512];
            if bs as usize > tmp.len() {
                return Err(());
            }
            if within != 0 || take != bs as usize {
                self.driver
                    .borrow()
                    .read_block(block, &mut tmp[..bs as usize])
                    .map_err(|_| ())?;
            }
            tmp[within..within + take].copy_from_slice(&src[..take]);
            self.driver
                .borrow_mut()
                .write_block(block, &tmp[..bs as usize])
                .map_err(|_| ())?;
            src = &src[take..];
            off += take as u64;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ()> {
        self.driver.borrow_mut().flush().map_err(|_| ())
    }

    fn health(&self) -> (u64, u32, u8) {
        let d = self.driver.borrow();
        (d.block_count(), d.block_size() as u32, 0)
    }
}

// ============================================================================
// 70.7（WP-70.7.2）：启动盘（virtio slot 0）受限安装后端
// ============================================================================

/// 启动盘后端：第二 VirtioBlkDriver 实例（slot 0 GPT 产品盘）+ 非活动槽
/// 写守卫。MMIO = 数据盘移交页 + slot 0 偏移 0（页覆盖 slot 0-7，零新增
/// 映射）；DMA 池 = spawn 独立连续分配的 9 页，基址经 **X8** 直传（root
/// `initial_registers` 单源；禁止 "+N 页" 派生——REG-70.7-BOOT-BACKEND-
/// ZEROSIZE 根因：first-fit 落位不保证紧邻，派生地址写 ghost 页 FAULT）。
/// 泛型参数供宿主仿真测试注入（SimMmio/TestPool）；生产装配 =
/// `BootSlotBackend<VolatileMmio, SvcDmaPool, NoopCache>`。
pub struct BootSlotBackend<M: eneros_storage::driver::virtio_blk::VirtioMmio, P: DmaPool, C: eneros_storage::driver::virtio_blk::ArmCache>
{
    inner: VirtioBlkBackend<M, P, C>,
    guard: crate::boot_guard::BootWriteGuard,
    /// 启动盘 DMA 池基址（svc VSpace 1:1 虚拟 = 物理）。
    boot_dma_vaddr: u64,
    /// 70.7（REG-70.7 run24/25 实弹）：BOOTCTRL 双副本**影子副本**——
    /// init 期读取填充（init 期驱动 I/O 正常），运行期 bootctrl 读全走
    /// 影子（运行期 boot 队列请求设备端不完成——挂死根因待 QEMU trace
    /// 定位）；事务写成功后同步更新影子。
    shadow: core::cell::RefCell<alloc::vec::Vec<u8>>,
}

impl<M: eneros_storage::driver::virtio_blk::VirtioMmio, P: DmaPool, C: eneros_storage::driver::virtio_blk::ArmCache>
    BootSlotBackend<M, P, C>
{
    /// 初始化（svc-storage 启动时一次）：
    ///
    /// - `boot_mmio_vaddr` = 数据盘移交页基址（slot 0 偏移 0）；
    /// - `boot_dma_vaddr/phys` = boot 池 1:1 基址（spawn X8 直传）。
    ///
    /// BOOTCTRL 双副本解析失败 → Err（守卫不可得 = 写面整体禁用；
    /// 每次安装会话由 svc-ota 经 BLK_BOOT_GUARD 重查）。
    pub fn guard(&self) -> &crate::boot_guard::BootWriteGuard {
        &self.guard
    }

    pub fn boot_dma_vaddr(&self) -> u64 {
        self.boot_dma_vaddr
    }
}

/// 生产装配专用构造（具体 Mmio/Pool 类型；宿主测试经泛型直接构造结构体）。
impl BootSlotBackend<
    VolatileMmio,
    SvcDmaPool,
    eneros_storage::driver::virtio_blk::NoopCache,
> {
    pub fn try_new_boot(
        boot_mmio_vaddr: u64,
        boot_dma_vaddr: u64,
        boot_dma_phys: u64,
    ) -> Result<Self, &'static str> {
        // REG-70.7-BOOT-BACKEND-ZEROSIZE 排查：init 前寄存器 dump
        //（magic@0x000 / deviceID@0x008 / vendor@0x00c / status@0x070）。
        {
            let dump = |tag: &str, regs: [(usize, &str); 4]| {
                for (off, name) in regs {
                    let v = unsafe {
                        core::ptr::read_volatile((boot_mmio_vaddr + off as u64) as *const u32)
                    };
                    eneros_runtime::println!(
                        "[svc-storage] BOOT-MMIO-DUMP {tag} {name}(0x{off:03x})=0x{v:08x}"
                    );
                }
            };
            dump(
                "pre-init",
                [
                    (0x000, "magic"),
                    (0x008, "devid"),
                    (0x00c, "vendor"),
                    (0x070, "status"),
                ],
            );
        }
        let mut inner = VirtioBlkBackend::try_new(boot_mmio_vaddr, boot_dma_vaddr, boot_dma_phys)?;
        // REG-70.7 排查：init 后 status/capacity 复核（对照 dump）。
        {
            let status =
                unsafe { core::ptr::read_volatile((boot_mmio_vaddr + 0x070) as *const u32) };
            let cap_lo =
                unsafe { core::ptr::read_volatile((boot_mmio_vaddr + 0x100) as *const u32) };
            let cap_hi =
                unsafe { core::ptr::read_volatile((boot_mmio_vaddr + 0x104) as *const u32) };
            eneros_runtime::println!(
                "[svc-storage] BOOT-MMIO-DUMP post-init status=0x{status:08x} cap=0x{cap_hi:08x}{cap_lo:08x}"
            );
        }
        // BOOTCTRL 双副本读取（qemu-v1：分区基 lba 18432；副本 0/+8 lba）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        let mut first = [0u8; 4096];
        let mut second = [0u8; 4096];
        use BlockBackend as _;
        // inner 的 read 形参是**字节偏移**（数据盘历史契约——run15 实弹：
        // 扇区 LBA 直传被 /512 换算成块 36，读回 GPT 区垃圾 → 双副本
        // decode 失败 "bootctrl-both-invalid"）。boot 面单位契约 = 512B
        // 扇区 LBA，此处与 BlockBackend::read（下方）为仅有的两个换算点。
        inner
            .read(bc_lba * 512, &mut first)
            .map_err(|_| "bootctrl-read")?;
        inner
            .read((bc_lba + 8) * 512, &mut second)
            .map_err(|_| "bootctrl-read")?;
        let guard = crate::boot_guard::BootWriteGuard::resolve(&first, &second)?;
        // 影子副本：init 期双副本（8 KiB）入 RAM——运行期 bootctrl 读
        // 零驱动 I/O。
        let mut shadow = alloc::vec![0u8; 8192];
        shadow[..4096].copy_from_slice(&first);
        shadow[4096..].copy_from_slice(&second);
        Ok(BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr,
            shadow: core::cell::RefCell::new(shadow),
        })
    }

}

impl<M: eneros_storage::driver::virtio_blk::VirtioMmio, P: DmaPool, C: eneros_storage::driver::virtio_blk::ArmCache> crate::block_cmd::InstallBackend
    for BootSlotBackend<M, P, C>
{
    fn install_ranges(&self) -> (u64, u64, u64, u64) {
        let g = &self.guard;
        let (bc_lba, bc_lbas) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        (g.inactive_start_lba, g.inactive_lbas, bc_lba, bc_lbas)
    }
}

impl<M: eneros_storage::driver::virtio_blk::VirtioMmio, P: DmaPool, C: eneros_storage::driver::virtio_blk::ArmCache> crate::block_cmd::BlockBackend
    for BootSlotBackend<M, P, C>
{
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] boot read enter lba={lba} n={}", buf.len());
        // boot 面单位契约 = **512B 扇区 LBA**（boot_disk::lba_range_of 同
        // 口径）；inner read 形参 = 字节偏移——本处单点换算（try_new_boot
        // 内部读为另一点，见上）。
        let off = lba.checked_mul(512).ok_or(())?;
        // BOOTCTRL 双副本区间 → 影子副本（运行期零驱动 I/O）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        let rec0 = bc_lba * 512;
        let rec1 = (bc_lba + 8) * 512;
        let shadow = self.shadow.borrow();
        if off == rec0 && buf.len() == 4096 {
            buf.copy_from_slice(&shadow[..4096]);
            return Ok(());
        }
        if off == rec1 && buf.len() == 4096 {
            buf.copy_from_slice(&shadow[4096..8192]);
            return Ok(());
        }
        crate::block_cmd::BlockBackend::read(&mut self.inner, off, buf)
    }
    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
        // 受限安装写：范围守卫在 handle_boot_cmd 已强制，此处**复核同一
        // 不变量**（深度防御）——非活动槽 ∪ BOOTCTRL 之外零写。扇区→字节
        // 换算后落 inner。
        let (is, il, bs, bl) = crate::block_cmd::InstallBackend::install_ranges(self);
        let end = lba.saturating_add(data.len() as u64);
        let ok = (lba >= is && end <= is + il) || (lba >= bs && end <= bs + bl);
        if !ok {
            return Err(());
        }
        let off = lba.checked_mul(512).ok_or(())?;
        crate::block_cmd::BlockBackend::write(&mut self.inner, off, data)?;
        // 影子同步（仅 BOOTCTRL 双副本区间；写守卫已限定范围）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        let mut shadow = self.shadow.borrow_mut();
        for (rec, base) in [(0usize, bc_lba * 512), (1usize, (bc_lba + 8) * 512)] {
            if off == base && data.len() == 4096 {
                let dst = if rec == 0 { 0 } else { 4096 };
                shadow[dst..dst + 4096].copy_from_slice(data);
            }
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), ()> {
        Ok(())
    }
    fn health(&self) -> (u64, u32, u8) {
        crate::block_cmd::BlockBackend::health(&self.inner)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use eneros_storage::driver::virtio_blk::VirtioMmio;

    use super::*;

    /// 模拟 MMIO 的 process_queues 需要驱动 pool 指针（QEMU 角色）。
    /// 测试用全局静态（与 storage crate host 测试同模式）。
    static TEST_POOL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    struct SimMmio {
        regs: [u32; 68],
        disk: alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>,
    }

    impl SimMmio {
        fn new(blocks: u64) -> Self {
            let mut regs = [0u32; 68];
            regs[0] = 0x7472_6976; // VIRTIO_MMIO_MAGIC
            regs[1] = 1; // version
            regs[0x008 / 4] = 2; // VIRTIO_BLK_DEVICE_ID
            regs[0x034 / 4] = 32; // QUEUE_NUM_MAX
            regs[0x100 / 4] = (blocks & 0xFFFF_FFFF) as u32; // capacity lo
            regs[0x104 / 4] = (blocks >> 32) as u32; // capacity hi
            SimMmio {
                regs,
                disk: alloc::rc::Rc::new(core::cell::RefCell::new(
                    alloc::vec![0u8; (blocks as usize) * 512],
                )),
            }
        }

        fn process_queues(&mut self) {
            let qp = TEST_POOL.load(core::sync::atomic::Ordering::Relaxed);
            if qp == 0 {
                return;
            }
            // SAFETY: 测试专用——qp 指向全局仿真队列（长度 0x2400），
            // 区间划分与生产路径一致。
            let queue = unsafe { core::slice::from_raw_parts_mut(qp as *mut u8, 9 * 4096) };
            let avail =
                unsafe { &mut *(queue[0x0100..0x1100].as_mut_ptr() as *mut crate::VirtqAvail) };
            let used =
                unsafe { &mut *(queue[0x1000..0x2000].as_mut_ptr() as *mut crate::VirtqUsed) };
            while used.idx != avail.idx {
                let head = avail.ring[used.idx as usize & (crate::QUEUE_MAX - 1)] as usize;
                // SAFETY: 测试专用——desc 只读视图区间合法。
                let descs = unsafe {
                    core::slice::from_raw_parts(
                        queue[0x0000..0x1000].as_ptr() as *const crate::VirtqDesc,
                        crate::QUEUE_MAX,
                    )
                };
                // 三描述符链：req(0,读) → data(1) → status(2,写)。
                let d0 = &descs[head];
                let d1 = &descs[(head + 1) & (crate::QUEUE_MAX - 1)];
                let d2 = &descs[(head + 2) & (crate::QUEUE_MAX - 1)];
                let base = 0x1000_0000u64; // 仿真物理基址
                let req_off = (d0.addr - base) as usize;
                let data_off = (d1.addr - base) as usize;
                let status_off = (d2.addr - base) as usize;
                // req: kind(4)+reserved(4)+sector(8)
                let kind = u32::from_le_bytes(queue[req_off..req_off + 4].try_into().unwrap());
                let sector =
                    u64::from_le_bytes(queue[req_off + 8..req_off + 16].try_into().unwrap());
                let start = (sector as usize) * 512;
                match kind {
                    0 => {
                        // READ：data 描述符写 512B
                        let disk = self.disk.borrow();
                        queue[data_off..data_off + d1.len as usize]
                            .copy_from_slice(&disk[start..start + 512]);
                        queue[status_off] = 0;
                    }
                    1 => {
                        // WRITE：data 描述符读入磁盘
                        let mut d = self.disk.borrow_mut();
                        d[start..start + 512]
                            .copy_from_slice(&queue[data_off..data_off + d1.len as usize]);
                        queue[status_off] = 0;
                    }
                    4 => {
                        // FLUSH：无数据描述符（datalen=0）——仅 status。
                        queue[status_off] = 0;
                    }
                    _ => {
                        queue[status_off] = 2; // IOERR
                    }
                }
                used.ring[used.idx as usize & (crate::QUEUE_MAX - 1)] = crate::VirtqUsedElem {
                    id: head as u32,
                    len: d1.len as u32,
                };
                used.idx = used.idx.wrapping_add(1);
            }
        }
    }

    impl VirtioMmio for SimMmio {
        fn read_reg(&self, offset: usize) -> u32 {
            self.regs[offset / 4]
        }
        fn write_reg(&mut self, offset: usize, value: u32) {
            if offset == 0x050 {
                // QUEUE_NOTIFY：QEMU 角色处理队列。
                self.process_queues();
            }
            self.regs[offset / 4] = value;
        }
    }

    /// 仿真 DMA 池（Vec arena + 仿真物理基址）。
    struct TestPool {
        arena: alloc::vec::Vec<u8>,
        phys: u64,
    }

    impl TestPool {
        fn new() -> Self {
            TestPool {
                arena: alloc::vec![0u8; 9 * 4096],
                phys: 0x1000_0000,
            }
        }
    }

    impl DmaPool for TestPool {
        fn len(&self) -> usize {
            self.arena.len()
        }
        fn is_empty(&self) -> bool {
            self.arena.is_empty()
        }
        fn phys_base(&self) -> u64 {
            self.phys
        }
        fn as_bytes_mut(&mut self) -> &mut [u8] {
            &mut self.arena
        }
        fn as_bytes(&self) -> &[u8] {
            &self.arena
        }
    }

    #[test]
    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    fn virtio_backend_read_write_roundtrip() {
        let blocks = 64u64;
        let mmio = SimMmio::new(blocks);
        let pool = TestPool::new();
        TEST_POOL.store(
            pool.arena.as_ptr() as u64,
            core::sync::atomic::Ordering::Relaxed,
        );
        let driver = VirtioBlkDriver::new(
            mmio,
            pool,
            eneros_storage::driver::virtio_blk::NoopCache,
            eneros_storage::StorageConfig::default(),
        )
        .expect("驱动初始化（仿真设备）");
        let mut backend: VirtioBlkBackend<SimMmio, TestPool, _> = VirtioBlkBackend {
            driver: RefCell::new(driver),
            block_size: 512,
        };
        // 写入 20 字节（对齐块内偏移 0）。
        backend.write(0, b"hello-storage-backend").unwrap();
        let mut buf = [0u8; 21];
        backend.read(0, &mut buf).unwrap();
        assert_eq!(&buf, b"hello-storage-backend");
        // 非对齐写（块内偏移 100）——RMW 路径。
        backend.write(100, b"unaligned").unwrap();
        let mut buf2 = [0u8; 9];
        backend.read(100, &mut buf2).unwrap();
        assert_eq!(&buf2, b"unaligned");
        // 跨块写（块 0 尾 + 块 1 头）。
        match backend.write(500, b"cross") {
            Ok(()) => {}
            Err(e) => panic!("cross write failed: {:?}", e),
        }
        let mut buf3 = [0u8; 5];
        match backend.read(500, &mut buf3) {
            Ok(()) => {}
            Err(e) => panic!("cross read failed: {:?}", e),
        }
        assert_eq!(&buf3, b"cross");
        // flush。
        backend.flush().unwrap();
        // health。
        let (blocks_out, bs, h) = backend.health();
        assert_eq!(blocks_out, blocks);
        assert_eq!(bs, 512);
        assert_eq!(h, 0);
    }

    #[test]
    fn handover_missing_rejected() {
        assert!(VirtioBlkBackend::try_new(0, 0, 0).is_err());
        assert!(VirtioBlkBackend::try_new(0x4000_0000, 0x4001_0000, 0).is_err());
    }

    /// run15 实弹回归锁：boot 面以 **512B 扇区 LBA** 寻址（boot_disk
    /// 同口径），inner 字节偏移换算只在 BootSlotBackend 边界。直传扇区
    /// 值曾读回 GPT 区垃圾（"bootctrl-both-invalid"）。同时锁写守卫：
    /// 非活动槽/BOOTCTRL 之外零写（活跃槽写拒绝）。
    #[test]
    fn boot_face_uses_sector_lba_and_guard_writes() {
        // 40000 块 ≈ 20.5MB——须覆盖非活动槽 BOOT_B 首块（lba 36864）。
        let blocks = 40000u64;
        let mmio = SimMmio::new(blocks);
        let pool = TestPool::new();
        TEST_POOL.store(
            pool.arena.as_ptr() as u64,
            core::sync::atomic::Ordering::Relaxed,
        );
        let driver = VirtioBlkDriver::new(
            mmio,
            pool,
            eneros_storage::driver::virtio_blk::NoopCache,
            eneros_storage::StorageConfig::default(),
        )
        .expect("驱动初始化（仿真设备）");
        let mut inner: VirtioBlkBackend<SimMmio, TestPool, _> = VirtioBlkBackend {
            driver: RefCell::new(driver),
            block_size: 512,
        };
        // 字节语义直写扇区 100（字节 51200）作地标。
        inner.write(100 * 512, b"SECTOR100!").unwrap();

        let record = eneros_boot_protocol::BootControl::factory([7u8; 16], 1, [7u8; 16])
            .encode()
            .unwrap();
        let guard = crate::boot_guard::BootWriteGuard::resolve(&record, &record)
            .expect("factory 记录可解析");
        let mut boot = BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr: 0,
            shadow: core::cell::RefCell::new(alloc::vec![0u8; 8192]),
        };
        use crate::block_cmd::BlockBackend as _;
        // 扇区 LBA 100 → 字节 51200（换算生效）；扇区 0 保持零。
        let mut buf = [0u8; 10];
        boot.read(100, &mut buf).unwrap();
        assert_eq!(&buf, b"SECTOR100!");
        let mut z = [0u8; 4];
        boot.read(0, &mut z).unwrap();
        assert!(z == [0u8; 4], "扇区 0 不得读到字节 0 偏移的地标");

        // 写守卫：factory A=trial(15) 活跃 → 非活动 = BOOT_B；BOOT_B 内
        // 写放行，活跃槽/BOOTCTRL 区外写拒绝。
        let (is, il, bs_, bl_) = crate::block_cmd::InstallBackend::install_ranges(&boot);
        assert!(il > 0 && bl_ > 0);
        assert!(boot.write(bs_, b"g").is_ok(), "BOOTCTRL 内写放行");
        assert!(boot.write(is, b"g").is_ok(), "非活动槽内写放行");
        assert!(boot.write(0, b"x").is_err(), "活跃槽零写");
        assert!(boot.write(bs_ + bl_, b"x").is_err(), "越 BOOTCTRL 尾零写");
    }
}
