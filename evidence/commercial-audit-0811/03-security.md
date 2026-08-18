# 03 — 安全域（crates/security/ 14 子 crate + 安全配置 + 控制写接缝）全量代码级审查

审查日期：2026-08-11　子代理域：security（零容忍口径）
审查基准：蓝图/24（控制写接缝）、蓝图/25（FIX-BUGS-0810，已修项不重复报告）、蓝图/27 §4 S 条目安全红线、evidence/crypto-review/、docs/security/
范围：crates/security/ 全部 14 子 crate（crypto、crypto-gmssl、attestation、tpm、secure-boot、iec62351、audit、did、model-security、ota-ab、recovery、pentest、compliance、tls）逐文件代码级；configs/ 11+ 安全 toml 消费面；AuthorizedWriter/AuthorizedCommand 消费面清点。

---

## 0. 先声明：已核查无发现 / 已修项不重复

| 范围 | 结论 |
|---|---|
| crypto-gmssl（含 gmssl_helpers.c + ffi.rs） | **不违规**：ADR-022 裁决的 vetted binding（OpenSSL 3，std 面专用，target no_std 不链接）；unsafe 集中且有 SAFETY 注释、FFI 面最小化（5 个 C 函数）、KAT 交叉验证齐全（GB/T 标准向量 + 篡改负例）。无发现 |
| crypto hw 层（engine/mod/phytium/kunpeng/software） | **零硬件依赖达标**：`CryptoSelector` 探测 + `is_available()` + 硬件错误自动 fallback；`create_selector_with_hw` 为 `unsafe fn` 显式调用语义，软件引擎始终注册；simulate 模式一等公民。例外见 SE-06（software.rs GCM tag） |
| attest.rs 四步验证流水线 / tpm.rs SoftTpm / event_log.rs / audit/chain.rs / model-security/signature.rs+model_guard.rs / recovery/factory_reset.rs+flash.rs / ota-ab ab_partition+rollback+kernel_ota+downloader 主路径 / tls/embedded.rs | 健壮：fail-closed、常时比较、先校验后落地、负例测试齐全（已核查无发现） |
| 蓝图/25 B5 DRBG 重播种 | 已修（fill_bytes_or_reseed），不重复报告 |
| PKI P0-12（SPKI 根比对） | 已修，不重复报告 |
| FramingRecordCodec / eneros-mtls | 已诚实更名 / 已删除；但其遗留配置见 SE-09 |
| secure-boot verifier.rs（10 步流水线）/ header.rs / chain.rs | 健壮无发现（但 manifest 消费面断裂见 SE-02） |
| 控制写接缝封焊 | **达标**：`PointAccess` 只读、唯一写路径 `AuthorizedWriter::write_authorized`（20+ 消费文件清点，modbus-tcp/rtu、iec104-slave/master、control-gateway、ai/runner、device-agent 均经接缝；`write_point_impl` 私有、无未授权旁路；control-gateway 有 default-deny fail-closed 测试）。configs/control-write.toml 有真实代码消费（device-agent assembly_demo 读 lease_id） |

---

## 1. P0 发现（2 项）

### SE-01（P0）eneros-attestation `tpm2` feature 编译失败——真实 TPM2 硬件路径完全不可用
- 类别：BUG
- 定位：`crates/security/attestation/src/real.rs:123`
  ```rust
  self.ak_pubkey.ok_or(TpmError::KeyUnavailable)
  ```
  `TpmError`（attestation/src/lib.rs）仅有 TpmUnavailable / InvalidPcrIndex / ExtendFailed / ReadFailed / QuoteFailed 五变体，**无 `KeyUnavailable`**。
