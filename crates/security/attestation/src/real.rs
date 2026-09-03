//! attestation 真实适配器（v0.236.0，B4-03）。
//!
//! - `Tpm2Backend`（feature `tpm2`）：真实 TPM2 适配器——包装
//!   `eneros-tpm` 的 `TpmDriver`（硬件 TPM2_Create/Sign/Quote 路径），
//!   实现 `TpmBackend`；SoftTpm 保留为显式降级档（构建配置标注
//!   非生产）。
//! - [`TunnelAttestTransport`]：真实远程验证传输——经 federation
//!   tunnel 注入发送/接收闭包（复用 v0.228 设备端 TLS）。

use alloc::vec::Vec;

use crate::attest::RemoteAttestation;
#[cfg(feature = "tpm2")]
use crate::tpm::TpmBackend;
#[cfg(feature = "tpm2")]
use crate::TpmError;
use crate::{AttestError, AttestResult, AttestTransport};

// ============================================================================
// 真实 TPM2 后端（feature: tpm2）
// ============================================================================

/// 真实 TPM2 适配器（B4-03）。
///
/// 包装 `eneros-tpm::TpmDriver`（硬件 TPM2 命令路径）。PCR 度量经
/// TPM2_PCR_Extend，Quote 经 TPM2_Quote + AK 签名。
/// 构建配置：`--features tpm2` 时使用硬件 TPM；否则 SoftTpm（显式
/// 降级档，非生产）。
#[cfg(feature = "tpm2")]
pub struct Tpm2Backend<T> {
    /// 硬件 TPM 驱动。
    tpm: T,
    /// 证明密钥（AK）持久化句柄（29 SE-03：原硬编码 0 使真实 TPM2 无法
    /// 定位 AK——句柄由集成层从 TPM 持久化加载后注入）。
    ak_handle: u32,
    /// 证明密钥（AK）公钥——由集成层从 TPM 持久化句柄加载后注入
    /// （蓝图/20 R3-1：未注入 → `attestation_pubkey` 返回错误，禁占位）。
    ak_pubkey: Option<eneros_crypto::Sm2PublicKey>,
}

#[cfg(feature = "tpm2")]
impl<T> Tpm2Backend<T>
where
    T: eneros_tpm::TpmDriver,
{
    /// 创建真实 TPM2 后端（29 SE-03：AK 句柄必须注入——真实 TPM2 Quote
    /// 签名经该句柄定位 AK；AK 公钥另行经 [`Self::with_ak`] 注入）。
    pub fn new(tpm: T, ak_handle: u32) -> Self {
        Self {
            tpm,
            ak_handle,
            ak_pubkey: None,
        }
    }

    /// 注入 AK 公钥（从 TPM 持久化句柄 / EK 派生加载，集成层调用）。
    pub fn with_ak(mut self, ak_pubkey: eneros_crypto::Sm2PublicKey) -> Self {
        self.ak_pubkey = Some(ak_pubkey);
        self
    }
}

