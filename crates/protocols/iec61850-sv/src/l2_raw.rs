//! SV 真实 L2 传输（v0.222.0，B2-05）。
//!
//! 基于 eneros-net `NetDevice` raw 帧收发（DMA 环直通）的
//! [`RawL2Transport`]：接收时按 EtherType 0x88BA + APPID 过滤。

use alloc::vec::Vec;

use eneros_net::NetDevice;

use crate::L2Transport;
use crate::SvError;

/// SV EtherType（IEC 61850-9-2）。
pub const ETHERTYPE_SV: u16 = 0x88BA;

/// 基于网卡设备 raw 帧收发的 SV L2 传输。
pub struct RawL2Transport<D: NetDevice> {
    /// 网卡设备（DMA 环）。
    device: D,
    /// 目标 APPID 过滤（0 = 接收所有 SV 帧）。
    appid_filter: u16,
    /// 接收缓冲。
    rx_buf: Vec<u8>,
}

impl<D: NetDevice> RawL2Transport<D> {
    /// 创建 SV L2 传输（启用混杂模式接收组播 SV）。
    pub fn new(mut device: D, appid_filter: u16) -> Self {
        let mtu = device.mtu();
        device.set_promiscuous(true);
        Self {
            device,
            appid_filter,
            rx_buf: Vec::with_capacity(mtu),
        }
    }
}

impl<D: NetDevice> L2Transport for RawL2Transport<D> {
    fn send(&mut self, frame: &[u8]) -> Result<(), SvError> {
        self.device.send(frame).map_err(|_| SvError::TransportError)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, SvError> {
        loop {
            let n = self
                .device
                .recv(&mut self.rx_buf)
                .map_err(|_| SvError::TransportError)?;
            if n == 0 {
                return Ok(0);
            }
            let frame = &self.rx_buf[..n];
            if frame.len() < 14 {
                continue;
            }
            // EtherType 过滤：0x88BA。
            let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
            if ethertype != ETHERTYPE_SV {
                continue;
            }
            // APPID 过滤（帧内第 14-15 字节）。
            let appid = u16::from_be_bytes([frame[14], frame[15]]);
            if self.appid_filter != 0 && appid != self.appid_filter {
                continue;
            }
            let out_len = n.min(buf.len());
            buf[..out_len].copy_from_slice(&frame[..out_len]);
            return Ok(out_len);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ethertype_sv_constant() {
        assert_eq!(ETHERTYPE_SV, 0x88BA);
    }
}
