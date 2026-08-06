//! virtio-blk 驱动（DRV-007）：read/write/flush/FUA/error/reset。
//!
//! 基于 split virtqueue + virtio-mmio transport。请求 = 3 段 desc 链
//! （header 16B + data + status 1B）。FLUSH 语义：VirtioBlkF::FLUSH；
//! FUA 语义：QEMU virtio-blk 的 write cache 由 CONFIG_WCE 控制，驱动
//! 在写后显式 FLUSH 等价 FUA（写透模式由 VIRTIO_BLK_F_CONFIG_WCE 下
//! 的 wce 配置位保证）。
//!
//! 错误路径：status 非 0 → IOERR/UNSUPP；设备复位（DRV-007 reset）：
//! 清 status + 队列 ready + 重新初始化。

use core::ptr;

use eneros_libeneros::{
    dma_alloc, dma_cache, dma_free, emit_evidence_id,
    irq_bind, shm_map_device,
};
use eneros_abi_syscall::{
    DMA_CACHE_CLEAN, DMA_CACHE_INVALIDATE, ERR_OK,
};

use crate::mmio::barrier;
use crate::virtio_mmio::{
    probe, VirtioMmio, VirtQueue, QUEUE_MEM_SIZE, REG_CONFIG,
    STATUS_ACKNOWLEDGE, STATUS_DRIVER, STATUS_FEATURES_OK, STATUS_DRIVER_OK,
    VirtqAvail,
};

/// virtio-blk 特性位。
pub const VIRTIO_BLK_F_SIZE_MAX: u32 = 1 << 1;
pub const VIRTIO_BLK_F_SEG_MAX: u32 = 1 << 2;
pub const VIRTIO_BLK_F_GEOMETRY: u32 = 1 << 4;
pub const VIRTIO_BLK_F_RO: u32 = 1 << 5;
pub const VIRTIO_BLK_F_BLK_SIZE: u32 = 1 << 6;
pub const VIRTIO_BLK_F_FLUSH: u32 = 1 << 9;
pub const VIRTIO_BLK_F_TOPOLOGY: u32 = 1 << 10;
pub const VIRTIO_BLK_F_CONFIG_WCE: u32 = 1 << 11;
pub const VIRTIO_BLK_F_DISCARD: u32 = 1 << 13;
pub const VIRTIO_BLK_F_WRITE_ZEROES: u32 = 1 << 14;

/// 请求类型。
pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
pub const VIRTIO_BLK_T_GET_ID: u32 = 8;

/// status 值。
pub const VIRTIO_BLK_S_OK: u8 = 0;
pub const VIRTIO_BLK_S_IOERR: u8 = 1;
pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// 配置区（blk 专属）。
pub const CONFIG_CAPACITY: usize = 0x00; // u64 扇区数
pub const CONFIG_BLK_SIZE: usize = 0x14; // u32（VERSION_1 后）

/// 请求头（16B）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BlkReqHeader {
    pub req_type: u32,
    pub reserved: u32,
    pub sector: u64,
}

/// virtio-blk 设备。
pub struct VirtioBlk {
    pub dev: VirtioMmio,
    pub queue: VirtQueue,
    /// 扇区数。
    pub capacity: u64,
    /// 逻辑块大小。
    pub block_size: u32,
    /// 支持 FLUSH。
    pub flush_supported: bool,
    /// 写缓存使能（CONFIG_WCE）。
    pub wce: bool,
    /// 请求缓冲（header + status，DMA 连续）。
    pub req_va: usize,
    pub req_phys: u64,
    /// 数据缓冲（最大 4MB-16B，DMA 连续）。
    pub data_va: usize,
    pub data_phys: u64,
    pub data_size: usize,
}

/// 设备复位后重新初始化。
pub fn init(
    base_phys: usize,
    data_size: usize,
) -> Result<VirtioBlk, &'static str> {
    let mut dev = probe(base_phys)?;
    if dev.device_id != crate::virtio_mmio::DEVICE_ID_BLK {
        return Err("not a blk device");
    }
    let features = dev.device_features();
    // 协商：支持 FLUSH/WCE（掉电一致性必需）+ BLK_SIZE。
    let accepted = features
        & (VIRTIO_BLK_F_FLUSH | VIRTIO_BLK_F_CONFIG_WCE | VIRTIO_BLK_F_BLK_SIZE);
    dev.set_driver_features(accepted)?;
    let flush_supported = features & VIRTIO_BLK_F_FLUSH != 0;
    let wce_supported = features & VIRTIO_BLK_F_CONFIG_WCE != 0;
    let queue = VirtQueue::setup(&mut dev, 0, 64)?;
    dev.driver_ok()?;
    let capacity = dev.config_read32(CONFIG_CAPACITY) as u64
        | ((dev.config_read32(CONFIG_CAPACITY + 4) as u64) << 32);
    let block_size = if features & VIRTIO_BLK_F_BLK_SIZE != 0 {
        dev.config_read32(CONFIG_BLK_SIZE)
    } else {
        512
    };
    if capacity == 0 || block_size == 0 {
        return Err("bad blk geometry");
    }
    // 分配请求缓冲（header 16B + status 1B）与数据缓冲。
    let (status, req_va, req_phys) = dma_alloc(4096);
    if status != 0 {
        return Err("dma_alloc header failed");
    }
    let data_size = if data_size == 0 { 4096 } else { data_size };
    let (status, data_va, data_phys) = dma_alloc(data_size as u64);
    if status != 0 {
        dma_free(req_va);
        return Err("dma_alloc data failed");
    }
    let req_va = req_va as usize;
    let data_va = data_va as usize;
    // wce 默认开启（QEMU 默认 writeback；FLUSH 语义保证持久化）。
    let wce = wce_supported;
    Ok(VirtioBlk {
        dev,
        queue,
        capacity,
        block_size,
        flush_supported,
        wce,
        req_va,
        req_phys,
        data_va,
        data_phys,
        data_size,
    })
}

