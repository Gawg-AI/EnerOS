//! BlockIo —— durable-state `PersistIo` 的 EIPC 块客户端实现（蓝图 41
//! WP-41.3 落盘：业务服务 durable 状态经 EIPC 调 svc-storage 真落盘）。
//!
//! 协议：drv-svc 信封（`DriverRequest/DriverResponse`，83B ≤ MR 112B 预算）
//! 经 `Sel4IpcClient::request_raw` 同步调用（数据从 MR[0] 起——40 实弹
//! 修正的裸帧语义，与 svc-storage `driver_service_main` 的 MR 读取一致）。
//! 命令子码 = `block_cmd` 的 BLK_CMD_*（READ/WRITE/FLUSH/HEALTH）。
//!
//! fail-closed：存储能力未授予（EIPC mask 对应位为 0）→ `Unavailable`；
//! 信封响应 status != ST_OK → 对应 `StoreError`（不伪造成功）。

use eneros_drv_svc::block_cmd::{BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE};
use eneros_drv_svc::runtime::{wire, DriverRequest, DriverResponse, OP_EXECUTE, ST_OK};
use eneros_durable_state::{PersistIo, StoreError};
use eneros_storage::driver::{BlockDevice, DeviceHealth};
use eneros_storage::StorageError;

/// 块客户端：经 EIPC 调 svc-storage（X3 授予表槽位）。
pub struct BlockIo<C> {
    /// EIPC 客户端（Sel4IpcClient；host 测试注入模拟）。
    client: C,
    /// svc-storage 的 EIPC 目标名（诊断）。
    _target: &'static str,
    /// 服务 epoch（重启后变化——陈旧句柄显式失败）。
    epoch: u64,
    /// 超时（毫秒）。
    timeout_ms: u64,
}

impl<C> BlockIo<C> {
    pub fn new(client: C, _target: &'static str, epoch: u64, timeout_ms: u64) -> Self {
        BlockIo {
            client,
            _target,
            epoch,
            timeout_ms,
        }
    }
}

/// EIPC 发送 seam（host 可测；target 侧经 `Sel4IpcClient::drv_call`——
/// 4 字节/word 打包，与 svc-storage `driver_service_main` 的 MR 语义
/// 一致；`request_raw` 是 8 字节/word，混用错位截断）。
#[allow(clippy::result_unit_err)] // 44.2：EIPC seam 错误面为 ()（drv_call 语义），错误类型化属后续卷
pub trait EipcSend {
    fn drv_call(&mut self, req: &[u8], timeout_ns: u64) -> Result<alloc::vec::Vec<u8>, ()>;
}

#[cfg(target_os = "none")]
impl EipcSend for eneros_agent_sdk::ipc_sel4::Sel4IpcClient {
    fn drv_call(&mut self, req: &[u8], timeout_ns: u64) -> Result<alloc::vec::Vec<u8>, ()> {
        use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
        Sel4IpcClient::drv_call(self, req, timeout_ns).map_err(|_| ())
    }
}

impl<C: EipcSend> BlockIo<C> {
    /// 发送一个块命令信封，返回解码后的响应。失败 → StoreError。
    fn cmd(&mut self, sub: u8, payload: &[u8]) -> Result<DriverResponse, StoreError> {
        if payload.len() > eneros_drv_svc::runtime::PAYLOAD_CAP - 1 {
            return Err(StoreError::Other("block-payload-overflow"));
        }
        let mut req = DriverRequest::new(0xB10C, OP_EXECUTE);
        req.payload[0] = sub;
        req.payload[1..1 + payload.len()].copy_from_slice(payload);
        req.payload_len = 1 + payload.len() as u16;
        let enc = wire::encode_request(&req);
        let resp_bytes = self
            .client
            .drv_call(&enc, self.timeout_ms * 1_000_000)
            .map_err(|_| StoreError::Unavailable)?;
        // drv-svc 服务端按 4B/word 回 MR，`RESPONSE_LEN=79` 会以 20 words
        // （80B）返回；wire::decode_response 要求精确 79B，直接解码会
        // 误判 bad-envelope（QEMU 实测 EIPC 块读首包后 store-unavailable）。
        // 这里接受尾部 padding（≥79B），只解前 79B。
        let decode_len = core::cmp::min(
            resp_bytes.len(),
            eneros_drv_svc::runtime::wire::RESPONSE_LEN,
        );
        let resp = wire::decode_response(&resp_bytes[..decode_len])
            .ok_or(StoreError::Encode("bad-envelope"))?;
        // 首个响应吸收 epoch（业务服务无法预知 svc-storage 的实例 epoch）；
        // 之后 epoch 变化 → 陈旧句柄（重启后必须重建，不跨重启复用）。
        if self.epoch == 0 {
            self.epoch = resp.epoch;
        } else if resp.epoch != self.epoch {
            return Err(StoreError::Unavailable);
        }
        if resp.status != ST_OK {
            return Err(StoreError::WriteFailed);
        }
        Ok(resp)
    }

