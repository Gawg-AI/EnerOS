# v0.98.0 跨域通信通道（gRPC + mTLS）+ v0.98.1 纵向加密认证 Spec

> 蓝图：`蓝图/phase2.md` v0.98.0（P2-E 第 2 版）+ v0.98.1（刚性合规子版本）。
> 按项目硬规则「0.98.x 下所有版本同一任务完成」，本 spec 覆盖 v0.98.0 与 v0.98.1。

## Why

v0.97.0 完成联邦发现（成员基础），蓝图 v0.98.0 要求实现 **Edge Coordinator 间跨域加密通信通道**（mTLS 双向认证 + 国密 SM2/SM3/SM4），防窃听篡改，为 v0.99.0 联邦共识提供安全通道。刚性子版本 v0.98.1（36 号文合规）在此基础上补齐 **纵向加密认证**（调度主站合规接入）：SM2 IKE 密钥协商 + SM4 密文隧道 + 重放保护，是 Phase 2 安全合规出口条件。

## What Changes

- **eneros-crypto 纯增量**（E11）：新增 `src/sm3/hmac.rs` — `hmac_sm3(key, msg) -> [u8;32]` 一次性接口 + `Sm3Hmac` 流式结构（RFC 2104 HMAC，底层 SM3）；`src/sm3/mod.rs` 仅加 `pub mod hmac;` 1 行声明。**既有代码零改动**。
- **eneros-federation 扩展**（既有 crate 追加 2 模块，membership.rs / discovery.rs 零改动）：
  - `Cargo.toml`：`[dependencies]` 追加 `eneros-crypto = { path = "../../security/crypto" }`（workspace 既有 crate，D9）
  - `src/channel.rs`（v0.98.0 新增）— `TlsConfig` / `Endpoint` / `ChannelError` / `SecureTransport` trait / `MockSecureTransport` / `FederationChannel`（connect 双向认证握手 + SM4-GCM 加密 call + 4 计数器）
  - `src/tunnel.rs`（v0.98.1 新增）— `VerticalEncryptTunnel` / `TunnelKeys` / `IkeSession`（SM2 IKE）/ `VerticalEncryptDevice` trait / `MockVerticalEncryptDevice` / `DispatchToken` / `AuthResult` / `TunnelManager` / `EncryptError`
  - `src/lib.rs`：`pub mod channel; pub mod tunnel;` + 重导出 + crate 文档升级为 v0.97.0+v0.98.0/v0.98.1 双版本说明与偏差表
- 新增 `configs/federation-channel.toml`（v0.98.0 证书与通道配置）、`configs/vertical-encrypt.toml`（v0.98.1 证书/隧道策略配置）
- 新增 `docs/agents/cross-domain-channel-design.md`（v0.98.0，12 章节 + 2 Mermaid + D1~D12）
- 新增 `docs/agents/vertical-encrypt-design.md`（v0.98.1，12 章节 + 2 Mermaid + E1~E12）
- 新增 `docs/agents/vertical-encrypt-compliance.md`（v0.98.1，《纵向加密对接指南》+《合规控制点矩阵（纵向加密部分）》）
- 根目录 4 文件版本同步 0.97.0 → 0.98.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 ~90 个（hmac ~10 + channel ~40 + tunnel ~40）
- **无 BREAKING**：既有全部 crate 公共 API 零改动

## Impact

- Affected specs：无既有 spec 受影响；关联 develop-v0970-federation-discovery（复用 CertVerifier）、develop-v0310-crypto-sm（扩展 hmac）
- Affected code：`crates/security/crypto/src/sm3/`（纯增量）、`crates/agents/federation/`（2 新模块）、`configs/`、`docs/agents/`、根 4 文件
- 依赖：eneros-federation 新增 path 依赖 eneros-crypto（既有 workspace crate）；**零新增第三方依赖**，SBOM 不变
- 下游解锁：v0.99.0 联邦共识协议、v0.117.0 审计哈希链（SM3-HMAC 复用）、Phase 2 安全合规出口