#[cfg(feature = "tpm2")]
impl<T> TpmBackend for Tpm2Backend<T>
where
    T: eneros_tpm::TpmDriver,
{
    fn pcr_extend(&mut self, pcr_idx: u8, digest: &[u8; 32]) -> Result<(), TpmError> {
        if pcr_idx >= 24 {
            return Err(TpmError::InvalidPcrIndex);
        }
        self.tpm
            .pcr_extend(pcr_idx as u32, digest)
            .map_err(|_| TpmError::ExtendFailed)
    }

    fn pcr_read(&mut self, pcr_idx: u8) -> Result<[u8; 32], TpmError> {
        if pcr_idx >= 24 {
            return Err(TpmError::InvalidPcrIndex);
        }
        // 蓝图/20 R3-1：真实 TPM2_PCR_Read（占位失败清零）。
        self.tpm
            .pcr_read(pcr_idx as u32)
            .map_err(|_| TpmError::ReadFailed)
    }

    fn quote(
        &mut self,
        pcr_indices: &[u8],
        nonce: &[u8; 20],
        now: u64,
        rng: &mut eneros_crypto::CsRng,
    ) -> Result<(crate::attest::PcrQuote, [u8; 64]), TpmError> {
        if pcr_indices.is_empty() {
            return Err(TpmError::QuoteFailed);
        }
        // 硬件 Quote：收集 PCR 值 + AK 签名 digest。
        let mut pcr_values = Vec::new();
        for &idx in pcr_indices {
            let v = self
                .tpm
                .pcr_read(idx as u32)
                .map_err(|_| TpmError::ReadFailed)?;
            pcr_values.push(v);
        }
        let quote = crate::attest::PcrQuote {
            pcr_select: pcr_indices.to_vec(),
            pcr_values,
            nonce: *nonce,
            quote_time: now,
        };
        // 签名 digest（AK 由集成层注册；此处用驱动 sign 语义）。
        let digest = crate::tpm::quote_digest(&quote);
        let sig = self
            .tpm
            .sign(self.ak_handle, &digest)
            .map_err(|_| TpmError::QuoteFailed)?;
        // 29 SE-03：TPM2 真实签名输出为 DER 编码（≤72B）——原实现截断/零
        // 填充破坏签名完整性。DER→r‖s 解析（复用 crypto-gmssl der 资产），
        // 解析失败即 Err（fail-closed，禁止截断）。
        let sig_bytes = match sig.len() {
            64 => {
                let mut out = [0u8; 64];
                out.copy_from_slice(&sig);
                out
            }
            _ => {
                let (r, s) =
                    eneros_crypto_gmssl::der::der_to_rs(&sig).map_err(|_| TpmError::QuoteFailed)?;
                let mut out = [0u8; 64];
                out[..32].copy_from_slice(&r);
                out[32..].copy_from_slice(&s);
                out
            }
        };
        // 蓝图/20 R3-1：rng 显式标注未用（TPM 硬件签名使用内部熵）。
        let _rng = rng;
        Ok((quote, sig_bytes))
    }

    fn attestation_pubkey(&self) -> Result<eneros_crypto::Sm2PublicKey, TpmError> {
        // 蓝图/20 R3-1：AK 未注入 → 明确失败（禁占位公钥）。
        self.ak_pubkey.ok_or(TpmError::KeyUnavailable)
    }
}

// ============================================================================
// 真实远程验证传输（经 federation tunnel）
// ============================================================================

/// 经 federation tunnel 的真实远程验证传输（B4-03）。
///
/// `send_fn` 将证明序列化后经 TLS tunnel 发送；`recv_fn` 接收远端
/// 判定。复用 v0.228 `TlsTunnelDevice` 接缝。
pub struct TunnelAttestTransport<S, R> {
    /// 发送闭包（证明字节 → tunnel）。
    send_fn: S,
    /// 接收闭包（tunnel → 判定字节）。
    recv_fn: R,
}

/// 隧道证明线格式版本（29 SE-04：版本字节防混读——格式变更时对端
/// 可拒绝而非误解析）。
pub const ENCODE_VERSION: u8 = 0x01;

impl<S, R> TunnelAttestTransport<S, R>
where
    S: FnMut(&[u8]) -> Result<(), AttestError>,
    R: FnMut() -> Result<Vec<u8>, AttestError>,
{
    /// 创建远程验证传输。
    pub fn new(send_fn: S, recv_fn: R) -> Self {
        Self { send_fn, recv_fn }
    }

    /// 编码证明：magic "RATT" + 格式版本 + 完整 Quote 材料 + 签名。
    pub fn encode(attest: &RemoteAttestation) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            4 + 1
                + 20
                + 1
                + attest.quote.pcr_select.len()
                + 1
                + attest.quote.pcr_values.len() * 32
                + 8
                + 64,
        );
        out.extend_from_slice(b"RATT");
        out.push(ENCODE_VERSION);
        out.extend_from_slice(&attest.quote.nonce);
        out.push(attest.quote.pcr_select.len() as u8);
        out.extend_from_slice(&attest.quote.pcr_select);
        out.push(attest.quote.pcr_values.len() as u8);
        for v in &attest.quote.pcr_values {
            out.extend_from_slice(v);
        }
        out.extend_from_slice(&attest.quote.quote_time.to_le_bytes());
        out.extend_from_slice(&attest.signature);
        out
    }

    /// 解析远端判定（29 SE-04：按对端回码映射真实 reason，不再一律
    /// PcrMismatch）。
    pub fn decode_result(resp: &[u8]) -> AttestResult {
        let trusted = resp.len() >= 3 && &resp[0..3] == b"ROK";
        let reason = if trusted {
            crate::attest::AttestReason::Verified
        } else {
            let head = &resp[..resp.len().min(24)];
            if head.starts_with(b"REJ:signature") {
                crate::attest::AttestReason::SignatureInvalid
            } else if head.starts_with(b"REJ:pcr") {
                crate::attest::AttestReason::PcrMismatch
            } else if head.starts_with(b"REJ:nonce") {
                crate::attest::AttestReason::NonceMismatch
            } else if head.starts_with(b"REJ:eventlog") {
                crate::attest::AttestReason::EventLogInconsistent
            } else {
                crate::attest::AttestReason::ServerRejected
            }
        };
        AttestResult {
            trusted,
            pcr_mismatches: Vec::new(),
            reason,
        }
    }
}

