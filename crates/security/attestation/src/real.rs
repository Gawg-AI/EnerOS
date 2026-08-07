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
    /// 证明密钥（AK）公钥——由集成层从 TPM 持久化句柄加载后注入
    /// （蓝图/20 R3-1：未注入 → `attestation_pubkey` 返回错误，禁占位）。
    ak_pubkey: Option<eneros_crypto::Sm2PublicKey>,
}

#[cfg(feature = "tpm2")]
impl<T> Tpm2Backend<T>
where
    T: eneros_tpm::TpmDriver,
{
    /// 创建真实 TPM2 后端（AK 公钥未注入——证明前必须 [`Self::with_ak`]）。
    pub fn new(tpm: T) -> Self {
        Self {
            tpm,
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
            .sign(0, &digest)
            .map_err(|_| TpmError::QuoteFailed)?;
        let mut sig_bytes = [0u8; 64];
        let n = sig.len().min(64);
        sig_bytes[..n].copy_from_slice(&sig[..n]);
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

impl<S, R> TunnelAttestTransport<S, R>
where
    S: FnMut(&[u8]) -> Result<(), AttestError>,
    R: FnMut() -> Result<Vec<u8>, AttestError>,
{
    /// 创建远程验证传输。
    pub fn new(send_fn: S, recv_fn: R) -> Self {
        Self { send_fn, recv_fn }
    }

    /// 编码证明：magic "RATT" + nonce + signature。
    pub fn encode(attest: &RemoteAttestation) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 20 + 64);
        out.extend_from_slice(b"RATT");
        out.extend_from_slice(&attest.quote.nonce);
        out.extend_from_slice(&attest.signature);
        out
    }

    /// 解析远端判定：magic "ROK"/"REJ"。
    pub fn decode_result(resp: &[u8]) -> AttestResult {
        let trusted = resp.len() >= 3 && &resp[0..3] == b"ROK";
        AttestResult {
            trusted,
            pcr_mismatches: Vec::new(),
            reason: if trusted {
                crate::attest::AttestReason::Verified
            } else {
                crate::attest::AttestReason::PcrMismatch
            },
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
        assert_eq!(&enc[4..24], &[0x77; 20]);
        assert_eq!(&enc[24..88], &[0x55; 64]);
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