- 根因：real.rs 编码期引用了不存在的错误变体，从未有人编译过 `--features tpm2` 路径。
- 实锤：`cargo check -p eneros-attestation --features tpm2` → E0599 编译失败（本次审查实测）。
- 修复：① TpmError 增补 `KeyUnavailable` 变体（含 `#[cfg(feature = "tpm2")]` 全 match 面同步）；或 ② real.rs:123 改用既有 `TpmError::ReadFailed`。推荐 ①，签名 `pub enum TpmError { ..., KeyUnavailable }`。同时在 CI 增补 `cargo check --all-features` 对 attestation crate 的矩阵项。
- 二次验证：消费方清点——`TpmBackend` trait 实现共 3 个（SoftTpm / TunnelAttestTransport / Tpm2Backend），增补变体影响全 match 面仅 real.rs 与 lib.rs 测试；修复不打开任何绕过路径（错误路径保持 Err）。不变量：attestation 任一环节 Err → 证明失败（fail-closed 保持）。
- host 验证：`cargo check -p eneros-attestation --features tpm2 && cargo test -p eneros-attestation --features tpm2` 判据：编译通过 + 测试全绿。无真机依赖（tpm2 feature 为 host 可编译适配层）。
- 冲突声明：**与蓝图/27 硬件能力探测+降级红线冲突**——"真实 TPM2 适配器"交付物当前不可编译，只剩 SoftTpm 单腿。

### SE-02（P0）Secure Boot 信任链下级公钥绑定断裂：verify_and_parse 成功路径永不可达，全仓实际消费未验签 parse()
- 类别：BUG（结构性）
- 定位：`crates/security/secure-boot/src/manifest.rs:78-91`
  ```rust
  pub fn verify_and_parse(...) -> Result<...> {
      let mut verifier = crate::verifier::BootVerifier::new(*root_key, 0); // stage=Rom
      verifier.verify_stage(BootStage::Bootloader, ...)? // 步骤1 顺序检查必 WrongStage
  ```
  `BootVerifier::new` 初始 stage=Rom（verifier.rs/chain.rs），首次 `verify_stage` 必须验 Rom 级，直接验 Bootloader 必返回 `WrongStage`。**探针测试实锤**（本次审查临时探针，已删除）：合法签名输入也恒返回 WrongStage，成功路径数学上不可达。
- 消费面清点（Grep 全仓）：生产与测试（secure_boot.rs、tests/secure_boot_test.rs）**全部使用未验签的 `BootManifest::parse()` + `encode()`**，`verify_and_parse` 零成功消费 → v0.235.0 B4-02「防张冠李戴密码学绑定」实际断裂，manifest 中的 bl_key 可由装配方任意注入而不被根密钥约束。
- 附带死代码：manifest.rs `let _header = encode_header(sig);`（编码产物丢弃）；`min_timestamp = 0` 硬编码使防降级失效。
- 根因：verify_and_parse 复用有状态流水线验证器却未先推进 Rom 步骤；且该函数从未被集成层真正接线（测试断言只测了"失败"分支，恰好被 WrongStage 满足，掩盖了断裂）。
- 修复：函数签名保持，内部改为无状态直验：`fn verify_and_parse(image: &[u8], root_key: &Sm2PublicKey, sig: &[u8;64]) -> Result<Manifest, BootError>` 内先 `sm3_hash(image)` → `sm2_verify(hash, sig, root_key)` 通过后才 parse，随后从已验签镜像体提取 bl_key；删除 `_header` 死代码；min_timestamp 参数化注入（与 secure-boot.toml [anti_rollback] 对齐）。
- 二次验证：消费方清点——parse() 当前全仓消费点需逐个替换为 verify_and_parse（secure_boot.rs 主流程 + 测试夹具）；替换后合法 manifest 路径以真实签名测试覆盖（正例+张冠李戴负例）。修复不引入新绕过：验签失败 → Err，parse 不再公开暴露或标记 deprecated。不变量：「Kernel/Runtime 级公钥必须经根密钥签名背书」恢复成立。
- host 验证：`cargo test -p eneros-secure-boot` 增补正例断言 `verify_and_parse(合法签名镜像).is_ok()`（现状此断言必失败，即为判据）；篡改 bl_key 后必 Err。
- 冲突声明：**与蓝图/27 S 条目「Secure Boot 信任链密码学完整性」直接冲突**；B4-02 销项应视为未达成。

