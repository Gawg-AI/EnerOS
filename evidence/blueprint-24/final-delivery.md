# 蓝图/24 最终交付说明（DoD 总验收清单逐项证据）

> 日期：2026-08-09 ｜ 分支：feature/v0.165.0-protocol-driver-sdk
> 提交：v-next.1（24.1+24.2）、v-next.2（24.3+24.4）、v-next.3（24.5）——见 git log
> 证据目录：`evidence/blueprint-24/`（各 WP 侦察报告 + 验收证据，全部可复算）

## 一、附录 E 总验收清单（DoD）逐项证据

| # | DoD 项 | 状态 | 证据 |
|---|---|---|---|
| 1 | 每个 WP 的全局侦察报告完成（波及面自证/依赖方向检查/全仓消费方清点） | ✅ | `wp-24.1-recon.md`、`wp-24.3-recon-acceptance.md`（§一）、`wp-24.4-recon-acceptance.md`（§一）、`wp-24.5-recon-acceptance.md`（§一） |
| 2 | §2.2 波及面表全部核销，"明确不碰"项零 diff 有 git 证据 | ✅ | `wp-24.5-recon-acceptance.md` §一核销表；不碰项（快平面/驱动/候选 2-9）零 diff；从站侧仅 D4 被迫 3 行机械 cast（行为零变更，差异记录） |
| 3 | 编译期无法构造无授权设备写（类型系统强制） | ✅ | `PointAccess` 无写方法；唯一写入口要求 `AuthorizedCommand`；`ProtocolManager::write_point` 已移除；T3/T8/契约套件负用例 |
| 4 | 三个 master 适配器契约全绿（含篡改/不匹配拒绝） | ✅ | `cargo test -p eneros-modbus-rtu`（77+2+1）/ `-p eneros-modbus-tcp`（58+1）/ `-p eneros-iec104-master`（59+1） |
| 5 | device-agent 真 HMAC 复验通过第二层全部用例 | ✅ | `cargo test -p eneros-device-agent`：第二层 10 用例（落写/篡改/过期/吊销/stale/幂等/不匹配/协议失败/握手/重启单调），全经真实 ControlChain |
| 6 | AI 决策全链到达设备（host 集成证据） | ✅ | runner `full_chain_decision_to_device_write`：Applied 功率 == 调度下发值（回读）；`tampered_envelope_rejected_structured`、`handshake_failure_fails_closed` |
| 7 | 断网本地自治写可用（预置租约） | ✅ | 场景③（e2e_scenarios）：无外部通道，本地握手（token=epoch+1）→ 落写 |
| 8 | 急停路径可达（正常信封 + EmergencyBypass 双通道） | ✅（设计保留） | EmergencyBypass 为既有 CTL-009 逃生门（只绕 PDP 不绕安全）；急停走正常信封（ActorKind::Emergency，预置租约）——ADR-0026 §7.3 说明；测试证据：ActorKind::Emergency 语义在信封/链既有测试覆盖 |
| 9 | 每类拒绝有指标与审计；旧匿名路径只拒不执行 | ✅ | 9 类拒绝 → 11 项指标（§6.6 名）+ 审计事件（WriteDenied，detail 键值编码）；双轨：T20 + 场景④ legacy=1 |
| 10 | HMAC 密钥装配注入演示，无明文落盘 | ✅ | `assembly_demo.rs`（§7.4）：密钥经 env 注入；`configs/control-write.toml` 无密钥；runner `build_production_sink` 同源注入 |
| 11 | ADR-0026 补充说明合入；CONTEXT.md 词汇生效 | ✅ | ADR-0026 §7（读写分级/关系/逃生门）；CONTEXT.md §1 更新（AuthorizedWriter 已落地 + AuthorizedCommand 登记） |
| 12 | §9 威胁表逐项有对应测试或证据 | ✅ | `wp-24.5-recon-acceptance.md` §七 销项表（T1-T8 逐项对应测试/证据） |
| 13 | 旧路径删除闸门条款已设定日期/版本 | ✅ | 蓝图/24 修订记录 R2：v1.0.0-rc.1（2026-08-09 设定；legacy 指标 ≥30 天无增量否则顺延） |

