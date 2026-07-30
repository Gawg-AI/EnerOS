# Agent OS 认证体系 (v0.184.0)

## 概述

`eneros-app-cert` 提供应用认证与设备兼容性认证能力，确保部署到 EnerOS
设备上的应用满足安全、性能与行为规范要求。

## 核心能力

| 能力 | 说明 |
|------|------|
| 应用认证 | 对应用包执行标准化测试套件，签发认证证书 |
| 设备兼容性 | 检查目标设备 BSP 认证状态，评估兼容性 |
| 认证规范 | 5 类强制认证项（SDK兼容/安全/性能/资源限制/行为审计） |

## 认证类别

- **SdkCompat** — SDK 兼容性验证
- **Security** — 签名与安全策略验证
- **Performance** — 性能基线验证
- **ResourceLimit** — 资源占用上限验证
- **BehaviorAudit** — 运行时行为审计

## 认证流程

1. 提交 `AppPackage`（含签名与二进制哈希）
2. 验证签名有效性
3. 执行 `AppTestKit` 测试套件
4. 检查所有强制项通过
5. 签发 `AppCertificate`

## 设备兼容性

- BSP 已认证 → 兼容评分 1.0
- BSP 未认证 → 兼容评分 0.5，标记不兼容

## 模块结构

```
crates/agents/app-cert/
├── Cargo.toml
└── src/
    ├── lib.rs            # 入口 + AppCertEngine
    ├── types.rs          # 核心数据结构
    ├── spec.rs           # 认证规范管理
    ├── testkit.rs        # 测试套件
    └── device_compat.rs  # 设备兼容性检查
```

## 约束

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 零第三方依赖
- u64 用于 ID / 时间戳