---

## 2. P1 发现（9 项）

### SE-03（P1）attestation real.rs：AK 句柄硬编码 0 + 签名静默截断/零填充
- 类别：BUG
- 定位：`crates/security/attestation/src/real.rs:111` `self.tpm.sign(0, &digest)`；`real.rs:113-115`
  ```rust
  let n = sig.len().min(64);
  sig_bytes[..n].copy_from_slice(&sig[..n]);
  ```
- 根因：AK 句柄不可注入（写死 0）；TPM2 真实签名输出 DER（≤72B），>64B 截断、<64B 零填充，均破坏签名完整性。
- 修复：`RealAttestor::new(tpm, ak_handle: KeyHandle, ak_pubkey)` 注入句柄；签名处理改为 DER→r‖s 解析（复用 crypto-gmssl::der::der_to_rs 或 crypto 内等价物），解析失败返回 Err（fail-closed），禁止截断。
- 二次验证：消费方为 attestation 主流程（tpm2 feature），与 SE-01 同批修复；无新绕过路径（解析失败即证明失败）。不变量：quote 签名为 TPM 原样输出映射，无信息丢失。
- host 验证：feature 编译通后 `cargo test -p eneros-attestation --features tpm2`（模拟后端注入 72B DER 签名判据：转 r‖s 后验签通过）。
- 冲突声明：与蓝图/27 远程证明证据链红线冲突（同 SE-01 族）。

### SE-04（P1）TunnelAttestTransport 证据链不完整：编码只发 nonce+signature，远端无法重算验签
- 类别：缺失功能
- 定位：`crates/security/attestation/src/real.rs:152-159` encode 仅 `magic "RATT" ‖ nonce(20) ‖ signature(64)`；`real.rs:162-173` decode 非 "ROK" 一律标 `AttestReason::PcrMismatch`。
- 根因：quote_digest 依赖 pcr_select/pcr_values/quote_time 等 Quote 内容，隧道编码未携带 → 远端验证方无法独立重算摘要验签，只能信任本端自报结果。
- 修复：encode 扩为 `RATT ‖ nonce ‖ pcr_select(3B) ‖ pcr_values(n×32) ‖ quote_time(8B) ‖ signature`；`decode_result` 按对端回码映射真实 reason（至少区分 SignatureInvalid/PcrMismatch/Internal）。
- 二次验证：消费方为 attestation 隧道传输（云侧验证入口）；协议变更需双端同步（格式版本字节防混读）。不变量：远端可离线重算 quote_digest 并验签（证据链自包含）。
- host 验证：`cargo test -p eneros-attestation` 增补环回断言：encode→decode→重算 digest→sm2_verify 通过。
- 冲突声明：与蓝图/27「attestation 证据链完整度」红线冲突。

### SE-05（P1）MockAttestTransport 无 cfg 门控——全项目唯一 S2 违规
- 类别：BUG（纪律违规）
- 定位：`crates/security/attestation/src/lib.rs:138` `pub struct MockAttestTransport` 无 `#[cfg(any(test, feature = "mock"))]`。
- 根因：遗漏门控。Grep 全项目实证：cloud-sync/model-ota/mqtt/mms/agent-bus-dds 等所有其他 Mock 均已门控，此处唯一漏网 → 生产二进制可实例化 Mock 证明传输器（证明结果可被伪造为通过）。
- 修复：加 `#[cfg(any(test, feature = "mock"))]` 于 struct 与 impl 块；Cargo.toml 增 `mock = []` feature（若缺）。
- 二次验证：消费方清点——仅 tests 与 attestation 内部使用；门控后默认 build 该类型消失，无新绕过。不变量：生产构建不存在可伪造证明的 Mock 面。
- host 验证：`cargo check -p eneros-attestation`（默认 features）后 `grep -rn "MockAttestTransport" target 无关`；判据：`rustc` 元数据中无该符号（`cargo doc` 不出现）+ `cargo test` 仍绿。
- 冲突声明：与蓝图/27 S2（Mock 门控不可回退清单）直接冲突。

