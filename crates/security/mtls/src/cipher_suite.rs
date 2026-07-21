//! SM 密码套件定义与协商（蓝图 §4.2）.
//!
//! 套件三字段：密钥交换（SM2-DHE）+ 分组加密（SM4-GCM/SM4-CBC）+
//! 消息认证（SM3-HMAC）。协商按服务端优先顺序选出首个双方共同支持的
//! 套件；无交集返回 [`TlsError::NoCommonCipherSuite`]。
//!
//! # no_std 合规
//! 仅使用 `core::*`，不依赖 `alloc::*` 或 `std::*`。

use crate::TlsError;

/// 密钥交换算法.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyExchange {
    /// SM2 临时 Diffie-Hellman（临时密钥对逐握手生成，前向安全）.
    Sm2Dhe,
    /// SM2 曲线上的 ECDHE 语义别名（与 [`KeyExchange::Sm2Dhe`] 同曲线实现，
    /// 保留以兼容蓝图套件命名）.
    EcdheSm2,
}

/// 分组加密算法.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cipher {
    /// SM4-GCM 认证加密（AEAD，推荐）.
    Sm4Gcm,
    /// SM4-CBC 加密（配合 SM3-HMAC，Encrypt-then-MAC）.
    Sm4Cbc,
}

/// 消息认证算法.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacAlgorithm {
    /// SM3-HMAC（HMAC 结构 + SM3 杂凑，输出 32 字节）.
    Sm3Hmac,
    /// 无独立 MAC（仅 GCM 套件允许：认证由 GCM tag 承担）.
    None,
}

/// SM 密码套件（三字段组合）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmCipherSuite {
    /// 密钥交换算法.
    pub key_exchange: KeyExchange,
    /// 分组加密算法.
    pub cipher: Cipher,
    /// 消息认证算法.
    pub mac: MacAlgorithm,
}

impl SmCipherSuite {
    /// 构造密码套件.
    pub const fn new(key_exchange: KeyExchange, cipher: Cipher, mac: MacAlgorithm) -> Self {
        Self {
            key_exchange,
            cipher,
            mac,
        }
    }

    /// 推荐默认套件：SM2-DHE + SM4-GCM + SM3-HMAC.
    pub const fn default_suite() -> Self {
        Self::new(KeyExchange::Sm2Dhe, Cipher::Sm4Gcm, MacAlgorithm::Sm3Hmac)
    }

    /// 编码为 3 字节线上格式（key_exchange ‖ cipher ‖ mac）.
    pub fn to_bytes(&self) -> [u8; 3] {
        let kx = match self.key_exchange {
            KeyExchange::Sm2Dhe => 0x01,
            KeyExchange::EcdheSm2 => 0x02,
        };
        let cipher = match self.cipher {
            Cipher::Sm4Gcm => 0x01,
            Cipher::Sm4Cbc => 0x02,
        };
        let mac = match self.mac {
            MacAlgorithm::Sm3Hmac => 0x01,
            MacAlgorithm::None => 0x00,
        };
        [kx, cipher, mac]
    }

    /// 从 3 字节线上格式解码；未知编码返回 `None`.
    pub fn from_bytes(bytes: [u8; 3]) -> Option<Self> {
        let key_exchange = match bytes[0] {
            0x01 => KeyExchange::Sm2Dhe,
            0x02 => KeyExchange::EcdheSm2,
            _ => return None,
        };
        let cipher = match bytes[1] {
            0x01 => Cipher::Sm4Gcm,
            0x02 => Cipher::Sm4Cbc,
            _ => return None,
        };
        let mac = match bytes[2] {
            0x01 => MacAlgorithm::Sm3Hmac,
            0x00 => MacAlgorithm::None,
            _ => return None,
        };
        Some(Self {
            key_exchange,
            cipher,
            mac,
        })
    }
}

