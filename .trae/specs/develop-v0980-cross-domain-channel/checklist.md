# Checklist — v0.98.0 跨域通信通道 + v0.98.1 纵向加密认证

> Spec：`spec.md`（develop-v0980-cross-domain-channel）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: eneros-crypto 增量 `src/sm3/hmac.rs`（既有 crate 内）；eneros-federation 扩展 `src/channel.rs` / `src/tunnel.rs`（既有 crate 内），均未新增根目录 crate
- [x] C2: 根 `Cargo.toml` workspace 成员无新增（eneros-federation / eneros-crypto 已为成员），workspace 仍可解析
- [x] C3: eneros-federation `Cargo.toml` path 引用 `../../security/crypto`（相对路径正确）
- [x] C4: 新文档 `cross-domain-channel-design.md` / `vertical-encrypt-design.md` / `vertical-encrypt-compliance.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-crypto`（含新增 hmac 测试）全部通过；`cargo test -p eneros-federation`（含 channel/tunnel 新增测试）全部通过
- [x] C8: `cargo build -p eneros-federation -p eneros-crypto --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy -p eneros-federation -p eneros-crypto --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；子模块不重复加 no_std attr；测试模块内 `std::` 位于 `#[cfg(test)]` 下允许）

## D. eneros-crypto SM3-HMAC 增量（C16~C25）

- [x] C16: `hmac_sm3(key, msg) -> [u8;32]` 一次性接口符合 RFC 2104（key > 64 字节先 SM3 压缩，ipad/opad 填 64 字节）
- [x] C17: `Sm3Hmac` 流式接口与一次性接口结果一致（同 key/msg 输出相同）
- [x] C18: `Sm3Hmac::Drop` 恒定时间清零内部状态（密钥 zeroize 硬约束）
- [x] C19: `sm3/mod.rs` 仅追加 `pub mod hmac;` 1 行，既有代码零改动
- [x] C20: 已知答案测试通过（至少 1 组 key/msg 的 HMAC-SM3 值可独立复算）
- [x] C21: key 长度边界 0/16/63/64/65/100 字节均不 panic 且输出正确
- [x] C22: 不同 key 或不同 msg 输出不同（区分性）
- [x] C23: 空消息 `hmac_sm3(b"key", b"")` 不 panic 且确定性
- [x] C24: `Sm3Hmac` 多次 update 与拼接后 hash 结果一致（update 语义正确）
- [x] C25: eneros-crypto 既有全部测试（sm2/sign/encrypt/keypair + sm3/hash + sm4/cbc/gcm + rng/csrng + pki 等）全通过（零回归）

## E. channel.rs 数据结构与错误（C26~C35）