### SE-06（P1）SoftwareCryptoEngine SM4-GCM/CTR 加密丢弃认证 tag——加解密不对称
- 类别：BUG
- 定位：`crates/security/crypto/src/hw/software.rs:167`
  ```rust
  let (ciphertext, _tag) = gcm.encrypt(&nonce, data, &[]);
  ```
  而 sm4_decrypt（L213-238）期望密文尾部 16B 为 tag → GCM 分支 encrypt 产物无 tag、无法被 decrypt 消费；语义退化为无认证 CTR。
- 根因：tag 未拼接进返回值。
- 损害面：Grep 全仓确认 crypto 之外当前无 `Sm4Mode::Gcm/Ctr` 生产消费者（损害面为空但接口是定时炸弹——任何新消费者接入即静默失去认证加密）。
- 修复：`sm4_encrypt` Gcm 分支改为返回 `ciphertext ‖ tag`（与 decrypt 契约对齐）；Ctr 分支明确无 tag 语义并在文档/类型层警示。补单元测试 GCM encrypt→decrypt 环回。
- 二次验证：消费方清点为零生产消费 → 修复零破坏；不变量：`sm4_decrypt(key, iv, Gcm, sm4_encrypt(...)) == plaintext` 且 tag 篡改必 Err。
- host 验证：`cargo test -p eneros-crypto hw::software` 判据：新增 GCM 环回测试通过 + 篡改尾 16B 必 Err。
- 冲突声明：无直接 27 条目冲突，但违反项目「AEAD 必须验 tag」密码学纪律。

### SE-07（P1）TPM 驱动 seal/unseal 句柄偏移不一致——硬件路径 seal 产物无法 unseal
- 类别：BUG
- 定位：`crates/security/tpm/src/driver.rs:538-551` seal 返回完整 resp（TPM2 响应 header 10B，newHandle 位于 resp[10..14]）；`driver.rs:553-562` unseal 却读 `sealed[0..4]` 作句柄 → 偏移差 10 字节。
- 根因：seal 未按约定剥离 header 即返回；unseal 按"裸句柄"契约解析。
- 损害面：仅 TPM2 真实硬件路径（fail-closed 语义下不造成泄露，但功能断裂）；SoftTpm 路径不受影响。
- 修复：seal 返回前剥离 header，`Ok(resp[10..].to_vec())`（或在 unseal 侧按完整 resp 解析——二选一并写契约注释 + 环回测试锁定）。
- 二次验证：消费方清点——eneros-tpm TpmDriver trait 的 seal/unseal 消费者为 key_management/attestation 密封存储；修复后 seal→unseal 环回测试为判据。无新绕过路径。
- host 验证：`cargo test -p eneros-tpm`（模拟后端）判据：seal→unseal 环回明文一致。
- 冲突声明：与蓝图/27 硬件降级红线相关（硬件路径功能不完整）。

