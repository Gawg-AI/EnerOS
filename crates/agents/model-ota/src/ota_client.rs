//! OTA 客户端与 manifest 二进制编解码（v0.111.0）.
//!
//! - [`ModelInfo`] / [`ModelSignature`] / [`SigAlgorithm`] — 模型清单与签名元数据
//!   （删除蓝图 `signer_cert` 字段，信任锚为构造注入的 `trusted_pubkey`，D11）
//! - [`encode_manifest`] / [`decode_manifest`] — 自定义二进制帧（magic 0x0A70 +
//!   version 1，全小端 TLV，零 serde 依赖，D5）
//! - [`OtaClient`] — 检查更新 + 断点续传下载 + 验签 + `update_once`/`rollback_once`
//!   编排（D4/D11）
//! - [`OtaStats`] — 更新状态统计（§9 可观测，D12）

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::Sm2PublicKey;

use crate::model_loader::HotLoader;
use crate::signature::verify_model_signature;
use crate::{OtaError, OtaTransport, OtaUpdateOutcome};

/// manifest 帧魔数（小端，D5）.
const MANIFEST_MAGIC: u16 = 0x0A70;
/// manifest 帧格式版本（D5）.
const MANIFEST_VERSION: u8 = 1;

/// 签名算法（D6：RsaSha256 仅占位不可验证）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SigAlgorithm {
    /// SM2 签名 + SM3 哈希（国密，本版唯一可验证算法）.
    Sm2Sm3,
    /// RSA + SHA-256（蓝图占位变体；eneros-crypto 纯国密无 RSA，不可验证）.
    RsaSha256,
}

/// 模型签名元数据（D11：删除蓝图 `signer_cert` 字段）.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSignature {
    /// 签名算法.
    pub algorithm: SigAlgorithm,
    /// 签名值（SM2 为 64 字节 r‖s 大端序）.
    pub signature: Vec<u8>,
    /// 云端签名时间戳（毫秒）.
    pub timestamp: u64,
}

/// 模型清单（云端下发的最新模型描述）.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    /// 模型标识.
    pub model_id: String,
    /// 版本号（语义化版本字符串）.
    pub version: String,
    /// 模型字节 SM3 哈希.
    pub hash: [u8; 32],
    /// 模型字节总大小.
    pub size: u64,
    /// 签名元数据.
    pub signature: ModelSignature,
    /// 云端训练完成时间戳（毫秒）.
    pub created_at: u64,
    /// 模型能力标签（如 "infer"/"solver"）.
    pub capabilities: Vec<String>,
}

/// 将模型清单编码为二进制 manifest 帧（全小端，D5）.
///
/// 帧布局：`[magic u16][version u8][model_id_len u8 + model_id][version_len u8 +
/// version][hash 32B][size u64][sig_algo u8][sig_len u16 + signature]
/// [sig_timestamp u64][created_at u64][cap_count u8 + 每 cap（len u8 + bytes）]`
pub fn encode_manifest(info: &ModelInfo) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MANIFEST_MAGIC.to_le_bytes());
    out.push(MANIFEST_VERSION);
    out.push(info.model_id.len() as u8);
    out.extend_from_slice(info.model_id.as_bytes());
    out.push(info.version.len() as u8);
    out.extend_from_slice(info.version.as_bytes());
    out.extend_from_slice(&info.hash);
    out.extend_from_slice(&info.size.to_le_bytes());
    out.push(match info.signature.algorithm {
        SigAlgorithm::Sm2Sm3 => 0u8,
        SigAlgorithm::RsaSha256 => 1u8,
    });
    out.extend_from_slice(&(info.signature.signature.len() as u16).to_le_bytes());
    out.extend_from_slice(&info.signature.signature);
    out.extend_from_slice(&info.signature.timestamp.to_le_bytes());
    out.extend_from_slice(&info.created_at.to_le_bytes());
    out.push(info.capabilities.len() as u8);
    for cap in &info.capabilities {
        out.push(cap.len() as u8);
        out.extend_from_slice(cap.as_bytes());
    }
    out
}

