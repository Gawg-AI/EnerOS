# ADR-024 — RNG / 确定性 nonce 标准化（21-B C4）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 21-B §2.C4 强约束 + C1 双实现联动）
- **关联**：ADR-022（GmSSL binding）、`蓝图/21-B` §2.C4

## 背景

21-B §2.C4 指出 `CsRng`（SM3-Hash-DRBG）与 `sm2/sign.rs` 的确定性 nonce
（`k = SM3(d‖msg)` 截断）是密码学评审必挑项，要求：DRBG 对接硬件 TRNG
（飞腾 RNG）+ 标准 SP800-90A DRBG，确定性签名改真 RFC6979。

## 现状侦察（2026-08-09 代码为准）

1. `CsRng` 旧实现为**计数器模式**（`V = SM3(V‖counter)`）——"SP 800-90A 风格"
   但**非标准 Hash_DRBG 结构**（无 0x00/0x03 域分隔、无 C 状态、无
   reseed_counter 语义）。
2. `sign_deterministic` 旧实现 `k = SM3(d ‖ msg)` 截断到 [1, n-1]——**非标**：
   无 HMAC 驱动、无 bits2octets、无候选循环；且用裸 msg 而非 e。
3. **熵源闭环已存在**（CRY-002）：`EntropySource` trait（`fill_entropy`
   fail-closed）+ `PhytiumTrng`/`KunpengTrng`（BLOCKED 缺硬件，返回
   `Unavailable` 不伪造）+ `QemuVirtioRng`（`qemu-entropy` feature，真实
   virtqueue 读熵，host MMIO 仿真测试在位）。
4. `CsRng::new()`（固定种子）已 `#[cfg(any(test, debug_assertions, feature="insecure-test-rng"))]`
   门控——release 生产路径唯一入口 `from_seed`（熵注入）。

## 决策

1. **CsRng 重构为 SP 800-90A Rev.1 §10.1.1 Hash_DRBG（SM3 变体）**：
   状态 `(V, C, reseed_counter)`；`Hash_DRBG_Update` 用 0x00/0x03 域分隔；
   Generate 用 `V = Hash(0x03‖V)` 循环输出 + 输出后 update；Reseed 用
   `0x01‖V‖entropy`。**API 不变**（`from_seed`/`fill_bytes`/`reseed`/
   `for_deterministic_signature`/`next_u32`/`next_u64`），消费者无感。
2. **`sign_deterministic` 改用标准 RFC 6979（SM3 变体）**：
   `rfc6979.rs` 按 RFC 6979 §3.2 逐条实现：V/K 初始化、`x = d`、
   `h1 = bits2octets(e)`（e mod n）、HMAC-SM3 驱动、候选 `[1, n-1]` 循环。
   旧 `k = SM3(d‖msg)` 删除。
3. **TRNG 注入点维持现状**（EntropySource trait 已有，硬件到位仅实现
   `fill_entropy` 的寄存器读取）；QEMU 仿真证据 = `QemuVirtioRng` MMIO
   仿真测试（已在位）。
4. **SM3 变体的向量说明（诚实声明）**：RFC 6979 官方示例为 SHA-256 + P-256，
   SP 800-90A 官方向量为 SHA-256 系列——**SM3 变体无第三方官方向量**。
   验证策略：RFC 6979 结构逐条实现 + 结构 KAT（确定性/域分离/候选区间/
   私钥与消息分离）+ SM2 验签闭环互操作（手卷↔openssl 10 组向量，ADR-022）；
   DRBG 性质 KAT（同种子同输出/异种子异输出/重播种改变序列/块间差异）。

## 替代方案（否决）

- **保留计数器模式 + 文档标注"风格"**：不满足 21-B C4 强约束（标准
  SP800-90A），评审必挑。
- **等待商密认证后再改**：C4 为软件侧 P0，无需硬件；认证是长期并行项。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| SM3 变体无官方向量 | 结构逐条对照 RFC/SP800-90A 实现 + 性质 KAT + 验签互操作闭环；ADR 明示 |
| DRBG 结构重写引入回归 | API 不变 + 23 个 RNG 测试（确定性/性质）+ crypto 全量 409+ 测试 |
| 确定性 nonce 变化影响既有向量 | `sign_deterministic` 仅测试/可复现场景使用（生产走随机 k）；sm2_kat 确定性测试全部重跑通过 |

## 验收判据映射（21-B §2.C4）

- [x] ADR 记录（本文）
- [x] KAT 验证：RFC6979 结构 KAT + DRBG 性质 KAT + 互操作 KAT 全 PASS
- [x] 硬件未到位：TRNG 路径留 trait 注入点（EntropySource，已有）+
      QEMU 仿真证据（QemuVirtioRng MMIO 仿真测试，已在位）
