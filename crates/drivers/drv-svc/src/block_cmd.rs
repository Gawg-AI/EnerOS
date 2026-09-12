//! 块设备命令面（蓝图 39 WP-39.12：svc-storage 的真实命令集）。
//!
//! 信封 payload 有界（PAYLOAD_CAP=461，68.3 扩容）——本命令面承载
//! lba 寻址的定长读/写（含历史 52B 小载荷语义，上限放宽不改变旧调用
//! 方行为）；整块数据通路（共享 DMA 窗口 + 设备句柄移交）随蓝图 40 的
//! EIPC 域接通。命令以 `payload[0]` 子操作码编码：
//!
//! | 子操作码 | 请求 payload | 响应 payload |
//! |---|---|---|
//! | `BLK_CMD_READ` | lba(8) + count(4) | data(≤[`BLK_READ_MAX`]) |
//! | `BLK_CMD_WRITE` | lba(8) + data(≤[`BLK_WRITE_MAX`]) | — |
//! | `BLK_CMD_FLUSH` | — | — |
//! | `BLK_CMD_HEALTH` | — | block_count(8) + block_size(4) + health(1) |
//!
//! fail-closed：无 backend → `ST_UNAVAILABLE`；坏命令/越界 → `ST_INVALID`；
//! 写失败 → `ST_UNAVAILABLE`（不伪装成功）。

extern crate alloc;
use alloc::boxed::Box;

use super::runtime::{DriverRequest, DriverResponse, ST_INVALID, ST_OK, ST_UNAVAILABLE};

/// 子操作码：读（lba 8B + count 4B → data）。
pub const BLK_CMD_READ: u8 = 0x01;
/// 子操作码：写（lba 8B + data ≤[`BLK_WRITE_MAX`]）。
pub const BLK_CMD_WRITE: u8 = 0x02;
/// 子操作码：flush。
pub const BLK_CMD_FLUSH: u8 = 0x03;
/// 子操作码：健康/容量（只读元数据）。
pub const BLK_CMD_HEALTH: u8 = 0x04;
/// 70.7（WP-70.7.2）：启动盘受限安装面子码（0x05-0x08；`install` 面缺席
/// → ST_UNAVAILABLE）。0x05 读全域放行；0x06 安装写（守卫强制：仅非活动
/// BOOT 槽 ∪ BOOTCTRL）；0x07 flush；0x08 守卫范围查询。
pub const BLK_BOOT_READ: u8 = 0x05;
/// 70.7.5（事务下沉）：trial 事务（secver 8 + image_id 16 → seq 8）。
pub const BLK_BOOT_TRIAL: u8 = 0x09;
/// 70.7.5：启动确认（slot_is_a 1 + secver 8 + image_id 16 → seq 8）。
pub const BLK_BOOT_CONFIRM: u8 = 0x0a;
/// 70.7.5：取消未确认 trial（→ seq 8 ‖ word 文本）。
pub const BLK_BOOT_CANCEL: u8 = 0x0b;
/// 70.7.5：状态投影（→ [`crate::boot_install::format_update_status`] 文本）。
pub const BLK_BOOT_STATUS: u8 = 0x0c;
pub const BLK_BOOT_INSTALL_WRITE: u8 = 0x06;
pub const BLK_BOOT_FLUSH: u8 = 0x07;
pub const BLK_BOOT_GUARD: u8 = 0x08;

/// 读命令单次数据上限（响应载荷 = PAYLOAD_CAP；68.3 WP-68.3.1
/// 52→461——旧调用方 ≤52B 行为不变，上限只放宽）。
pub const BLK_READ_MAX: usize = super::runtime::PAYLOAD_CAP;
/// 写命令单次数据上限（payload = 子码 1B + lba 8B + data）。
pub const BLK_WRITE_MAX: usize = super::runtime::PAYLOAD_CAP - 9;

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

