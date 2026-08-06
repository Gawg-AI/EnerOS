//! virtio-mmio transport（DRV-006/007/008 共用）。
//!
//! 实现 virtio 1.0 modern MMIO transport（version=2）：设备发现、特性
//! 协商、split virtqueue 配置、通知、中断状态与设备复位。所有队列内存
//! 来自 SYS_DMA_ALLOC（物理连续），寄存器来自 SYS_SHM_MAP（设备窗口）。
//!
//! 引用：Virtual I/O Device (VIRTIO) Version 1.0，Section 4.2
//! "MMIO Transport"。

use core::ptr;

use eneros_libeneros::{dma_alloc, dma_free, shm_map_device};

use crate::mmio::{barrier, read32, write32, write64};

/// virtio-mmio 寄存器偏移（virtio 1.0 §4.2.2）。
pub const REG_MAGIC: usize = 0x000;
pub const REG_VERSION: usize = 0x004;
pub const REG_DEVICE_ID: usize = 0x008;
pub const REG_VENDOR_ID: usize = 0x00C;
pub const REG_DEVICE_FEATURES: usize = 0x010;
pub const REG_DEVICE_FEATURES_SEL: usize = 0x014;
pub const REG_DRIVER_FEATURES: usize = 0x018;
pub const REG_DRIVER_FEATURES_SEL: usize = 0x01C;
pub const REG_QUEUE_SEL: usize = 0x020;
pub const REG_QUEUE_NUM_MAX: usize = 0x024;
pub const REG_QUEUE_NUM: usize = 0x028;
pub const REG_QUEUE_READY: usize = 0x02C;
pub const REG_QUEUE_NOTIFY: usize = 0x030;
pub const REG_INTERRUPT_STATUS: usize = 0x034;
pub const REG_INTERRUPT_ACK: usize = 0x038;
pub const REG_STATUS: usize = 0x03C;
pub const REG_QUEUE_DESC_LOW: usize = 0x044;
pub const REG_QUEUE_DESC_HIGH: usize = 0x048;
pub const REG_QUEUE_DRIVER_LOW: usize = 0x050;
pub const REG_QUEUE_DRIVER_HIGH: usize = 0x054;
pub const REG_QUEUE_DEVICE_LOW: usize = 0x058;
pub const REG_QUEUE_DEVICE_HIGH: usize = 0x05C;
pub const REG_CONFIG_GENERATION: usize = 0x0FC;
/// 设备配置区起始。
pub const REG_CONFIG: usize = 0x100;

/// MagicValue 期望值 "virt"。
pub const MAGIC_VALUE: u32 = 0x7472_6976;
/// 支持的 transport 版本（modern）。
pub const VERSION_MODERN: u32 = 2;

/// 设备 ID：网络。
pub const DEVICE_ID_NET: u32 = 1;
/// 设备 ID：块。
pub const DEVICE_ID_BLK: u32 = 2;
/// 设备 ID：熵源。
pub const DEVICE_ID_RNG: u32 = 4;
/// 设备 ID：控制台。
pub const DEVICE_ID_CONSOLE: u32 = 16;
/// 设备 ID：GPIO。
pub const DEVICE_ID_GPIO: u32 = 17;
/// 设备 ID：FS。
pub const DEVICE_ID_FS: u32 = 21;

/// device status 位。
pub const STATUS_ACKNOWLEDGE: u32 = 1;
pub const STATUS_DRIVER: u32 = 2;
pub const STATUS_FAILED: u32 = 4;
pub const STATUS_FEATURES_OK: u32 = 8;
pub const STATUS_DRIVER_OK: u32 = 16;
pub const STATUS_DEVICE_NEEDS_RESET: u32 = 64;

/// 中断状态位：used ring 通知。
pub const ISR_USED_RING: u32 = 1;
/// 中断状态位：配置变更。
pub const ISR_CONFIG_CHANGE: u32 = 2;

/// 队列数上限（每设备）。
pub const MAX_QUEUES: usize = 2;

