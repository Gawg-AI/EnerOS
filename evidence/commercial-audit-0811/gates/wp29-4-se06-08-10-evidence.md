# WP-29.4 密码学与信任链 P1（SE-06/07/08/10）修复与证据

执行日期：2026-08-11。蓝图坐标：29 §6（WP-29.4）。

## SE-10（最高优先，nonce 复用）
- 定位：iec62351/src/secure_goose.rs IV 计数器每实例从 0 → 同密钥重装载后
  (key, IV) 复用，GCM nonce 复用可导出认证密钥。
- 修复：SecureChannel::new(session, rng) 注入 CsRng，构造时取 4B 随机前缀
  （iv_random 字段）；IV = random(4B) ‖ counter(4B BE) ‖ key_id(4B BE)。
  SecureGoose::new / SecureSv::new 同步加 rng 参数。
- 判据测试：sg20_same_key_reloaded_channel_iv_differs——同一 key 两次构造
  channel 首帧 IV 必须不同（修复前 counter 归零必复用）。SG11 断言更新。
- 消费方：零外部（grep 核实），仅 secure_sv 委托 + 内部测试。

## SE-06 SM4-GCM tag 丢弃
- 定位：crypto/src/hw/software.rs:167 encrypt 丢弃 GCM tag，decrypt 却期望
  尾部 16B tag → 加解密不对称，语义退化为无认证 CTR。
- 修复：Gcm 分支返回 ciphertext ‖ tag（与 decrypt 契约对齐）；Ctr 分支保持
  无 tag（文档注明无认证语义）。
- 判据：GCM 环回（ct.len == pt.len+16 且 decrypt 一致）+ 篡改尾 16B 必 Err
  （修复前 tag 从未参与认证，篡改无法被发现）。
- 二次验证：crypto 之外 Sm4Mode::Gcm/Ctr 零生产消费（grep 核实）→ 零破坏。

## SE-07 TPM seal/unseal 偏移差 10B
- 定位：tpm/src/driver.rs seal 返回完整 resp（含 10B TPM2 header），unseal
  读 sealed[0..4] 作句柄 → 偏移差 10B，seal 产物无法 unseal。
- 修复：seal 返回前剥离 header（strip_resp_header 提取为可测纯函数）；
  契约注释两端同步；模拟后端不受影响。
- 判据：seal_unseal_handle_offset_contract——模拟 TPM2_Create 响应
  （10B 头+4B newHandle+载荷），剥离后首 4B 句柄 == newHandle（host 可验证
  偏移契约；真实 TPM 路径为真机项）。

## SE-08 DID 证书链不验签
- 定位：did/src/verifier.rs verify_certificate_chain 只查有效期+长度；
  全零签名通过测试（:197 实证）。
- 修复：补链签名验证——root_public_key 验 chain.signature（签名材料 = 链
  字段定序编码 SM3：三 DID 的 method/network/id + valid_from/until）；
  失败 Err(InvalidSignature)。API 增 root_public_key 参数。
- **S8 决策明示（二选一落地为补实装）**：单锚模型——链整体由 root 签发；
  两级级联签发（issuer 独立密钥分别签发 subject）需链内嵌 issuer 公钥与
  双签名结构，当前 CertificateChain 类型不支持，文档明示不冒充。
- 判据：正例（root 真实签名链 Ok）+ 篡改任一字节 Err + 全零签名拒绝（修复
  前的'通过'输入）+ 张冠李戴 root 公钥 Err。

## 回归
- crypto 415 / tpm 52 / did 43 / iec62351 31 / attestation 33（+tpm2 33）
  全部通过，0 失败。
- DER 测试 cfg(feature="tpm2") 门控（依赖仅 tpm2 启用）。
