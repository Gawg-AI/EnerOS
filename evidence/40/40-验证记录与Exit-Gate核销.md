# 40 — 验证记录与 Exit Gate 核销（Agent、AI 与设备控制闭环修复蓝图）

> schema: `eneros.blueprint-40.v1`
> 前置：39 Exit Gate 10 PASS/1 PARTIAL（PARTIAL 面 = 40 Entry 前置：EL0 设备句柄移交、net 服务装配、协议 transport 注入）——允许进入 40
> 编制：2026-08-15（蓝图 40 执行完毕）；**2026-08-16 收尾复验（40 收尾卷）**：3 项 PARTIAL 全部实弹达成，Exit Gate 12/12
> 分支：`feature/v0.165.0-protocol-driver-sdk`；工作树含 36/37/38/39 未提交代码（本卷保护不覆盖）
> HEAD：`b96774e`（未提交本卷改动——按任务禁令不自动 commit）
> 证据：`target/eneros/evidence/40-qemu/qemu-serial.log`（QEMU 最终轮，`PRODUCT-READY: true` + `CONTROL-CHAIN dispatch=Committed` + `HEARTBEAT-KILL svc-hang`）+ 全仓 host 测试全绿 + aarch64 target 构建

## 0. 结论

**40 Exit Gate：12 PASS / 0 PARTIAL（40 范围）。** 收尾卷完成三项 PARTIAL
的实弹面：EIPC 端点 capability 移交（root spawn 授予表 + 服务 X3/X4 接线）、
EIPC 端到端真实投递（采集代理 → 可信评估 → heuristic 决策 → 唯一
ControlDispatch → 270B 信封分块 → device-agent 全链 → 虚拟设备写+读回 →
Committed）、supervisor 心跳超时 kill（svc-hang 死循环注入 → 超时 → kill+
restart → 控制链继续）。产品商业状态仍为 **NO-GO**（41 域：真实协议设备/
采集链设备侧 capability 移交、市场/结算/OTA/运维）。

## 1. Entry Gate

- 39 PARTIAL 面（svc 设备句柄移交、net 服务装配、协议 transport 注入）：本卷
  承接为 svc 服务循环 + EIPC 协议（chunked）装配。**40 收尾**：服务端点
  capability 移交（CVR-39-02 端点面）完成（EIPC-GRANT 实弹）——设备侧
  （MMIO/DMA/IRQ）移交仍为 41 Entry 前置（CROSS_VERSION-40 登记）。
- 39 全仓 7242 测试基线 → 本卷全仓测试全绿（357 套件，无回归）。

## 2. 工作包核销

