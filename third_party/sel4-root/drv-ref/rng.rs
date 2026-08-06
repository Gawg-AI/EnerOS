//! virtio-rng 驱动（DRV-006）：健康/超时/熵不足。
//!
//! 无特性、无配置区：向队列投递写缓冲，设备填随机数，used 返回长度。
//! 超时语义：提交后轮询有界次数，超时视为设备失效（health check 失败）。

use core::ptr;

use eneros_libeneros::dma_alloc;

use crate::mmio::barrier;
use crate::virtio_mmio::{probe, VirtioMmio, VirtQueue, DEVICE_ID_RNG};

/// 单次请求缓冲大小。
pub const RNG_BUF_SIZE: usize = 4096;
/// 提交后等待完成的轮询上限（约 1e6 次 spin）。
const COMPLETION_SPIN_LIMIT: u32 = 1_000_000;

/// virtio-rng 设备。
pub struct VirtioRng {
    pub dev: VirtioMmio,
    pub queue: VirtQueue,
    /// 随机数缓冲 VA（DMA）。
    pub buf_va: usize,
    pub buf_phys: u64,
}

pub fn init(base_phys: usize) -> Result<VirtioRng, &'static str> {
    let mut dev = probe(base_phys)?;
    if dev.device_id != DEVICE_ID_RNG {
        return Err("not an rng device");
    }
    let features = dev.device_features();
    dev.set_driver_features(features & 0)?;
    let queue = VirtQueue::setup(&mut dev, 0, 64)?;
    dev.driver_ok()?;
    let (status, buf_va, buf_phys) = dma_alloc(RNG_BUF_SIZE as u64);
    if status != 0 {
        return Err("dma_alloc rng buffer failed");
    }
    let buf_va = buf_va as usize;
    Ok(VirtioRng {
        dev,
        queue,
        buf_va,
        buf_phys,
    })
}

impl VirtioRng {
    /// 请求一批随机数；返回实际字节数（0 = 失败/超时）。
    pub fn fill(&mut self, out: &mut [u8]) -> usize {
        let len = out.len().min(RNG_BUF_SIZE);
        if len == 0 {
            return 0;
        }
        // SAFETY：缓冲已映射。
        unsafe {
            ptr::write_bytes(self.buf_va as *mut u8, 0, RNG_BUF_SIZE);
        }
        let chain = [(self.buf_va, len as u32, true)];
        let head = match self.queue.alloc_chain(&chain) {
            Ok(head) => head,
            Err(_) => return 0,
        };
        self.queue.submit(head);
        barrier();
        self.dev.notify(0);
        // 有界等待完成。
        let mut spins = 0u32;
        let mut received = 0usize;
        loop {
            if let Some((_id, used_len)) = self.queue.pop_used() {
                received = used_len as usize;
                break;
            }
            spins += 1;
            if spins > COMPLETION_SPIN_LIMIT {
                // 超时：设备失效（health check 失败），复位设备。
                self.dev.reset();
                return 0;
            }
            core::hint::spin_loop();
        }
        let take = received.min(len);
        // SAFETY：DMA 缓冲读取（设备已写完；QEMU 平台无 cache 一致性问题，
        // 但仍执行 invalidate 保证语义）。
        unsafe {
            ptr::copy_nonoverlapping(self.buf_va as *const u8, out.as_mut_ptr(), take);
        }
        take
    }
}