## 偏差声明（v0.98.0：D1~D12）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/federation/src/{channel,tls,grpc_service}.rs` | 既有 `crates/agents/federation/src/channel.rs` 单模块（项目 §2.3.1 硬规则；tls/grpc_service 语义并入 channel：TlsConfig 纯数据 + SecureTransport 服务抽象，不过度拆分） |
| **D2** | `node_id: String` / `connect(target: &str)` | `node_id: u64` / `connect(node_id: u64, addr: SocketAddr)`（无堆字符串，v0.97.0 D2 惯例） |
| **D3** | `pub async fn connect/call` + tonic gRPC | sync 方法 + `SecureTransport` sync trait（no_std 硬规则禁 async；tonic 依赖 std/tokio/hyper，无法交叉编译 aarch64-unknown-none；真实 gRPC 栈由集成阶段 Agent Runtime 适配层以 `Box<dyn SecureTransport>` 注入，接口先行模式同 v0.97.0 D5/D6） |
| **D4** | `tonic::transport::ClientTlsConfig` / `Certificate::from_pem` | `TlsConfig { ca_cert, client_cert, client_key, use_sm }` 纯数据 + `validate()` 非空校验（PEM 解析/真实 TLS 握手后置集成） |
| **D5** | mTLS 证书验证（tonic 内部） | 复用 v0.97.0 `CertVerifier` trait 验证对端证书（§5.5 防重复造轮子；PKI v0.32.0 适配器后续注入） |
| **D6** | TLS 握手（真实 socket） | 确定性握手语义：hello 帧 `MAGIC[4]‖cert_len:u32‖cert‖nonce[32]` → 对端应答帧 → CertVerifier 验证 → 会话密钥 `SM3("eneros-ch-enc"‖init_cert‖resp_cert‖nonce)` 取前 16 字节（双方同序拼接可独立复算；`derive_session_key` 与 `handle_hello` 公开辅助函数支持应答方/回环双端测试） |
| **D7** | TLS record 层加密 | SM4-GCM 认证加密（eneros-crypto 既有 `Sm4Gcm`）：帧 `seq:u64‖ciphertext‖tag[16]`，nonce = `0u32‖seq_be`（12 字节逐 seq 唯一，GCM 安全），aad = `node_id_be‖seq_be` |
| **D8** | `use_sm: bool` 国密开关 | 纯配置字段保留（配置兼容/未来非国密 TLS 集成占位）；本版本仅国密路径（项目无 RSA/AES 实现，§5.6 国密合规），use_sm 不产生分支行为差异 |
| **D9** | 外部依赖 tonic | 零新增第三方依赖；path 依赖 eneros-crypto（既有 workspace crate，SBOM 不变） |
| **D10** | 错误仅"握手失败/证书过期"2 类 | `ChannelError { HandshakeFailed, CertInvalid, ConnectionRefused, UnknownNode, CryptoFailed, TransportFailed }`（6 变体最小完备：握手/证书/对端拒绝/未知节点/加解密失败/传输失败） |
| **D11** | 测试 `tests/mtls.rs` | crate 内嵌 `#[cfg(test)]` ~40 测试（v0.87.0~v0.97.0 项目惯例；Mock 故障注入覆盖握手失败/证书拒绝/篡改） |
| **D12** | §9 可观测"连接状态 metric" | 4 个 pub 计数器：`connect_count` / `call_count` / `handshake_fail_count` / `crypto_fail_count` |

