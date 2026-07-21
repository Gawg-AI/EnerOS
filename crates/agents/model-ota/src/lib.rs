//! EnerOS v0.111.0 模型 OTA 推送（P2-H 第 3 版）.
//!
//! AI 模型需云端训练 → 签名 → 边缘热加载的远程迭代能力，免去现场维护（蓝图 §1）。
//! 无签名校验则模型可能被篡改（§2 阻塞项）。本 crate 在 v0.110.0 云边同步通道、
//! v0.31.0/v0.32.0/v0.33.0 国密 SM2/SM3 与 PKI 基座上，实现 OTA 客户端（检查更新 +
//! 断点续传下载）+ SM3 哈希/SM2 签名双重验证 + 白名单 + 原子热切换与回滚，打通
//! 「云端训练 → 签名 → 推送 → 验证 → 热加载」链路，为 v0.112.0 云端孪生与联邦 AI
//! 持续演进提供模型更新通道。
//!
//! # 核心类型
//!
//! - [`OtaClient`] / [`ModelInfo`] / [`ModelSignature`] / [`SigAlgorithm`] — OTA
//!   客户端（`check_update` + `download_model` 断点续传 + `verify_model` +
//!   `update_once`/`rollback_once` 编排）与模型清单（D6/D11）
//! - [`encode_manifest`] / [`decode_manifest`] — manifest 二进制编解码（magic
//!   0x0A70 + version 1，全小端 TLV，D5）
//! - [`verify_model_signature`] — SM3 哈希 → SM2 验签纯函数（复用 eneros-crypto，D7）
//! - [`HotLoader`] / [`ModelInstance`] — 白名单 `load_new` + 原子 `swap` + `rollback`
//!   （`alloc::sync::Arc`，无锁 &mut self 单线程惯例，D9/D10）
//! - [`OtaTransport`] / [`MockOtaTransport`] — 传输抽象 + mock 实现（D4）
//! - [`OtaError`] — 错误枚举（11 变体，D12）
//! - [`OtaStats`] — 更新状态统计（§9 可观测，D12）
//! - [`OtaUpdateOutcome`] — `update_once` 结果（NoUpdate / Updated）
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§5/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/model_ota/` → `crates/agents/model-ota/`（eneros-model-ota） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；OTA 为云边推送通道，与 v0.110.0 cloud-sync / v0.95.0 cloud-coordinator 同属 agents 子系统 |
//! | **D2** | 蓝图 `docs/phase2/model_ota.md` → `docs/agents/model-ota-design.md` | 记忆 §2.3.3 强制：文档按方向分类（cloud-sync-design.md 同目录先例） |
//! | **D3** | 蓝图 `tests/model_ota.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.110.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 蓝图 `async check_update/download_model` + `HttpClient` + `server_url`/`download_dir` 字段 + `sleep().await` 退避 → `OtaTransport` sync trait（`fetch_latest(current_version)` + `download_range(model_id, offset, len)`）+ `MockOtaTransport`（fail_remaining 故障注入 + chunk_size 截断 + download_calls 计数，置于 lib.rs）；server_url/download_dir 移出 OtaClient；下载循环失败立即重试（退避由传输实现层自持） | no_std 无 async runtime/无 std::net/无 sleep（v0.110.0 D4 SyncTransport 同先例）；主机可测；真实 HTTP/gRPC 适配器在集成层注入；断点续传语义（offset 从已下载长度继续）保留 |
//! | **D5** | 蓝图 `serde_json::from_slice(&resp.body)` 解析 ModelInfo → 自定义二进制 manifest 编解码（`encode_manifest`/`decode_manifest`，magic 0x0A70 + version 1，全小端 TLV） | 零外部依赖（serde/serde_json 不入仓，v0.110.0 D11 同先例）；magic+version 支撑云端 API 版本演进 |
//! | **D6** | `SigAlgorithm { Sm2Sm3, RsaSha256 }` 保留 2 变体（对齐蓝图数据结构），但 RsaSha256 不可验证——`verify_model_signature` 遇之返回 `UnsupportedAlgorithm` | eneros-crypto 纯国密无 RSA（信创 §5.6 全程国密要求）；v0.110.0 D5 CompressionType 占位同先例 |
//! | **D7** | 蓝图 `sm3_hash`/`sm2_verify` 未指明实现 → 复用 eneros-crypto（path 依赖 `../../security/crypto`）：`sm3_hash(data) -> [u8;32]`、`sm2_verify(&hash, &Sm2Signature, &Sm2PublicKey)`、`Sm2Signature::from_bytes`（64B r‖s） | 记忆 §5.5/禁忌 14 禁止重复造轮子；国密实现已经安全评审（常量时间/零化/Drop），自研重引入风险 |
//! | **D8** | 蓝图 `current_time_ms()` 全局时间函数 → `now: u64` 参数注入（load_new/update_once/rollback_once） | no_std 无系统时间（v0.110.0 D7 / v0.108.0 D9 同先例）；集成层由 v0.12.0 RTC 供给 |
//! | **D9** | 蓝图 HotLoader 用 std `Arc/Mutex/AtomicU32` + `swap_lock` + `mem::replace(&mut *self.current.as_ref())`（不可编译：Arc 不可变借用）+ rollback 引用未声明的 `self.previous` 字段 → `alloc::sync::Arc` + 无锁 &mut self 单线程惯例：`current: Arc<ModelInstance>` / `previous: Option<Arc<ModelInstance>>` / `loading: Option<ModelInstance>`；swap 持 loading.take() + mem::replace 原子轮换并留存 previous；删除 swap_lock 与 AlreadySwapping 变体（&mut self 编译期排他） | 蓝图代码两处编译错误必须修复；v0.110.0 D4 单线程无 Send+Sync 惯例；Arc 仅在 current() 读侧克隆，写侧 &mut self 排他 |
//! | **D10** | 删除蓝图 `ModelInstance.ref_count: AtomicU32` | `alloc::sync::Arc` 强引用计数即生命周期管理，手写计数属重复造轮子（禁忌 14）；「旧模型引用归零自动释放」语义由 Arc drop 承载 |
//! | **D11** | ① 蓝图 `trusted_ca_pubkey()` 从本地存储 `load_ca_pubkey().unwrap_or_default()` → 构造注入 `trusted_pubkey: Sm2PublicKey`；② 删除蓝图 `ModelSignature.signer_cert: Sm2Cert` 字段；③ 白名单 white_list 构造注入 | ① 安全关键件禁止静默默认空值（空公钥语义不明，no_std 无本地安全存储抽象）；② 信任锚为注入公钥，证书链验证归 v0.32.0 PKI 层职责，本版不做链式验证（Karpathy 最小实现）；③ 白名单运维下发属集成层 |
//! | **D12** | 错误模型 `OtaError` = TransportError / DownloadFailed / InvalidManifest / HashMismatch / SignatureInvalid / NotInWhitelist / NothingToSwap / NoPreviousVersion / UnsupportedAlgorithm / InvalidConfig / SizeMismatch（11 变体，Debug/Clone/Copy/PartialEq，Copy 对齐 v0.95.0 CloudError 惯例；DownloadFailed 删除蓝图 String payload）；`verify_model` 由蓝图 `Result<bool>` 改 `Result<(), OtaError>` 区分哈希/签名失败支撑 §4.4 安全告警；新增 `OtaStats { total_updates, total_rejected, total_rollbacks, last_update_at }` 落地 §9 更新状态 metric；性能「100MB < 60s」落地为 cfg(test) Instant 主机断言 | 蓝图引用 OtaError 未定义；变体覆盖 §4.4 各失败面；bool 无法区分拒绝原因不利审计；性能口径与 v0.109.0/v0.110.0 D12 一致（真实网络时延为实验室项） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，唯一依赖 eneros-crypto（workspace 内 path 依赖），
//! 零第三方依赖，零 unsafe，零 extern "C"，不调用 `panic!` / `todo!` /
//! `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod model_loader;
mod ota_client;
mod signature;

