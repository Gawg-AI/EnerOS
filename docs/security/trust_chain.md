# 信任链文档 (v0.160.0)

## 信任链模型

EnerOS Secure Boot 采用四级信任链：

```
Stage 0: Boot ROM (硬件信任根)
    ↓ 验证
Stage 1: Bootloader (信任根公钥验签)
    ↓ 验证 + 密钥传递
Stage 2: Kernel (BL 公钥验签)
    ↓ 验证
Stage 3: Runtime (stage_key 验签)
    ↓
Complete (启动成功)
```

## 密钥层次

- **Root Key**: 信任根 SM2 公钥，烧录于熔丝/安全存储
- **BL Key**: Bootloader 签名密钥，公钥随镜像传递
- **Stage Key**: 下级验签密钥，BL→Kernel 时安装

## PCR 度量

| PCR | 度量内容 | 扩展时机 |
|-----|---------|---------|
| 0 | Bootloader | BL 验签通过后 |
| 4 | Kernel | Kernel 验签通过后 |
| 2 | Runtime | Runtime 验签通过后 |

## 安全属性

- 信任链不可跳过（强制逐级顺序）
- 密钥不可导出（TPM 硬件保护）
- 签名不可伪造（SM2 数学安全性）
- 防降级（时间戳下限检查）