/// 70.7（WP-70.7.2）：启动盘受限安装命令（`install` 面守卫强制）。
///
/// | 子码 | 请求 payload | 响应 payload |
/// |---|---|---|
/// | `BLK_BOOT_READ` | lba(8) + count(4) | data(≤[`BLK_READ_MAX`]) |
/// | `BLK_BOOT_INSTALL_WRITE` | lba(8) + data(≤[`BLK_WRITE_MAX`]) | — |
/// | `BLK_BOOT_FLUSH` | — | — |
/// | `BLK_BOOT_GUARD` | — | inact_start(8) + inact_lbas(8) + bc_start(8) + bc_lbas(8) + active_is_a(1) |
///
/// fail-closed：无安装面 → ST_UNAVAILABLE；越界/跨域写 → ST_INVALID
/// （守卫强制：仅非活动 BOOT 槽 ∪ BOOTCTRL；活跃槽/其他分区零写）。
///
/// 70.7.5 事务臂（TRIAL/CONFIRM/CANCEL/STATUS）：每次事务 = **单次**
/// guard 式 IPC——bootctrl 记录零 IPC 搬运（REG-70.7 run16-22 实弹）。
pub fn handle_boot_cmd(
    cmd: u8,
    req: &DriverRequest,
    install: &mut dyn InstallBackend,
    resp: &mut DriverResponse,
) {
    match cmd {
        BLK_BOOT_READ => {
            if req.payload_len < 13 {
                resp.status = ST_INVALID;
                return;
            }
            let lba = u64::from_le_bytes(req.payload[1..9].try_into().unwrap());
            let count = u32::from_le_bytes(req.payload[9..13].try_into().unwrap()) as usize;
            let n = count.min(BLK_READ_MAX);
            let mut buf = [0u8; BLK_READ_MAX];
            match install.read(lba, &mut buf[..n]) {
                Ok(()) => {
                    resp.payload[..n].copy_from_slice(&buf[..n]);
                    resp.payload_len = n as u16;
                    resp.status = ST_OK;
                }
                Err(_) => resp.status = ST_UNAVAILABLE,
            }
        }
        BLK_BOOT_INSTALL_WRITE => {
            if req.payload_len < 9 {
                resp.status = ST_INVALID;
                return;
            }
            let lba = u64::from_le_bytes(req.payload[1..9].try_into().unwrap());
            let data = &req.payload[9..req.payload_len as usize];
            // 守卫强制（WP-70.7.2 核心不变量）：非活动槽 ∪ BOOTCTRL 之外
            // 零写——活跃槽/数据盘/其他分区写请求在存储服务侧拒绝。
            let (is, il, bs, bl) = install.install_ranges();
            let end = lba.saturating_add(data.len() as u64);
            let ok = (lba >= is && end <= is + il) || (lba >= bs && end <= bs + bl);
            if !ok {
                resp.status = ST_INVALID;
                return;
            }
            match install.write(lba, data) {
                Ok(()) => resp.status = ST_OK,
                Err(_) => resp.status = ST_UNAVAILABLE,
            }
        }
        BLK_BOOT_FLUSH => match install.flush() {
            Ok(()) => resp.status = ST_OK,
            Err(_) => resp.status = ST_UNAVAILABLE,
        },
        BLK_BOOT_GUARD => {
            let (is, il, bs, bl) = install.install_ranges();
            resp.payload[..40].copy_from_slice(&{
                let mut b = [0u8; 40];
                b[..8].copy_from_slice(&is.to_le_bytes());
                b[8..16].copy_from_slice(&il.to_le_bytes());
                b[16..24].copy_from_slice(&bs.to_le_bytes());
                b[24..32].copy_from_slice(&bl.to_le_bytes());
                b
            });
            resp.payload_len = 40;
            resp.status = ST_OK;
        }
        // 70.7.5（事务下沉）：bootctrl 事务单次 IPC 完成——bootctrl 记录
        // 零 IPC 搬运（run16-22 实弹：事务拆多次 BLK_BOOT_READ/WRITE 时
        // 同客户端第二次调用稳定挂死于 seL4 Call）。
        BLK_BOOT_TRIAL => {
            if req.payload_len < 25 {
                resp.status = ST_INVALID;
                return;
            }
            let secver = u64::from_le_bytes(req.payload[1..9].try_into().unwrap());
            let mut image_id = [0u8; 16];
            image_id.copy_from_slice(&req.payload[9..25]);
            let mut inst = crate::boot_install::BootInstall::new(install);
            match inst.write_bootctrl_trial(secver, image_id) {
                Ok(seq) => {
                    resp.payload[..8].copy_from_slice(&seq.to_le_bytes());
                    resp.payload_len = 8;
                    resp.status = ST_OK;
                }
                Err(word) => boot_word_reject(word, resp),
            }
        }
        BLK_BOOT_CONFIRM => {
            if req.payload_len < 26 {
                resp.status = ST_INVALID;
                return;
            }
            let slot_is_a = req.payload[1] == 1;
            let secver = u64::from_le_bytes(req.payload[2..10].try_into().unwrap());
            let mut image_id = [0u8; 16];
            image_id.copy_from_slice(&req.payload[10..26]);
            let mut inst = crate::boot_install::BootInstall::new(install);
            match inst.confirm_running(slot_is_a, secver, image_id) {
                Ok(seq) => {
                    resp.payload[..8].copy_from_slice(&seq.to_le_bytes());
                    resp.payload_len = 8;
                    resp.status = ST_OK;
                }
                Err(word) => boot_word_reject(word, resp),
            }
        }
        BLK_BOOT_CANCEL => {
            let mut inst = crate::boot_install::BootInstall::new(install);
            match inst.cancel_trial() {
                Ok(seq) => {
                    resp.payload[..8].copy_from_slice(&seq.to_le_bytes());
                    resp.payload_len = 8;
                    resp.status = ST_OK;
                }
                Err(word) => boot_word_reject(word, resp),
            }
        }
        BLK_BOOT_STATUS => {
            let mut inst = crate::boot_install::BootInstall::new(install);
            match inst.read_current() {
                Ok((c, _)) => {
                    let line = crate::boot_install::format_update_status(&c);
                    let n = core::cmp::min(line.len(), crate::runtime::PAYLOAD_CAP);
                    resp.payload[..n].copy_from_slice(&line.as_bytes()[..n]);
                    resp.payload_len = n as u16;
                    resp.status = ST_OK;
                }
                Err(word) => boot_word_reject(word, resp),
            }
        }
        _ => resp.status = ST_INVALID,
    }
}

