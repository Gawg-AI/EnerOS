# 多租户支持 (Multi-Tenant Support)

> v0.182.0 · crate: `eneros-tenant`

## 概述

EnerOS Edge Box 多租户模块允许单台边缘设备同时服务多个主体（租户），
并在 CPU、内存、存储、Agent 数量和带宽维度实施资源配额与隔离。

## 架构

```
┌─────────────────────────────────────────┐
│            TenantIsolation              │  ← 隔离边界 (IsolationBoundary)
├─────────────────────────────────────────┤
│            TenantStore                  │  ← 生命周期管理 (TenantManager)
├──────────┬──────────┬───────────────────┤
│  Tenant  │  Quota   │   Assignments     │
└──────────┴──────────┴───────────────────┘
```

## 核心类型

| 类型 | 说明 |
|------|------|
| `TenantQuota` | 资源配额上限 (cpu/mem/storage/agents/bandwidth) |
| `Tenant` | 租户实体 (id, name, created_at, quota) |
| `QuotaUsage` | 当前资源使用快照 |
| `ResourceReq` | 单次资源分配请求 |

## 主要 API

- `TenantManager::create(name, quota)` — 创建租户
- `TenantManager::delete(tenant_id)` — 删除租户（有 Agent 绑定时返回 TenantBusy）
- `TenantManager::assign_agent(tenant_id, agent_id)` — 绑定 Agent，受 max_agents 约束
- `check_quota(quota, usage)` — 校验当前使用是否越限
- `check_resource(quota, req)` — 校验新请求是否可容纳
- `remaining(quota, usage)` — 计算剩余容量
- `IsolationBoundary::enforce(tenant_id, req)` — 隔离层统一入口

## 错误码

| 变体 | 含义 |
|------|------|
| `TenantNotFound` | 租户 ID 不存在 |
| `QuotaExceeded` | 配额超限 |
| `TenantBusy` | 仍有 Agent 绑定，不可删除 |
| `InvalidQuota` | 配额参数非法 |
| `AgentAlreadyAssigned` | Agent 已分配 |

## 约束

- `#![no_std]` + `extern crate alloc`，零第三方依赖
- f32 表示百分比，u64 表示 ID，usize 表示尺寸
- 所有公开接口均有单元测试覆盖
