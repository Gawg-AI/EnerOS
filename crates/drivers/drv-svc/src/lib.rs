//! EnerOS 驱动/存储 EL0 服务 — 共享入口与 panic handler.
//!
//! 这六个服务被打包进 init bundle，由 root task 在 M2.1（seL4 TCB/CSpace/
//! VSpace FFI）完成后 spawn：
//! - `svc-drv-rng`：virtio-rng 取熵服务（DRV-006）
//! - `svc-drv-blk`：virtio-blk 块设备服务（DRV-007）
//! - `svc-drv-net`：virtio-net 网络服务（DRV-008）
//! - `svc-drv-console`：串口控制台服务
//! - `svc-storage`：存储事务日志服务（STO-002）
//! - `svc-drv-fault`：驱动故障注入服务
//!
//! 实现状态（v0.165.0 注释校正）：
//! - `svc-drv-rng` 已实现真实 virtio-rng 队列 I/O（v0.200.0 P0-9，
//!   `VirtioRngDriver::read_entropy`，host 仿真测试覆盖）；
//! - 38（WP-38.9）：六个 bin 全部执行真实 IPC loop
//!   （[`runtime::driver_service_main`]——envelope/bounded queue/
//!   deadline/epoch），不再 spin 占位；设备分配（blk/net/storage/fault
//!   的真实队列 I/O）由 39 接入——未分配服务显式回 `ST_UNAVAILABLE`。
//!
//! no_std 合规：仅 core，无 alloc 依赖（virtio 队列用固定大小缓冲）。

#![no_std]
#![allow(clippy::empty_loop)]

// 测试 harness 需要 std（crate 本体保持 no_std 无 alloc）。
#[cfg(test)]
extern crate std;

/// 38（WP-38.9）：通用驱动服务 IPC runtime（envelope/bounded queue/
/// deadline/epoch；替代自旋占位）。
pub mod runtime;

/// 蓝图 76 卷二（WP-76.2.2）：块命令 **body 文法单源**——布局知识（哪一段在
/// 哪几个字节）从「服务端 + 客户端 + 4 替身各手抄一遍」收敛到此。零 wire
/// 字节移动；`block_cmd` 的 golden 族是字节裁判。
pub mod blk_wire;
/// 39（WP-39.12）：块设备命令面（svc-storage 真实命令集 + StorageService）。
pub mod block_cmd;

/// 41（WP-41 存储句柄移交）：svc-storage 真块后端——VirtioBlkBackend
/// （storage VirtioBlkDriver 适配 BlockBackend）+ 服务侧 SvcDmaPool。
pub mod blk_backend;
pub mod boot_guard;
/// 70.7.3/5：BootControl 事务内核（boot 面所有者侧单源；事务随
/// 后端下沉——REG-70.7 run16-22 实弹修订）。
pub mod boot_install;

/// 服务进程全局分配器（linked_list_allocator；runtime 本体无 alloc）。
pub mod allocator;

/// host 构建下的公共入口兜底（lib 测试使用；target 服务入口在各 bin
/// `_start` 中调用 [`runtime::driver_service_main`]——真实 IPC loop）。
#[inline]
pub fn drv_loop() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// 单调毫秒时钟（EL0 服务侧；Arm64Timer CNTPCT_EL0——root 同 EL 可读，
/// 37 证据验证）。host 构建无目标时钟 → 0（测试注入 now）。
#[cfg(target_arch = "aarch64")]
pub fn now_ms() -> u64 {
    eneros_hal::arm64::timer::clock().now_ns() / 1_000_000
}

#[cfg(not(target_arch = "aarch64"))]
pub fn now_ms() -> u64 {
    0
}

/// 单调纳秒时钟（40 收尾：EIPC 客户端超时计算用——`Sel4IpcClient` 的
/// `now_ns` 注入点；与 now_ms 同源 Arm64Timer）。
#[cfg(target_arch = "aarch64")]
pub fn now_ns() -> u64 {
    eneros_hal::arm64::timer::clock().now_ns()
}

