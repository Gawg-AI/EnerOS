# EnerOS 插件市场设计 (v0.166.0)

> **版本**：v0.166.0  
> **子系统**：crates/kernel/plugin（eneros-plugin）  
> **Phase 定位**：P3-J 生态开放  
> **前序依赖**：v0.165.0 Protocol/Driver SDK、v0.160.0 Secure Boot

---

## 1. 概述

插件市场框架是 EnerOS 生态核心，提供安全可控的第三方插件加载机制。通过 **签名验证 + 权限检查 + 沙箱隔离** 三重保护，确保插件来源可信、行为受限、崩溃隔离。

## 2. 架构

```text
┌─────────────────────────────────────────────────────────┐
│  eneros-plugin (v0.166.0)                               │
│                                                         │
│  ┌──────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │ types    │  │ signature    │  │ sandbox          │   │
│  │ (核心    │  │ (SM2/SM3    │  │ (内存/能力/      │   │
│  │  类型)   │  │  验签)       │  │  IPC/CPU 配额)   │   │
│  └────┬─────┘  └──────┬───────┘  └────────┬─────────┘   │
│       │               │                    │             │
│       │        ┌──────▼────────────────────▼──────────┐  │
│       │        │ manager (插件管理器)                  │  │
│       └───────►│ install / uninstall / enable / disable│  │
│                └──────────────────┬───────────────────┘  │
│                                   │                      │
│                ┌──────────────────▼───────────────────┐  │
│                │ loader (ELF64 AArch64 加载器)         │  │
│                └──────────────────────────────────────┘  │
│                                                         │
│  依赖：eneros-crypto (SM2/SM3/PKI)                      │
└─────────────────────────────────────────────────────────┘
```

## 3. 插件加载流程

```text
插件包(.epp) → 解析 Manifest → 验证签名(SM2) → 检查权限
  → 检查 SDK 兼容 → 创建沙箱 → ELF 加载 → 注册 → 运行
```

1. **解析 Manifest**：提取插件名称、版本、类型、权限声明
2. **验证签名**：SM3 哈希 + SM2 验签，签名者必须在受信任根列表
3. **检查权限**：权限数量 ≤ 16，所有声明权限须合法
4. **SDK 兼容**：`min_sdk_version` ≤ 当前 SDK 版本
5. **创建沙箱**：分配独立内存区域（默认 1MB），映射能力集
6. **ELF 加载**：解析 ELF64 AArch64 格式，加载 PT_LOAD 段
7. **注册**：分配 PluginId，状态置为 Loaded

## 4. 安全模型

### 4.1 签名验证

- 算法：国密 SM2（GB/T 32918.2-2017）+ SM3（GB/T 32905-2016）
- 签名格式：64 字节（r‖s）
- 信任锚：构造注入的受信任 CA 根公钥列表
- 复用 `eneros-crypto` 实现（零第三方依赖）

### 4.2 沙箱隔离

| 维度 | 机制 | 默认配额 |
|------|------|---------|
| 内存 | 独立 MemoryRegion，越界检测 | 1 MB |
| 能力 | 权限→能力映射，未声明不可用 | 按 Manifest |
| IPC | 消息数/秒 + 消息大小限制 | 100 msg/s, 4 KB/msg |
| CPU | 时间片配额 | 10 ms/周期 |

### 4.3 崩溃隔离

- 插件崩溃 → 标记 `Crashed` → 沙箱停用 → 不影响宿主
- 支持 `restart()` 从崩溃恢复（重置配额，状态回 Loaded）

## 5. 插件生命周期

```text
         install()
            │
            ▼
  ┌──── Loaded ◄──── restart()
  │         │                ▲
  │  enable()               │
  │         ▼                │
  │     Enabled ──── mark_crashed() → Crashed
  │         │
  │  disable()
  │         ▼
  └──► Disabled
            │
     uninstall()
            │
            ▼
        [移除]
```

## 6. 错误模型

| 错误 | 触发条件 |
|------|---------|
| `SignatureInvalid` | SM2 验签失败 |
| `InvalidSignatureFormat` | 签名非 64 字节 |
| `InvalidCertificate` | 证书公钥格式错误 |
| `UntrustedSigner` | 签名者不在信任列表 |
| `PermissionDenied` | 权限声明过多/非法 |
| `IncompatibleSdk` | SDK 版本不兼容 |
| `SandboxViolation` | 内存越界/能力滥用 |
| `QuotaExceeded` | IPC/CPU 配额耗尽 |
| `PluginCrashed` | 对崩溃插件操作 |
| `InvalidElfFormat` | ELF 格式无效 |
| `UnsupportedArchitecture` | 非 AArch64 |
| `OutOfMemory` | 沙箱内存不足 |
| `MaxPluginsReached` | 超过 64 插件上限 |

## 7. 性能指标

| 指标 | 目标 |
|------|------|
| 插件加载时间 | < 100 ms |
| 沙箱开销 | < 5% |
| 最大并发插件 | 64 |

## 8. 文件清单

| 文件 | 职责 |
|------|------|
| `crates/kernel/plugin/src/lib.rs` | crate 入口 + 错误枚举 |
| `crates/kernel/plugin/src/types.rs` | 核心类型定义 |
| `crates/kernel/plugin/src/signature.rs` | SM2/SM3 签名验证 |
| `crates/kernel/plugin/src/sandbox.rs` | 沙箱隔离 |
| `crates/kernel/plugin/src/loader.rs` | ELF64 加载器 |
| `crates/kernel/plugin/src/manager.rs` | 插件管理器 |

## 9. 后续演进

- v0.167.0：模拟器完整版（PC 无硬件调试）
- v0.168.0：信誉体系（插件评分/审计）
- 未来：插件热更新、插件间通信、插件市场 UI