use alloc::vec::Vec;

pub use model_loader::{HotLoader, ModelInstance};
pub use ota_client::{
    decode_manifest, encode_manifest, ModelInfo, ModelSignature, OtaClient, OtaStats, SigAlgorithm,
};
pub use signature::verify_model_signature;

/// OTA 错误（D12：11 变体覆盖蓝图 §4.4 各失败面，Copy 对齐 v0.95.0 CloudError 惯例）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OtaError {
    /// 传输层错误（网络不可达/超时等，由 `OtaTransport` 上报）.
    TransportError,
    /// 下载失败（连续失败超 max_retries，或 transport 返回空 chunk 防死循环）.
    DownloadFailed,
    /// manifest 帧无效（magic 错误/版本不符/截断/字段越界）.
    InvalidManifest,
    /// 模型 SM3 哈希与清单声明不匹配（§4.4 安全告警）.
    HashMismatch,
    /// SM2 签名验证失败（签名长度非 64B/验签 false/验签内部错误）.
    SignatureInvalid,
    /// 模型哈希不在白名单（§4.3 拒绝 + 审计）.
    NotInWhitelist,
    /// 无待切换模型（swap 前未 load_new）.
    NothingToSwap,
    /// 无上一版本可回滚.
    NoPreviousVersion,
    /// 签名算法不支持（RsaSha256 仅占位不可验证，D6）.
    UnsupportedAlgorithm,
    /// 配置无效（info.size == 0 等）.
    InvalidConfig,
    /// 下载完成字节数与清单声明 size 不一致.
    SizeMismatch,
}

