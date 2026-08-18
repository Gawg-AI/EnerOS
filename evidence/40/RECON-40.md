# RECON-40 — 蓝图 40 入口侦察（Agent、AI 与设备控制闭环修复蓝图）

> schema: `eneros.blueprint-40.recon.v1`
> 编制：2026-08-15（40 执行会话，恢复自 2026-08-15 暂停的侦察）
> 分支：`feature/v0.165.0-protocol-driver-sdk`；HEAD=`b96774e`；工作树 266 改动条目（36/37/38/39 未提交代码，本卷保护不覆盖）
> 基线：`蓝图/35-00`、`35-01`、`39` 蓝图 + `evidence/39/39-验证记录与Exit-Gate核销.md`
> Entry Gate：39 的安全存储、网络服务、协议插件与独立软件 peer 通过（10 PASS/1 PARTIAL，PARTIAL 面为本卷 Entry 前置：svc 设备句柄移交、net 服务装配、协议 production transport 注入，见 §0）

## 0. Entry Gate 前置（39 PARTIAL 面 → 40 承接）

- 39 Exit #5/#6/#8 PARTIAL：EL0 设备句柄移交（MMIO/DMA/IRQ cap 从 root DriverSupervisor 转给 EL0 服务；CVR-39-02/CVR-38-01 登记）、EL0 NetworkStackService 装配、协议 production transport 注入——均依赖本卷 EIPC 双端与服务装配。
- 39 closure gate 存量违规 4 项（eneros-crypto usage×3 + ecdsa 未登记）：非 39 引入，属 34/41 域存量缺口，本卷不修、如实登记。
- 39 观察点：`net_echo_poll` 经全局指针访问 net 驱动（root 单线程）；svc-storage 的 EL0 attach 随 40 移交；`ready_ntfn`/`READY-SIGNALED`/`PRODUCT-READY` 机制已就绪，新服务可直接复用 `drv-svc::runtime::driver_service_main` 模式。

## 1. EnergyAgent observer/tick/feedback 全路径

- `crates/agents/energy-market-agent/src/energy_agent.rs`：
  - `DeviceObserver::observe_device() -> Option<DeviceObservation>`（:38-41）；`with_observer`（:152-155）**全仓零生产 caller**（仅测试 StubObserver/NoneObserver）。
  - `on_tick`（:246-361）：观察 Some → 构造 `SystemState{voltage_v, current_a, frequency_hz: 50.0 硬编码, soc_pct, timestamp_ms}`（:255-262）；None → `SystemState::default()`（:263-266）只计数 `outcome_unknown_count`，**决策继续**（P0-09 缺口本体）。
  - feedback：观测存在且预测有限即 `learn()`（:315/319）——**与 ACK/读回/Committed 无关**；`validation_passed=true` 仅凭观测存在（:299）。
  - twin：仅 feedforward（`set_twin_forecast` :164-167）；无观测回灌 twin。
- `SystemState` 定义 `crates/ai/safety-validator/src/state.rs:12-36`，`Default` = 380V/0A/50Hz/SOC0.5/0ms（:25-36，"典型运行点"= 蓝图禁止的默认健康值）。
- `SystemState::default()` 生产使用点（须全部处理）：
  1. `energy_agent.rs:265`（观测缺失 → default）；
  2. `dual-brain/src/coordinator.rs:157`（`IntentParser::new(config, SystemState::default())` D9 构造）；
  3. `fast-path/src/state.rs:25`（`RealtimeState::default`）；
  4. `runner/src/scenario.rs:73`（host demo 输入，可保留 demo 构造）；
  5. `intent-parser/src/parser.rs:303`（`Default for IntentParser`）。
- `TickOutcome` 不存在；`on_tick -> Result<(), AgentRuntimeError>`（runtime.rs:25），`Ok` 仅表示"tick 执行完"，FastPath 返回计划但没写 sink 时同样 Ok（P0-10 缺口）。