## 偏差声明（v0.98.1：E1~E12）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **E1** | 纵向加密卡驱动/密钥协商/密文隧道为独立交付物 | 同 crate `tunnel.rs` 单模块（v0.98.1 为 v0.98.0 补充子版本，同属联邦安全通道族；crate 分组硬规则归 agents） |
| **E2** | 纵向加密卡驱动（真实硬件） | `VerticalEncryptDevice` sync trait（`xmit`/`poll`）+ `MockVerticalEncryptDevice` 回环（CI 无硬件；驱动语义=帧收发 seam，真实卡驱动现场适配注入） |
| **E3** | SM2/SM3 基于证书的 IKE | 最小两方密钥协商：发起方生成 32 字节 PMS（注入 CsRng）→ SM2 加密至对端公钥 + SM2 签名 + SPI 提议 → 应答方解密验签 → 双方独立派生 `TunnelKeys`（`encrypt_key = SM3("enc"‖PMS‖spi_l‖spi_r)[..16]`、`auth_key = SM3("auth"‖PMS‖spi_l‖spi_r)`）；完整 IKE 状态机/证书链后置集成 |
| **E4** | `cert: &Sm2Cert` | 证书 opaque bytes + 复用 `CertVerifier`；IKE 用 eneros-crypto 既有 `Sm2KeyPair`/`Sm2PublicKey`/`Sm2Signature`（不新造证书类型） |
| **E5** | `tunnel_recv -> HeaplessVec<u8, 1500>` | `Vec<u8>`（Agent Runtime 有用户堆 v0.11.0，alloc 可用；heapless 仅无堆场景，全项目惯例） |
| **E6** | tunnel_send（IV 来源未定义） | `tunnel_send(&mut self, plaintext, rng: &mut CsRng)`：随机 IV 由注入 RNG 生成（CBC 可预测 IV 不安全，测试用固定种子确定性复现，生产 `CsRng::from_seed` 接硬件 TRNG） |
| **E7** | `EncryptError { HandshakeFailed, CertInvalid, ReplayDetected, DeviceError }` | 补 `TagMismatch` / `InvalidFrame` / `UnknownTunnel`（7 变体最小完备：HMAC 校验失败/帧格式错/未知 SPI） |
| **E8** | `DispatchToken` / `AuthResult` 未定义结构 | `DispatchToken { payload: Vec<u8>, signature: Sm2Signature, expires_ms: u64 }`；`AuthResult { Granted, Denied, Expired }`；`verify_dispatch_auth(token, pk, now_ms)`：过期判定先于验签（过期不验签直接 Expired） |
| **E9** | `replay_window: u32` | u64 seq + 64-bit 滑动位图（IPsec 惯例窗口 64）：`recv_seq` 记录最大已收 seq，位图记录窗口内已收；`seq == 已收` 或 `seq <= recv_seq - 64` → ReplayDetected |
| **E10** | 与真实纵向加密装置互通测试 | Mock 双端回环互通测试替代（蓝图 §2 阻塞条件声明"无对端调度主站测试环境则无法验证互通"；真实装置互通为现场验收项，文档标注） |
| **E11** | `auth_key: Sm3HmacKey`（SM3-HMAC 未存在于算法库） | eneros-crypto 纯增量 `sm3/hmac.rs`（通用密码原语归属 crypto crate，§5.5；既有代码零改动 + 1 行 mod 声明；v0.117.0 审计哈希链复用） |
| **E12** | §5 难点"密钥更新与轮换" / §9"证书更新不影响隧道已有连接" | `VerticalEncryptTunnel::rotate(new_keys)` 原位替换派生密钥 + 重置重放窗口（隧道持有派生密钥而非证书，证书轮换天然不影响已有连接）；`TunnelManager` 按 local_spi 管理多隧道 + 4 计数器（established/send/recv/replay_reject） |

## ADDED Requirements

### Requirement: SM3-HMAC 消息认证码（eneros-crypto 增量）

系统 SHALL 在 `eneros_crypto::sm3::hmac` 提供（no_std + alloc，RFC 2104 HMAC，块长 64 字节）：
- `pub fn hmac_sm3(key: &[u8], msg: &[u8]) -> [u8; 32]` — 一次性计算：key > 64 字节先 SM3 压缩，ipad=0x36/opad=0x5C 填充至 64 字节，`SM3(opad‖SM3(ipad‖msg))`
- `pub struct Sm3Hmac` — 流式：`new(key)` / `update(&mut self, data)` / `finalize(self) -> [u8; 32]`，与一次性接口结果一致
- 密钥清零：`Sm3Hmac` 实现 `Drop` 恒定时间清零内部状态（项目记忆硬约束：密钥材料必须 zeroize）

#### Scenario: HMAC-SM3 正确性
- **WHEN** `hmac_sm3(b"key", b"hello")` 与 `Sm3Hmac::new(b"key")` 流式 update 同消息后 finalize
- **THEN** 两者输出一致且为 32 字节；不同 key 或不同 msg 输出不同；key 长度 0/16/64/65/100 字节均不 panic 且确定性

### Requirement: 跨域通信通道 FederationChannel（v0.98.0）

