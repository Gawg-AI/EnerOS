# Tasks — v0.98.0 跨域通信通道 + v0.98.1 纵向加密认证

> Spec：`spec.md`（develop-v0980-cross-domain-channel）。蓝图：`蓝图/phase2.md` v0.98.0 + v0.98.1（刚性合规子版本同任务完成）。
> 全部 no_std + alloc 合规；eneros-crypto 纯增量（sm3/hmac.rs）；eneros-federation 追加 channel.rs / tunnel.rs；既有代码零改动。

- [x] Task 1: eneros-crypto 纯增量 — `src/sm3/hmac.rs`
  - [x] SubTask 1.1: `pub fn hmac_sm3(key: &[u8], msg: &[u8]) -> [u8; 32]`（RFC 2104：key > 64 先 SM3 压缩，ipad=0x36/opad=0x5C 补 64 字节，`SM3(opad‖SM3(ipad‖msg))`，复用既有 `Sm3Hasher`）
  - [x] SubTask 1.2: `pub struct Sm3Hmac` 流式（`new(key)` / `update(&mut self, data)` / `finalize(self) -> [u8; 32]`），与一次性接口结果一致；实现 `Drop` 恒定时间清零内部状态（密钥 zeroize 硬约束）
  - [x] SubTask 1.3: `src/sm3/mod.rs` 仅追加 `pub mod hmac;` 1 行（既有代码零改动）；crate 根重导出按需追加
  - [x] SubTask 1.4: 内嵌测试 TH1~TH10（已知答案/一次性 vs 流式一致 TH1~TH3；key 长度边界 0/16/63/64/65/100 TH4~TH7；不同 key/msg 区分 TH8~TH9；空消息 TH10）
  - 验证：`cargo test -p eneros-crypto hmac` 通过且既有 crypto 测试全过（零回归）

- [x] Task 2: eneros-federation 骨架扩展
  - [x] SubTask 2.1: `Cargo.toml` `[dependencies]` 追加 `eneros-crypto = { path = "../../security/crypto" }`；description 升级双版本
  - [x] SubTask 2.2: `src/lib.rs` 追加 `pub mod channel; pub mod tunnel;` + 新增类型全量重导出 + crate 文档追加 v0.98.0/v0.98.1 说明与 D1~D12/E1~E12 偏差表（既有 membership/discovery 文档与重导出保留）
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `src/channel.rs` — v0.98.0 跨域通信通道
  - [x] SubTask 3.1: `ChannelError { HandshakeFailed, CertInvalid, ConnectionRefused, UnknownNode, CryptoFailed, TransportFailed }`（Debug/Clone/Copy/PartialEq/Eq，D10）
  - [x] SubTask 3.2: `TlsConfig { ca_cert, client_cert, client_key, use_sm }`（Debug/Clone/PartialEq，字段全 pub）+ `validate()`（任一证书/密钥空 → Err(CertInvalid)）；`Endpoint { node_id: u64, addr: core::net::SocketAddr, established: bool, session_key: [u8;16], send_seq: u64 }`（Debug/Clone/PartialEq，字段全 pub）
  - [x] SubTask 3.3: sync trait `SecureTransport`（send/recv，无 async 无 Send+Sync，D3）+ `MockSecureTransport { sent, inbox: BTreeMap<u64, Vec<Vec<u8>>>, fail_send_times, fail_recv_times }`（send 故障注入 → Err(ConnectionRefused)；recv 弹出队首，空/故障 → Err(TransportFailed)）
  - [x] SubTask 3.4: `FederationChannel`（字段全 pub：tls/verifier/transport/rng/endpoints/4 计数器）+ `new(tls, verifier, transport, rng)`（计数器全零）
  - [x] SubTask 3.5: `connect(node_id, addr)`：validate → nonce[32]（注入 rng）→ hello 帧 `MAGIC[4]‖cert_len:u32‖client_cert‖nonce` send（Err → handshake_fail_count+=1 + ConnectionRefused）→ recv 应答帧解析（Err → handshake_fail_count+=1 + HandshakeFailed）→ verifier.verify 对端 cert（Err → handshake_fail_count+=1 + CertInvalid）→ `derive_session_key` → push Endpoint（established=true, send_seq=0）→ connect_count+=1
  - [x] SubTask 3.6: `pub fn derive_session_key(init_cert, resp_cert, nonce: &[u8;32]) -> [u8;16]`（`SM3("eneros-ch-enc"‖init‖resp‖nonce)` 前 16 字节，D6）；`pub fn handle_hello(hello, own_cert) -> Result<(Vec<u8>, [u8;32], Vec<u8>), ChannelError>`（应答方辅助：解析 → (对端 cert, nonce, 应答帧)；格式错 → Err(HandshakeFailed)）
  - [x] SubTask 3.7: `call(node_id, plaintext)`：UnknownNode/未 established → ConnectionRefused；send_seq+=1 → SM4-GCM(session_key)（nonce=`0u32‖seq_be`，aad=`node_id_be‖seq_be`）→ 帧 `seq‖ct‖tag` send → recv → GCM 解密（TagMismatch → crypto_fail_count+=1 + Err(CryptoFailed)）→ call_count+=1 + Ok(明文)；`disconnect(node_id) -> bool`；`reconnect(node_id)`（按已存 addr 重握手）
  - [x] SubTask 3.8: 内嵌测试 TC1~TC40（数据结构派生/validate TC1~TC6；Mock 故障注入 TC7~TC12；connect 成功/握手失败/证书拒绝/会话密钥双向一致（handle_hello 复算）TC13~TC24；call 成功/GCM 篡改/未知节点/未建立 TC25~TC34；disconnect/reconnect TC35~TC37；计数器累计 + connect→call→disconnect→reconnect 全链路 TC38~TC40）
  - 验证：`cargo test -p eneros-federation channel` 40 通过

