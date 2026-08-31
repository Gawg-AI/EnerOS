# 蓝图 61 Phase 7 WP-7.2 登记与退出 gate 核销（v0.314.48，分片/HA/DR/容量）

日期：2026-09-01
提交：见 git HEAD（本卷唯一提交）

## 1. 交付内容（蓝图 §12 第 3 节）

| 蓝图条目 | 落点 | 实现 |
|---|---|---|
| §3.2 分片 | `crates/runtime/scale/src/shard.rs` | rendezvous hash `(tenant,site,source)`→vnode（FNV-1a 64，版本化环）；重分片五步计划（copy→catch-up→dual-read→atomic switch→drain），模拟执行验证键集合守恒 + 零设备副作用；`dual_route` 双读比对 |
| §3.3 HA | `quorum.rs` | 3 voter quorum（prepare/commit 票集）+ leader lease + 单调 fencing token；设备边 `FenceGuard`（高 term 无条件接管、同 term 单调、租约过期同 term 拒）；readiness 四组件（registry/snapshot/journal/audit） |
| §3.4 DR | `dr.rs` | RestorePlanner 顺序约束（控制临界先于业务）+ RestoreProgress（D4/D0 门）+ RecoveryMetrics（RPO=0/RTO≤60s→D4、≤5min→D0 记账） |
| §3.5/§3.6 容量 | `capacity.rs` | 负载生成器（确定性）+ Oracle 参考模型 + 等价性比对 + 有界队列八指标（深度/高水位/丢弃/拒绝/重试等）——「先建负载生成器和 oracle，禁止先调大常数」 |

## 2. 退出 gate 核销（.48）

| 判据 | 状态 | 证据 |
|---|---|---|
| 目标硬件证据（不接受 host-only） | BLOCKED（登记） | C2/C3 目标硬件（RK3568/飞腾类 + 生产 profile）不在本环境；host 侧等价面 + oracle 已就绪，目标硬件进门即测 |
| C2/C3 内存磁盘预算内、无静默 drop | PARTIAL（host 模拟） | 队列八指标全观测 + 100k 事件计数闭合 + 慢消费者可观测（host）；目标硬件预算实测登记 |
| 双主/旧 fencing 零设备写 | PASS | scale_fence 双主三路写入账本实证（旧 term/旧 token/过期租约全拒，新 term 接管） |
| RPO/RTO 实测满足 profile | PARTIAL（host 记账） | RecoveryMetrics 判据 + 单元测试；实测需目标硬件（登记） |
| bottleneck/容量余量/不支持上限明确 | PASS（文档） | 登记文档 §3 不支持上限；容量面在 host 侧已暴露分片/队列失配检测 |

## 3. 不支持上限与登记项（诚实声明）

1. **C2/C3 目标硬件容量证据**：100k 点/50k events/s/24h、200k/s 突发、磁盘 80/95/100%、1000 资源、C3 100 站分布式——均需目标硬件与真实存储层（littlefs/块服务），本环境不可复现，登记 BLOCKED。
2. **快照/注册表实测分片**：Snapshot state 与 ResourceRegistry 的按 site/purpose 分片是既有 svc 的装配面（svc-snapshot/svc-resource-registry 目标侧接线尚未立）；本卷提供路由/等价面/重分片编排，服务侧接线随目标装配（与 Phase 6 存储句柄移交同一 BLOCKED 链）。
3. **命令 Saga 分片 leader 固定**：同资源命令固定 leader 的路由语义由 ShardRing 确定性保证（同键恒同分片）；Saga 侧接线（control-dispatch）随目标装配登记。
4. **协调器 quorum 接线**：quorum/lease/fencing 以库形式交付；svc-federation 的 3-voter 集群化（多实例 + 选举驱动）本卷未装配（.48 只到库 + 策略门）。

## 4. 顺手修复（测试先红的真实缺陷）

- **重分片 Copy/CatchUp 目标路由**：按 `from % new.shards` 复制会把键放错分片（双读必不一致）——测试先红后修正为按新环路由；这是五步流程正确性关键。
- **FenceGuard term 接管优先级**：初版把租约过期检查放在 term 比较之前，更高 term 的 leader 在旧租约过期窗口内被拒（fencing 失效）——修正为高 term 无条件接管，同 term 才受 token 单调与租约约束。