/// 事务拒绝（ST_INVALID + payload = word 文本；调用方据词形取证）。
fn boot_word_reject(word: &'static str, resp: &mut DriverResponse) {
    let w = word.as_bytes();
    let n = core::cmp::min(w.len(), resp.payload.len());
    resp.payload[..n].copy_from_slice(&w[..n]);
    resp.payload_len = n as u16;
    resp.status = ST_INVALID;
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
            if count == 0 || count > BLK_READ_MAX {
                resp.status = ST_INVALID;
                return;
            }
            let mut buf = [0u8; BLK_READ_MAX];
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
            if data_len == 0 || data_len > BLK_WRITE_MAX {
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

/// 70.7（WP-70.7.2）：启动盘受限安装面（读全域 + 写守卫范围强制）。
/// 宿主测试以 mock 实现；target 实现 = `blk_backend::BootSlotBackend`。
pub trait InstallBackend: BlockBackend {
    /// (非活动槽起始 lba, 非活动槽 lbas, BOOTCTRL 起始 lba, BOOTCTRL lbas)。
    fn install_ranges(&self) -> (u64, u64, u64, u64);
}

/// 存储服务（svc-storage 用）：持有可选 backend。
pub struct StorageService<B: BlockBackend> {
    backend: Option<B>,
    /// 70.7.2：启动盘受限安装面（None = 未移交 → BLK_BOOT_* 显式
    /// ST_UNAVAILABLE）。
    install: Option<Box<dyn InstallBackend>>,
}

impl<B: BlockBackend> StorageService<B> {
    pub const fn new() -> Self {
        Self {
            backend: None,
            install: None,
        }
    }

    /// 装配 backend（设备句柄移交时调用；39 host 测试直接注入）。
    pub fn attach(&mut self, backend: B) {
        self.backend = Some(backend);
    }

    /// 70.7.2：装配启动盘受限安装面。
    pub fn attach_install(&mut self, install: Box<dyn InstallBackend>) {
        self.install = Some(install);
    }

    pub fn is_attached(&self) -> bool {
        self.backend.is_some()
    }

    pub fn install_attached(&self) -> bool {
        self.install.is_some()
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
        // 70.7.2：启动盘受限安装面子码（在数据面之前判别——子码空间
        // 0x05-0x08 为 boot 专属，数据面子码 0x01-0x04 不受影响）。
        if req.payload_len >= 1
            && matches!(
                req.payload[0],
                BLK_BOOT_READ
                    | BLK_BOOT_INSTALL_WRITE
                    | BLK_BOOT_FLUSH
                    | BLK_BOOT_GUARD
                    | BLK_BOOT_TRIAL
                    | BLK_BOOT_CONFIRM
                    | BLK_BOOT_CANCEL
                    | BLK_BOOT_STATUS
            )
        {
            let Some(install) = self.install.as_mut() else {
                resp.status = ST_UNAVAILABLE;
                return;
            };
            #[cfg(target_os = "none")]
            eneros_runtime::println!("[svc-storage] handle boot sub=0x{:02x} enter", req.payload[0]);
            handle_boot_cmd(req.payload[0], req, install.as_mut(), resp);
            #[cfg(target_os = "none")]
            eneros_runtime::println!("[svc-storage] handle boot sub=0x{:02x} done", req.payload[0]);
            return;
        }
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

    /// 68.3 WP-68.3.1：扩容后满额/奇数长度 roundtrip（旧 ≤52B 调用方
    /// 语义不变——本测只证上限放宽后新旧两段都可用）。
    #[test]
    fn enlarged_payload_roundtrip_and_odd_len() {
        let mut service = StorageService::new();
        service.attach(RamBackend::new(4096));
        let mut resp = DriverResponse::new(1, 0, 1);

        // 满额写（BLK_WRITE_MAX=452）→ 满额读（count=BLK_READ_MAX=461
        // 截到介质边界内）逐字节一致。
        let data: std::vec::Vec<u8> = (0..BLK_WRITE_MAX).map(|i| (i * 7) as u8).collect();
        let mut req = mk_req(BLK_CMD_WRITE);
        req.payload[1..9].copy_from_slice(&0u64.to_le_bytes());
        req.payload[9..9 + data.len()].copy_from_slice(&data);
        req.payload_len = (9 + data.len()) as u16;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK, "满额写必须接受（PAYLOAD_CAP 扩容）");

        let mut req = mk_req(BLK_CMD_READ);
        req.payload[1..9].copy_from_slice(&0u64.to_le_bytes());
        req.payload[9..13].copy_from_slice(&(data.len() as u32).to_le_bytes());
        req.payload_len = 13;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len as usize, data.len());
        assert_eq!(&resp.payload[..data.len()], &data[..]);

        // 奇数长度写读（对齐不敏感）。
        let mut req = mk_req(BLK_CMD_WRITE);
        req.payload[1..9].copy_from_slice(&1001u64.to_le_bytes());
        req.payload[9..12].copy_from_slice(b"abc");
        req.payload_len = 12;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
        let mut req = mk_req(BLK_CMD_READ);
        req.payload[1..9].copy_from_slice(&1001u64.to_le_bytes());
        req.payload[9..13].copy_from_slice(&3u32.to_le_bytes());
        req.payload_len = 13;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(&resp.payload[..3], b"abc");

        // 超上限读拒绝（count > BLK_READ_MAX → ST_INVALID；写超上限在
        // wire 层已被信封容量封死，服务端防线保持防御性对称）。
        let mut req = mk_req(BLK_CMD_READ);
        req.payload[1..9].copy_from_slice(&0u64.to_le_bytes());
        req.payload[9..13].copy_from_slice(&((BLK_READ_MAX + 1) as u32).to_le_bytes());
        req.payload_len = 13;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_INVALID);
    }
}
