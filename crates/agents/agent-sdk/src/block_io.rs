//! 47（WP-47.4）：`BlockIo` —— durable-state `PersistIo` 的 EIPC 块客户端
//! 实现（蓝图 41 WP-41.3 落盘先例；现为 control-services 与
//! business-services 共享适配）。
//!
//! 协议：drv-svc 信封（`DriverRequest/DriverResponse`；68.3 起 PAYLOAD_
//! CAP=461 顶格 MR 120 words×4B=480B 物理上限——执行卡 WP-68.3.0 丈量）
//! 经 `Sel4IpcClient::drv_call` 同步调用（4 字节/word 打包，与
//! svc-storage `driver_service_main` 的 MR 语义一致）。命令子码 =
//! `block_cmd` 的 BLK_CMD_*（READ/WRITE/FLUSH/HEALTH）。
//!
//! fail-closed：存储能力未授予（EIPC mask 对应位为 0）→ `Unavailable`；
//! 信封响应 status != ST_OK → 对应 `StoreError`（不伪造成功）。

/// 70.7（WP-70.7.2）：BLK_BOOT_* 安装面子码 re-export（svc-ota 安装
/// 控制器消费；常量单源 = drv-svc::block_cmd）。
pub use eneros_drv_svc::block_cmd::{
    BLK_BOOT_CANCEL, BLK_BOOT_CONFIRM, BLK_BOOT_FLUSH, BLK_BOOT_GUARD,
    BLK_BOOT_INSTALL_WRITE, BLK_BOOT_READ, BLK_BOOT_STATUS, BLK_BOOT_TRIAL,
};
use eneros_drv_svc::block_cmd::{
    BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE, BLK_READ_MAX, BLK_WRITE_MAX,
};
use eneros_drv_svc::runtime::{wire, DriverRequest, DriverResponse, OP_EXECUTE, ST_OK};
use eneros_durable_state::{PersistIo, StoreError};
// SPEC-B/C（2026-08-28）：EipcBlockDevice（BlockIo→storage BlockDevice
// 单件）迁入本模块——storage 依赖方向已验无环。
use eneros_storage::driver::{BlockDevice, DeviceHealth};
use eneros_storage::StorageError;

/// 读分块（68.3 WP-68.3.1：单源 blk_cmd 服务端上限——响应载荷容量）。
pub const BLK_READ_CHUNK: usize = BLK_READ_MAX;
/// 写分块（payload = 子码 1B + lba 8B + data；单源 blk_cmd 服务端上限）。
pub const BLK_WRITE_CHUNK: usize = BLK_WRITE_MAX;

const _: () = assert!(BLK_WRITE_CHUNK + 9 <= eneros_drv_svc::runtime::PAYLOAD_CAP);
const _: () = assert!(BLK_READ_CHUNK <= eneros_drv_svc::runtime::PAYLOAD_CAP);

/// BOOT 安装事务请求体构造（70.7；**body only**——子码由 [`BlockIo::cmd`]
/// 前置，调用方永不再带子码字节）。
///
/// REG-70.7-OTA1-DOUBLE-SUB（run56 实弹）：旧 svc-ota 臂把子码一并放进载荷
/// → 服务端解析全体右移 1B（读面 lba 串位 → `[blk] read OutOfRange`；
/// trial/confirm 的 secver/image_id 变噪声）。三构造器与 svc-storage
/// `handle_boot_cmd` 解析布局逐字段对齐，长度即判据（多带子码 = 长 1B，
/// 宿主测试锁死）。
pub mod boot_body {
    use alloc::vec::Vec;

    /// `BLK_BOOT_TRIAL` body = secver(8) + image_id(16)（共 24B）。
    pub fn trial(secver: u64, image_id: &[u8; 16]) -> Vec<u8> {
        let mut p = Vec::with_capacity(24);
        p.extend_from_slice(&secver.to_le_bytes());
        p.extend_from_slice(image_id);
        p
    }

    /// `BLK_BOOT_CONFIRM` body = slot_is_a(1) + secver(8) + image_id(16)（25B）。
    pub fn confirm(slot_is_a: bool, secver: u64, image_id: &[u8; 16]) -> Vec<u8> {
        let mut p = Vec::with_capacity(25);
        p.push(u8::from(slot_is_a));
        p.extend_from_slice(&secver.to_le_bytes());
        p.extend_from_slice(image_id);
        p
    }

