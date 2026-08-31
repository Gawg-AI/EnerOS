# 蓝图 61 Phase 7 WP-7.3 商用资格与 RC 冻结（v0.314.49）

日期：2026-09-01
生效版本：0.314.49（本卷**不新增架构能力**，只做冻结、追踪与证据闭合）

## 0. 唯一不变量（§4.1）

> 所有承诺场景均有可复现的目标/HIL/现场证据、故障与恢复证据、需求追踪和独立审查；此版本不新增架构能力。

本卷的诚实声明：目标硬件/HIL/E7 证据在本环境**不可复现**（登记 BLOCKED，
见 §3）；本卷做到的是**完全可复现的 host 证据闭包** + 需求追踪表 +
冻结门 + P0/P1 清零复查，并把硬件项钉进高亮登记。

## 1. Schema 冻结（§4.2）

| 面 | 冻结方式 | 证据 |
|---|---|---|
| 联邦 wire | `tests/phase7_wire_frozen.rs`（4 测）：WIRE_VERSION=1、MAX_FRAME=65536、MAX_INTERVALS=288、kind 域 1..=8、canonical 头 86B、offer 定长体 177B | 代码测试 + 本版本 git diff 零 wire 改动 |
| 站内既有 wire/config/catalog/model/skill | 本版本零改动（diff 审计） | git diff `fa03925..HEAD` 仅 federation/scale/evidence 新增 |
| 生产 endpoint/key/profile 与 sandbox 分离 | 既有（svc-market 0x02 fail-closed 等）；本卷无新 endpoint | Phase 6 登记 §2.1 |

## 2. 需求追踪表（§4.4）

映射：requirement → threat/failure → code owner → test id → profile → evidence artifact → reviewer → verdict。

| # | requirement | threat/failure | owner | test id | profile | evidence artifact | verdict |
|---|---|---|---|---|---|---|---|
| R1 | 联邦只交换能力/承诺/证据 | 原始点位/配方出站泄露 | federation | P7-WIRE(data_minimized) / P7-SECURITY-9(evidence_aggregate_only) | host | offer.rs / phase7_security.rs | PASS |
| R2 | 站内安全权威不外移 | 云目标超界/恶意 | federation | P7-SECURITY-9(malicious_cloud_target) | host | negotiation.rs | PASS |
| R3 | 防重放/乱序/过期 | 重复承诺、陈旧 leader | federation | P7-SECURITY-9(replay/coordinator_restart/stale leader) | host | negotiation.rs window | PASS |
| R4 | 密钥轮换/撤销 | 证书盗用 | federation | P7-KEYS / P7-SECURITY-9(cert_revocation) | host | keys.rs | PASS |
| R5 | 两节点不自动选主 | 双主 | federation | leadership 单测 ×4 | host | leadership.rs | PASS |
| R6 | 目标不可行不 default-fill | 不可行计划下放 | federation | P7-ALLOC(infeasible) / P7-SECURITY-9(超容量) | host | allocation.rs | PASS |
| R7 | 双主/旧 fencing 零设备写 | 旧 leader 恢复后写设备 | scale | SCALE-FENCE-2 | host | quorum.rs + scale_fence.rs | PASS |
| R8 | 重分片无双重写副作用 | 重分片触发设备命令 | scale | SCALE-RESHARD-4 | host | shard.rs + scale_reshard.rs | PASS |
| R9 | 分片结果与 reference 等价 | 分片丢键/乱序 | scale | SCALE-CAPACITY-3 | host | capacity.rs | PASS |
| R10 | 无静默 drop（队列全观测） | 慢消费者丢数据不可见 | scale | SCALE-CAPACITY-3 | host | capacity.rs | PASS |
| R11 | RPO=0（intent/audit 已确认写） | 已确认写丢失 | scale | DR 单测（metrics_within_target） | host 记账 | dr.rs | PARTIAL→目标硬件实测登记 |
| R12 | RTO ≤60s→D4 / ≤5min→D0 | 恢复超时站内失守 | scale | DR 单测 | host 记账 | dr.rs | PARTIAL→目标硬件实测登记 |
| R13 | C2/C3 容量资格（50k/500k events/s 等） | 宣称超实 | scale | SCALE-CAPACITY(host) | **目标硬件 BLOCKED** | capacity.rs + 登记 | BLOCKED 登记 |
| R14 | 全协议 E5 互操作证据 | 协议宣称无对端验证 | federation | P7-TLS-2(E3) | **HIL BLOCKED** | fedxport + phase7_tls.rs | BLOCKED 登记 |
| R15 | 72h 长稳/断电≥100 次 | 稳定性宣称 | ops | — | **E7 BLOCKED** | — | BLOCKED 登记 |
| R16 | LLM prompt/tool 对抗 | 越权输出 | ai(model-harness) | Phase 6 既有 | host+target BLOCKED | 蓝图 11 卷 | Phase 6 已交付面 PASS |

