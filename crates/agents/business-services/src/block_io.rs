//! BlockIo —— durable-state `PersistIo` 的 EIPC 块客户端实现（蓝图 41
//! WP-41.3 落盘）。**47（WP-47.4）**：适配迁入 `agent-sdk::block_io`
//! （control-services 与 business-services 共享同一实现），本模块
//! re-export 保持既有调用面；**SPEC-B/C（2026-08-28）**：`EipcBlockDevice`
//! 亦迁入 agent-sdk 单件（同构双件 BlockIoFace/LearnBlockIo 已删），
//! OTA 侧测试留在本 crate 但改引 agent-sdk 类型。

#[cfg(all(target_os = "none", feature = "sel4"))]
pub use eneros_agent_sdk::block_io::eipc_block_io;
pub use eneros_agent_sdk::block_io::{BlockIo, EipcBlockDevice, EipcSend};

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_drv_svc::block_cmd::{BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE};
    use eneros_drv_svc::runtime::{wire, DriverResponse, ST_OK};
    // SPEC-B/C：trait 方法（PersistIo/BlockDevice）需显式导入——原文件顶部
    // 导入随 EipcBlockDevice 迁出而移除。
    use eneros_durable_state::PersistIo;
    use eneros_durable_state::StoreError;
    use eneros_storage::driver::BlockDevice;

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
        assert_eq!(io.epoch(), 7, "首个响应吸收服务 epoch");
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
        assert_eq!(io3.epoch(), 5);
        // 模拟重启：底层响应 epoch 变为 11（新 SimStorage），客户端 epoch
        // 仍 5 → 拒绝。
        let storage4 = SimStorage::new(4096, 11);
        let client4 = SimClient { storage: storage4 };
        let mut io4 = BlockIo::new(client4, "svc-storage", 5, 1000);
        assert_eq!(io4.write(0, b"w"), Err(StoreError::Unavailable));
    }

    /// 70.5 v2 API 测试夹具：确定性密钥 + 真实 AbPartitionOta（SimStorage
    /// 块命令面）+ 已签描述构造。
    mod ota_v2 {
        use alloc::vec::Vec;

        use eneros_agent_sdk::ota_wire::{
            encode_download_v2, UpdateDescription, DESC_USAGE_OTA_AB_IMAGE,
        };
        use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair, Sm2PrivateKey};

        use crate::ota_service::{DeviceIdentity, OtaPolicy};

        pub(super) fn keypair(seed: u8) -> (Sm2PrivateKey, [u8; 64], Sm2KeyPair) {
            let sk = Sm2PrivateKey::from_bytes(&[seed; 32]).expect("测试私钥");
            let kp = Sm2KeyPair::from_private_key(&sk).expect("测试公钥");
            let mut root_key = [0u8; 64];
            root_key.copy_from_slice(&kp.public_key.to_bytes_uncompressed()[1..65]);
            (sk, root_key, kp)
        }

        pub(super) fn policy(root_key: [u8; 64]) -> OtaPolicy {
            OtaPolicy {
                partition_blocks: 2048,
                chunk_size: 512,
                max_boot_attempts: 3,
                root_key,
                signature_mandatory: true,
                generation: 1,
            }
        }

        pub(super) fn identity() -> DeviceIdentity {
            DeviceIdentity {
                current_version: alloc::string::String::from("0.122.0"),
                sku: alloc::string::String::from("qemu"),
                security_version: 1,
                security_floor: 1,
            }
        }

        pub(super) fn sign_digest(sk: &Sm2PrivateKey, digest: &[u8; 32]) -> [u8; 64] {
            let kp = Sm2KeyPair::from_private_key(sk).expect("kp");
            let sig = sm2_sign(
                digest,
                sk,
                &kp.public_key,
                &mut CsRng::from_seed(&[0x77; 32]),
            )
            .expect("sign");
            let mut out = [0u8; 64];
            out.copy_from_slice(&sig.to_bytes()[..64]);
            out
        }

        /// 已签描述 + 镜像签章 + 镜像 → 0x02 v2 请求。
        pub(super) fn req02(sk: &Sm2PrivateKey, target: &str, image: &[u8]) -> Vec<u8> {
            let hash = sm3_hash(image);
            let desc = UpdateDescription {
                usage: DESC_USAGE_OTA_AB_IMAGE,
                sku: alloc::string::String::from("qemu"),
                target_version: alloc::string::String::from(target),
                baseline_version: alloc::string::String::from("0.122.0"),
                security_version: 2,
                image_len: image.len() as u64,
                image_hash: hash,
                policy_generation: 1,
            };
            let desc_sig = {
                let bytes = desc.encode().expect("desc encode");
                sign_digest(sk, &sm3_hash(&bytes))
            };
            let image_sig = sign_digest(sk, &hash);
            encode_download_v2(&desc, &desc_sig, &image_sig, 512, image)
        }
    }

    #[test]
    fn eipc_block_device_full_ota_download_verify_commit() {
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use self::ota_v2::{identity, keypair, policy, req02};
        use crate::ota_service::{OtaService, OtaStep};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS + 1;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let (sk, root_key, kp) = keypair(0x5A);
        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let mut svc = OtaService::new(ota, policy(root_key), &identity());

        // 1024B 镜像，描述/镜像双签（真实 SM2/SM3 回读验证）。
        let image: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let req = req02(&sk, "0.123.0", &image);
        let mut resp = Vec::new();
        assert!(crate::ota_service::handle_ota_ab_command(Some(&mut svc), &req, &mut resp).handled);
        assert_eq!(resp, vec![2u8]); // Verified
        let trial = svc
            .mark_trial_boot(eneros_ota_ab::ab_partition::PartitionSlot::B, "0.123.0")
            .expect("trial");
        assert!(matches!(trial, OtaStep::TrialStarted));
        let commit = svc.commit().expect("commit");
        assert!(matches!(commit, OtaStep::Committed));
    }

    /// 非对齐镜像（长度非 chunk 整数倍）全链下载：尾部块必须照常落盘并
    /// Verified——total_chunks 口径 = div_ceil（与 kernel_ota/rollback 同源），
    /// 地板除会让 AbPartitionOta 写状态机提前置 Written，尾块 WriteFailed。
    #[test]
    fn non_aligned_image_download_survives_tail_chunk() {
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use self::ota_v2::{identity, keypair, policy, req02};
        use crate::ota_service::{handle_ota_ab_command, OtaService};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS + 1;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let (sk, root_key, kp) = keypair(0x5A);
        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let mut svc = OtaService::new(ota, policy(root_key), &identity());

        // 1224B = 512+512+200（3 块；地板除 = 2 → 尾块必失败的对齐盲区）。
        let image: Vec<u8> = (0..1224u32).map(|i| (i % 251) as u8).collect();
        let req = req02(&sk, "0.123.0", &image);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req, &mut resp).handled);
        assert_eq!(resp, vec![2u8]); // Verified
    }

    #[test]
    fn ota_ab_command_protocol_roundtrip() {
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use self::ota_v2::{identity, keypair, policy, req02};
        use crate::ota_service::{handle_ota_ab_command, OtaService};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS + 1;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let (sk, root_key, kp) = keypair(0x5A);
        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let mut svc = OtaService::new(ota, policy(root_key), &identity());

        // 0x02 DOWNLOAD-DESC（v2 wire 全链）。
        let image: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let req = req02(&sk, "0.123.0", &image);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req, &mut resp).handled);
        assert_eq!(resp, [2u8]);

        // 0x03 TRIAL
        let mut req3: Vec<u8> = b"OTA1".to_vec();
        req3.push(0x03);
        req3.push(1u8); // B
        let mut ver = [0u8; 32];
        ver[..7].copy_from_slice(b"0.123.0");
        req3.extend_from_slice(&ver);
        let mut resp3 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req3, &mut resp3).handled);
        assert_eq!(resp3, [3u8]);

        // 0x04 COMMIT
        let req4: Vec<u8> = b"OTA1".to_vec().into_iter().chain([0x04]).collect();
        let mut resp4 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req4, &mut resp4).handled);
        assert_eq!(resp4, [4u8]);
    }

    #[test]
    fn ota_ab_command_negative_cases() {
        use eneros_agent_sdk::ota_wire::{
            encode_check_desc, UpdateDescription, DESC_USAGE_OTA_AB_IMAGE,
        };
        use eneros_crypto::sm3_hash;
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use self::ota_v2::{identity, keypair, policy};
        use crate::ota_service::{handle_ota_ab_command, OtaService};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS + 1;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let (sk, root_key, kp) = keypair(0x5A);
        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let mut svc = OtaService::new(ota, policy(root_key), &identity());

        // 0x01 CHECK-DESC 等版本 → verdict 0（判据 v2：签名描述形）。
        let desc = UpdateDescription {
            usage: DESC_USAGE_OTA_AB_IMAGE,
            sku: alloc::string::String::from("qemu"),
            target_version: alloc::string::String::from("0.122.0"),
            baseline_version: alloc::string::String::from("0.122.0"),
            security_version: 2,
            image_len: 16,
            image_hash: [7; 32],
            policy_generation: 1,
        };
        let bytes = desc.encode().expect("encode");
        let desc_sig = ota_v2::sign_digest(&sk, &sm3_hash(&bytes));
        let req1 = encode_check_desc(&desc, &desc_sig);
        let mut resp1 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req1, &mut resp1).handled);
        assert_eq!(resp1, [0u8]);

        // 0x02 镜像签章损坏 → verify-failed（回读 SM2 验签拒绝，不激活）。
        let image: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let mut req2 = ota_v2::req02(&sk, "0.123.0", &image);
        // image_sig 段 = 5+158+64 .. +64：翻转一位。
        let isig_off = 5 + 158 + 64;
        req2[isig_off] ^= 0xFF;
        let mut resp2 = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req2, &mut resp2).handled);
        assert!(resp2.starts_with(b"ERR:"));

        // 0x09 unknown -> ERR:cmd
        let req9: Vec<u8> = b"OTA1".to_vec().into_iter().chain([0x09]).collect();
        let mut resp9 = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req9, &mut resp9).handled);
        assert!(resp9.starts_with(b"ERR:cmd"));
    }

    #[test]
    fn ota_ab_command_unavailable_without_service() {
        use crate::ota_service::handle_ota_ab_command;
        let mut resp = Vec::new();
        let mut req: Vec<u8> = b"OTA1".to_vec();
        req.push(0x01);
        req.resize(5 + 158 + 64, 0u8);
        assert!(
            !handle_ota_ab_command::<
                eneros_ota_ab::ab_partition::AbPartitionOta<EipcBlockDevice<SimClient>>,
            >(None, &req, &mut resp)
            .handled
        );
        assert!(resp.starts_with(b"ERR:ota-unavailable"));
    }

    #[test]
    fn ota_ab_command_regression_rolls_back() {
        use eneros_ota_ab::ab_partition::{AbPartitionOta, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

        use self::ota_v2::{identity, keypair, policy, req02};
        use crate::ota_service::{handle_ota_ab_command, OtaService};

        let total_blocks = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS + 1;
        let storage = SimStorage::new((total_blocks * 512) as usize, 7);
        let client = SimClient { storage };
        let io = BlockIo::new(client, "svc-storage", 7, 1000);
        let dev = EipcBlockDevice::new(io, 512, total_blocks);

        let (sk, root_key, kp) = keypair(0x5A);
        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let mut pol = policy(root_key);
        pol.max_boot_attempts = 1;
        let mut svc = OtaService::new(ota, pol, &identity());

        // 0x02 download+verify
        let image: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let req2 = req02(&sk, "0.123.0", &image);
        let mut resp2 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req2, &mut resp2).handled);
        assert_eq!(resp2, [2u8]);

        // 0x03 trial
        let mut req3: Vec<u8> = b"OTA1".to_vec();
        req3.push(0x03);
        req3.push(1u8);
        let mut ver = [0u8; 32];
        ver[..7].copy_from_slice(b"0.123.0");
        req3.extend_from_slice(&ver);
        let mut resp3 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req3, &mut resp3).handled);
        assert_eq!(resp3, [3u8]);

        // 0x05 regression with max_boot_attempts=1 -> rollback
        let req5: Vec<u8> = b"OTA1".to_vec().into_iter().chain([0x05]).collect();
        let mut resp5 = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req5, &mut resp5).handled);
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