/// 密码套件协商：按服务端优先顺序选出首个双方共同支持的套件.
///
/// # 参数
/// - `client`：客户端 hello 中声明的套件列表（客户端优先序）
/// - `server`：服务端本地配置的套件列表（服务端优先序）
///
/// # 返回
/// - `Ok(suite)`：服务端列表中第一个同时出现在客户端列表中的套件
/// - `Err(TlsError::NoCommonCipherSuite)`：双方列表无交集或任一为空
pub fn negotiate(
    client: &[SmCipherSuite],
    server: &[SmCipherSuite],
) -> Result<SmCipherSuite, TlsError> {
    for s in server {
        if client.contains(s) {
            return Ok(*s);
        }
    }
    Err(TlsError::NoCommonCipherSuite)
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const GCM_HMAC: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::Sm2Dhe, Cipher::Sm4Gcm, MacAlgorithm::Sm3Hmac);
    const CBC_HMAC: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::Sm2Dhe, Cipher::Sm4Cbc, MacAlgorithm::Sm3Hmac);
    const GCM_NONE: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::EcdheSm2, Cipher::Sm4Gcm, MacAlgorithm::None);

    /// SUITE1：协商成功 —— 客户端 [GCM, CBC]，服务端 [CBC] → 交集 CBC.
    #[test]
    fn suite1_negotiate_success() {
        let client = [GCM_HMAC, CBC_HMAC];
        let server = [CBC_HMAC];
        let suite = negotiate(&client, &server).expect("应协商成功");
        assert_eq!(suite, CBC_HMAC, "应选中双方共同支持的 CBC 套件");
    }

    /// SUITE2：无交集 → NoCommonCipherSuite；空列表同样无交集.
    #[test]
    fn suite2_no_common_suite() {
        let client = [GCM_HMAC];
        let server = [CBC_HMAC];
        assert_eq!(
            negotiate(&client, &server),
            Err(TlsError::NoCommonCipherSuite),
            "无交集应返回 NoCommonCipherSuite"
        );
        assert_eq!(
            negotiate(&[], &server),
            Err(TlsError::NoCommonCipherSuite),
            "客户端空列表应返回 NoCommonCipherSuite"
        );
        assert_eq!(
            negotiate(&client, &[]),
            Err(TlsError::NoCommonCipherSuite),
            "服务端空列表应返回 NoCommonCipherSuite"
        );
    }

    /// SUITE3：服务端优先 —— 双方交集含多个套件时，选服务端列表中靠前项.
    #[test]
    fn suite3_server_priority() {
        // 客户端优先 GCM，但服务端优先 CBC → 应选 CBC（服务端优先序）
        let client = [GCM_HMAC, CBC_HMAC, GCM_NONE];
        let server = [CBC_HMAC, GCM_HMAC];
        let suite = negotiate(&client, &server).expect("应协商成功");
        assert_eq!(suite, CBC_HMAC, "服务端优先序靠前项应被选中");

        // 反转服务端顺序 → 应选 GCM
        let server2 = [GCM_HMAC, CBC_HMAC];
        let suite2 = negotiate(&client, &server2).expect("应协商成功");
        assert_eq!(suite2, GCM_HMAC, "服务端优先序变化应影响结果");
    }

    /// 编解码往返（附加工件测试，保障线上格式稳定）.
    #[test]
    fn suite_codec_roundtrip() {
        for suite in &[GCM_HMAC, CBC_HMAC, GCM_NONE] {
            let bytes = suite.to_bytes();
            let decoded = SmCipherSuite::from_bytes(bytes).expect("编码应可解码");
            assert_eq!(&decoded, suite, "编解码往返应一致");
        }
        assert_eq!(
            SmCipherSuite::from_bytes([0xFF, 0x01, 0x01]),
            None,
            "未知密钥交换编码应拒绝"
        );
        assert_eq!(
            SmCipherSuite::from_bytes([0x01, 0xFF, 0x01]),
            None,
            "未知加密编码应拒绝"
        );
        assert_eq!(
            SmCipherSuite::from_bytes([0x01, 0x01, 0xFF]),
            None,
            "未知 MAC 编码应拒绝"
        );
    }
}
