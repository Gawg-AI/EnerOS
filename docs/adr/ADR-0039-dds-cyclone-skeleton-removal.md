# ADR-0039：删除 CycloneDdsNode 骨架（agent-bus-dds 为 trait-only scope）

> 状态：Accepted（2026-08-09）
> 决策者：架构深化实施（蓝图 23 W3.3）
> 前置：ADR-0026（seam 唯一 + 两实现者才算真）、ADR-0021（commercial_scope 预设）；蓝图 23 §1 锁定 2。
> 审计证据（实施时 grep + 代码复核）：
> - `crates/protocols/agent-bus-dds/src/cyclone_dds.rs`（184 行）的 `CycloneDdsNode` 是
>   `DdsNode` 的"第二实现者"，但所有实质方法体返回 `Err(DdsError::Ffi(-1))` 或
>   `Err(DdsError::Closed)`——空骨架，零真实逻辑。
> - `crates/protocols/agent-bus-dds/src/ffi.rs`（117 行）的 extern "C" 符号零调用者
>   （唯一引用方 cyclone_dds.rs 不进 CI 默认编译）。
> - `cyclone-dds` feature 定义为空（`cyclone-dds = []`），无 build.rs 链接 C 库——骨架永远跑不起来。
> - crate 的 `commercial_scope = "out-of-v1.0"`（ADR-021）：agent-bus-dds 不进 v1.0.0 发布闭包。

## 1. 背景

蓝图 23 §0.2 结论 2 指出 `agent-bus-dds` 的 `CycloneDdsNode` 用空骨架冒充 `DdsNode` 的第二实现者，
让 deletion test 的实现者计数失真（看起来有 Mock + Cyclone 两个，实际生产仅 Mock）。
对比：同 crate 的 `MessageSigner`（Mock + 真 Sm2）、`CapabilityVerifier`（Mock + 真）是健康 seam。

## 2. 决策

1. **删除 `cyclone_dds.rs` + `ffi.rs`**：空骨架不提供功能，只制造"有第二实现者"的假象。
2. **移除 `cyclone-dds` feature**：无对应 C 库链接，feature 是空声明。
3. **`DdsNode` 标注为 trait-only scope**：当前唯一实现者是 `MockDdsNode`（`cfg(any(test, feature="mock"))`）。
   未来接真实 DDS 后端时，按 ADR-0026 纪律"先有真实需求再加"——届时重新引入实现者并验证。
4. **不降级为 `#[doc(hidden)]`**：骨架方法体全 `Err`，标注"骨架"也不改变它是死代码的事实。
   ADR-0028 纪律：删即删，不保留过渡态。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 保留骨架 + `#[cfg(feature="cyclone-dds")]` 下 `#[doc(hidden)]` + 注释"非生产可用" | 骨架全 `Err`，保留它只维持"有第二实现者"的假象；doc(hidden) 不改变 deletion test 计数失真。out-of-v1.0 crate 无消费者，无过渡必要 |
| 接真实 Cyclone DDS C 库 | 超出 v1.0.0 范围（commercial_scope=out-of-v1.0）；agent 总线 v1.0.0 用 MQTT（eneros-mqtt 真实可用） |

## 4. 影响面

- `crates/protocols/agent-bus-dds/src/cyclone_dds.rs`：删除。
- `crates/protocols/agent-bus-dds/src/ffi.rs`：删除。
- `crates/protocols/agent-bus-dds/src/lib.rs`：移除模块声明 + feature 文档 + test_t17。
- `crates/protocols/agent-bus-dds/Cargo.toml`：移除 `cyclone-dds` feature。
- 无跨 crate 影响（crate 为 out-of-v1.0，无外部消费者引用 CycloneDdsNode）。

## 5. 验收

- `cyclone_dds` / `ffi` / `CycloneDdsNode` grep 零命中（生产代码）。
- `cargo test -p eneros-agent-bus-dds` → 64 passed; 0 failed。

## 6. 追踪

- 蓝图：23 §4 W3.3；§1 锁定 2。
- 纪律：ADR-0026（两实现者才算真）、ADR-0028（删即删）、ADR-0021（commercial_scope）。
