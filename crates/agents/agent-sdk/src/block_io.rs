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

use eneros_drv_svc::blk_wire;
/// 70.7（WP-70.7.2）：BLK_BOOT_* 安装面子码 re-export（svc-ota 安装
/// 控制器消费；常量单源 = drv-svc::block_cmd）。
pub use eneros_drv_svc::block_cmd::{
    BLK_BOOT_CANCEL, BLK_BOOT_CONFIRM, BLK_BOOT_FLUSH, BLK_BOOT_GUARD, BLK_BOOT_INSTALL_WRITE,
    BLK_BOOT_READ, BLK_BOOT_STATUS, BLK_BOOT_TRIAL,
};
use eneros_drv_svc::block_cmd::{
    BLK_CMD_FLUSH, BLK_CMD_HEALTH, BLK_CMD_READ, BLK_CMD_WRITE, BLK_READ_MAX, BLK_WRITE_MAX,
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

    use eneros_drv_svc::blk_wire;

    /// `BLK_BOOT_TRIAL` body = secver(8) + image_id(16)（共 24B）。
    pub fn trial(secver: u64, image_id: &[u8; 16]) -> Vec<u8> {
        blk_wire::pack_boot_trial(secver, *image_id).to_vec()
    }

    /// `BLK_BOOT_CONFIRM` body = slot_is_a(1) + secver(8) + image_id(16)（25B）。
    pub fn confirm(slot_is_a: bool, secver: u64, image_id: &[u8; 16]) -> Vec<u8> {
        blk_wire::pack_boot_confirm(slot_is_a, secver, *image_id).to_vec()
    }

    /// `BLK_BOOT_READ` body = lba(8) + count_bytes(4)（12B）。
    pub fn read(lba: u64, count_bytes: u32) -> Vec<u8> {
        blk_wire::pack_boot_read(lba, count_bytes).to_vec()
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
            let resp = self.cmd(BLK_CMD_READ, &blk_wire::pack_read(off, n as u32))?;
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
            p[..8].copy_from_slice(&blk_wire::pack_write_prefix(off));
            p[8..8 + n].copy_from_slice(&src[..n]);
            let _ = self.cmd(BLK_CMD_WRITE, &p[..8 + n])?;
            src = &src[n..];
            off += n as u64;
        }
        Ok(())
    }

    /// R16-L（蓝图 75-03 WP-75.3.2）：BLK_CMD_HEALTH attach 期探测。
    /// 应答 13B：blocks(8) + block_size(4) + 折码(1)——wire 形状冻结，
    /// 不传 bad_blocks/wear/life 连续量（若未来要连续量须扩 wire 另立卷）。
    /// 任何失败（transport/非 ST_OK/短载荷）→ `None`：观测面零裁决消费
    /// 方，探测失败不得拖垮构造序列。
    pub fn probe_health(&mut self) -> Option<DeviceHealth> {
        let resp = self.cmd(BLK_CMD_HEALTH, &[]).ok()?;
        // 13B 定形之外一律 None（观测面零裁决）；block_size 是服务端回显，
        // 与本地已知值冗余，不入解包结论。
        let (total_blocks, _block_size, code) =
            blk_wire::parse_health(&resp.payload[..resp.payload_len as usize]).ok()?;
        Some(unfold_health_code(code, total_blocks))
    }
}

