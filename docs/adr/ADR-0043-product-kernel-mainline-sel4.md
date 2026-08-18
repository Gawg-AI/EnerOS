# ADR-0043：唯一产品内核路线裁决 = seL4（蓝图 36 WP-36.1 决策 Gate）

> 状态：Accepted（2026-08-12）
> 决策者：产品负责人（蓝图 36 WP-36.1 负责人决策 Gate，签署标识
> `CHG-2026-08-12-36-01-sel4`）+ 主开发 AI 执行
> 前置：ADR-0023（Accepted，2026-08-02）、ADR-0030（Accepted，2026-08-05）
> 取代：ADR-0001（本 ADR 明确 supersede 其"自研内核唯一"的产品主线段）

## 1. 结论

**唯一产品内核 = seL4**。`product/kernel-policy.toml` 为唯一机器可读裁决：

```toml
product_kernel = "sel4"
runtime_root = "eneros-root-task"
allowed_release_kernels = ["sel4"]
research_only = ["eneros-product-kernel"]
decision_adr = "ADR-0043"
owner_approval = "CHG-2026-08-12-36-01-sel4"
```

- 自研内核（`eneros-product-kernel` 方向，ADR-0001）降级为 `research_only`，
  不得进入任何 release/recovery/factory profile 的正式闭包；
- seL4 14.0.0（elfloader + eneros-root-task）为当前正式介质唯一内核与
  root-task 装配，qemu-prod / board-prod profile 与开发链与此一致；
- `docs/adr/registry.toml` 为 ADR 唯一 registry；本 ADR 的
  `supersedes = ["ADR-0001"]`；ADR-0023 / ADR-0030 以
  `superseded_by = "ADR-0043"` 并入本裁决（内容一致，统一执行语义）。

## 2. 依据

| 事实 | 来源 |
|---|---|
| qemu-prod/board-prod profile 实际选择 seL4 | `product/profiles/*.json` |
| root-task 与 EL0 spawn/IPC 基于 seL4 TCB/VSpace/Endpoint | `crates/runtime/root-task` |
| 35-01 默认建议：按当前正式 profile 与较新用户裁决保留 seL4 | `蓝图/35-01` §4 |
| 蓝图 36：默认建议 seL4；负责人签署后 37–43 才可继续 | `蓝图/36` WP-36.1 |

## 3. 影响

- 37–43 按 seL4 唯一主线执行；不再出现两条内核路线并存；
- 任何 release 闭包中出现 `eneros-product-kernel` 或非 allowlist 内核，
  由 `cargo xtask policy kernel` + `closure gate` 阻断；
- 恢复 ADR-0001 为 Accepted（无 superseded_by）时
  `cargo xtask policy adr` 必须非零退出（二验要求）。

## 4. 附录：36 crypto 闭包处置

本 ADR 一并记录 36 WP-36.2 的闭包处置：`eneros-crypto`
（SM2/SM3/SM4，`product_allowed=false`，无商业批准）已从产品服务闭包
移出——root-task / services / audit 的链式哈希改用已批准 SHA-256
（sha2），正式 UEFI 启动链维持 Ed25519/SHA-256。详见
`product/crypto-policy.toml`。