/// OTA 传输抽象（D4：sync trait，no_std 单线程惯例，不要求 Send+Sync；
/// 真实 HTTP/gRPC 适配器在集成层注入）.
pub trait OtaTransport {
    /// 查询云端最新模型清单；无更新返回 `Ok(None)`（HTTP 204/JSON 语义由实现侧封装）.
    fn fetch_latest(&mut self, current_version: &str) -> Result<Option<ModelInfo>, OtaError>;
    /// 断点续传分块下载：返回 `[offset, offset+len)` 区间字节（可按块大小截断）.
    fn download_range(
        &mut self,
        model_id: &str,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, OtaError>;
}

/// Mock OTA 传输（D4，v0.110.0 MockSyncTransport 先例：故障注入 + 调用计数）.
pub struct MockOtaTransport {
    /// 云端最新清单（fetch_latest 返回其克隆）.
    latest: Option<ModelInfo>,
    /// 云端模型字节（download_range 数据源）.
    model_bytes: Vec<u8>,
    /// 剩余故障注入次数：>0 时递减并返回 `Err(TransportError)`.
    fail_remaining: u32,
    /// 单次 download_range 最大返回字节数（模拟分块）.
    chunk_size: usize,
    /// 成功下载调用次数统计（测试断言用）.
    pub download_calls: u32,
}

impl MockOtaTransport {
    /// 构造无更新（latest=None）的 mock.
    pub fn new(model_bytes: Vec<u8>, chunk_size: usize) -> Self {
        Self {
            latest: None,
            model_bytes,
            fail_remaining: 0,
            chunk_size,
            download_calls: 0,
        }
    }

    /// 构造持有最新清单 `latest` 的 mock.
    pub fn with_latest(latest: ModelInfo, model_bytes: Vec<u8>, chunk_size: usize) -> Self {
        Self {
            latest: Some(latest),
            model_bytes,
            fail_remaining: 0,
            chunk_size,
            download_calls: 0,
        }
    }
}

impl OtaTransport for MockOtaTransport {
    fn fetch_latest(&mut self, _current_version: &str) -> Result<Option<ModelInfo>, OtaError> {
        Ok(self.latest.clone())
    }

    fn download_range(
        &mut self,
        _model_id: &str,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, OtaError> {
        if self.fail_remaining > 0 {
            self.fail_remaining -= 1;
            return Err(OtaError::TransportError);
        }
        let start = offset as usize;
        let want = core::cmp::min(len as usize, self.chunk_size);
        let end = core::cmp::min(start.saturating_add(want), self.model_bytes.len());
        let chunk = match self.model_bytes.get(start..end) {
            Some(slice) => slice.to_vec(),
            None => Vec::new(),
        };
        self.download_calls += 1;
        Ok(chunk)
    }
}

/// `update_once` 编排结果.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OtaUpdateOutcome {
    /// 云端无更新（或同版本），本次零动作.
    NoUpdate,
    /// 新模型已下载、验证、热切换成功.
    Updated,
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};

    use super::*;

