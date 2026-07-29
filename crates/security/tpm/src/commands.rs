//! TPM 2.0 命令集定义 (v0.159.0).
//!
//! 按照 TCG TPM 2.0 规范定义命令/响应结构。
//! 所有多字节字段使用大端序（Big-Endian）传输。
//!
//! # 参考
//! - TCG TPM 2.0 Specification, Part 2: Structures
//! - TCG TPM 2.0 Specification, Part 3: Commands

use alloc::vec::Vec;

// ============================================================
// TPM Constants
// ============================================================

/// TPM 命令标签：无会话
pub const TPM_ST_NO_SESSIONS: u16 = 0x8001;
/// TPM 命令标签：有会话
pub const TPM_ST_SESSIONS: u16 = 0x8002;

// ── TPM Command Codes (CC) ──
/// PCR Read
pub const TPM_CC_PCR_READ: u32 = 0x0000_017E;
/// PCR Extend
pub const TPM_CC_PCR_EXTEND: u32 = 0x0000_0182;
/// PCR Reset
pub const TPM_CC_PCR_RESET: u32 = 0x0000_013D;
/// Create Primary Key
pub const TPM_CC_CREATE_PRIMARY: u32 = 0x0000_0131;
/// Create Key
pub const TPM_CC_CREATE: u32 = 0x0000_0153;
/// Load Key
pub const TPM_CC_LOAD: u32 = 0x0000_0157;
/// Sign
pub const TPM_CC_SIGN: u32 = 0x0000_015D;
/// Get Random
pub const TPM_CC_GET_RANDOM: u32 = 0x0000_017B;
/// Startup
pub const TPM_CC_STARTUP: u32 = 0x0000_0144;
/// Self Test
pub const TPM_CC_SELF_TEST: u32 = 0x0000_0143;
/// Hierarchy Control
pub const TPM_CC_HIERARCHY_CONTROL: u32 = 0x0000_0121;
/// Read Public
pub const TPM_CC_READ_PUBLIC: u32 = 0x0000_0173;
/// Unseal
pub const TPM_CC_UNSEAL: u32 = 0x0000_015E;
/// Create Loaded
pub const TPM_CC_CREATE_LOADED: u32 = 0x0000_0191;

// ── TPM Algorithm IDs ──
/// SM3 哈希算法
pub const TPM_ALG_SM3_256: u16 = 0x0012;
/// SM2 签名算法
pub const TPM_ALG_SM2: u16 = 0x001B;
/// SM4 对称加密
pub const TPM_ALG_SM4: u16 = 0x0013;
/// SHA-256
pub const TPM_ALG_SHA256: u16 = 0x000B;
/// RSA
pub const TPM_ALG_RSA: u16 = 0x0001;
/// ECC
pub const TPM_ALG_ECC: u16 = 0x0023;
/// Null
pub const TPM_ALG_NULL: u16 = 0x0010;

// ── TPM Return Codes ──
/// 成功
pub const TPM_RC_SUCCESS: u32 = 0x0000_0000;
/// 初始化
pub const TPM_RC_INITIALIZE: u32 = 0x0000_0100;
/// 认证失败
pub const TPM_RC_AUTH_FAIL: u32 = 0x0000_098E;
/// PCR 已改变
pub const TPM_RC_PCR_CHANGED: u32 = 0x0000_0990;

// ── TPM Startup Types ──
/// TPM_SU_CLEAR
pub const TPM_SU_CLEAR: u16 = 0x0000;
/// TPM_SU_STATE
pub const TPM_SU_STATE: u16 = 0x0001;

// ── PCR Constants ──
/// PCR 数量（TPM 2.0 标准 24 个）
pub const PCR_COUNT: usize = 24;
/// PCR 摘要长度（SM3-256 = 32 字节）
pub const PCR_DIGEST_SIZE: usize = 32;

// ── TPM Handles ──
/// 所有者层级
pub const TPM_RH_OWNER: u32 = 0x4000_0001;
/// 背书层级
pub const TPM_RH_ENDORSEMENT: u32 = 0x4000_000B;
/// 平台层级
pub const TPM_RH_PLATFORM: u32 = 0x4000_000C;
/// 持久密钥句柄范围起始
pub const TPM_HR_PERSISTENT: u32 = 0x8100_0000;
/// 瞬态密钥句柄范围起始
pub const TPM_HR_TRANSIENT: u32 = 0x8000_0000;

// ============================================================
// Command/Response Structures
// ============================================================

/// TPM 2.0 命令头.
#[derive(Debug, Clone)]
pub struct TpmCommandHeader {
    /// 命令标签
    pub tag: u16,
    /// 命令总大小（含头部）
    pub command_size: u32,
    /// 命令码
    pub command_code: u32,
}

