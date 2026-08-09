# ADR-022 — 国密资质路线：OpenSSL-SM binding + no_std 双实现（21-B C1/C2）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 21-B §5 预设裁决 C1 方案 1 落地）
- **关联**：ADR-021（DDS→MQTT 降级）、ADR-023（QEMU-first transport）、`蓝图/21-B` §2.C1/C2

## 背景

`eneros-crypto`（`crates/security/crypto`）自述 `product_allowed = false`（`isolation_reason: 等待独立密码学评审`）——手卷 SM2/SM3/SM4 未审计是 v1.0.0 商业授权的法律前置阻塞。21-B §5 预设裁决：**C1 走方案 1（换 vetted 库）**——"eneros-crypto 改为 binding GmSSL/OpenSSL-SM，保留现有 API，内部实现替换；手卷代码降级为参考实现，仅 test"。

## 约束事实（侦察确认，代码为准）

1. `eneros-crypto` 为 **no_std** crate（`extern crate alloc`，无 std）；24 个消费者中，`secure-boot` / `ota-ab` / `tpm` / `attestation` / `did` / `control-gateway` 等关键链路均为 **no_std EL0 服务**（aarch64-unknown-none）。
2. GmSSL/OpenSSL 是 **C 库**（需要 libc/动态链接）。**no_std 目标（seL4 EL0）无法链接 C 库**——"内部实现替换为 binding"在目标侧不可执行。
3. 本机/CI 平台为 OpenSSL 3.5（`libssl-dev` 已装，`pkg-config --modversion openssl` = 3.5.5）；OpenSSL 3.x 原生提供 SM2/SM3/SM4（`openssl ecparam -name sm2`、`EVP_sm3`、`EVP_sm4_ecb` 实测可用）。GmSSL 无系统包，需源码编译（`third_party` 构建链 + 维护成本）。
4. 互操作桥接已验证：手卷 `Sm2Signer::new()` 默认 user_id = `"1234567812345678"`（国标默认，与 OpenSSL 默认 distid 一致）；公钥编码 `04||X||Y`（X9.62 未压缩）与 OpenSSL 兼容；签名 `r||s`（64B）↔ OpenSSL DER（ECDSA-Sig-Value）需双向转换。

## 决策

1. **binding 对端选 OpenSSL 3（libcrypto），不选 GmSSL 源码**。理由：蓝图明文允许 "OpenSSL-SM"；本机/CI 即装即用（系统包，经发行版审计）；GmSSL 需自建构建链且无包管理背书。影响：binding crate 依赖系统 `libssl-dev`（CI ubuntu-22.04 预装），KAT 对端为 openssl CLI（同库）。
2. **双实现架构（ADR 裁决，修订蓝图）**：
   - `eneros-crypto`（no_std 手卷）**保留**：是 seL4 EL0 目标侧唯一可链接的实现（no_std 约束不可绕开），`product_allowed = false` 维持（target 手卷未审计，不造假）。
   - 新增 `eneros-crypto-gmssl`（std binding）：vetted 算法实现，供 host 工具链 / 富 OS 服务 / 互操作验证对端。
   - **手卷正确性由 binding KAT 交叉验证**：openssl（vetted）生成第三方向量 → 手卷验签；手卷签名 → openssl 验证。这把 C2（第三方互操作 KAT）与 C1 绑定为一个联合交付。
   - 蓝图 `21-B §2.C1` 方案 1 文本修订：在 no_std 目标存在的前提下，"内部实现替换"修正为"std 面替换 + target 手卷保留并由 vetted binding 交叉验证"（单一事实源铁律：代码事实优先，改蓝图 + 记 ADR）。
3. **实现路径选"C 帮助层 + 薄 FFI"**：`gmssl_helpers.c`（cc 编译链接）封装
   SM2 签名/验签/SM3/SM4 的最小调用序列，Rust 侧仅声明 5 个 C 函数。
   理由：纯 Rust FFI（20+ 函数直接声明）在 2026-08-09 实测中与 C 调用
   存在隐蔽差异（同一 libcrypto、同一调用序列，C 成功而 Rust 失败，
   valgrind 无内存错误，libloading 手工声明同样失败——定位为 Rust 调用
   层与 C 编译调用的行为差异，未进一步深挖）；C 帮助层与已验证的
   C 探针逐字节同源，消除了 FFI 面过宽的不确定性。影响：unsafe 面收缩
   到 5 个简单函数，每个均带 SAFETY 注释（C-243 gate 覆盖）。

