# S3 出口报告（阶段三：安全与硬件地基）

## 工作包状态

| 工作包 | 状态 | 机器证据 |
|---|---|---|
| CRY-001 SM2 Jacobian/定次模逆/恒定时间 | ✅ | crypto 393+5；计时统计测试（max/min≤2.2）连续 3 次过 |
| CRY-002 熵源闭环 | ✅ | entropy 4 测试双 feature；release 0 error + new() cfg 门控 |
| CRY-003 TPM2 命令路径 + 门控 | ✅ | tpm 48+9+1；release nm 无 SimulatedTpm 符号 |
| CRY-004 AK EK 派生 | ✅ | attestation 24（跨重启稳定 + 签名互验） |
| CRY-005 TLS 选型 ADR | ✅ | ADR-0031；tls anomaly 6（真实 TLS1.3 握手 + 异常拒绝） |
| CRY-006 OTA 防降级/回滚 | ✅ | ota-ab 32+5；QEMU 断电恢复 BLOCKED |
| CRY-007 验签性能 | ✅ | dev 3.2ms / release 2.15ms（旧 150–214ms） |
| CRY-008 评审材料 | ✅ 材料 / BLOCKED 评审 | evidence/crypto-review/algorithm-inventory.md |
| BSP-001 D2000 平台 | ✅ 代码 / BLOCKED 真机 | hal 25（平台配置测试） |

## 出口判据

1. **密钥生成-签名-验签-封仓-证明全链路**：host 集成测试
   `attestation/tests/s3_key_chain.rs`（1 测试）串起 SM2 生成/签名/验签/
   篡改拒绝/SM2 封仓-解封/篡改密文拒绝/Quote 证明-验证——**QEMU 运行时
   端到端 BLOCKED**（无 qemu + seL4 工具链缓存，同 KRN-003 解锁条件）。
2. **product_allowed = false 解除评审材料**：crypto-review/algorithm-inventory.md
   （算法清单 8 项 + KAT 来源 + 威胁模型 4 域）——评审本身 BLOCKED（外部）。
3. **硬件专属项显式 BLOCKED 无静默降级**：
   - 飞腾/鲲鹏 TRNG（CRY-002）：接口占位 fail-closed，不伪造读数；
   - SPI TPM 硬件路径（CRY-003）：真实 TIS 序列，无硬件时 BLOCKED；
   - TPM EK 读取（CRY-004）：软件 seed 派生，真实 EK BLOCKED；
   - 裸机 TLS（CRY-005）：Tongsuo 移植 BLOCKED，不降级自研；
   - D2000 真机（BSP-001）：地址基于公开资料，真机校准 BLOCKED。
   全部为显式 BLOCKED 状态，无 exit 0 冒充。

## 遗留

- QEMU 全链路（含熵源 virtio-rng 路径）待有 qemu 的机器复跑；
- 第三方密码学评审待外部评审方。
