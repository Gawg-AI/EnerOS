//! GOOSE 加密封装（SM4-GCM 机密性 + SM3-HMAC 完整性/认证，蓝图 §4.5）。
//!
//! 本模块同时承载 GOOSE 与 SV 共用的私有安全通道 [`SecureChannel`]（D8）：
//! [`SecureGoose`] 与 `secure_sv::SecureSv` 均委托同一通道实现，
//! 公共加密/认证逻辑只维护一份，避免重复（Simplicity First）。

use alloc::vec::Vec;

use eneros_crypto::ct_eq;
use eneros_crypto::sm3::hmac::hmac_sm3;
use eneros_crypto::sm4::gcm::Sm4Gcm;

use crate::key_mgmt::SessionKey;
use crate::SecError;

/// 安全帧封装（SM4-GCM 密文 + SM3-HMAC 认证）。
#[derive(Debug, Clone, PartialEq)]
pub struct SecureFrame {
    /// 密钥标识符。
    pub key_id: u32,
    /// 12 字节初始化向量（计数器 + key_id 构造）。
    pub iv: [u8; 12],
    /// SM4-GCM 密文。
    pub ciphertext: Vec<u8>,
    /// SM4-GCM 16 字节认证标签。
    pub tag: [u8; 16],
    /// SM3-HMAC 32 字节认证码。
    pub hmac: [u8; 32],
}

/// GOOSE/SV 共用的安全通道（D8：公共逻辑抽取私有结构）。
///
/// 封装 SM4-GCM 加密、SM3-HMAC 认证与 IV 计数器，
/// 供 [`SecureGoose`] 与 `secure_sv::SecureSv` 委托使用。
pub(crate) struct SecureChannel {
    /// SM4-GCM 认证加密实例（由会话密钥 key_data 初始化）。
    cipher: Sm4Gcm,
    /// SM3-HMAC 认证密钥（256 bit）。
    mac_key: [u8; 32],
    /// 密钥标识符（随帧携带）。
    key_id: u32,
    /// IV 计数器（每次加密自增，保证 Nonce 唯一）。
    iv_counter: u64,
}

impl SecureChannel {
    /// 以会话密钥初始化安全通道。
    pub(crate) fn new(session: &SessionKey) -> Self {
        Self {
            cipher: Sm4Gcm::new(&session.key_data),
            mac_key: session.mac_key,
            key_id: session.key_id,
            iv_counter: 0,
        }
    }

    /// 生成 12 字节 IV（蓝图 §4.5：iv[0..8] = 计数器 BE，iv[8..12] = key_id BE）。
    fn generate_iv(&mut self) -> [u8; 12] {
        self.iv_counter += 1;
        let mut iv = [0u8; 12];
        iv[0..8].copy_from_slice(&self.iv_counter.to_be_bytes());
        iv[8..12].copy_from_slice(&self.key_id.to_be_bytes());
        iv
    }

    /// 加密明文 → SecureFrame。
    ///
    /// 流程（蓝图 §4.5）：生成 IV → SM4-GCM 加密 → 计算 HMAC(IV ‖ 密文 ‖ tag)。
    /// `Sm4Gcm::encrypt` 为无错接口，故本实现不会产生 `SecError::EncryptFailed`。
    pub(crate) fn encrypt(&mut self, plaintext: &[u8]) -> Result<SecureFrame, SecError> {
        let iv = self.generate_iv();
        let (ciphertext, tag) = self.cipher.encrypt(&iv, plaintext, &[]);
        let mut auth_data = Vec::with_capacity(12 + ciphertext.len() + 16);
        auth_data.extend_from_slice(&iv);
        auth_data.extend_from_slice(&ciphertext);
        auth_data.extend_from_slice(&tag);
        let mac = hmac_sm3(&self.mac_key, &auth_data);
        Ok(SecureFrame {
            key_id: self.key_id,
            iv,
            ciphertext,
            tag,
            hmac: mac,
        })
    }

    /// 解密 SecureFrame → 明文。
    ///
    /// 先以常量时间比较校验 HMAC（防时序攻击），再执行 SM4-GCM 解密。
    pub(crate) fn decrypt(&self, frame: &SecureFrame) -> Result<Vec<u8>, SecError> {
        let mut auth_data = Vec::with_capacity(12 + frame.ciphertext.len() + 16);
        auth_data.extend_from_slice(&frame.iv);
        auth_data.extend_from_slice(&frame.ciphertext);
        auth_data.extend_from_slice(&frame.tag);
        let expected = hmac_sm3(&self.mac_key, &auth_data);
        if !ct_eq(&expected, &frame.hmac) {
            return Err(SecError::HmacMismatch);
        }
        self.cipher
            .decrypt(&frame.iv, &frame.ciphertext, &[], &frame.tag)
            .map_err(|_| SecError::DecryptFailed)
    }
}

