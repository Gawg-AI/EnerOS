//! SJA1000 CAN 控制器寄存器级驱动（v0.213.0，B1-01）.
//!
//! 实现 NXP/Philips SJA1000 独立 CAN 控制器（PeliCAN 模式）的寄存器级驱动，
//! 作为 `CanController` trait 的第一个生产实现（此前全 workspace 仅 `MockCanController`）。
//!
//! # 覆盖内容
//! - PeliCAN 模式寄存器映射（MOD/CMR/SR/BTR/ACR/AMR/OCR/CDR/IER）
//! - 标准帧（11-bit ID）与扩展帧（29-bit ID）收发编解码
//! - 波特率 → BTR0/BTR1 计算（遍历 BRP/TSEG1/TSEG2 选最小误差）
//! - 验收码/掩码（ACR/AMR 单过滤器模式）硬件过滤
//! - 总线关闭（Bus-Off）检测与恢复路径（复位重初始化）
//! - 生产 MMIO 后端 [`VolatileMmio`]（真机寄存器访问）
//!
//! # 偏差声明
//! - D10: MMIO 访问抽象为 `Sja1000Mmio` trait（地址 8-bit，对齐 SJA1000 并行接口）；
//!   真机后端 `VolatileMmio` 以 `read_volatile`/`write_volatile` 实现。
//! - D11: 时间戳由调用方注入时钟函数（`now: fn() -> u64`），SJA1000 本身无时间源。
//! - D12: 多过滤器时仅 `index == 0` 映射到硬件 ACR/AMR（单过滤器硬件限制），
//!   其余由 `CanDriver::passes_filters` 软件过滤兜底。
//! - D13: 复位后默认按 16 MHz 晶振计算波特率；`set_baud_rate` 重新计算并写入。

use core::ptr;

use crate::config::CanMode;
use crate::controller::CanController;
use crate::filter::CanFilter;
use crate::frame::{CanFrame, CanId, FrameType};

// ===== SJA1000 寄存器地址（PeliCAN 模式） =====

/// 模式寄存器（MOD）：RM/LOM/STM/AFM/SM
pub const REG_MOD: u8 = 0x00;
/// 命令寄存器（CMR）：TR/AT/RRB/CDO/SRR
pub const REG_CMR: u8 = 0x01;
/// 状态寄存器（SR）：RBS/DOS/TBS/TCS/RS/TS/ES/BS
pub const REG_SR: u8 = 0x02;
/// 中断寄存器（IR）
pub const REG_IR: u8 = 0x03;
/// 中断使能寄存器（IER）
pub const REG_IER: u8 = 0x04;
/// 总线定时 0（BTR0）：SJW/BRP
pub const REG_BTR0: u8 = 0x06;
/// 总线定时 1（BTR1）：SAM/TSEG2/TSEG1
pub const REG_BTR1: u8 = 0x07;
/// 输出控制（OCR）
pub const REG_OCR: u8 = 0x08;
/// 仲裁丢失捕获（ALC）
pub const REG_ALC: u8 = 0x0B;
/// 错误代码捕获（ECC）
pub const REG_ECC: u8 = 0x0C;
/// 接收错误计数（RXERR）
pub const REG_RXERR: u8 = 0x0E;
/// 发送错误计数（TXERR）
pub const REG_TXERR: u8 = 0x0F;
/// 验收码 0..3（ACR0..3，复位模式可写）
pub const REG_ACR0: u8 = 0x10;
pub const REG_ACR1: u8 = 0x11;
pub const REG_ACR2: u8 = 0x12;
pub const REG_ACR3: u8 = 0x13;
/// 验收掩码 0..3（AMR0..3，复位模式可写；bit=1 表示该位不关心）
pub const REG_AMR0: u8 = 0x14;
pub const REG_AMR1: u8 = 0x15;
pub const REG_AMR2: u8 = 0x16;
pub const REG_AMR3: u8 = 0x17;
/// 时钟分频（CDR）：CAN 模式选择（bit7=1 → PeliCAN）
pub const REG_CDR: u8 = 0x1F;
/// TX/RX 缓冲起始地址（13 字节：帧信息 + 4 字节 ID + 8 字节数据）
pub const REG_BUF: u8 = 0x10;

// ===== MOD 模式寄存器位 =====

/// 复位模式（1=进入复位，可写配置寄存器）
const MOD_RM: u8 = 0x01;
/// 只听模式（1=只听）
const MOD_LOM: u8 = 0x02;
/// 自测模式（1=自发自收）
const MOD_STM: u8 = 0x04;
/// 验收过滤器模式（1=单过滤器，0=双过滤器）
const MOD_AFM: u8 = 0x08;

// 以下位定义保留作完整寄存器文档（当前驱动未使用，禁止误删）。
#[allow(dead_code)]
mod mod_bits {
    pub const MOD_SM: u8 = 0x10;
}