    /// 读 `offset` 起 `buf.len()` 字节（分块 ≤52B；lba = 字节偏移，
    /// 与 39 命令面 RamBackend 语义一致）。
    fn read_into(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        const CHUNK: usize = 52;
        let mut off = offset;
        let mut out = buf;
        while !out.is_empty() {
            let n = core::cmp::min(out.len(), CHUNK);
            let mut p = [0u8; 12];
            p[..8].copy_from_slice(&off.to_le_bytes());
            p[8..12].copy_from_slice(&(n as u32).to_le_bytes());
            let resp = self.cmd(BLK_CMD_READ, &p)?;
            let got = resp.payload_len as usize;
            if got < n {
                return Err(StoreError::Corrupt);
            }
            out[..n].copy_from_slice(&resp.payload[..n]);
            out = &mut out[n..];
            off += n as u64;
        }
        Ok(())
    }

    /// 写 `data` 到 `offset` 起（分块 ≤52B）。
    fn write_from(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        const CHUNK: usize = 52;
        let mut off = offset;
        let mut src = data;
        while !src.is_empty() {
            let n = core::cmp::min(src.len(), CHUNK);
            let mut p = [0u8; 8 + CHUNK];
            p[..8].copy_from_slice(&off.to_le_bytes());
            p[8..8 + n].copy_from_slice(&src[..n]);
            let _ = self.cmd(BLK_CMD_WRITE, &p[..8 + n])?;
            src = &src[n..];
            off += n as u64;
        }
        Ok(())
    }
}

impl<C: EipcSend> PersistIo for BlockIo<C> {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        self.read_into(offset, buf)
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        self.write_from(offset, data)
    }
    fn flush(&mut self) -> Result<(), StoreError> {
        let _ = self.cmd(BLK_CMD_FLUSH, &[])?;
        Ok(())
    }
    fn capacity(&self) -> u64 {
        // HEALTH 需要可变 self（cmd 是 &mut）——按 39 命令面语义：
        // capacity 未知时返回 0（调用方视为不可用）；本实现不支持
        // 只读查询（host 语义：调用方直接指定区域）。
        0
    }
}

/// 构造 EIPC block 客户端（target 侧；`slot` = X3 授予表槽位）。
#[cfg(target_os = "none")]
pub fn eipc_block_io(
    slot: u64,
    epoch: u64,
    timeout_ms: u64,
) -> Option<BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>> {
    let ipc =
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new("block-io", slot, eneros_drv_svc::now_ns)
            .ok()?;
    Some(BlockIo::new(ipc, "svc-storage", epoch, timeout_ms))
}

/// 将 [`BlockIo`]（EIPC 块客户端）适配为 storage crate 的 [`BlockDevice`]，
/// 供 `AbPartitionOta` 等 OTA 组件做真实 A/B 分区读写。
pub struct EipcBlockDevice<C> {
    io: core::cell::RefCell<BlockIo<C>>,
    block_size: usize,
    block_count: u64,
}

impl<C: EipcSend> EipcBlockDevice<C> {
    pub fn new(io: BlockIo<C>, block_size: usize, block_count: u64) -> Self {
        Self {
            io: core::cell::RefCell::new(io),
            block_size,
            block_count,
        }
    }
}

