# EnerOS 插件签名指南 (v0.166.0)

> **适用对象**：第三方插件开发者  
> **签名算法**：国密 SM2 (GB/T 32918.2-2017) + SM3 (GB/T 32905-2016)

---

## 1. 概述

所有 EnerOS 插件必须经过数字签名才能安装。签名确保：

- **来源可信**：插件由受信任 CA 签发的证书持有者签名
- **完整性**：插件二进制未被篡改（SM3 哈希保护）
- **不可否认**：签名者无法否认签名行为

## 2. 签名流程

```text
开发者                        EnerOS 插件市场
   │                               │
   ├─ 1. 开发插件 ELF              │
   ├─ 2. 编写 Manifest             │
   ├─ 3. SM3 哈希(binary)          │
   ├─ 4. SM2 私钥签名(hash)        │
   ├─ 5. 打包: binary + sig + cert │
   ├─ 6. 提交插件包 ──────────────►│
   │                               ├─ 7. 验证证书链
   │                               ├─ 8. SM2 验签
   │                               ├─ 9. 权限审查
   │                               ├─ 10. 沙箱测试
   │◄─ 11. 上架通知 ───────────────┤
```

## 3. 密钥管理

### 3.1 密钥生成

```bash
# 使用 EnerOS 密钥工具（规划中）
eneros-keygen --algorithm sm2 --output developer_key.pem

# 或使用 OpenSSL（需支持国密）
openssl ecparam -genkey -name SM2 -out developer_key.pem
openssl ec -in developer_key.pem -pubout -out developer_pub.pem
```

### 3.2 证书申请

1. 生成 SM2 密钥对
2. 创建证书签名请求（CSR）
3. 提交至 EnerOS 插件 CA
4. CA 审核通过后颁发开发者证书

### 3.3 密钥安全

| 规则 | 说明 |
|------|------|
| 私钥离线存储 | 签名操作在离线环境完成 |
| 禁止入仓 | `*.pem` / `*.key` 绝不提交 Git |
| 定期轮换 | 证书有效期建议 ≤ 2 年 |
| 泄露即吊销 | 通过 CRL 机制吊销证书 |

## 4. 插件包格式

### 4.1 包结构

```text
plugin.epp (EnerOS Plugin Package)
├── manifest.json      # 插件清单
├── plugin.elf         # AArch64 ELF 二进制
├── signature.bin      # SM2 签名 (64 字节: r‖s)
└── certificate.der    # 签名者证书 (DER 格式)
```

### 4.2 Manifest 字段

```json
{
  "name": "my-protocol-plugin",
  "version": "1.0.0",
  "author": "Developer Name",
  "plugin_type": "Protocol",
  "permissions": ["Network", "Ipc"],
  "min_sdk_version": "0.166.0"
}
```

### 4.3 插件类型

| 类型 | 说明 | 典型权限 |
|------|------|---------|
| `Protocol` | 协议插件 | Network, Ipc |
| `Driver` | 驱动插件 | DeviceAccess, MemoryMap |
| `Agent` | Agent 扩展 | Ipc, Timer, Network |
| `Algorithm` | 算法插件 | MemoryMap |
| `Generic` | 通用插件 | 按需 |

### 4.4 权限说明

| 权限 | 能力 | 说明 |
|------|------|------|
| `Network` | NetworkAccess | 访问网络栈（smoltcp） |
| `FileRead` | ReadFile | 读取文件系统 |
| `FileWrite` | WriteFile | 写入文件系统 |
| `DeviceAccess` | DeviceAccess | 访问硬件设备 |
| `Ipc` | SendIpc + RecvIpc | 进程间通信 |
| `Timer` | UseTimer | 使用定时器 |
| `MemoryMap` | — | 内存映射（无额外能力） |

## 5. 签名实现示例（Rust）

```rust
use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{sm2_sign, Sm2KeyPair};

// 1. 加载开发者密钥（生产环境从安全存储读取）
let keypair: Sm2KeyPair = load_developer_key();

// 2. 读取插件 ELF 二进制
let binary: Vec<u8> = read_plugin_elf("plugin.elf");

// 3. SM2 签名（内部自动 SM3 哈希 + Z 值计算）
let mut rng = CsRng::new(); // 生产环境接入硬件 TRNG
let signature = sm2_sign(&binary, &keypair.private_key, &keypair.public_key, &mut rng)
    .expect("signing failed");

// 4. 序列化为 64 字节
let sig_bytes: [u8; 64] = signature.to_bytes();

// 5. 打包插件
write_plugin_package(&binary, &sig_bytes, &certificate);
```

## 6. 验证流程（系统侧）

系统安装插件时自动执行：

1. **解析签名**：64 字节 → `Sm2Signature { r, s }`
2. **提取公钥**：从证书中获取 SM2 公钥（65 字节未压缩格式）
3. **信任检查**：公钥必须在受信任根列表中
4. **SM2 验签**：`sm2_verify(binary, signature, public_key)`
5. **权限审查**：权限数量 ≤ 16，类型合法
6. **SDK 兼容**：`min_sdk_version` ≤ 当前版本

任何一步失败 → 拒绝安装，返回对应 `PluginError`。

## 7. 常见错误

| 错误 | 原因 | 解决方案 |
|------|------|---------|
| `SignatureInvalid` | 签名与二进制不匹配 | 重新签名（确保二进制未修改） |
| `InvalidSignatureFormat` | 签名非 64 字节 | 检查签名序列化格式 |
| `UntrustedSigner` | 证书不受信任 | 向 EnerOS CA 申请证书 |
| `InvalidCertificate` | 公钥格式错误 | 使用未压缩格式（0x04 前缀） |
| `IncompatibleSdk` | SDK 版本过低 | 更新 `min_sdk_version` |

## 8. 安全最佳实践

1. **最小权限原则**：仅声明插件实际需要的权限
2. **签名后勿改**：签名后任何二进制修改都会导致验签失败
3. **使用硬件 RNG**：生产环境签名必须使用硬件随机数源
4. **证书链完整**：提交包含完整证书链的插件包
5. **定期更新**：关注安全公告，及时更新签名密钥
