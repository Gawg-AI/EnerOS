# v0.34.0 — Agent 注册表与发现设计文档

> 版本：v0.34.0 — Agent 注册表与发现
> crate：`eneros-agent-registry`（`crates/agents/registry/`）
> 依赖：零外部依赖（仅 `alloc` / `core`），no_std

本文档描述 `AgentRegistry` 数据结构，提供全局 Agent 注册与发现能力。注册表是后续生命周期管理、启动初始化与 Agent 间通信的发现基础。

---

## 1. 版本目标

实现全局 Agent 注册表，支持 `register` / `unregister` / `lookup` / `enumerate` 四类核心操作。本版本解锁 v0.35.0（生命周期状态机）、v0.36.0（启动初始化）以及 Agent-to-Agent 通信所需的发现能力。

---

## 2. 架构定位

Phase 1 Layer 7。构建于 v0.33.0 `AgentDescriptor` 之上，为后续所有 Agent Runtime 特性提供发现层基础。注册表本身为纯数据结构，不承担调度或同步职责。

---

## 3. 前置依赖

| 依赖 | 版本 | 提供能力 |
|------|------|----------|
| `AgentDescriptor` | v0.33.0 | 注册单元的 13 字段描述符、`AgentId`、`AgentType`、`AgentState` |
| 用户态堆分配器 | v0.11.0 | 提供 `alloc::collections::{BTreeMap, Vec}`，支撑动态注册 |

---

## 4. 数据结构设计

### 4.1 AgentRegistry（双索引）

| 字段 | 类型 | 说明 |
|------|------|------|
| `agents` | `BTreeMap<AgentId, AgentDescriptor>` | 主表，按 AgentId 排序，O(log n) ID 查询 |
| `by_type` | `BTreeMap<AgentType, Vec<AgentId>>` | 类型索引，避免 `find_by_type` 全表扫描 |

双索引设计动机：主表服务于按 ID 的点查（`get` / `exists` / `unregister`），类型索引服务于按类型的批量查询（`find_by_type` / `count_by_type`）。单一索引下 `find_by_type` 需 O(n) 全扫描；双索引将其降为 O(log n) 定位 + O(k) 遍历（k 为该类型数量）。

### 4.2 RegistryStats

| 字段 | 类型 | 说明 |
|------|------|------|
| `total` | `usize` | 已注册总数 |
| `alive` | `usize` | 非终态（`state != Dead`）数量 |
| `by_type` | `BTreeMap<AgentType, usize>` | 各类型计数 |

---

## 5. 双索引设计

- **主表** `agents: BTreeMap<AgentId, AgentDescriptor>`：按 `AgentId` 排序，O(log n) 查找。`list_all` / `list_alive` 直接利用 BTreeMap 自然顺序返回排序结果。
- **类型索引** `by_type: BTreeMap<AgentType, Vec<AgentId>>`：每个 `AgentType` 映射到其 AgentId 列表。由于 `AgentId::generate()` 单调递增，按插入顺序 append 即得升序 Vec，无需二次排序。
- **一致性保证**：`register()` 同时写入主表与类型索引；`unregister()` 同时从两者移除（类型索引 Vec 使用 `retain()` 过滤被删除的 ID）。两者在同一 `&mut self` 调用内完成，无中间不一致窗口。

---

## 6. 接口清单

| 方法签名 | 说明 |
|----------|------|
| `new() -> Self` | 构造空注册表 |
| `register(&mut self, desc: AgentDescriptor) -> Result<AgentId, AgentError>` | 注册描述符，返回分配的 AgentId；ID 冲突返回 `AlreadyRegistered` |
| `unregister(&mut self, id: AgentId) -> Result<(), AgentError>` | 注销 Agent，同时清理主表与类型索引 |
| `get(&self, id: AgentId) -> Option<&AgentDescriptor>` | 按 ID 不可变查找 |
| `get_mut(&mut self, id: AgentId) -> Option<&mut AgentDescriptor>` | 按 ID 可变查找 |
| `exists(&self, id: AgentId) -> bool` | 判断 ID 是否已注册 |
| `find_by_type(&self, agent_type: AgentType) -> Vec<&AgentDescriptor>` | 按类型批量查找 |
| `find_by_name(&self, name: &str) -> Option<&AgentDescriptor>` | 按名称查找（线性扫描，命中即返回） |
| `list_all(&self) -> Vec<&AgentDescriptor>` | 列举全部，按 AgentId 排序 |
| `list_alive(&self) -> Vec<&AgentDescriptor>` | 列举非终态 Agent |
| `count(&self) -> usize` | 总数 |
| `count_by_type(&self, agent_type: AgentType) -> usize` | 按类型计数 |
| `stats(&self) -> RegistryStats` | 返回聚合统计快照 |