### SE-08（P1）DID 证书链"验证"从不校验链签名——签名验证名义缺失
- 类别：缺失功能 + 命名诚实性
- 定位：`crates/security/did/src/verifier.rs:64-83` `verify_certificate_chain` 只做有效期 + DID 非空 + `signature.len() == 64` 检查；`CertificateChain.signature` 字段从不参与密码学验证。测试实证（verifier.rs:197）：全零签名 `vec![0u8; 64]` 通过"验证"。
- 根因：链签名验证逻辑缺失，函数名与文档（"验证证书链"）超出实际能力。
- 损害面：当前 CertificateChain 仅 did crate 内部使用（Grep 无外部生产消费），但 DID 跨域认证是蓝图承诺能力。
- 修复：`verify_certificate_chain(&self, chain, current_time) -> Result<bool, DidError>` 内增补：用 root_did 对应公钥验 issuer 签发签名、issuer 公钥验 subject 链签名（`sm2_verify`），签名材料 = 链字段定序编码 SM3 哈希；验证失败返回 `Err(InvalidSignature)`。若短期无法落地，函数必须更名 `verify_certificate_chain_validity`（只验时效）并文档明示无签名验证。
- 二次验证：消费方清点为零外部消费 → 修复零破坏；不变量：伪造链签名必被拒。
- host 验证：`cargo test -p eneros-did` 判据：篡改 signature 任一字节后 verify_certificate_chain 必 Err。
- 冲突声明：与蓝图/27「证书链校验强制」红线冲突（名义有、实际无）。

### SE-09（P1）mtls.toml 孤儿配置指向已删除实现——承诺的 mTLS 套件/吊销热加载无代码
- 类别：缺失功能 + 命名诚实性
- 定位：`configs/mtls.toml`（v0.115.0 模板）承诺 `MtlsContext`、`CertManager::load_crl`、套件 `SM2DHE-SM4GCM-SM3HMAC`/`SM2DHE-SM4CBC-SM3HMAC`、CRL 每日热加载。Grep 全仓：`MtlsContext`/`CertManager`/`load_crl` **零 Rust 引用**（eneros-mtls 已删除）；现有 tls crate 为 TLS 1.3（ECDSA P-256 + AES-128-GCM，embedded.rs），与配置承诺的国密套件完全不同。
- 根因：eneros-mtls 删除后配置未同步退役。
- 修复：二选一——① 删除 configs/mtls.toml，新建 `configs/tls.toml` 描述真实实现（roots 注入、VerifyRole、CRL 经 CertVerifier::set_crl、hostname 校验）；② 若国密 TLS 套件是交付承诺，则立项实现（当前判定为缺失功能）。推荐 ① 并同步 docs/security。
- 二次验证：消费方清点为零 → 删除零破坏；config-refs 门禁（若转阻断）需同步。
- host 验证：`grep -rn "mtls.toml" crates/ xtask/ ci/` 判据 0 命中后删除；`cargo xtask gate config-refs` 通过。
- 冲突声明：与蓝图/27 S 条目「L3 mTLS 双向认证层」交付声明冲突——文档/配置宣称的国密 mTLS 与真实实现（标准 TLS 1.3）不符，商用验收口径需重新锁定。

### SE-10（P1）IEC 62351 SecureChannel IV 计数器重启归零——同密钥 GCM nonce 复用风险
- 类别：BUG（密码学正确性）
- 定位：`crates/security/iec62351/src/secure_goose.rs:43,53,59-61` `iv_counter: u64` 每实例从 0 起；IV = counter(8B BE) ‖ key_id(4B BE)。配置注释（iec62351.toml 注释点 4）自称"计数器回绕前须轮换密钥"，但**未覆盖设备重启/密钥重装载场景**：同一 SessionKey 重新装载后 counter 归零 → 相同 (key, IV) 复用。
- 根因：IV 唯一性依赖进程内存状态，无持久化/无随机成分。
- 修复：SecureChannel::new/密钥装载时以 CsRng 取随机 4B 前缀并入 IV 构造（IV = random(4B) ‖ counter(4B) ‖ key_id(4B)），或 counter 起点持久化/取自单调时钟高位；`fn new(key, rng: &mut CsRng)` 签名级变更。
- 二次验证：消费方清点——secure_goose 用于 GOOSE 安全层（联邦保护协同通道）；IV 格式变更需双端同版本（帧格式已含 key_id，可扩展 IV 字段解析）。不变量：同密钥下 IV 全生命周期唯一。GCM nonce 复用可导出认证密钥，属最高优先密码学缺陷族。
- host 验证：`cargo test -p eneros-iec62351` 增补断言：同一 key 两次构造 channel，首帧 IV 不同。
- 冲突声明：与蓝图/27 密码学正确性红线冲突（nonce 复用）。

