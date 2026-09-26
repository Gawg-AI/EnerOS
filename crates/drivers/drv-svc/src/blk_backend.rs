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

use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
// SPEC-B/C（2026-08-28）：移交 vaddr 常量单一事实源 = abi/handover-abi
//（历史与 spawn.rs 同值不同名双源已收编；此处仅别名 re-use，无值定义）。
pub use eneros_handover_abi::{
    STORAGE_DMA_PAGES as SVC_STORAGE_DMA_PAGES, STORAGE_DMA_VADDR as SVC_STORAGE_DMA_VADDR,
    STORAGE_MMIO_VADDR as SVC_STORAGE_MMIO_VADDR,
};
use eneros_storage::driver::virtio_blk::{DmaPool, VirtioBlkDriver, VolatileMmio};
use eneros_storage::driver::BlockDevice;

use crate::block_cmd::BlockBackend;
use crate::boot_guard::BOOTCTRL_COPY_STRIDE_LBAS;

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

/// 生产块后端：VirtioBlkDriver 适配 `BlockBackend`（`offset_bytes` =
/// 字节偏移，数据面单位契约见 trait doc；与 39 命令面 RamBackend 测试
/// 语义一致；按 512B 块 RMW 落盘）。
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
    fn read(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), ()> {
        let bs = self.block_size as u64;
        if buf.is_empty() {
            return Ok(());
        }
        // 可能跨块：逐块读（数据面单位 = 字节偏移；块号 = off / bs）。
        let mut off = offset_bytes;
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
                        offset_bytes,
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

    fn write(&mut self, offset_bytes: u64, data: &[u8]) -> Result<(), ()> {
        // 70.14 探针（第七轮，REG-70.12-PROBE-BOOT-TRIAL-STALL）：**方法体首行**
        // 先原子自增再打印。上一轮已证 PRE 探针（调用前）的自增与打印都发生、
        // 而本函数旧的"进入"打印**不**出现；本轮把"自增"与"打印"都放到首行，
        // 于是可直接判定：
        //   本计数递增（串口可能无输出）⇒ 已进入驱动方法体 ⇒ 阻塞在驱动内部
        //   本计数不递增 ⇒ 阻塞在 **trait 分派调用边界**
        // 计数由 `driver_service_main` 之外的只读探针供 svc-ota 读回。
        #[cfg(target_os = "none")]
        {
            crate::block_cmd::BLK_WRITE_ENTER_PROBES
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        let bs = self.block_size as u64;
        if data.is_empty() {
            return Ok(());
        }
        #[cfg(target_os = "none")]
        eneros_runtime::println!(
            "[svc-storage] blk-write enter lba={} len={} bs={}",
            offset_bytes,
            data.len(),
            bs
        );
        let mut off = offset_bytes;
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
            #[cfg(target_os = "none")]
            eneros_runtime::println!("[svc-storage] blk-write blk={} take={}", block, take);
            match self
                .driver
                .borrow_mut()
                .write_block(block, &tmp[..bs as usize])
            {
                Ok(()) => {}
                Err(_e) => {
                    #[cfg(target_os = "none")]
                    eneros_runtime::println!(
                        "[svc-storage] blk-write FAILED blk={} err={_e:?}",
                        block
                    );
                    return Err(());
                }
            }
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
        (
            d.block_count(),
            d.block_size() as u32,
            fold_health_code(d.health_status()),
        )
    }
}

/// R16-L（蓝图 75-03 WP-75.3.2）：驱动 [`eneros_storage::DeviceHealth`]
/// 全包折成非零劣化码写 BLK_CMD_HEALTH 应答第三字节（wire 13B 冻结——
/// 只传折码不传连续量，原恒 0 = 通道两端死码）。折码规则（显式可测；
/// **阈值为初版工程判据可调**——wear>80 / life<20，非冻结 spec）：
/// 多条件取最严重，优先级 bad_blocks > wear_level > remaining_life；
/// 2=坏块 / 4=磨损 / 3=寿命 / 0=健康。
fn fold_health_code(h: eneros_storage::DeviceHealth) -> u8 {
    if h.bad_blocks > 0 {
        2
    } else if h.wear_level > 80 {
        4
    } else if h.remaining_life < 20 {
        3
    } else {
        0
    }
}

// ============================================================================
// 70.7（WP-70.7.2）：启动盘（virtio slot 0）受限安装后端
// ============================================================================

/// 启动盘后端：第二 VirtioBlkDriver 实例（slot 0 GPT 产品盘）+ 非活动槽
/// 写守卫。MMIO = 数据盘移交页 + slot 0 偏移 0（页覆盖 slot 0-7，零新增
/// 映射）；DMA 池 = spawn 独立连续分配的 9 页，基址经 **X7** 直传（root
/// `initial_registers` 单源；禁止 "+N 页" 派生——REG-70.7-BOOT-BACKEND-
/// ZEROSIZE 根因：first-fit 落位不保证紧邻，派生地址写 ghost 页 FAULT）。
/// 泛型参数供宿主仿真测试注入（SimMmio/TestPool）；生产装配 =
/// `BootSlotBackend<VolatileMmio, SvcDmaPool, NoopCache>`。
pub struct BootSlotBackend<
    M: eneros_storage::driver::virtio_blk::VirtioMmio,
    P: DmaPool,
    C: eneros_storage::driver::virtio_blk::ArmCache,
> {
    inner: VirtioBlkBackend<M, P, C>,
    guard: crate::boot_guard::BootWriteGuard,
    /// 启动盘 DMA 池基址（svc VSpace 1:1 虚拟 = 物理）。
    boot_dma_vaddr: u64,
    /// 70.7（REG-70.7 run24/25 实弹）：BOOTCTRL 双副本**影子副本**——
    /// init 期读取填充（init 期驱动 I/O 正常），运行期 bootctrl 读全走
    /// 影子（运行期 boot 队列请求设备端不完成——挂死根因待 QEMU trace
    /// 定位）；事务写成功后同步更新影子。定长数组（零堆——处理链上
    /// 禁 alloc）；大小 = 双副本记录（布局单源派生，非独立魔数）。
    shadow: core::cell::RefCell<[u8; 2 * BOOT_CONTROL_RECORD_SIZE]>,
    /// 影子与介质一致性（70.7 复查）：部分/错位 BOOTCTRL 写会使其失效，
    /// 此后 bootctrl 读面显式 Err 而非回陈旧记录。整条 4096B 记录写
    /// （唯一生产形态）恢复有效。
    shadow_valid: core::cell::Cell<bool>,
}

impl<
        M: eneros_storage::driver::virtio_blk::VirtioMmio,
        P: DmaPool,
        C: eneros_storage::driver::virtio_blk::ArmCache,
    > BootSlotBackend<M, P, C>
{
    /// 初始化（svc-storage 启动时一次）：
    ///
    /// - `boot_mmio_vaddr` = 数据盘移交页基址（slot 0 偏移 0）；
    /// - `boot_dma_vaddr/phys` = boot 池 1:1 基址（spawn X7 直传）。
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
impl BootSlotBackend<VolatileMmio, SvcDmaPool, eneros_storage::driver::virtio_blk::NoopCache> {
    pub fn try_new_boot(
        boot_mmio_vaddr: u64,
        boot_dma_vaddr: u64,
        boot_dma_phys: u64,
        running: Option<crate::boot_guard::RunningIdentity>,
    ) -> Result<Self, &'static str> {
        // REG-70.7（run16-33 实弹定性）：bootctrl 读为**布局/时序敏感**
        // 操作——ELF 布局漂移即可使 init 读超时/失败（50 二分实验同款
        // 注记）。整轮重试（每轮 VirtioBlkBackend::try_new 内部 Status=0
        // 起步 = device reset + 重配 + 重读），最多 3 轮。
        let mut last_err = "bootctrl-read";
        for attempt in 1..=3u8 {
            match Self::try_new_boot_once(
                boot_mmio_vaddr,
                boot_dma_vaddr,
                boot_dma_phys,
                running,
                attempt,
            ) {
                Ok(boot) => return Ok(boot),
                Err(e) => {
                    last_err = e;
                    eneros_runtime::println!(
                        "[svc-storage] BOOT-INSTALL attempt {attempt} failed ({e}) —— 重试"
                    );
                }
            }
        }
        Err(last_err)
    }

    fn try_new_boot_once(
        boot_mmio_vaddr: u64,
        boot_dma_vaddr: u64,
        boot_dma_phys: u64,
        running: Option<crate::boot_guard::RunningIdentity>,
        attempt: u8,
    ) -> Result<Self, &'static str> {
        let _ = attempt;
        let mut inner = VirtioBlkBackend::try_new(boot_mmio_vaddr, boot_dma_vaddr, boot_dma_phys)?;
        // BOOTCTRL 双副本读取（qemu-v1：分区基 lba 18432；副本 0/+8 lba）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        use BlockBackend as _;
        // inner 的 read 形参是**字节偏移**（数据盘历史契约——run15 实弹：
        // 扇区 LBA 直传被 /512 换算成块 36，读回 GPT 区垃圾 → 双副本
        // decode 失败 "bootctrl-both-invalid"）。boot 面单位契约 = 512B
        // 扇区 LBA；扇区→字节换算单源 = `boot_guard::sector_byte_offset`
        //（init 期此处与 `read_sectors` 共用同一换算）。
        //
        // 70.7（REG-70.7-BOOT-DEV-IO-HANG 栈预算修复）：双副本**直接读入
        // 影子缓冲原地**（8 KiB 单缓冲）——旧版栈上 first+second 2×4 KiB
        // 再拷入影子，`try_new_boot` 帧 16 KiB，叠加 BootControl::decode
        // 解码帧后越过 svc-storage 32 KiB 栈 → guard 页 FAULT → 重启
        // （实弹：0x225930 = guard 页内）。
        let mut shadow = [0u8; 2 * BOOT_CONTROL_RECORD_SIZE];
        {
            let (first, second) = shadow.split_at_mut(BOOT_CONTROL_RECORD_SIZE);
            // 换算单源与 read_sectors 同一 helper（2026-09-24 复查修正：
            // 注释先于代码为真——bc_lba=18432 量级无溢出，Err 分支为
            // 结构一致性而非可达路径）。
            let off0 = crate::boot_guard::sector_byte_offset(bc_lba).ok_or("bootctrl-read")?;
            let off1 = crate::boot_guard::sector_byte_offset(bc_lba + BOOTCTRL_COPY_STRIDE_LBAS)
                .ok_or("bootctrl-read")?;
            inner.read(off0, first).map_err(|_| "bootctrl-read")?;
            inner.read(off1, second).map_err(|_| "bootctrl-read")?;
        }
        let guard = crate::boot_guard::BootWriteGuard::resolve_with_running(
            &shadow[..BOOT_CONTROL_RECORD_SIZE],
            &shadow[BOOT_CONTROL_RECORD_SIZE..],
            running,
        )?;
        // 影子副本 = 上述双副本本身（运行期 bootctrl 读零驱动 I/O）。
        Ok(BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr,
            shadow: core::cell::RefCell::new(shadow),
            shadow_valid: core::cell::Cell::new(true),
        })
    }
}