/// 队列深度（num；QEMU virtio-mmio 默认 256）。
pub const QUEUE_DEPTH: u16 = 64;

/// virtio-mmio 设备视图（寄存器 VA 来自 SYS_SHM_MAP）。
#[derive(Debug, Clone, Copy)]
pub struct VirtioMmio {
    /// 寄存器窗口 VA。
    pub regs_va: usize,
    /// 设备 ID（探测后）。
    pub device_id: u32,
    /// 队列数。
    pub queue_count: u32,
}

/// 探测并复位 virtio-mmio 设备。
pub fn probe(base_phys: usize) -> Result<VirtioMmio, &'static str> {
    let (status, va) = shm_map_device(base_phys as u64, 0x200);
    if status != 0 {
        return Err("shm_map_device failed");
    }
    let va = va as usize;
    let magic = read32(va + REG_MAGIC);
    if magic != MAGIC_VALUE {
        return Err("bad virtio magic");
    }
    let version = read32(va + REG_VERSION);
    if version != VERSION_MODERN {
        return Err("unsupported virtio version");
    }
    let device_id = read32(va + REG_DEVICE_ID);
    if device_id == 0 {
        return Err("no device");
    }
    // 复位设备：清 status。
    write32(va + REG_STATUS, 0);
    barrier();
    let mut dev = VirtioMmio {
        regs_va: va,
        device_id,
        queue_count: 0,
    };
    dev.reset();
    Ok(dev)
}

impl VirtioMmio {
    /// 设备复位（DRV-007/008 reset 路径）：清 status + 所有队列 ready。
    pub fn reset(&mut self) {
        write32(self.regs_va + REG_STATUS, 0);
        barrier();
        for queue in 0..MAX_QUEUES as u32 {
            write32(self.regs_va + REG_QUEUE_SEL, queue);
            write32(self.regs_va + REG_QUEUE_READY, 0);
        }
        barrier();
        self.queue_count = 0;
    }

    /// 读设备特性（低 32 位）。
    pub fn device_features(&self) -> u32 {
        write32(self.regs_va + REG_DEVICE_FEATURES_SEL, 0);
        read32(self.regs_va + REG_DEVICE_FEATURES)
    }

    /// 写驱动特性（低 32 位）并进入 FEATURES_OK。
    pub fn set_driver_features(&mut self, features: u32) -> Result<(), &'static str> {
        write32(self.regs_va + REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        write32(self.regs_va + REG_DRIVER_FEATURES_SEL, 0);
        write32(self.regs_va + REG_DRIVER_FEATURES, features);
        barrier();
        write32(self.regs_va + REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        barrier();
        let status = read32(self.regs_va + REG_STATUS);
        if status & STATUS_FEATURES_OK == 0 {
            return Err("features not accepted");
        }
        Ok(())
    }

    /// 进入 DRIVER_OK。
    pub fn driver_ok(&mut self) -> Result<(), &'static str> {
        write32(self.regs_va + REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
        barrier();
        let status = read32(self.regs_va + REG_STATUS);
        if status & STATUS_DRIVER_OK == 0 {
            return Err("driver ok not accepted");
        }
        Ok(())
    }

    /// 设备失败标记（错误路径诊断）。
    pub fn set_failed(&mut self) {
        let status = read32(self.regs_va + REG_STATUS);
        write32(self.regs_va + REG_STATUS, status | STATUS_FAILED);
        barrier();
    }

    /// 队列通知。
    pub fn notify(&self, queue: u32) {
        write32(self.regs_va + REG_QUEUE_NOTIFY, queue);
    }

    /// 读中断状态。
    pub fn interrupt_status(&self) -> u32 {
        read32(self.regs_va + REG_INTERRUPT_STATUS)
    }

    /// 清中断。
    pub fn ack_interrupt(&self, bits: u32) {
        write32(self.regs_va + REG_INTERRUPT_ACK, bits);
    }

    /// 读设备配置区（32 位）。
    pub fn config_read32(&self, offset: usize) -> u32 {
        read32(self.regs_va + REG_CONFIG + offset)
    }

    /// 读设备配置区（16 位）。
    pub fn config_read16(&self, offset: usize) -> u16 {
        read32(self.regs_va + REG_CONFIG + offset) as u16
    }
}

/// split virtqueue。
#[derive(Debug)]
pub struct VirtQueue {
    /// 队列索引。
    pub index: u32,
    /// 深度。
    pub num: u16,
    /// 队列内存 VA（DMA 窗口；desc/avail/used 物理连续）。
    pub mem_va: usize,
    /// desc 表物理地址（写入设备）。
    pub desc_phys: u64,
    /// avail 物理地址。
    pub avail_phys: u64,
    /// used 物理地址。
    pub used_phys: u64,
    /// desc 表分配游标。
    next_desc: core::sync::atomic::AtomicU16,
    /// used ring 消费游标。
    last_used: core::sync::atomic::AtomicU16,
}

/// desc 表项（16B）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// avail ring 头（后接 ring[num] + used_event）。
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; QUEUE_DEPTH as usize],
    pub used_event: u16,
}

