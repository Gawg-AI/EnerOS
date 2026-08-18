//! 块设备命令面（蓝图 39 WP-39.12：svc-storage 的真实命令集）。
//!
//! 信封 payload 有界（64 B）——本命令面承载**小载荷**元数据操作
//! （≤56 B 读/写 + lba 寻址）；整块数据通路（共享 DMA 窗口 + 设备句柄
//! 移交）随蓝图 40 的 EIPC 域接通。命令以 `payload[0]` 子操作码编码：
//!
//! | 子操作码 | 请求 payload | 响应 payload |
//! |---|---|---|
//! | `BLK_CMD_READ` | lba(8) + count(4) | data(≤52) |
//! | `BLK_CMD_WRITE` | lba(8) + data(≤56) | — |
//! | `BLK_CMD_FLUSH` | — | — |
//! | `BLK_CMD_HEALTH` | — | block_count(8) + block_size(4) + health(1) |
//!
//! fail-closed：无 backend → `ST_UNAVAILABLE`；坏命令/越界 → `ST_INVALID`；
//! 写失败 → `ST_UNAVAILABLE`（不伪装成功）。

use super::runtime::{DriverRequest, DriverResponse, ST_INVALID, ST_OK, ST_UNAVAILABLE};

/// 子操作码：读（lba 8B + count 4B → data）。
pub const BLK_CMD_READ: u8 = 0x01;
/// 子操作码：写（lba 8B + data ≤56B）。
pub const BLK_CMD_WRITE: u8 = 0x02;
/// 子操作码：flush。
pub const BLK_CMD_FLUSH: u8 = 0x03;
/// 子操作码：健康/容量（只读元数据）。
pub const BLK_CMD_HEALTH: u8 = 0x04;

/// 小载荷数据上限（64B 信封 - 1B 子码 - 8B lba = 55；取 52 留响应头余量）。
pub const BLK_SMALL_DATA: usize = 52;

/// 块后端（服务持有的设备抽象；target 设备句柄随 40 移交）。
pub trait BlockBackend {
    /// 读 `buf.len()` 字节到 `lba` 起（越界 → Err）。
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()>;
    /// 写 `data` 到 `lba` 起。
    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), ()>;
    /// flush（持久化）。
    fn flush(&mut self) -> Result<(), ()>;
    /// (block_count, block_size, health_code)——health 0=healthy。
    fn health(&self) -> (u64, u32, u8);
}

/// 处理一个块命令（子码在 `req.payload[0]`）。
pub fn handle_block_cmd(
    cmd: u8,
    req: &DriverRequest,
    backend: &mut dyn BlockBackend,
    resp: &mut DriverResponse,
) {
    match cmd {
        BLK_CMD_READ => {
            if req.payload_len < 13 {
                resp.status = ST_INVALID;
                return;
            }
            let lba = u64::from_le_bytes(req.payload[1..9].try_into().unwrap());
            let count = u32::from_le_bytes(req.payload[9..13].try_into().unwrap()) as usize;
            if count == 0 || count > BLK_SMALL_DATA {
                resp.status = ST_INVALID;
                return;
            }
            let mut buf = [0u8; BLK_SMALL_DATA];
            match backend.read(lba, &mut buf[..count]) {
                Ok(()) => {
                    resp.status = ST_OK;
                    resp.payload[..count].copy_from_slice(&buf[..count]);
                    resp.payload_len = count as u16;
                }
                Err(()) => {
                    resp.status = ST_UNAVAILABLE;
                }
            }
        }
        BLK_CMD_WRITE => {
            if req.payload_len < 9 {
                resp.status = ST_INVALID;
                return;
            }
            let lba = u64::from_le_bytes(req.payload[1..9].try_into().unwrap());
            let data_len = (req.payload_len - 9) as usize;
            if data_len == 0 || data_len > BLK_SMALL_DATA {
                resp.status = ST_INVALID;
                return;
            }
            match backend.write(lba, &req.payload[9..9 + data_len]) {
                Ok(()) => {
                    resp.status = ST_OK;
                }
                Err(()) => {
                    resp.status = ST_UNAVAILABLE;
                }
            }
        }
        BLK_CMD_FLUSH => match backend.flush() {
            Ok(()) => resp.status = ST_OK,
            Err(()) => resp.status = ST_UNAVAILABLE,
        },
        BLK_CMD_HEALTH => {
            let (blocks, block_size, health) = backend.health();
            resp.payload[..8].copy_from_slice(&blocks.to_le_bytes());
            resp.payload[8..12].copy_from_slice(&block_size.to_le_bytes());
            resp.payload[12] = health;
            resp.payload_len = 13;
            resp.status = ST_OK;
        }
        _ => {
            resp.status = ST_INVALID;
        }
    }
}

/// 存储服务（svc-storage 用）：持有可选 backend。
pub struct StorageService<B: BlockBackend> {
    backend: Option<B>,
}