## 2. dual-brain Fast/Slow/Fallback/Degraded 与 sink/audit/gate 调用

- sink：`CommandSink{write(DispatchCommand)->Result<(),DualBrainError>; write_count()}`（`sink.rs:31-39`）；生产 `ChainCommandSink`（`chain_sink.rs:105-249`，write = sbo_select → On/Off → EnvelopeBuilder 签名 → `chain.execute` → `DeviceExecutor::write` 真实写）；`MockCommandSink` 仅 test|mock。
- `execute(state, now_ms)`（coordinator.rs:262-549）四路径：
  | 路径 | 位置 | 行为 | sink | gate.record | audit |
  |---|---|---|---|---|---|
  | 速率闸 | :275-277 | Err(GateRejected) | - | 否 | 否 |
  | FastPath | :282-312 | Ok(schedule) | **否** | **是（:304，下发前）** | 否 |
  | Degraded | :352-363 | Ok(last_successful_schedule) | **否** | 否 | 否 |
  | Fallback | :364-383 | Ok(fast.schedule)（伪装 FastPath） | **否** | 否 | 否 |
  | SlowPath | :316-548 | 校验→sink.write(:486)→gate.record(:490)→audit(:520)→Ok(feedback) | 是 | 是（空调度也 record） | 是 |
- `PathType{SlowPath,FastPath,Degraded}`（`fast-path/src/selector.rs:9-16`）**无 Fallback 变体**。
- 慢路径 audit 在 sink 写后记 `ExecOutcome::Succeeded`（:512-513），无 transport ACK/readback 概念。
- `DecisionOutcome/ProposedControlPlan/NoActionReason/DecisionRejection` **全仓零命中**（仅蓝图 md）；`DualBrainResult{path_type, schedule, latency, feedback}`（coordinator.rs:61-70）是唯一返回形态。
- gate：`DecisionGate::new(60, 1000.0, 3_600_000)`（coordinator.rs:174-178）；`record`（gate.rs:107-110）push 窗口+used_kwh；被拒不 record。
- Fatal：`DualBrainError::SafetyFatal`（error.rs:19）；`RejectionHandler::classify`（rejection.rs:97）SafetyBlocked → EscalateToHuman。

## 3. runner 的 host/target feature 与所有 DeliveryChannel impl

- `crates/ai/runner`：**host std crate**（main.rs 用 std::process/fs/thread）；`main.rs:78` 唯一 bin `eneros-ai-runner`；`main.rs:100` **固定 `FeedbackBundle::untrusted`** → `chain.execute` 第 7 步 reconcile 必拒（`control-gateway/src/chain/execute.rs:93-114` FeedbackUntrusted → Denied）→ 正常模式必失败退出（与 WP-40.5 要求一致，但当前是"唯一入口必拒"而非"分离后 demo 诚实"）。
- sink：`FileExecutor`（`sink.rs:85-132`，JSONL `eneros.executor-file.v1`，172B 信封 hex 落盘，无 fsync）——生产路径使用（run_once → ChainCommandSink → FileExecutor）；`build_production_sink`（lib.rs:316-357，EnvelopeDispatcher + 握手 + ChainCommandSink）**仅测试调用**。
- `DeliveryChannel{request(target,payload)->Result<Vec<u8>,ChannelError>}`（`dispatcher.rs:27-31`）；全部 6 个 impl（FailChannel/InMemory/Tamper/Dead/Capture/Denied）均 `#[cfg(test)]`；**无生产 impl**；`EnvelopeDispatcher`（dispatcher.rs:45-166，握手有界重试 ≤5、fail-closed Offline）本身生产可用，只缺真实通道。
- `wire.rs`（agent-sdk）：magic "EIPC"+ver1+CRC32；`MessageType::{Send=1,Request=2,Response=3,**Cancel=4**,Event=5}`——Cancel 类型已存在但 Sel4IpcClient 无取消 API；`MAX_WIRE_SIZE=4178`。
- `Sel4IpcClient`（agent-sdk/src/ipc_sel4.rs:54，`cfg(any(test, feature="sel4"))`）：`new(endpoint, dest_cap, now_ns)`/`request`（correlation id 校验，MR 预算 120B）/`send`；**全仓零生产调用方**；no_std + alloc + heapless。

