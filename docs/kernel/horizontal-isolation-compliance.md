# 横向隔离合规路径书面结论

> **版本**：EnerOS v0.9.1 — 横向隔离合规验证
> **模块**：`crates/kernel/mm/src/isolation/`
> **依据**：36 号文《电力监控系统安全防护总体方案》§3.2 横向隔离
> **配置**：`configs/compliance/isolation-policy.toml`
> **最后更新**：2026-07-12

---

## 1. 合规依据

### 1.1 法规引用

36 号文《电力监控系统安全防护总体方案》明确要求：电力监控系统的安全区（安全控制大区）与管理信息大区之间必须实现**横向隔离**，具体要求包括：

- 安全控制大区与管理信息大区之间必须采用物理隔离设备（网络隔离器 / 正向型安全隔离网闸 / 光电隔离网闸）。
- 数据流必须**单向**：仅允许管理信息大区单向接收来自安全控制大区的数据，禁止反向数据流。
- 隔离强度须达到或超过国密认证的物理隔离设备指标。

### 1.2 适用范围

本合规验证覆盖 EnerOS v0.9.0 实现的双分区内存隔离（`partition.rs`）与 DMA 保护（`dma_guard.rs`）原语，验证其在 EnerOS v0.9.1 中满足横向隔离的合规要求。

### 1.3 合规结论类型

| 结论 | 含义 | 后续动作 |
|------|------|---------|
| **Go** | 四项检查中前三项（物理隔离 / capability / 单向流）全部满足，第四项（形式化验证）可选 | 可进入 Phase 1 |
| **Go（含条件）** | 前三项满足但形式化验证未完成 | 进入 Phase 1，但须在 v1.0.0 前补齐形式化验证 |
| **NoGo** | 前三项中任一不满足 | 不得进入 Phase 1；须补充物理隔离设备并更新 BOM |

---

## 2. 证据链

`collect_isolation_evidence()` 采集以下四项证据：

| # | 证据项 | 来源 | 判定标准 |
|---|--------|------|---------|
| 1 | `partition_separation` | v0.9.0 `Partition::is_isolated_from()` | 分区 A/B 的 `allowed_phys` 区间无重叠 |
| 2 | `capability_enforced` | v0.9.0 `DmaGuard` / capability 框架 | 所有跨分区访问经 capability 授权检查 |
| 3 | `unidirectional_flow` | 数据流审计 | 跨边界数据流仅 A→B 方向，无 B→A 反向流 |
| 4 | `formal_verification` | seL4 形式化证明（Phase 3） | 隔离属性经形式化验证工具证明 |

### 2.1 参考分区配置

| 属性 | 分区 A（安全控制大区） | 分区 B（管理信息大区） |
|------|----------------------|----------------------|
| 名称 | `safety_control` | `agent_runtime` |
| 内存基址 | `0x4000_0000` | `0x4800_0000` |
| 内存大小 | 128 MB | 128 MB |
| Capability Root | `0x1000` | `0x2000` |
| 内存区间 | `[0x4000_0000, 0x4800_0000)` | `[0x4800_0000, 0x5000_0000)` |

两分区内存区间相邻但不重叠，满足物理隔离要求。

---

## 3. Go/No-Go 判定流程

```
采集证据 (collect_isolation_evidence)
    │
    ▼
┌─────────────────────────────┐
│ 物理内存隔离 (partition_separation)? │──否──▶ NoGo (need_physical_device=true, BOM +网络隔离器)
└─────────────┬───────────────┘
              是
              ▼
┌─────────────────────────────┐
│ capability 强制 (capability_enforced)? │──否──▶ NoGo (need_physical_device=false, 软件修复)
└─────────────┬───────────────┘
              是
              ▼
┌─────────────────────────────┐
│ 单向数据流 (unidirectional_flow)? │──否──▶ NoGo (need_physical_device=true, BOM +光电隔离网闸)
└─────────────┬───────────────┘
              是
              ▼
┌─────────────────────────────┐
│ 形式化验证 (formal_verification)? │──否──▶ Go（含条件）
└─────────────┬───────────────┘
              是
              ▼
             Go
```

**判定规则**：前三项为强制检查，任一不满足即 NoGo；第四项（形式化验证）为可选，不满足时仍为 Go 但附条件（须在 v1.0.0 前补齐）。等价表述：四项中至少三项满足且前三项全部满足 → Go；否则 NoGo。

---

## 4. BOM 影响分析

当结论为 NoGo 时，`BomImpact` 结构体记录对物料清单的影响：

| NoGo 原因 | 需物理设备 | 增量成本（元） | BOM 新增项 |
|-----------|-----------|--------------|-----------|
| 物理内存隔离不满足 | 是 | 12,000 | `network-isolator`, `sfp-module` |
| capability 未强制 | 否 | 0 | （软件修复，无 BOM 影响） |
| 单向数据流不满足 | 是 | 8,000 | `diode-gateway` |

### 4.1 BOM 回写

`writeback_bom()` 将 BOM 影响回写到配置管理数据库。回写前验证：
- 若 `need_isolator = true`，则 `bom_items` 不得为空（否则返回 `InvalidImpact`）。
- `cost_delta_yuan` 不得超过 1,000,000 元（否则返回 `InvalidImpact`，防止预算溢出）。

---

## 5. 合规审计报告

`generate_compliance_report()` 生成完整的审计报告，包含：

| 字段 | 说明 |
|------|------|
| `conclusion` | Go/No-Go 结论（含证据与 BOM 影响） |
| `partition_a` | 分区 A 信息（名称 / 内存基址 / 大小 / capability root） |
| `partition_b` | 分区 B 信息 |
| `data_flow_verified` | 数据流是否已验证为单向 |
| `regulatory_clause` | 法规条款引用（`36号文-横向隔离-§3.2`） |

---

## 6. 参考配置文件

双分区策略配置见 `configs/compliance/isolation-policy.toml`，包含：
- 分区 A/B 内存基址与 capability root
- 36 号文条款引用
- Go/No-Go 阈值（`go_threshold = 3`，强制检查项列表）
- BOM 影响阈值（隔离器单价、最大成本增量）

---

## 7. 签字栏

本合规结论须由架构负责人签字确认后方可作为 Phase 0 硬出口条件。

| 角色 | 姓名 | 签字 | 日期 |
|------|------|------|------|
| 架构负责人 | | | |
| 安全合规负责人 | | | |
| 内核子系统负责人 | | | |

> **备注**：依据 ADR-0003，横向隔离合规结论为 Phase 0 硬出口条件，未通过 Go 不得进入 Phase 1。

---

## 8. 相关文件

| 文件 | 说明 |
|------|------|
| `crates/kernel/mm/src/isolation/mod.rs` | 模块入口与公共类型定义 |
| `crates/kernel/mm/src/isolation/compliance.rs` | 合规验证逻辑（Go/No-Go 判定） |
| `crates/kernel/mm/src/isolation/audit.rs` | 审计报告生成与 BOM 回写 |
| `crates/kernel/mm/src/partition.rs` | v0.9.0 物理内存分区隔离原语 |
| `crates/kernel/mm/src/dma_guard.rs` | v0.9.0 DMA 保护域守卫 |
| `configs/compliance/isolation-policy.toml` | 双分区策略配置 |