    /// 测试辅助：用真实 SM2 密钥对对模型 SM3 哈希签名，构造 ModelInfo.
    fn make_signed_model(
        model_id: &str,
        version: &str,
        data: &[u8],
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
    ) -> ModelInfo {
        let hash = sm3_hash(data);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).unwrap();
        ModelInfo {
            model_id: String::from(model_id),
            version: String::from(version),
            hash,
            size: data.len() as u64,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: sig.to_bytes().to_vec(),
                timestamp: 1_000,
            },
            created_at: 2_000,
            capabilities: vec![String::from("infer"), String::from("solver")],
        }
    }

    fn make_loader_and_client(
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
        extra_white: &[[u8; 32]],
    ) -> (HotLoader, OtaClient) {
        let v1_data = b"model-v1-weights".to_vec();
        let v1_info = make_signed_model("eneros-lp", "1.0.0", &v1_data, kp, rng);
        let v1_instance = ModelInstance {
            info: v1_info.clone(),
            data: v1_data,
            loaded_at: 100,
        };
        let mut white_list = vec![sm3_hash(b"model-v1-weights")];
        white_list.extend_from_slice(extra_white);
        let loader = HotLoader::new(v1_instance, white_list);
        let client = OtaClient::new(v1_info, kp.public_key, 3);
        (loader, client)
    }

    /// INT22 端到端更新全流程（蓝图 §6.2 集成测试）.
    #[test]
    fn int22_end_to_end_update() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let v2_data = b"model-v2-weights-larger".to_vec();
        let v2_hash = sm3_hash(&v2_data);
        let (mut loader, mut client) = make_loader_and_client(&kp, &mut rng, &[v2_hash]);
        let v2_info = make_signed_model("eneros-lp", "2.0.0", &v2_data, &kp, &mut rng);

        let mut transport = MockOtaTransport::with_latest(v2_info, v2_data, 4);
        let r = client.update_once(&mut transport, &mut loader, 9_999);
        assert_eq!(r, Ok(OtaUpdateOutcome::Updated));
        assert_eq!(loader.current().info.version, "2.0.0");
        assert_eq!(client.current_model().version, "2.0.0");
        assert_eq!(client.stats().total_updates, 1);
        assert_eq!(client.stats().last_update_at, 9_999);
        assert_eq!(client.stats().total_rejected, 0);
    }

    /// INT23 篡改模型拒绝（蓝图 §6.5 故障注入）：loader 零变化.
    #[test]
    fn int23_tampered_model_rejected() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let v2_data = b"model-v2-authentic".to_vec();
        let v2_hash = sm3_hash(&v2_data);
        let (mut loader, mut client) = make_loader_and_client(&kp, &mut rng, &[v2_hash]);
        let v2_info = make_signed_model("eneros-lp", "2.0.0", &v2_data, &kp, &mut rng);

        // 传输层字节被篡改 1 字节（哈希不匹配）
        let mut tampered = v2_data.clone();
        tampered[0] ^= 0xFF;
        let before = loader.current();
        let mut transport = MockOtaTransport::with_latest(v2_info, tampered, 4);
        let r = client.update_once(&mut transport, &mut loader, 5_000);
        assert_eq!(r, Err(OtaError::HashMismatch));
        assert_eq!(client.stats().total_rejected, 1);
        assert_eq!(client.stats().total_updates, 0);
        // loader 零变化（Arc 指针相等 + 版本不变）
        let after = loader.current();
        assert!(alloc::sync::Arc::ptr_eq(&before, &after));
        assert_eq!(after.info.version, "1.0.0");
        assert_eq!(client.current_model().version, "1.0.0");
    }

    /// 按计划注入故障的测试传输（指定第 N 次 download_range 调用失败）.
    struct FlakyTransport {
        latest: Option<ModelInfo>,
        model_bytes: Vec<u8>,
        chunk_size: usize,
        fail_on_calls: Vec<u32>,
        call_index: u32,
        download_calls: u32,
        offsets: Vec<u64>,
    }

    impl OtaTransport for FlakyTransport {
        fn fetch_latest(&mut self, _v: &str) -> Result<Option<ModelInfo>, OtaError> {
            Ok(self.latest.clone())
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
            if self.fail_on_calls.contains(&idx) {
                return Err(OtaError::TransportError);
            }
            let start = offset as usize;
            let end = core::cmp::min(start + len as usize, self.model_bytes.len());
            let end = core::cmp::min(end, start + self.chunk_size);
            self.download_calls += 1;
            Ok(self.model_bytes[start..end].to_vec())
        }
    }

    /// INT24 断点续传集成（蓝图 §6.5）：2 次非连续失败仍更新成功.
    #[test]
    fn int24_resume_two_nonconsecutive_failures() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        // 13 字节 / chunk 4 → 4 个成功分块
        let v2_data = b"0123456789abc".to_vec();
        let v2_hash = sm3_hash(&v2_data);
        let (mut loader, mut client) = make_loader_and_client(&kp, &mut rng, &[v2_hash]);
        let v2_info = make_signed_model("eneros-lp", "2.0.0", &v2_data, &kp, &mut rng);

        let mut transport = FlakyTransport {
            latest: Some(v2_info),
            model_bytes: v2_data.clone(),
            chunk_size: 4,
            fail_on_calls: vec![0, 2], // 第 1 次与第 3 次调用失败（非连续）
            call_index: 0,
            download_calls: 0,
            offsets: Vec::new(),
        };
        let r = client.update_once(&mut transport, &mut loader, 7_777);
        assert_eq!(r, Ok(OtaUpdateOutcome::Updated));
        assert!(transport.download_calls >= 4);
        assert_eq!(loader.current().data, v2_data);
        // 续传 offset 单调前进（失败重试从已下载长度继续）
        assert!(transport.offsets.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(
            transport.offsets,
            vec![0u64, 0, 4, 4, 8, 12],
            "失败重试 offset 必须等于已下载长度"
        );
    }

    /// INT25 切换后回滚（蓝图 §6.4 回归）.
    #[test]
    fn int25_update_then_rollback() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let v2_data = b"model-v2-rollback-target".to_vec();
        let v2_hash = sm3_hash(&v2_data);
        let (mut loader, mut client) = make_loader_and_client(&kp, &mut rng, &[v2_hash]);
        let v2_info = make_signed_model("eneros-lp", "2.0.0", &v2_data, &kp, &mut rng);

        let mut transport = MockOtaTransport::with_latest(v2_info, v2_data, 8);
        assert_eq!(
            client.update_once(&mut transport, &mut loader, 1_000),
            Ok(OtaUpdateOutcome::Updated)
        );
        assert_eq!(loader.current().info.version, "2.0.0");

        client.rollback_once(&mut loader, 2_000).unwrap();
        assert_eq!(loader.current().info.version, "1.0.0");
        assert_eq!(client.current_model().version, "1.0.0");
        assert_eq!(client.stats().total_rollbacks, 1);
        assert_eq!(client.stats().last_update_at, 2_000);
        assert_eq!(client.stats().total_updates, 1);
    }

    /// PERF26 100MB 下载 + SM3 校验 < 60s（蓝图 §6.3/§7.2，cfg(test) Instant 口径，D12）.
    #[test]
    fn perf26_100mb_download_sm3() {
        let start = std::time::Instant::now();
        let size = 100usize * 1024 * 1024;
        let mut bytes = vec![0u8; size];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let hash = sm3_hash(&bytes);
        let info = ModelInfo {
            model_id: String::from("big-model"),
            version: String::from("9.9.9"),
            hash,
            size: size as u64,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: Vec::new(),
                timestamp: 0,
            },
            created_at: 0,
            capabilities: Vec::new(),
        };
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let current = ModelInfo {
            model_id: String::from("big-model"),
            version: String::from("9.9.8"),
            hash: [0u8; 32],
            size: 1,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: Vec::new(),
                timestamp: 0,
            },
            created_at: 0,
            capabilities: Vec::new(),
        };
        let client = OtaClient::new(current, kp.public_key, 3);
        let mut transport = MockOtaTransport::with_latest(info.clone(), bytes, 1024 * 1024);
        let data = client.download_model(&mut transport, &info).unwrap();
        assert_eq!(data.len(), size);
        assert_eq!(sm3_hash(&data), hash);
        assert_eq!(transport.download_calls, 100);
        assert!(
            start.elapsed().as_secs() < 60,
            "100MB 下载 + SM3 校验耗时 {:?} 超 60s",
            start.elapsed()
        );
    }
}