## 4. device-agent ingress/registry/persist/CommandSource 全部 caller

- `process_envelope` 生产调用链：`agent.rs:272-274 DeviceAgent::process_envelope`（唯一生产入口，**无生产 caller**）；其余全是 cfg(test)。
- ingress 链（`ingress.rs:431-605`）：DeliveryFrame::decode → CommandEnvelope::decode → HMAC 预检（:463）→ 路由/点匹配（:474-490）→ **真实反馈 build_feedback**（:496,613-639：5 点真实回读，缺/坏 → untrusted）→ chain.sbo_select + chain.execute（:501-502）→ `adapter.write_authorized`（:515-518）→ readback（:523-530，`WriteOutcome.echoed` 优先，**Mock 回显刚写值 = 违反"读回不得回显 payload"**；None 时才 `read_point`）→ Applied/deny（:642-669 审计+指标，绝不 panic）。
- `AuthorizedCommand`：`telemetry-model/src/command.rs:134-215` 全私有字段 + `#[doc(hidden)] from_parts`；**唯一生产构造 `ControlGateway::authorize`**（gateway.rs:179-242）；其余 from_parts 调用全测试。
- `AuthorizedWriter`：`protocol-abstract/src/access.rs:79-89`；`PointAccessWriter = PointAccess + AuthorizedWriter`（:95-97）；writer adapter：modbus-tcp `adapter.rs:158`、modbus-rtu `adapter.rs:182`、iec104-master `adapter.rs:190`、MockAdapter（test）。`PointWriteIntent`（`write.rs:11-46`）已定义**零消费**。
- `DeviceRegistry`（registry.rs:238-307）：内存 BTreeMap；register 5 参；**无 update/remove/持久化**；adapter 即 `Box<dyn PointAccessWriter>`。
- `CommandSource`（command.rs:23-26）：唯一 impl `MockCommandSource`（test|mock）；`DeviceCommand`/`execute_commands` 双轨（agent.rs:258-266）只拒不执行；`firmware-agent`（crates/scenes）host demo 使用。
- `persist.rs`：**未编入**（lib.rs:53-59 无 `mod persist`），三处漂移（register 4 参 vs 5 参、`crate::registry::PointAccess` 路径不存在、`DeviceType::Other` 不存在）；CSV 格式无 device_id；`save_registry`/`load_registry` 签名旧（FileSystem 泛型可用，`fs_trait.rs:31/77`）；load 重建闭包直接 skip 丢设备。**基线作废，WP-40.10 重规**。
- 无 bin/`fn main`/服务循环；`assembly_demo.rs`/`e2e_scenarios.rs` 均 cfg(test)。

## 5. AuthorizedWriter 的 Modbus/IEC104 adapter

- 三个 adapter 实现 `AuthorizedWriter`，模式一致：device/point 映射二次匹配 → CommandMismatch；载荷类型 → Unauthorized；协议写失败 → Protocol；Modbus 回读可空 echoed。
- `ControlChain::execute`（`chain/execute.rs:11-137`）10 步：authorize → lease 墓碑 → idempotency → OverrideAuthority（EmergencyBypass 跳过 PDP 不过安全）→ PDP+scope → SBO → **reconcile 反馈**（:93-114）→ safety.validate → DryRun → record。untrusted 必拒（FeedbackUntrusted）。
- 信封 wire（`envelope.rs`）：172B 定长、VERSION=2、全小端；EnvelopeBuilder 默认 fencing/policy=0 → 结构拒绝；HMAC-SM3 over [0..140]。
- `DeliveryFrame{envelope[172], power_kw f64}`（delivery.rs:26-56，DELIVERY_LEN=180）；`OutcomeFrame{code,class}` 2B（CODE_APPLIED=0/IDEMPOTENT_HIT=1/DRY_RUN=2/DENIED=3）；`HANDSHAKE_MARKER=[0x00]`。

