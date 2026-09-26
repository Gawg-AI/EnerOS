//! 块设备命令面（蓝图 39 WP-39.12：svc-storage 的真实命令集）。
//!
//! 信封 payload 有界（PAYLOAD_CAP=461，68.3 扩容）——本命令面承载
//! **字节偏移**寻址的定长读/写（含历史 52B 小载荷语义，上限放宽不改变
//! 旧调用方行为；wire 字段历史名 `lba`，语义 = `offset_bytes`——单位
//! 契约见 [`BlockBackend`]；boot 面扇区语义只在 [`InstallBackend`]）；
//! 整块数据通路（共享 DMA 窗口 + 设备句柄移交）随蓝图 40 的
//! EIPC 域接通。命令以 `payload[0]` 子操作码编码：
//!
//! | 子操作码 | 请求 payload | 响应 payload |
//! |---|---|---|
//! | `BLK_CMD_READ` | offset(8, 字节) + count(4) | data(≤[`BLK_READ_MAX`]) |
//! | `BLK_CMD_WRITE` | offset(8, 字节) + data(≤[`BLK_WRITE_MAX`]) | — |
//! | `BLK_CMD_FLUSH` | — | — |
//! | `BLK_CMD_HEALTH` | — | block_count(8) + block_size(4) + health(1) |
//!
//! fail-closed：无 backend → `ST_UNAVAILABLE`；坏命令/越界 → `ST_INVALID`；
//! 写失败 → `ST_UNAVAILABLE`（不伪装成功）。

extern crate alloc;
use alloc::boxed::Box;

use super::runtime::{DriverRequest, DriverResponse, ST_INVALID, ST_OK, ST_UNAVAILABLE};
use crate::blk_wire;

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

/// 70.14 探针（REG-70.12-PROBE-BOOT-TRIAL-STALL 第六轮定位）：boot 面写
/// **进入驱动调用前**的自增计数。与串口行配对使用——串口无输出而计数递增
/// ⇒ 阻塞在 console 打印路径；计数不递增 ⇒ 阻塞在该调用边界。
/// 生产可读（`driver_service_main` 之外无副作用），零行为影响。
#[cfg(target_os = "none")]
pub static BOOT_WRITE_PRE_PROBES: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// 70.14 探针（REG-70.12-PROBE-BOOT-TRIAL-STALL 第七轮）：**驱动面进入**
/// 计数——由 `VirtioBlkBackend::write` 方法体首行自增。与
/// [`BOOT_WRITE_PRE_PROBES`] 配对：若 PRE 递增而本计数**不**递增 ⇒ 阻塞在
/// trait 分派调用边界；两者都递增 ⇒ 阻塞在驱动方法体内部（下一次可再细分为
/// 「进入 write_block 前 / issue 内 / wait_used 内」）。
#[cfg(target_os = "none")]
pub static BLK_WRITE_ENTER_PROBES: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
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
///
/// **单位契约（73 批A C9）**：本 trait 的 `offset_bytes` = **字节偏移**
/// ——数据面语义，与 2/3 实现（VirtioBlkBackend/RamBackend）及 39 命令
/// 面一致。boot 面（512B 扇区 LBA）一律走 [`InstallBackend::read_sectors`]
/// / [`InstallBackend::write_sectors`]——同名方法不承载两种单位
///（[`InstallBackend`] 已不带本 trait 约束：boot 面实现不再被迫提供
/// 数据面保留体，BootSlotBackend 塞进 [`StorageService::attach`] 的
/// 静默 ×512 陷阱从「约定不发生」变「编译不可表示」）。
pub trait BlockBackend {
    /// 读 `buf.len()` 字节到 `offset_bytes` 起（越界 → Err）。
    fn read(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), ()>;
    /// 写 `data` 到 `offset_bytes` 起。
    fn write(&mut self, offset_bytes: u64, data: &[u8]) -> Result<(), ()>;
    /// flush（持久化）。
    fn flush(&mut self) -> Result<(), ()>;
    /// (block_count, block_size, health_code)——health 0=healthy。
    fn health(&self) -> (u64, u32, u8);
}

