# Secure Boot 设计文档 (v0.160.0)

> **版本**：v0.160.0  
> **状态**：已实现  
> **依赖**：v0.158.0 国密硬件加速 / v0.159.0 TPM 2.0

## 1. 概述

实现自研 Secure Boot 全链签名验证，从 Bootloader 到用户态组件逐级验签，篡改任何环节拒绝启动。

### 1.1 安全目标

- 防止恶意固件/内核/Agent 植入
- 满足等保 2.0 三级"安全边界"要求
- 全链国密算法（SM2 + SM3 + TPM）

## 2. 信任链架构

```
Boot ROM 信任根 (SM2 公钥)
    │ SM2 验签
    ▼
Bootloader ──── PCR Extend 0
    │ SM2 验签 (BL 公钥)
    ▼
Kernel ──────── PCR Extend 4
    │ SM2 验签 (stage_key)
    ▼
Runtime ─────── PCR Extend 2
    │
    ▼
启动成功 / 拒绝启动
```

## 3. 核心组件

| 组件 | 文件 | 职责 |
|------|------|------|
| RootOfTrust | root_of_trust.rs | 信任根（SM2 公钥 + TPM PCR 锚点） |
| BootVerifier | verifier.rs | 四级验签器 |
| EnerosSecureBoot | secure_boot.rs | Secure Boot 主逻辑 + TPM 集成 |
| BootChainReport | report.rs | 验证报告 |

## 4. 验证流程

1. 验证信任根有效性
2. Rom → Bootloader：信任根公钥验签
3. Bootloader → Kernel：安装 BL 公钥，验签
4. Kernel → Runtime：沿用 stage_key，验签
5. 每级验签后扩展 TPM PCR
6. 全部通过 → 启动；任何失败 → 拒绝

## 5. 错误处理

| 错误 | 处理 |
|------|------|
| HashMismatch | 镜像被篡改，拒绝启动 |
| SignatureInvalid | 签名无效，拒绝启动 |
| StaleImage | 防降级拒绝 |
| TPM 不可用 | 降级启动（pcr_extended = false） |

## 6. 配置

配置文件：`configs/secure_boot.toml`

## 7. 测试

- 单元测试：33 项
- 集成测试：8 项（含篡改/伪造/降级）
- 回归：v0.158.0/v0.159.0 通过
