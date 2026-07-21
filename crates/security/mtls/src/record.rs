//! 记录层加密通道（蓝图 §4.4）.
//!
//! # 线上记录格式
//!
//! ```text
//! seq(8B BE) ‖ ciphertext ‖ tag
//! ```
//!
//! - **SM4-GCM 套件**：nonce = `0x00000000 ‖ seq(8B)`（12B），AAD = `seq(8B)`
//!   （绑定序列号防篡改），tag = GCM 16B 认证标签
//! - **SM4-CBC 套件**：IV = `seq(8B) ‖ seq(8B)`（16B），Encrypt-then-MAC：
//!   tag = `HMAC-SM3(mac_key, seq ‖ ciphertext)`（32B）
//!
//! # 防重放
//!
//! 64 位滑动窗口（IPsec 标准语义）：记录最大已收序列号 `max_seq` 与位图
//! （bit i 对应 `max_seq - i`）。**先认证后窗口**：GCM tag / HMAC 通过 AAD
//! 绑定序列号，认证通过才更新窗口，防伪造高序号滑动窗口 DoS。重复帧与滑出
//! 窗口左界的帧 → [`TlsError::ReplayDetected`]。
//!
//! # 密钥派生（标签分离）
//!
//! 由握手产物二次派生：`enc_key = SM3(session_key ‖ "enc")` 前 16 字节；
//! `mac_key = SM3(session_key ‖ "mac")`（32 字节）。
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use alloc::vec::Vec;

use eneros_crypto::sm4::cbc::Sm4Cbc;
use eneros_crypto::sm4::gcm::Sm4Gcm;
use eneros_crypto::{ct_eq, hmac_sm3, Sm3Hasher};

use crate::cipher_suite::Cipher;
use crate::handshake::HandshakeOutcome;
use crate::TlsError;

/// 记录层加密通道（单向发送 + 单向接收状态）.
///
/// 通信双方各自持有一个 `MtlsRecord`：发送方 `seal`，接收方 `open`。
/// 双方用同一 `HandshakeOutcome` 构造即共享密钥与套件。
pub struct MtlsRecord {
    /// 协商套件中的分组加密算法.
    cipher: Cipher,
    /// 记录加密密钥（SM4，16 字节）.
    enc_key: [u8; 16],
    /// 消息认证密钥（SM3-HMAC，32 字节；GCM 套件下保留不用）.
    mac_key: [u8; 32],
    /// 发送序列号（单调递增，构造 nonce/IV/AAD）.
    send_seq: u64,
    /// 最大已接收序列号.
    max_recv_seq: u64,
    /// 防重放位图（bit i 对应 max_recv_seq - i）.
    recv_bitmap: u64,
    /// 是否已收到首帧（窗口初始化标记）.
    recv_started: bool,
    /// 累计发送记录数.
    pub records_sent: u32,
    /// 累计接收记录数.
    pub records_recv: u32,
}

impl MtlsRecord {
    /// 由握手产物构造记录层（SM3 标签分离二次派生 enc/mac 密钥）.
    pub fn new(outcome: &HandshakeOutcome) -> Self {
        let mut enc_h = Sm3Hasher::new();
        enc_h.update(&outcome.session_key);
        enc_h.update(b"enc");
        let enc_digest = enc_h.finalize();
        let mut enc_key = [0u8; 16];
        enc_key.copy_from_slice(&enc_digest[..16]);

        let mut mac_h = Sm3Hasher::new();
        mac_h.update(&outcome.session_key);
        mac_h.update(b"mac");
        let mac_key = mac_h.finalize();

        Self::from_keys(outcome.suite.cipher, enc_key, mac_key)
    }

    /// 由显式密钥构造（集成层 / 测试用）.
    pub fn from_keys(cipher: Cipher, enc_key: [u8; 16], mac_key: [u8; 32]) -> Self {
        Self {
            cipher,
            enc_key,
            mac_key,
            send_seq: 0,
            max_recv_seq: 0,
            recv_bitmap: 0,
            recv_started: false,
            records_sent: 0,
            records_recv: 0,
        }
    }

    /// 防重放窗口检查与更新（必须先通过认证再调用）.
    ///
    /// - `seq > max_recv_seq`：窗口右移，接受
    /// - `max_recv_seq - seq >= 64`：滑出窗口左界，拒绝
    /// - 位图命中：重复帧，拒绝
    fn window_check_and_update(&mut self, seq: u64) -> Result<(), TlsError> {
        if !self.recv_started {
            self.recv_started = true;
            self.max_recv_seq = seq;
            self.recv_bitmap = 1;
            return Ok(());
        }
        if seq > self.max_recv_seq {
            let shift = seq - self.max_recv_seq;
            if shift >= 64 {
                self.recv_bitmap = 1;
            } else {
                self.recv_bitmap = (self.recv_bitmap << shift) | 1;
            }
            self.max_recv_seq = seq;
            Ok(())
        } else {
            let diff = self.max_recv_seq - seq;
            if diff >= 64 {
                return Err(TlsError::ReplayDetected);
            }
            let mask = 1u64 << diff;
            if self.recv_bitmap & mask != 0 {
                return Err(TlsError::ReplayDetected);
            }
            self.recv_bitmap |= mask;
            Ok(())
        }
    }