/// 从二进制 manifest 帧解码模型清单（D5）.
///
/// magic 错误 / 版本不符 / 截断 / 字段越界 / 非法算法码 / 非法 UTF-8 →
/// `Err(InvalidManifest)`。
pub fn decode_manifest(data: &[u8]) -> Result<ModelInfo, OtaError> {
    let mut pos = 0usize;
    let magic = read_u16(data, &mut pos)?;
    if magic != MANIFEST_MAGIC {
        return Err(OtaError::InvalidManifest);
    }
    let version = read_u8(data, &mut pos)?;
    if version != MANIFEST_VERSION {
        return Err(OtaError::InvalidManifest);
    }
    let model_id = read_str(data, &mut pos)?;
    let model_version = read_str(data, &mut pos)?;
    let hash = read_hash(data, &mut pos)?;
    let size = read_u64(data, &mut pos)?;
    let algorithm = match read_u8(data, &mut pos)? {
        0 => SigAlgorithm::Sm2Sm3,
        1 => SigAlgorithm::RsaSha256,
        _ => return Err(OtaError::InvalidManifest),
    };
    let sig_len = read_u16(data, &mut pos)? as usize;
    let signature = read_bytes(data, &mut pos, sig_len)?.to_vec();
    let sig_timestamp = read_u64(data, &mut pos)?;
    let created_at = read_u64(data, &mut pos)?;
    let cap_count = read_u8(data, &mut pos)?;
    let mut capabilities = Vec::with_capacity(cap_count as usize);
    for _ in 0..cap_count {
        capabilities.push(read_str(data, &mut pos)?);
    }
    Ok(ModelInfo {
        model_id,
        version: model_version,
        hash,
        size,
        signature: ModelSignature {
            algorithm,
            signature,
            timestamp: sig_timestamp,
        },
        created_at,
        capabilities,
    })
}

/// 读取 1 字节，越界 → `Err(InvalidManifest)`.
fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8, OtaError> {
    let b = *data.get(*pos).ok_or(OtaError::InvalidManifest)?;
    *pos += 1;
    Ok(b)
}

/// 读取 `len` 字节切片，越界/溢出 → `Err(InvalidManifest)`.
fn read_bytes<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], OtaError> {
    let end = pos.checked_add(len).ok_or(OtaError::InvalidManifest)?;
    let slice = data.get(*pos..end).ok_or(OtaError::InvalidManifest)?;
    *pos = end;
    Ok(slice)
}

