// 快速验证：virtio-blk 上 Lfs::format
use eneros_storage::{NoopCache, RamMmio, RamPool, StorageConfig, StorageType, VirtioBlkDriver, BlockDevice};
use eneros_fs::Lfs;
fn main() {
    for (blocks, bs) in [(64usize, 4096usize), (256, 4096), (512, 4096), (1024, 4096)] {
        let mmio = RamMmio::new((blocks * bs / 512) as u64);
        let pool = RamPool::new(9);
        let cfg = StorageConfig { storage_type: StorageType::VirtioBlk, base_addr: 0, irq_num: 0, dma_channel: 0, block_size: bs, max_transfer_blocks: 256 };
        let mut dev = VirtioBlkDriver::new(mmio, pool, NoopCache, cfg).expect("init");
        dev.set_block_count(blocks as u64);
        let dev: Box<dyn BlockDevice> = Box::new(dev);
        match Lfs::format(dev) {
            Ok(_) => println!("format OK blocks={blocks} bs={bs}"),
            Err(e) => println!("format FAIL blocks={blocks} bs={bs}: {e:?}"),
        }
    }
}