### SE-11（P1）Agent Bus 消息签名层零生产消费——跨域消息验签实际未接线
- 类别：缺失功能（孤儿配置）
- 定位：`crates/protocols/agent-bus-dds/src/signing.rs`（pack_and_sign/unpack_and_verify/Sm2Signer 全部 `#[cfg(feature = "sm2")]`）；Grep 全仓：无任何 Cargo.toml 启用 `features = ["sm2"]`，无任何生产代码调用 `pack_and_sign`/`unpack_and_verify`/`Sm2Signer`（消费方 grid_agent/twin-agent 均默认 features）。`configs/signing_keys.toml`（私钥路径、防重放窗口、对端公钥列表）零代码读取。
- 根因：签名层建成交付后未接线；`enable_sm2 = false` 默认 + MockSigner 叙事掩盖了"生产无签名"事实。
- 修复：商用交付二选一——① 接线：device-agent/agent 装配层启用 sm2 feature，装配时注入 KeyStore（私钥经安全存储），入口消息强制 unpack_and_verify（fail-closed）；② 若传输层已有等价保护（如全链路 TLS），则退役 signing.rs + signing_keys.toml 并在 docs 明示决策。禁止维持"有配置无实现"现状。
- 二次验证：接线后清点 agent-bus 全部入口（ingress 路径）确认无未验签消费旁路；不变量：入口消息验签失败即丢弃 + 审计。
- host 验证：接线后 `cargo test -p eneros-agent-bus-dds --features sm2` + 集成断言"篡改 payload → unpack_and_verify Err"。
- 冲突声明：与蓝图/27 纵深防御「消息完整性」层冲突（L 层宣称与实装不符）。

---

## 3. P2 发现（9 项）

### SE-12（P2）CsRng Drop 只清零 v 未清零 c——zeroize 不完整
- 类别：BUG（zeroize 疏漏）
- 定位：`crates/security/crypto/src/rng/csrng.rs:254-256` Drop 仅 `ct_zeroize(&mut self.v)`，DRBG 内部计数器 c 残留。
- 修复：Drop 内补 `ct_zeroize(&mut self.c)`（字段名以实际为准）。
- 二次验证：无消费面影响；不变量：Drop 后 DRBG 全部内部状态归零。
- host 验证：`cargo test -p eneros-crypto rng` 增补 Drop 后内存检查测试（构造于 Box、drop 后读裸字节判零）。
- 冲突声明：无。

### SE-13（P2）tls/src/lib.rs read_to_end 无界 Vec——DoS 面
- 类别：BUG（DoS）
- 定位：`crates/security/tls/src/lib.rs:338-340` `read_to_end` 无上限增长。
- 修复：`fn read_to_end_limited<R: Read>(r: &mut R, max: usize) -> std::io::Result<Vec<u8>>`，超限 Err；调用方传入协议级上限。
- 二次验证：消费方清点后逐个传上限；不变量：读超限即断，无 OOM。
- host 验证：单元测试以无限流注入判据 Err。
- 冲突声明：无。

### SE-14（P2）Secure104Session::handshake 空壳——"握手"仅置布尔位
- 类别：命名诚实性
- 定位：`crates/security/iec62351/src/secure_104.rs:125-131` `handshake()` 仅 `self.handshake_done = true`，入参 `let _ = rec;` 丢弃。
- 根因：104 安全层握手设计未落地，模块首行有诚实声明但函数名仍具误导性。
- 修复：更名 `mark_session_ready` + 文档明示"无密码学握手，密钥带外分发"，或立项真实握手（与 SE-10 修复协同）。
- 二次验证：消费方清点为测试；更名零破坏。
- host 验证：`cargo test -p eneros-iec62351` 全绿。
- 冲突声明：无（诚实性纪律项）。