impl<C: EipcSend> BlockDevice for EipcBlockDevice<C> {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        let off = block_idx * self.block_size as u64;
        self.io
            .borrow_mut()
            .read(off, buf)
            .map_err(|_| StorageError::HardwareFault {
                detail: "eipc-block-read",
            })
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        if block_idx >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx,
                max: self.block_count,
            });
        }
        let off = block_idx * self.block_size as u64;
        self.io
            .borrow_mut()
            .write(off, buf)
            .map_err(|_| StorageError::HardwareFault {
                detail: "eipc-block-write",
            })
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        let zero = alloc::vec![0u8; self.block_size];
        self.write_block(block_idx, &zero)
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.io
            .borrow_mut()
            .flush()
            .map_err(|_| StorageError::HardwareFault {
                detail: "eipc-block-flush",
            })
    }

    fn health_status(&self) -> DeviceHealth {
        DeviceHealth {
            total_blocks: self.block_count,
            bad_blocks: 0,
            wear_level: 0,
            temperature: 0,
            remaining_life: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_ota_ab::upgrade::UpgradeMeta;
    use heapless::String as HString;

    use super::*;

    /// 内存仿真后端（host 测试；模拟 svc-storage 行为）。介质用
    /// Rc<RefCell> 共享——重启模拟复用同一持久化介质。
    struct SimStorage {
        data: std::rc::Rc<core::cell::RefCell<Vec<u8>>>,
        epoch: u64,
    }

    impl SimStorage {
        fn new(capacity: usize, epoch: u64) -> Self {
            SimStorage {
                data: std::rc::Rc::new(core::cell::RefCell::new(vec![0u8; capacity])),
                epoch,
            }
        }

        /// 共享既有介质（掉电/重启模拟：同一磁盘跨实例）。
        fn with_media(data: std::rc::Rc<core::cell::RefCell<Vec<u8>>>, epoch: u64) -> Self {
            SimStorage { data, epoch }
        }

        fn handle(&mut self, req: &[u8]) -> Vec<u8> {
            let dreq = wire::decode_request(req).unwrap();
            let mut resp = DriverResponse::new(dreq.correlation, ST_OK, self.epoch);
            let sub = dreq.payload[0];
            match sub {
                BLK_CMD_READ => {
                    let lba = u64::from_le_bytes(dreq.payload[1..9].try_into().unwrap());
                    let count =
                        u32::from_le_bytes(dreq.payload[9..13].try_into().unwrap()) as usize;
                    let start = lba as usize;
                    let data = self.data.borrow();
                    if start + count > data.len() {
                        resp.status = 4; // ST_UNAVAILABLE
                    } else {
                        resp.payload[..count].copy_from_slice(&data[start..start + count]);
                        resp.payload_len = count as u16;
                    }
                }
                BLK_CMD_WRITE => {
                    let lba = u64::from_le_bytes(dreq.payload[1..9].try_into().unwrap());
                    let payload = &dreq.payload[9..dreq.payload_len as usize];
                    let start = lba as usize;
                    let mut data = self.data.borrow_mut();
                    if start + payload.len() > data.len() {
                        resp.status = 4;
                    } else {
                        data[start..start + payload.len()].copy_from_slice(payload);
                    }
                }
                BLK_CMD_FLUSH => {}
                _ => resp.status = 5, // ST_INVALID
            }
            wire::encode_response(&resp).to_vec()
        }
    }

    struct SimClient {
        storage: SimStorage,
    }

    impl EipcSend for SimClient {
        fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
            Ok(self.storage.handle(req))
        }
    }

    #[test]
    fn block_io_roundtrip_via_eipc() {
        let storage = SimStorage::new(4096, 7);
        let client = SimClient { storage };
        let mut io = BlockIo::new(client, "svc-storage", 7, 1000);
        // 写 100 字节（跨块 52B 分块）。
        let payload: Vec<u8> = (0u8..100).collect();
        io.write(1024, &payload).unwrap();
        // 读回。
        let mut buf = vec![0u8; 100];
        io.read(1024, &mut buf).unwrap();
        assert_eq!(buf, payload);
        // flush。
        io.flush().unwrap();
        // 未写入区域保持零。
        let mut z = [0u8; 8];
        io.read(0, &mut z).unwrap();
        assert_eq!(z, [0u8; 8]);
    }

    #[test]
    fn eipc_block_device_read_write_roundtrip() {
        let storage = SimStorage::new(4096, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let mut dev = EipcBlockDevice::new(io, 512, 8);
        let data = [0xAAu8; 512];
        dev.write_block(2, &data).unwrap();
        let mut buf = [0u8; 512];
        dev.read_block(2, &mut buf).unwrap();
        assert_eq!(buf, data);
        assert_eq!(dev.block_count(), 8);
        assert_eq!(dev.block_size(), 512);
        dev.flush().unwrap();
        // erase 清零。
        dev.erase_block(3).unwrap();
        let mut erased = [0xFFu8; 512];
        dev.read_block(3, &mut erased).unwrap();
        assert_eq!(erased, [0u8; 512]);
        // 越界块拒绝。
        assert!(dev.read_block(8, &mut buf).is_err());
        assert!(dev.write_block(8, &data).is_err());
    }

    #[test]
    fn block_io_epoch_absorb_then_reject_stale() {
        let storage = SimStorage::new(4096, 7);
        let client = SimClient { storage };
        // 未知 epoch（0）→ 首个响应吸收（7）；随后响应仍 7 → 正常。
        let mut io = BlockIo::new(client, "svc-storage", 0, 1000);
        assert!(io.write(0, b"x").is_ok());
        assert_eq!(io.epoch, 7, "首个响应吸收服务 epoch");
        // 服务重启（epoch 变化 7→9）→ 陈旧句柄 → Unavailable（不跨重启）。
        let storage2 = SimStorage::new(4096, 9);
        let client2 = SimClient { storage: storage2 };
        let mut io2 = BlockIo::new(client2, "svc-storage", 0, 1000);
        let _ = io2.write(0, b"y"); // 吸收 9
                                    // 手动模拟服务重启后 epoch 变化（同一客户端，响应 epoch 不同）：
                                    // 构造一个 epoch 固定 9 的存储，先吸收再验证——直接构造不一致：
        let storage3 = SimStorage::new(4096, 5);
        let client3 = SimClient { storage: storage3 };
        let mut io3 = BlockIo::new(client3, "svc-storage", 0, 1000);
        // 首次吸收 5
        assert!(io3.write(0, b"z").is_ok());
        assert_eq!(io3.epoch, 5);
        // 模拟重启：底层响应 epoch 变为 11（新 SimStorage），客户端 epoch
        // 仍 5 → 拒绝。
        let storage4 = SimStorage::new(4096, 11);
        let client4 = SimClient { storage: storage4 };
        let mut io4 = BlockIo::new(client4, "svc-storage", 5, 1000);
        assert_eq!(io4.write(0, b"w"), Err(StoreError::Unavailable));
    }

    #[test]
    fn eipc_block_device_full_ota_download_verify_commit() {
        use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use crate::ota_service::{OtaPolicy, OtaService};
        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keygen");
        let pub_bytes = kp.public_key.to_bytes_uncompressed();
        let mut root_key = [0u8; 64];
        root_key.copy_from_slice(&pub_bytes[1..]);

        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let policy = OtaPolicy {
            partition_blocks: PARTITION_BLOCKS,
            chunk_size: 512,
            max_boot_attempts: 3,
            root_key,
            signature_mandatory: true,
        };
        let mut svc = OtaService::new(ota, policy, "0.0.0", "qemu", 1, 1);

        // 构造 1024B 镜像并签名。
        let image: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        let meta = UpgradeMeta {
            target_version: HString::try_from("0.123.0").unwrap(),
            image_size: image.len() as u64,
            image_hash: hash,
            image_sig: sig.to_bytes(),
            download_url: HString::new(),
            chunk_size: 512,
            total_chunks: 2,
            downloaded_chunks: 0,
        };

        let step = svc
            .download_and_verify(&meta, "qemu", &image)
            .expect("download_and_verify should succeed with valid key/signature");
        assert!(matches!(step, crate::ota_service::OtaStep::Verified));
        let trial = svc
            .mark_trial_boot(eneros_ota_ab::ab_partition::PartitionSlot::B, "0.123.0")
            .expect("trial");
        assert!(matches!(trial, crate::ota_service::OtaStep::TrialStarted));
        let commit = svc.commit().expect("commit");
        assert!(matches!(commit, crate::ota_service::OtaStep::Committed));
    }

    #[test]
    fn ota_ab_command_protocol_roundtrip() {
        use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use crate::ota_service::handle_ota_ab_command;
        use crate::ota_service::{OtaPolicy, OtaService};
        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keygen");
        let pub_bytes = kp.public_key.to_bytes_uncompressed();
        let mut root_key = [0u8; 64];
        root_key.copy_from_slice(&pub_bytes[1..]);

        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let policy = OtaPolicy {
            partition_blocks: PARTITION_BLOCKS,
            chunk_size: 512,
            max_boot_attempts: 3,
            root_key,
            signature_mandatory: true,
        };
        let mut svc = OtaService::new(ota, policy, "0.0.0", "qemu", 1, 1);

        let image: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        let meta = UpgradeMeta {
            target_version: HString::try_from("0.123.0").unwrap(),
            image_size: image.len() as u64,
            image_hash: hash,
            image_sig: sig.to_bytes(),
            download_url: HString::new(),
            chunk_size: 512,
            total_chunks: 2,
            downloaded_chunks: 0,
        };

        // 0x02 DOWNLOAD+VERIFY
        let mut req: Vec<u8> = b"OTA1".to_vec();
        req.push(0x02);
        let mut target = [0u8; 32];
        target[..7].copy_from_slice(b"0.123.0");
        req.extend_from_slice(&target);
        let mut sku = [0u8; 16];
        sku[..4].copy_from_slice(b"qemu");
        req.extend_from_slice(&sku);
        req.extend_from_slice(&1u32.to_le_bytes());
        req.extend_from_slice(&meta.image_size.to_le_bytes());
        req.extend_from_slice(&meta.image_hash);
        req.extend_from_slice(&meta.image_sig);
        req.extend_from_slice(&meta.chunk_size.to_le_bytes());
        req.extend_from_slice(&(image.len() as u32).to_le_bytes());
        req.extend_from_slice(&image);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(resp, [2u8]);

        // 0x03 TRIAL
        let mut req3: Vec<u8> = b"OTA1".to_vec();
        req3.push(0x03);
        req3.push(1u8); // B
        let mut ver = [0u8; 32];
        ver[..7].copy_from_slice(b"0.123.0");
        req3.extend_from_slice(&ver);
        let mut resp3 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req3, &mut resp3));
        assert_eq!(resp3, [3u8]);

        // 0x04 COMMIT
        let req4: Vec<u8> = b"OTA1".to_vec().into_iter().chain([0x04]).collect();
        let mut resp4 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req4, &mut resp4));
        assert_eq!(resp4, [4u8]);
    }

    #[test]
    fn ota_ab_command_negative_cases() {
        use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use crate::ota_service::handle_ota_ab_command;
        use crate::ota_service::{OtaPolicy, OtaService};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keygen");
        let pub_bytes = kp.public_key.to_bytes_uncompressed();
        let mut root_key = [0u8; 64];
        root_key.copy_from_slice(&pub_bytes[1..]);

        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let policy = OtaPolicy {
            partition_blocks: PARTITION_BLOCKS,
            chunk_size: 512,
            max_boot_attempts: 3,
            root_key,
            signature_mandatory: true,
        };
        let mut svc = OtaService::new(ota, policy, "0.0.0", "qemu", 1, 1);

        // 0x01 downgrade/equal -> Reject(0)
        let mut req1: Vec<u8> = b"OTA1".to_vec();
        req1.push(0x01);
        let mut target = [0u8; 32];
        target[..5].copy_from_slice(b"0.0.0");
        req1.extend_from_slice(&target);
        let mut sku = [0u8; 16];
        sku[..4].copy_from_slice(b"qemu");
        req1.extend_from_slice(&sku);
        req1.extend_from_slice(&1u32.to_le_bytes());
        let mut resp1 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req1, &mut resp1));
        assert_eq!(resp1, [0u8]);

        // 0x02 bad signature -> rejected/error
        let image: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        let mut bad_sig = sig.to_bytes();
        bad_sig[0] ^= 0xFF;
        let meta = UpgradeMeta {
            target_version: HString::try_from("0.123.0").unwrap(),
            image_size: image.len() as u64,
            image_hash: hash,
            image_sig: bad_sig,
            download_url: HString::new(),
            chunk_size: 512,
            total_chunks: 1,
            downloaded_chunks: 0,
        };
        let mut req2: Vec<u8> = b"OTA1".to_vec();
        req2.push(0x02);
        let mut target2 = [0u8; 32];
        target2[..7].copy_from_slice(b"0.123.0");
        req2.extend_from_slice(&target2);
        let mut sku2 = [0u8; 16];
        sku2[..4].copy_from_slice(b"qemu");
        req2.extend_from_slice(&sku2);
        req2.extend_from_slice(&1u32.to_le_bytes());
        req2.extend_from_slice(&meta.image_size.to_le_bytes());
        req2.extend_from_slice(&meta.image_hash);
        req2.extend_from_slice(&meta.image_sig);
        req2.extend_from_slice(&meta.chunk_size.to_le_bytes());
        req2.extend_from_slice(&(image.len() as u32).to_le_bytes());
        req2.extend_from_slice(&image);
        let mut resp2 = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req2, &mut resp2));
        assert!(resp2.starts_with(b"ERR:"));

        // 0x09 unknown -> ERR:cmd
        let req9: Vec<u8> = b"OTA1".to_vec().into_iter().chain([0x09]).collect();
        let mut resp9 = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req9, &mut resp9));
        assert!(resp9.starts_with(b"ERR:cmd"));
    }

    #[test]
    fn ota_ab_command_unavailable_without_service() {
        use crate::ota_service::handle_ota_ab_command;
        let mut resp = Vec::new();
        let mut req: Vec<u8> = b"OTA1".to_vec();
        req.push(0x01);
        req.resize(5 + 52, 0u8);
        assert!(!handle_ota_ab_command::<
            eneros_ota_ab::ab_partition::AbPartitionOta<EipcBlockDevice<SimClient>>,
        >(None, &req, &mut resp));
        assert!(resp.starts_with(b"ERR:ota-unavailable"));
    }

    #[test]
    fn ota_ab_command_regression_rolls_back() {
        use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use crate::ota_service::handle_ota_ab_command;
        use crate::ota_service::{OtaPolicy, OtaService};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keygen");
        let pub_bytes = kp.public_key.to_bytes_uncompressed();
        let mut root_key = [0u8; 64];
        root_key.copy_from_slice(&pub_bytes[1..]);

        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let policy = OtaPolicy {
            partition_blocks: PARTITION_BLOCKS,
            chunk_size: 512,
            max_boot_attempts: 1,
            root_key,
            signature_mandatory: true,
        };
        let mut svc = OtaService::new(ota, policy, "0.0.0", "qemu", 1, 1);

        // 0x02 download+verify
        let image: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        let mut req2: Vec<u8> = b"OTA1".to_vec();
        req2.push(0x02);
        let mut target = [0u8; 32];
        target[..7].copy_from_slice(b"0.123.0");
        req2.extend_from_slice(&target);
        let mut sku = [0u8; 16];
        sku[..4].copy_from_slice(b"qemu");
        req2.extend_from_slice(&sku);
        req2.extend_from_slice(&1u32.to_le_bytes());
        req2.extend_from_slice(&(image.len() as u64).to_le_bytes());
        req2.extend_from_slice(&hash);
        req2.extend_from_slice(&sig.to_bytes());
        req2.extend_from_slice(&512u32.to_le_bytes());
        req2.extend_from_slice(&(image.len() as u32).to_le_bytes());
        req2.extend_from_slice(&image);
        let mut resp2 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req2, &mut resp2));
        assert_eq!(resp2, [2u8]);

        // 0x03 trial
        let mut req3: Vec<u8> = b"OTA1".to_vec();
        req3.push(0x03);
        req3.push(1u8);
        let mut ver = [0u8; 32];
        ver[..7].copy_from_slice(b"0.123.0");
        req3.extend_from_slice(&ver);
        let mut resp3 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req3, &mut resp3));
        assert_eq!(resp3, [3u8]);

        // 0x05 regression with max_boot_attempts=1 -> rollback
        let req5: Vec<u8> = b"OTA1".to_vec().into_iter().chain([0x05]).collect();
        let mut resp5 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req5, &mut resp5));
        assert_eq!(resp5, [5u8]);
    }

    #[test]
    fn block_io_oob_reports_write_failed() {
        let storage = SimStorage::new(64, 1);
        let client = SimClient { storage };
        let mut io = BlockIo::new(client, "svc-storage", 1, 1000);
        // 越界写 → 服务回 ST_UNAVAILABLE → 客户端 WriteFailed（不伪造）。
        assert_eq!(io.write(1000, b"oob"), Err(StoreError::WriteFailed));
    }

    use alloc::string::ToString;

    use eneros_durable_state::{Decode, DurableStateStore, Encode, Generation, RegionStateStore};

    /// 测试用状态（仿 TrialState）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct DemoState {
        remaining: u32,
        version: alloc::string::String,
    }

    impl Encode for DemoState {
        fn encode(&self) -> Result<Vec<u8>, StoreError> {
            let mut out = Vec::with_capacity(64);
            out.extend_from_slice(&self.remaining.to_le_bytes());
            let v = self.version.as_bytes();
            out.push(v.len() as u8);
            out.extend_from_slice(v);
            Ok(out)
        }
    }

    impl Decode for DemoState {
        fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
            // 记录 payload：generation(8) ‖ remaining u32 ‖ vlen u8 ‖ version。
            if bytes.len() < 8 + 5 {
                return Err(StoreError::Corrupt);
            }
            let gen = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
            let b = &bytes[8..];
            let remaining = u32::from_le_bytes(b[0..4].try_into().unwrap());
            let vlen = b[4] as usize;
            if 5 + vlen > b.len() {
                return Err(StoreError::Corrupt);
            }
            let version = core::str::from_utf8(&b[5..5 + vlen])
                .map_err(|_| StoreError::Corrupt)?
                .to_string();
            Ok((Generation(gen), DemoState { remaining, version }))
        }
    }

    #[test]
    fn region_store_cas_reload_over_block_io() {
        // 模拟 svc-storage（epoch 7）+ 16MB 数据盘（Rc 共享介质——同一
        // 持久化介质跨"重启"实例）。
        let media = std::rc::Rc::new(core::cell::RefCell::new(vec![0u8; 16 * 1024 * 1024]));
        let storage = SimStorage::with_media(std::rc::Rc::clone(&media), 7);
        let client = SimClient { storage };
        let mut io = BlockIo::new(client, "svc-storage", 0, 1000);
        // 首次 CAS（expected=None → gen 1）。
        {
            let mut store = RegionStateStore::new(&mut io, 8192, 128);
            store.open().unwrap();
            let gen = store
                .compare_and_swap(
                    None,
                    &DemoState {
                        remaining: 2,
                        version: "1.2.0".into(),
                    },
                )
                .unwrap();
            assert_eq!(gen.0, 1);
        }
        // 重启模拟：**新** BlockIo（epoch 吸收重新开始）+ 同一介质（Rc）。
        let storage2 = SimStorage::with_media(std::rc::Rc::clone(&media), 7);
        let client2 = SimClient { storage: storage2 };
        let mut io2 = BlockIo::new(client2, "svc-storage", 0, 1000);
        let mut store2 = RegionStateStore::new(&mut io2, 8192, 128);
        store2.open().unwrap();
        let loaded: eneros_durable_state::Versioned<DemoState> =
            store2.load().unwrap().expect("重启后必须恢复");
        assert_eq!(loaded.generation.0, 1);
        assert_eq!(loaded.state.remaining, 2);
        assert_eq!(loaded.state.version, "1.2.0");
        // CAS gen 2（expected=Some(1)）。
        let gen2 = store2
            .compare_and_swap(
                Some(Generation(1)),
                &DemoState {
                    remaining: 1,
                    version: "1.2.0".into(),
                },
            )
            .unwrap();
        assert_eq!(gen2.0, 2);
    }
}