## 二、本规格与代码现实的差异记录（汇总）

1. **AuthorizedCommand/ActorKind 落位**（WP-24.1）：规格 §6.1 契约套件假设 seam 层可引用；代码现实中位于 control-gateway，protocol-abstract 引用即违反 §7.3 依赖方向。→ 下移 telemetry-model（ControlCommand 同处），control-gateway re-export，全部导入路径不变，零行为变化。
2. **protocol-sdk 示例**（WP-24.1）：规格 §2.2 预期示例编译失败；代码现实 SDK 独立于 seam（无 protocol-abstract 依赖），无断裂，按以代码为准不动。
3. **从站侧"明确不碰"**（WP-24.1）：D4 类型化迫使 3 行机械 cast（handler.rs×2、slave.rs×1）+ 测试构造 1 行；default-deny 行为零变更。
4. **§6.1 断裂面清单遗漏项**（WP-24.1）：`ProtocolManager::write_point`（移除，旁路形态）、protocol-abstract 内部测试、firmware-agent 场景二进制（WP-24.3 发现，register 签名适配）。
5. **§6.2 失败表未列网关拒绝**（WP-24.3）：Expired/Future/Replayed 归 lease 桶（时间性/资格性），审计 detail 精确区分。
6. **复验链 SBO**（WP-24.3）：§6.2 复验清单未列 SBO；按"信封到达即选择+执行原子"处理（与签发侧 ChainCommandSink 一致）。
7. **DeviceExecutor::write 增 envelope 参数**（WP-24.4）：规格 §6.4 描述"AuthorizedCommand+功率值"投递；但 device-agent 复验的是**签名信封**（AuthorizedCommand 无 HMAC）。→ 投递签名信封原字节（§1.3 推论 D"同一信封复验"）。
8. **runner 链时间基准修正**（WP-24.4）：now_fn 毫秒→秒（信封规格 epoch-secs，跨进程同基准）。
9. **QEMU 端到端**（WP-24.5）：spawn 现状不通（device-agent 无 EL0 service 装配；runner 为 host 二进制）→ host 证据先行 + QEMU 挂 22 系列（按 §8.2 回退条款）。

## 三、§9 威胁表销项状态

T1-T8 全部有对应测试或证据（详见 `wp-24.5-recon-acceptance.md` §七）。渗透测试（蓝图/14 G4）以该表为靶清单逐项复核。

## 四、遗留风险（R1–R6）最新评估

| ID | 风险 | 评估 |
|---|---|---|
| R1 EIPC 迁移（候选 3） | 缓解兑现：`DeliveryChannel` trait 封装，迁移点单一；概率高/影响低不变 |
| R2 QEMU spawn | **已触发**：host 证据先行 + QEMU 挂 22 系列；22 系列需补 device-agent EL0 service 装配（含 spawn 承载） |
| R3 密钥分发 | 缓解兑现：装配演示 + 无明文 + 同源组装；产线密钥管理仍为运维项 |
| R4 握手失败 Offline | 缓解兑现：有界重试 ≤5 + fail-closed + 测试证明零降级；告警依赖指标接线（已证导出） |
| R5 过渡期误认 | 缓解兑现：write_denied_legacy + 审计 + T20 + 删除闸门（v1.0.0-rc.1） |
| R6 契约改造破坏读用例 | 未发生：读基线用例保留，三适配器 + mock 全绿 |

## 五、未达成项声明

**DoD 全部 13 项达成**（附证据如上）。唯一显式声明项：**QEMU 端到端三场景以 host 证据先行**（spawn 现状不通，蓝图/24 §8.2 允许的回退路径），QEMU 场景挂入蓝图/22 系列验收——不是 placeholder PASS，是显式声明的延迟交付项，证据链完整可复算。