    /// `BLK_BOOT_READ` body = lba(8) + count_bytes(4)（12B）。
    pub fn read(lba: u64, count_bytes: u32) -> Vec<u8> {
        let mut p = Vec::with_capacity(12);
        p.extend_from_slice(&lba.to_le_bytes());
        p.extend_from_slice(&count_bytes.to_le_bytes());
        p
    }
}

/// 块客户端：经 EIPC 调 svc-storage（X3 授予表槽位）。
#[derive(Clone)]
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

    /// 当前服务 epoch（首个响应吸收后；观测/测试）。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// EIPC 发送 seam（host 可测；target 侧经 `Sel4IpcClient::drv_call`——
/// 4 字节/word 打包，与 svc-storage `driver_service_main` 的 MR 语义
/// 一致；`request_raw` 是 8 字节/word，混用错位截断）。
#[allow(clippy::result_unit_err)] // 44.2：EIPC seam 错误面为 ()（drv_call 语义），错误类型化属后续卷
pub trait EipcSend {
    fn drv_call(&mut self, req: &[u8], timeout_ns: u64) -> Result<alloc::vec::Vec<u8>, ()>;
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl EipcSend for crate::ipc_sel4::Sel4IpcClient {
    fn drv_call(&mut self, req: &[u8], timeout_ns: u64) -> Result<alloc::vec::Vec<u8>, ()> {
        crate::ipc_sel4::Sel4IpcClient::drv_call(self, req, timeout_ns).map_err(|_| ())
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
        // drv-svc 服务端按 4B/word 回 MR：响应字节数非 4 倍数时会以
        // 整 word 数返回（尾部 padding）——只解前 RESPONSE_LEN 字节。
        // 容差解码单一事实源 = wire::decode_response_padded（SPEC-B
        // WP-B.2 收编）。
        let resp =
            wire::decode_response_padded(&resp_bytes).ok_or(StoreError::Encode("bad-envelope"))?;
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

    /// 读 `offset` 起 `buf.len()` 字节（分块 = [`BLK_READ_CHUNK`]；lba =
    /// 字节偏移，与 39 命令面 RamBackend 语义一致）。
    /// 70.7（WP-70.7.2）：原始子码命令（BLK_BOOT_* 安装面；svc-ota
    /// 安装控制器消费）。payload = 子码后的请求体；返回 ST_OK 时的
    /// 响应数据段。非 ST_OK → 显式错误（fail-closed 不伪装）。
    pub fn raw_command(
        &mut self,
        sub: u8,
        payload: &[u8],
    ) -> Result<alloc::vec::Vec<u8>, StoreError> {
        let resp = self.cmd(sub, payload)?;
        if resp.status != 0 {
            return Err(StoreError::Other("boot-cmd-status"));
        }
        Ok(resp.payload[..resp.payload_len as usize].to_vec())
    }

    fn read_into(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        let mut off = offset;
        let mut out = buf;
        while !out.is_empty() {
            let n = core::cmp::min(out.len(), BLK_READ_CHUNK);
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

    /// 写 `data` 到 `offset` 起（分块 = [`BLK_WRITE_CHUNK`]）。
    fn write_from(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        let mut off = offset;
        let mut src = data;
        while !src.is_empty() {
            let n = core::cmp::min(src.len(), BLK_WRITE_CHUNK);
            let mut p = [0u8; 8 + BLK_WRITE_CHUNK];
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

/// 构造 EIPC block 客户端（target 侧 sel4；`slot` = X3 授予表槽位）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub fn eipc_block_io(
    slot: u64,
    epoch: u64,
    timeout_ms: u64,
) -> Option<BlockIo<crate::ipc_sel4::Sel4IpcClient>> {
    let ipc = crate::ipc_sel4::Sel4IpcClient::new("block-io", slot, eneros_drv_svc::now_ns).ok()?;
    Some(BlockIo::new(ipc, "svc-storage", epoch, timeout_ms))
}

/// 将 [`BlockIo`]（EIPC 块客户端）适配为 storage crate 的 [`BlockDevice`]，
/// 供 `AbPartitionOta` / 磁盘 TSDB 等真实持久化组件做块级读写。
///
/// SPEC-B/C（2026-08-28）：自 business-services 迁入，全仓单件——历史
/// 同构双件（telemetry_history `BlockIoFace`、ai-worker `LearnBlockIo`）
/// 删除；"erase = 零写"语义的唯一契约测试在本模块（擦后读零）。
#[derive(Clone)]
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
        // 块命令面无独立擦除——零写语义（唯一契约测试：擦后读零）。
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

    use eneros_drv_svc::block_cmd::{BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE};
    use eneros_drv_svc::runtime::{wire, DriverResponse, ST_OK};

    use super::*;

    /// REG-70.7-OTA1-DOUBLE-SUB 回归锁：BOOT 安装请求体 = **body only**
    /// （子码由 [`BlockIo::cmd`] 前置）。旧 svc-ota 臂把子码也放进 body →
    /// 服务端解析全体右移 1B（读面 lba 串位实弹 `[blk] read OutOfRange`；
    /// trial/confirm 的 secver/image_id 变噪声）。长度 + 逐字段位置 +
    /// 组帧后端到端解析三重锁。
    #[test]
    fn boot_install_bodies_are_body_only() {
        use eneros_drv_svc::block_cmd::{BLK_BOOT_CONFIRM, BLK_BOOT_READ, BLK_BOOT_TRIAL};
        use eneros_drv_svc::runtime::DriverRequest;

        let id = [0x5Au8; 16];
        // body 形态。
        let trial = boot_body::trial(2, &id);
        assert_eq!(trial.len(), 24, "多带子码即 25B = 解析位移");
        assert_eq!(&trial[..8], &2u64.to_le_bytes());
        assert_eq!(&trial[8..], &id);

        let confirm = boot_body::confirm(false, 7, &id);
        assert_eq!(confirm.len(), 25);
        assert_eq!(confirm[0], 0);
        assert_eq!(&confirm[1..9], &7u64.to_le_bytes());
        assert_eq!(&confirm[9..], &id);

        let read = boot_body::read(36_864, BLK_READ_CHUNK as u32);
        assert_eq!(read.len(), 12);
        assert_eq!(&read[..8], &36_864u64.to_le_bytes());

        // 组帧 → 服务端判据/解析（`cmd` 置 payload[0]=子码；body 紧随）。
        let framed = |sub: u8, body: &[u8]| {
            let mut req = DriverRequest::new(0xB10C, 0x10);
            req.payload[0] = sub;
            req.payload[1..1 + body.len()].copy_from_slice(body);
            req.payload_len = 1 + body.len() as u16;
            req
        };
        let t = framed(BLK_BOOT_TRIAL, &trial);
        assert!(t.payload_len >= 25, "svc-storage BLK_BOOT_TRIAL 判据");
        assert_eq!(
            u64::from_le_bytes(t.payload[1..9].try_into().unwrap()),
            2,
            "trial secver 逐位对齐（位移即红）"
        );
        assert_eq!(&t.payload[9..25], &id);

        let c = framed(BLK_BOOT_CONFIRM, &confirm);
        assert!(c.payload_len >= 26, "svc-storage BLK_BOOT_CONFIRM 判据");
        assert_eq!(c.payload[1], 0);
        assert_eq!(u64::from_le_bytes(c.payload[2..10].try_into().unwrap()), 7);
        assert_eq!(&c.payload[10..26], &id);

        let r = framed(BLK_BOOT_READ, &read);
        assert!(r.payload_len >= 13, "svc-storage BLK_BOOT_READ 判据");
        assert_eq!(
            u64::from_le_bytes(r.payload[1..9].try_into().unwrap()),
            36_864
        );
        assert_eq!(
            u32::from_le_bytes(r.payload[9..13].try_into().unwrap()) as usize,
            BLK_READ_CHUNK
        );
    }

    /// 内存仿真后端（host；模拟 svc-storage 块命令面）。
    #[derive(Clone)]
    struct SimStorage {
        data: std::rc::Rc<core::cell::RefCell<Vec<u8>>>,
    }

    impl SimStorage {
        fn handle(&self, req: &[u8]) -> Vec<u8> {
            let dreq = wire::decode_request(req).unwrap();
            let mut resp = DriverResponse::new(dreq.correlation, ST_OK, 7);
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

    impl EipcSend for SimStorage {
        fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
            Ok(self.handle(req))
        }
    }

    /// SPEC-B/C：EipcBlockDevice 单件契约（7 方法 + 擦后读零 + 越界拒绝）
    /// ——原 business-services 测试迁移，erase 零写语义唯一契约处。
    #[test]
    fn eipc_block_device_contract() {
        let media = std::rc::Rc::new(core::cell::RefCell::new(vec![0u8; 4096]));
        let storage = SimStorage {
            data: std::rc::Rc::clone(&media),
        };
        let io = BlockIo::new(storage, "svc-storage", 7, 1000);
        let mut dev = EipcBlockDevice::new(io, 512, 8);
        // 写读往返。
        let data = [0xAAu8; 512];
        dev.write_block(2, &data).unwrap();
        let mut buf = [0u8; 512];
        dev.read_block(2, &mut buf).unwrap();
        assert_eq!(buf, data);
        assert_eq!(dev.block_count(), 8);
        assert_eq!(dev.block_size(), 512);
        dev.flush().unwrap();
        // 擦后读零（零写语义契约）。
        dev.erase_block(3).unwrap();
        let mut erased = [0xFFu8; 512];
        dev.read_block(3, &mut erased).unwrap();
        assert_eq!(erased, [0u8; 512]);
        // 越界块拒绝。
        assert!(dev.read_block(8, &mut buf).is_err());
        assert!(dev.write_block(8, &data).is_err());
        // Clone（mount-or-format 双实例路径）。
        let dev2 = dev.clone();
        let mut b2 = [0u8; 512];
        dev2.read_block(2, &mut b2).unwrap();
        assert_eq!(b2, data);
    }

    /// 68.3 WP-68.3.1：扩容后大块面边界——奇数长度 / 跨块（多命令拼装）/
    /// 断在中途（介质边界截断 → 错误如实上抛，不伪成功）。
    #[test]
    fn enlarged_chunks_odd_cross_block_and_midfail() {
        // 4 KiB 介质：跨块写读 + 奇数长度。
        let media = std::rc::Rc::new(core::cell::RefCell::new(vec![0u8; 4096]));
        let storage = SimStorage {
            data: std::rc::Rc::clone(&media),
        };
        let mut dev = EipcBlockDevice::new(BlockIo::new(storage, "svc-storage", 0, 1000), 512, 8);

        // 跨块大块写（821B > 旧 52B 单命令容量，触发多 chunk 拼装）。
        let big: alloc::vec::Vec<u8> = (0..821u32).map(|i| (i % 251) as u8).collect();
        dev.write_block(1, &big).unwrap();
        let mut back = alloc::vec![0u8; 821];
        dev.read_block(1, &mut back).unwrap();
        assert_eq!(back, big, "跨块 roundtrip 逐字节一致");

        // 奇数长度（512 块内的 137B）。
        let odd = [0x5Au8; 137];
        dev.write_block(3, &odd).unwrap();
        let mut ob = [0u8; 137];
        dev.read_block(3, &mut ob).unwrap();
        assert_eq!(ob, odd);

        // 断在中途：写越过介质尾（4096B 介质 + 512 越界）→ 第 10 个
        // chunk（4068..4520）越界失败必须上抛（不伪成功）。
        let media2 = std::rc::Rc::new(core::cell::RefCell::new(vec![0u8; 4096]));
        let storage2 = SimStorage {
            data: std::rc::Rc::clone(&media2),
        };
        let mut io2 = BlockIo::new(storage2, "svc-storage", 0, 1000);
        let overflow: alloc::vec::Vec<u8> = (0..4608u32).map(|i| (i % 251) as u8).collect();
        let r = io2.write(0, &overflow);
        assert!(r.is_err(), "越界写必须失败（不伪成功）");
        // 错误词形 = 首个失败命令的映射（越界 → 服务端 ST_UNAVAILABLE →
        // WriteFailed）。
        assert!(matches!(r.unwrap_err(), StoreError::WriteFailed));
        // 已写前缀保留（fail-visible 不伪造；块面无事务回滚语义——调用方
        // 重试即重写同区域）。
        let d = media2.borrow();
        assert_eq!(d[0], 0);
        assert_eq!(
            d[1000],
            (1000 % 251) as u8,
            "已成功 chunk 的前缀必须真实落盘"
        );
        assert_eq!(d[4067], (4067 % 251) as u8, "第 10 chunk 起点前一字节已写");
    }
}