    /// 加密一条应用记录，返回线上字节（seq ‖ ciphertext ‖ tag）.
    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let seq = self.send_seq;
        self.send_seq += 1;
        let seq_be = seq.to_be_bytes();

        let mut out = Vec::with_capacity(8 + plaintext.len() + 32);
        out.extend_from_slice(&seq_be);
        match self.cipher {
            Cipher::Sm4Gcm => {
                // nonce = 0x00000000 ‖ seq(8B)；AAD = seq（绑定序列号）
                let mut nonce = [0u8; 12];
                nonce[4..].copy_from_slice(&seq_be);
                let gcm = Sm4Gcm::new(&self.enc_key);
                let (ct, tag) = gcm.encrypt(&nonce, plaintext, &seq_be);
                out.extend_from_slice(&ct);
                out.extend_from_slice(&tag);
            }
            Cipher::Sm4Cbc => {
                // IV = seq ‖ seq（16B）；Encrypt-then-MAC
                let mut iv = [0u8; 16];
                iv[..8].copy_from_slice(&seq_be);
                iv[8..].copy_from_slice(&seq_be);
                let cbc = Sm4Cbc::new(&self.enc_key, &iv);
                let ct = cbc.encrypt(plaintext);
                let mut mac_input = Vec::with_capacity(8 + ct.len());
                mac_input.extend_from_slice(&seq_be);
                mac_input.extend_from_slice(&ct);
                let tag = hmac_sm3(&self.mac_key, &mac_input);
                out.extend_from_slice(&ct);
                out.extend_from_slice(&tag);
            }
        }
        self.records_sent += 1;
        out
    }

    /// 解密一条线上记录：tag 校验 → 防重放窗口 → 返回明文.
    pub fn open(&mut self, record: &[u8]) -> Result<Vec<u8>, TlsError> {
        if record.len() < 8 {
            return Err(TlsError::DecryptFailed);
        }
        let mut seq_be = [0u8; 8];
        seq_be.copy_from_slice(&record[..8]);
        let seq = u64::from_be_bytes(seq_be);
        let body = &record[8..];

        let plaintext = match self.cipher {
            Cipher::Sm4Gcm => {
                if body.len() < 16 {
                    return Err(TlsError::DecryptFailed);
                }
                let (ct, tag_slice) = body.split_at(body.len() - 16);
                let mut tag = [0u8; 16];
                tag.copy_from_slice(tag_slice);
                let mut nonce = [0u8; 12];
                nonce[4..].copy_from_slice(&seq_be);
                let gcm = Sm4Gcm::new(&self.enc_key);
                // tag 校验失败 → DecryptFailed（AAD 绑序列号，篡改 seq 同样失败）
                gcm.decrypt(&nonce, ct, &seq_be, &tag)
                    .map_err(|_| TlsError::DecryptFailed)?
            }
            Cipher::Sm4Cbc => {
                if body.len() < 32 + 16 || (body.len() - 32) % 16 != 0 {
                    return Err(TlsError::DecryptFailed);
                }
                let (ct, tag_slice) = body.split_at(body.len() - 32);
                let mut mac_input = Vec::with_capacity(8 + ct.len());
                mac_input.extend_from_slice(&seq_be);
                mac_input.extend_from_slice(ct);
                let expected = hmac_sm3(&self.mac_key, &mac_input);
                if !ct_eq(&expected, tag_slice) {
                    return Err(TlsError::DecryptFailed);
                }
                let mut iv = [0u8; 16];
                iv[..8].copy_from_slice(&seq_be);
                iv[8..].copy_from_slice(&seq_be);
                let cbc = Sm4Cbc::new(&self.enc_key, &iv);
                cbc.decrypt(ct).map_err(|_| TlsError::DecryptFailed)?
            }
        };

        // 先认证后窗口：认证通过才更新防重放窗口
        self.window_check_and_update(seq)?;
        self.records_recv += 1;
        Ok(plaintext)
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cipher_suite::{KeyExchange, MacAlgorithm, SmCipherSuite};

    const GCM_SUITE: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::Sm2Dhe, Cipher::Sm4Gcm, MacAlgorithm::Sm3Hmac);
    const CBC_SUITE: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::Sm2Dhe, Cipher::Sm4Cbc, MacAlgorithm::Sm3Hmac);

    fn outcome(suite: SmCipherSuite) -> HandshakeOutcome {
        HandshakeOutcome {
            session_key: [0x42; 16],
            suite,
            peer_cert_fingerprint: [0u8; 32],
        }
    }

    fn read_seq(record: &[u8]) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&record[..8]);
        u64::from_be_bytes(b)
    }

    /// REC14：加密往返 —— open(seal(pt)) == pt，且线上字节与明文完全不同；
    /// GCM 与 CBC 套件均覆盖.
    #[test]
    fn rec14_encrypt_roundtrip() {
        let pt = b"eneros mtls record payload";

        // SM4-GCM 套件
        let mut sender = MtlsRecord::new(&outcome(GCM_SUITE));
        let mut receiver = MtlsRecord::new(&outcome(GCM_SUITE));
        let wire = sender.seal(pt);
        assert_eq!(
            wire.len(),
            8 + pt.len() + 16,
            "GCM 记录 = seq + ct + 16B tag"
        );
        assert!(
            !wire.windows(pt.len()).any(|w| w == pt.as_slice()),
            "线上字节不得包含明文（抓包全加密）"
        );
        let opened = receiver.open(&wire).expect("GCM 解密应成功");
        assert_eq!(opened, pt, "GCM 解密明文应与原文一致");

        // SM4-CBC 套件
        let mut sender = MtlsRecord::new(&outcome(CBC_SUITE));
        let mut receiver = MtlsRecord::new(&outcome(CBC_SUITE));
        let wire = sender.seal(pt);
        let opened = receiver.open(&wire).expect("CBC 解密应成功");
        assert_eq!(opened, pt, "CBC 解密明文应与原文一致");

        assert_eq!(sender.records_sent, 1);
        assert_eq!(receiver.records_recv, 1);
    }

    /// REC15：篡改密文任一字节 → open 返回 DecryptFailed.
    #[test]
    fn rec15_tampered_ciphertext_rejected() {
        let mut sender = MtlsRecord::new(&outcome(GCM_SUITE));
        let mut receiver = MtlsRecord::new(&outcome(GCM_SUITE));
        let mut wire = sender.seal(b"tamper target");

        // 篡改密文字节
        wire[10] ^= 0xFF;
        assert_eq!(
            receiver.open(&wire),
            Err(TlsError::DecryptFailed),
            "篡改密文应返回 DecryptFailed"
        );

        // 篡改 tag 字节
        let mut wire2 = sender.seal(b"tamper target 2");
        let n = wire2.len();
        wire2[n - 1] ^= 0xFF;
        assert_eq!(
            receiver.open(&wire2),
            Err(TlsError::DecryptFailed),
            "篡改 tag 应返回 DecryptFailed"
        );

        // 篡改序列号（AAD 绑定，seq 改动导致 tag 校验失败）
        let mut wire3 = sender.seal(b"tamper target 3");
        wire3[7] ^= 0x01;
        assert_eq!(
            receiver.open(&wire3),
            Err(TlsError::DecryptFailed),
            "篡改序列号应返回 DecryptFailed"
        );
    }

    /// REC16：重放帧 → 拒绝（ReplayDetected）；乱序旧帧滑出窗口同样拒绝.
    #[test]
    fn rec16_replay_rejected() {
        let mut sender = MtlsRecord::new(&outcome(GCM_SUITE));
        let mut receiver = MtlsRecord::new(&outcome(GCM_SUITE));

        let wire0 = sender.seal(b"frame-0");
        let wire1 = sender.seal(b"frame-1");

        // 正常接收两帧
        assert!(receiver.open(&wire0).is_ok());
        assert!(receiver.open(&wire1).is_ok());

        // 重放第 0 帧 → ReplayDetected
        assert_eq!(
            receiver.open(&wire0),
            Err(TlsError::ReplayDetected),
            "重复帧应被防重放窗口拒绝"
        );
        // 重放第 1 帧 → ReplayDetected
        assert_eq!(
            receiver.open(&wire1),
            Err(TlsError::ReplayDetected),
            "重复最新帧同样被拒绝"
        );
        // 新帧仍可正常接收（窗口右移）
        let wire2 = sender.seal(b"frame-2");
        assert!(receiver.open(&wire2).is_ok(), "新帧应正常接收");
    }

    /// REC17：发送序列号单调递增（0, 1, 2…），nonce/AAD 随之变化.
    #[test]
    fn rec17_sequence_monotonic() {
        let mut sender = MtlsRecord::new(&outcome(GCM_SUITE));
        let w0 = sender.seal(b"a");
        let w1 = sender.seal(b"a");
        let w2 = sender.seal(b"a");
        assert_eq!(read_seq(&w0), 0, "首帧序列号应为 0");
        assert_eq!(read_seq(&w1), 1, "次帧序列号应为 1");
        assert_eq!(read_seq(&w2), 2, "第三帧序列号应为 2");
        // 相同明文不同序列号 → 密文不同（nonce 唯一性）
        assert_ne!(&w0[8..], &w1[8..], "序列号驱动 nonce，密文必须不同");
    }
}