系统 SHALL 提供（`eneros_federation::channel`，no_std + alloc）：

**数据结构**（全部 Debug/Clone/PartialEq，字段全 pub）：
- `TlsConfig { ca_cert: Vec<u8>, client_cert: Vec<u8>, client_key: Vec<u8>, use_sm: bool }` + `validate(&self) -> Result<(), ChannelError>`（ca_cert/client_cert/client_key 任一空 → `Err(CertInvalid)`）
- `Endpoint { node_id: u64, addr: core::net::SocketAddr, established: bool, session_key: [u8; 16], send_seq: u64 }`
- `ChannelError { HandshakeFailed, CertInvalid, ConnectionRefused, UnknownNode, CryptoFailed, TransportFailed }`（Debug/Clone/Copy/PartialEq/Eq）

**抽象**：sync trait `SecureTransport { fn send(&mut self, node_id: u64, data: &[u8]) -> Result<(), ChannelError>; fn recv(&mut self, node_id: u64) -> Result<Vec<u8>, ChannelError>; }`（无 async、无 Send+Sync）；`MockSecureTransport { pub sent: Vec<(u64, Vec<u8>)>, pub inbox: BTreeMap<u64, Vec<Vec<u8>>>, pub fail_send_times: u32, pub fail_recv_times: u32 }`（fail_send_times>0 → Err(ConnectionRefused) 递减；recv 依次弹出 inbox 队首，空/故障注入 → Err(TransportFailed)）

**FederationChannel**（字段全 pub）：`{ tls: TlsConfig, verifier: Box<dyn CertVerifier>, transport: Box<dyn SecureTransport>, rng: CsRng, endpoints: Vec<Endpoint>, connect_count: u64, call_count: u64, handshake_fail_count: u64, crypto_fail_count: u64 }`：
- `new(tls, verifier, transport, rng)`（endpoints 空、4 计数器全零）
- `connect(&mut self, node_id: u64, addr: SocketAddr) -> Result<(), ChannelError>`：tls.validate() → rng 生成 nonce[32] → hello 帧 `MAGIC‖cert_len‖client_cert‖nonce` 经 transport.send（Err → handshake_fail_count+=1 + ConnectionRefused）→ transport.recv 应答帧（Err → handshake_fail_count+=1 + HandshakeFailed）→ 解析对端 cert → verifier.verify（Err → handshake_fail_count+=1 + CertInvalid）→ 派生 session_key → push Endpoint（established=true）→ connect_count+=1
- `pub fn derive_session_key(init_cert: &[u8], resp_cert: &[u8], nonce: &[u8; 32]) -> [u8; 16]`（SM3 折叠确定性，关联 pub fn 供应答方/测试复算）
- `pub fn handle_hello(hello: &[u8], own_cert: &[u8]) -> Result<(Vec<u8>, [u8; 32], Vec<u8>), ChannelError>`（应答方辅助：解析 hello → Ok((对端 cert, nonce, 应答帧 `MAGIC‖cert_len‖own_cert`))；帧格式错 → Err(HandshakeFailed)）
- `call(&mut self, node_id: u64, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError>`：查 endpoint（无 → UnknownNode；established=false → ConnectionRefused）→ send_seq+=1 → SM4-GCM(session_key) 加密（nonce=`0u32‖seq_be`，aad=`node_id_be‖seq_be`）→ 帧 `seq‖ct‖tag` transport.send → transport.recv 应答帧 → 解析 → GCM 解密（TagMismatch → crypto_fail_count+=1 + Err(CryptoFailed)）→ call_count+=1 + Ok(明文)
- `disconnect(&mut self, node_id) -> bool`（移除 endpoint）；`reconnect(&mut self, node_id) -> Result<(), ChannelError>`（按已存 addr 重握手，蓝图 §9 可靠：断连重连）