### SE-15（P2）compare_versions 不可解析版本视为 Equal 放行——防降级弱口径
- 类别：BUG（fail-open 弱候选）
- 定位：`crates/security/ota-ab/src/upgrade.rs:54-66` 解析失败 → `Ordering::Equal`；`check_monotonic` 因此放行任意不可解析 target（文档明示"调用方把关"，但调用方无统一把关实现）。
- 修复：`check_monotonic` 对不可解析版本返回 `Err(OtaError::VerifyFailed)`（fail-closed），保留 compare_versions 原语义供展示用途。
- 二次验证：消费方清点——prepare_upgrade 唯一调用点；变更会使 "dev" 类版本串被拒，需确认 OTA 元数据版本串均为 X.Y.Z（UpgradeMeta 契约本就如此）。
- host 验证：`cargo test -p eneros-ota-ab` 更新 test_cry006 判据：`check_monotonic("dev", "0.123.0")` 必 Err。
- 冲突声明：无。

### SE-16（P2）SoftTpm::from_identity_seed 密钥材料未清零
- 类别：BUG（zeroize）
- 定位：`crates/security/attestation/src/tpm.rs:156` 派生路径的中间字节（seed 展开/sk 材料）无 ct_zeroize。
- 修复：派生完成后对局部 `sk_bytes`/中间缓冲 `ct_zeroize`（作用域结束前显式清零）。
- 二次验证：无接口变化；不变量：派生返回后栈上无密钥残留。
- host 验证：代码走查 + `cargo test -p eneros-attestation` 回归。
- 冲突声明：无。

### SE-17（P2）CRL 吊销表无签名与时效校验——吊销机制可伪造/可过期滥用
- 类别：缺失功能
- 定位：`crates/security/crypto/src/pki/crl.rs` 无签名字段（grep 实证）；`pki/verify.rs:197` 仅 `crl.is_revoked(serial)`，不验 CRL 完整性、不校验 nextUpdate 时效。
- 根因：CRL 为简化版（RFC 5280 子集），但商用交付的吊销能力需要防伪造。
- 修复：Crl 增补 `issuer_sig: Vec<u8>` + `next_update: u64`；`set_crl` 时用 CA 根公钥验 CRL 签名（sm2_verify/p256 按算法分支），`is_revoked` 前置时效检查（`now > next_update → Err(CrlExpired)`）。
- 二次验证：消费方清点——verify.rs + pentest.rs + tests/pki_test.rs；mtls.toml 承诺的 CRL 热加载（SE-09）修复时同步采用。不变量：伪造/过期 CRL 一律拒绝（fail-closed，拒绝时按"无吊销信息"还是"验证失败"取舍——安全口径取后者）。
- host 验证：`cargo test -p eneros-crypto pki` 增补：篡改 CRL 条目必验签失败；next_update 过期必 Err。
- 冲突声明：与蓝图/27「密钥吊销」商用能力红线相关。

### SE-18（P2）OTA 分区表持久化仅 CRC32 无签名——本地写权限攻击者可伪造 Active 标记
- 类别：缺失功能（完整性保护不足）
- 定位：`crates/security/ota-ab/src/rollback.rs:134-156` serialize 仅 magic + CRC32。
- 根因：CRC 只防意外损坏，不防恶意改写（block 0 写权限持有者可伪造分区表把 Failed 分区标 Active）。
- 损害面：需本地块设备写权限；真实启动仍受 secure-boot 验签约束（缓解因素），故 P2。
- 修复：PartitionTable 增补 SM3-HMAC 字段（密钥注入构造器），deserialize 验 HMAC；或声明威胁模型排除本地写攻击并文档锁定。
- 二次验证：消费方清点为 kernel_ota save/load；HMAC 密钥与 OTA root_key 同源派生即可。不变量：无密钥不能构造合法分区表。
- host 验证：`cargo test -p eneros-ota-ab` 增补：重算 CRC 但不带 HMAC 的伪造表必拒。
- 冲突声明：无。

