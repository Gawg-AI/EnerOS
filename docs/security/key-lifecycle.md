# EnerOS 密钥生命周期管理

> 版本：v0.257.0（RC-3a，D3 安全评审准备）
> 依据：蓝图/14-阶段D-商用验收闸门.md D3「密钥生命周期」；
> 配套：docs/security/release-key-hierarchy.md（密钥层级/用途）、
>       docs/security/secure-boot-root-binding.md（信任根绑定）、
>       docs/security/entropy-profile.md（熵源）。

## 1. 密钥清单与生命周期总览

| 密钥 | 用途 | 生成 | 存储 | 使用 | 轮换 | 销毁 |
|---|---|---|---|---|---|---|
| 根签名私钥（SM2） | 固件/OTA 签名 | 离线仪式（HSM） | HSM/离线介质 | 仅发布仪式 | 根绑定流程（secure-boot-root-binding） | 物理销毁 |
| OTA 签名私钥 | 差分包签名 | 离线 | HSM | 发布 | 跟随根链 | 物理销毁 |
| EL0 插件签名私钥 | 插件签名 | 离线 | HSM | 发布 | 跟随根链 | 物理销毁 |
| attestation AK（SM2） | 远程证明 | 设备内（CSRNG） | TPM/SE | 证明挑战 | 证明期过期重生成 | TPM 清除 |
| mgmt-api Token 签发私钥 | 管理鉴权 | 设备外（管理侧） | 管理侧安全存储 | 签发 token | 周期轮换 | 安全擦除 |
| TLS 证书私钥 | 设备端 TLS | 设备内（CSRNG） | TPM/SE | 通信握手 | 证书过期前轮换 | TPM 清除 |
| 会话密钥（SM4） | 协议加密 | 设备内（密钥协商） | 内存 | 会话内 | 会话结束即弃 | 内存清零 |
| 存储加密密钥（SM4-XTS） | 落盘加密 | 设备内（CSRNG） | TPM/SE | 全盘 | 设备重置 | TPM 清除 |

## 2. 生命周期阶段

### 2.1 创建（Creation）

- **离线根密钥**：在发布仪式中经 HSM 生成（`xtask release` 仪式演练，
  docs/security/release-key-hierarchy.md）；私钥永不离开 HSM/离线介质。
- **设备内密钥**：一律经 `eneros-crypto::CsRng`（真实熵源：virtio-rng /
  片上 TRNG，见 entropy-profile.md）生成；禁止固定种子进生产路径
  （v0.245 已审计 CsRng::for_deterministic_signature 仅测试用）。

### 2.2 分发（Distribution）

- 公钥：经签名证书/点表分发；信任根公钥经安全启动根绑定流程固化。
- 私钥：无分发——设备内生成的永不出设备，离线的永不复制。

### 2.3 使用（Usage）

- 使用点全部经 `eneros-crypto` 统一 API（sm2_sign/sm3_hash/sm4_encrypt）；
- 私钥操作在 TPM/SE 内执行（tpm_integration.md）；无 TPM 时 SoftTpm
  仅测试/降级档（v0.236 显式标注非生产）；
- 密钥材料在内存中经 `ct_zeroize` 清零（v0.243 unsafe 审计覆盖）。

### 2.4 轮换（Rotation）

- 会话密钥：会话级轮换（协议层）；
- 证明密钥：按证明策略周期重生成；
- 根密钥：仅经 secure-boot-root-binding 文档定义的绑定流程（涉及
  信任根更新，需签名仪式 + 回滚保护联动）。

### 2.5 销毁（Destruction）

- 会话结束：内存清零（ct_zeroize）；
- 设备重置/退货：TPM 清除 + 存储密钥重生成；
- 物理介质：物理销毁（离线密钥）。

## 3. 与代码的映射

| 阶段 | 代码/文档证据 |
|---|---|
| 创建（设备内） | `crates/security/crypto/src/rng/`（CsRng + 熵源）、`crates/security/crypto/src/sm2/` |
| 创建（离线） | `xtask/src/release/`、`docs/security/release-key-hierarchy.md` |
| 使用 | `crates/security/crypto/src/constant_time/`（ct_zeroize）、`crates/security/tpm/` |
| 轮换 | `crates/security/iec62351/src/key_mgmt.rs`（会话密钥轮换）、`docs/security/secure-boot-root-binding.md` |
| 销毁 | `ct_zeroize` + TPM 清除路径（tpm_integration.md） |
| 审计 | `crates/security/audit/`（密钥操作审计链） |

## 4. 评审要点

1. 私钥不得以明文出现在仓库/镜像/支持包（脱敏引擎验证，v0.246）；
2. 设备内密钥生成熵源必须真实（v0.243 起 virtio-rng/TRNG 接入）；
3. 密钥轮换与回滚保护联动（降级密钥不得被接受）；
4. 销毁路径必须有验证（清零后可观测性测试）。
