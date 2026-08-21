# 安全通道宣称边界（47 WP-47.5）

> 建立：2026-08-21 · 依据：蓝图 47 WP-47.5（安全通道一致性）+ 44-00 D4
> （v1.0 不宣称商密）· 机器校验：`protocol-abstract::channel::assembly_allows`
> （policy 与实现不一致 = 装配失败）。

## 产品通道用途表（D4 收敛）

| 用途 | 线 | 实现 | 状态 |
|---|---|---|---|
| 启动/引导链 | Ed25519 | ed25519-dalek | 产品（crypto-policy `boot_signature`） |
| 审计链 | SHA-256 | sha2 | 产品（`audit_hash`） |
| 安全通道 | P-256 ECDSA + AES-128-GCM（TLS 1.3） | embedded-tls（目标）/ rustls（host） | 产品（`channel_tls`；本卷 `EmbeddedTlsFactory` 第一实现） |
| OTA 分区表防篡改 | SM3-HMAC | eneros-crypto（SM3） | 产品（`ota_partition_tamper`）——**哈希用途，非商密宣称** |

## 明文与补偿控制（不冒充安全通道）

- **qemu-test 装配**：允许显式明文（`InsecureStream`，`negotiated_security()==Plain`
  永不冒充）；仅经 `ChannelPolicy.default_dev()` 放行。
- **release-tls 装配**：声明 SecureProfile（Tls12/Tls13）的协议必须有已注册
  `SecureChannelFactory`，否则 **装配失败**（`assembly_allows` 机器校验，
  fail-closed）。
- **GOOSE / SV（62351 现状）**：无 TLS 实现的广播订阅协议——**如实登记为
  明文 + 网络隔离补偿控制**（IEC 62351-6 现状：GOOSE/SV 明文字节流；部署补偿 =
  专网/隔离 VLAN/交换机组播受限，见 62351-6 部署章）。不声明安全通道能力；
  `SecurityProfile::None` 协议在 release-tls 按宣称边界允许（`assembly_allows`
  ReleaseTls + None → 允许），**不得**声称加密传输。
- **控制写（ControlDispatch 出站）**：协议 master 写路径在 qemu-test 为
  明文（演示）；release 经安全通道策略放行（`ChannelPolicy::default_release`
  禁明文）。

## 国密（SM2/3/4）宣称边界（D4）

- 手卷国密降级为**实验性能力**——不进 v1.0 商密宣称；维持 KAT 与 OpenSSL
  互操作向量（`crates/security/crypto/tests/*`）。
- in-product 过渡：config 验签 / OTA 验签仍经 SM2（42-CHG 批准 + review
  `review-status.json` 机器评审门）；**迁移（→ Ed25519 / SM3-HMAC）登记为
  48-52 卷 owner 项**（`crypto-policy.toml` 内 SM2 用途 `evaluation_status =
  "experimental"`）。
- 商密认证/送检：44-00 D4 明令 47/52 卷不得新增工作项。

## 机器校验点

- `SecureChannelRegistry`（protocol-abstract）：协议 id → `SecureChannelFactory`。
- `assembly_allows(mode, profile, has_factory, policy)`：ReleaseTls +
  SecureProfile 无工厂 → **拒绝装配**；QemuTest 显式明文允许（单测矩阵覆盖，
  `crates/protocols/protocol-abstract/src/channel.rs`）。
- 验签/签章分离：证据链验签钥独立注入（`ENEROS_VERIFY_PUBLIC_KEY`，WP-47.3）。

## 现有能力核对（诚实状态）

- `EmbeddedTlsFactory`（WP-47.5）经 host 环回测试（handshake/SAN/CN/policy），
  **目标侧 EL0 装配（embedded-tls over smoltcp）为后续卷接线项**；当前
  EL0 协议服务经 qemu-test 明文路径（登记，不冒充）。