## 6. agent-sdk Sel4IpcClient 与 root service declaration

- 服务 CSpace 槽位：0=endpoint（mint badge=0）、1=fault_ep（badge=idx+1）、2=ready ntfn（badge=idx+1）；`_start(ep, ipc_buffer, ready_ntfn)` 收参（spawn.rs:1363-1389）；`CAP_DEVICE_IO(1<<19)` 已登记（bundle.rs:52）但**无授予路径**（CVR-39-02：EL0 拿不到 MMIO/DMA/IRQ cap）。
- 服务循环参考：`drv-svc/src/runtime.rs:370` `driver_service_main(ep, name, ready_ntfn, now, epoch, service)`：READY-SIGNALED → loop ipc_recv → process_request（有界队列+deadline+epoch+capability 归因）→ reply；错误 fail-closed 挂起；`DriverService{capability()->Option<&str>; handle(req,resp)}`；ST_* 状态码；`#![no_std]` 无 alloc。
- root 侧：`service.rs:88-96 run_ipc_loop(handles, ...)`；readiness 收集（service.rs:189-337）→ `PRODUCT-READY: <required_degraded.is_empty()>`；按名硬编码 svc-hello/svc-fault（service.rs:107-125）。
- profile：qemu-prod.json 20 服务（root-task + 5 demo required + 14 optional）；board-prod.json 驱动服务 required。`ServiceSpec`（product_manifest.rs:150-182）；`Requirement::{Required,Optional}`；`validate()`（:330-345）/`validate_bound()`（:414-425）/`verify_media_bundle()`（product_deriver.rs:281-347）。
- **required-service 删除阻断不可机器验证**：evidence/36 M5 显示删除 required 后 manifest/bundle 自洽即 PASS——需本卷新增基线集合比对（蓝图 40 §7 要求）。
- `SERVICE_KIND_REGISTRY`（product_manifest.rs:699-740）：control.telemetry/energy-agent/ai-worker/device-agent 已登记；**control.dispatch 未登记**；`validate_service_kinds` 无 caller（36 决定不接线，本卷只补 registry 条目不接线）。
- normalize → `product_deriver.rs:136-177 init_bundle()`（kind=2 required/flags=criticality|capability 位图）；REVERSE-PARSE-GATE 在 image compose 后（image_factory.rs:1904-1912）。

## 7. solver/llm 的 target/native backend 边界

- `llm-engine`：`LlmEngine` trait（engine.rs:16-57，no_std）；`LlamaCppEngine` 仅 `feature="llama-cpp"`（llama_cpp.rs:46，host native 静态库 libllama.a + stdc++/gomp，build.rs cmake）；`mock` 仅 test|mock + release compile_error；**target 无 LLM 后端**——target profile 必须不声明。
- `solver-core`：`Solver` trait（solver.rs:22-48）；`HighsSolver` 仅 `feature="highs-ffi"`（highs.rs:19，host native libhighs.a）；**default=["highs-ffi"]**（注意：默认带 host 后端）；`mock` release 熔断。solver-milp/solver-model 透传 feature。
- infer-scheduler：`MAX_QUEUE_DEPTH=64` QueueFull（scheduler.rs:59）；timeout 仅排队阶段（:78-85）；**已开始的 infer 不可取消**（tick 同步内联 :109）；InferRequest 无 correlation/budget/model digest；`llm_infer_bounded` 同步自旋（coordinator.rs:224-249）。
- runner/dev-deps/energy-market-agent 无条件依赖 llama-cpp feature（host 污染面，40 只分离 demo，不修 xtask 污染——42 域）。

## 8. 未提交改动保护清单（40 不得破坏）