## 3. 资格 campaign（§4.3 12 项）状态

| 项 | 状态 | 说明 |
|---|---|---|
| 1. 72h C2 steady + burst | BLOCKED(硬件) | soak 基础设施既有（soak72.sh 先例）；目标硬件进门即跑 |
| 2. 7d shadow/canary | BLOCKED(现场) | 无现场站 |
| 3. 断电/重启 ≥100 次 | BLOCKED(硬件) | QEMU 窗口回归可做子集（登记） |
| 4. 分区/双主/撤销/时钟/磁盘满 | 代码面 PASS(host) | WP-7.1/7.2 故障测试 + 蓝图 5 期磁盘满修复链；磁盘满目标实测登记 |
| 5. 全协议 E5 矩阵 | BLOCKED(HIL) | 互操作框架存在（52 卷）；真实对端外部项 |
| 6. 多设备 Saga/补偿/HIL | BLOCKED(HIL) | Phase 5 既有 host 面 |
| 7. solver/certifier 变异/病态/超时 | 代码面 PASS(host) | Phase 4 既有（mutate 13/13 先例） |
| 8. LLM 对抗 | 代码面 PASS(host) | Phase 6 harness 对抗测试 |
| 9. sandbox/production 隔离 | 代码面 PASS(host) | svc-market 0x02 fail-closed + 本卷 svc-federation 无 writer |
| 10. 100 站联邦+灾备 | 控制面 PASS(host)，规模 BLOCKED(硬件) | WP-7.1 pressure + WP-7.2 reshard/fence |
| 11. 渗透/租户隔离/隐私脱敏 | 代码面 PASS(host) | 租户隔离/数据最小化测试 + pentest 规则库 |
| 12. 人工接管 D0-D6 演练 | BLOCKED(现场/HIL) | Phase 5 降级链 host 面既有 |

## 4. 退出 gate（§4.6）核销

| 判据 | 状态 |
|---|---|
| 04 文件的 E7、所有全局反例、本版 campaign 全绿 | **BLOCKED（E7 硬件项）**——host 全绿；E7 项清单见 §3；全局反例 15 项对应测试在 `docs/evidence` 与蓝图 04 §3 引用链 |
| 独立 reviewers 对 safety/security/operations/release 签字 | 本卷 code-review 轮完成（P0/P1=0）；外部 reviewer 签字=外部项 BLOCKED 登记 |
| open P0/P1=0 | PASS（复查轮） |
| 生成 v0.314.49 不可变证据快照 | PASS（evidence/61/0.314.49/） |

## 5. 不留“待以后”

按蓝图 §4.4「没有自动证据的 critical requirement 必须明确人工测试、双人
签字和到期复验，不能写‘待以后’」——上表所有 BLOCKED 项都有：
owner（目标硬件/现场/HIL 环境）+ 复验入口（对应 host 测试/工具）+ 条件
（环境就绪即执行），附机器可查的 host 面等价测试 id。