impl TpmCommandHeader {
    /// 序列化为大端字节.
    pub fn to_bytes(&self) -> [u8; 10] {
        let mut buf = [0u8; 10];
        buf[0..2].copy_from_slice(&self.tag.to_be_bytes());
        buf[2..6].copy_from_slice(&self.command_size.to_be_bytes());
        buf[6..10].copy_from_slice(&self.command_code.to_be_bytes());
        buf
    }
}

/// TPM 2.0 响应头.
#[derive(Debug, Clone)]
pub struct TpmResponseHeader {
    /// 响应标签
    pub tag: u16,
    /// 响应总大小
    pub response_size: u32,
    /// 响应码
    pub response_code: u32,
}

impl TpmResponseHeader {
    /// 从大端字节解析.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 10 {
            return None;
        }
        Some(Self {
            tag: u16::from_be_bytes([data[0], data[1]]),
            response_size: u32::from_be_bytes([data[2], data[3], data[4], data[5]]),
            response_code: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
        })
    }

    /// 判断响应是否成功.
    pub fn is_success(&self) -> bool {
        self.response_code == TPM_RC_SUCCESS
    }
}

/// TPM 命令构建器.
///
/// 简化 TPM 命令的序列化过程。
pub struct TpmCommandBuilder {
    tag: u16,
    command_code: u32,
    params: Vec<u8>,
}

impl TpmCommandBuilder {
    /// 创建新的命令构建器.
    pub fn new(command_code: u32) -> Self {
        Self {
            tag: TPM_ST_NO_SESSIONS,
            command_code,
            params: Vec::new(),
        }
    }

    /// 设置命令标签.
    pub fn with_tag(mut self, tag: u16) -> Self {
        self.tag = tag;
        self
    }

    /// 追加 u16 参数（大端）.
    pub fn push_u16(mut self, val: u16) -> Self {
        self.params.extend_from_slice(&val.to_be_bytes());
        self
    }

    /// 追加 u32 参数（大端）.
    pub fn push_u32(mut self, val: u32) -> Self {
        self.params.extend_from_slice(&val.to_be_bytes());
        self
    }

    /// 追加原始字节.
    pub fn push_bytes(mut self, data: &[u8]) -> Self {
        self.params.extend_from_slice(data);
        self
    }

    /// 构建完整命令字节序列.
    pub fn build(self) -> Vec<u8> {
        let total_size = 10 + self.params.len();
        let header = TpmCommandHeader {
            tag: self.tag,
            command_size: total_size as u32,
            command_code: self.command_code,
        };
        let mut cmd = Vec::with_capacity(total_size);
        cmd.extend_from_slice(&header.to_bytes());
        cmd.extend_from_slice(&self.params);
        cmd
    }
}

// ============================================================
// Command Builders (Convenience)
// ============================================================

/// 构建 TPM2_Startup 命令.
pub fn build_startup_cmd(startup_type: u16) -> Vec<u8> {
    TpmCommandBuilder::new(TPM_CC_STARTUP)
        .push_u16(startup_type)
        .build()
}

/// 构建 TPM2_SelfTest 命令.
pub fn build_self_test_cmd(full_test: bool) -> Vec<u8> {
    TpmCommandBuilder::new(TPM_CC_SELF_TEST)
        .push_bytes(if full_test { &[0x01] } else { &[0x00] })
        .build()
}

/// 构建 TPM2_PCR_Read 命令.
pub fn build_pcr_read_cmd(pcr_idx: u32, alg: u16) -> Vec<u8> {
    TpmCommandBuilder::new(TPM_CC_PCR_READ)
        .push_u32(1) // TPML_PCR_SELECTION count
        .push_u16(alg) // hash algorithm
        .push_bytes(&[3]) // sizeofSelect
        .push_bytes(&pcr_select_mask(pcr_idx)) // pcrSelect
        .build()
}

/// 构建 TPM2_PCR_Extend 命令.
pub fn build_pcr_extend_cmd(pcr_idx: u32, alg: u16, digest: &[u8]) -> Vec<u8> {
    TpmCommandBuilder::new(TPM_CC_PCR_EXTEND)
        .push_u32(pcr_idx) // PCR handle
        .push_u32(1) // digest count
        .push_u16(alg) // algorithm
        .push_bytes(digest) // digest value
        .build()
}

/// 构建 TPM2_GetRandom 命令.
pub fn build_get_random_cmd(bytes_requested: u16) -> Vec<u8> {
    TpmCommandBuilder::new(TPM_CC_GET_RANDOM)
        .push_u16(bytes_requested)
        .build()
}