| WP | 交付 | 证据 |
|---|---|---|
| 40.1 | `TrustedTelemetry`/`TelemetrySample`/`SampleQuality`/`TelemetryTrustEvaluator`（safety-validator `trusted` 模块：覆盖/新鲜度/质量/范围/有限性/重放/来源 epoch/映射版本/完整性/控制时域 + SM3 provenance）；`SystemState::default()` **移除**（编译期保证）+ `TestSystemStateBuilder` + `SystemState::unknown()`（NaN fail-closed） | safety-validator 53 测试（trusted 13：含重放/epoch/完整性/篡改摘要/多源冲突/恢复态）；全仓 `SystemState::default` 零生产使用 |
| 40.2 | `TelemetryService`（`control-services/telemetry.rs`：有界队列/去重/epoch 推进/快照）+ svc-telemetry EL0 服务 + 请求 wire（INGEST/SNAPSHOT/ADVANCE_EPOCH） | telemetry 模块 7 测试；QEMU `svc-telemetry READINESS: Ready` |
| 40.3 | `DecisionOutcome`（Proposal/NoAction/Rejected）+ `DecisionPath`（Fast/Slow/Fallback/Degraded）+ `ProposedControlPlan`；coordinator 重构——四路径只产 proposal，sink 字段移除（协调器不写设备），gate.record 移 dispatch，全路径审计（audit.rs 增 Proposed/NoAction 变体） | dual-brain 59 测试（含 fallback/degraded 路径断言 + mark_committed + 全路径审计）；runner demo 20 测试 |
| 40.4 | infer-scheduler：`InferRequest` 增 correlation/budget_tokens/model_digest；`cancel`/`cancel_by_correlation`；`SchedulerError::Cancelled/LateResult`；结果携带 correlation（late-drop 依据） | infer-scheduler 19 测试（含取消/按 correlation 取消/结果 correlation） |
| 40.5 | runner 拆 `eneros-ai-runner-demo`（host JSONL，证据类别 `DEMO_ONLY`，`--demo-trusted-feedback` 显式开关，无输入退出非零）+ `svc-ai-worker`（target，**静态 HeuristicBackend**——SOC 死区规则，bounded O(1)，WP-40.4 允许；Llama/HiGHS profile 不声明） | runner 20 测试 + ai_worker 测试（heuristic 放电/充电/死区 NoAction/plan_id 唯一）；QEMU `svc-ai-worker READINESS: Ready` + `backend=heuristic` |
| 40.6 | `ControlDispatch` trait + 12 态 `DispatchState` + `DispatchReceipt` + `DispatchEngine`（validate→authorize→persist→deliver→ACK→readback→audit 全链；幂等/Indeterminate/读回不符/预算/过期全语义） | control-dispatch 11 lib 测试 + 集成 8/8（Committed/超时→Indeterminate/不可达→Err/预算→Err 零投递/过期→Err/越限→Err/篡改→Failed/幂等重放不重复写）；QEMU `dispatch=Committed` |
| 40.7 | 复用 AuthorizedWriter：ControlDispatch 唯一 issuer（经 ControlChain/EnvelopeBuilder）+ device-agent ingress 唯一 executor；`ControlEnvelope`（payload_digest 覆盖功率——HMAC 域外载荷完整性） | 集成测试（篡改功率 → Denied）；wire 3 测试 |
| 40.8 | `ControlEnvelope`（control-gateway `wire`：270B 定长/schema 版本化/payload SM3）+ `OutcomeFrame`（扩展码 4/5/6）+ agent-sdk `ChunkedClient`/`ChunkedReceiver`（MR 预算分块协议）+ `Sel4EipcChannel`（feature sel4）。**40 收尾**：裸帧 MR[0..] 语义（QEMU 实弹）+ 分页响应（ENVP/ENVN）+ `&mut T: RawIpc`；**端点 cap 授予实弹**（EIPC-GRANT×4） | wire 3 + agent-sdk 83（chunked 乱序/长度/半组装/端到端/分页/裸帧预算）；QEMU 每轮真实投递 |
| 40.9 | svc-device-agent EL0 服务（chunked EIPC 循环 + ingress 执行）；`control_service_main`（通用服务循环：header→ack/data→组装→final→回执，fail-closed；心跳 signal + 分页回执）。**40 收尾**：虚拟 PCS 设备（VIRTUAL-PCS_WRITE）+ 产品装配（同源 key/lease/policy/safety） | QEMU `svc-device-agent READINESS: Ready` + `INGRESS: Applied { echoed: Some(Float(5.0)) }` + `[virtual-pcs] WRITE point=10 value=5` |
| 40.10 | Registry 持久化重建：`DeviceRegistration`（AdapterKind/EndpointRef/OperationSet/PointSchemaVersion/capability_digest/generation）+ snapshot（SM3 摘要校验）+ WAL（read-modify-write + generation 单调）+ load 重绑（factory 缺失 → `available=false` 不静默丢设备）+ registry `update/remove/set_available` | persist 9 测试（roundtrip/篡改拒绝/重复 ID 拒绝/WAL 回放/unavailable 标记）；device-agent 51 测试 |
| 40.11 | 分层成功语义：ingress 独立 readback（不再用 WriteOutcome.echoed）+ `ReadbackPolicy`（精确/容差/枚举/单调/异步）+ 结果码 APPLIED/READBACK_UNAVAILABLE/MISMATCH | device-agent ingress 测试 + dispatch 集成（读回不符→Failed/不可得→Indeterminate） |
| 40.12 | 失败对账：`IntentStore`（durable intent + pending 对账）+ Indeterminate 语义 + learning 只消费 Committed（EnergyAgent 重构：可信遥测输入 → Proposal → dispatch.submit → Committed 才学习/mark_committed） | control-dispatch intent 4 测试；energy-market-agent 209 测试（含 Committed 门控/失败不学习/无遥测零推理零写） |