impl<
        M: eneros_storage::driver::virtio_blk::VirtioMmio,
        P: DmaPool,
        C: eneros_storage::driver::virtio_blk::ArmCache,
    > crate::block_cmd::InstallBackend for BootSlotBackend<M, P, C>
{
    fn install_ranges(&self) -> crate::boot_guard::InstallRanges {
        // 守卫投影单源（BOOTCTRL 侧 = boot_disk 常量；与 73 批A 前的
        // 手摆位三元组等值——golden 测试锁死 wire 字节不变）。
        self.guard.install_ranges()
    }

    /// boot 面读取（512B 扇区 LBA 单位）——boot 面调用点一律走本方法。
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        // boot 面单位契约 = **512B 扇区 LBA**（boot_disk::lba_range_of 同
        // 口径）；扇区→字节换算单源 = `boot_guard::sector_byte_offset`
        //（init 期双副本读为另一调用点，见 try_new_boot_once）。
        let off = crate::boot_guard::sector_byte_offset(lba).ok_or(())?;
        // BOOTCTRL 双副本区间 → 影子副本（运行期零驱动 I/O）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        // 布局算术（BOOTCTRL 基址/副本基址字节）同走换算单源。
        let rec0 = crate::boot_guard::sector_byte_offset(bc_lba).ok_or(())?;
        let rec1 =
            crate::boot_guard::sector_byte_offset(bc_lba + BOOTCTRL_COPY_STRIDE_LBAS).ok_or(())?;
        let is_bootctrl_record_read =
            (off == rec0 || off == rec1) && buf.len() == BOOT_CONTROL_RECORD_SIZE;
        if is_bootctrl_record_read && !self.shadow_valid.get() {
            // 影子已因部分写失效——回陈旧记录 = 事务基于错误基线规划；
            // 显式失败（fail-closed），等待下次服务重启重新填充。
            #[cfg(target_os = "none")]
            eneros_runtime::println!(
                "[svc-storage] BOOTCTRL read refused: shadow invalidated by partial write"
            );
            return Err(());
        }
        let shadow = self.shadow.borrow();
        if off == rec0 && buf.len() == BOOT_CONTROL_RECORD_SIZE {
            buf.copy_from_slice(&shadow[..BOOT_CONTROL_RECORD_SIZE]);
            return Ok(());
        }
        if off == rec1 && buf.len() == BOOT_CONTROL_RECORD_SIZE {
            buf.copy_from_slice(&shadow[BOOT_CONTROL_RECORD_SIZE..]);
            return Ok(());
        }
        crate::block_cmd::BlockBackend::read(&mut self.inner, off, buf)
    }

    /// boot 面写入（512B 扇区 LBA 单位）——boot 面调用点一律走本方法。
    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
        // 受限安装写：范围守卫在 handle_boot_cmd 已强制，此处**复核同一
        // 不变量**（深度防御）——非活动槽 ∪ BOOTCTRL 之外零写。扇区→字节
        // 换算后落 inner。单位换算单源 = `InstallBackend::write_allowed`
        // （run54 实弹：旧内联式把字节当扇区 → BOOTCTRL 记录写恒拒）。
        let allowed = crate::block_cmd::InstallBackend::write_allowed(self, lba, data.len());
        // 70.14 插桩：boot 面写的分支可见（守卫放行？进 inner？）
        #[cfg(target_os = "none")]
        eneros_runtime::println!(
            "[svc-storage] boot-write lba={} len={} allowed={}",
            lba,
            data.len(),
            allowed
        );
        if !allowed {
            return Err(());
        }
        let off = crate::boot_guard::sector_byte_offset(lba).ok_or(())?;
        // 70.14 插桩（第六轮）：把"console 路径阻塞"与"调用边界阻塞"分开。
        // 第五轮已证：本行**打印**，紧随的驱动方法体首行**不打印**，且同窗其它
        // 服务仍在打印（console 未死）、宿主 %CPU=0（非自旋）。故本行先自增一个
        // 静态计数器再打印——串口无输出但计数器递增 ⇒ console 路径被阻塞；
        // 计数器不递增 ⇒ 阻塞在打印之前的调用边界。
        #[cfg(target_os = "none")]
        {
            crate::block_cmd::BOOT_WRITE_PRE_PROBES
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            eneros_runtime::println!(
                "[svc-storage] boot-write inner pre off={} len={}",
                off,
                data.len()
            );
        }
        crate::block_cmd::BlockBackend::write(&mut self.inner, off, data)?;
        #[cfg(target_os = "none")]
        eneros_runtime::println!("[svc-storage] boot-write inner post lba={}", lba);
        // 影子同步（仅 BOOTCTRL 双副本区间；写守卫已限定范围）。
        //
        // 形状契约：**整条 4096B 记录落在副本基址** = 快速路径（生产唯一
        // 形态：`BootInstall` 四个事务与 `BLK_BOOT_INSTALL_WRITE` 的载荷
        // 生成都只产这一种），直接写影子。
        //
        // 部分/错位写（守卫放行的合法范围，但非事务形态）：**落盘后回读
        // 整条记录重建影子**——70.7 复查：旧版对这类写返回 Ok 却静默跳过
        // 同步，后续 `read_current` 会把**旧影子**当权威、事务基于陈旧记录
        // 规划（无提示的状态分叉）。重建失败（驱动读不回）才置失效位，
        // 此后 bootctrl 读面显式 Err（fail-closed，不回陈旧记录）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        // BOOTCTRL 区段基址（字节）——换算单源。
        let span_lo = crate::boot_guard::sector_byte_offset(bc_lba).ok_or(())?;
        if off >= span_lo && off < span_lo + (2 * BOOT_CONTROL_RECORD_SIZE) as u64 {
            let dst = (off - span_lo) as usize;
            if data.len() == BOOT_CONTROL_RECORD_SIZE && dst % BOOT_CONTROL_RECORD_SIZE == 0 {
                let mut shadow = self.shadow.borrow_mut();
                shadow[dst..dst + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(data);
            } else {
                let rec = dst / BOOT_CONTROL_RECORD_SIZE;
                let rec_off = span_lo + (rec as u64) * BOOT_CONTROL_RECORD_SIZE as u64;
                // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 真根因修复）：旧实现用
                // `let mut fresh = [0u8; 4096]` —— 该局部使**本函数帧恒为 4 KiB**
                // （实弹反汇编：函数入口 `sub sp, sp, #0x1000`），于是 trial 的
                // 首写还没走到任何日志/原子探针，就在入口的栈指针下移处触发
                // seL4 fault；线程停在内核、整机 idle（实测四 CPU 全 idle_thread、
                // 宿主 %CPU=0、串口在此行之后完全静默）。改为**堆缓冲**：帧降到
                // 数百字节，boot 安装长链不再触碰 svc-storage 的 64 KiB 栈上限。
                let mut fresh = alloc::vec![0u8; BOOT_CONTROL_RECORD_SIZE];
                let refresh =
                    crate::block_cmd::BlockBackend::read(&mut self.inner, rec_off, &mut fresh);
                if refresh.is_ok() {
                    let mut shadow = self.shadow.borrow_mut();
                    shadow[rec * BOOT_CONTROL_RECORD_SIZE..(rec + 1) * BOOT_CONTROL_RECORD_SIZE]
                        .copy_from_slice(&fresh);
                } else {
                    self.shadow_valid.set(false);
                    #[cfg(target_os = "none")]
                    eneros_runtime::println!(
                        "[svc-storage] BOOTCTRL partial write off={off} len={} —— 影子重建失败，读面 fail-closed",
                        data.len()
                    );
                }
            }
        }
        Ok(())
    }

    /// F70-02：本次运行身份（init 期从只读身份页解析、随守卫冻结）。
    /// 确认面（BLK_BOOT_CONFIRM）据此核对槽/载荷身份/安全版本。
    fn running_identity(&self) -> Option<crate::boot_guard::RunningIdentity> {
        self.guard.running
    }

    /// 70.14 复查修复：**必须下沉到 inner**（原实现恒 `Ok(())`）。
    ///
    /// `BootInstall` 的四个事务（trial/confirm/cancel/install）把"双副本写 +
    /// flush"当作"掉电时任一时刻 resolve 均取新块"的持久化承诺
    ///（见 `boot_install.rs` 各事务文档），而设备写缓存不刷 = 掉电后可能只剩
    /// 半份记录（高序列副本未落、低序列副本已落 → resolve 看到的是**旧序列**，
    /// 事务自认成功但重启后消失）。此处恒真返回等于把该承诺变成空头支票。
    ///
    /// 下沉后 `VirtioBlkBackend::flush` → `issue(VIRTIO_BLK_T_FLUSH)`——与数据盘
    /// `BLK_CMD_FLUSH` 同一条已验证路径（boot 盘与数据盘同为 virtio-blk，
    /// QEMU 与真机都支持 FLUSH）。supertrait 拆除（2026-09-24）后为本
    /// trait 自有方法，路由不变。
    fn flush(&mut self) -> Result<(), ()> {
        crate::block_cmd::BlockBackend::flush(&mut self.inner)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use eneros_storage::driver::virtio_blk::VirtioMmio;

    use super::*;

    /// F70-02：测试用运行身份（A 槽；factory 记录形态与生产 trial 同族）。
    fn test_running_identity() -> Option<crate::boot_guard::RunningIdentity> {
        Some(crate::boot_guard::RunningIdentity {
            is_a: true,
            security_version: 1,
            release_id: [7u8; 16],
        })
    }

    /// 模拟 MMIO 的 process_queues 需要驱动 pool 指针（QEMU 角色）。
    /// 测试用全局静态（与 storage crate host 测试同模式）。
    static TEST_POOL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    /// TEST_POOL 为进程级单槽：并行测试互踩（boot_face × roundtrip 的
    /// 间歇性假失败）——测试级互斥串行化。
    static TEST_SEQ: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct SimMmio {
        regs: [u32; 68],
        disk: alloc::rc::Rc<core::cell::RefCell<alloc::vec::Vec<u8>>>,
        /// FLUSH（kind=4）请求计数——70.14 复查：`BootSlotBackend::flush`
        /// 曾恒 `Ok(())` 不透传，事务"双副本 + flush"的持久化承诺无法判别。
        flushes: alloc::rc::Rc<core::cell::Cell<u32>>,
    }

    impl SimMmio {
        fn new(blocks: u64) -> Self {
            let mut regs = [0u32; 68];
            regs[0] = 0x7472_6976; // VIRTIO_MMIO_MAGIC
            regs[1] = 1; // version
            regs[0x008 / 4] = 2; // VIRTIO_BLK_DEVICE_ID
            regs[0x034 / 4] = 128; // QUEUE_NUM_MAX
            regs[0x100 / 4] = (blocks & 0xFFFF_FFFF) as u32; // capacity lo
            regs[0x104 / 4] = (blocks >> 32) as u32; // capacity hi
            SimMmio {
                regs,
                disk: alloc::rc::Rc::new(core::cell::RefCell::new(
                    alloc::vec![0u8; (blocks as usize) * 512],
                )),
                flushes: alloc::rc::Rc::new(core::cell::Cell::new(0)),
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
                // 描述符链**按 next 走**，不假定段数：READ/WRITE 是
                // req → data → status 三段，**FLUSH 只有 req → status 两段**
                // （`virtio_blk.rs::issue` 的 FLUSH 分支）。旧夹具硬编码取
                // descs[head+2] 当 status —— FLUSH 时 head+2 是不属于本请求的
                // 陈旧描述符（addr 可能是 0），一旦真的下发 FLUSH 就会
                // `0 - base` 溢出 panic。该夹具此前没炸，正说明**boot 面
                // flush 从未被调用过**（与本轮修的 `flush` 空实现互证）。
                let d0 = &descs[head];
                let base = 0x1000_0000u64; // 仿真物理基址
                let req_off = (d0.addr - base) as usize;
                // req: kind(4)+reserved(4)+sector(8)
                let kind = u32::from_le_bytes(queue[req_off..req_off + 4].try_into().unwrap());
                let sector =
                    u64::from_le_bytes(queue[req_off + 8..req_off + 16].try_into().unwrap());
                let start = (sector as usize) * 512;
                let second = &descs[d0.next as usize & (crate::QUEUE_MAX - 1)];
                // FLUSH：第二段即 status（无数据描述符）；其余：第二段 = data，
                // 第三段 = status。
                let (data_desc, status_desc) = if kind == 4 {
                    (None, second)
                } else {
                    (
                        Some(second),
                        &descs[second.next as usize & (crate::QUEUE_MAX - 1)],
                    )
                };
                let status_off = (status_desc.addr - base) as usize;
                match kind {
                    0 => {
                        // READ：**按请求长度**（data.len）回填——旧实现恒拷
                        // 512B，多块读的调用方缓冲尾部停在调用前内容（测试
                        // 假绿：读到的是零缓冲区而非介质）。调用方
                        // （VirtioBlkBackend）逐 512B 块发起，故此处
                        // 512 ≤ data.len 恒成立。
                        let dd = data_desc.expect("READ 必有数据描述符");
                        let data_off = (dd.addr - base) as usize;
                        let disk = self.disk.borrow();
                        let n = dd.len as usize;
                        assert!(n <= 512 && start + 512 <= disk.len());
                        queue[data_off..data_off + n].copy_from_slice(&disk[start..start + n]);
                        queue[status_off] = 0;
                    }
                    1 => {
                        // WRITE：**按请求长度**写入（同读侧，旧实现恒 512B）。
                        let dd = data_desc.expect("WRITE 必有数据描述符");
                        let data_off = (dd.addr - base) as usize;
                        let mut d = self.disk.borrow_mut();
                        let n = dd.len as usize;
                        assert!(n <= 512 && start + 512 <= d.len());
                        d[start..start + n].copy_from_slice(&queue[data_off..data_off + n]);
                        queue[status_off] = 0;
                    }
                    4 => {
                        // FLUSH：无数据描述符（datalen=0）——仅 status。
                        self.flushes.set(self.flushes.get() + 1);
                        queue[status_off] = 0;
                    }
                    _ => {
                        queue[status_off] = 2; // IOERR
                    }
                }
                used.ring[used.idx as usize & (crate::QUEUE_MAX - 1)] = crate::VirtqUsedElem {
                    id: head as u32,
                    len: data_desc.map(|d| d.len).unwrap_or(0),
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
        let _seq = TEST_SEQ.lock().unwrap();
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

    /// R16-L（蓝图 75-03 WP-75.3.2）：health 折码规则——驱动
    /// health_status() 全包折成非零劣化码（现状红：恒 0）。阈值（wear>80 /
    /// life<20）为初版工程判据可调；多条件取最严重，优先级
    /// bad_blocks > wear_level > remaining_life。
    #[test]
    fn health_fold_code_rules() {
        let h = |bad: u64, wear: u8, life: u8| eneros_storage::DeviceHealth {
            total_blocks: 64,
            bad_blocks: bad,
            wear_level: wear,
            temperature: 0,
            remaining_life: life,
        };
        assert_eq!(fold_health_code(h(0, 0, 100)), 0, "健康态 = 0");
        assert_eq!(fold_health_code(h(1, 0, 100)), 2, "bad_blocks > 0 → 2");
        assert_eq!(fold_health_code(h(0, 81, 100)), 4, "wear_level > 80 → 4");
        assert_eq!(fold_health_code(h(0, 80, 100)), 0, "wear=80 边界不劣化");
        assert_eq!(fold_health_code(h(0, 0, 19)), 3, "remaining_life < 20 → 3");
        assert_eq!(fold_health_code(h(0, 0, 20)), 0, "life=20 边界不劣化");
        // 优先级：多条件取最严重。
        assert_eq!(fold_health_code(h(1, 90, 10)), 2, "bad_blocks 优先级最高");
        assert_eq!(fold_health_code(h(0, 90, 10)), 4, "wear_level 次之");
    }

    /// R16-L 接线守卫：health() 必须经折码单源（漏接线死代码/恒 0 回归
    /// 即红——75-02 漏接线教训）。token 运行期拼接防自匹配。
    #[test]
    fn health_is_wired_through_fold() {
        let src = include_str!("blk_backend.rs");
        let t = concat!("fold_health_code(d.he", "alth_status())");
        assert_eq!(
            src.matches(t).count(),
            1,
            "health() 折码接线缺失/漂移（应恰一处经 fold_health_code）"
        );
    }

    /// run15 实弹回归锁：boot 面以 **512B 扇区 LBA** 寻址（boot_disk
    /// 同口径），inner 字节偏移换算只在 BootSlotBackend 边界。直传扇区
    /// 值曾读回 GPT 区垃圾（"bootctrl-both-invalid"）。同时锁写守卫：
    /// 非活动槽/BOOTCTRL 之外零写（活跃槽写拒绝）。
    #[test]
    fn boot_face_uses_sector_lba_and_guard_writes() {
        let _seq = TEST_SEQ.lock().unwrap();
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
        let guard = crate::boot_guard::BootWriteGuard::resolve_with_running(
            &record,
            &record,
            test_running_identity(),
        )
        .expect("factory 记录可解析");
        let mut boot = BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr: 0,
            shadow: core::cell::RefCell::new([0u8; 2 * BOOT_CONTROL_RECORD_SIZE]),
            shadow_valid: core::cell::Cell::new(true),
        };
        // 扇区 LBA 100 → 字节 51200（换算生效）；扇区 0 保持零。
        // boot 面一律走具名单位方法（supertrait 已拆除——继承名 read/write
        // 不复存在，裸名混用从编译期不可表示）。
        let mut buf = [0u8; 10];
        boot.read_sectors(100, &mut buf).unwrap();
        assert_eq!(&buf, b"SECTOR100!");
        let mut z = [0u8; 4];
        boot.read_sectors(0, &mut z).unwrap();
        assert!(z == [0u8; 4], "扇区 0 不得读到字节 0 偏移的地标");

        // 写守卫：factory A=trial(15) 活跃 → 非活动 = BOOT_B；BOOT_B 内
        // 写放行，活跃槽/BOOTCTRL 区外写拒绝。
        let ranges = crate::block_cmd::InstallBackend::install_ranges(&boot);
        assert!(ranges.inactive_lbas > 0 && ranges.bootctrl_lbas > 0);
        let (is, bs_, bl_) = (
            ranges.inactive_start_lba,
            ranges.bootctrl_start_lba,
            ranges.bootctrl_lbas,
        );
        assert!(boot.write_sectors(bs_, b"g").is_ok(), "BOOTCTRL 内写放行");
        assert!(boot.write_sectors(is, b"g").is_ok(), "非活动槽内写放行");
        assert!(boot.write_sectors(0, b"x").is_err(), "活跃槽零写");
        assert!(
            boot.write_sectors(bs_ + bl_, b"x").is_err(),
            "越 BOOTCTRL 尾零写"
        );

        // REG-70.7-BOOT-TRIAL-IO 回归锁（run54 实弹）：守卫判定是**扇区**
        // 单位——4096B 记录写（8 扇区）必须放行。旧实现在此按字节与 lba
        // 相加（18432+4096 越出 16 扇区 BOOTCTRL 区间）→ 安装事务恒
        // ST_UNAVAILABLE（`BOOT-TRIAL: rejected (io)`）。
        use crate::block_cmd::InstallBackend as _;
        assert!(
            boot.write_allowed(bs_, 4096),
            "4096B BOOTCTRL 记录写必须放行（扇区换算）"
        );
        assert!(
            boot.write_allowed(bs_ + 8, 4096),
            "副本 1 区间 [bs+8, bs+16) 内的记录写必须放行"
        );
        assert!(
            boot.write_allowed(bs_, 4096 * 2),
            "整分区写（双副本 8192B = 16 扇区）放行"
        );
        assert!(
            !boot.write_allowed(bs_ + bl_ - 4, 4096),
            "跨界写拒绝（尾前 4 扇区 + 8 扇区 > BOOTCTRL 分区）"
        );
        assert!(
            !boot.write_allowed(bs_ + bl_, 1),
            "BOOTCTRL 尾后一字节仍拒绝"
        );
        // 端到端：二副本记录写真实落盘（run54 曾 Err）。副本 1 基址 =
        // BOOTCTRL 基址 + 双副本间距（布局单源常量）。
        let rec = [0xA5u8; BOOT_CONTROL_RECORD_SIZE];
        boot.write_sectors(bs_, &rec)
            .expect("BOOTCTRL 副本 0 记录写");
        boot.write_sectors(bs_ + BOOTCTRL_COPY_STRIDE_LBAS, &rec)
            .expect("BOOTCTRL 副本 1 记录写");
        let mut back = [0u8; BOOT_CONTROL_RECORD_SIZE];
        boot.read_sectors(bs_ + BOOTCTRL_COPY_STRIDE_LBAS, &mut back)
            .expect("读回副本 1");
        assert_eq!(back, rec, "记录写→影子/介质读回一致");
    }

    /// 70.14 复查红测：**boot 面 flush 必须下沉到设备**。
    ///
    /// 缺陷形态（旧实现）：`BootSlotBackend::flush` 恒 `Ok(())`，不调
    /// `self.inner.flush()`。而 `BootInstall` 的四个事务（trial/confirm/
    /// cancel/install）把"双副本写 + flush"当掉电持久化承诺（高序列副本先
    /// 写、低序列后写，flush 后才算提交）——flush 为空实现即承诺不成立：
    /// 掉电后设备写缓存里可能只有低序列副本，resolve 回到旧序列 = 事务
    /// 自认成功却消失。
    #[test]
    fn boot_face_flush_reaches_the_device() {
        let _seq = TEST_SEQ.lock().unwrap();
        let blocks = 40000u64;
        let mmio = SimMmio::new(blocks);
        let flushes = mmio.flushes.clone();
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
        let inner: VirtioBlkBackend<SimMmio, TestPool, _> = VirtioBlkBackend {
            driver: RefCell::new(driver),
            block_size: 512,
        };
        let record = eneros_boot_protocol::BootControl::factory([7u8; 16], 1, [7u8; 16])
            .encode()
            .unwrap();
        let guard = crate::boot_guard::BootWriteGuard::resolve_with_running(
            &record,
            &record,
            test_running_identity(),
        )
        .expect("factory 记录可解析");
        let mut boot = BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr: 0,
            shadow: core::cell::RefCell::new([0u8; 2 * BOOT_CONTROL_RECORD_SIZE]),
            shadow_valid: core::cell::Cell::new(true),
        };
        use crate::block_cmd::InstallBackend as _;
        let before = flushes.get();
        boot.flush().expect("boot 面 flush");
        assert_eq!(
            flushes.get(),
            before + 1,
            "boot 面 flush 必须产生一次设备 FLUSH（旧实现恒 Ok 不透传 = 事务持久化承诺落空）"
        );
    }

    /// 70.7 复查回归锁：BOOTCTRL 的**部分写**不得让影子与介质分叉。
    ///
    /// 缺陷形态（旧实现）：`write` 只在「整条 4096B 落在副本基址」时同步
    /// 影子，其余形状返回 `Ok` 却静默跳过——后续 `read_current`（走影子）
    /// 把**旧记录**当权威，`write_bootctrl_trial` 基于陈旧基线规划事务
    /// （旧活跃槽/旧序列），写出的记录与介质实际状态不符。
    ///
    /// 本锁：① 预置记录 A 的双副本；② 对副本 0 做 1B 部分写 → 读回必须
    /// 反映「介质现状」（A 被改后的字节），而不是被跳过同步后的旧值；
    /// ③ 整条记录写仍按新内容生效。
    #[test]
    fn bootctrl_partial_write_refreshes_shadow_not_stale() {
        let _seq = TEST_SEQ.lock().unwrap();
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
        // 记录 A：可解析（双 successful 稳态）——经真实驱动写入两副本。
        let id = [9u8; 16];
        let mut ctl = eneros_boot_protocol::BootControl::factory(id, 1, id);
        ctl.slot_a = eneros_boot_protocol::SlotState::successful(15, 1, id);
        ctl.slot_b = eneros_boot_protocol::SlotState::successful(14, 1, id);
        let rec_a = ctl.encode().expect("encode A");
        use crate::block_cmd::BlockBackend as _;
        inner.write(18432 * 512, &rec_a).expect("写副本 0");
        inner.write(18440 * 512, &rec_a).expect("写副本 1");

        let guard = crate::boot_guard::BootWriteGuard::resolve_with_running(
            &rec_a,
            &rec_a,
            test_running_identity(),
        )
        .expect("resolve");
        let mut boot = BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr: 0,
            // 影子 = init 期读入的双副本（与 try_new_boot_once 同形态）。
            shadow: core::cell::RefCell::new({
                let mut s = [0u8; 2 * BOOT_CONTROL_RECORD_SIZE];
                s[..BOOT_CONTROL_RECORD_SIZE].copy_from_slice(&rec_a);
                s[BOOT_CONTROL_RECORD_SIZE..].copy_from_slice(&rec_a);
                s
            }),
            shadow_valid: core::cell::Cell::new(true),
        };

        // install_ranges 字段名进类型（73 批A S1：位置解构不可表示）。
        let bs_ = crate::block_cmd::InstallBackend::install_ranges(&boot).bootctrl_start_lba;
        assert_eq!(bs_, 18432, "qemu-v1 BOOTCTRL 基址");
        use crate::block_cmd::InstallBackend as _;
        boot.write_sectors(bs_, &[0xEE, 0xEE, 0xEE])
            .expect("部分写必须放行");
        // ② 读回副本 0：必须看到介质现状（含 0xEE），而不是旧影子。
        let mut back = [0u8; BOOT_CONTROL_RECORD_SIZE];
        boot.read_sectors(bs_, &mut back).expect("读回副本 0");
        assert_eq!(
            &back[..3],
            &[0xEE, 0xEE, 0xEE],
            "部分写后影子必须反映介质现状（旧实现此处回陈旧记录）"
        );
        // 记录中段本身多为零（BootControl 头部保留区/槽间隙），故取
        // **尾部实测非零字节**作「未被清零」的判据（CRC 域 4092..4096）。
        for i in 3..64usize {
            assert_eq!(
                back[i], rec_a[i],
                "字节 {i}: 部分写后影子必须保持记录 A 的该字节（旧实现整记录陈旧/被清零）"
            );
        }
        assert_ne!(&back[4092..4096], &[0u8; 4], "CRC 域不得被清零");
        assert_eq!(&back[4092..4096], &rec_a[4092..4096], "CRC 与记录 A 一致");
        // ③ 整条记录写（生产唯一形态）仍直接生效。
        let rec_b = [0x5Au8; BOOT_CONTROL_RECORD_SIZE];
        boot.write_sectors(bs_, &rec_b).expect("整条记录写");
        let mut back2 = [0u8; BOOT_CONTROL_RECORD_SIZE];
        boot.read_sectors(bs_, &mut back2).expect("读回副本 0");
        assert_eq!(back2, rec_b, "整条记录写覆盖影子");
    }

    /// 73 批A（T-unit）：双平面单位契约现状锁——同一介质、同一数值实参，
    /// 两平面读到**不同**字节。boot 面（BootSlotBackend）`lba=1` 读到的是
    /// 字节 512..（512B 扇区语义）；数据面（VirtioBlkBackend）同数值 `1`
    /// 读到的是字节 1..（字节偏移语义）。§1.2 的同名异单位分裂由本测变成
    /// 可执行事实；C9 落地后 boot 面改走 `read_sectors` 断言同一语义。
    #[test]
    fn dual_plane_unit_contract_boot_sectors_vs_data_bytes() {
        let _seq = TEST_SEQ.lock().unwrap();
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
        use crate::block_cmd::BlockBackend as _;
        // 地标：字节 0..512 填 0xAA（扇区 0），字节 512..1024 填 0xBB（扇区 1）。
        inner
            .write(0, &[0xAA; 512])
            .expect("数据面写扇区 0（字节 0）");
        inner
            .write(512, &[0xBB; 512])
            .expect("数据面写扇区 1（字节 512）");
        let record = eneros_boot_protocol::BootControl::factory([7u8; 16], 1, [7u8; 16])
            .encode()
            .unwrap();
        let guard = crate::boot_guard::BootWriteGuard::resolve_with_running(
            &record,
            &record,
            test_running_identity(),
        )
        .expect("factory 记录可解析");
        let mut boot = BootSlotBackend {
            inner,
            guard,
            boot_dma_vaddr: 0,
            shadow: core::cell::RefCell::new([0u8; 2 * BOOT_CONTROL_RECORD_SIZE]),
            shadow_valid: core::cell::Cell::new(true),
        };
        // boot 面：数值 1 = 扇区 LBA 1 → 字节 512..（0xBB 地标）。
        // C9 落地后走具名单位方法 read_sectors（同一语义）。
        use crate::block_cmd::InstallBackend as _;
        let mut sector_view = [0u8; 8];
        boot.read_sectors(1, &mut sector_view)
            .expect("boot 面读 lba=1");
        assert_eq!(
            sector_view, [0xBB; 8],
            "boot 面 lba=1 必须落在字节 512（512B 扇区语义）"
        );
        // 数据面：同一数值 1 = 字节偏移 1 → 字节 1..（0xAA 地标）。
        let mut byte_view = [0u8; 8];
        crate::block_cmd::BlockBackend::read(&mut boot.inner, 1, &mut byte_view)
            .expect("数据面读 offset=1");
        assert_eq!(
            byte_view, [0xAA; 8],
            "数据面 offset=1 必须落在字节 1（字节偏移语义）"
        );
    }
}