/// used ring 头（后接 ring[num] + avail_event）。
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; QUEUE_DEPTH as usize],
    pub avail_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// 队列内存布局（页对齐，物理连续）。
pub const DESC_TABLE_SIZE: usize = 16 * QUEUE_DEPTH as usize;
pub const AVAIL_SIZE: usize = 6 + 2 * QUEUE_DEPTH as usize;
pub const USED_SIZE: usize = 6 + 8 * QUEUE_DEPTH as usize;
/// 单队列内存总大小（页对齐）。
pub const QUEUE_MEM_SIZE: usize = 0x4000;

impl VirtQueue {
    /// 分配并初始化队列（SYS_DMA_ALLOC + 寄存器配置）。
    ///
    /// 队列内存布局：desc[0..DESC_TABLE_SIZE)，avail 紧随其后（2 对齐），
    /// used 紧随 avail（4 对齐）；全部落在同一物理连续块内。
    pub fn setup(
        dev: &mut VirtioMmio,
        index: u32,
        num: u16,
    ) -> Result<VirtQueue, &'static str> {
        if index >= MAX_QUEUES as u32 {
            return Err("queue index out of range");
        }
        // 选择队列并读取设备支持的最大深度。
        write32(dev.regs_va + REG_QUEUE_SEL, index);
        let num_max = read32(dev.regs_va + REG_QUEUE_NUM_MAX);
        if num_max == 0 {
            return Err("queue not available");
        }
        let num = num.min(num_max as u16).min(QUEUE_DEPTH);
        let (status, va, phys) = dma_alloc(QUEUE_MEM_SIZE as u64);
        if status != 0 {
            return Err("dma_alloc queue failed");
        }
        let va = va as usize;
        // 清零队列内存。
        // SAFETY：DMA 窗口内已映射页，可写。
        unsafe {
            ptr::write_bytes(va as *mut u8, 0, QUEUE_MEM_SIZE);
        }
        // 布局：desc 对齐 16；avail 对齐 2（紧随 desc）；used 对齐 4。
        let desc_offset = 0usize;
        let avail_offset = align_up(DESC_TABLE_SIZE, 2);
        let used_offset = align_up(avail_offset + AVAIL_SIZE, 4);
        debug_assert!(used_offset + USED_SIZE <= QUEUE_MEM_SIZE);
        let queue = VirtQueue {
            index,
            num,
            mem_va: va,
            desc_phys: phys + desc_offset as u64,
            avail_phys: phys + avail_offset as u64,
            used_phys: phys + used_offset as u64,
            next_desc: core::sync::atomic::AtomicU16::new(0),
            last_used: core::sync::atomic::AtomicU16::new(0),
        };
        write32(dev.regs_va + REG_QUEUE_NUM, u32::from(num));
        write64(dev.regs_va + REG_QUEUE_DESC_LOW, queue.desc_phys);
        write64(dev.regs_va + REG_QUEUE_DRIVER_LOW, queue.avail_phys);
        write64(dev.regs_va + REG_QUEUE_DEVICE_LOW, queue.used_phys);
        barrier();
        write32(dev.regs_va + REG_QUEUE_READY, 1);
        barrier();
        if read32(dev.regs_va + REG_QUEUE_READY) != 1 {
            dma_free(va as u64);
            return Err("queue ready failed");
        }
        dev.queue_count = dev.queue_count.max(index + 1);
        Ok(queue)
    }

    /// desc 表访问（数组视图）。
    pub fn desc(&self) -> *mut VirtqDesc {
        self.mem_va as *mut VirtqDesc
    }

    /// avail 访问。
    pub fn avail(&self) -> *mut VirtqAvail {
        self.mem_va as *mut VirtqAvail
    }

    /// used 访问。
    pub fn used(&self) -> *mut VirtqUsed {
        self.mem_va as *mut VirtqUsed
    }

    /// 分配一个 desc 链（环形使用 desc 表；返回头 desc id）。
    ///
    /// `chain`：[(va, len, write)] 段列表。desc 表按 next_desc 游标
    /// 环形分配，链内 next 连接；段内物理连续（DMA 缓冲）。
    pub fn alloc_chain(
        &self,
        chain: &[(usize, u32, bool)],
    ) -> Result<u16, &'static str> {
        if chain.is_empty() || chain.len() > self.num as usize {
            return Err("bad chain");
        }
        // SAFETY：队列内存已映射；desc 表读写（内存布局 = num 个 16B 项）。
        let desc = unsafe { core::slice::from_raw_parts_mut(self.desc(), self.num as usize) };
        let head = self.next_desc.load(core::sync::atomic::Ordering::Relaxed) % self.num;
        // 检查链不会回绕覆盖未消费 desc（简化：头+长度不越过 num）。
        if (head as usize) + chain.len() > self.num as usize {
            // 回绕时跳到 0（调用方负责在 used 消费后调用）。
            self.next_desc.store(0, core::sync::atomic::Ordering::Relaxed);
        }
        let head = self.next_desc.load(core::sync::atomic::Ordering::Relaxed) % self.num;
        let mut prev: Option<u16> = None;
        for (position, &(va, len, write)) in chain.iter().enumerate() {
            let id = (head + position as u16) % self.num;
            let next = if position + 1 < chain.len() {
                (id + 1) % self.num
            } else {
                0
            };
            desc[id as usize] = VirtqDesc {
                addr: va as u64,
                len,
                flags: if write { VIRTQ_DESC_F_WRITE } else { 0 }
                    | if position + 1 < chain.len() {
                        VIRTQ_DESC_F_NEXT
                    } else {
                        0
                    },
                next,
            };
            if let Some(p) = prev {
                desc[p as usize].next = id;
            }
            prev = Some(id);
        }
        let advance = (head + chain.len() as u16) % self.num;
        self.next_desc
            .store(advance, core::sync::atomic::Ordering::Relaxed);
        Ok(head)
    }

    /// 提交 avail（发布 descriptor 链；设备通知由调用方执行）。
    pub fn submit(&self, head: u16) {
        // SAFETY：avail ring 写入。
        let avail = unsafe { &mut *self.avail() };
        let idx = avail.idx;
        avail.ring[(idx as usize) % self.num as usize] = head;
        barrier();
        avail.idx = idx.wrapping_add(1);
        barrier();
    }

    /// 取 used 项（返回 (desc_id, len)；无完成返回 None）。
    pub fn pop_used(&self) -> Option<(u32, u32)> {
        // SAFETY：used ring 读取。
        let used = unsafe { &*self.used() };
        let idx = used.idx;
        let last = self.last_used.load(core::sync::atomic::Ordering::Relaxed);
        if idx == last {
            return None;
        }
        let entry = used.ring[(last as usize) % self.num as usize];
        self.last_used
            .store(last.wrapping_add(1), core::sync::atomic::Ordering::Relaxed);
        Some((entry.id, entry.len))
    }
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