impl<B: BlockBackend> StorageService<B> {
    pub const fn new() -> Self {
        Self { backend: None }
    }

    /// 装配 backend（设备句柄移交时调用；39 host 测试直接注入）。
    pub fn attach(&mut self, backend: B) {
        self.backend = Some(backend);
    }

    pub fn is_attached(&self) -> bool {
        self.backend.is_some()
    }
}

impl<B: BlockBackend> Default for StorageService<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: BlockBackend> super::runtime::DriverService for StorageService<B> {
    fn capability(&self) -> Option<&'static str> {
        if self.backend.is_some() {
            Some("block")
        } else {
            None
        }
    }
    fn handle(&mut self, req: &DriverRequest, resp: &mut DriverResponse) {
        let Some(backend) = self.backend.as_mut() else {
            // capability() 已拦 None；防御性兜底（不伪装）。
            resp.status = ST_UNAVAILABLE;
            return;
        };
        if req.payload_len == 0 {
            resp.status = ST_INVALID;
            return;
        }
        handle_block_cmd(req.payload[0], req, backend, resp);
    }
}

#[cfg(test)]
mod tests {
    use std::vec;

    use super::super::runtime::{
        DriverRequest, DriverResponse, DriverService, ST_OK, ST_UNAVAILABLE,
    };
    use super::*;

    /// RAM 块后端（测试；drv-svc 本体 no_std 无 alloc——测试经
    /// lib.rs 的 `#[cfg(test)] extern crate std` 提供）。
    struct RamBackend {
        data: std::vec::Vec<u8>,
        flushes: u64,
    }

    impl RamBackend {
        fn new(capacity: usize) -> Self {
            Self {
                data: vec![0u8; capacity],
                flushes: 0,
            }
        }
    }

    impl BlockBackend for RamBackend {
        fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            let start = lba as usize;
            if start + buf.len() > self.data.len() {
                return Err(());
            }
            buf.copy_from_slice(&self.data[start..start + buf.len()]);
            Ok(())
        }
        fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
            let start = lba as usize;
            if start + data.len() > self.data.len() {
                return Err(());
            }
            self.data[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), ()> {
            self.flushes += 1;
            Ok(())
        }
        fn health(&self) -> (u64, u32, u8) {
            (self.data.len() as u64, 1, 0)
        }
    }

    fn mk_req(cmd: u8) -> DriverRequest {
        let mut req = DriverRequest::new(1, super::super::runtime::OP_EXECUTE);
        req.payload[0] = cmd;
        req.payload_len = 1;
        req
    }

    #[test]
    fn read_write_flush_roundtrip() {
        let mut service = StorageService::new();
        service.attach(RamBackend::new(4096));
        let mut resp = DriverResponse::new(1, 0, 1);

        // 写 "hello" 到 lba=10。
        let mut req = mk_req(BLK_CMD_WRITE);
        req.payload[1..9].copy_from_slice(&10u64.to_le_bytes());
        req.payload[9..14].copy_from_slice(b"hello");
        req.payload_len = 14;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);

        // 读回。
        let mut req = mk_req(BLK_CMD_READ);
        req.payload[1..9].copy_from_slice(&10u64.to_le_bytes());
        req.payload[9..13].copy_from_slice(&5u32.to_le_bytes());
        req.payload_len = 13;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(&resp.payload[..resp.payload_len as usize], b"hello");

        // flush。
        let req = mk_req(BLK_CMD_FLUSH);
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
    }

    #[test]
    fn health_reports_capacity() {
        let mut service = StorageService::new();
        service.attach(RamBackend::new(4096));
        let mut resp = DriverResponse::new(1, 0, 1);
        let req = mk_req(BLK_CMD_HEALTH);
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 13);
        let blocks = u64::from_le_bytes(resp.payload[..8].try_into().unwrap());
        assert_eq!(blocks, 4096);
        assert_eq!(resp.payload[12], 0); // healthy
    }

    #[test]
    fn no_backend_reports_unavailable() {
        let mut service: StorageService<RamBackend> = StorageService::new();
        assert!(service.capability().is_none());
        let mut resp = DriverResponse::new(1, 0, 1);
        let req = mk_req(BLK_CMD_HEALTH);
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_UNAVAILABLE);
    }

    #[test]
    fn bad_command_and_oob_rejected() {
        let mut service = StorageService::new();
        service.attach(RamBackend::new(64));
        let mut resp = DriverResponse::new(1, 0, 1);

        // 未知子码 → INVALID。
        let req = mk_req(0x7F);
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_INVALID);

        // 越界读 → UNAVAILABLE。
        let mut req = mk_req(BLK_CMD_READ);
        req.payload[1..9].copy_from_slice(&1000u64.to_le_bytes());
        req.payload[9..13].copy_from_slice(&10u32.to_le_bytes());
        req.payload_len = 13;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_UNAVAILABLE);

        // 空 payload → INVALID。
        let req = DriverRequest::new(1, super::super::runtime::OP_EXECUTE);
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_INVALID);
    }
}
