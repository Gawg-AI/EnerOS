# 第三方密码学评审材料（CRY-008，S3）

> 状态：材料齐备；**评审本身 BLOCKED（外部依赖：需独立密码学评审方）**。

## 1. 算法清单（EnerOS 使用的密码原语）

| 算法 | 标准 | 用途 | 实现位置 | KAT |
|---|---|---|---|---|
| SM2（签名/验签/加密） | GB/T 32918.5-2017 | Agent 签名/OTA 验签/证明/加密 | `crates/security/crypto/src/sm2` | tests/sm2_kat.rs（11）+ cry001（5） |
| SM3（哈希/HMAC/DRBG） | GB/T 32905-2016 | 摘要/度量/PCR/CSRNG | `crates/security/crypto/src/sm3` | sm3 KAT 测试 |
| SM4（ECB/CBC） | GB/T 32907-2016 | 数据加密 | `crates/security/crypto/src/sm4` | sm4 测试 |
| 模幂/模逆（费马定次） | — | SM2 点运算基元 | `crates/security/crypto/src/bigint` | inv_mod/pow_mod 测试 |
| TLS 1.3 + rustls/ring | RFC 8446 | 云边/联邦通信 | `crates/security/tls` | anomaly.rs（6） |
| TPM 2.0 命令集 | TCG TPM 2.0 | 度量/密钥/Seal | `crates/security/tpm` | 48+9 测试 |
| CRC32（IEEE 802.3） | 帧完整性 | IPC/存储（非安全） | `crates/abi/ipc-wire` | KAT 向量 |

## 2. KAT 向量来源

- SM2：GB/T 32918.5 附录示例 + 独立 Python 计算对拍（2G/3G/4G/5G、模逆 2^-1）；
- SM3：GB/T 32905 标准向量（"abc" 等）；
- 恒定时间性：多输入计时统计（scalar_mult/inv_mod max/min ≤ 2.2）；
- 篡改/变异：逐字节变异拒绝测试（boot-protocol 风格，S1 SEC 系列）。

## 3. 威胁模型（覆盖范围）

1. 时序侧信道：SM2 标量乘法（ladder+掩码+ct_select，恒定轮数）、模逆（定次费马）；
2. 故障注入：验签 fail-closed（篡改签名/哈希拒绝）、熵源缺失 fail-closed；
3. 密钥生命周期：私钥零化（Drop）、AK 不落盘（EK 派生）、熵 seed 单向哈希；
4. 降级攻击：SimulatedTpm feature 门控禁入 release、CsRng::new release 不可达、
   OTA 版本单调防降级。

## 4. 评审 BLOCKED

独立第三方密码学评审（算法实现正确性/侧信道/合规）需外部评审方——**BLOCKED
（外部依赖）**。解锁条件：评审方介入后提供评审报告；材料（本清单 + KAT + 威胁
模型）已就绪供评审。
