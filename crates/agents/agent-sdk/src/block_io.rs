//! 47（WP-47.4）：`BlockIo` —— durable-state `PersistIo` 的 EIPC 块客户端
//! 实现（蓝图 41 WP-41.3 落盘先例；现为 control-services 与
//! business-services 共享适配）。
//!
//! 协议：drv-svc 信封（`DriverRequest/DriverResponse`，83B ≤ MR 112B 预算）
//! 经 `Sel4IpcClient::drv_call` 同步调用（4 字节/word 打包，与
//! svc-storage `driver_service_main` 的 MR 语义一致）。命令子码 =
//! `block_cmd` 的 BLK_CMD_*（READ/WRITE/FLUSH/HEALTH）。
//!
//! fail-closed：存储能力未授予（EIPC mask 对应位为 0）→ `Unavailable`；
//! 信封响应 status != ST_OK → 对应 `StoreError`（不伪造成功）。

use eneros_drv_svc::block_cmd::{BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE};
use eneros_drv_svc::runtime::{wire, DriverRequest, DriverResponse, OP_EXECUTE, ST_OK};
use eneros_durable_state::{PersistIo, StoreError};

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