#### Scenario: 握手与加密通话（蓝图 §4.3）
- **WHEN** Mock verifier accept、transport inbox 预置合法应答帧（对端 cert），connect(node_id=2, addr)
- **THEN** `Ok(())`：connect_count==1、endpoint established、session_key == `derive_session_key(本地 cert, 对端 cert, nonce)`（测试内经 handle_hello 复算一致，双向同密钥）
- **WHEN** verifier reject
- **THEN** `Err(CertInvalid)`、handshake_fail_count==1、endpoints 空
- **WHEN** 已连接后 call(2, b"payload")（inbox 预置同密钥同 aad/nonce 加密的应答帧）
- **THEN** `Ok(明文)`、call_count==1、endpoint.send_seq==1；篡改应答帧 tag → `Err(CryptoFailed)`、crypto_fail_count==1
- **WHEN** call(99, _)（未连接节点）
- **THEN** `Err(UnknownNode)`，计数器不变

### Requirement: 纵向加密隧道（v0.98.1）

系统 SHALL 提供（`eneros_federation::tunnel`，no_std + alloc）：

**数据结构**：
- `TunnelKeys { encrypt_key: [u8; 16], auth_key: [u8; 32] }`（Clone/PartialEq；**不派生 Debug**，项目记忆硬约束：密钥不明文泄露；`Drop` 恒定时间清零）
- `VerticalEncryptTunnel { local_spi: u32, remote_spi: u32, keys: TunnelKeys, send_seq: u64, recv_seq: u64, replay_bitmap: u64 }`（字段全 pub）
- `EncryptError { HandshakeFailed, CertInvalid, ReplayDetected, DeviceError, TagMismatch, InvalidFrame, UnknownTunnel }`（Debug/Clone/Copy/PartialEq/Eq）
- `DispatchToken { payload: Vec<u8>, signature: Sm2Signature, expires_ms: u64 }`（Debug/Clone/PartialEq）、`AuthResult { Granted, Denied, Expired }`（Debug/Clone/Copy/PartialEq/Eq）

**IKE（IkeSession）**：
- `pub fn initiator_hello(local_kp: &Sm2KeyPair, peer_pk: &Sm2PublicKey, spi_offer: u32, rng: &mut CsRng) -> Result<(Vec<u8>, [u8; 32]), EncryptError>` — 生成 PMS[32]，hello = `spi_offer:u32‖SM2加密(peer_pk, PMS)‖SM2签名(SM3(PMS‖spi_offer))`，Ok((hello_frame, PMS))
- `pub fn responder_accept(hello: &[u8], own_kp: &Sm2KeyPair, peer_pk: &Sm2PublicKey, spi_answer: u32) -> Result<(Vec<u8>, [u8; 32]), EncryptError>` — 解密 PMS（失败 → HandshakeFailed）→ 验签（失败 → CertInvalid）→ Ok((answer_frame `spi_answer‖SM2签名(SM3(PMS‖spi_answer))`, PMS))
- `pub fn initiator_finish(answer: &[u8], local_kp: &Sm2KeyPair, peer_pk: &Sm2PublicKey, pms: &[u8; 32]) -> Result<u32, EncryptError>`（验签 → Ok(spi_answer)）
- `pub fn derive_tunnel_keys(pms: &[u8; 32], spi_l: u32, spi_r: u32) -> TunnelKeys`（SM3 域分离确定性派生，双方一致）

**隧道收发**：
- `VerticalEncryptTunnel::new(local_spi, remote_spi, keys)`（send_seq/recv_seq=0、replay_bitmap=0）
- `tunnel_send(&mut self, plaintext: &[u8], rng: &mut CsRng) -> Vec<u8>` — send_seq+=1，随机 IV[16]，帧 `local_spi‖seq:u64‖iv‖SM4-CBC(iv, plaintext)‖SM3-HMAC(auth_key, spi‖seq‖iv‖ct)`
- `tunnel_recv(&mut self, frame: &[u8]) -> Result<Vec<u8>, EncryptError>` — 解析（长度/格式错 → InvalidFrame）→ remote_spi 匹配（否 → InvalidFrame）→ SM3-HMAC 恒定时间校验（否 → TagMismatch）→ 重放检查（seq 已收或超窗 → ReplayDetected）→ CBC 解密 → 更新 recv_seq/位图 → Ok(明文)
- `rotate(&mut self, new_keys: TunnelKeys)`（原位换钥 + send_seq/recv_seq/replay_bitmap 清零，E12）
- `pub fn verify_dispatch_auth(token: &DispatchToken, pk: &Sm2PublicKey, now_ms: u64) -> AuthResult`（now_ms >= expires_ms → Expired；SM2 验签 payload 通过 → Granted，否则 → Denied）