// ===== CMR 命令寄存器位 =====

/// 发送请求
const CMR_TR: u8 = 0x01;
/// 释放接收缓冲
const CMR_RRB: u8 = 0x04;
/// 中止发送 / 清除数据溢出 / 自接收请求（文档保留）
#[allow(dead_code)]
mod cmr_bits {
    pub const CMR_AT: u8 = 0x02;
    pub const CMR_CDO: u8 = 0x08;
    pub const CMR_SRR: u8 = 0x10;
}

// ===== SR 状态寄存器位 =====

/// 接收缓冲状态（1=有数据可读）
const SR_RBS: u8 = 0x01;
/// 发送缓冲状态（1=释放可写）
const SR_TBS: u8 = 0x04;
/// 发送完成状态（1=发送完成）
#[cfg_attr(not(test), allow(dead_code))] // 测试模拟器使用
const SR_TCS: u8 = 0x08;
/// 错误状态（1=错误被动或总线关闭）
#[cfg_attr(not(test), allow(dead_code))] // 测试模拟器使用
const SR_ES: u8 = 0x40;
/// 总线状态（1=总线关闭）
const SR_BS: u8 = 0x80;
/// 数据溢出/收发中状态位（文档保留）
#[allow(dead_code)]
mod sr_bits {
    pub const SR_DOS: u8 = 0x02;
    pub const SR_RS: u8 = 0x10;
    pub const SR_TS: u8 = 0x20;
}

// ===== IER 中断使能位 =====

/// 接收中断使能
const IER_RIE: u8 = 0x01;
/// 错误中断使能
const IER_EIE: u8 = 0x04;
/// 数据溢出中断使能
const IER_DOIE: u8 = 0x08;
/// 发送中断使能（文档保留）
#[allow(dead_code)]
const IER_TIE: u8 = 0x02;

// ===== CDR 时钟分频 =====

/// PeliCAN 模式（bit7=1）+ 常规时钟
const CDR_PELICAN: u8 = 0x80;

// ===== 帧缓冲布局 =====

/// TX/RX 缓冲第 0 字节：帧信息
const BUF_INFO: u8 = 0x00;
/// 帧信息：扩展帧标志（bit7）
const INFO_FF: u8 = 0x80;
/// 帧信息：远程帧标志（bit6）
const INFO_RTR: u8 = 0x40;
/// 帧信息：DLC 掩码（bit3-0）
const INFO_DLC: u8 = 0x0F;
/// ID 起始字节（帧信息之后）
const BUF_ID: u8 = 0x01;
/// 数据起始字节（帧信息 + 4 字节 ID 之后）
const BUF_DATA: u8 = 0x05;
/// 缓冲总长（帧信息 1 + ID 4 + 数据 8）
const BUF_LEN: u8 = 0x0D;
/// RX/TX 缓冲寄存器范围终点（含）
#[cfg_attr(not(test), allow(dead_code))] // 测试模拟器使用
const REG_BUF_END: u8 = REG_BUF + BUF_LEN - 1;

/// 默认晶振频率（Hz）：SJA1000 典型 16 MHz
pub const DEFAULT_CLOCK_HZ: u32 = 16_000_000;

// ===== MMIO 抽象 =====

/// SJA1000 寄存器访问抽象（D10）。
///
/// 真机实现 [`VolatileMmio`]；测试/回环模块用内存模拟实现。
pub trait Sja1000Mmio: Send + Sync {
    /// 读 8-bit 寄存器
    fn read(&mut self, addr: u8) -> u8;
    /// 写 8-bit 寄存器
    fn write(&mut self, addr: u8, value: u8);
}

/// 真机 MMIO 后端：以 `read_volatile`/`write_volatile` 访问固定基址。
pub struct VolatileMmio {
    /// 寄存器基址（SJA1000 并行接口映射地址）
    base: *mut u8,
}

// SAFETY: `VolatileMmio` 仅持有裸指针，不拥有内存；`Send + Sync` 由调用方
// 保证基址映射的有效性与独占性（设备寄存器访问本身无数据竞争语义）。
unsafe impl Send for VolatileMmio {}
// SAFETY: 见上；对同一地址的并发 volatile 访问由驱动层串行化（&mut self）。
unsafe impl Sync for VolatileMmio {}

impl VolatileMmio {
    /// 从寄存器基址构造（基址必须为设备已映射 MMIO 地址）。
    ///
    /// # Safety
    /// `base` 必须指向有效且已映射的 SJA1000 寄存器窗口，且生命周期内地址保持有效；
    /// 调用方保证该窗口不与其它独占映射重叠。
    pub unsafe fn new(base: *mut u8) -> Self {
        Self { base }
    }
}