## 3. 装配与 QEMU

- 5 required service 入 qemu-prod/board-prod profile（`svc-telemetry`/`svc-energy-agent`/
  `svc-ai-worker`/`svc-control-dispatch`/`svc-device-agent`，criticality=control）；
  `REQUIRED_SERVICE_BASELINE` Gate（product_manifest.validate）——**删除任一
  required 服务 → normalize FAIL**（mutation 实弹验证：删除 svc-telemetry →
  `required-service-baseline-missing:svc-telemetry` 转红；恢复后 PASS）。
- image qemu-prod 构建 PASS（26 服务——收尾卷新增 svc-hang 注入参考服务，init_bytes 随收尾构建更新）；
  REVERSE-PARSE-GATE PASS entries=26；manifest normalize PASS；
  KERNEL-POLICY-GATE PASS；ADR-REGISTRY-GATE PASS（39 条目）；
  closure gate 4 项违规 = 39 登记的存量（eneros-crypto×3 + ecdsa 未登记，非 40 引入）。
- **QEMU 最终轮**（零 VARS + bootctrl reset + 全新数据盘）：
  `PRODUCT-READY: true (degraded: ; required degraded: )`；
  5 控制服务 `READINESS: Ready`；`media_writes=16`（仅 bootctrl 固件写）；
  唯一 FAULT-INFO 为 svc-fault（故意注入演示）。镜像 sha256=`536d1b73…`。

## 4. QEMU 装配排障记录（本卷实弹发现并修复）

| 问题 | 根因 | 修复 |
|---|---|---|
| 服务静默无 READY | ELF 段非 4 KiB 页对齐（rust-lld 默认 64 KiB max-page-size）——39 R4 同源 | control-services build.rs（max-page-size=0x1000 + separate-code） |
| svc-device-agent VM fault（fsr=0x92000047） | `EnvelopeIngress` 结构体 276,440 B（metrics 的 heapless::Vec<Metric,256> 内嵌）→ rustc 为 `_start` 生成 ~274 KiB 栈清零帧 → 16 KiB 栈写入 guard 页 | metrics 字段改 `Box`（结构体降到 KB 级）；heap-large 384 KiB（OOM size=276040 于 256 KiB 堆） |
| root 堆 OOM（25 服务期） | 3 MiB 堆在 bundle 1.69 MiB 解析期 used=3130304 | root-task 默认堆 3→4 MiB |
| NoCapSlot（root CSpace 4096 槽） | 全局大栈/大堆 bss 超槽预算 | per-service 栈机制（`CONTROL_SERVICE_STACK_PAGES`，Box 化后置空）+ heap-large 仅控制服务 |

## 5. Mutation 防假绿

| Mutation | 方法 | 结果 |
|---|---|---|
| 恢复 `SystemState::default` | Default impl 已删——任何恢复无法编译 | 编译期保证 |
| 删除 required 服务（svc-telemetry） | normalize 实弹 | Gate 转红（`required-service-baseline-missing`） |
| 用 WriteOutcome.echoed 当 readback | ingress 测试断言独立 read_point | 测试存在（设计保障） |
| 恢复旧 epoch 接受 | evaluator `restore`/`advance_epoch` 负例 | 测试转红（设计保障） |
| 删 registry WAL | persist wal_replay 测试 | 测试转红（设计保障） |
| FileExecutor 当生产 DeliveryChannel | runner demo 证据类别 DEMO_ONLY + Sel4EipcChannel 独立 | 结构隔离 |
| ACK 当物理成功 | OutcomeFrame 扩展码 4/5/6 + dispatch 映射 | 测试存在（设计保障） |

## 6. 诚实边界（40 不冒充）

- **EIPC 端到端投递（Exit #6）**：40 收尾完成端点 cap 授予（EIPC-GRANT 实弹
  证据行）+ 协议错位修复（MR 布局/长度语义/分页/Final，见 §7.1 十条）——
  QEMU 每轮真实投递。**未冒充**：协议层修复均为实弹暴露后的最小修正，
  测试与证据行逐条绑定。