### SE-19（P2）OtaDownloader 按未信任 total_size 预分配——内存耗尽面
- 类别：BUG（DoS）
- 定位：`crates/security/ota-ab/src/downloader.rs:80` `Vec::with_capacity(self.total_size as usize)`——total_size 来自服务端元数据，`as usize` 在 32 位平台还有截断风险。
- 修复：构造器校验 `total_size <= MAX_IMAGE_SIZE`（分区容量即上限，KernelOta 已有 partition_capacity 可注入）；with_capacity 取 `min(total_size, 64KiB)` 起步增量增长。
- 二次验证：消费方清点为 downloader 装配点；上限即分区容量，无功能损失。
- host 验证：`cargo test -p eneros-ota-ab` 增补：total_size 超上限构造即 Err。
- 冲突声明：无。

### SE-20（P2）secure_boot.toml 双变体 + debug_mode 验签旁路开关（安全域交叉引用）
- 类别：命名诚实性 + fail-open 隐患
- 定位：`configs/secure_boot.toml`（v0.160.0）与 `configs/secure-boot.toml`（v0.113.0）并存且字段口径不同；前者含 `debug_mode = false`（"跳过验签，仅开发环境"）。
- 现状：已由可观测/治理域报告 CF-02（P1）覆盖文件级处置；安全域在此补充**安全口径声明**——`debug_mode` 类验签旁路开关即使为 false 也不得存在于商用交付配置（攻击面=改一个字节）；且 secure_boot.toml 的 `on_failure = "halt"`、`require_tpm = false` 等策略字段零代码消费（真实策略在 secure-boot.toml 模板 + BootVerifier 注入）。
- 修复：随 CF-02 一并删除 secure_boot.toml；secure-boot.toml 永不引入任何"跳过验签"字段。
- 二次验证：Grep `debug_mode` 全仓零生产消费（已核实，仅 fleet-ops 漂移检测测试字符串），删除零破坏。
- host 验证：`grep -rn "secure_boot.toml\|debug_mode" configs/` 判据 0 命中。
- 冲突声明：与蓝图/27「验签强制、无旁路」红线相关（潜在）。

---

## 4. 统计与 P0 一行清单

| 严重度 | 数量 |
|---|---|
| P0 | 2 |
| P1 | 9 |
| P2 | 9 |
| 合计 | 20 |

**P0 一行清单**：
1. SE-01：eneros-attestation `tpm2` feature 编译失败（real.rs:123 TpmError::KeyUnavailable 不存在），真实 TPM2 路径不可用。
2. SE-02：secure-boot `verify_and_parse` 成功路径永不可达（恒 WrongStage），全仓消费未验签 parse()，信任链下级公钥绑定断裂（B4-02 失效）。

**与蓝图/27 冲突项**：SE-01/SE-03/SE-04（硬件能力探测+降级与远程证明证据链）、SE-02（Secure Boot 信任链完整性）、SE-05（S2 Mock 门控不可回退）、SE-08/SE-09/SE-11/SE-17（证书链校验、L3 mTLS 层宣称、消息完整性、密钥吊销的商用能力宣称与实装不符）、SE-10（密码学 nonce 唯一性）。

**已核查无发现**：crypto-gmssl（ADR-022 合规）、crypto hw 选择器/fallback、attest.rs/SoftTpm/event_log、audit 哈希链、model-guard/签名验证器、recovery flash/factory_reset、ota-ab 主状态机、tls/embedded.rs、控制写接缝封焊（详见 §0）。
