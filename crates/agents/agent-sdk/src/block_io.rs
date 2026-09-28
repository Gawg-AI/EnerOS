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
/// 蓝图 77 卷二 WP-77.2.3：BootInstall 拒绝词枚举（`BootCommandError::
/// Rejected` 载荷；词形单源 = drv-svc::boot_install）。
pub use eneros_drv_svc::boot_install::BootReject;
use eneros_drv_svc::runtime::{wire, DriverRequest, DriverResponse, OP_EXECUTE, ST_INVALID, ST_OK};
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

/// boot 面原始命令错误（蓝图 77 卷二 WP-77.2.3 具名分类）。
///
/// - [`BootCommandError::Rejected`]：服务端 `ST_INVALID` + 完整已知
///   [`BootReject`] 词（仅四个事务子码）；携带真实恢复语义
///   （`no-pending-trial` ≠ `confirm-secver-mismatch` ≠ `write1`）。
/// - [`BootCommandError::Io`]：传输/信封/epoch/其他状态/未知词形——
///   与数据面同词形（`StoreError`），svc-ota 维持原 `ERR:*io`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootCommandError {
    Rejected(BootReject),
    Io(StoreError),
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
    /// 一次 IPC 交换（蓝图 77 卷二 WP-77.2.3 私有 seam）：payload 上界 +
    /// 一次 `drv_call` + `decode_response_padded` + epoch 验证——**不含**
    /// status 语义映射（那是 [`Self::cmd`] 数据面与 [`Self::raw_command`]
    /// boot 面各自的事）。私有：跨 seam 的响应状态判读只许走这两个出口。
    ///
    /// 错误映射（与收编前 `cmd` 逐项一致，仅不再提前吞 status）：
    /// transport → `Unavailable`；坏信封 → `Encode("bad-envelope")`；
    /// epoch 变化 → `Unavailable`（陈旧句柄，重启后必须重建）。
    fn exchange(&mut self, sub: u8, payload: &[u8]) -> Result<DriverResponse, StoreError> {
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
        Ok(resp)
    }

    /// 数据面命令：非 ST_OK 一律 `WriteFailed`（fail-closed 不伪装——
    /// 数据面无「带原因失败」语义，WP-77.2.3 后该映射显式收拢于此）。
    fn cmd(&mut self, sub: u8, payload: &[u8]) -> Result<DriverResponse, StoreError> {
        let resp = self.exchange(sub, payload)?;
        if resp.status != ST_OK {
            return Err(StoreError::WriteFailed);
        }
        Ok(resp)
    }

    /// 70.7（WP-70.7.2）：原始子码命令（BLK_BOOT_* 安装面；svc-ota 安装
    /// 控制器消费）。payload = 子码后的请求体；ST_OK → 响应数据段。
    ///
    /// 蓝图 77 卷二 WP-77.2.3：boot 面错误**具名分类**——仅对四个事务
    /// 子码（TRIAL/CONFIRM/CANCEL/STATUS）+ `ST_INVALID` + payload 为
    /// **完整已知** [`BootReject`] 词构造 [`BootCommandError::Rejected`]；
    /// 其他子码、其他状态、空词/未知词/坏字节、传输/信封/epoch 错一律
    /// [`BootCommandError::Io`]（svc-ota 维持原 `ERR:*io` 词形）。
    /// 判据历史（不得回退）：收编前 `cmd` 把**所有**非 ST_OK 提前映成
    /// `WriteFailed`，本函数的 status 判断不可达——拒绝词（
    /// `no-pending-trial`/`confirm-secver-mismatch`/`write1`…的不同恢复
    /// 含义）到不了管理端。恢复提前状态门 = 本分类存在的前提；每事务
    /// 仍恰好**一次** storage IPC（不追加「取原因」第二次 Call——历史
    /// 事务链在第二次 seL4 Call 挂死，REG-70.7 run16-22 实弹）。
    pub fn raw_command(
        &mut self,
        sub: u8,
        payload: &[u8],
    ) -> Result<alloc::vec::Vec<u8>, BootCommandError> {
        let resp = self.exchange(sub, payload).map_err(BootCommandError::Io)?;
        if resp.status == ST_OK {
            return Ok(resp.payload[..resp.payload_len as usize].to_vec());
        }
        // 拒绝词分类面：四个事务子码 + ST_INVALID + 完整已知词。
        if resp.status == ST_INVALID
            && matches!(
                sub,
                BLK_BOOT_TRIAL | BLK_BOOT_CONFIRM | BLK_BOOT_CANCEL | BLK_BOOT_STATUS
            )
        {
            let word = &resp.payload[..resp.payload_len as usize];
            if let Some(reject) = BootReject::from_word(word) {
                return Err(BootCommandError::Rejected(reject));
            }
        }
        // 其余失败 = Io（数据面既有词形 `WriteFailed` 承载，服务端
        // ST_UNAVAILABLE/ST_INVALID 皆如此映射——行为与收编前一致）。
        Err(BootCommandError::Io(StoreError::WriteFailed))
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
                    let Ok((off, n)) = blk_wire::parse_read(blk_wire::body_of(&dreq)) else {
                        return Err(());
                    };
                    let n = n as usize;
                    for (i, b) in resp.payload[..n].iter_mut().enumerate() {
                        // 应答字节**随请求偏移而变**：恒等填充（0x5A）会被
                        // 任何形如"读回一堆 5A"的断言自满足，无判别力（独立
                        // 复查轮 P2-5）。此处要求数据取决于 off ⇒ 下面的
                        // read_into 若把偏移解错，断言即红。
                        *b = (off as u8).wrapping_add(i as u8);
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
        assert_eq!(
            buf,
            std::array::from_fn(|i| 0x88u8.wrapping_add(i as u8)),
            "读回段 = f(请求偏移)（OFF 低字节 0x88 起递增）——偏移解错即红"
        );
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

    // ==================== WP-77.2.3：拒绝词穿透端到端 ====================
    //
    // 真实 `handle_boot_cmd`（drv-svc 服务端）经模拟 EIPC 到 `BlockIo`
    // 的失败矩阵：带词/空词/未知词 ST_INVALID、ST_UNAVAILABLE、传输错、
    // 坏响应长度、4B padding、epoch 改变。判据回归面（不得回退）：把
    // `exchange` 的 status 判读提前吞掉（收编前形态）或把 `ERR:` 前缀
    // 去掉，本族即红。

    use core::cell::RefCell;

    use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
    use eneros_drv_svc::block_cmd::{InstallBackend, StorageService};
    use eneros_drv_svc::boot_guard::{BootWriteGuard, RunningIdentity};
    use eneros_drv_svc::runtime::{DriverService, ST_INVALID};

    /// 测试安装面（与 drv-svc `MockFace` 同型：内存介质 + 守卫冻结 +
    /// 运行身份 + 次写失败注入）。
    struct TestInstallFace {
        media: RefCell<vec::Vec<u8>>,
        running: Option<RunningIdentity>,
        fail_second_write: bool,
        writes: usize,
    }

    const LBA: usize = 512;

    impl TestInstallFace {
        fn new() -> std::rc::Rc<core::cell::RefCell<Self>> {
            std::rc::Rc::new(core::cell::RefCell::new(Self {
                media: RefCell::new(vec![0u8; 18500 * LBA]),
                running: Some(RunningIdentity {
                    is_a: true,
                    security_version: 1,
                    release_id: [9u8; 16],
                }),
                fail_second_write: false,
                writes: 0,
            }))
        }

        /// 稳态种子（双 successful：A=15 / B=14 / seq=1 ⇒ 无 pending trial）。
        fn seed_stable(&self) {
            let id = [9u8; 16];
            let mut c = eneros_boot_protocol::BootControl::factory(id, 1, id);
            c.slot_a = eneros_boot_protocol::SlotState::successful(15, 1, id);
            c.slot_b = eneros_boot_protocol::SlotState::successful(14, 1, id);
            c.sequence = 1;
            let rec = c.encode().unwrap();
            let off = 18432 * LBA; // BOOTCTRL_START_LBA（boot_disk 单源）
            self.media.borrow_mut()[off..off + BOOT_CONTROL_RECORD_SIZE]
                .copy_from_slice(&rec[..BOOT_CONTROL_RECORD_SIZE]);
        }
    }

    impl InstallBackend for TestInstallFace {
        fn install_ranges(&self) -> eneros_drv_svc::boot_guard::InstallRanges {
            let media = self.media.borrow();
            let (bc, _) = eneros_ota_ab::boot_disk::lba_range_of(
                eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
            );
            let off = bc as usize * LBA;
            let stride = eneros_drv_svc::boot_guard::BOOTCTRL_COPY_STRIDE_LBAS as usize * LBA;
            let g = BootWriteGuard::resolve_with_running(
                &media[off..off + BOOT_CONTROL_RECORD_SIZE],
                &media[off + stride..off + stride + BOOT_CONTROL_RECORD_SIZE],
                self.running,
            )
            .unwrap_or(BootWriteGuard {
                inactive_start_lba: 0,
                inactive_lbas: 0,
                active_is_a: false,
                active_security_version: 0,
                sequence: 0,
                running: None,
            });
            g.install_ranges()
        }

        fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            let off = eneros_drv_svc::boot_guard::sector_byte_offset(lba).ok_or(())? as usize;
            let media = self.media.borrow();
            let end = off + buf.len();
            if end > media.len() {
                return Err(());
            }
            buf.copy_from_slice(&media[off..end]);
            Ok(())
        }

        fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
            self.writes += 1;
            if self.fail_second_write && self.writes == 2 {
                return Err(());
            }
            let off = eneros_drv_svc::boot_guard::sector_byte_offset(lba).ok_or(())? as usize;
            let mut media = self.media.borrow_mut();
            let end = off + data.len();
            if end > media.len() {
                return Err(());
            }
            media[off..end].copy_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), ()> {
            Ok(())
        }

        fn running_identity(&self) -> Option<RunningIdentity> {
            self.running
        }
    }

    /// 数据面占位后端（本族不走数据面命令）。
    struct NullBackend;
    impl eneros_drv_svc::block_cmd::BlockBackend for NullBackend {
        fn read(&mut self, _o: u64, _b: &mut [u8]) -> Result<(), ()> {
            Err(())
        }
        fn write(&mut self, _o: u64, _d: &[u8]) -> Result<(), ()> {
            Err(())
        }
        fn flush(&mut self) -> Result<(), ()> {
            Ok(())
        }
        fn health(&self) -> (u64, u32, u8) {
            (0, 512, 0)
        }
    }

    /// 模拟 EIPC：请求帧 → 真实 `StorageService::handle` → 响应帧
    /// （可注入：传输错 / 截断 / 伪词 / 4B padding / epoch 漂移）。
    enum MockMode {
        Server,
        TransportErr,
        Truncated,
        /// 伪造响应：status + payload（绕过服务端——空词/未知词形态
        /// 真服务端不会产出，必须直摆）。
        Raw(u8, &'static [u8]),
        /// 正常响应 + 4B 尾部 padding（MR 整 word 形态）。
        Padded,
        /// 首响应 epoch=5（被吸收），次响应 epoch=6（陈旧句柄）。
        EpochSwap,
    }

    struct BootServerMock {
        svc: StorageService<NullBackend>,
        face: Option<std::rc::Rc<core::cell::RefCell<TestInstallFace>>>,
        mode: core::cell::Cell<MockMode>,
        call: core::cell::Cell<usize>,
    }

    impl EipcSend for BootServerMock {
        fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
            match &self.mode.replace(MockMode::Server) {
                MockMode::TransportErr => return Err(()),
                MockMode::Truncated => {
                    let dreq = wire::decode_request(req).unwrap();
                    let mut resp = DriverResponse::new(dreq.correlation, ST_INVALID, 7);
                    resp.payload_len = 0;
                    let full = wire::encode_response(&resp).to_vec();
                    return Ok(full[..full.len() - 8].to_vec()); // < RESPONSE_LEN
                }
                MockMode::Raw(status, payload) => {
                    let dreq = wire::decode_request(req).unwrap();
                    let mut resp = DriverResponse::new(dreq.correlation, *status, 7);
                    resp.payload[..payload.len()].copy_from_slice(payload);
                    resp.payload_len = payload.len() as u16;
                    return Ok(wire::encode_response(&resp).to_vec());
                }
                MockMode::Padded => {
                    let full = self.serve(req, 7);
                    let mut out = full;
                    out.extend_from_slice(&[0u8; 4]);
                    return Ok(out);
                }
                MockMode::EpochSwap => {
                    let n = self.call.get();
                    self.call.set(n + 1);
                    let epoch = if n == 0 { 5 } else { 6 };
                    return Ok(self.serve(req, epoch));
                }
                MockMode::Server => {}
            }
            // Server 模式同样计数（单次 IPC 回归锁的读数源；各 Raw/Padded/
            // EpochSwap 臂此前已各自处理）。
            let n = self.call.get();
            self.call.set(n + 1);
            Ok(self.serve(req, 7))
        }
    }

    impl BootServerMock {
        fn serve(&mut self, req: &[u8], epoch: u8) -> Vec<u8> {
            let dreq = wire::decode_request(req).unwrap();
            let mut resp = DriverResponse::new(dreq.correlation, 0, u64::from(epoch));
            if let Some(face) = &self.face {
                self.svc.attach_install(Box::new(SharedFace(face.clone())));
            }
            self.svc.handle(&dreq, &mut resp);
            wire::encode_response(&resp).to_vec()
        }
    }

    /// Rc<RefCell<TestInstallFace>> → InstallBackend 转发壳
    ///（StorageService 持 Box<dyn InstallBackend>，测试要保种子引用）。
    struct SharedFace(std::rc::Rc<core::cell::RefCell<TestInstallFace>>);
    impl InstallBackend for SharedFace {
        fn install_ranges(&self) -> eneros_drv_svc::boot_guard::InstallRanges {
            self.0.borrow().install_ranges()
        }
        fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            self.0.borrow_mut().read_sectors(lba, buf)
        }
        fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
            self.0.borrow_mut().write_sectors(lba, data)
        }
        fn flush(&mut self) -> Result<(), ()> {
            self.0.borrow_mut().flush()
        }
        fn running_identity(&self) -> Option<RunningIdentity> {
            self.0.borrow().running_identity()
        }
    }

    fn mock_with(
        mode: MockMode,
        seed: bool,
    ) -> (
        BlockIo<BootServerMock>,
        std::rc::Rc<core::cell::RefCell<TestInstallFace>>,
    ) {
        let face = TestInstallFace::new();
        if seed {
            face.borrow().seed_stable();
        }
        let inner = BootServerMock {
            svc: StorageService::new(),
            face: Some(face.clone()),
            mode: core::cell::Cell::new(mode),
            call: core::cell::Cell::new(0),
        };
        (BlockIo::new(inner, "svc-storage", 7, 1000), face)
    }

    /// 真服务端拒绝词 → `Rejected`（四个事务子码各取一形）。
    /// 附「每事务恰好一次 storage IPC」回归锁（蓝图 §3 WP-77.2.3 完成判据；
    /// 历史缺陷 = 追加「取原因」第二次 Call 卡死事务链——计数器此前只在
    /// EpochSwap 臂工作，Server 模式从不自增也从不被断言）。
    #[test]
    fn server_reject_words_classify_as_rejected() {
        // STATUS 空介质 → bootctrl-both-invalid。
        let (mut io, _f) = mock_with(MockMode::Server, false);
        assert_eq!(
            io.raw_command(BLK_BOOT_STATUS, &[]),
            Err(BootCommandError::Rejected(BootReject::BootctrlBothInvalid))
        );
        assert_eq!(io.client.call.get(), 1, "STATUS 事务恰一次 storage IPC");

        // CANCEL 稳态记录（无 pending trial）→ no-pending-trial。
        let (mut io, _f) = mock_with(MockMode::Server, true);
        assert_eq!(
            io.raw_command(BLK_BOOT_CANCEL, &[]),
            Err(BootCommandError::Rejected(BootReject::NoPendingTrial))
        );
        assert_eq!(io.client.call.get(), 1, "CANCEL 事务恰一次 storage IPC");

        // CONFIRM 虚增水位 → confirm-secver-mismatch。
        let (mut io, _f) = mock_with(MockMode::Server, true);
        let p = boot_body::confirm(true, 9, &[9u8; 16]);
        assert_eq!(
            io.raw_command(BLK_BOOT_CONFIRM, &p),
            Err(BootCommandError::Rejected(
                BootReject::ConfirmSecverMismatch
            ))
        );
        assert_eq!(io.client.call.get(), 1, "CONFIRM 事务恰一次 storage IPC");

        // TRIAL 次写失败注入 → write1（阶段词——介质可能已部分更新）。
        let (mut io, f) = mock_with(MockMode::Server, true);
        f.borrow_mut().fail_second_write = true;
        let p = boot_body::trial(2, &[3u8; 16]);
        assert_eq!(
            io.raw_command(BLK_BOOT_TRIAL, &p),
            Err(BootCommandError::Rejected(BootReject::Write1))
        );
    }

    /// 空词 / 未知词 / 其他状态 / 非事务子码的 ST_INVALID 一律 `Io`；
    /// ST_UNAVAILABLE（无安装面）→ Io。裸词不得直返（svc-mgmt 会把
    /// 非 `ERR:` 文本当成功）。
    #[test]
    fn empty_unknown_words_and_other_statuses_stay_io() {
        // 空词 ST_INVALID（TRIAL 事务子码）→ Io。
        let (mut io, _f) = mock_with(MockMode::Raw(ST_INVALID, b""), true);
        let p = boot_body::trial(2, &[3u8; 16]);
        assert_eq!(
            io.raw_command(BLK_BOOT_TRIAL, &p),
            Err(BootCommandError::Io(StoreError::WriteFailed))
        );

        // 未知词 ST_INVALID → Io。
        let (mut io, _f) = mock_with(MockMode::Raw(ST_INVALID, b"no-such-word"), true);
        assert_eq!(
            io.raw_command(BLK_BOOT_CANCEL, &[]),
            Err(BootCommandError::Io(StoreError::WriteFailed))
        );

        // 已知词但**非事务子码**（GUARD 不携带 BootInstall 词）→ Io。
        let (mut io, _f) = mock_with(MockMode::Raw(ST_INVALID, b"flush"), true);
        assert_eq!(
            io.raw_command(BLK_BOOT_GUARD, &[]),
            Err(BootCommandError::Io(StoreError::WriteFailed))
        );

        // 已知词**截断**形态（ST_INVALID + `no-pending-tria`）→ Io：词判定
        // 是完整词匹配，截断词升格 Rejected 即 `ERR:` 假成功面打开（蓝图
        // §2.5 明令禁止）。两段判据分属两 crate——drv-svc 单元 golden 锁
        // 服务端词表，本臂锁客户端切片→from_word 之间不得出现 trim/补齐。
        let (mut io, _f) = mock_with(MockMode::Raw(ST_INVALID, b"no-pending-tria"), true);
        assert_eq!(
            io.raw_command(BLK_BOOT_CANCEL, &[]),
            Err(BootCommandError::Io(StoreError::WriteFailed))
        );

        // ST_UNAVAILABLE（真服务端：安装面缺席 → 能力门先拒）。
        let face = TestInstallFace::new();
        let inner = BootServerMock {
            svc: StorageService::new(),
            face: None,
            mode: core::cell::Cell::new(MockMode::Server),
            call: core::cell::Cell::new(0),
        };
        let mut io = BlockIo::new(inner, "svc-storage", 7, 1000);
        assert_eq!(
            io.raw_command(BLK_BOOT_STATUS, &[]),
            Err(BootCommandError::Io(StoreError::WriteFailed)),
            "ST_UNAVAILABLE 必须落 Io（err 词形 = 服务端各臂 io）"
        );
        let _ = face;
    }

    /// 传输错 → Io(Unavailable)；坏响应长度 → Io(Encode)。
    #[test]
    fn transport_and_envelope_failures_are_io() {
        let (mut io, _f) = mock_with(MockMode::TransportErr, true);
        assert_eq!(
            io.raw_command(BLK_BOOT_TRIAL, &[]),
            Err(BootCommandError::Io(StoreError::Unavailable))
        );
        let (mut io, _f) = mock_with(MockMode::Truncated, true);
        assert!(matches!(
            io.raw_command(BLK_BOOT_STATUS, &[]),
            Err(BootCommandError::Io(StoreError::Encode("bad-envelope")))
        ));
    }

    /// 4B MR padding 容差（真服务端响应 + 尾部 4B）+ 成功臂 8B seq 直通。
    #[test]
    fn padded_response_and_success_path() {
        // 稳态记录：trial → B（seq=2）。Padded = 响应 + 4B 尾 padding。
        let (mut io, _f) = mock_with(MockMode::Padded, true);
        let p = boot_body::trial(2, &[3u8; 16]);
        let v = io.raw_command(BLK_BOOT_TRIAL, &p).expect("padded ok");
        assert_eq!(v.len(), 8, "成功段严格 8B");
        assert_eq!(v, 2u64.to_le_bytes());
    }

    /// epoch 变化 → 陈旧句柄 Io(Unavailable)（不跨重启复用）。
    #[test]
    fn epoch_change_is_io_not_rejected() {
        // 首调吸收语义仅在 client epoch=0 时成立——直构 0 值客户端。
        let (_f, mut io) = {
            let (io, f) = mock_with(MockMode::EpochSwap, true);
            (f, BlockIo::new(io.client, "svc-storage", 0, 1000))
        };
        let p = boot_body::trial(2, &[3u8; 16]);
        // 首调：吸收 epoch 5。
        assert!(io.raw_command(BLK_BOOT_TRIAL, &p).is_ok());
        // 次调：epoch 6 ≠ 5 → Io。
        assert_eq!(
            io.raw_command(BLK_BOOT_TRIAL, &p),
            Err(BootCommandError::Io(StoreError::Unavailable))
        );
    }

    /// 数据面判据不变：非 ST_OK 仍 `WriteFailed`（收编前行为保持——
    /// 数据面无「带原因失败」语义）。
    #[test]
    fn data_plane_failure_still_write_failed() {
        let media = std::rc::Rc::new(RefCell::new(vec![0u8; 4096]));
        let mut io = BlockIo::new(SimStorage { data: media }, "svc-storage", 7, 1000);
        // 越界写 → 服务端 ST_UNAVAILABLE → WriteFailed。
        let overflow: alloc::vec::Vec<u8> = vec![0x5A; 5000];
        assert!(matches!(
            io.write(0, &overflow),
            Err(StoreError::WriteFailed)
        ));
    }
}