/// 构建 TPM2_Sign 命令（简化版）.
pub fn build_sign_cmd(key_handle: u32, digest: &[u8], scheme_alg: u16) -> Vec<u8> {
    TpmCommandBuilder::new(TPM_CC_SIGN)
        .push_u32(key_handle)
        .push_bytes(digest)
        .push_u16(scheme_alg) // sig scheme
        .push_u16(TPM_ALG_SM3_256) // hash alg
        .build()
}

// ============================================================
// Helper Functions
// ============================================================

/// 生成 PCR 选择掩码（3 字节）.
fn pcr_select_mask(pcr_idx: u32) -> [u8; 3] {
    let mut mask = [0u8; 3];
    if pcr_idx < 24 {
        let byte_idx = (pcr_idx / 8) as usize;
        let bit_idx = pcr_idx % 8;
        mask[byte_idx] = 1 << bit_idx;
    }
    mask
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_header_serialize() {
        let header = TpmCommandHeader {
            tag: TPM_ST_NO_SESSIONS,
            command_size: 12,
            command_code: TPM_CC_PCR_READ,
        };
        let bytes = header.to_bytes();
        assert_eq!(bytes[0..2], 0x8001u16.to_be_bytes());
        assert_eq!(bytes[2..6], 12u32.to_be_bytes());
        assert_eq!(bytes[6..10], TPM_CC_PCR_READ.to_be_bytes());
    }

    #[test]
    fn test_response_header_parse() {
        let data: [u8; 10] = [0x80, 0x01, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00];
        let header = TpmResponseHeader::from_bytes(&data).unwrap();
        assert_eq!(header.tag, TPM_ST_NO_SESSIONS);
        assert_eq!(header.response_size, 10);
        assert!(header.is_success());
    }

    #[test]
    fn test_response_header_too_short() {
        let data: [u8; 5] = [0x80, 0x01, 0x00, 0x00, 0x00];
        assert!(TpmResponseHeader::from_bytes(&data).is_none());
    }

    #[test]
    fn test_pcr_select_mask() {
        // PCR 0 → byte 0, bit 0
        assert_eq!(pcr_select_mask(0), [0x01, 0x00, 0x00]);
        // PCR 7 → byte 0, bit 7
        assert_eq!(pcr_select_mask(7), [0x80, 0x00, 0x00]);
        // PCR 8 → byte 1, bit 0
        assert_eq!(pcr_select_mask(8), [0x00, 0x01, 0x00]);
        // PCR 23 → byte 2, bit 7
        assert_eq!(pcr_select_mask(23), [0x00, 0x00, 0x80]);
    }

    #[test]
    fn test_build_startup_cmd() {
        let cmd = build_startup_cmd(TPM_SU_CLEAR);
        // header (10) + startup_type (2) = 12
        assert_eq!(cmd.len(), 12);
        // 验证 command_code
        let cc = u32::from_be_bytes([cmd[6], cmd[7], cmd[8], cmd[9]]);
        assert_eq!(cc, TPM_CC_STARTUP);
    }

    #[test]
    fn test_build_pcr_read_cmd() {
        let cmd = build_pcr_read_cmd(0, TPM_ALG_SM3_256);
        assert!(cmd.len() > 10);
        let cc = u32::from_be_bytes([cmd[6], cmd[7], cmd[8], cmd[9]]);
        assert_eq!(cc, TPM_CC_PCR_READ);
    }

    #[test]
    fn test_build_pcr_extend_cmd() {
        let digest = [0xAA; 32];
        let cmd = build_pcr_extend_cmd(0, TPM_ALG_SM3_256, &digest);
        let cc = u32::from_be_bytes([cmd[6], cmd[7], cmd[8], cmd[9]]);
        assert_eq!(cc, TPM_CC_PCR_EXTEND);
    }

    #[test]
    fn test_command_builder() {
        let cmd = TpmCommandBuilder::new(TPM_CC_GET_RANDOM)
            .push_u16(32)
            .build();
        assert_eq!(cmd.len(), 12); // 10 header + 2 param
        let size = u32::from_be_bytes([cmd[2], cmd[3], cmd[4], cmd[5]]);
        assert_eq!(size, 12);
    }

    #[test]
    fn test_tpm_constants() {
        assert_eq!(PCR_COUNT, 24);
        assert_eq!(PCR_DIGEST_SIZE, 32);
        assert_eq!(TPM_ALG_SM3_256, 0x0012);
        assert_eq!(TPM_ALG_SM2, 0x001B);
    }
}