/// 读取 u16（小端）.
fn read_u16(data: &[u8], pos: &mut usize) -> Result<u16, OtaError> {
    let b = read_bytes(data, pos, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

/// 读取 u64（小端）.
fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64, OtaError> {
    let b = read_bytes(data, pos, 8)?;
    let mut arr = [0u8; 8];
    arr.copy_from_slice(b);
    Ok(u64::from_le_bytes(arr))
}

/// 读取 32 字节哈希.
fn read_hash(data: &[u8], pos: &mut usize) -> Result<[u8; 32], OtaError> {
    let b = read_bytes(data, pos, 32)?;
    let mut arr = [0u8; 32];
    arr.copy_from_slice(b);
    Ok(arr)
}

/// 读取 `len u8 + bytes` 形式的 UTF-8 字符串.
fn read_str(data: &[u8], pos: &mut usize) -> Result<String, OtaError> {
    let len = read_u8(data, pos)? as usize;
    let bytes = read_bytes(data, pos, len)?;
    let s = core::str::from_utf8(bytes).map_err(|_| OtaError::InvalidManifest)?;
    Ok(String::from(s))
}

/// OTA 更新状态统计（§9 可观测，D12）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OtaStats {
    /// 累计成功更新次数.
    pub total_updates: u64,
    /// 累计被拒绝次数（验签失败 / 不在白名单）.
    pub total_rejected: u64,
    /// 累计回滚次数.
    pub total_rollbacks: u64,
    /// 最近一次更新/回滚时间戳（毫秒）.
    pub last_update_at: u64,
}

/// OTA 客户端（D4/D11：信任锚与重试上限构造注入，字段私有）.
pub struct OtaClient {
    /// 当前在运行模型的清单.
    current_model: ModelInfo,
    /// 云端签名信任锚（构造注入，D11）.
    trusted_pubkey: Sm2PublicKey,
    /// 下载失败最大重试次数.
    max_retries: u32,
    /// 更新状态统计.
    stats: OtaStats,
}

impl OtaClient {
    /// 构造 OTA 客户端.
    pub fn new(current_model: ModelInfo, trusted_pubkey: Sm2PublicKey, max_retries: u32) -> Self {
        Self {
            current_model,
            trusted_pubkey,
            max_retries,
            stats: OtaStats {
                total_updates: 0,
                total_rejected: 0,
                total_rollbacks: 0,
                last_update_at: 0,
            },
        }
    }

    /// 检查云端是否有新版本（蓝图 §4.5：无更新/同版本 → `Ok(None)`）.
    pub fn check_update<T: OtaTransport>(
        &self,
        transport: &mut T,
    ) -> Result<Option<ModelInfo>, OtaError> {
        match transport.fetch_latest(&self.current_model.version)? {
            None => Ok(None),
            Some(info) if info.version == self.current_model.version => Ok(None),
            Some(info) => Ok(Some(info)),
        }
    }

    /// 断点续传下载完整模型字节（蓝图 §4.4/§6.5，D4）.
    ///
    /// - `info.size == 0` → `Err(InvalidConfig)`
    /// - 每次 `download_range(model_id, 已下载长度, 剩余长度)`；失败立即重试
    ///   （退避由传输实现层自持），连续失败 > max_retries → `Err(DownloadFailed)`
    /// - 成功但返回空 chunk → `Err(DownloadFailed)`（防蓝图死循环 bug）
    /// - 完成后字节数 != size → `Err(SizeMismatch)`
    pub fn download_model<T: OtaTransport>(
        &self,
        transport: &mut T,
        info: &ModelInfo,
    ) -> Result<Vec<u8>, OtaError> {
        if info.size == 0 {
            return Err(OtaError::InvalidConfig);
        }
        let mut data: Vec<u8> = Vec::new();
        let mut retries: u32 = 0;
        while (data.len() as u64) < info.size {
            let offset = data.len() as u64;
            let remaining = info.size - offset;
            match transport.download_range(&info.model_id, offset, remaining) {
                Ok(chunk) => {
                    if chunk.is_empty() {
                        return Err(OtaError::DownloadFailed);
                    }
                    data.extend_from_slice(&chunk);
                }
                Err(_) => {
                    retries += 1;
                    if retries > self.max_retries {
                        return Err(OtaError::DownloadFailed);
                    }
                }
            }
        }
        if data.len() as u64 != info.size {
            return Err(OtaError::SizeMismatch);
        }
        Ok(data)
    }

    /// 验证模型（委托 `verify_model_signature`：算法门 → SM3 哈希 → SM2 验签，D12）.
    pub fn verify_model(&self, data: &[u8], info: &ModelInfo) -> Result<(), OtaError> {
        verify_model_signature(data, info, &self.trusted_pubkey)
    }

    /// 端到端单轮更新编排（蓝图 §4.3 流程）.
    ///
    /// check_update → None ⇒ `Ok(NoUpdate)` → download_model → verify_model 失败 ⇒
    /// `total_rejected += 1` 原样返回 `Err`（§4.4 拒绝 + 安全告警）→ load_new
    /// （NotInWhitelist 同样 total_rejected+1）→ swap → current_model 更新 +
    /// `total_updates += 1` + `last_update_at = now` → `Ok(Updated)`。
    pub fn update_once<T: OtaTransport>(
        &mut self,
        transport: &mut T,
        loader: &mut HotLoader,
        now: u64,
    ) -> Result<OtaUpdateOutcome, OtaError> {
        let info = match self.check_update(transport)? {
            None => return Ok(OtaUpdateOutcome::NoUpdate),
            Some(info) => info,
        };
        let data = self.download_model(transport, &info)?;
        if let Err(e) = self.verify_model(&data, &info) {
            self.stats.total_rejected += 1;
            return Err(e);
        }
        if let Err(e) = loader.load_new(&data, &info, now) {
            self.stats.total_rejected += 1;
            return Err(e);
        }
        loader.swap()?;
        self.current_model = info;
        self.stats.total_updates += 1;
        self.stats.last_update_at = now;
        Ok(OtaUpdateOutcome::Updated)
    }

    /// 回滚到上一版本（蓝图 §6.4）：loader 回滚成功后同步 current_model 与统计.
    pub fn rollback_once(&mut self, loader: &mut HotLoader, now: u64) -> Result<(), OtaError> {
        loader.rollback()?;
        self.current_model = loader.current().info.clone();
        self.stats.total_rollbacks += 1;
        self.stats.last_update_at = now;
        Ok(())
    }

    /// 当前在运行模型的清单.
    pub fn current_model(&self) -> &ModelInfo {
        &self.current_model
    }

    /// 更新状态统计.
    pub fn stats(&self) -> OtaStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};

    use super::*;
    use crate::MockOtaTransport;

    /// 构造无真实签名的清单（manifest/下载测试用）.
    fn make_info(model_id: &str, version: &str, data: &[u8]) -> ModelInfo {
        ModelInfo {
            model_id: model_id.to_string(),
            version: version.to_string(),
            hash: sm3_hash(data),
            size: data.len() as u64,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: vec![0xAB; 64],
                timestamp: 123_456,
            },
            created_at: 654_321,
            capabilities: vec!["infer".to_string(), "solver".to_string()],
        }
    }

    /// 构造真实 SM2 签名的清单（验签测试用）.
    fn make_signed_info(
        model_id: &str,
        version: &str,
        data: &[u8],
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
    ) -> ModelInfo {
        let hash = sm3_hash(data);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).unwrap();
        ModelInfo {
            model_id: model_id.to_string(),
            version: version.to_string(),
            hash,
            size: data.len() as u64,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: sig.to_bytes().to_vec(),
                timestamp: 123_456,
            },
            created_at: 654_321,
            capabilities: Vec::new(),
        }
    }

    /// OC1 manifest 编解码往返（D5）.
    #[test]
    fn oc1_manifest_roundtrip() {
        let info = make_info("eneros-lp", "2.0.0", b"model-bytes-oc1");
        let frame = encode_manifest(&info);
        // 帧头断言：magic LE + version
        assert_eq!(frame[0], 0x70);
        assert_eq!(frame[1], 0x0A);
        assert_eq!(frame[2], 1);
        let decoded = decode_manifest(&frame).unwrap();
        assert_eq!(decoded, info);
    }

    /// OC2 manifest 坏 magic / 截断 → Err(InvalidManifest).
    #[test]
    fn oc2_manifest_bad_magic_and_truncated() {
        let info = make_info("m", "1.0.0", b"x");
        let frame = encode_manifest(&info);

        let mut bad_magic = frame.clone();
        bad_magic[0] ^= 0xFF;
        assert_eq!(decode_manifest(&bad_magic), Err(OtaError::InvalidManifest));

        // 各长度截断（含帧头内截断与尾部字段截断）
        for cut in [0usize, 1, 2, 5, 10, frame.len() - 1] {
            assert_eq!(
                decode_manifest(&frame[..cut]),
                Err(OtaError::InvalidManifest),
                "截断长度 {} 必须报 InvalidManifest",
                cut
            );
        }

        // 字段越界：model_id_len 夸大
        let mut bad_len = frame.clone();
        bad_len[3] = 200;
        assert_eq!(decode_manifest(&bad_len), Err(OtaError::InvalidManifest));
    }

    /// OC3 check_update 无更新 → Ok(None).
    #[test]
    fn oc3_check_update_none() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let current = make_info("m", "1.0.0", b"cur");
        let client = OtaClient::new(current, kp.public_key, 3);
        let mut transport = MockOtaTransport::new(b"bytes".to_vec(), 4);
        assert_eq!(client.check_update(&mut transport), Ok(None));
    }

    /// OC4 check_update 同版本 / 新版本.
    #[test]
    fn oc4_check_update_same_and_new_version() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let current = make_info("m", "1.0.0", b"cur");
        let client = OtaClient::new(current, kp.public_key, 3);

        // 同版本 → None
        let same = make_info("m", "1.0.0", b"other-bytes");
        let mut t_same = MockOtaTransport::with_latest(same, b"bytes".to_vec(), 4);
        assert_eq!(client.check_update(&mut t_same), Ok(None));

        // 新版本 → Some(info)
        let newer = make_info("m", "2.0.0", b"new-bytes");
        let mut t_new = MockOtaTransport::with_latest(newer.clone(), b"bytes".to_vec(), 4);
        assert_eq!(client.check_update(&mut t_new), Ok(Some(newer)));
    }

    /// OC5 download_model 单 chunk 成功.
    #[test]
    fn oc5_download_single_chunk() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let client = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp.public_key, 3);
        let model = b"single-chunk-model".to_vec();
        let info = make_info("m", "2.0.0", &model);
        let mut transport = MockOtaTransport::with_latest(info.clone(), model.clone(), 1024);
        let data = client.download_model(&mut transport, &info).unwrap();
        assert_eq!(data, model);
        assert_eq!(transport.download_calls, 1);
    }

    /// 记录 offset 序列的测试传输（OC6/OC7 断言续传 offset）.
    ///
    /// `fail_on_call` 指定第 N 次 download_range 调用注入 1 次 TransportError；
    /// `always_fail` 为 true 时所有调用失败。
    struct RecordingTransport {
        model_bytes: Vec<u8>,
        chunk_size: usize,
        fail_on_call: Option<u32>,
        always_fail: bool,
        call_index: u32,
        offsets: Vec<u64>,
    }

    impl OtaTransport for RecordingTransport {
        fn fetch_latest(&mut self, _v: &str) -> Result<Option<ModelInfo>, OtaError> {
            Ok(None)
        }

        fn download_range(
            &mut self,
            _id: &str,
            offset: u64,
            len: u64,
        ) -> Result<Vec<u8>, OtaError> {
            let idx = self.call_index;
            self.call_index += 1;
            self.offsets.push(offset);
            if self.always_fail || self.fail_on_call == Some(idx) {
                return Err(OtaError::TransportError);
            }
            let start = offset as usize;
            let end = core::cmp::min(start + len as usize, self.model_bytes.len());
            let end = core::cmp::min(end, start + self.chunk_size);
            Ok(self.model_bytes[start..end].to_vec())
        }
    }

    /// OC6 download_model 多 chunk + 1 次失败续传（蓝图 §6.5）.
    #[test]
    fn oc6_download_multi_chunk_with_retry() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let client = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp.public_key, 3);
        // 10 字节模型，chunk_size=4 → 3 个分块（4+4+2）；第 2 次调用注入 1 次失败
        let model = b"0123456789".to_vec();
        let info = make_info("m", "2.0.0", &model);
        let mut transport = RecordingTransport {
            model_bytes: model.clone(),
            chunk_size: 4,
            fail_on_call: Some(1),
            always_fail: false,
            call_index: 0,
            offsets: Vec::new(),
        };
        let data = client.download_model(&mut transport, &info).unwrap();
        assert_eq!(data, model);
        // offset 序列：0（4B）→ 4（失败）→ 4（重试成功 4B）→ 8（2B）；
        // 重试 offset == 已下载长度，前 4 字节不重下
        assert_eq!(transport.offsets, vec![0u64, 4, 4, 8]);
    }

    /// OC7 download_model 连续失败超限 → Err(DownloadFailed).
    #[test]
    fn oc7_download_consecutive_failures_exceed() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let client = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp.public_key, 2);
        let model = b"0123456789".to_vec();
        let info = make_info("m", "2.0.0", &model);
        let mut transport = RecordingTransport {
            model_bytes: model,
            chunk_size: 4,
            fail_on_call: None,
            always_fail: true,
            call_index: 0,
            offsets: Vec::new(),
        };
        assert_eq!(
            client.download_model(&mut transport, &info),
            Err(OtaError::DownloadFailed)
        );
        // 初次尝试 + max_retries 次重试 = max_retries + 1 次调用
        assert_eq!(transport.offsets.len() as u32, 2 + 1);
    }

    /// OC8 download_model 空 chunk / size==0（防死循环）.
    #[test]
    fn oc8_download_empty_chunk_and_zero_size() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let client = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp.public_key, 3);

        // 空 chunk：实际字节(4) < 声明 size(10)，第二轮 offset 越界返回空
        let short = b"1234".to_vec();
        let mut info = make_info("m", "2.0.0", &short);
        info.size = 10;
        let mut transport = MockOtaTransport::with_latest(info.clone(), short, 4);
        assert_eq!(
            client.download_model(&mut transport, &info),
            Err(OtaError::DownloadFailed)
        );

        // size == 0 → InvalidConfig
        let mut zero = make_info("m", "2.0.0", b"z");
        zero.size = 0;
        let mut t2 = MockOtaTransport::new(Vec::new(), 4);
        assert_eq!(
            client.download_model(&mut t2, &zero),
            Err(OtaError::InvalidConfig)
        );
    }

    /// OC9 verify_model 哈希不匹配 → Err(HashMismatch).
    #[test]
    fn oc9_verify_hash_mismatch() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let client = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp.public_key, 3);
        // info.hash 为另一份数据的哈希
        let info = make_info("m", "2.0.0", b"authentic-data");
        assert_eq!(
            client.verify_model(b"tampered-data!", &info),
            Err(OtaError::HashMismatch)
        );
    }

    /// OC10 verify_model 签名无效（错公钥 / 坏签名）→ Err(SignatureInvalid).
    #[test]
    fn oc10_verify_signature_invalid() {
        let mut rng = CsRng::new();
        let kp_signer = Sm2KeyPair::generate(&mut rng).unwrap();
        let kp_wrong = Sm2KeyPair::generate(&mut rng).unwrap();
        let data = b"model-data-oc10";

        // (a) 错公钥：用 kp_signer 签名，client 持 kp_wrong 公钥
        let info = make_signed_info("m", "2.0.0", data, &kp_signer, &mut rng);
        let client_wrong = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp_wrong.public_key, 3);
        assert_eq!(
            client_wrong.verify_model(data, &info),
            Err(OtaError::SignatureInvalid)
        );

        // (b) 坏签名：签名值篡改 1 字节
        let client_right = OtaClient::new(make_info("m", "1.0.0", b"cur"), kp_signer.public_key, 3);
        let mut bad_sig = info.clone();
        bad_sig.signature.signature[10] ^= 0xFF;
        assert_eq!(
            client_right.verify_model(data, &bad_sig),
            Err(OtaError::SignatureInvalid)
        );

        // (c) 签名长度非 64B
        let mut short_sig = info.clone();
        short_sig.signature.signature = vec![0u8; 10];
        assert_eq!(
            client_right.verify_model(data, &short_sig),
            Err(OtaError::SignatureInvalid)
        );
    }
}
