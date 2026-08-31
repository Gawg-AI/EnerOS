# PROJECT.md — EnerOS 项目记忆（第一性原则）

> **状态纠偏（2026-08-12）**：本文的旧版本号、crate 数、DDS/产品能力、`蓝图/09–20` 入口和阶段状态已经漂移。当前开发入口改为 `蓝图/35-00`、`35-01` 与唯一待执行的 36–43 版本；已提交和未提交代码都按 `35-01` 判定。本文第一性原则仍有效，旧进度/路线不得作为验收事实。

> 本文件是每次开发会话的**第一输入**。简洁、只说事实、不定虚目标。
> 领域词汇事实源见 `CONTEXT.md`；架构决策冲突先由蓝图 36 裁决；当前差距与规划见 `蓝图/35-00`、`35-01`、`36–43`。

## 1. 项目是什么

**EnerOS：基于 seL4 微内核（ARM64）+ Rust no_std 的能源 Agent OS。**
一句话：把"采集→决策→控制→交易"做成一个可验证、可审计、AI 驱动的操作系统，跑在飞腾/鲲鹏等国产硬件上。

- 内核：seL4 14.0.0（形式化验证微内核），用户态全 Rust。
- 已有地基：签名介质、seL4 root-task、EL0 spawn/IPC、协议与 AI/安全组件；其产品闭环状态逐项见 `蓝图/35-01`。
- 目标 AI 链：意图/solver/LLM 组件 → 可信采集 → 唯一 ControlDispatch → 设备 ACK/readback；当前尚未产品闭环。
- 协议组件：Modbus / IEC104 / IEC61850 / MQTT；DDS 路线正在退出，协议服务是否进入产品只认 ProductManifest。
- 安全组件：SM2/3/4、TLS、启动、OTA、证明、PKI；具体产品算法/provider 与商业授权尚须蓝图 36 裁决。

## 2. 第一性原则（不可妥协）

1. **真实大于完整**：宁可能力少而真，不要全而假。Mock 只属于测试，生产路径零假实现。
2. **验证大于声称**：任何能力必须附带可执行证据（测试/QEMU 输出/真机报告）；"自证通过"无效。
3. **安全默认拒绝**：control-gateway default-deny；验签不过不写盘；fatal 违规必阻断。
4. **no_std 是边界**：EL0/目标侧 crate 禁 std；标准库只能出现在 host 工具与声明过的服务。
5. **单一事实源**：领域词汇看 `CONTEXT.md`；当前代码/缺口看 `蓝图/35-00`、`35-01`；执行只看唯一一卷 `36–43`。

## 3. 目标

**v1.0.0 = 商业交付授权**，唯一现行判据见蓝图 43：

- 真机 72h 全链路浸泡通过（启动→采集→存储→协议→调度→安全→运维）；
- 真实对端互操作报告（104 主站 / Modbus 设备 / MMS 客户端 / MQTT broker）；
- 密码学独立评审通过；渗透测试零 P0/P1；
- 合规证据机器可重算；CI 换机可复现；宣称能力=实测能力。

## 4. 当前状态（诚实基线）

- workspace 版本字段为 `0.314.35`（44-00 §3 纪律：每 WP Exit 递增 patch）；版本号不代表验收；当前商业裁决维持 **NO-GO / NOT-READY**（52 终验登记，GO 前置=DEF-52-03/05 owner 定责与外部四项）。
- 当前基线：蓝图 61 Phase 7 出口（v0.314.49，2026-09-01）——联邦 offer/commit/evidence 协议（真实 mTLS transport + SM2 签名 + 100 站控制面压力）、分片/HA/DR/容量基础设施（rendezvous 分片 + 3-voter quorum/fencing + 恢复编排 + oracle 等价面）、RC 冻结面（wire schema 冻结 + 需求追踪 + 资格 campaign 状态）；C2/C3 目标硬件容量/E5-HIL/E7 长稳登记 BLOCKED（环境前置）。前序：Phase 3 出口 v0.314.35（2026-08-31）——可信快照与资源池三卷：v0.314.33 SemanticCatalog 深模块（schema/单位含受控换算图/资产/拓扑含 energized 视图/点语义目录/反馈槽/安全覆盖/控制合同/安全约束/别名/tombstone 编译进签名 bundle；生产反馈/覆盖/控制绑定目录单源化并删除「首个 Control 点」绑定；岗位 E4-P3-GATE PASS——目录 gen1 激活/gen2 换代/回滚全判据行，PLAN-TYPED→EXEC-JOURNAL 完整）；v0.314.34 SnapshotModule（多源水位/政策/冲突确定性裁决/[02]§5 provenance 根/重放确定性 100%/C1 10k 观测 100 源，evaluator↔快照差分 7 例；target 接线登记 BLOCKED：svc-snapshot 独立服务 + 新 EIPC 边 + 存储句柄移交为前置）；v0.314.35 ResourceRegistry（11 种类 CapabilityProvider/包络求交只收紧绝不放大/聚合参考/DevicePool·VPP f32MW→f64kW 导入；dispatch 命令批绑定权威=PlannerView，planner 输入只查 Registry）。证据 evidence/61/0.314.33/34/35 + phase3-e4。含环境修复：sel4 内核 BOOT-007 init bundle 窗口按补丁对齐 8MiB（构建树刷新 + PT×4 页表/预留 +5——补丁 n+=4 少一表实弹根因）。下一阶段 = Phase 4 确定性决策（蓝图 61 `09`，v0.314.36 起）；v1.0.0-rc.2 已因 54 卷 lineage 重开作废，GO 判定须对新 lineage 重跑 52 终验矩阵。
- 产品装配现状以 53 卷 QEMU 冷启动证据为准（29 服务 ready、EIPC 授予、控制链面在库）；遗留缺口见蓝图 54 附录 C 与 43 卷 blocked-register。svc-control-dispatch 启动期 readiness=Degraded 为蓝图 50 时代即存在的环境性现象（evidence50-head 实证），非 55 卷回归。
- 审计台账：95 项审计修复收官见 `docs/delivery/reduced-line-audit/`（2026-08-23 全账 95=P0×4/P1×23/P2×41/P3×27；两轮终态=完全修复 89＋PARTIAL 2＋BLOCKED 1＋待集成接线 1＋维持登记 2，见 `2026-08-24-fix-round2.md` §二）；交付差距与 triage 结论见 `docs/delivery/53-*`。
- 路线：44–52 交付冲刺系列（已完成基线）→ 53 基底恢复（Accepted，X5 fuzz/L2 场景 PARTIAL 排队）→ 54 架构深化（Accepted）→ 55 控制链同源档案收敛（Accepted）→ **58 OTA/OBS 探针 wire 下沉（当前执行入口）**；后续按状态板滚动。