impl Sja1000Mmio for VolatileMmio {
    fn read(&mut self, addr: u8) -> u8 {
        // SAFETY: `base + addr` 位于调用方保证有效的 MMIO 窗口内；
        // `read_volatile` 防止编译器合并/重排设备寄存器访问。
        unsafe { ptr::read_volatile(self.base.add(addr as usize)) }
    }

    fn write(&mut self, addr: u8, value: u8) {
        // SAFETY: 同上；`write_volatile` 保证写直达设备寄存器。
        unsafe { ptr::write_volatile(self.base.add(addr as usize), value) }
    }
}

// ===== 波特率 → BTR0/BTR1 =====

/// 波特率配置结果：BTR0/BTR1 寄存器值与相对误差。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaudTiming {
    /// BTR0 寄存器值
    pub btr0: u8,
    /// BTR1 寄存器值
    pub btr1: u8,
    /// 实际波特率（Hz）
    pub actual_baud: u32,
    /// 与目标波特率的相对误差（ppm，千分之一的百万分之一）
    pub error_ppm: u32,
}

/// 计算 SJA1000 位定时寄存器（D13）。
///
/// 位时间 = (1 + TSEG1 + TSEG2) 个 tq，tq = (BRP + 1) / clock_hz。
/// 遍历 BRP(0..=63)、TSEG1(1..=16)、TSEG2(1..=8)：
/// 1. 优先最小波特率误差（error_ppm）；
/// 2. 误差相同时优先采样点最接近 87.5%（CAN 标准推荐采样点）的组合；
/// 3. SJW 固定 1 tq（BTR0 bit7-6 = 00）。
pub fn baud_to_timing(baud: u32, clock_hz: u32) -> Option<BaudTiming> {
    if baud == 0 || clock_hz == 0 {
        return None;
    }
    let mut best: Option<BaudTiming> = None;
    for brp in 0u32..=63 {
        let tq_hz = clock_hz / (brp + 1);
        for tseg1 in 1u32..=16 {
            for tseg2 in 1u32..=8 {
                let total_tq = 1 + tseg1 + tseg2;
                let actual = tq_hz / total_tq;
                if actual == 0 {
                    continue;
                }
                let err_ppm =
                    ((actual as i64 - baud as i64).unsigned_abs() * 1_000_000) / baud as i64 as u64;
                let err_ppm = err_ppm as u32;
                // 采样点 = (1 + TSEG1) / total_tq，目标 87.5%（=7/8）
                let sample_x1000 = ((1 + tseg1) * 1000 + total_tq / 2) / total_tq;
                let sample_dev = sample_x1000.abs_diff(875);
                let candidate = BaudTiming {
                    btr0: (brp as u8) & 0x3F,
                    btr1: (((tseg2 - 1) as u8) << 4) | ((tseg1 - 1) as u8),
                    actual_baud: actual,
                    error_ppm: err_ppm,
                };
                let better = match &best {
                    None => true,
                    Some(b) => {
                        // 从已选 best 的 BTR1 反推 tseg1/tseg2 重新计算采样点偏差
                        let b_tseg1 = (b.btr1 & 0x0F) as u32 + 1;
                        let b_tseg2 = ((b.btr1 >> 4) & 0x07) as u32 + 1;
                        let b_total = 1 + b_tseg1 + b_tseg2;
                        let b_sample_x1000 = ((1 + b_tseg1) * 1000 + b_total / 2) / b_total;
                        let b_dev = b_sample_x1000.abs_diff(875);
                        (candidate.error_ppm, sample_dev) < (b.error_ppm, b_dev)
                    }
                };
                if better {
                    best = Some(candidate);
                }
            }
        }
    }
    best
}

// ===== 帧编解码 =====

/// 将帧编码为 SJA1000 TX 缓冲布局（13 字节）。
///
/// 返回写入的字节数（恒为 `BUF_LEN`）。
pub fn encode_frame(frame: &CanFrame, buf: &mut [u8; 13]) -> usize {
    let mut info: u8 = frame.dlc.min(8) & INFO_DLC;
    if frame.frame_type == FrameType::Remote {
        info |= INFO_RTR;
    }
    match frame.id {
        CanId::Standard(id) => {
            // 标准帧：Byte1 = ID[10:3]，Byte2 = ID[2:0]<<5
            buf[BUF_ID as usize] = ((id >> 3) & 0xFF) as u8;
            buf[BUF_ID as usize + 1] = ((id & 0x07) << 5) as u8;
            buf[BUF_ID as usize + 2] = 0;
            buf[BUF_ID as usize + 3] = 0;
        }
        CanId::Extended(id) => {
            info |= INFO_FF;
            buf[BUF_ID as usize] = ((id >> 21) & 0xFF) as u8;
            buf[BUF_ID as usize + 1] = ((id >> 13) & 0xFF) as u8;
            buf[BUF_ID as usize + 2] = ((id >> 5) & 0xFF) as u8;
            buf[BUF_ID as usize + 3] = ((id & 0x1F) << 3) as u8;
        }
    }
    buf[BUF_INFO as usize] = info;
    let dlc = frame.dlc.min(8) as usize;
    for i in 0..dlc {
        buf[BUF_DATA as usize + i] = frame.data[i];
    }
    BUF_LEN as usize
}