impl<S, R> AttestTransport for TunnelAttestTransport<S, R>
where
    S: FnMut(&[u8]) -> Result<(), AttestError>,
    R: FnMut() -> Result<Vec<u8>, AttestError>,
{
    fn verify_remote(&mut self, attest: &RemoteAttestation) -> Result<AttestResult, AttestError> {
        let payload = Self::encode(attest);
        (self.send_fn)(&payload)?;
        let resp = (self.recv_fn)()?;
        Ok(Self::decode_result(&resp))
    }
}

// ============================================================================
// Tests（闭包注入 + 编码/解码）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::PcrQuote;

    fn attest() -> RemoteAttestation {
        RemoteAttestation {
            quote: PcrQuote {
                pcr_select: Vec::new(),
                pcr_values: Vec::new(),
                nonce: [0x77; 20],
                quote_time: 0,
            },
            signature: [0x55; 64],
            event_log: Vec::new(),
        }
    }

    #[test]
    fn test_encode_layout() {
        let a = attest();
        let enc = TunnelAttestTransport::<
            fn(&[u8]) -> Result<(), AttestError>,
            fn() -> Result<Vec<u8>, AttestError>,
        >::encode(&a);
        assert_eq!(&enc[0..4], b"RATT");
        assert_eq!(enc[4], super::ENCODE_VERSION);
        assert_eq!(&enc[5..25], &[0x77; 20]);
        // pcr_select_len=0（空选择）→ pcr_values_n=0 → quote_time 8B → sig。
        assert_eq!(enc[25], 0);
        assert_eq!(enc[26], 0);
        let sig_off = 27 + 8;
        assert_eq!(&enc[sig_off..sig_off + 64], &[0x55; 64]);
    }

    #[test]
    fn test_encode_layout_with_pcr() {
        // 带 PCR 材料的编码：len/values/quote_time 全部落位，远端可重算。
        let a = RemoteAttestation {
            quote: PcrQuote {
                pcr_select: vec![1, 2, 3],
                pcr_values: vec![[0x11; 32], [0x22; 32], [0x33; 32]],
                nonce: [0x77; 20],
                quote_time: 0x1122334455667788,
            },
            signature: [0x55; 64],
            event_log: Vec::new(),
        };
        let enc = TunnelAttestTransport::<
            fn(&[u8]) -> Result<(), AttestError>,
            fn() -> Result<Vec<u8>, AttestError>,
        >::encode(&a);
        assert_eq!(enc[25], 3, "pcr_select_len");
        assert_eq!(&enc[26..29], &[1, 2, 3]);
        assert_eq!(enc[29], 3, "pcr_values_n");
        assert_eq!(&enc[30..30 + 32], &[0x11; 32]);
        assert_eq!(&enc[30 + 32..30 + 64], &[0x22; 32]);
        assert_eq!(&enc[30 + 64..30 + 96], &[0x33; 32]);
        let off = 30 + 96;
        assert_eq!(
            u64::from_le_bytes(enc[off..off + 8].try_into().unwrap()),
            0x1122334455667788
        );
        assert_eq!(&enc[off + 8..off + 8 + 64], &[0x55; 64]);
    }

    /// 29 SE-04 判据：encode → 解码 Quote 材料 → 重算 quote_digest →
    /// SM2 验签通过（证据链自包含，远端可离线复核）。
    #[test]
    fn test_encode_decode_requote_verify_roundtrip() {
        use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair, Sm2Signature};
        let mut rng = CsRng::from_seed(&[0x7A; 32]);
        let kp = Sm2KeyPair::generate(&mut rng).expect("kp");
        let quote = PcrQuote {
            pcr_select: vec![0, 1, 2],
            pcr_values: vec![[0xAB; 32], [0xCD; 32], [0xEF; 32]],
            nonce: [0x42; 20],
            quote_time: 1_700_000_000,
        };
        let digest = crate::tpm::quote_digest(&quote);
        let sig = sm2_sign(&digest, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        let attest = RemoteAttestation {
            quote,
            signature: sig.to_bytes(),
            event_log: Vec::new(),
        };
        let enc = TunnelAttestTransport::<
            fn(&[u8]) -> Result<(), AttestError>,
            fn() -> Result<Vec<u8>, AttestError>,
        >::encode(&attest);
        // 解码（远端侧）：跳过 magic+ver，读回 Quote 材料。
        let nonce: [u8; 20] = enc[5..25].try_into().unwrap();
        let pcr_len = enc[25] as usize;
        let pcr_select = enc[26..26 + pcr_len].to_vec();
        let values_n = enc[26 + pcr_len] as usize;
        let mut pcr_values = Vec::new();
        for i in 0..values_n {
            let off = 27 + pcr_len + i * 32;
            pcr_values.push(enc[off..off + 32].try_into().unwrap());
        }
        let qt_off = 27 + pcr_len + values_n * 32;
        let quote_time = u64::from_le_bytes(enc[qt_off..qt_off + 8].try_into().unwrap());
        let sig_off = qt_off + 8;
        let signature: [u8; 64] = enc[sig_off..sig_off + 64].try_into().unwrap();
        // 重算 digest 并验签。
        let requote = PcrQuote {
            pcr_select,
            pcr_values,
            nonce,
            quote_time,
        };
        let redigest = crate::tpm::quote_digest(&requote);
        assert_eq!(redigest, digest, "远端重算 digest 必须与签名方一致");
        let ok = eneros_signer_keyring::verify_signature(
            &redigest,
            &Sm2Signature::from_bytes(&signature),
            &kp.public_key,
        )
        .is_ok();
        assert!(ok, "远端离线验签必须通过（证据链自包含）");
    }

    /// 29 SE-03 判据：TPM2 真实签名 DER（72B）注入 → r‖s 转换后验签通过。
    /// 直接验证 DER→r‖s 转换路径与 64B 直通路径一致性。
    /// 依赖 eneros-crypto-gmssl（仅 tpm2 feature 启用）——非 tpm2 构建跳过。
    #[cfg(feature = "tpm2")]
    #[test]
    fn test_der_signature_to_rs_verify() {
        use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair, Sm2Signature};
        let mut rng = CsRng::from_seed(&[0x3C; 32]);
        let kp = Sm2KeyPair::generate(&mut rng).expect("kp");
        let digest = [0x11; 32];
        let sig = sm2_sign(&digest, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        // DER 编码（r‖s → DER：0x30 len 0x02 r 0x02 s，SIMPLEST 形式）。
        let der = der_encode(&sig.r, &sig.s);
        assert!(der.len() <= 72, "SM2 DER 签名 ≤72B");
        // 转换回 r‖s。
        let (r, s) = eneros_crypto_gmssl::der::der_to_rs(&der).expect("der_to_rs");
        let mut rs = [0u8; 64];
        rs[..32].copy_from_slice(&r);
        rs[32..].copy_from_slice(&s);
        assert_eq!(rs, sig.to_bytes());
        // 用转换后的 r‖s 验签（等价于 real.rs quote() 的 DER 分支产物）。
        let ok = eneros_signer_keyring::verify_signature(
            &digest,
            &Sm2Signature::from_bytes(&rs),
            &kp.public_key,
        )
        .is_ok();
        assert!(ok, "DER→r‖s 转换后必须可验签");
    }

    /// 最小 SM2 DER 签名编码（测试夹具：0x30 len 0x02 r 0x02 s）。
    #[allow(dead_code)] // 44.2：DER 编码辅助（44.4 冻结登记）
    fn der_encode(r: &[u8; 32], s: &[u8; 32]) -> Vec<u8> {
        let mut out = Vec::new();
        let r_trim = trim_leading_zero(r);
        let s_trim = trim_leading_zero(s);
        let total = 2 + r_trim.len() + 2 + s_trim.len();
        out.push(0x30);
        out.push(total as u8);
        out.push(0x02);
        out.push(r_trim.len() as u8);
        out.extend_from_slice(r_trim);
        out.push(0x02);
        out.push(s_trim.len() as u8);
        out.extend_from_slice(s_trim);
        out
    }

    #[allow(dead_code)] // 44.2：DER 前缀归零辅助（44.4 冻结登记）
    fn trim_leading_zero(b: &[u8; 32]) -> &[u8] {
        let mut i = 0;
        while i < b.len() - 1 && b[i] == 0 {
            i += 1;
        }
        &b[i..]
    }

    #[test]
    fn test_decode_result() {
        assert!(
            TunnelAttestTransport::<
                fn(&[u8]) -> Result<(), AttestError>,
                fn() -> Result<Vec<u8>, AttestError>,
            >::decode_result(b"ROK")
            .trusted
        );
        assert!(
            !TunnelAttestTransport::<
                fn(&[u8]) -> Result<(), AttestError>,
                fn() -> Result<Vec<u8>, AttestError>,
            >::decode_result(b"REJ:bad-pcr")
            .trusted
        );
        assert!(
            !TunnelAttestTransport::<
                fn(&[u8]) -> Result<(), AttestError>,
                fn() -> Result<Vec<u8>, AttestError>,
            >::decode_result(b"x")
            .trusted
        );
    }

    /// 29 SE-04：decode_result 按回码映射真实 reason（不再一律 PcrMismatch）。
    #[test]
    fn test_decode_result_reason_mapping() {
        use crate::attest::AttestReason;
        let cases: &[(&[u8], AttestReason)] = &[
            (b"ROK", AttestReason::Verified),
            (b"REJ:signature", AttestReason::SignatureInvalid),
            (b"REJ:pcr:12", AttestReason::PcrMismatch),
            (b"REJ:nonce", AttestReason::NonceMismatch),
            (b"REJ:eventlog", AttestReason::EventLogInconsistent),
            (b"REJ:oops", AttestReason::ServerRejected),
            (b"x", AttestReason::ServerRejected),
        ];
        for (resp, expect) in cases {
            let r = TunnelAttestTransport::<
                fn(&[u8]) -> Result<(), AttestError>,
                fn() -> Result<Vec<u8>, AttestError>,
            >::decode_result(resp);
            assert_eq!(
                r.reason,
                *expect,
                "resp={:?}",
                String::from_utf8_lossy(resp)
            );
        }
    }

    #[test]
    fn test_tunnel_verify_remote_end_to_end() {
        let mut transport = TunnelAttestTransport::new(
            |payload: &[u8]| {
                assert_eq!(&payload[0..4], b"RATT");
                Ok(())
            },
            || Ok(b"ROK".to_vec()),
        );
        let r = transport.verify_remote(&attest()).expect("verify");
        assert!(r.trusted);
    }

    #[test]
    fn test_tunnel_rejected_result() {
        let mut transport =
            TunnelAttestTransport::new(|_p: &[u8]| Ok(()), || Ok(b"REJ:nonce-mismatch".to_vec()));
        let r = transport.verify_remote(&attest()).expect("verify");
        assert!(!r.trusted);
    }

    #[test]
    fn test_tunnel_send_error_propagates() {
        let mut transport = TunnelAttestTransport::new(
            |_p: &[u8]| Err(AttestError::QuoteFailed),
            || Ok(b"ROK".to_vec()),
        );
        assert!(transport.verify_remote(&attest()).is_err());
    }
}
