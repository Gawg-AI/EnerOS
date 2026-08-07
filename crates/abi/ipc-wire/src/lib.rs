//! EnerOS IPC wire 共享层（ARCH-S04 / ADR-0025）。
//!
//! Agent/驱动/协议三 SDK 跨进程消息的公共部分：
//! - [`crc32`] — 帧完整性 CRC（IEEE 802.3 0xEDB88320，表无关软实现，消除
//!   agent-sdk 与 protocol-sdk 的重复实现）；
//! - 版本发现 / ABI 常量（[`SDK_ABI_VERSION`]、[`wire_version`]、
//!   [`abi_compatible`]）——插件 manifest 与宿主握手共用；
//! - 长度边界原语（[`check_payload_len`]）。
//!
//! 各 SDK 保留自己的帧布局（agent-sdk EIPC：16 字节头含 target；protocol-sdk
//! EPRO：12 字节头无 target），但帧完整性、版本与长度规则共用本层。
//!
//! # no_std
//!
//! 纯 `core`，零分配，零外部依赖。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

/// SDK ABI 版本（插件兼容性检查，三 SDK 一致）。
pub const SDK_ABI_VERSION: u32 = 1;

/// Wire 协议版本（所有 SDK 消息帧共用）。
pub const WIRE_VERSION: u8 = 1;

/// 帧完整性 CRC 尾长（字节）。
pub const CRC_LEN: usize = 4;

/// 默认载荷长度上界（字节）。
pub const DEFAULT_MAX_PAYLOAD_LEN: usize = 1024;

/// 返回当前 wire 协议版本（版本发现，供插件与宿主握手）。
pub const fn wire_version() -> u8 {
    WIRE_VERSION
}

/// 检查插件要求的 SDK ABI 版本是否与宿主兼容（版本发现）。
///
/// 规则：major 必须完全相等（不兼容 fail-closed）；SDK 侧 minor 不得高于宿主。
/// 当前 ABI v1 无 minor 维度，兼容性 = `required == SDK_ABI_VERSION`。
pub const fn abi_compatible(required: u32) -> bool {
    required == SDK_ABI_VERSION
}

/// 校验载荷长度是否在 `max` 上界内（长度边界原语）。
pub const fn check_payload_len(len: usize, max: usize) -> bool {
    len <= max
}

/// Software CRC32（IEEE 802.3 polynomial 0xEDB88320，表无关）。
///
/// 仅用于消息完整性；认证由传输层（HMAC/AEAD）负责，非加密 MAC。
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC32 已知向量（"123456789" → 0xCBF43926，IEEE 802.3 校验值）。
    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    /// 版本发现与 ABI 兼容性。
    #[test]
    fn version_discovery() {
        assert_eq!(wire_version(), WIRE_VERSION);
        assert_eq!(SDK_ABI_VERSION, 1);
        assert!(abi_compatible(1));
        assert!(!abi_compatible(2));
        assert!(!abi_compatible(0));
    }

    /// 长度边界原语。
    #[test]
    fn length_bounds() {
        assert!(check_payload_len(0, DEFAULT_MAX_PAYLOAD_LEN));
        assert!(check_payload_len(
            DEFAULT_MAX_PAYLOAD_LEN,
            DEFAULT_MAX_PAYLOAD_LEN
        ));
        assert!(!check_payload_len(
            DEFAULT_MAX_PAYLOAD_LEN + 1,
            DEFAULT_MAX_PAYLOAD_LEN
        ));
    }
}

pub mod el0;