/// R16-L：health 折码反卷——服务端 [`crate::block_io`] 对端折码
///（blk_backend `fold_health_code`：2=坏块/4=磨损/3=寿命/0=健康）映射回
/// [`DeviceHealth`] 的**语义化近似**：`bad_blocks = (code==2) as u64`、
/// `remaining_life = if code==3 {0} else {100}`、`wear_level = if code==4
/// {100} else {0}`。诚实取舍：wire 13B 冻结只传折码，消费侧表达
/// 「健康/劣化」二值语义而非连续量；temperature 恒 0（wire 无域）。
///
/// 单位口径（75 全局复查轮注记）：`total_blocks` 直传 wire 应答首八字节
/// = 驱动侧 `block_count()`（512B 扇区口径），与 `EipcBlockDevice::
/// block_count` 字段（构造传入的本地几何块口径，如 4K 区段计数）**不同
/// 单位**——两读取端勿直接比较/混算（当前零裁决消费方）。
pub fn unfold_health_code(code: u8, total_blocks: u64) -> DeviceHealth {
    DeviceHealth {
        total_blocks,
        bad_blocks: u64::from(code == 2),
        wear_level: if code == 4 { 100 } else { 0 },
        temperature: 0,
        remaining_life: if code == 3 { 0 } else { 100 },
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
    /// R16-L（WP-75.3.2）：attach 期 health 快照。`health_status(&self)`
    /// 不可发起 wire 调用（拨号需可变通道）⇒ attach 期快照是唯一无
    /// trait 签名变化的形态（构造序列一次 BLK_CMD_HEALTH 往返）。
    health: DeviceHealth,
}

/// 未探测时的缺省健康投影（历史「恒完美」行为面——mock/测试装配与
/// 探测失败兜底共用；观测面零裁决消费方，语义保持）。
fn default_health(block_count: u64) -> DeviceHealth {
    DeviceHealth {
        total_blocks: block_count,
        bad_blocks: 0,
        wear_level: 0,
        temperature: 0,
        remaining_life: 100,
    }
}

impl<C: EipcSend> EipcBlockDevice<C> {
    pub fn new(io: BlockIo<C>, block_size: usize, block_count: u64) -> Self {
        Self {
            io: core::cell::RefCell::new(io),
            block_size,
            block_count,
            health: default_health(block_count),
        }
    }

    /// R16-L（WP-75.3.2）：attach 期探针构造——与 [`Self::new`] 同参，
    /// 构造序列内先发一次 BLK_CMD_HEALTH（+1 EIPC 往返，open 批内微秒
    /// 级）并把 13B 应答折码快照进缓存；探测失败 → 缺省完美兜底
    ///（观测面零裁决消费方，构造不因观测失败而失败）。生产装配面
    /// （event_face/fact_face/telemetry_history/svc-ota）一律走本构造。
    pub fn new_probed(mut io: BlockIo<C>, block_size: usize, block_count: u64) -> Self {
        let health = io
            .probe_health()
            .unwrap_or_else(|| default_health(block_count));
        Self {
            io: core::cell::RefCell::new(io),
            block_size,
            block_count,
            health,
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
        // R16-L：attach 期快照回读（原恒伪造完美值且不发起 wire 调用——
        // 通道两端死码；生产装配面经 new_probed 通电）。
        self.health
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_drv_svc::blk_wire;
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
                    // WP-76.2.4 弱门修复：本替身收编前**既无长度门也无 count
                    // 域界**——短载荷直切 `payload[9..13]` 是 panic 面，超上限
                    // count 在介质边界内还会越 `resp.payload`（461B）写界。
                    // 门与生产同源（blk_wire 布局 + BLK_READ_MAX）；拒绝形态
                    // 保持本替身惯例 = status 语义而非 transport Err（其测试族
                    // 既有契约，卷二 §10 登记不修统一）。
                    let gate = blk_wire::parse_read(blk_wire::body_of(&dreq))
                        .ok()
                        .filter(|(_, n)| *n > 0 && (*n as usize) <= blk_wire::BLK_READ_MAX);
                    let Some((lba, count)) = gate else {
                        resp.status = 5; // ST_INVALID
                        return wire::encode_response(&resp).to_vec();
                    };
                    let count = count as usize;
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
                    let Ok((lba, payload)) = blk_wire::parse_write(blk_wire::body_of(&dreq)) else {
                        resp.status = 5;
                        return wire::encode_response(&resp).to_vec();
                    };
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

    /// R16-L（蓝图 75-03 WP-75.3.2）专用 mock：BLK_CMD_HEALTH 13B 折码
    /// 应答（blocks(8)+block_size(4)+health 码(1)，wire 形状冻结）。
    struct HealthMock {
        blocks: u64,
        bs: u32,
        code: u8,
        fail: bool,
    }

    impl EipcSend for HealthMock {
        fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
            if self.fail {
                return Err(());
            }
            let dreq = wire::decode_request(req).unwrap();
            assert_eq!(
                dreq.payload[0], BLK_CMD_HEALTH,
                "attach 期探针只发 HEALTH 子命令"
            );
            let mut resp = DriverResponse::new(dreq.correlation, ST_OK, 7);
            resp.payload[..13].copy_from_slice(&blk_wire::pack_health(
                self.blocks,
                self.bs,
                self.code,
            ));
            resp.payload_len = 13;
            Ok(wire::encode_response(&resp).to_vec())
        }
    }

    /// R16-L：attach 期 health 快照——`new_probed` 构造期一次
    /// BLK_CMD_HEALTH 往返，`health_status()` 读缓存（现状红：恒伪造
    /// 完美值且不发起 wire 调用）。折码反卷语义化近似（wire 13B 冻结，
    /// 不传连续量）：2→bad_blocks=1；3→remaining_life=0；4→wear=100；
    /// 0→完美。探测失败 → 本地缺省完美兜底（观测面零裁决消费方，
    /// 构造不得因观测失败而失败）。
    #[test]
    fn health_probe_snapshot_at_attach() {
        let mk = |code: u8, fail: bool| {
            BlockIo::new(
                HealthMock {
                    blocks: 4096,
                    bs: 512,
                    code,
                    fail,
                },
                "svc-storage",
                0,
                1000,
            )
        };
        // 折码 2 → bad_blocks 语义化近似。
        let dev = EipcBlockDevice::new_probed(mk(2, false), 512, 8);
        let h = dev.health_status();
        assert_eq!(h.total_blocks, 4096, "total_blocks 从 wire 应答取");
        assert_eq!(h.bad_blocks, 1, "折码 2 → bad_blocks=1");
        assert_eq!(h.remaining_life, 100, "折码 2 不动 life 位");
        // 折码 3 / 4 / 0。
        assert_eq!(
            EipcBlockDevice::new_probed(mk(3, false), 512, 8)
                .health_status()
                .remaining_life,
            0,
            "折码 3 → remaining_life=0"
        );
        assert_eq!(
            EipcBlockDevice::new_probed(mk(4, false), 512, 8)
                .health_status()
                .wear_level,
            100,
            "折码 4 → wear_level=100"
        );
        let perfect = EipcBlockDevice::new_probed(mk(0, false), 512, 8).health_status();
        assert_eq!(
            perfect,
            DeviceHealth {
                total_blocks: 4096,
                bad_blocks: 0,
                wear_level: 0,
                temperature: 0,
                remaining_life: 100,
            },
            "折码 0 = 完美（bad/wear/life 三位干净）"
        );
        // 探测失败（transport Err）→ 本地缺省兜底（total_blocks=构造参数）。
        let dev = EipcBlockDevice::new_probed(mk(2, true), 512, 8);
        let h = dev.health_status();
        assert_eq!(h.total_blocks, 8, "兜底用本地 block_count");
        assert_eq!(h.bad_blocks, 0);
        assert_eq!(h.remaining_life, 100);
        // 既有 `new`（未探测路径）= 完美缺省（mock/测试装配兼容）。
        let dev = EipcBlockDevice::new(mk(2, false), 512, 8);
        assert_eq!(dev.health_status().bad_blocks, 0, "new 不探测=缺省完美");
        assert_eq!(dev.health_status().total_blocks, 8);
    }
    /// 蓝图 76 卷二 WP-76.2.1：数据面**客户端构帧**逐字节金锁。
    ///
    /// 值与 drv-svc `blk_request_bytes_golden_all_subcodes` **同源**
    /// （off = 0x1122334455667788 / data = 0xE0..0xEF）——服务端「解」与
    /// 客户端「构」各有一枚字节锁：任一侧移动布局即双红。WP-76.2.2/3 把
    /// 两侧收编为 `drv-svc::blk_wire` 单源后，本锁仍须原样绿（单源未失守
    /// 的实证；旧形只是各写一遍）。数据面 READ/WRITE 构帧此前**零字节锁**
    /// （`boot_install_bodies_are_body_only` 只覆盖 boot 面）。
    #[test]
    fn data_plane_frames_match_server_golden() {
        struct Recorder {
            frames: Vec<Vec<u8>>,
        }
        impl EipcSend for Recorder {
            fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
                let dreq = wire::decode_request(req).unwrap();
                self.frames
                    .push(dreq.payload[..dreq.payload_len as usize].to_vec());
                let mut resp = DriverResponse::new(dreq.correlation, ST_OK, 7);
                if dreq.payload[0] == BLK_CMD_READ {
                    // 夹具只「应答」不「锁字节」——解请求走同一单源，本测的
                    // 字节裁判是下面的 frames 对拍（夹具自解一遍布局会自洽）。
                    let Ok((_, n)) = blk_wire::parse_read(blk_wire::body_of(&dreq)) else {
                        return Err(());
                    };
                    let n = n as usize;
                    for b in resp.payload[..n].iter_mut() {
                        *b = 0x5A;
                    }
                    resp.payload_len = n as u16;
                }
                Ok(wire::encode_response(&resp).to_vec())
            }
        }

        const OFF: u64 = 0x1122_3344_5566_7788;
        let pat = [
            0xE0u8, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED,
            0xEE, 0xEF,
        ];
        const WRITE_FRAME: [u8; 25] = [
            0x02, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4,
            0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED, 0xEE, 0xEF,
        ];
        const READ_FRAME: [u8; 13] = [
            0x01, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x10, 0x00, 0x00, 0x00,
        ];

        let mut io = BlockIo::new(Recorder { frames: vec![] }, "svc-storage", 7, 1000);
        io.write_from(OFF, &pat).unwrap();
        let mut buf = [0u8; 16];
        io.read_into(OFF, &mut buf).unwrap();

        assert_eq!(io.client.frames.len(), 2);
        assert_eq!(
            &io.client.frames[0][..],
            &WRITE_FRAME[..],
            "BLK_CMD_WRITE 构帧逐字节（子码 + 字节偏移 8 + data）"
        );
        assert_eq!(
            &io.client.frames[1][..],
            &READ_FRAME[..],
            "BLK_CMD_READ 构帧逐字节（子码 + 字节偏移 8 + count 4）"
        );
        assert_eq!(&buf, &[0x5Au8; 16], "读回的响应数据段须落在请求偏移");
    }

    /// 蓝图 76 卷二 WP-76.2.1：`probe_health` **解包方向**逐字节锁。
    /// 13B 应答（blocks 8 + block_size 4 + 折码 1）是 wire 冻结形状——
    /// 客户端只消费 blocks 与折码，`block_size` 为回显冗余（本地已知）。
    /// 短于 13B 一律 `None`（观测面零裁决：不得因应答形状拖垮构造）。
    #[test]
    fn health_response_bytes_decode_lock() {
        struct Fixed {
            bytes: Vec<u8>,
        }
        impl EipcSend for Fixed {
            fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
                let dreq = wire::decode_request(req).unwrap();
                assert_eq!(dreq.payload[0], BLK_CMD_HEALTH);
                let mut resp = DriverResponse::new(dreq.correlation, ST_OK, 7);
                let n = self.bytes.len();
                resp.payload[..n].copy_from_slice(&self.bytes);
                resp.payload_len = n as u16;
                Ok(wire::encode_response(&resp).to_vec())
            }
        }
        // 与 drv-svc HEALTH 金锁同源（blocks=4096 / bs=1 / 折码 2=坏块）。
        const RESP_BAD: [u8; 13] = [
            0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
        ];
        let mut io = BlockIo::new(
            Fixed {
                bytes: RESP_BAD.to_vec(),
            },
            "svc-storage",
            7,
            1000,
        );
        let h = io.probe_health().expect("13B 定形应答必须可解");
        assert_eq!(h.total_blocks, 4096, "blocks 取 payload[0..8] 小端");
        assert_eq!(h.bad_blocks, 1, "折码 2 → bad_blocks=1");
        assert_eq!(h.wear_level, 0);
        assert_eq!(h.remaining_life, 100);
        assert_eq!(h.temperature, 0, "wire 无温度域（恒 0）");

        // 短一字节 = 形状外 → None（不得半解）。
        let mut io = BlockIo::new(
            Fixed {
                bytes: RESP_BAD[..12].to_vec(),
            },
            "svc-storage",
            7,
            1000,
        );
        assert!(io.probe_health().is_none(), "12B 短应答必须拒解");
    }
}