---

## 7. 偏差声明

### D1：BTreeMap vs HashMap

蓝图 §4.5 存在不一致——结构体声明为 `BTreeMap`，但 `new()` 使用了 `HashMap::new()`。决策：统一采用 `BTreeMap`，以维持零外部依赖不变量（`HashMap` 需引入 hasher crate）。代价为 O(log n) vs O(1)，但 n < 100，差异可忽略；蓝图 §6.3 "查找 < 1μs" 的要求轻松满足。

### D2：无内部锁

蓝图 §8.1 提及"需要锁或 RwLock"。决策：采用纯 `&mut self` 方法，不内置任何同步原语。理由：`spin::RwLock` 为外部依赖，会破坏零依赖不变量；同步属更高层职责（v0.36.0 启动初始化、v0.19.0 分区调度器）。蓝图 §6.2 "并发注册测试"重新解读为顺序压力测试（100 次顺序注册），真正的多线程并发测试延后至 v0.36.0。

### D3：AlreadyRegistered vs DuplicateId

v0.33.0 已有 `DuplicateId`（描述符层 ID 冲突）。蓝图 §4.4 要求新增 `AlreadyRegistered`（注册表层重复）。决策：按蓝图新增 `AlreadyRegistered` 变体，保留 `DuplicateId` 不变。语义区分：`DuplicateId` = 构造描述符时 ID 冲突；`AlreadyRegistered` = 注册表已包含该 ID。

---

## 8. 性能分析

对典型规模 n = 100：BTreeMap 查找约 7 次比较，远低于 1μs（满足蓝图 §6.3）。`list_all` / `list_alive` 按 AgentId 升序返回（BTreeMap 自然顺序）。`register` / `unregister` 主表操作 O(log n)，类型索引 `retain()` 为 O(m)（m 为该类型 Agent 数，通常很小）。

---

## 9. 并发设计

注册表为纯数据结构，仅暴露 `&mut self` 方法，不内置任何锁。调用方（v0.36.0 启动初始化、v0.19.0 分区调度器）负责在更高层包络合适的同步原语（如 `spin::Mutex<AgentRegistry>`）。此设计使注册表 crate 保持零依赖，可独立单元测试。

---

## 10. 索引一致性保证

`unregister()` 原子地从主表与类型索引同步移除：主表 `remove(id)`，类型索引 Vec 使用 `retain(|x| x != id)` 过滤。两者在同一 `&mut self` 调用内完成。测试覆盖：`test_unregister_cleans_type_index`（单次注销后类型索引无残留）与 `integration_type_index_consistency_after_unregisters`（多次交错注册/注销后两索引完全一致）。

---

## 11. ID 复用说明

`unregister(id)` 后，该 ID 从注册表完全移除。新的 `AgentDescriptor`（携带由 `AgentId::generate()` 生成的新 ID）可正常注册。注意：`AgentId::generate()` 单调递增，已用 ID 不会被自动重新下发；手动复用（构造携带曾用 ID 的描述符）在注销后亦受支持。测试：`test_unregister_all_then_register`。

---

## 12. 后续解锁版本

| 版本 | 内容 | 依赖本版本的能力 |
|------|------|------------------|
| v0.35.0 | 生命周期状态机 | 通过 `get_mut` 推进状态转换 |
| v0.36.0 | 启动初始化 | 批量注册初始 Agent 集，并发同步在此层包络 |
| v0.37.0 | 心跳机制 | 通过 `find_by_type` / `list_alive` 定位需心跳的 Agent |
| Agent-to-Agent 通信 | 发现层 | 注册表提供全局 Agent 发现基础 |