- `crates/ai/dual-brain/Cargo.toml`：39 时代新增 eneros-crypto 依赖（AG-04 审计创世锚派生）——保留。
- `crates/ai/intent-parser/src/lib.rs`：工作树有改动（git status M）——先看 diff 再动。
- `crates/agents/federation/*`、`market-types`、`drivers/net/*`、`protocols/*`（iec104/mqtt/modbus/tsn 等 39 产物）——本卷不得重写。
- 36/37/38 全部未提交代码（manifest/image/ci/gate/root-task/hal/drivers）——40 只读不改，除非 CROSS_VERSION_CHANGE_REQUEST。
- 全仓禁止：git reset/clean/checkout 覆盖、自动 commit/tag/push。

## 9. 40 缺口清单（WP → 现状 → 动作）

| WP | 现状 | 动作 |
|---|---|---|
| 40.1 | TrustedTelemetry 不存在；SystemState::default 5 处生产使用 | 新 trusted-telemetry 模块（TelemetrySample/Quality/TrustedTelemetry/TelemetryTrustEvaluator）；SystemState 去 Default + TestSystemStateBuilder；修 5 处 caller |
| 40.2 | TelemetryService 不存在；DeviceObserver 零生产 caller | svc-telemetry bin + TelemetryService（有界队列/去重/epoch/质量传播）；manifest 声明 |
| 40.3 | DecisionOutcome 不存在；3 路径不 dispatch、FastPath gate 提前 record、PathType 无 Fallback | coordinator 返回 DecisionOutcome{Proposal/NoAction/Rejected}；四路径统一 proposal；PathType+Fallback；audit 全覆盖；gate.record 移 confirmed dispatch |
| 40.4 | 已开始 infer 不可取消；无 correlation/budget/model digest | infer-scheduler 增 cancel/correlation/deadline/budget；svc-ai-worker 进程隔离 + supervisor 重启；late result 丢弃审计 |
| 40.5 | runner 单 bin 生产路径 JSONL+untrusted 必拒 | 拆 `eneros-ai-runner-demo`（host JSONL，DEMO_ONLY）+ `svc-ai-worker`（no_std EIPC）；FileExecutor 不实现生产 DeliveryChannel |
| 40.6 | ControlDispatch 不存在 | 新 control-dispatch crate：trait + DispatchState 12 态 + 12 步链（validate→authorize→persist→deliver→ACK→readback→audit） |
| 40.7 | AuthorizedWriter 齐备；CommandSource 双轨未删 | ControlDispatch 唯一 issuer（经 ControlGateway::authorize）；device-agent 唯一 executor；release 删 legacy anonymous input |
| 40.8 | DeliveryChannel 无生产 impl；Sel4IpcClient 零调用 | Sel4EipcChannel（feature="sel4"）+ ControlEnvelope versioned wire；EnvelopeDispatcher 复用 |
| 40.9 | device-agent 无服务 | svc-device-agent bin（EIPC loop + process_envelope 核心 + rate limit + drain） |
| 40.10 | persist.rs 死代码漂移 | DeviceRegistration + snapshot+WAL + update/remove + load 重绑 + 审计；API 对齐 PointAccess |
| 40.11 | 单层 Applied{echoed}，Mock 回显刚写值 | 分层 Delivered/TransportAcked/ReadbackVerified/Committed；readback 独立读 + 按点类型 policy |
| 40.12 | learning 无 outcome 门控；无 Indeterminate | 失败对账状态机；learning 只消费 Committed；twin 用 observed state |
| 装配 | 20 服务无 telemetry/energy-agent/ai-worker/control-dispatch/device-agent；required 删除不阻断 | 5 新 bin + profile ×2 + SERVICE_KIND_REGISTRY + required 基线 Gate |

## 10. 验证计划

- 每 WP：host 测试（正例/负例/故障/恢复）+ mutation；5 服务 host 构建 + aarch64 target 构建；manifest normalize + verify-bundle；QEMU 装配轮（时间允许则实弹，否则 BLOCKED_EVIDENCE 如实登记）；Exit Gate 逐条核销；evidence/40 收口。
