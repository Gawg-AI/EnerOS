# ADR-0038：federation 传输唯一 seam = VerticalEncryptDevice（删除 SecureTransport）

> 状态：Accepted（2026-08-09）
> 决策者：架构深化实施（蓝图 23 W3.1）
> 前置：ADR-0026（协议唯一 seam 纪律）、ADR-0028（驱动唯一 seam 纪律）；蓝图 23 §1 锁定 2。
> 审计证据（2026-08-09 第四轮架构体检 + 实施时 grep 复核）：
> - `trait SecureTransport`（原 channel.rs:109）**唯一实现者**为 `MockSecureTransport`（test/mock-only），
>   全仓零生产实现者、零生产构造方——textbook hypothetical seam。
> - `trait VerticalEncryptDevice`（tunnel.rs:496）有 3 个实现者：`MockVerticalEncryptDevice`（测试）、
>   `TlsTunnelDevice`（real.rs，纯闭包 passthrough）、`TlsTunnelTransport`（tunnel.rs，带 mTLS 状态机）。
> - `FederationChannel`（会话层：mTLS 握手 + SM4-GCM 加密通话）原持 `Box<dyn SecureTransport>`，
>   零生产接线（仅 channel.rs 测试构造）。

## 1. 背景

蓝图 23 §0.2 结论 2 指出 federation 保留双传输 seam 未收敛，违反 ADR-0026/0028 纪律：
- `SecureTransport`（node_id 路由的 send/recv）喂 `FederationChannel`；
- `VerticalEncryptDevice`（单帧 xmit/poll）喂 `TunnelManager`。

两者签名同构（都是加密帧传输抽象），但 `SecureTransport` 零实现、`VerticalEncryptDevice` 多实现。
按"两实现者才算真"纪律（锁定 2），`SecureTransport` 是 hypothetical seam，必须收敛。

## 2. 决策

1. **唯一传输 seam = `VerticalEncryptDevice`**：federation 不再保留第二套传输抽象。
   `FederationChannel` 改为 `FederationChannel<D: VerticalEncryptDevice>`，直接经装置的字节管道
   （`xmit`/`poll`）收发握手帧与加密通话帧。
2. **删除 `SecureTransport` trait + `MockSecureTransport`**：从生产 public API 撤出。测试改用
   `ChannelTestDevice`（channel.rs 测试模块内，实现 `VerticalEncryptDevice`，保留收发故障注入能力）。
3. **删除 `real.rs::TlsTunnelDevice`**：与 `tunnel.rs::TlsTunnelTransport` 功能重复（同为 TLS 闭包
   适配 `VerticalEncryptDevice`）。隧道生产路径统一用 `tunnel.rs::TlsTunnelTransport`（带 mTLS
   状态机更完整）。real.rs 仅保留 `SeedPresenceBus`/`TwoNodeConsensusBus`（discovery/consensus 真实现）。
4. **会话层与隧道层职责不重叠**：`FederationChannel`（SM4-GCM 会话加密）与 `TunnelManager`（IKE/
   重放窗口/SPI 路由）是两个独立深模块，不合并。本决策只收敛传输抽象层，不改两者职责边界。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 保留 SecureTransport 标 `#[cfg(any(test,feature="mock"))]` trait-only scope | 减少了改动但留下"trait 存在但永不生产实现"的认知债；本 crate 无外部消费者用 SecureTransport，直接删更干净（ADR-0028 纪律：删即删） |
| 让 FederationChannel 改用 `TunnelManager<D>` 而非直接持 `D` | 会把会话层焊到隧道层，导致双层加密叠加 + SPI 映射复杂度，混淆两个独立深模块。决策 4 明确职责不重叠 |
| 删整个 FederationChannel 会话层 | 算法逻辑（mTLS 握手、SM4-GCM、derive_session_key）有 27 个测试覆盖，是有保留价值的领域代码。零生产接线不等于无价值——保留为可接线的深模块 |

## 4. 影响面

- `crates/agents/federation/src/channel.rs`：`FederationChannel` 泛型化 `<D: VerticalEncryptDevice>`；
  `SecureTransport`/`MockSecureTransport` 删除；测试改用 `ChannelTestDevice`。
- `crates/agents/federation/src/real.rs`：`TlsTunnelDevice` 及其测试删除。
- `crates/agents/federation/src/lib.rs`：re-export 移除 `SecureTransport`/`MockSecureTransport`。
- 无跨 crate 影响（`FederationChannel` 零外部消费者，vpp-scenario/pentest 用 federation 其他类型）。

## 5. 验收

- `SecureTransport` 与 `TlsTunnelDevice` grep 零命中（生产代码）。
- `cargo test -p eneros-federation` → 233 passed; 0 failed（契约测试不减少）。
- `cargo build -p eneros-{vpp-scenario,pentest}` → 通过。

## 6. 追踪

- 蓝图：23 §4 W3.1；§1 锁定 2。
- 纪律：ADR-0026（seam 唯一 + 两实现者才算真）、ADR-0028（删即删，不留兼容区）。