**设备与管理**：
- sync trait `VerticalEncryptDevice { fn xmit(&mut self, frame: &[u8]) -> Result<(), EncryptError>; fn poll(&mut self) -> Result<Option<Vec<u8>>, EncryptError>; }`；`MockVerticalEncryptDevice { pub xmitted: Vec<Vec<u8>>, pub pending: Vec<Vec<u8>>, pub fail_times: u32 }`
- `TunnelManager { tunnels: BTreeMap<u32, VerticalEncryptTunnel>, established_count: u64, send_count: u64, recv_count: u64, replay_reject_count: u64 }`（字段全 pub）：`add(tunnel)` / `remove(local_spi) -> bool` / `send(local_spi, plaintext, rng) -> Result<Vec<u8>, EncryptError>`（无此 spi → UnknownTunnel；Ok → send_count+=1）/ `recv(frame) -> Result<(u32, Vec<u8>), EncryptError>`（按帧 spi 路由，无 → UnknownTunnel；ReplayDetected → replay_reject_count+=1；Ok → recv_count+=1 + (spi, 明文)）

#### Scenario: IKE 协商 + 密文隧道全链路（蓝图 §4.3）
- **WHEN** 双方 SM2 密钥对就绪，initiator_hello → responder_accept → initiator_finish → 双方 derive_tunnel_keys
- **THEN** 双方 TunnelKeys 完全一致；answer 验签失败（篡改）→ `Err(CertInvalid)`
- **WHEN** A 隧道（spi_l=100, spi_r=200）tunnel_send → B 隧道（spi_l=200, spi_r=100）tunnel_recv
- **THEN** `Ok(原文)`；同帧第二次 recv → `Err(ReplayDetected)`；篡改密文 → `Err(TagMismatch)`；乱序旧 seq（超窗）→ ReplayDetected
- **WHEN** token expires_ms=5000，now_ms=4999 验签通过
- **THEN** Granted；now_ms=5000 → Expired（不验签）；签名错 → Denied

### Requirement: 配置交付物

系统 SHALL 提供 `configs/federation-channel.toml`（`[channel]` 段：use_sm / cert 路径占位 / handshake 超时 / max_endpoints / 重连说明，中文注释 ≥6 点含 mTLS 双向强制 §7.3、往返 <50ms §7.2 集成阶段验收、会话密钥派生 D6、帧格式 D7、计数器可观测 D12、证书轮换提示 §4.4）与 `configs/vertical-encrypt.toml`（`[vertical_encrypt]` 段：spi 范围 / replay_window=64 / 密钥轮换间隔 / 隧道策略，中文注释 ≥6 点含 36 号文合规、重放窗口 E9、密钥轮换 E12、PMS 保护、装置适配 E2、吞吐 ≥10Mbps 现场验收 E10）。

## MODIFIED Requirements

### Requirement: workspace 集成与版本

- `crates/agents/federation/Cargo.toml`：`[dependencies]` 追加 `eneros-crypto = { path = "../../security/crypto" }`；description 升级为 v0.97.0+v0.98.0/v0.98.1 双版本
- `crates/agents/federation/src/lib.rs`：`pub mod channel; pub mod tunnel;` + 全量重导出（新增 19 项：TlsConfig / Endpoint / ChannelError / SecureTransport / MockSecureTransport / FederationChannel / TunnelKeys / VerticalEncryptTunnel / IkeSession（或等价 pub fn 组）/ EncryptError / DispatchToken / AuthResult / VerticalEncryptDevice / MockVerticalEncryptDevice / TunnelManager 等）+ crate 文档追加 v0.98.0/v0.98.1 说明与 D/E 偏差表
- `crates/security/crypto/src/sm3/mod.rs`：仅追加 `pub mod hmac;`（既有代码零改动）
- 根 `Cargo.toml`：`[workspace.package] version = "0.98.0"`；`Makefile` / `ci.yml` 版本注释同步；`ci/src/gate.rs` 注释串尾追加 v0.98.x 类型清单
- **既有 crate 公共 API 全部保留**（membership/discovery/crypto 既有函数签名零改动）

## REMOVED Requirements

无。