- **supervisor 超时 kill（Exit #5）**：svc-hang 死循环注入 → 心跳超时 →
  kill+restart 实弹。**未冒充**：演示靶为独立注入参考服务（svc-fault 同
  性质）；链上服务（heuristic bounded O(1)）无卡死风险；真实 LLM worker
  的卡死处理依赖 41 的取消/预算语义深化。
- **QEMU 外部虚拟设备写与读回（Exit #10）**：虚拟 PCS 设备（进程内寄存器
  模型，产品代码非 mock）真实写 + 独立读回 + 串口外部观察。**未冒充**：
  真实协议设备（Modbus/IEC104）与采集链的**设备侧** capability 移交
  （MMIO/DMA/IRQ cap 授予 EL0 服务）仍为 41 Entry 前置；root 采集代理注入
  样本为移交完成前的 QEMU 装配路径（样本经 evaluator 全量校验，不绕过
  信任评估）。
- AI 后端（Llama/HiGHS）target 不可用：profile 不声明（WP-40.4 允许），
  决策路径为静态 heuristic（bounded O(1)，只产生 proposal）——非伪造。
- board-prod verify-bundle BLOCKED（xtask 无 board 镜像模式——39 基线同态）。
- closure gate 4 项存量违规（39 已登记）非本卷引入。

## 7. Exit Gate 逐条核销

| # | Exit 项 | 结果 | 证据 |
|---|---|---|---|
| 1 | `SystemState::default()` 不入控制主路径 | PASS | Default 移除（编译期）；`unknown()` NaN fail-closed + t1 测试 |
| 2 | TrustedTelemetry 校验 coverage/freshness/quality/provenance | PASS | trusted 模块 13 测试（覆盖/新鲜/质量/范围/有限/重放/epoch/映射/完整性/摘要） |
| 3 | 四路径只产 Proposal/NoAction/Rejected | PASS | DecisionOutcome + coordinator 重构 + 全路径审计 + 59 测试 |
| 4 | 唯一 ControlDispatch 全链 | PASS | DispatchEngine 12 步 + 集成 8/8（Committed/Indeterminate/Failed/幂等/篡改/预算/过期/越限） |
| 5 | target AI worker 可取消/隔离 + demo 不产产品证据 | PASS | 取消 API ✓ + svc-ai-worker 入 bundle ✓（heuristic backend）+ DEMO_ONLY ✓；**supervisor 心跳超时 kill 实弹**（svc-hang 注入 → HEARTBEAT-TIMEOUT → kill+restart → 控制链继续，QEMU 证据行） |
| 6 | DeliveryChannel 真实 seL4 EIPC impl | PASS | **端点 cap 授予实弹**（EIPC-GRANT×4：energy-agent→telemetry/ai-worker/dispatch、dispatch→device-agent）+ Sel4EipcChannel + chunked 裸帧/分页协议 + QEMU 每轮真实投递（INGEST 349B/TICK/ControlEnvelope 270B） |
| 7 | device-agent 正式独立服务 | PASS | svc-device-agent 入 bundle + READY + ingress 构造全通 + PRODUCT-READY true |
| 8 | registry 可持久恢复且 API 对齐 PointAccess | PASS | persist v2（snapshot+WAL+generation+unavailable 标记）+ 9 测试 |
| 9 | AuthorizedWriter 复用且写路径不可绕过 | PASS | ControlDispatch 唯一 issuer + ingress 唯一 executor + 集成测试 |
| 10 | 外部虚拟设备/peer 观察真实写与读回 | PASS | **QEMU 实弹**：`VIRTUAL-PCS WRITE point=10 value=5`（真实写副作用）+ `INGRESS: Applied { echoed: Some(Float(5.0)) }`（独立读回）+ `CONTROL-CHAIN dispatch=Committed`（串口=外部观察面） |
| 11 | 无观测/坏质量/失联/写失败/重启/重复/迟到零误写 | PASS | 测试矩阵（energy-agent 209 + dispatch 集成 + ingress 负例） |
| 12 | 36–39 Gate 保持通过 | PASS | 全仓测试全绿；manifest/verify-bundle/kernel-policy/ADR PASS；closure 4 项存量（39 登记同源） |

**40 Exit Gate：12 PASS / 0 PARTIAL（40 范围）。**

## 7.1 40 收尾实弹修复（QEMU 实弹暴露并修复，见 §4.1）