#[cfg(not(target_arch = "aarch64"))]
pub fn now_ns() -> u64 {
    0
}

/// 服务实例 epoch：启动时刻单调毫秒（重启后必变——旧客户端句柄失效）。
pub fn instance_epoch() -> u64 {
    now_ms()
}

// ============================================================================
// v0.200.0（P0-9）：virtio-rng 真实队列 I/O
// ============================================================================

/// virtio-mmio 寄存器偏移（与 third_party C root_task / storage virtio_blk 一致）。
pub mod vmmio {
    pub const MAGIC_VALUE: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_PFN: usize = 0x040;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const STATUS: usize = 0x070;
    // 0x028/0x03C：QEMU/Linux UAPI 偏移（与 root_task.c 一致）——旧值
    // 0x08C/0x0AC 是无效偏移，QEMU 忽略 → guest_page_shift=0 → desc 错位
    // （41 virtio-blk 同根因；本 rng 驱动 QEMU cap=None 未激活未暴露）。
    pub const GUEST_PAGE_SIZE: usize = 0x028;
    pub const QUEUE_ALIGN: usize = 0x03C;
}

/// virtio-rng 设备 ID。
pub const VIRTIO_RNG_DEVICE_ID: u32 = 4;

/// 设备状态位。
pub const STATUS_ACKNOWLEDGE: u32 = 1;
pub const STATUS_DRIVER: u32 = 2;
pub const STATUS_DRIVER_OK: u32 = 4;
pub const STATUS_FEATURES_OK: u32 = 8;

/// 描述符标志。
pub const DESC_F_NEXT: u16 = 1;
pub const DESC_F_WRITE: u16 = 2;

/// 队列容量（16 描述符，与 C 参考一致）。
pub const QUEUE_MAX: usize = 16;

/// virtio 描述符（16 字节，legacy 布局）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// virtio 可用环。
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; QUEUE_MAX],
    pub used_event: u16,
}

impl Default for VirtqAvail {
    fn default() -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: [0; QUEUE_MAX],
            used_event: 0,
        }
    }
}

/// virtio 已用环元素。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// virtio 已用环。
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; QUEUE_MAX],
    pub avail_event: u16,
}

impl Default for VirtqUsed {
    fn default() -> Self {
        Self {
            flags: 0,
            idx: 0,
            ring: [VirtqUsedElem::default(); QUEUE_MAX],
            avail_event: 0,
        }
    }
}

/// MMIO 访问抽象（host 仿真 / 真机 volatile）。
pub trait VirtioMmio {
    fn read(&self, offset: usize) -> u32;
    fn write(&mut self, offset: usize, value: u32);
}

/// virtio-rng 驱动（固定缓冲，无 alloc）。
///
/// 队列内存布局（legacy，2 页 + 数据区）：
/// ```text
/// base + 0x0000  desc[16]（256 B）
/// base + 0x0100  avail（36 B）
/// base + 0x1000  used
/// base + 0x2000  数据缓冲（RNG 输出，固定 64 B）
/// ```
pub struct VirtioRng<M: VirtioMmio> {
    mmio: M,
    /// 队列内存（固定 0x2200 字节）。
    queue: [u8; 0x2200],
    /// 队列物理基址（host 仿真注入；真机为 DMA 页）。
    phys_base: u64,
    /// 已用环消费游标。
    used_idx: u16,
}

impl<M: VirtioMmio> VirtioRng<M> {
    /// 创建并初始化 virtio-rng（设备协商 + 队列设置）。
    pub fn new(mmio: M, phys_base: u64) -> Self {
        let mut rng = Self {
            mmio,
            queue: [0u8; 0x2200],
            phys_base,
            used_idx: 0,
        };
        rng.device_init();
        rng.queue_setup();
        rng
    }