- [x] Task 4: 实现 `src/tunnel.rs` — v0.98.1 纵向加密认证
  - [x] SubTask 4.1: `EncryptError { HandshakeFailed, CertInvalid, ReplayDetected, DeviceError, TagMismatch, InvalidFrame, UnknownTunnel }`（Debug/Clone/Copy/PartialEq/Eq，E7）；`TunnelKeys { encrypt_key: [u8;16], auth_key: [u8;32] }`（Clone/PartialEq，**禁 Debug**，Drop 恒定时间清零）
  - [x] SubTask 4.2: IkeSession pub fn 组（E3）：`initiator_hello(local_kp, peer_pk, spi_offer, rng) -> Result<(Vec<u8>, [u8;32]), EncryptError>`（PMS + SM2 加密 + SM2 签名帧）；`responder_accept(hello, own_kp, peer_pk, spi_answer, rng) -> Result<(Vec<u8>, [u8;32]), EncryptError>`（解密失败 → HandshakeFailed，验签失败 → CertInvalid；EX1 增加 rng 参数：SM2 应答签名需要随机 k 值）；`initiator_finish(answer, local_kp, peer_pk, pms) -> Result<u32, EncryptError>`；`derive_tunnel_keys(pms, spi_l, spi_r) -> TunnelKeys`（SM3 域分离 + SPI 排序，双方一致）
  - [x] SubTask 4.3: `VerticalEncryptTunnel { local_spi, remote_spi, keys, send_seq, recv_seq, replay_bitmap }`（字段全 pub）+ `new` / `tunnel_send(plaintext, rng) -> Vec<u8>`（帧 `spi‖seq‖iv‖SM4-CBC‖SM3-HMAC`）/ `tunnel_recv(frame) -> Result<Vec<u8>, EncryptError>`（InvalidFrame/TagMismatch 恒定时间/ReplayDetected 64 位滑动窗口 E9，IPsec 标准位图语义 EX4 修正）/ `rotate(new_keys)`（E12）
  - [x] SubTask 4.4: `DispatchToken { payload, signature, expires_ms }`（Debug/Clone/PartialEq）+ `AuthResult { Granted, Denied, Expired }` + `verify_dispatch_auth(token, pk, now_ms)`（过期先于验签 E8）
  - [x] SubTask 4.5: sync trait `VerticalEncryptDevice`（xmit/poll）+ `MockVerticalEncryptDevice { xmitted, pending, fail_times }`（E2）；`TunnelManager { tunnels: BTreeMap<u32, _>, established_count, send_count, recv_count, replay_reject_count }`（字段全 pub）+ add/remove/send/recv（UnknownTunnel；按帧 spi 路由；计数器累计 E12）
  - [x] SubTask 4.6: 内嵌测试 TV1~TV40（EncryptError/TunnelKeys 派生与禁 Debug TV1~TV4；IKE 双端协商/密钥一致/篡改拒绝 TV5~TV14；tunnel 帧格式/加解密往返/双端互通 TV15~TV22；重放攻击（同帧二次/窗口外旧 seq/边界 seq）TV23~TV28；HMAC 篡改 TV29~TV30；rotate 换钥 TV31~TV32；DispatchToken Granted/Denied/Expired TV33~TV36；Mock 设备 + TunnelManager 路由/UnknownTunnel/计数器 TV37~TV40）
  - 验证：`cargo test -p eneros-federation tunnel` 40 通过（1439 行，120/120 全 crate 通过，clippy 0 warning，fmt 通过）