/// 从 SJA1000 RX 缓冲布局解码帧（13 字节）。
///
/// 帧信息非法（DLC > 8）时返回 `None`。
pub fn decode_frame(buf: &[u8; 13]) -> Option<CanFrame> {
    let info = buf[BUF_INFO as usize];
    let dlc = (info & INFO_DLC) as usize;
    if dlc > 8 {
        return None;
    }
    let extended = info & INFO_FF != 0;
    let remote = info & INFO_RTR != 0;
    let frame_type = if remote {
        FrameType::Remote
    } else {
        FrameType::Data
    };
    let mut data = alloc::vec![0u8; dlc];
    data.copy_from_slice(&buf[BUF_DATA as usize..BUF_DATA as usize + dlc]);
    let id = if extended {
        let raw = ((buf[BUF_ID as usize] as u32) << 21)
            | ((buf[BUF_ID as usize + 1] as u32) << 13)
            | ((buf[BUF_ID as usize + 2] as u32) << 5)
            | ((buf[BUF_ID as usize + 3] as u32) >> 3);
        CanId::Extended(raw & 0x1FFF_FFFF)
    } else {
        let raw = ((buf[BUF_ID as usize] as u16) << 3) | ((buf[BUF_ID as usize + 1] as u16) >> 5);
        CanId::Standard(raw & 0x7FF)
    };
    Some(CanFrame {
        id,
        frame_type,
        data,
        dlc: dlc as u8,
    })
}

// ===== 过滤器 → ACR/AMR =====

/// 将单个 `CanFilter` 映射为 ACR0-3/AMR0-3（单过滤器模式，D12）。
///
/// AMR bit=1 表示该 ID 位不参与比较（不关心）。
pub fn filter_to_acr_amr(filter: &CanFilter) -> ([u8; 4], [u8; 4]) {
    if filter.filter_mask == 0 {
        // 接收所有：AMR 全 1
        return ([0; 4], [0xFF; 4]);
    }
    let id = filter.filter_id;
    let mask = filter.filter_mask;
    if filter.extended {
        let acr = [
            ((id >> 21) & 0xFF) as u8,
            ((id >> 13) & 0xFF) as u8,
            ((id >> 5) & 0xFF) as u8,
            ((id & 0x1F) << 3) as u8,
        ];
        let amr = [
            (!(mask >> 21) & 0xFF) as u8,
            (!(mask >> 13) & 0xFF) as u8,
            (!(mask >> 5) & 0xFF) as u8,
            (!(mask & 0x1F) << 3) as u8,
        ];
        (acr, amr)
    } else {
        let acr = [((id >> 3) & 0xFF) as u8, ((id & 0x07) << 5) as u8, 0, 0];
        let amr = [
            (!(mask >> 3) & 0xFF) as u8,
            (!(mask & 0x07) << 5) as u8,
            0xFF,
            0xFF,
        ];
        (acr, amr)
    }
}

// ===== SJA1000 控制器 =====

/// SJA1000 CAN 控制器（PeliCAN 模式）.
///
/// 实现 [`CanController`] trait 的寄存器级生产驱动（B1-01）。
/// 泛型 `M` 为 MMIO 后端（真机 [`VolatileMmio`] / 测试模拟）。
pub struct Sja1000Controller<M: Sja1000Mmio> {
    /// MMIO 后端
    mmio: M,
    /// 晶振频率（Hz）
    clock_hz: u32,
    /// 时间戳时钟源（D11）
    now: fn() -> u64,
    /// 最近一次配置的波特率
    baud: u32,
}

impl<M: Sja1000Mmio> Sja1000Controller<M> {
    /// 构造控制器。
    ///
    /// # 参数
    /// - `mmio`: 寄存器访问后端
    /// - `clock_hz`: 晶振频率（Hz）
    /// - `now`: 纳秒时间戳时钟函数（无捕获，`Send + Sync`）
    pub fn new(mmio: M, clock_hz: u32, now: fn() -> u64) -> Self {
        Self {
            mmio,
            clock_hz,
            now,
            baud: 0,
        }
    }

    /// 进入复位模式（RM=1，可写 BTR/ACR/AMR/OCR/CDR）。
    fn enter_reset(&mut self) {
        let mod_val = self.mmio.read(REG_MOD);
        self.mmio.write(REG_MOD, mod_val | MOD_RM);
    }

    /// 退出复位模式（RM=0，进入工作模式）。
    fn leave_reset(&mut self) {
        let mod_val = self.mmio.read(REG_MOD);
        self.mmio.write(REG_MOD, mod_val & !MOD_RM);
    }

