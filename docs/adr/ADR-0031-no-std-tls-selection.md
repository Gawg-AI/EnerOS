# ADR-0031：no_std TLS 方案选型（CRY-005）

> 状态：Accepted（2026-08-05，S3 CRY-005）
> 前置：NET-005（eneros-tls 基于 rustls 的 TLS 1.3/mTLS）

## 1. 决策

**TLS 实现 = rustls（保持现状，host 侧 std）**；裸机（seL4 用户态 no_std）
TLS 不另行自研——Tongsuo 交叉移植与 TLCP 套件扩展列为 BLOCKED（基础设施
依赖），不做降级替代（不重复造轮子，ADR-0021）。

## 2. 评估（三候选）

| 候选 | 形态 | 审计/维护 | no_std 可用性 | 结论 |
|---|---|---|---|---|
| **rustls**（已实装 eneros-tls） | 纯 Rust，rustls-webpki 证书链验证 + ring provider | 广泛审计，工业级 | **否**（需 std/alloc，state machine 与 io 耦合） | ✅ host 侧唯一实现 |
| **Tongsuo**（蚂蚁 BoringSSL 分支） | C 库，国密 TLCP/SSLEngine | 活跃维护，国密合规 | 需交叉编译 aarch64-unknown-none + 无 libc 的定制链接 | BLOCKED：缺 aarch64-none C 交叉构建链与真实网络栈（M 期） |
| TLCP 自研 | Rust no_std | 无 | 理论上可 | 否决：违反 ADR-0021 禁重复造轮子；国密协议正确性风险 |

## 3. 实装现状与 BLOCKED

- **host 侧已实装**：`crates/security/tls`（rustls 0.23 TLS 1.3-only + mTLS +
  rustls-webpki 验证，host 服务侧 std）。
- **裸机侧 BLOCKED（两项基础设施依赖）**：
  1. aarch64-unknown-none 的 C 交叉工具链（Tongsuo 需编译 C）——sandbox 无；
  2. seL4 用户态网络栈（联邦通信传输面）——M 期（KRN-002/003 仅 IPC 闭环）。
- **不降级**：不用自研 AEAD/握手伪装"裸机 TLS"；裸机加密在传输面就绪后
  以 Tongsuo（或 rustls 的 no_std 移植若社区出现）接入。

## 4. 后果

- eneros-tls 继续为唯一 TLS 实现（host 服务侧，联邦网关/云边）；
- CRY-005 出口：选型 ADR 落盘 + host 侧 rustls 实装验证（TLS 1.3-only 强制、
  mTLS 拒绝链）持续绿；裸机项显式 BLOCKED。
