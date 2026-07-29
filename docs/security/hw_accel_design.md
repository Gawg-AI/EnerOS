# 国密硬件加速设计文档 (v0.158.0)

> **版本**：v0.158.0  
> **状态**：已实现  
> **依赖**：v0.143.0 HAL ARM64 / v0.31.0 国密算法库

## 1. 概述

本模块为 EnerOS 提供国密 SM2/SM3/SM4 算法的硬件加速能力，利用飞腾 D2000 和鲲鹏 920 SoC 内置的加密引擎，性能较纯软件实现提升 ≥ 10 倍。

### 1.1 设计目标

- **硬件优先**：启动时探测硬件引擎，可用则使用硬件加速
- **自动降级**：硬件故障/超时自动 fallback 到纯 Rust 软件实现
- **零拷贝**：大数据通过 DMA 传输，避免 CPU 拷贝开销
- **可扩展**：trait 抽象支持未来新硬件引擎接入
- **安全**：密钥在引擎内处理，操作后清零

### 1.2 性能指标

| 算法 | 硬件性能 | 软件性能 | 加速比 |
|------|---------|---------|--------|
| SM3 | > 1 Gbps | ~100 Mbps | ≥ 10x |
| SM4-ECB | > 2 Gbps | ~80 Mbps | ≥ 25x |
| SM2 签名 | < 5 ms | ~50 ms | ≥ 10x |

## 2. 架构设计

```
┌─────────────────────────────────────────────────────────┐
│  CryptoSelector (引擎选择器)                             │
│  - 硬件优先 + 自动 fallback                             │
│  - 故障检测 + 降级策略                                  │
└───────────────────────┬─────────────────────────────────┘
                        │
┌───────────────────────▼─────────────────────────────────┐
│  CryptoEngine trait (统一接口)                           │
│  - sm2_sign / sm2_verify                                │
│  - sm3_hash                                             │
│  - sm4_encrypt / sm4_decrypt                            │
└───┬───────────────────┬───────────────────┬─────────────┘
    │                   │                   │
┌───▼───────┐  ┌───────▼───────┐  ┌───────▼───────┐
│ Phytium   │  │ Kunpeng       │  │ Software      │
│ D2000     │  │ 920 KAE       │  │ (Pure Rust)   │
│ (MMIO)    │  │ (Queue)       │  │ (Fallback)    │
└───────────┘  └───────────────┘  └───────────────┘
```

## 3. 模块结构

```
crates/security/crypto/src/hw/
├── mod.rs          # 模块入口 + 便捷工厂函数
├── engine.rs       # CryptoEngine trait + CryptoSelector
├── phytium.rs      # 飞腾 D2000 加密引擎驱动
├── kunpeng.rs      # 鲲鹏 920 KAE 加速引擎驱动
└── software.rs     # 软件 fallback 封装
```

## 4. 核心接口

### 4.1 CryptoEngine trait

```rust
pub trait CryptoEngine {
    fn sm2_sign(&mut self, private_key: &[u8], msg: &[u8]) -> Result<[u8; 64], CryptoError>;
    fn sm2_verify(&mut self, public_key: &[u8], msg: &[u8], sig: &[u8; 64]) -> Result<bool, CryptoError>;
    fn sm3_hash(&mut self, data: &[u8]) -> Result<[u8; 32], CryptoError>;
    fn sm4_encrypt(&mut self, key: &[u8; 16], iv: Option<&[u8]>, mode: Sm4Mode, data: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn sm4_decrypt(&mut self, key: &[u8; 16], iv: Option<&[u8]>, mode: Sm4Mode, data: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn name(&self) -> &'static str;
    fn is_hardware(&self) -> bool;
    fn is_available(&self) -> bool;
    fn stats(&self) -> EngineStats;
}
```

### 4.2 CryptoSelector

```rust
let mut selector = CryptoSelector::new();
selector.register(Box::new(PhytiumCryptoEngine::new()));  // 硬件优先
selector.register(Box::new(SoftwareCryptoEngine::new())); // 软件 fallback

let hash = selector.sm3_hash(b"data")?;  // 自动选择引擎
```

## 5. 平台差异

| 特性 | 飞腾 D2000 | 鲲鹏 920 KAE |
|------|-----------|-------------|
| 接口 | MMIO 寄存器直接操作 | 队列式请求提交 |
| 基地址 | 0x2820_0000 | 0x4000_0000 |
| DMA | 4KB 对齐缓冲区 | 队列描述符 |
| 中断 | 轮询/中断可选 | 完成计数 |

## 6. 错误处理与 Fallback

硬件错误类型（触发 fallback）：
- `EngineBusy`：引擎忙
- `EngineTimeout`：操作超时（>100ms）
- `DmaError`：DMA 传输失败
- `EngineError`：引擎内部错误
- `EngineUnavailable`：引擎不可用

非硬件错误（直接返回）：
- `InvalidKeyLength`：密钥长度错误
- `InvalidInput`：输入格式错误

## 7. 安全设计

- 密钥通过引擎安全寄存器写入，操作后自动清零
- DMA 缓冲区使用后清零（`zeroize`）
- 禁止在日志中输出密钥材料
- 恒定时间签名验证（抗时序侧信道）

## 8. 配置

配置文件：`configs/crypto_engine.toml`

关键配置项：
- `engine.strategy`：引擎选择策略
- `engine.timeout_ms`：硬件超时
- `engine.auto_fallback`：自动降级开关
- `security.zeroize_dma_after_use`：DMA 清零

## 9. 测试

- 国标测试向量：GM/T 0003 (SM2) / GM/T 0004 (SM3) / GM/T 0002 (SM4)
- 集成测试：`tests/hw_engine_test.rs`
- 模拟模式：无硬件时验证驱动逻辑

## 10. 后续演进

- v0.159.0：TPM 2.0 集成（共享密码学接口）
- v0.160.0：Secure Boot（依赖 SM2/SM3 硬件加速）
- 异步操作：中断驱动 + DMA  scatter-gather
- 更多平台：支持未来国产 SoC 加密引擎