- [x] C26: `ChannelError { HandshakeFailed, CertInvalid, ConnectionRefused, UnknownNode, CryptoFailed, TransportFailed }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C27: `TlsConfig { ca_cert, client_cert, client_key, use_sm }` 派生 Debug/Clone/PartialEq，字段全 pub
- [x] C28: `TlsConfig::validate()` 证书/密钥任一 `is_empty()` → `Err(CertInvalid)`；全非空 → Ok
- [x] C29: `Endpoint { node_id: u64, addr: core::net::SocketAddr, established: bool, session_key: [u8;16], send_seq: u64 }` 派生 Debug/Clone/PartialEq，字段全 pub
- [x] C30: `SecureTransport` 为 sync trait（send/recv，无 async，无 Send+Sync）
- [x] C31: `MockSecureTransport` send 故障注入语义正确（fail_send_times 递减 → Err(ConnectionRefused)；0 后入 sent 队列）
- [x] C32: `MockSecureTransport` recv 语义正确（按 node_id 弹出 inbox 队首，空 → Err(TransportFailed)）
- [x] C33: `FederationChannel` 字段全 pub（tls/verifier/transport/rng/endpoints/connect_count/call_count/handshake_fail_count/crypto_fail_count）
- [x] C34: `new(tls, verifier, transport, rng)` 时 endpoints 空、4 计数器全零
- [x] C35: `core::net::SocketAddr` 使用（no_std 原生）

## F. channel.rs 连接与通话（C36~C50）

- [x] C36: `connect`：validate 失败 → Err(CertInvalid)、计数器不变
- [x] C37: `connect`：transport.send 失败 → handshake_fail_count+=1 + Err(ConnectionRefused)
- [x] C38: `connect`：transport.recv 失败 → handshake_fail_count+=1 + Err(HandshakeFailed)
- [x] C39: `connect`：verifier.verify 失败 → handshake_fail_count+=1 + Err(CertInvalid)
- [x] C40: `connect`：成功路径 → connect_count+=1、push Endpoint（established=true, send_seq=0）
- [x] C41: `derive_session_key(init_cert, resp_cert, nonce)` 输出 `[u8;16]` 且 `handle_hello` 应答方复算一致（双方同密钥）
- [x] C42: `handle_hello(hello, own_cert)` 帧格式错 → Err(HandshakeFailed)；格式正确 → Ok((对端 cert, nonce, 应答帧))
- [x] C43: `call` 未知 node_id → Err(UnknownNode)、计数器不变
- [x] C44: `call` endpoint established=false → Err(ConnectionRefused)
- [x] C45: `call` 成功路径 → send_seq+=1、SM4-GCM 加密（nonce=`0u32‖seq_be`，aad=`node_id_be‖seq_be`）、call_count+=1 + Ok(明文)
- [x] C46: `call` 篡改 tag → GCM TagMismatch → crypto_fail_count+=1 + Err(CryptoFailed)
- [x] C47: `disconnect(node_id)` → 移除 endpoint，后续 call 该 node_id → UnknownNode
- [x] C48: `reconnect(node_id)` 按已存 addr 重走 connect 流程（握手全链路语义）
- [x] C49: Mock 回环测试：connect→call 往返明文一致（双方共享 session_key 复算）
- [x] C50: channel 内嵌测试 TC1~TC40 全部通过

## G. tunnel.rs 数据结构与 IKE（C51~C65）

- [x] C51: `EncryptError { HandshakeFailed, CertInvalid, ReplayDetected, DeviceError, TagMismatch, InvalidFrame, UnknownTunnel }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C52: `TunnelKeys` **不派生 Debug**（密钥防泄露硬约束），派生 Clone/PartialEq，`Drop` 恒定时间清零
- [x] C53: `VerticalEncryptTunnel` 字段全 pub（local_spi/remote_spi/keys/send_seq/recv_seq/replay_bitmap）
- [x] C54: `initiator_hello` 输出帧含 `spi_offer:u32‖SM2加密‖SM2签名`，同时输出 PMS[32]
- [x] C55: `responder_accept` 解密失败 → Err(HandshakeFailed)；验签失败 → Err(CertInvalid)
- [x] C56: `initiator_finish` 验签成功 → Ok(spi_answer)；签名篡改 → Err(CertInvalid)
- [x] C57: `derive_tunnel_keys(pms, spi_l, spi_r)` 输出 `TunnelKeys`，双方输入对称时结果一致
- [x] C58: IKE 全链路：initiator_hello → responder_accept → initiator_finish → 双方 derive_tunnel_keys 输出相同
- [x] C59: IKE 篡改 PMS/签名/帧 → 协商在某步失败（HandshakeFailed/CertInvalid）
- [x] C60: `DispatchToken { payload, signature, expires_ms }` 派生 Debug/Clone/PartialEq
- [x] C61: `AuthResult { Granted, Denied, Expired }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C62: `verify_dispatch_auth` 过期判定先于验签（now_ms >= expires_ms → Expired，不验签）
- [x] C63: `verify_dispatch_auth` 未过期 + 签名正确 → Granted；签名错误 → Denied
- [x] C64: `VerticalEncryptDevice` 为 sync trait（xmit/poll，无 async，无 Send+Sync）
- [x] C65: `MockVerticalEncryptDevice` xmit 故障注入语义正确（fail_times 递减 → Err(DeviceError)，0 后入 xmitted）

## H. tunnel.rs 隧道收发与管理（C66~C80）

- [x] C66: `tunnel_send` 帧格式：`local_spi‖seq:u64‖iv[16]‖SM4-CBC(iv, plaintext)‖SM3-HMAC(auth_key, spi‖seq‖iv‖ct)`
- [x] C67: `tunnel_recv` 帧格式错 → Err(InvalidFrame)
- [x] C68: `tunnel_recv` remote_spi 不匹配 → Err(InvalidFrame)
- [x] C69: `tunnel_recv` SM3-HMAC 校验失败 → Err(TagMismatch)（恒定时间比较）
- [x] C70: `tunnel_recv` 重放检查：同 seq 二次接收 → Err(ReplayDetected)
- [x] C71: `tunnel_recv` 重放检查：seq <= recv_seq - 64 → Err(ReplayDetected)
- [x] C72: `tunnel_recv` 成功 → 更新 recv_seq 与 replay_bitmap（64 位滑动窗口位图正确）
- [x] C73: `rotate(new_keys)` → 原位替换 keys、send_seq/recv_seq/replay_bitmap 清零
- [x] C74: `TunnelManager` 字段全 pub（tunnels/BTreeMap + 4 计数器）
- [x] C75: `TunnelManager::send` 未知 spi → Err(UnknownTunnel)；成功 → send_count+=1
- [x] C76: `TunnelManager::recv` 未知 spi → Err(UnknownTunnel)；ReplayDetected → replay_reject_count+=1；成功 → recv_count+=1 + (spi, 明文)
- [x] C77: tunnel 内嵌测试 TV1~TV40 全部通过
- [x] C78: tunnel 双端互通测试：A tunnel_send → B tunnel_recv（明文一致）
- [x] C79: tunnel HMAC 篡改测试：改帧任一 byte → TagMismatch（恒定时间不泄露位置）
- [x] C80: tunnel rotate 后旧帧不可再用（新 keys 旧 tag 校验失败）

## I. 配置与文档（C81~C92）

- [x] C81: `configs/federation-channel.toml` 存在，`[channel]` 段完整
- [x] C82: federation-channel.toml 中文注释 ≥6 点且与 spec 一致（mTLS 双向 / 往返 <50ms / 会话密钥派生 / 帧格式 / 计数器 / 证书轮换）
- [x] C83: `configs/vertical-encrypt.toml` 存在，`[vertical_encrypt]` 段完整
- [x] C84: vertical-encrypt.toml 中文注释 ≥6 点且与 spec 一致（36 号文 / 重放窗口 / 密钥轮换 / PMS / 装置适配 / 吞吐 ≥10Mbps）
- [x] C85: `docs/agents/cross-domain-channel-design.md` 存在，12 章节齐全
- [x] C86: cross-domain-channel-design.md 含 2 个 Mermaid 图（握手+加密时序图、connect/call 决策流程图）
- [x] C87: cross-domain-channel-design.md 含 D1~D12 偏差表与 spec 一致
- [x] C88: `docs/agents/vertical-encrypt-design.md` 存在，12 章节齐全
- [x] C89: vertical-encrypt-design.md 含 2 个 Mermaid 图（IKE+隧道流程图、帧处理流程图）
- [x] C90: vertical-encrypt-design.md 含 E1~E12 偏差表与 spec 一致
- [x] C91: `docs/agents/vertical-encrypt-compliance.md` 含《纵向加密对接指南》与《合规控制点矩阵》
- [x] C92: 全部文档接口契约与实现签名一致（EX 偏差已声明）

## J. 版本同步（C93~C96）

- [x] C93: 根 `Cargo.toml` `[workspace.package] version = "0.98.0"`
- [x] C94: `Makefile` 版本注释同步 0.98.0
- [x] C95: `.github/workflows/ci.yml` 版本注释同步 0.98.0
- [x] C96: `ci/src/gate.rs` 注释追加 v0.98.x 类型清单（新增 channel/tunnel/hmac 类型）

## K. 测试覆盖（C97~C110）

- [x] C97: SM3-HMAC 内嵌测试 ≥10 个且全通过
- [x] C98: channel 内嵌测试 TC1~TC40 全部实现并通过
- [x] C99: tunnel 内嵌测试 TV1~TV40 全部实现并通过
- [x] C100: HMAC-SM3 一次性 vs 流式一致性验证
- [x] C101: channel Mock 回环 connect→call 往返明文一致
- [x] C102: channel 证书拒绝 / 握手失败 / 篡改 tag / 未知节点 全路径覆盖
- [x] C103: tunnel IKE 全链路协商 + 篡改拒绝覆盖
- [x] C104: tunnel 重放攻击 3 类覆盖（同帧二次 / 窗口内重复 / 超窗旧 seq）
- [x] C105: tunnel DispatchToken 3 状态覆盖（Granted / Denied / Expired）
- [x] C106: TunnelManager 路由 / UnknownTunnel / 计数器覆盖
- [x] C107: 所有测试无 `std::*` 违规（no_std 合规）
- [x] C108: 回归零破坏：eneros-cloud-coordinator（80）/ eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过
- [x] C109: `cargo test -p eneros-federation` 全部通过（含 membership/discovery 既有 40 + channel 40 + tunnel 40 ≈ 120）
- [x] C110: `cargo test -p eneros-crypto` 全部通过（既有 + hmac 增量）

## L. 蓝图达成（C111~C120）

- [x] C111: v0.98.0 交付物全覆盖：TlsConfig / Endpoint / ChannelError / SecureTransport trait / Mock / FederationChannel（connect/call/disconnect/reconnect）/ session_key 派生 / handle_hello / SM4-GCM 加密帧
- [x] C112: v0.98.0 mTLS 双向认证：握手帧含证书 + CertVerifier 验证 + 派生会话密钥（双向同密钥）
- [x] C113: v0.98.0 加密通话：SM4-GCM（nonce 逐 seq 唯一 + aad）+ TagMismatch 防篡改
- [x] C114: v0.98.0 可观测性：connect_count / call_count / handshake_fail_count / crypto_fail_count 4 计数器
- [x] C115: v0.98.0 可靠：disconnect + reconnect 支持断连重连
- [x] C116: v0.98.1 交付物全覆盖：IkeSession / TunnelKeys（禁 Debug）/ VerticalEncryptTunnel / tunnel_send/tunnel_recv / rotate / DispatchToken / AuthResult / VerticalEncryptDevice / Mock / TunnelManager
- [x] C117: v0.98.1 SM2 IKE：PMS 加密 + 签名验证 + 双方同密钥派生
- [x] C118: v0.98.1 SM4 密文隧道：CBC 加密 + SM3-HMAC 认证 + 重放窗口 64 位滑动位图
- [x] C119: v0.98.1 调度主站合规：verify_dispatch_auth 过期判定 + SM2 验签（36 号文）
- [x] C120: 无 BREAKING：既有全部 crate 零改动，既有公共 API 全保留；下游解锁 v0.99.0 联邦共识协议