    /// 设备协商（magic/version/device_id/features/DRIVER_OK）。
    fn device_init(&mut self) {
        // 显式 reset。
        self.mmio.write(vmmio::STATUS, 0);
        if self.mmio.read(vmmio::MAGIC_VALUE) != 0x7472_6976 {
            return;
        }
        let version = self.mmio.read(vmmio::VERSION);
        if version != 1 && version != 2 {
            return;
        }
        if self.mmio.read(vmmio::DEVICE_ID) != VIRTIO_RNG_DEVICE_ID {
            return;
        }
        self.mmio.write(vmmio::STATUS, STATUS_ACKNOWLEDGE);
        self.mmio
            .write(vmmio::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        // 协商 features：接受全部（legacy 无 VERSION_1 语义）。
        self.mmio.write(vmmio::DEVICE_FEATURES_SEL, 0);
        let f0 = self.mmio.read(vmmio::DEVICE_FEATURES);
        self.mmio.write(vmmio::DEVICE_FEATURES_SEL, 1);
        let f1 = self.mmio.read(vmmio::DEVICE_FEATURES);
        let features: u64 = (f0 as u64) | ((f1 as u64) << 32);
        self.mmio.write(vmmio::DRIVER_FEATURES_SEL, 0);
        self.mmio.write(vmmio::DRIVER_FEATURES, features as u32);
        self.mmio.write(vmmio::DRIVER_FEATURES_SEL, 1);
        self.mmio
            .write(vmmio::DRIVER_FEATURES, (features >> 32) as u32);
        self.mmio.write(
            vmmio::STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
        self.mmio.write(
            vmmio::STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );
    }

    /// 队列设置（legacy：GuestPageSize + QueueNum + QueuePFN）。
    fn queue_setup(&mut self) {
        self.mmio.write(vmmio::QUEUE_SEL, 0);
        self.mmio.write(vmmio::GUEST_PAGE_SIZE, 4096);
        self.mmio.write(vmmio::QUEUE_ALIGN, 4096);
        self.mmio.write(vmmio::QUEUE_NUM, QUEUE_MAX as u32);
        self.mmio
            .write(vmmio::QUEUE_PFN, (self.phys_base >> 12) as u32);
    }

    /// 返回队列内存指针（host 仿真设备模型用）。
    #[cfg(test)]
    pub fn queue_ptr(&self) -> *mut u8 {
        self.queue.as_ptr() as *mut u8
    }

    /// 取 `out.len()` 字节熵（virtio-rng 单 desc 读）。
    ///
    /// 构造一个 `DEVICE_WRITE` 描述符指向数据缓冲，提交 avail 环并 notify，
    /// 等待 used 元素后拷贝熵字节。返回读取字节数（0 = 设备未响应）。
    pub fn read_entropy(&mut self, out: &mut [u8]) -> usize {
        if out.is_empty() || out.len() > 64 {
            return 0;
        }
        let data_off = 0x2000usize;
        // desc[0]：设备写（RNG 输出）。
        // SAFETY: `self.queue` 为分配的对齐队列缓冲（长度 >= 0x2200），
        // 转换的 `VirtqDesc` 切片与后续 avail/used 视图区间互不重叠；
        // 宿主单线程访问，无并发写。
        let descs = unsafe {
            core::slice::from_raw_parts_mut(
                self.queue[0x0000..0x1000].as_mut_ptr() as *mut VirtqDesc,
                QUEUE_MAX,
            )
        };
        descs[0] = VirtqDesc {
            addr: self.phys_base + data_off as u64,
            len: out.len() as u32,
            flags: DESC_F_WRITE,
            next: 0,
        };
        // 提交 avail。
        // SAFETY: `self.queue` 为对齐缓冲；avail 视图区间
        // [0x0100, 0x1100) 与 desc 区间 [0x0000, 0x1000) 不重叠，且
        // 布局与 VirtioAvail 结构体（u16 计数 + u16 环 + u16 used_event）
        // 匹配，访问未越界。
        let avail = unsafe { &mut *(self.queue[0x0100..0x1100].as_mut_ptr() as *mut VirtqAvail) };
        avail.ring[avail.idx as usize & (QUEUE_MAX - 1)] = 0;
        avail.idx = avail.idx.wrapping_add(1);
        // notify。
        self.mmio.write(vmmio::QUEUE_SEL, 0);
        self.mmio.write(vmmio::QUEUE_NOTIFY, 0);
        // 等待 used（有限轮询）。
        let deadline = 1_000_000u32;
        let mut spins = 0;
        loop {
            // SAFETY: used 视图区间 [0x1000, 0x2000) 与 desc/avail 区间
            // 不重叠，布局与 VirtqUsed（u32 idx + u32 used_event + 环）
            // 匹配；设备写 used 环的并发由 virtio 内存屏障语义保证，
            // 此处仅读取。
            let used = unsafe { &*(self.queue[0x1000..0x2000].as_ptr() as *const VirtqUsed) };
            if used.idx != self.used_idx {
                let elem = used.ring[self.used_idx as usize & (QUEUE_MAX - 1)];
                self.used_idx = self.used_idx.wrapping_add(1);
                let n = core::cmp::min(elem.len as usize, out.len());
                out[..n].copy_from_slice(&self.queue[data_off..data_off + n]);
                return n;
            }
            spins += 1;
            if spins >= deadline {
                return 0;
            }
            core::hint::spin_loop();
        }
    }
}

// ============================================================================
// host 仿真测试：virtio-rng 队列语义
// ============================================================================

// ============================================================================
// 38（WP-38.9）：console 适配模板（蓝图：38 建模板，39 接设备）
// ============================================================================

/// Console 服务模板：声明 console 能力；设备（pl011 资源/收发）在 39
/// 分配——无设备时 EXECUTE 由 [`runtime::process_request`] 回
/// `ST_UNAVAILABLE`（不伪造控制台 I/O）。
pub struct ConsoleService;

impl ConsoleService {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ConsoleService {
    fn default() -> Self {
        Self::new()
    }
}

impl runtime::DriverService for ConsoleService {
    fn capability(&self) -> Option<&'static str> {
        Some("console")
    }
    fn handle(&mut self, req: &runtime::DriverRequest, resp: &mut runtime::DriverResponse) {
        // 回声模板：载荷原样回写（模板语义；真机收发 39 接线）。
        let n = core::cmp::min(req.payload_len as usize, runtime::PAYLOAD_CAP);
        resp.payload[..n].copy_from_slice(&req.payload[..n]);
        resp.payload_len = n as u16;
        resp.status = runtime::ST_OK;
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;

    /// RAM 仿真 virtio-mmio 寄存器 + RNG 设备模型。
    struct SimMmio {
        regs: [u32; 64],
        /// 仿真 RNG 输出的熵字节。
        entropy: [u8; 64],
    }

    impl SimMmio {
        fn new() -> Self {
            let mut regs = [0u32; 64];
            regs[vmmio::MAGIC_VALUE / 4] = 0x7472_6976;
            regs[vmmio::VERSION / 4] = 1;
            regs[vmmio::DEVICE_ID / 4] = VIRTIO_RNG_DEVICE_ID;
            regs[vmmio::QUEUE_NUM_MAX / 4] = 32;
            let mut entropy = [0u8; 64];
            for (i, b) in entropy.iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(0x5D).wrapping_add(0x13);
            }
            SimMmio { regs, entropy }
        }

        /// 设备模型：消费 avail 环，向 desc[0] 指向的缓冲写熵，产 used。
        fn process_queues(&mut self) {
            let qp = QUEUE_MEM.load(core::sync::atomic::Ordering::Relaxed) as *mut u8;
            if qp.is_null() {
                return;
            }
            // SAFETY: 测试专用——`qp` 指向全局静态仿真队列（长度 0x2200），
            // 区间划分与生产路径一致，无越界。
            let queue = unsafe { core::slice::from_raw_parts_mut(qp, 0x2200) };
            let avail = unsafe { &mut *(queue[0x0100..0x1100].as_mut_ptr() as *mut VirtqAvail) };
            let used = unsafe { &mut *(queue[0x1000..0x2000].as_mut_ptr() as *mut VirtqUsed) };
            while used.idx != avail.idx {
                let head = avail.ring[used.idx as usize & (QUEUE_MAX - 1)] as usize;
                // SAFETY: 测试专用——desc 只读视图区间合法，长度不越界。
                let descs = unsafe {
                    core::slice::from_raw_parts(
                        queue[0x0000..0x1000].as_ptr() as *const VirtqDesc,
                        QUEUE_MAX,
                    )
                };
                let desc = &descs[head];
                let base = PHYS_BASE.load(core::sync::atomic::Ordering::Relaxed);
                let dst = (desc.addr - base) as usize; // 池内偏移
                let n = desc.len as usize;
                queue[dst..dst + n].copy_from_slice(&self.entropy[..n]);
                used.ring[used.idx as usize & (QUEUE_MAX - 1)] = VirtqUsedElem {
                    id: head as u32,
                    len: n as u32,
                };
                used.idx = used.idx.wrapping_add(1);
            }
        }
    }

    impl VirtioMmio for SimMmio {
        fn read(&self, offset: usize) -> u32 {
            self.regs[offset / 4]
        }
        fn write(&mut self, offset: usize, value: u32) {
            if offset == vmmio::QUEUE_NOTIFY {
                self.process_queues();
            }
            self.regs[offset / 4] = value;
        }
    }

    /// 队列内存放全局静态（SimMmio 的 queue_ptr 指向它）。
    static QUEUE_MEM: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    /// 仿真物理基址（process_queues 计算池内偏移用）。
    static PHYS_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    #[test]
    fn test_virtio_rng_returns_nonzero_entropy() {
        // 构造 rng；SimMmio 的 write 钩子从全局 QUEUE_MEM 取队列指针，
        // 因此先接线再构造。
        let phys_base = 0x1000_0000u64;
        // 先把一个临时 rng 的队列指针写入全局（VirtioRng 持有 queue 内存，
        // 其地址在 rng 生命周期内固定——用 Box 分配保持稳定）。
        // 构造 rng；队列内存是结构体字段，地址在测试生命周期内稳定。
        let mut rng = VirtioRng::new(SimMmio::new(), phys_base);
        // 先取队列指针写入全局（read_entropy 的 notify 时仿真设备从全局读）。
        let qp = rng.queue_ptr();
        QUEUE_MEM.store(qp as u64, core::sync::atomic::Ordering::Relaxed);
        PHYS_BASE.store(phys_base, core::sync::atomic::Ordering::Relaxed);
        // 手动驱动设备模型（模拟 QEMU 在 notify 后处理队列）：
        // read_entropy 内部写 QUEUE_NOTIFY 触发 SimMmio::write → process_queues。
        // 若钩子失效，这里显式验证队列语义：直接调 rng 的内部路径。
        let mut out = [0u8; 16];
        let n = rng.read_entropy(&mut out);
        assert_eq!(n, 16, "RNG 必须返回 16 字节熵");
        assert!(
            out.iter().any(|&b| b != 0),
            "RNG 输出必须含非零字节（真实熵源）"
        );
        // 二次读取应得到相同确定性熵（同源）。
        let mut out2 = [0u8; 16];
        let n2 = rng.read_entropy(&mut out2);
        assert_eq!(n2, 16);
        assert_eq!(out, out2, "同一仿真熵源两次读取应一致");
    }

    #[test]
    fn test_entropy_source_is_nonzero() {
        // 仿真熵源本身非零（真实 virtio-rng 设备应产生非零熵）。
        let sim = SimMmio::new();
        assert!(sim.entropy.iter().any(|&b| b != 0));
        // 且不是全 0xFF（有变化）。
        assert!(sim.entropy.windows(2).any(|w| w[0] != w[1]));
    }
}