    /// 写验收码/掩码（须处于复位模式）。
    fn write_acr_amr(&mut self, acr: &[u8; 4], amr: &[u8; 4]) {
        for i in 0..4 {
            self.mmio.write(REG_ACR0 + i as u8, acr[i]);
            self.mmio.write(REG_AMR0 + i as u8, amr[i]);
        }
    }

    /// 是否处于复位模式
    pub fn in_reset_mode(&mut self) -> bool {
        self.mmio.read(REG_MOD) & MOD_RM != 0
    }

    /// 访问 MMIO 后端（查询/调试寄存器）
    pub fn mmio_mut(&mut self) -> &mut M {
        &mut self.mmio
    }

    /// 读取总线状态（1=总线关闭）。
    pub fn bus_off_bit(&mut self) -> bool {
        self.mmio.read(REG_SR) & SR_BS != 0
    }

    /// 总线关闭恢复：进入复位 → 重写全部配置 → 退出复位。
    ///
    /// 返回 `Err(())` 表示恢复后仍处于总线关闭（硬件未响应）。
    #[allow(clippy::result_unit_err)] // 对齐 `CanController` trait 的 Result<(), ()> 约定
    pub fn recover_bus_off(&mut self) -> Result<(), ()> {
        self.enter_reset();
        // 重写 PeliCAN 模式与输出配置
        self.mmio.write(REG_CDR, CDR_PELICAN);
        self.mmio.write(REG_OCR, 0x1A); // 输出配置：正常推挽
                                        // 重写波特率
        if self.baud > 0 {
            self.set_baud_rate(self.baud)?;
        }
        // 清错误计数（PeliCAN 模式下 RXERR/TXERR 复位模式可读；写 0 复位）
        self.mmio.write(REG_RXERR, 0);
        self.mmio.write(REG_TXERR, 0);
        self.leave_reset();
        if self.bus_off_bit() {
            Err(())
        } else {
            Ok(())
        }
    }
}

impl<M: Sja1000Mmio> CanController for Sja1000Controller<M> {
    fn reset(&mut self) -> Result<(), ()> {
        self.enter_reset();
        // PeliCAN 模式
        self.mmio.write(REG_CDR, CDR_PELICAN);
        // 输出控制：正常输出模式（bit7-6=00 关闭时钟输出, 输出正常推挽 0x1A）
        self.mmio.write(REG_OCR, 0x1A);
        // 单过滤器模式
        let mod_val = self.mmio.read(REG_MOD);
        self.mmio.write(REG_MOD, mod_val | MOD_AFM);
        // 默认 BTR：16 MHz @ 500 kbps
        self.set_baud_rate(500_000)?;
        self.leave_reset();
        Ok(())
    }

    fn set_baud_rate(&mut self, baud: u32) -> Result<(), ()> {
        let timing = baud_to_timing(baud, self.clock_hz).ok_or(())?;
        self.enter_reset();
        self.mmio.write(REG_BTR0, timing.btr0);
        self.mmio.write(REG_BTR1, timing.btr1);
        self.leave_reset();
        self.baud = timing.actual_baud;
        Ok(())
    }

    fn set_mode(&mut self, mode: CanMode) -> Result<(), ()> {
        self.enter_reset();
        let base = MOD_AFM; // 单过滤器模式保持
        let mode_bits = match mode {
            CanMode::Normal => 0,
            CanMode::ListenOnly => MOD_LOM,
            CanMode::Loopback => MOD_STM,
        };
        self.mmio.write(REG_MOD, base | mode_bits);
        self.leave_reset();
        Ok(())
    }

    fn set_filter(&mut self, index: usize, filter: &CanFilter) -> Result<(), ()> {
        if index > 0 {
            // D12: 单过滤器硬件限制，index>0 由软件过滤兜底
            return Ok(());
        }
        self.enter_reset();
        let (acr, amr) = filter_to_acr_amr(filter);
        self.write_acr_amr(&acr, &amr);
        self.leave_reset();
        Ok(())
    }

    fn enable_rx_irq(&mut self) -> Result<(), ()> {
        let ier = self.mmio.read(REG_IER);
        self.mmio.write(REG_IER, ier | IER_RIE | IER_EIE | IER_DOIE);
        Ok(())
    }

    fn disable_rx_irq(&mut self) -> Result<(), ()> {
        let ier = self.mmio.read(REG_IER);
        self.mmio
            .write(REG_IER, ier & !(IER_RIE | IER_EIE | IER_DOIE));
        Ok(())
    }

    fn read_rx_buffer(&mut self) -> Option<CanFrame> {
        let sr = self.mmio.read(REG_SR);
        if sr & SR_RBS == 0 {
            return None;
        }
        let mut buf = [0u8; 13];
        for i in 0..BUF_LEN {
            buf[i as usize] = self.mmio.read(REG_BUF + i);
        }
        // 释放接收缓冲
        self.mmio.write(REG_CMR, CMR_RRB);
        decode_frame(&buf)
    }