- [x] Task 5: 新增配置文件 ×2
  - [x] SubTask 5.1: `configs/federation-channel.toml`（`[channel]` 段：use_sm / ca_cert_path / client_cert_path / client_key_path / handshake_timeout_ms / max_endpoints；中文注释 ≥6 点：mTLS 双向强制 §7.3 / 往返 <50ms §7.2 集成阶段验收 / 会话密钥派生 D6 / 帧格式 SM4-GCM D7 / 4 计数器可观测 D12 / 证书过期提示轮换 §4.4）
  - [x] SubTask 5.2: `configs/vertical-encrypt.toml`（`[vertical_encrypt]` 段：spi_start / spi_end / replay_window=64 / key_rotation_interval_ms / tunnel 策略；中文注释 ≥6 点：36 号文合规 / 重放窗口 E9 / 密钥轮换 E12 / PMS 内存保护 / 装置适配 E2 / 吞吐 ≥10Mbps 现场验收 E10）

- [x] Task 6: 新增文档 ×3
  - [x] SubTask 6.1: `docs/agents/cross-domain-channel-design.md`（12 章节 + 2 Mermaid：mTLS 握手+加密调用时序图、connect/call 决策流程图含 CertInvalid/HandshakeFailed/CryptoFailed 分支 + D1~D12 偏差表与 spec 一致 + 接口契约与实现签名一致）
  - [x] SubTask 6.2: `docs/agents/vertical-encrypt-design.md`（12 章节 + 2 Mermaid：IKE 协商+隧道建立流程图（含降级本地自治分支）、tunnel_send/tunnel_recv 帧处理与重放窗口流程图 + E1~E12 偏差表与 spec 一致）
  - [x] SubTask 6.3: `docs/agents/vertical-encrypt-compliance.md`（《纵向加密对接指南》章节：装置适配 seam/证书配置/隧道策略/现场互通验收步骤；《合规控制点矩阵（纵向加密部分）》章节：36 号文控制点 → 实现 → 验收证据映射表）

- [x] Task 7: 根目录版本同步 0.97.0 → 0.98.0
  - [x] SubTask 7.1: 根 `Cargo.toml` `[workspace.package] version = "0.98.0"`
  - [x] SubTask 7.2: `Makefile` 版本注释同步
  - [x] SubTask 7.3: `.github/workflows/ci.yml` 版本注释同步
  - [x] SubTask 7.4: `ci/src/gate.rs` 注释串尾追加 v0.98.x 类型清单（channel/tunnel 新增类型 + hmac）

- [x] Task 8: 构建验证（§2.4.2 全量）
  - [x] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 8.2: `cargo test -p eneros-federation`（120 全过）与 `cargo test -p eneros-crypto`（417 全过，含 13 个 hmac 测试）
  - [x] SubTask 8.3: `cargo build -p eneros-federation -p eneros-crypto --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.4: `cargo fmt --all -- --check` 通过
  - [x] SubTask 8.5: `cargo clippy -p eneros-federation -p eneros-crypto --all-targets -- -D warnings` 0 warning
  - [x] SubTask 8.6: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖；既有 spin 0.9.8 yanked warning 与本次无关）
  - [x] SubTask 8.7: 回归零破坏：eneros-cloud-coordinator（80）/ eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

- [x] Task 9: 按 `checklist.md` 逐项核验并勾选（120/120 通过，C92 追加 EX 偏差已声明标注）

# Task Dependencies

- Task 1 独立（eneros-crypto 增量），与 Task 2 可并行
- Task 3/4 依赖 Task 1（hmac/SM4/SM2 API）+ Task 2（模块声明）；Task 3 与 Task 4 可并行
- Task 5/6 与 Task 3/4 可并行（配置/文档）
- Task 7 依赖 Task 3/4 完成（类型清单定稿）
- Task 8 依赖 Task 1~7 全部完成
- Task 9 依赖 Task 8 通过