## 实测关键事实（2026-08-09，OpenSSL 3.5.5）

1. **SM2 密钥构造必须走 `EVP_PKEY_fromdata`**（`EVP_PKEY_new_raw_private_key`
   对 SM2 报 "EC_GROUP_new_from_params: invalid field"），且**必须显式提供
   公钥点**（SM2 keymgmt 不从 priv 自动派生 pub，Z 值计算需要公钥）。
2. **SM2 签名必须走 `EVP_DigestSign`**（自动计算 Z 与 e=SM3(Z||M)）；
   裸 `EVP_PKEY_sign` 要求调用方已算好 e。
3. **OpenSSL 3.5 SM2 provider 没有默认 distid**——未设置时
   `ctx->id == NULL`，Z 值不含用户 ID（ENTL=0），与 GB/T 默认
   "1234567812345678" **不互认**。CLI（`pkeyutl`）同样不设默认。
   **互操作必须显式设置 distid**（C 层 `set_params(OSSL_PKEY_PARAM_DIST_ID)`；
   CLI 加 `-pkeyopt distid:1234567812345678`）——本 ADR 的 KAT 向量
   与手卷实现（默认 user_id = 国标默认）因此一致。
4. `EVP_Cipher` 对块密码返回**处理字节数**（SM4-ECB 单块 = 16），非 1。
5. `OSSL_PARAM_BLD_push_BN` **引用** BN（`to_param` 时才复制值），
   BN 必须存活到 `to_param` 之后（Rust 与 C 层先后踩中此坑，
   已双处修复并由 valgrind + 官方向量 KAT 验证）。

## 替代方案（否决）

- **GmSSL 源码编译**：工程量大（third_party 构建链、版本管理）、无发行版审计背书；本机/CI 无现成包。
- **openssl crate**：SM2 API 版本敏感性（0.10.55+ 才有 Id::SM2，签名路径需实测）；增加一层依赖解析不确定性。
- **方案 2（走商密认证）**：周期不可控（送审排期），且 21-B §5 已预设否决；仅作长期并行项（认证通过后 target 手卷可获资质）。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| no_std target 手卷仍 `product_allowed=false`，授权文档需限定"国密合规场景"范围 | ADR 明示 + crypto Cargo.toml `isolation_reason` 更新（双实现描述）；长期走商密认证（方案 2 并行） |
| 手写 FFI 的 unsafe 面 | 每 FFI 函数 SAFETY 注释（参数校验、长度、生命周期）；KAT 覆盖真/假签名路径 |
| OpenSSL 版本漂移（3.5 vs 3.0） | 仅使用稳定 C API（EVP_PKEY_*、EVP_Digest、EVP_Cipher*）；CI 与本地同版 3.5 验证 |
| 向量文件被篡改 | 向量 JSON + SHA-256 锚定文件提交，CI KAT job 先 `sha256sum -c` 再跑测试 |

## 验收判据映射（21-B §2.C1/C2）

- [x] ADR 记录（本文，含选型理由/替代方案/风险）
- [x] binding crate 通过 SM2/SM3/SM4 第三方互操作 KAT（对 openssl 官方向量：
  SM3/SM4 GB/T 附录 KAT + SM2 10 组 openssl 向量双向互操作）
- [x] `crypto/tests/sm2_interop_kat.rs` 10 组第三方向量全部 PASS
  （openssl→手卷验签 + 手卷→openssl CLI 验签 + 篡改负向）
- [x] CI 新增 KAT job（g1-fuzz-gates），向量文件哈希锚定
  （`sha256sum -c vectors.json.sha256 --strict` 先于测试）
- [x] `product_allowed` 字段与决策一致（crypto 维持 false + isolation_reason
  更新为 ADR-022 双实现描述；crypto-gmssl = true，vetted binding）