    fn write_tx_buffer(&mut self, frame: &CanFrame) -> Result<(), ()> {
        let sr = self.mmio.read(REG_SR);
        if sr & SR_TBS == 0 {
            // 发送缓冲忙（前一帧未发送完成）
            return Err(());
        }
        let mut buf = [0u8; 13];
        encode_frame(frame, &mut buf);
        for i in 0..BUF_LEN {
            self.mmio.write(REG_BUF + i, buf[i as usize]);
        }
        // 请求发送
        self.mmio.write(REG_CMR, CMR_TR);
        Ok(())
    }

    fn now_ns(&self) -> u64 {
        (self.now)()
    }
}

// ===== 测试 =====

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};

    // ---- 模拟 MMIO（回环模块：寄存到内存，模拟芯片行为） ----

    /// 内存模拟 SJA1000：寄存器数组 + 帧缓冲。
    ///
    /// 模拟芯片行为：CMR.TR 写入后帧进入"总线"（回环），
    /// 随后置 TCS/TBS；回环帧进入 RX 缓冲并置 RBS。
    #[derive(Default)]
    struct SimRegs {
        regs: [u8; 0x20],
        rx_pending: Option<[u8; 13]>,
        /// 测试注入：总线关闭
        inject_bus_off: bool,
    }

    impl SimRegs {
        fn new() -> Self {
            let mut s = Self::default();
            // 初始状态：发送缓冲释放、发送完成
            s.regs[REG_SR as usize] = SR_TBS | SR_TCS;
            s
        }

        /// 计算动态状态寄存器值
        fn status(&self) -> u8 {
            let mut sr = self.regs[REG_SR as usize] & !SR_RBS;
            if self.rx_pending.is_some() {
                sr |= SR_RBS;
            }
            if self.inject_bus_off {
                sr |= SR_BS | SR_ES;
            }
            sr
        }
    }

    impl Sja1000Mmio for SimRegs {
        fn read(&mut self, addr: u8) -> u8 {
            match addr {
                REG_SR => self.status(),
                REG_BUF..=REG_BUF_END => {
                    if let Some(p) = &self.rx_pending {
                        p[(addr - REG_BUF) as usize]
                    } else {
                        0
                    }
                }
                _ => self.regs[addr as usize],
            }
        }

        fn write(&mut self, addr: u8, value: u8) {
            match addr {
                REG_CMR => {
                    // 发送请求：帧从 TX 缓冲进入总线，回环到 RX
                    if value & CMR_TR != 0 {
                        let mut buf = [0u8; 13];
                        for i in 0..BUF_LEN {
                            buf[i as usize] = self.regs[(REG_BUF + i) as usize];
                        }
                        self.rx_pending = Some(buf);
                        self.regs[REG_SR as usize] |= SR_TCS;
                    }
                    if value & CMR_RRB != 0 {
                        self.rx_pending = None;
                    }
                    self.regs[addr as usize] = value;
                }
                _ => {
                    self.regs[addr as usize] = value;
                }
            }
        }
    }

    /// 测试时钟
    static TEST_NOW_NS: AtomicU64 = AtomicU64::new(1_000_000_000);
    fn test_now() -> u64 {
        TEST_NOW_NS.load(Ordering::Relaxed)
    }

    fn make_controller() -> Sja1000Controller<SimRegs> {
        let sim = SimRegs::new();
        let mut ctrl = Sja1000Controller::new(sim, DEFAULT_CLOCK_HZ, test_now);
        ctrl.reset().expect("reset");
        ctrl
    }

    // ---- 波特率 ----

    #[test]
    fn test_baud_500k_16mhz() {
        let t = baud_to_timing(500_000, 16_000_000).expect("timing");
        // 500 kbps @ 16 MHz：BRP=1, TSEG1=13(0x0C), TSEG2=2(0x10) → BTR1=0x1C
        assert_eq!(t.btr0 & 0x3F, 1);
        assert_eq!(t.btr1 & 0x0F, 0x0C);
        assert_eq!((t.btr1 >> 4) & 0x07, 1);
        assert!(t.error_ppm < 1_000, "error {} ppm", t.error_ppm);
        assert_eq!(t.actual_baud, 500_000);
    }

    #[test]
    fn test_baud_250k_16mhz() {
        let t = baud_to_timing(250_000, 16_000_000).expect("timing");
        // 250 kbps：BRP=3, 位时间 16 tq → 实际 250 kHz
        assert_eq!(t.actual_baud, 250_000);
        assert!(t.error_ppm == 0);
    }

    #[test]
    fn test_baud_invalid() {
        assert!(baud_to_timing(0, 16_000_000).is_none());
        assert!(baud_to_timing(500_000, 0).is_none());
    }

    #[test]
    fn test_baud_1m_16mhz() {
        let t = baud_to_timing(1_000_000, 16_000_000).expect("timing");
        assert_eq!(t.actual_baud, 1_000_000);
        assert_eq!(t.error_ppm, 0);
    }

    // ---- 帧编解码 ----

    #[test]
    fn test_encode_decode_standard_roundtrip() {
        let frame = CanFrame::new_standard(0x123, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let mut buf = [0u8; 13];
        let n = encode_frame(&frame, &mut buf);
        assert_eq!(n, 13);
        let decoded = decode_frame(&buf).expect("decode");
        assert_eq!(decoded.id, CanId::Standard(0x123));
        assert_eq!(decoded.frame_type, FrameType::Data);
        assert_eq!(decoded.data, alloc::vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(decoded.dlc, 4);
    }

    #[test]
    fn test_encode_decode_extended_roundtrip() {
        let frame = CanFrame::new_extended(0x1FEDCBA9, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut buf = [0u8; 13];
        encode_frame(&frame, &mut buf);
        let decoded = decode_frame(&buf).expect("decode");
        assert_eq!(decoded.id, CanId::Extended(0x1FEDCBA9));
        assert_eq!(decoded.data, alloc::vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(decoded.dlc, 8);
    }

    #[test]
    fn test_encode_decode_remote_frame() {
        let frame = CanFrame::new_remote(CanId::Standard(0x100));
        let mut buf = [0u8; 13];
        encode_frame(&frame, &mut buf);
        let decoded = decode_frame(&buf).expect("decode");
        assert_eq!(decoded.frame_type, FrameType::Remote);
        assert_eq!(decoded.id, CanId::Standard(0x100));
        assert_eq!(decoded.dlc, 0);
    }

    #[test]
    fn test_decode_invalid_dlc_rejected() {
        let mut buf = [0u8; 13];
        buf[0] = 0x09; // DLC=9 非法
        assert!(decode_frame(&buf).is_none());
    }

    #[test]
    fn test_extended_id_boundary() {
        // 29-bit 全 1
        let frame = CanFrame::new_extended(0x1FFF_FFFF, &[]);
        let mut buf = [0u8; 13];
        encode_frame(&frame, &mut buf);
        let decoded = decode_frame(&buf).expect("decode");
        assert_eq!(decoded.id, CanId::Extended(0x1FFF_FFFF));
        // 标准帧全 1
        let frame = CanFrame::new_standard(0x7FF, &[]);
        let mut buf2 = [0u8; 13];
        encode_frame(&frame, &mut buf2);
        let decoded2 = decode_frame(&buf2).expect("decode");
        assert_eq!(decoded2.id, CanId::Standard(0x7FF));
    }

    // ---- 过滤器映射 ----

    #[test]
    fn test_filter_accept_all() {
        let f = CanFilter::accept_all();
        let (acr, amr) = filter_to_acr_amr(&f);
        assert_eq!(acr, [0; 4]);
        assert_eq!(amr, [0xFF; 4]);
    }

    #[test]
    fn test_filter_exact_standard() {
        let f = CanFilter::match_exact(0x123, false);
        let (acr, amr) = filter_to_acr_amr(&f);
        // ID 0x123 = 0b001_0010_0011：ACR0 = 0x24（ID[10:3]），ACR1 = 0x60（ID[2:0]=3<<5）
        assert_eq!(acr[0], 0x24);
        assert_eq!(acr[1], 0x60);
        // 全位参与比较
        assert_eq!(amr[0], 0x00);
        assert_eq!(amr[1], 0x00);
        assert_eq!(amr[2], 0xFF);
        assert_eq!(amr[3], 0xFF);
    }

    #[test]
    fn test_filter_exact_extended() {
        let f = CanFilter::match_exact(0x1FEDCBA9, true);
        let (acr, amr) = filter_to_acr_amr(&f);
        assert_eq!(acr[0], 0xFF); // ID[28:21]
        assert_eq!(acr[1], 0x6E); // ID[20:13]
        assert_eq!(acr[2], 0x5D); // ID[12:5]
        assert_eq!(acr[3], 0x48); // ID[4:0] = 0x09 << 3
        assert_eq!(amr, [0x00, 0x00, 0x00, 0x00]);
    }

    // ---- 控制器行为 ----

    #[test]
    fn test_reset_configures_pelican() {
        let mut ctrl = make_controller();
        assert!(!ctrl.in_reset_mode());
        // CDR 写入 PeliCAN（在复位期间）
        ctrl.enter_reset();
        assert!(ctrl.in_reset_mode());
        let cdr = ctrl.mmio_mut().read(REG_CDR);
        assert_eq!(cdr, CDR_PELICAN);
        ctrl.leave_reset();
    }

    #[test]
    fn test_set_baud_rate_writes_btr() {
        let mut ctrl = make_controller();
        ctrl.set_baud_rate(250_000).expect("baud");
        // BTR0/BTR1 已写入
        let btr0 = ctrl.mmio_mut().read(REG_BTR0);
        let btr1 = ctrl.mmio_mut().read(REG_BTR1);
        assert_eq!(btr0 & 0x3F, 3); // BRP=3
        assert_eq!(btr1, 0x1C); // TSEG2=1<<4, TSEG1=0x0C
    }

    #[test]
    fn test_send_loopback_receive_standard() {
        let mut ctrl = make_controller();
        ctrl.set_mode(CanMode::Loopback).expect("mode");
        let frame = CanFrame::new_standard(0x123, &[0x01, 0x02, 0x03]);
        ctrl.write_tx_buffer(&frame).expect("tx");
        // 回环：RX 缓冲应可读到同一帧
        let rx = ctrl.read_rx_buffer().expect("rx");
        assert_eq!(rx.id, CanId::Standard(0x123));
        assert_eq!(rx.data, alloc::vec![0x01, 0x02, 0x03]);
        // 释放后无更多帧
        assert!(ctrl.read_rx_buffer().is_none());
    }

    #[test]
    fn test_send_loopback_receive_extended() {
        let mut ctrl = make_controller();
        let frame = CanFrame::new_extended(0x1FEDCBA9, &[0xAA, 0xBB, 0xCC, 0xDD]);
        ctrl.write_tx_buffer(&frame).expect("tx");
        let rx = ctrl.read_rx_buffer().expect("rx");
        assert_eq!(rx.id, CanId::Extended(0x1FEDCBA9));
        assert_eq!(rx.data, alloc::vec![0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn test_write_tx_busy_rejected() {
        let mut ctrl = make_controller();
        // 模拟发送缓冲忙：清 TBS
        ctrl.mmio_mut().regs[REG_SR as usize] &= !SR_TBS;
        let frame = CanFrame::new_standard(0x100, &[]);
        assert!(ctrl.write_tx_buffer(&frame).is_err());
    }

    #[test]
    fn test_now_ns_from_clock() {
        let ctrl = make_controller();
        TEST_NOW_NS.store(42, Ordering::Relaxed);
        assert_eq!(ctrl.now_ns(), 42);
    }

    // ---- 总线关闭恢复 ----

    #[test]
    fn test_bus_off_detection() {
        let mut ctrl = make_controller();
        assert!(!ctrl.bus_off_bit());
        ctrl.mmio_mut().inject_bus_off = true;
        assert!(ctrl.bus_off_bit());
    }

    #[test]
    fn test_bus_off_recovery_path() {
        let mut ctrl = make_controller();
        // 进入总线关闭
        ctrl.mmio_mut().inject_bus_off = true;
        assert!(ctrl.bus_off_bit());
        // 总线恢复正常（硬件侧）
        ctrl.mmio_mut().inject_bus_off = false;
        // 恢复流程
        ctrl.recover_bus_off().expect("recover");
        assert!(!ctrl.bus_off_bit());
        // 恢复后可正常收发
        let frame = CanFrame::new_standard(0x100, &[0x55]);
        ctrl.write_tx_buffer(&frame).expect("tx after recover");
        let rx = ctrl.read_rx_buffer().expect("rx after recover");
        assert_eq!(rx.id, CanId::Standard(0x100));
        assert_eq!(rx.data, alloc::vec![0x55]);
    }

    #[test]
    fn test_bus_off_recovery_fails_when_hw_stuck() {
        let mut ctrl = make_controller();
        // 硬件一直处于总线关闭（注入不清除）
        ctrl.mmio_mut().inject_bus_off = true;
        assert!(ctrl.recover_bus_off().is_err());
    }

    // ---- CanDriver 集成（真实控制器 + 回环） ----

    #[test]
    fn test_driver_with_sja1000_loopback() {
        use crate::config::{CanConfig, CanControllerType};
        use crate::driver::CanDriver;
        use eneros_driver_framework::{DeviceDriver, DriverId};

        let sim = SimRegs::new();
        let ctrl = Sja1000Controller::new(sim, DEFAULT_CLOCK_HZ, test_now);
        let config = CanConfig {
            controller_type: CanControllerType::SJA1000,
            baud_rate: 500_000,
            ..Default::default()
        };
        let mut driver = CanDriver::new(DriverId(7), config, Box::new(ctrl));
        driver.init().expect("init");
        driver.start().expect("start");

        // 标准帧发送 → 回环接收
        let frame = CanFrame::new_standard(0x321, &[0x10, 0x20]);
        driver.send(&frame).expect("send");
        let rx = driver.recv(1_000_000_000, 100).expect("recv");
        assert_eq!(rx.id, CanId::Standard(0x321));
        assert_eq!(rx.data, alloc::vec![0x10, 0x20]);
        assert_eq!(driver.stats().tx_count, 1);
        assert_eq!(driver.stats().rx_count, 1);
    }
}