## 5. 架构分层（crate 数以 `cargo metadata` 当前结果为准）

```
boot/abi        UEFI 引导链、镜像格式、ELF 加载、IPC wire        （真实度：高）
hal/runtime     seL4 FFI、root-task、EL0 spawn/IPC                 （基础接入；NX/PIE/严格装配待37）
drivers         storage/fs/tsdb/net/cellular/rs485/can/...         （组件多；生命周期/DMA/产品服务待38–39）
protocols       modbus/iec104/iec61850/mqtt/control-gateway        （codec多；产品transport/service待39）
security        crypto/tls/secure-boot/ota/attestation/audit       （组件多；唯一信任/授权/产品接线待36–41）
ai              dual-brain/solver/llm/intent/safety/...            （算法多；可信输入/dispatch/EIPC待40）
agents          市场/设备/联邦/结算/租户/...                       （状态机多；正式装配/持久恢复待40–41）
observability   metrics/logging/tracing/dashboard/mgmt-api/...     （组件多；target provider/exporter/auth待41）
xtask/ci/fuzz   介质/签名/QEMU/SBOM/evidence                       （基础多；假绿与可移植性待36/42）
```

## 6. 开发铁律

1. **Mock 纪律**：mock 仅允许 `#[cfg(test)]` 或 release 下 `compile_error!` 熔断的 feature；生产构造一律依赖注入。
2. **测试纪律**：禁改断言迁就实现；防回归测试必须能证明"旧行为错"；测试与实现同源即重写。
3. **错误纪律**：生产路径禁新增 `unwrap/expect`；错误要么处理、要么入指标、要么注释豁免理由。
4. **unsafe 纪律**：每块 SAFETY 注释；禁共享引用造可写指针。
5. **数值纪律**：金额/电量定点化（MilliYuan 先例）；禁 `as` 截断，用 `try_from`/clamp；量纲必标。
6. **版本纪律**：一版本一工作包；验收标准逐条自证后才递增版本号；CI 必须真绿。
7. **术语纪律**：新类型先查 `CONTEXT.md` 与已废弃词汇表，禁发明同义词。
8. **会话纪律**：完整读取 `35-00`、`35-01` 和当前唯一版本；按该卷 Entry/侦察/实现/二次验证/Exit Gate 执行。

## 7. 文档地图

| 文件 | 作用 |
|---|---|
| `PROJECT.md`（本文） | 第一性原则与会话入口 |
| `CONTEXT.md` + `docs/adr/` | 领域词汇；ADR 冲突先由 36 registry/policy 裁决 |
| `蓝图/35-00` | 当前工作树商业代码审查与 NO-GO 总报告 |
| `蓝图/35-01` | 已提交/未提交代码事实、已实现勿重做、旧规划作废迁移总账 |
| `蓝图/36–43` | 唯一串行修复与商业终验路线 |
| `蓝图/00–34` | 历史设计/需求/修复来源，无执行与验收权 |

## 8. 常用命令

```bash
cargo xtask build all            # 目标命令；当前环境/依赖失败不得记 PASS
cargo xtask image raw|iso|recovery|vars|update
cargo xtask run qemu-prod
cargo xtask evidence verify|gate # release 只接受 gate PASS
cargo test --workspace           # 当前并非全绿，执行结果按35-00记录
```