EIPC 端到端实弹过程中暴露 40 原始协议层的**三处从未实弹验证的错位**（host 测试
无法覆盖 seL4 MR 语义），全部修复并绑定 QEMU 证据：

1. **MR 数据区从 MR[0] 开始**（seL4 非 MCS IPC 拷贝发送方 ipc buffer 前
   `length` words，从 offset 0 起）——40 原始 chunked 客户端/服务端均用
   MR[1..]，实弹下数据永远拷不到（响应端残留垃圾）→ 两端统一 MR[0..]；
2. **服务端必须按接收 MessageInfo 的 length（words）截断读取**——固定读满
   缓冲含残留字节导致协议解码 BadLength；
3. **Final 语义**：状态机在最后一块 data 时即 Complete 并复位，Final 再进
   状态机报 NoTransfer——Complete 后缓存完整载荷，Final 才触发 handler；
4. **分页响应（ENVP/ENVN）**：SNAPSHOT 145B 响应超 MR 112B 预算会被静默
   截断——响应方向分页拉取（每页 ≤104B，长度由协议头承载，消除 word
   粒度歧义）；
5. **跨进程时钟基准**：root 相对时钟（MonotonicClock 减 boot_ns）与 EL0
   服务绝对时钟（CNTPCT_EL0）混用导致样本全被误判 Stale——样本时间戳
   改用与服务端同基准的绝对时钟；
6. **INGEST 全量替换语义**：多轮样本累积导致快照评估同点重复 Replay /
   跨周期 ClockNotMonotonic——INGEST 先 clear 再入库；
7. **policy 版本**：`Policy::replace_rules` 使 generation +1——产品装配初始
   代取 `PROD_POLICY_VERSION - 1`（QEMU 实弹 StalePolicyVersion）；
8. **rate gate 演示配置**：60 次/小时在演示驱动频率下 6 分钟虚拟即撞限
   （round 61 Rejected）——demo profile 放宽（100000/h，产品值 41 注入）；
9. **死循环服务优先级**：任何高于 root 优先级的死循环都会饿死 root（seL4
   严格优先级抢占）→ 心跳判定无法运行——svc-hang（注入参考服务）与
   root 同优先级（1）轮转；
10. **心跳 probe 清除语义**：服务响应本轮请求（心跳 ≥ probe 轮）后解除
    pending——驱动周期 > 判限时无此清除必误杀空闲服务。

## 8. 允许进入下一版本

40 全部 12 项 Exit Gate 达成（0 PARTIAL）。**EIPC 端到端投递（Exit #6）实弹**：
root 采集代理 → svc-telemetry（evaluator 全量校验）→ svc-energy-agent（编排）
→ svc-ai-worker（heuristic 决策）→ svc-control-dispatch（唯一 ControlDispatch
全链）→ EIPC 270B ControlEnvelope 分块投递 → svc-device-agent（ingress 全链：
HMAC/lease/policy/safety）→ 虚拟 PCS 设备真实写 + 独立读回 → Committed。
**心跳超时 kill（Exit #5）实弹**：svc-hang 死循环注入 → HEARTBEAT-TIMEOUT →
kill+restart → 控制链继续（隔离成立）。**外部虚拟设备写与读回（Exit #10）
实弹**：VIRTUAL-PCS WRITE + INGRESS Applied echoed（串口=外部观察面）。

诚实边界（不冒充）：
- 虚拟 PCS 设备为进程内寄存器模型（真实副作用语义）；真实协议设备
  （Modbus/IEC104）仍依赖 39 的 transport 注入（CVR-39-02 的**设备侧**
  移交——EL0 服务拿 MMIO/DMA/IRQ cap），41 Entry 前置保留；
- 采集链代理：root 注入遥测样本（39 采集链设备句柄移交完成前的 QEMU
  装配路径）；样本经 evaluator 全量校验（不绕过信任评估）；
- Llama/HiGHS target 不可用：profile 不声明，决策为静态 heuristic
  （bounded O(1)，WP-40.4 允许）；
- board-prod verify-bundle BLOCKED（xtask 无 board 镜像模式——39 基线同态）；
- closure gate 4 项存量违规（39 已登记）非本卷引入。
