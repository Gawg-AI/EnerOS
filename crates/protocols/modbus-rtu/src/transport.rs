//! 21-B B1：真实 RS485 传输（`Pl011UartHw` MMIO 路径）。
//!
//! `Rs485RtuTransport` 是 `RtuTransport` trait 的生产实现：基于
//! `UartHw`（真 PL011 适配器 `Pl011UartHw` 或 host 仿真），配合真实
//! 帧层 `RtuFramer`（3.5 字符间隔成帧）完成 Modbus-RTU 请求-响应。
//! 与 `MockRtuTransport` 同 trait、不同类型（mock 仅 `#[cfg(test)]`）。

use alloc::vec::Vec;

use eneros_driver_framework::DriverError;
use eneros_rs485::uart_hw::UartHw;

use crate::framer::RtuFramer;
use crate::master::RtuTransport;

/// RX 缓冲上限（RTU 最长帧 256B + 余量）。
const RX_MAX: usize = 256;

/// 基于 `UartHw`（真 PL011 MMIO 或 host 仿真）的 RTU 传输。
pub struct Rs485RtuTransport<H: UartHw> {
    /// 底层 UART 硬件适配器（真 PL011 寄存器路径）。
    uart: H,
    /// 帧层：3.5 字符间隔成帧（真实 RTU 帧边界）。
    framer: RtuFramer,
}

impl<H: UartHw> Rs485RtuTransport<H> {
    /// 创建 RTU 传输。
    ///
    /// `baud` 用于帧间隔计算（3.5 字符时间）；`uart` 需已配置
    /// （`configure(baud, 8, One, None)` 由调用方完成——B1 强约束：
    /// 生产实现不隐藏硬件配置）。
    pub fn new(uart: H, baud: u32) -> Self {
        Self {
            uart,
            framer: RtuFramer::new(baud),
        }
    }
}

impl<H: UartHw> Rs485RtuTransport<H> {
    /// 当前单调时间（毫秒）——测试/诊断探针（经 uart 时钟）。
    pub fn now_ms_probe(&self) -> u64 {
        self.uart.now_ns() / 1_000_000
    }
}

impl<H: UartHw> RtuTransport for Rs485RtuTransport<H> {
    fn send(&mut self, data: &[u8]) -> Result<(), DriverError> {
        // 真 PL011 路径：逐字节写 DR（TX FIFO）+ 等待发送完成（BUSY 清除）。
        let written = self.uart.write_bytes(data)?;
        if written != data.len() {
            return Err(DriverError::InvalidState);
        }
        self.uart.wait_tx_done(1000)?;
        Ok(())
    }

    fn recv(&mut self, timeout_ms: u32) -> Result<Vec<u8>, DriverError> {
        // 真实超时：单调时钟 deadline（有界轮询，禁止无界循环）。
        let deadline = self
            .uart
            .now_ns()
            .saturating_add(timeout_ms as u64 * 1_000_000);
        loop {
            if let Some(byte) = self.uart.read_byte() {
                match self.framer.feed_byte(byte, self.uart.now_ns()) {
                    Ok(Some(frame)) => {
                        // 完整帧（前帧间隔超时结束）——帧内可能含多余
                        // 半帧（对端发送的中间状态），按帧边界返回。
                        let frame = frame.to_vec();
                        if frame.len() <= RX_MAX {
                            return Ok(frame);
                        }
                        // 超长帧丢弃，继续接收（framer 已丢弃）。
                        continue;
                    }
                    Ok(None) => continue,
                    Err(()) => continue, // 溢出丢弃信号，继续接收。
                }
            }
            if self.uart.now_ns() >= deadline {
                // 超时兜底：flush 半帧（对端结束传输但未达 3.5 字符间隔）。
                if let Some(frame) = self.framer.flush() {
                    return Ok(frame.to_vec());
                }
                return Err(DriverError::Timeout);
            }
        }
    }
}