/// 70.7（WP-70.7.2）：启动盘受限安装命令（`install` 面守卫强制）。
///
/// | 子码 | 请求 payload | 响应 payload |
/// |---|---|---|
/// | `BLK_BOOT_READ` | lba(8, 扇区) + count(4, 字节) | data(≤[`BLK_READ_MAX`]) |
/// | `BLK_BOOT_INSTALL_WRITE` | lba(8, 扇区) + data(≤[`BLK_WRITE_MAX`], 字节) | — |
/// | `BLK_BOOT_FLUSH` | — | — |
/// | `BLK_BOOT_GUARD` | — | inact_start(8) + inact_lbas(8) + bc_start(8) + bc_lbas(8) + 保留(8, 恒零) |
///
/// wire 纠偏（73 批A P3）：`BLK_BOOT_GUARD` 响应实际 40B = 32B 有效
///（4×u64 小端，[`crate::boot_guard::InstallRanges::pack_le`] 单源）+
/// 尾部 8B 保留恒零。旧表声称 33B 含 `active_is_a(1)`——该字节全仓无
/// 写入者，**登记未上线**（Open：是否上线属 owner 决策，上线 = wire
/// 行为变化另立卷；本表只写真话）。
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
            let (lba, count) = match blk_wire::parse_boot_read(blk_wire::body_of(req)) {
                Ok(v) => v,
                Err(_) => {
                    resp.status = ST_INVALID;
                    return;
                }
            };
            let count = count as usize;
            // 70.14 复查修复：**拒绝而非静默截断**（与 `BLK_CMD_READ` 同律）。
            // 旧实现 `count.min(BLK_READ_MAX)` 让"请求 4096B、实收 461B"以
            // ST_OK 返回，调用方无法从状态区分截断——单发读面上的静默短读
            // 等于把"证据不全"伪装成"读成功"。count=0 同样拒绝（非读语义）。
            if count == 0 || count > BLK_READ_MAX {
                resp.status = ST_INVALID;
                return;
            }
            let n = count;
            let mut buf = [0u8; BLK_READ_MAX];
            match install.read_sectors(lba, &mut buf[..n]) {
                Ok(()) => {
                    resp.payload[..n].copy_from_slice(&buf[..n]);
                    resp.payload_len = n as u16;
                    resp.status = ST_OK;
                }
                Err(_) => resp.status = ST_UNAVAILABLE,
            }
        }
        BLK_BOOT_INSTALL_WRITE => {
            let (lba, data) = match blk_wire::parse_boot_write(blk_wire::body_of(req)) {
                Ok(v) => v,
                Err(_) => {
                    resp.status = ST_INVALID;
                    return;
                }
            };
            // 守卫强制（WP-70.7.2 核心不变量）：非活动槽 ∪ BOOTCTRL 之外
            // 零写——活跃槽/数据盘/其他分区写请求在存储服务侧拒绝。
            //（单位换算 = `write_allowed` 单源；见其文档。）
            if !install.write_allowed(lba, data.len()) {
                resp.status = ST_INVALID;
                return;
            }
            match install.write_sectors(lba, data) {
                Ok(()) => resp.status = ST_OK,
                Err(_) => resp.status = ST_UNAVAILABLE,
            }
        }
        BLK_BOOT_FLUSH => match install.flush() {
            Ok(()) => resp.status = ST_OK,
            Err(_) => resp.status = ST_UNAVAILABLE,
        },
        BLK_BOOT_GUARD => {
            // 40B wire 单源打包（C1：产物与基线逐字节一致——小端
            // offsets 0/8/16/24 + 尾部 8B 保留恒零；golden 测试锁死）。
            resp.payload[..40].copy_from_slice(&install.install_ranges().pack_le());
            resp.payload_len = 40;
            resp.status = ST_OK;
        }
        // 70.7.5（事务下沉）：bootctrl 事务单次 IPC 完成——bootctrl 记录
        // 零 IPC 搬运（run16-22 实弹：事务拆多次 BLK_BOOT_READ/WRITE 时
        // 同客户端第二次调用稳定挂死于 seL4 Call）。
        BLK_BOOT_TRIAL => {
            // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 定位插桩）：storage 侧
            // 是否真的收到 trial 请求——与 svc-ota 侧 `TX-SENT/RX-*` 两行配对，
            // 一眼分出"请求没到 storage"与"storage 回了但 ota 没收"。
            #[cfg(target_os = "none")]
            eneros_runtime::println!(
                "[svc-storage] BOOT-TRIAL: RX req_len={} (slot 0x09)",
                req.payload_len
            );
            let (secver, image_id) = match blk_wire::parse_boot_trial(blk_wire::body_of(req)) {
                Ok(v) => v,
                Err(_) => {
                    resp.status = ST_INVALID;
                    return;
                }
            };
            let mut inst = crate::boot_install::BootInstall::new(install);
            match inst.write_bootctrl_trial(secver, image_id) {
                Ok(seq) => {
                    resp.payload[..8].copy_from_slice(&blk_wire::pack_seq_resp(seq));
                    resp.payload_len = 8;
                    resp.status = ST_OK;
                }
                Err(word) => boot_word_reject(word, resp),
            }
        }
        BLK_BOOT_CONFIRM => {
            let (slot_is_a, secver, image_id) =
                match blk_wire::parse_boot_confirm(blk_wire::body_of(req)) {
                    Ok(v) => v,
                    Err(_) => {
                        resp.status = ST_INVALID;
                        return;
                    }
                };
            let mut inst = crate::boot_install::BootInstall::new(install);
            match inst.confirm_running(slot_is_a, secver, image_id) {
                Ok(seq) => {
                    resp.payload[..8].copy_from_slice(&blk_wire::pack_seq_resp(seq));
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
                    resp.payload[..8].copy_from_slice(&blk_wire::pack_seq_resp(seq));
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
                    match crate::boot_install::format_update_status(&c, &mut resp.payload[..]) {
                        Ok(n) => {
                            resp.payload_len = n as u16;
                            resp.status = ST_OK;
                        }
                        Err(word) => boot_word_reject(word, resp),
                    }
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
            // wire 字段历史名 lba，数据面语义 = 字节偏移（73 批A C9 单位
            // 契约；与 BlockBackend::read 形参同名对齐）。
            let (offset_bytes, count) = match blk_wire::parse_read(blk_wire::body_of(req)) {
                Ok(v) => v,
                Err(_) => {
                    resp.status = ST_INVALID;
                    return;
                }
            };
            let count = count as usize;
            if count == 0 || count > BLK_READ_MAX {
                resp.status = ST_INVALID;
                return;
            }
            let mut buf = [0u8; BLK_READ_MAX];
            match backend.read(offset_bytes, &mut buf[..count]) {
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
            let (offset_bytes, data) = match blk_wire::parse_write(blk_wire::body_of(req)) {
                Ok(v) => v,
                Err(_) => {
                    resp.status = ST_INVALID;
                    return;
                }
            };
            // 域规则留在调用方（卷二 §10）：空数据段是「写零字节」语义问题，
            // 不是布局问题。
            if data.is_empty() || data.len() > BLK_WRITE_MAX {
                resp.status = ST_INVALID;
                return;
            }
            match backend.write(offset_bytes, data) {
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
            resp.payload[..13].copy_from_slice(&blk_wire::pack_health(blocks, block_size, health));
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
///
/// **不带 [`BlockBackend`] 约束**（2026-09-24 单位契约收口）：经
/// `dyn InstallBackend` 实际消费的数据面方法只有 `flush`——supertrait
/// 只为它强加了整张数据面 interface，迫使每个安装面实现写一套同名
/// 异单位保留体（数据面 `read`=字节 / 保留体 `read`=扇区，塞进
/// [`StorageService::attach`] 即静默 ×512）。现 `flush` 为本 trait
/// 自有方法，双 trait 互不隶属；boot 面与数据面的分派槽位互不可达。
pub trait InstallBackend {
    /// 安装区间（具名承载；单位 = 512B 扇区 LBA / 扇区数）。字段语义：
    /// `inactive_start_lba`/`inactive_lbas` = 非运行 BOOT 槽（可写安装
    /// 目标，身份缺失时 0/0 = 写面关闭）；`bootctrl_start_lba`/
    /// `bootctrl_lbas` = BOOTCTRL 事务分区。wire 打包/解包单源 =
    /// [`crate::boot_guard::InstallRanges::pack_le`]/[`unpack_le`]。
    fn install_ranges(&self) -> crate::boot_guard::InstallRanges;

    /// boot 面读取（单位 = **512B 扇区 LBA**）。boot 面调用点一律走本
    /// 方法——继承名 `BlockBackend::read` 是数据面字节偏移语义（73 批A
    /// C9：同一方法名不承载两种单位）。无默认体 = 每个 boot 面实现
    /// 必须显式表态单位。
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()>;

    /// boot 面写入（单位 = **512B 扇区 LBA**；写守卫由实现内强制——
    /// `BootSlotBackend::write_sectors` 深度防御复核同一不变量）。
    fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), ()>;

    /// F70-02（S1）：本次**实际运行**槽身份（可信启动交接数据；init 期
    /// 解析、本 boot 内冻结）。None = 身份缺失/无效 ⇒ 确认面拒绝、安装
    /// 写面已由空区间整体关闭。
    fn running_identity(&self) -> Option<crate::boot_guard::RunningIdentity>;

    /// 安装面 flush（事务持久化承诺：bootctrl 双副本写后必须落设备——
    /// `BootInstall` 四事务的提交点）。原经 supertrait 借用数据面
    /// `BlockBackend::flush`；supertrait 拆除后为本 trait 自有方法，
    /// 语义不变（生产实现仍下沉同一驱动 FLUSH 路径）。
    fn flush(&mut self) -> Result<(), ()>;

    /// 安装写范围守卫（WP-70.7.2 核心不变量：非活动槽 ∪ BOOTCTRL 之外
    /// 零写）。**单位契约**：`lba` = 512B 扇区；`len_bytes` = 写入字节数
    /// ——字节→扇区上取整的换算**只在此处**发生（旧实现在两处调用点各
    /// 抄一份判定式且按字节相加 → 4096B 记录写被误判越界，安装事务恒
    /// ST_UNAVAILABLE；run54 实弹 REG-70.7-BOOT-TRIAL-IO）。
    fn write_allowed(&self, lba: u64, len_bytes: usize) -> bool {
        let sectors = (len_bytes as u64).div_ceil(512);
        crate::boot_guard::install_write_allowed(&self.install_ranges(), lba, sectors)
    }
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
            handle_boot_cmd(req.payload[0], req, install.as_mut(), resp);
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
        fn read(&mut self, offset_bytes: u64, buf: &mut [u8]) -> Result<(), ()> {
            let start = offset_bytes as usize;
            if start + buf.len() > self.data.len() {
                return Err(());
            }
            buf.copy_from_slice(&self.data[start..start + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset_bytes: u64, data: &[u8]) -> Result<(), ()> {
            let start = offset_bytes as usize;
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

    /// 启动盘安装面测试替身（`install_ranges` 与生产同源 = boot_disk 介质图）。
    /// supertrait 拆除后只实现 [`InstallBackend`]——数据面保留体不复存在。
    struct InstallFace {
        ram: RamBackend,
    }

    impl InstallBackend for InstallFace {
        fn install_ranges(&self) -> crate::boot_guard::InstallRanges {
            use eneros_ota_ab::boot_disk::{lba_range_of, BootPartition};
            let (b, bl) = lba_range_of(BootPartition::BootB);
            let (bc, bcl) = lba_range_of(BootPartition::BootCtrl);
            crate::boot_guard::InstallRanges {
                inactive_start_lba: b,
                inactive_lbas: bl,
                bootctrl_start_lba: bc,
                bootctrl_lbas: bcl,
            }
        }

        /// boot 面单位（扇区 LBA）——扇区→字节换算单源 =
        /// `boot_guard::sector_byte_offset`（RamBackend 是字节平面；73 批A
        /// C9 后测试替身与生产同单位口径）。
        fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            let off = crate::boot_guard::sector_byte_offset(lba).ok_or(())?;
            self.ram.read(off, buf)
        }
        fn write_sectors(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
            let off = crate::boot_guard::sector_byte_offset(lba).ok_or(())?;
            self.ram.write(off, data)
        }

        fn flush(&mut self) -> Result<(), ()> {
            self.ram.flush()
        }

        /// F70-02：测试替身 = A 槽运行身份（与 install_ranges 的非活动=B
        /// 口径一致）。
        fn running_identity(&self) -> Option<crate::boot_guard::RunningIdentity> {
            Some(crate::boot_guard::RunningIdentity {
                is_a: true,
                security_version: 1,
                release_id: [0u8; 16],
            })
        }
    }

    /// 70.14 复查红测：`BLK_BOOT_READ` 的 `count` **拒绝而不截断**。
    ///
    /// 缺陷形态（旧实现）：`count.min(BLK_READ_MAX)` 让"请求 4096B、实收
    /// 461B"以 ST_OK 返回——调用方从状态位无法区分"读全了"与"被截断"，
    /// 等于把"证据不全"伪装成"读成功"（`BLK_CMD_READ` 早已是拒绝语义，
    /// 两者不对称）。count=0 同样非读语义。
    #[test]
    fn boot_read_rejects_zero_and_over_cap_count_instead_of_truncating() {
        // 容量按扇区平面计（read_sectors/write_sectors ×512 → 字节）：
        // 40000 扇区 ≈ 20.5MB，覆盖 BOOTCTRL 基址 18432（73 批A C9 后
        // 测试替身与生产同单位口径）。数据面槽位用 RamBackend 占位
        //（本测只走 boot 子码——supertrait 拆除后 InstallFace 不再可
        // 兼任数据面后端，双面装配各自诚实）。
        let mut service: StorageService<RamBackend> = StorageService::new();
        service.attach(RamBackend::new(64));
        service.attach_install(std::boxed::Box::new(InstallFace {
            ram: RamBackend::new(40000 * 512),
        }));
        let mut resp = DriverResponse::new(1, 0, 1);

        // 预置数据：经**安装面**写 BOOTCTRL 区首（守卫放行；读走同一实例）。
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        let mut req = mk_req(BLK_BOOT_INSTALL_WRITE);
        req.payload[1..9].copy_from_slice(&bc_lba.to_le_bytes());
        req.payload[9..13].copy_from_slice(b"abcd");
        req.payload_len = 13;
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK, "BOOTCTRL 区内安装写放行");

        let mut boot_read = |count: u32, resp: &mut DriverResponse| {
            let mut req = mk_req(BLK_BOOT_READ);
            req.payload[1..9].copy_from_slice(&bc_lba.to_le_bytes());
            req.payload[9..13].copy_from_slice(&count.to_le_bytes());
            req.payload_len = 13;
            service.handle(&req, resp);
        };

        // 超上限：拒绝（旧实现截断到 BLK_READ_MAX 并回 ST_OK）。
        let mut resp = DriverResponse::new(1, 0, 1);
        boot_read((BLK_READ_MAX + 1) as u32, &mut resp);
        assert_eq!(
            resp.status, ST_INVALID,
            "count > BLK_READ_MAX 必须拒绝（不得静默截断）"
        );

        // 零长读：拒绝。
        let mut resp = DriverResponse::new(1, 0, 1);
        boot_read(0, &mut resp);
        assert_eq!(resp.status, ST_INVALID, "count=0 非读语义");

        // 域内读：正常返回，且**实收 = 请求**（截断不可见性已消除）。
        let mut resp = DriverResponse::new(1, 0, 1);
        boot_read(4, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 4, "实收长度必须等于请求长度");
        assert_eq!(&resp.payload[..4], b"abcd");
    }

    /// 73 批A（T-golden，C1 执行机构）：`BLK_BOOT_GUARD` 40B 响应逐字节
    /// 金锁。值 = boot_disk qemu-v1 真值（A 运行 ⇒ 非活动 = BOOT_B
    /// (36864, 16384)；BOOTCTRL (18432, 2048)）；布局 = 4×u64 小端
    /// offsets 0/8/16/24 + 尾部 8B 保留恒零，`payload_len=40`。
    /// 期望字节**手工展开**（不借道生产打包路径）——本测在 InstallRanges
    /// 具名化（P1）前后必须原样绿，wire 冻结由此执行。
    #[test]
    fn blk_boot_guard_response_bytes_golden() {
        let mut service: StorageService<RamBackend> = StorageService::new();
        service.attach_install(std::boxed::Box::new(InstallFace {
            ram: RamBackend::new(1024),
        }));
        let mut resp = DriverResponse::new(1, 0, 1);
        let req = mk_req(BLK_BOOT_GUARD);
        service.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 40, "40B = 32B 有效 + 8B 保留");
        let expect: [u8; 40] = [
            // inactive_start_lba = BOOT_B 基址 36864（qemu-v1）
            0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // inactive_lbas = 16384（8 MiB / 512B）
            0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // bootctrl_start_lba = 18432
            0x00, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // bootctrl_lbas = 2048（1 MiB / 512B）
            0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // 尾部 8B 保留恒零（active_is_a 登记未上线，不在线格式）
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(&resp.payload[..40], &expect, "BLK_BOOT_GUARD wire 逐字节");
    }

    /// 73 批A（T-signature-lock，S1 防回归）：`install_ranges` 不得回归
    /// 裸四元组返回签名。**窄域**：只扫三个具名文件（蓝图 72 教训——
    /// 全文件宽子串禁令假红）；**模式**取完整签名尾巴。禁令针在运行时
    /// 拼装（源码字面不含该串——include 自身会自匹配）。
    #[test]
    fn install_ranges_bare_tuple_signature_regression_lock() {
        let needle: std::string::String = ["-> (u64", ", u64", ", u64", ", u64)"].concat();
        let files: [(&str, &str); 3] = [
            ("block_cmd.rs", include_str!("block_cmd.rs")),
            ("blk_backend.rs", include_str!("blk_backend.rs")),
            ("boot_install.rs", include_str!("boot_install.rs")),
        ];
        for (name, src) in files {
            assert!(
                !src.contains(needle.as_str()),
                "{name}: install_ranges 不得回归裸四元组签名（73 批A S1：字段名进类型）"
            );
        }
    }

    /// 2026-09-24 单位契约收口（supertrait 拆除）防回归锁：三个具名文件
    /// 不得再现（a）`InstallBackend` 的数据面 trait 约束；（b）任何 boot 面
    /// 类型的「数据面保留体」impl——同名 read/write 双单位陷阱（run15
    /// 实弹：扇区值直传数据面 /512 错块）从「约定不发生」变「编译不可
    /// 表示」，本锁把「不再可表示」本身钉住。禁令针在运行时拼装（源码
    /// 字面不含完整串——include 自身不会自匹配）。
    #[test]
    fn install_backend_supertrait_and_retention_bodies_regression_lock() {
        let supertrait_needle: std::string::String =
            ["InstallBackend", ": Block", "Backend"].concat();
        let retention_needles: [std::string::String; 3] = [
            ["BlockBackend for Boot", "SlotBackend"].concat(),
            ["BlockBackend for Install", "Face"].concat(),
            ["BlockBackend for Mock", "Face"].concat(),
        ];
        let files: [(&str, &str); 3] = [
            ("block_cmd.rs", include_str!("block_cmd.rs")),
            ("blk_backend.rs", include_str!("blk_backend.rs")),
            ("boot_install.rs", include_str!("boot_install.rs")),
        ];
        for (name, src) in files {
            assert!(
                !src.contains(supertrait_needle.as_str()),
                "{name}: InstallBackend 不得回归数据面 trait 约束（2026-09-24 拆除）"
            );
            for needle in &retention_needles {
                assert!(
                    !src.contains(needle.as_str()),
                    "{name}: 不得再现 boot 面类型的数据面保留体 impl（同名双单位陷阱）"
                );
            }
        }
    }

    // ===== 蓝图 76 卷二 WP-76.2.1：块命令 wire 逐字节金锁（全子码）=====
    //
    // 工艺 = 与 [`blk_boot_guard_response_bytes_golden`] 同源：期望值是**现服务端
    // 跑出来的**（一次性夹具测试 `-- --nocapture` 打 hex 后固化为字面量），不是
    // 手算——手算会把「我以为的布局」当成「实际布局」，金锁就失去判别力。
    // 本族是 WP-76.2.2~4 全部重构的**唯一字节裁判**：任何 wire 布局移动都在这里
    // 变红（含替身收编后与 blk_wire 的对拍）。
    //
    // 夹具事实（生成时的取值，勿"顺手改得更漂亮"——改了即换 wire）：
    //   off = 0x1122334455667788 / count = 0x99AADDBB / data 模式串 = 0xE0..0xEF
    //   trial: secver = 0x0B0C0D0E0F101112 + image_id = 0xA0..0xAF
    //   boot 事务夹具 = A 运行身份（secver 1 / release_id 全零）+ bootctrl 种子
    //   （A=successful(15) / B=successful(14) / sequence=1）⇒ 非活动 = B。

    /// 数据面 + boot 面**请求**帧逐字节（READ/WRITE/FLUSH/BOOT_READ）。
    /// READ 与 BOOT_READ 布局同形而**单位不同**（字节偏移 vs 扇区 LBA）——
    /// 金锁锁字节，单位史实写进 [`blk_wire`] 模块 doc（WP-76.2.2）。
    #[test]
    fn blk_request_bytes_golden_all_subcodes() {
        let off = 0x1122_3344_5566_7788u64;
        let count = 0x99AA_DDBBu32;
        let pat = [
            0xE0u8, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED,
            0xEE, 0xEF,
        ];

        const READ_REQ: [u8; 13] = [
            0x01, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xBB, 0xDD, 0xAA, 0x99,
        ];
        const BOOT_READ_REQ: [u8; 13] = [
            0x05, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xBB, 0xDD, 0xAA, 0x99,
        ];
        const WRITE_REQ: [u8; 25] = [
            0x02, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4,
            0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED, 0xEE, 0xEF,
        ];
        const FLUSH_REQ: [u8; 1] = [0x03];

        let mut req = mk_req(BLK_CMD_READ);
        req.payload[1..9].copy_from_slice(&off.to_le_bytes());
        req.payload[9..13].copy_from_slice(&count.to_le_bytes());
        req.payload_len = 13;
        assert_eq!(&req.payload[..13], &READ_REQ, "BLK_CMD_READ 请求逐字节");

        let mut req = mk_req(BLK_BOOT_READ);
        req.payload[1..9].copy_from_slice(&off.to_le_bytes());
        req.payload[9..13].copy_from_slice(&count.to_le_bytes());
        req.payload_len = 13;
        assert_eq!(
            &req.payload[..13],
            &BOOT_READ_REQ,
            "BLK_BOOT_READ 请求逐字节"
        );

        let mut req = mk_req(BLK_CMD_WRITE);
        req.payload[1..9].copy_from_slice(&off.to_le_bytes());
        req.payload[9..25].copy_from_slice(&pat);
        req.payload_len = 25;
        assert_eq!(&req.payload[..25], &WRITE_REQ, "BLK_CMD_WRITE 请求逐字节");

        let req = mk_req(BLK_CMD_FLUSH);
        assert_eq!(
            &req.payload[..1],
            &FLUSH_REQ,
            "BLK_CMD_FLUSH 请求 = 只带子码"
        );
    }

    /// 数据面**响应**逐字节：READ 数据段（`payload_len = count`，数据从
    /// payload[0] 起、无头部）、HEALTH 13B（blocks/bs/折码）、FLUSH 空。
    #[test]
    fn blk_data_plane_response_bytes_golden() {
        let pat = [
            0xE0u8, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED,
            0xEE, 0xEF,
        ];
        const HEALTH_RESP: [u8; 13] = [
            0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // blocks = 4096
            0x01, 0x00, 0x00, 0x00, // block_size = 1（RamBackend 口径）
            0x00, // 折码 0 = healthy
        ];

        let mut service = StorageService::new();
        service.attach(RamBackend::new(4096));

        let mut resp = DriverResponse::new(1, 0, 1);
        let mut w = mk_req(BLK_CMD_WRITE);
        w.payload[1..9].copy_from_slice(&0x100u64.to_le_bytes());
        w.payload[9..25].copy_from_slice(&pat);
        w.payload_len = 25;
        service.handle(&w, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 0, "WRITE 成功响应零载荷");

        let mut resp = DriverResponse::new(1, 0, 1);
        let mut r = mk_req(BLK_CMD_READ);
        r.payload[1..9].copy_from_slice(&0x100u64.to_le_bytes());
        r.payload[9..13].copy_from_slice(&16u32.to_le_bytes());
        r.payload_len = 13;
        service.handle(&r, &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 16, "READ 响应 payload_len = 请求 count");
        assert_eq!(&resp.payload[..16], &pat, "READ 响应数据段逐字节");

        let mut resp = DriverResponse::new(1, 0, 1);
        service.handle(&mk_req(BLK_CMD_FLUSH), &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(
            resp.payload_len, 0,
            "FLUSH 响应零载荷（每请求独立响应结构）"
        );

        let mut resp = DriverResponse::new(1, 0, 1);
        service.handle(&mk_req(BLK_CMD_HEALTH), &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 13);
        assert_eq!(&resp.payload[..13], &HEALTH_RESP, "HEALTH 13B 逐字节");
    }

    /// boot 面**请求**逐字节：TRIAL 25B（secver 8 + image_id 16）、
    /// CONFIRM 26B（slot_is_a 1 + secver 8 + image_id 16）。
    /// CONFIRM 首字节判据是 `== 1`（非「非零即 A」）——`slot_is_a` 解析单源
    /// 在 [`blk_wire::parse_boot_confirm`]，本锁冻结其线形。
    #[test]
    fn blk_boot_transaction_request_bytes_golden() {
        let secver = 0x0B0C_0D0E_0F10_1112u64;
        let id = [
            0xA0u8, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD,
            0xAE, 0xAF,
        ];
        const TRIAL_REQ: [u8; 25] = [
            0x09, 0x12, 0x11, 0x10, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4,
            0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF,
        ];
        const CONFIRM_REQ: [u8; 26] = [
            0x0A, 0x01, 0x12, 0x11, 0x10, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0xA0, 0xA1, 0xA2, 0xA3,
            0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF,
        ];

        let mut req = mk_req(BLK_BOOT_TRIAL);
        req.payload[1..9].copy_from_slice(&secver.to_le_bytes());
        req.payload[9..25].copy_from_slice(&id);
        req.payload_len = 25;
        assert_eq!(&req.payload[..25], &TRIAL_REQ, "BLK_BOOT_TRIAL 请求逐字节");

        let mut req = mk_req(BLK_BOOT_CONFIRM);
        req.payload[1] = 1;
        req.payload[2..10].copy_from_slice(&secver.to_le_bytes());
        req.payload[10..26].copy_from_slice(&id);
        req.payload_len = 26;
        assert_eq!(
            &req.payload[..26],
            &CONFIRM_REQ,
            "BLK_BOOT_CONFIRM 请求逐字节"
        );
    }

    /// boot 面**事务响应 seq** 逐字节（TRIAL/CANCEL 成功臂 8B 小端 + len=8）。
    /// CONFIRM 成功臂不在此锁：block_cmd 的 `InstallFace` 只建模 A 运行身份，
    /// confirm 过身份门后卡在记录重编码（seq 成功属 boot_install 事务域，
    /// API 级锁 `confirm_marks_successful_and_sequence_advances` 已存）——
    /// seq **打包字节形**与 TRIAL/CANCEL 同一处代码，两臂已足以冻结
    /// （收编后由 [`blk_wire::pack_seq_resp`] 单源承担）。
    #[test]
    fn blk_boot_seq_response_bytes_golden() {
        use eneros_boot_protocol::boot_control::BOOT_CONTROL_RECORD_SIZE;
        const TRIAL_SEQ_RESP: [u8; 8] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        const CANCEL_SEQ_RESP: [u8; 8] = [0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

        // 种子：A 运行 successful(15) / B successful(14) / sequence=1
        // ⇒ trial 提升非活动 B → sequence 2，cancel 再 +1 → 3。
        let sid = [9u8; 16];
        let mut ctl = eneros_boot_protocol::BootControl::factory(sid, 1, sid);
        ctl.slot_a = eneros_boot_protocol::SlotState::successful(15, 1, sid);
        ctl.slot_b = eneros_boot_protocol::SlotState::successful(14, 1, sid);
        ctl.sequence = 1;
        let rec = ctl.encode().unwrap();
        let (bc_lba, _) = eneros_ota_ab::boot_disk::lba_range_of(
            eneros_ota_ab::boot_disk::BootPartition::BootCtrl,
        );
        let bc_off = bc_lba as usize * 512;

        let mut face = InstallFace {
            ram: RamBackend::new(40000 * 512),
        };
        face.ram.data[bc_off..bc_off + BOOT_CONTROL_RECORD_SIZE]
            .copy_from_slice(&rec[..BOOT_CONTROL_RECORD_SIZE]);

        let mut svc: StorageService<RamBackend> = StorageService::new();
        svc.attach(RamBackend::new(64));
        svc.attach_install(std::boxed::Box::new(face));

        let secver = 0x0B0C_0D0E_0F10_1112u64;
        let id = [
            0xA0u8, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD,
            0xAE, 0xAF,
        ];
        let mut req = mk_req(BLK_BOOT_TRIAL);
        req.payload[1..9].copy_from_slice(&secver.to_le_bytes());
        req.payload[9..25].copy_from_slice(&id);
        req.payload_len = 25;

        let mut resp = DriverResponse::new(1, 0, 1);
        svc.handle(&req, &mut resp);
        assert_eq!(resp.status, ST_OK, "trial 必须成功（否则本锁夹具漂移）");
        assert_eq!(resp.payload_len, 8, "seq 响应定长 8B");
        assert_eq!(
            &resp.payload[..8],
            &TRIAL_SEQ_RESP,
            "BLK_BOOT_TRIAL seq 响应逐字节"
        );

        let mut resp = DriverResponse::new(1, 0, 1);
        svc.handle(&mk_req(BLK_BOOT_CANCEL), &mut resp);
        assert_eq!(resp.status, ST_OK);
        assert_eq!(resp.payload_len, 8);
        assert_eq!(
            &resp.payload[..8],
            &CANCEL_SEQ_RESP,
            "BLK_BOOT_CANCEL seq 响应逐字节"
        );
    }
}