impl VirtioBlk {
    /// 提交请求并等待完成（轮询 used ring；IRQ 由服务循环驱动，
    /// 本函数在收到 IRQ 后调用以消费 used）。
    pub fn submit_request(
        &mut self,
        req_type: u32,
        sector: u64,
        data_len: u32,
        data_write: bool,
    ) -> Result<(), u8> {
        // 填请求头。
        // SAFETY：DMA 缓冲已映射。
        unsafe {
            let header = self.req_va as *mut BlkReqHeader;
            ptr::write_volatile(
                header,
                BlkReqHeader {
                    req_type,
                    reserved: 0,
                    sector,
                },
            );
            // status 字节清零。
            ptr::write_volatile((self.req_va + 0x1000 - 1) as *mut u8, 0xFF);
        }
        // 数据方向：IN → 设备写（desc WRITE）；OUT → 设备读。
        let chain = [
            (self.req_va, 16u32, false),
            (self.data_va, data_len, data_write),
            (self.req_va + 0x1000 - 1, 1u32, true),
        ];
        // 数据提交前 clean（ToDevice）；header 也 clean。
        dma_cache(self.req_va as u64, 16, DMA_CACHE_CLEAN);
        if data_write {
            dma_cache(self.data_va as u64, data_len as u64, DMA_CACHE_CLEAN);
        }
        let head = self.queue.alloc_chain(&chain).map_err(|_| VIRTIO_BLK_S_IOERR)?;
        self.queue.submit(head);
        barrier();
        self.dev.notify(0);
        // 等待完成：轮询 used（服务循环在 IRQ 后调用；此处也兜底轮询）。
        loop {
            if let Some((_id, _len)) = self.queue.pop_used() {
                break;
            }
            core::hint::spin_loop();
        }
        // 读回 status；数据完成时 invalidate（FromDevice）。
        let status = unsafe { ptr::read_volatile((self.req_va + 0x1000 - 1) as *const u8) };
        if !data_write {
            dma_cache(self.data_va as u64, data_len as u64, DMA_CACHE_INVALIDATE);
        }
        if status == VIRTIO_BLK_S_OK {
            Ok(())
        } else {
            Err(status)
        }
    }

    /// 读扇区（sector 单位 block_size）。
    pub fn read(&mut self, sector: u64, blocks: u32) -> Result<(), u8> {
        let len = blocks * self.block_size;
        if len as usize > self.data_size {
            return Err(VIRTIO_BLK_S_IOERR);
        }
        self.submit_request(VIRTIO_BLK_T_IN, sector, len, false)
    }

    /// 写扇区。
    pub fn write(&mut self, sector: u64, blocks: u32) -> Result<(), u8> {
        let len = blocks * self.block_size;
        if len as usize > self.data_size {
            return Err(VIRTIO_BLK_S_IOERR);
        }
        self.submit_request(VIRTIO_BLK_T_OUT, sector, len, true)
    }

    /// FLUSH（持久化到介质；FUA 等价路径）。
    pub fn flush(&mut self) -> Result<(), u8> {
        if !self.flush_supported {
            return Err(VIRTIO_BLK_S_UNSUPP);
        }
        self.submit_request(VIRTIO_BLK_T_FLUSH, 0, 0, false)
    }

    /// 设备复位（DRV-007 reset）：完整重新初始化。
    pub fn reset_device(&mut self) -> Result<(), &'static str> {
        self.dev.reset();
        let features = self.dev.device_features();
        let accepted = features
            & (VIRTIO_BLK_F_FLUSH | VIRTIO_BLK_F_CONFIG_WCE | VIRTIO_BLK_F_BLK_SIZE);
        self.dev.set_driver_features(accepted)?;
        self.queue = VirtQueue::setup(&mut self.dev, 0, 64)?;
        self.dev.driver_ok()?;
        Ok(())
    }

    /// 数据缓冲访问。
    pub fn data(&self) -> *mut u8 {
        self.data_va as *mut u8
    }
}