/// GOOSE 加密封装（SM4-GCM + SM3-HMAC）。
pub struct SecureGoose {
    /// 委托的公共安全通道（D8）。
    channel: SecureChannel,
}

impl SecureGoose {
    /// 以会话密钥初始化 GOOSE 安全通道。
    pub fn new(session: &SessionKey) -> Self {
        Self {
            channel: SecureChannel::new(session),
        }
    }

    /// 加密明文 → SecureFrame（SM4-GCM 加密 + SM3-HMAC 认证）。
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<SecureFrame, SecError> {
        self.channel.encrypt(plaintext)
    }

    /// 解密 SecureFrame → 明文（先校验 HMAC，再 GCM 解密）。
    pub fn decrypt(&self, frame: &SecureFrame) -> Result<Vec<u8>, SecError> {
        self.channel.decrypt(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用会话密钥（固定材料，与 key_mgmt 测试风格一致）。
    fn test_session() -> SessionKey {
        SessionKey {
            key_id: 7,
            key_data: [0x42u8; 16],
            mac_key: [0x24u8; 32],
            expiry: 9999,
        }
    }

    /// SG9: 加密后立即解密，往返结果等于原明文。
    #[test]
    fn sg9_encrypt_decrypt_roundtrip() {
        let mut sg = SecureGoose::new(&test_session());
        let plaintext = b"GOOSE PDU payload";
        let frame = sg.encrypt(plaintext).unwrap();
        let decrypted = sg.decrypt(&frame).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    /// SG10: SecureFrame 携带的 key_id 与会话密钥一致。
    #[test]
    fn sg10_frame_key_id_matches_session() {
        let mut sg = SecureGoose::new(&test_session());
        let frame = sg.encrypt(b"key id check").unwrap();
        assert_eq!(frame.key_id, 7);
    }

    /// SG11: 同一实例连续加密两次，IV 不同（计数器递增），且 iv[8..12] 为 key_id 大端。
    #[test]
    fn sg11_iv_counter_increments() {
        let mut sg = SecureGoose::new(&test_session());
        let f1 = sg.encrypt(b"first").unwrap();
        let f2 = sg.encrypt(b"second").unwrap();
        assert_ne!(f1.iv, f2.iv);
        assert_eq!(&f1.iv[8..12], &7u32.to_be_bytes());
        assert_eq!(&f2.iv[8..12], &7u32.to_be_bytes());
        let c1 = u64::from_be_bytes(f1.iv[0..8].try_into().unwrap());
        let c2 = u64::from_be_bytes(f2.iv[0..8].try_into().unwrap());
        assert_eq!(c2, c1 + 1);
    }

    /// SG12: 基线——合法帧先通过 HMAC 校验再成功解密（HMAC 先于解密校验）。
    #[test]
    fn sg12_valid_frame_decrypts_ok() {
        let mut sg = SecureGoose::new(&test_session());
        let frame = sg.encrypt(b"baseline hmac-then-decrypt").unwrap();
        assert!(sg.decrypt(&frame).is_ok());
    }

    /// SG13: 篡改密文首字节 → HmacMismatch（HMAC 先于解密校验）。
    #[test]
    fn sg13_tamper_ciphertext_detected() {
        let mut sg = SecureGoose::new(&test_session());
        let mut frame = sg.encrypt(b"tamper me").unwrap();
        frame.ciphertext[0] ^= 0xFF;
        assert_eq!(sg.decrypt(&frame), Err(SecError::HmacMismatch));
    }

    /// SG14: 篡改 GCM tag 首字节 → HmacMismatch（HMAC 覆盖 tag，先校验 HMAC）。
    #[test]
    fn sg14_tamper_tag_detected() {
        let mut sg = SecureGoose::new(&test_session());
        let mut frame = sg.encrypt(b"tamper tag").unwrap();
        frame.tag[0] ^= 0xFF;
        assert_eq!(sg.decrypt(&frame), Err(SecError::HmacMismatch));
    }

    /// SG15: 篡改 HMAC 首字节 → HmacMismatch。
    #[test]
    fn sg15_tamper_hmac_detected() {
        let mut sg = SecureGoose::new(&test_session());
        let mut frame = sg.encrypt(b"tamper hmac").unwrap();
        frame.hmac[0] ^= 0xFF;
        assert_eq!(sg.decrypt(&frame), Err(SecError::HmacMismatch));
    }

    /// SG16: 解密空密文帧（空明文加密产物）→ Ok(空)。
    #[test]
    fn sg16_decrypt_empty_ciphertext() {
        let mut sg = SecureGoose::new(&test_session());
        let frame = sg.encrypt(b"").unwrap();
        let decrypted = sg.decrypt(&frame).unwrap();
        assert!(decrypted.is_empty());
    }

    /// SG17: 加密空明文 → 密文为空；解密后仍为空。
    #[test]
    fn sg17_encrypt_empty_plaintext() {
        let mut sg = SecureGoose::new(&test_session());
        let frame = sg.encrypt(b"").unwrap();
        assert!(frame.ciphertext.is_empty());
        let decrypted = sg.decrypt(&frame).unwrap();
        assert!(decrypted.is_empty());
    }

    /// SG18: 不同会话（mac_key 不同）解密他人帧 → Err(HmacMismatch)。
    #[test]
    fn sg18_different_session_fails() {
        let mut sender = SecureGoose::new(&test_session());
        let frame = sender.encrypt(b"cross session").unwrap();

        let other_session = SessionKey {
            key_id: 7,
            key_data: [0x42u8; 16],
            mac_key: [0x99u8; 32],
            expiry: 9999,
        };
        let receiver = SecureGoose::new(&other_session);
        assert_eq!(receiver.decrypt(&frame), Err(SecError::HmacMismatch));
    }

    /// SG19: 性能 < 0.5ms/次（D11，cfg(test) Instant 断言）+ MockL2 回路风格
    /// GOOSE PDU 序列化往返（不依赖 eneros-iec61850-goose crate）。
    #[test]
    fn sg19_perf_and_goose_pdu_loopback() {
        // ---- 性能断言：256 字节载荷 × 100 次加密+解密 < 50ms ----
        let mut sg = SecureGoose::new(&test_session());
        let payload = [0xABu8; 256];
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let frame = sg.encrypt(&payload).unwrap();
            let back = sg.decrypt(&frame).unwrap();
            assert_eq!(back, payload);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 50,
            "100 次加密+解密耗时 {elapsed:?}，超过 50ms 上限（< 0.5ms/次，D11）"
        );

        // ---- MockL2 回路风格：GOOSE-ish PDU 加密 → 线格式序列化 → 解析 → 解密 ----
        let mut pdu = alloc::vec::Vec::with_capacity(128);
        pdu.extend_from_slice(&[0x61, 0x7E]); // 伪 GOOSE PDU 头（BER 风格）
        pdu.extend_from_slice(&[0x80, 0x02, 0x00, 0x64]); // 伪 stNum
        pdu.resize(128, 0x5A); // 填充至 128 字节
        let frame = sg.encrypt(&pdu).unwrap();

        // 线格式：key_id(4 BE) ‖ iv(12) ‖ ct_len(4 BE) ‖ ciphertext ‖ tag(16) ‖ hmac(32)
        let mut wire = alloc::vec::Vec::new();
        wire.extend_from_slice(&frame.key_id.to_be_bytes());
        wire.extend_from_slice(&frame.iv);
        wire.extend_from_slice(&(frame.ciphertext.len() as u32).to_be_bytes());
        wire.extend_from_slice(&frame.ciphertext);
        wire.extend_from_slice(&frame.tag);
        wire.extend_from_slice(&frame.hmac);

        // “接收侧”解析回 SecureFrame
        let key_id = u32::from_be_bytes(wire[0..4].try_into().unwrap());
        let iv: [u8; 12] = wire[4..16].try_into().unwrap();
        let ct_len = u32::from_be_bytes(wire[16..20].try_into().unwrap()) as usize;
        let ciphertext = wire[20..20 + ct_len].to_vec();
        let tag: [u8; 16] = wire[20 + ct_len..36 + ct_len].try_into().unwrap();
        let hmac: [u8; 32] = wire[36 + ct_len..68 + ct_len].try_into().unwrap();
        let parsed = SecureFrame {
            key_id,
            iv,
            ciphertext,
            tag,
            hmac,
        };

        let decrypted = sg.decrypt(&parsed).unwrap();
        assert_eq!(decrypted, pdu);
    }
}