## 6. 供应链证明（§4.2 末行）

- SBOM/license：`cargo xtask release sbom` 需要外部 cyclonedx/spdx 工件
  （生成器不在本环境）→ BLOCKED 登记；`cargo deny`（audit 门）在六门内
  每日运行 PASS。
- binary closure：`cargo xtask closure gate --profile qemu-prod`（CI 既有）；
  本卷新增 crate 无第三方依赖（federation=仅 crypto(+tls under feature)；
  scale=零依赖）——依赖闭包测试在 phase7_no_writer.rs 固化。
## 7. 独立复查轮：P0/P1 全修复（2026-09-01，附于本卷）

独立子代理对 WP-7.1/7.2 全部新代码四向复查（spec/standards/adversarial/
assembly），发现 2 P0 + 5 P1 + 9 P2。**全部 P0/P1 已修复**（修复提交随本卷），
P2 择要修复 4 项、余 5 项登记。修复清单：

| # | 级别 | 缺陷 | 修复 |
|---|---|---|---|
| P0-1 | P0 | **身份绑定缺失**：任一已登记站点可冒充他站（伪造 offer/commitment/结算证据）——verify 只验签名、`env.site_id` 与 key 零关联 | KeyRegistry 增 `owners`（key_id→站点）映射 + `register_with_owner`；协商层验签后强制 `owner_of(signer_key_id)==env.site_id`；commitment 校验加 `c.site_id==session.site_id`；svc `--site-keys` 格式增 `owner=`；新增 `impostor_site_rejected` 场景测试（冒 offer/commitment 全拒、真实站不受污染） |
| P0-2 | P0 | svc `in_record/out_record` 对签名合法但语义非法 body `.expect()` → 远程崩溃 DoS | 改 `Option<JournalRecord>`，不可解码记 None 跳过；实弹复跑确认流程不受影响 |
| P1-1 | P1 | 撤销状态不持久：协调器重启后撤销 key 复活 Active | `keys.journal` append-only 事件行 + `apply_key_events` 重放；新增 `revoke` 运维子命令（写事件行） |
| P1-2 | P1 | epoch 投毒：peer 自报 epoch 进重启基线 → 溢出/回绕 | `journal_max_epoch` 只统计本站 Out 记录；`EpochRegulator::sanitized` 钳制 `u64::MAX-1` |
| P1-3 | P1 | 站点可对已过期 offer 签 7 天承诺（无 valid_until 门） | 站点签 commitment 前加过期门（Expired 结构化拒绝）；协调器 propose() 同步查过期 |
| P1-4 | P1 | FenceGuard 无持久化基线：设备侧重启后"先到先写"，旧 leader 可复活；原"across restarts"测试名不副实 | `from_baseline`/`baseline` API + 真正重启场景测试（旧 token/旧 term 全拒、高 term 接管） |
| P1-5 | P1 | ReshardPlan Copy 预设 `to_shard=from%new.shards`：扩张方向漏拷新分片 | Copy 改为键集级指令（目标由执行器按新环路由）；测试断言计划语义 |
| P2 修复 | P2 | 负 reserve 穿透 wire；LoadGen 空数组 panic；帧解码上限不齐（-80 vs encode MAX_FRAME）；check_envelope 无网格覆盖校验 | 全部修复（encode/decode 双侧拒负、空 classes 返回 0、上限统一 -150、曲线覆盖 offer 时段校验） |
| P2 登记 | P2 | 证据/journal 无界增长上限；retry_count 恒 0；单槽 commitment 覆盖语义；站点 failover 无重发；缺 --site-keys 即锁死 | 登记（生产装配面随目标侧接线） |

复查结论（摘要）：canonical 验签/窗口边界/数值/确定性四个方向未发现缺陷；
身份绑定与远端 panic 是协议面真实 P0，已闭环。
