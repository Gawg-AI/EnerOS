# 04 · Agent/AI/场景域 商业交付级代码审查报告

- 审查日期：2026-08-11
- 范围：crates/agents（38 crate）、crates/ai（21 crate）、crates/scenes/firmware-agent、第三方接缝（llama.cpp / HiGHS 我方封装层）
- 方法：crate 规模分级（LOC 排名）→ 核心深读 + 薄壳快查 + 全仓 grep 消费方清点
- 依据：蓝图/27 §4 S 条目、蓝图/24（AuthorizedWriter 封焊）、蓝图/22 W 条目（双脑）

## 0. 摘要

| 项 | 数值 |
|---|---|
| 发现总数 | 12 |
| P0 | 0（控制面封焊核查无绕过路径） |
| P1 | 6 |
| P2 | 6 |
| 与蓝图/27 冲突项 | 3（见各发现 §冲突声明） |
| 已核查无发现 | 9 项（见 §3） |

**控制面安全结论（重点项）**：AI/Agent 全部控制写路径均经 `ChainCommandSink`（自证链，default-deny）→ `EnvelopeDispatcher`（签名信封经 EIPC）→ device-agent `EnvelopeIngress` 复验 → `AuthorizedWriter::write_authorized`。全仓 grep 确认 agents 域生产代码零 `ControlCommand` 直接构造（仅 device-agent 测试/场景文件）；`sink.write` 唯一调用点在 coordinator.rs:440（慢路径，Fatal 闸之后）。**未发现绕过 AuthorizedWriter 封焊面的路径。**

---

## 1. P1 发现

### AI-01 P1 · 信封 expiry 单位混用：毫秒 TTL 加到秒基准时钟

**类别**：BUG（时间语义）/ 安全削弱

**定位**：`crates/ai/dual-brain/src/chain_sink.rs:197`

```rust
now.saturating_add(cmd.ttl_ms as u64)
```

**证据链（三方确证信封为 epoch-秒基准）**：
- `crates/protocols/control-envelope/src/envelope.rs:24-25`：wire 格式注释明确 timestamp/expiry 为 epoch-**秒**
- `crates/protocols/control-gateway/src/gateway.rs:189`：`if now > env.expiry`（now 为秒）
- `crates/agents/device-agent/src/ingress.rs:437`：`let now_secs = now_ms / 1000`（设备侧复验除以 1000）

**根因**：`ttl_ms`（毫秒，coordinator 装配传 300_000 = 5 分钟）被直接加到秒基准 `now` 上 → expiry = now + 300000 **秒** ≈ 3.47 天。信封有效期被放大 1000 倍。

**影响**：过期语义名存实亡——信封在"名义 5 分钟"后仍可被投递端接受，重放窗口从 5 分钟扩至 ~3.5 天。ingress 复验仍会拦真正过期件，故非完全绕过，但纵深防御被削弱一层。

**修复**：
```rust
// chain_sink.rs：统一秒基准
let ttl_secs = cmd.ttl_ms / 1000;
expiry: now.saturating_add(ttl_secs.max(1))
```
或在 `DispatchCommand` 上改字段名为 `ttl_secs` 消除歧义（推荐，编译器强制消费方适配）。

**二次验证**：
- 调用方清点：`ChainCommandSink::new` 由 runner `run_once`（lib.rs:217）与 `build_production_sink`（lib.rs:300）装配；`DispatchCommand.ttl_ms` 由 coordinator 慢路径构造（coordinator.rs ~430）。共 2 处装配 + 1 处构造，改动面收敛。
- 不变量：修复后 expiry − timestamp ≈ TTL/1000 秒，与 gateway/ingress 的秒比较一致。
- 次生风险：若存在依赖"超长 expiry"的测试（chain_sink 单测断言 expiry = now + ttl_ms），需同步改为秒断言；gateway 租约/策略层若有独立 expiry 语义不受影响。

**host/QEMU 验证**：
```bash
cargo test -p eneros-dual-brain --features mock chain_sink   # expiry 断言改为秒
cargo test -p eneros-runner                                    # run_once 端到端
cargo test -p eneros-device-agent ingress                      # 复验接受新 expiry
```
判据：信封 expiry − timestamp == ttl_ms/1000 ± 1；device-agent 在 now_secs > expiry 时拒收。

**与蓝图/27 冲突**：间接削弱 S3（AuthorizedCommand 唯一入口的完整性假设——信封时效是封焊链一环）。

---

### AI-02 P1 · W2.3 降级模式死代码：`last_successful_schedule` 零赋值

**类别**：缺失功能（规格承诺未生效）

**定位**：`crates/ai/dual-brain/src/coordinator.rs`
- :113 字段声明、:166 `None` 初始化、:325 读取（降级分支）
- **全仓 grep 确认零赋值点**

**根因**：慢路径成功产出 schedule 后从未执行 `self.last_successful_schedule = Some(...)`。W2.3 承诺"连续 LLM 超时 → 降级模式用上一次成功决策"，实际降级分支永远读到 `None` → 只能走 fast-path 回退，"用上一次成功决策"永不生效。

**佐证**：lib.rs:604-627 测试自认"未测超时降级路径"。

**修复**：
```rust
// coordinator.rs 慢路径成功后（sink.write 前后均可，建议写成功后）：
self.last_successful_schedule = Some(final_schedule.clone());
```
签名不变。补测试：连续 N 次超时后 `run` 返回 `path_type == Degraded` 且 schedule 等于上次成功值。

**二次验证**：
- 消费方：仅 :325 降级分支一处，无其它读取。
- 不变量：降级 schedule 必经 gate 与 clamp（既有降级分支已含），无新增下发路径。
- 次生风险：若上次成功决策时点过旧（价格/SOC 已剧变），照发可能不经济——建议加 staleness 上限（如 >30 min 弃用），但这属增强，不阻塞修复。

**host/QEMU 验证**：
```bash
cargo test -p eneros-dual-brain --features mock degraded
```
判据：注入超时引擎 → streak 达阈值 → 返回 Degraded + 上次成功 schedule；再喂新超时 → fast-path 兜底。

**与蓝图/27 冲突**：违反蓝图/22 §2.W2.3 验收语义；S9 语境下相关"验收测试"实为未覆盖路径——建议该验收项在门禁标 PENDING 而非通过。

---

### AI-03 P1 · 快路径缺 Fatal 安全闸（P0-17 不对称）

**类别**：BUG（安全防御不对称）

**定位**：
- `crates/ai/dual-brain/src/coordinator.rs:263-284`：FastPath 分支直接 `return Ok(... fast_result.schedule ...)`，无 `validation.passed` 检查
- `crates/ai/fast-path/src/engine.rs:98`：`schedule: validation.clamped_schedule.clone().unwrap_or(schedule)` —— `validation.passed == false`（Fatal）时仍返回 schedule
- 对照慢路径：coordinator.rs:414 `if !validation.passed { return Err(SafetyFatal) }`（P0-17 闭环）

**根因**：P0-17 Fatal 阻断只在慢路径实现；快路径把校验结果当"建议"而非"闸"。

**影响边界**：协调器快路径**不写 sink**（sink.write 仅 :440 慢路径），故当前无直接控制面绕过；但 `DualBrainResult` 不暴露 validation 给调用方，调用方无法区分"安全通过"与"Fatal 违规"，任何未来把快路径结果接 sink 的改动都会直接破封焊。

**修复**：
```rust
// fast-path/engine.rs execute()：
if !validation.passed {
    return Err(FastPathError::SafetyFatal(validation.violations.len()));
}
// coordinator.rs 快路径分支随之经 ? 传播为 DualBrainError::SafetyFatal
```
并在 `DualBrainResult` 增加 `pub validation: ValidationResult` 字段（或至少 `passed: bool`）。

**二次验证**：
- 调用方清点：`RealtimePathEngine::execute` 仅被 coordinator 快路径与自身测试调用；`FastPathError` 增变体是兼容新增。
- 不变量：Fatal → Err → run_once 语义"零写入"（runner lib.rs:189 注释已承诺此语义，当前快路径反而不满足）。
- 次生风险：快路径 Fatal 频率若高会造成大量 Err——这是期望行为（fail-closed），但需指标计数（复用 rejection.rs `SafetyBlocked` 映射已存在）。

**host/QEMU 验证**：
```bash
cargo test -p eneros-fast-path --features mock
cargo test -p eneros-dual-brain --features mock fast_path_fatal
```
判据：构造越限 state → execute 返回 SafetyFatal；coordinator 快路径返回 Err 且 sink write_count 不变。

**与蓝图/27 冲突**：P0-17 是 27 §4 自身清单条目，快慢不对称 = 该 P0 未完整闭环。

---

### AG-01 P1 · EnergyAgent Error 态永久化：运营性错误 → 永久 Dead

**类别**：BUG（状态机缺陷）

**定位**：`crates/agents/energy-market-agent/src/energy_agent.rs:303-311`

**根因**：`on_tick` 对**任何** Err 一律 `self.state = AgentState::Error`，无恢复路径；`on_heartbeat` 见 Error → 永久报 Dead。问题在于错误集包含运营性错误——`GateRejected`（W5.2 限流/能量闸拒绝，正常高频现象）与求解器瞬时失败被当作致命错误同等处理。

**影响**：一次限流即令 agent 永久离线，需外部重启。市场场景下闸门限流是常态，该 agent 实际可用性趋零。

**修复**：
```rust
// energy_agent.rs on_tick 错误分流：
Err(EnergyAgentError::GateRejected(_)) | Err(EnergyAgentError::Transient(_)) => {
    self.consecutive_errors += 1;
    if self.consecutive_errors >= MAX_CONSECUTIVE { self.state = AgentState::Error; }
    // 否则保持 Running，下轮重试
}
Err(_) => { self.state = AgentState::Error; }
// on_tick 成功时 self.consecutive_errors = 0;
```
需将错误枚举分为 Fatal / Transient 两类（GateRejected、超时、限流 → Transient）。

**二次验证**：
- 消费方清点：`AgentState::Error` 被 on_heartbeat 与 fleet/coordinator 心跳聚合读取；改为可恢复后，心跳语义从"一次失败即死"变"N 次连续失败才死"，需同步 fleet 侧告警阈值文档。
- 不变量：Fatal 错误仍立即 Error→Dead，fail-closed 不放松。
- 次生风险：Transient 连续失败期间 agent 持续重试可能放大下游压力——consecutive 上限 + 既有 gate 限流足以约束。

**host/QEMU 验证**：
```bash
cargo test -p eneros-energy-market-agent --features mock state_recovery
```
判据：注入 GateRejected ×(N-1) → 仍 Running；×N → Error；随后成功 tick → 恢复 Running。

**与蓝图/27 冲突**：无直接 S 条目冲突；属 S1 triage 语境下的正确性缺陷。

---

### AG-02 P1 · W2.5 学习闭环"只学不用" + 单位错配

**类别**：缺失功能 + BUG（单位）

**定位**：`crates/agents/energy-market-agent/src/energy_agent.rs:289-290`

```rust
self.deadband_learner.learn(dev);
self.forecast_corrector.learn(predicted - fb.actual_revenue);
```

**根因（两层）**：
1. **单位错配**：`predicted - fb.actual_revenue` 是绝对偏差（元），但 `ForecastCorrector` 语义是无量纲比率——`correction()` 输出夹在 ±max_correction（0.5），`correct()` 做 `prediction * (1 + c)`。元级偏差（数十~数千）喂入后必然越界 → 永远触发 `rejected_updates` 回退 0 → 修正器恒为 0，学习实质失效。
2. **零消费**：全仓 grep `forecast_corrector.correct`/`.correction()`/`deadband_learner.current` —— **生产代码零消费方**。W2.5"学习输出反哺预测/死区"承诺未闭环，学出的值无人使用。

**修复**：
```rust
// ① 归一化偏差（相对误差）：
let rel_bias = if predicted.abs() > f64::EPSILON {
    (predicted - fb.actual_revenue) / predicted.abs()
} else { 0.0 };
self.forecast_corrector.learn(rel_bias);
// ② 接线消费：构造 LP 输入前
let corrected_price = self.forecast_corrector.correct(spot_price);
let deadband = self.deadband_learner.current; // 喂遥测上报死区配置
```

**二次验证**：
- 消费方清点：learning.rs 学习器自身经 SafetyCatalog 校验（W5.3，已核查合格），接线消费不新增未校验路径。
- 波及面：energy-market-agent 决策输入变化 → 市场报价行为变化，需回测。
- 次生风险：修正系数作用于价格输入会改变投标策略——max_correction 0.5（±50%）界内，且 catalog 校验在位，风险受控。

**host/QEMU 验证**：
```bash
cargo test -p eneros-energy-market-agent --features mock learning_loop
```
判据：喂 20% 相对偏差 100 轮 → correction() ≈ +0.2 且 ≠ 0；rejected_updates 不再持续增长。

**与蓝图/27 冲突**：蓝图/22 W2.5 验收项当前仅"学习器单测通过"，未验证闭环——按 S9 精神应标注覆盖不全。

---

### AG-03 P1 · 5 个市场/优化配置无生产消费代码（桩化能力）

**类别**：缺失功能（配置-代码脱节）

**定位**：`configs/` 下
- `bid_strategy.toml`、`spot_market.toml`、`realtime_market.toml`、`domain_optimizer.toml`、`multi_objective.toml`

**证据**：全仓 grep 确认五个 TOML **无任何 Rust 生产加载代码**（无 include_str!/toml::from_str 指向这些路径）。对应实现符号 `BidGenerator`、`MarketSubscriber`、`MultiDeviceDispatcher`、`MultiObjectiveOptimizer` 存在，但**仅被测试使用**；energy-market-agent lib.rs:193-229 大量导出符号被 `#[cfg(any(test, feature = "mock"))]` 门控。

**根因**：市场/优化模块实现停留在"单测可过"层面，未装配进任何生产二进制。

**影响**：商业声称的现货/实时市场投标、多目标优化能力实际不存在于运行系统——这是交付面问题而非纯代码 bug。

**修复**（二选一，需产品决策）：
- A（补闭环）：energy-market-agent 增加 `load_market_config(path) -> Result<MarketConfig>` 并在 agent 装配期消费，接入 on_tick 决策；
- B（诚实降级）：将 5 个 TOML 标记 `status = "deferred"`，蓝图 32 记录为 v1.x 能力，避免验收声称已具备。

**二次验证**：
- 波及面：A 方案涉及 agent 装配链 + configs 装配脚本（host 侧）；B 方案仅文档/标注。
- 不变量：不触及控制写路径。

**host/QEMU 验证**（A 方案）：
```bash
cargo test -p eneros-energy-market-agent --features mock config_consume
```
判据：装配 bid_strategy.toml → BidGenerator 在生产 cfg 下可用（非 mock 门控）。

**与蓝图/27 冲突**：与 S6"配置只在 host/装配期生效"的表面合规下隐藏能力缺失——配置存在但无消费者，S6 的"active/dormant"分类无法反映此事实，需在蓝图 34 验收清单显式核对。

---

## 2. P2 发现

### AI-04 P2 · InferScheduler 请求队列无上限（QueueFull 声明未兑现）

**类别**：BUG（无界增长）

**定位**：`crates/ai/infer-scheduler/src/scheduler.rs:50-57` —— `submit()` 无条件 `push_back`；`crates/ai/infer-scheduler/src/error.rs:46` 声明 `SchedulerError::QueueFull` 但全仓无任何返回点。

**根因**：错误变体先行声明，容量检查未实现。

**影响**：单线程轮询模型下提交速率受调用方约束（当前 coordinator 同步 submit→tick 循环，实际队列恒 ≤1），当前风险低；但 API 层面无背压，后续接入多请求源即成无界内存增长。

**修复**：
```rust
pub fn submit(&mut self, mut req: InferRequest) -> Result<u64, SchedulerError> {
    if self.queue.len() >= MAX_QUEUE_DEPTH { // 建议 const 64，可配置
        return Err(SchedulerError::QueueFull);
    }
    ...
}
```
coordinator `llm_infer_bounded` 对 QueueFull 按超时降级同等处理。

**二次验证**：调用方仅 coordinator 一处 + 测试；返回类型变化需同步 lib.rs 测试（queue_len 断言处）。

**host/QEMU 验证**：
```bash
cargo test -p eneros-infer-scheduler queue_full
```
判据：填满 MAX_QUEUE_DEPTH 后第 N+1 次 submit 返回 QueueFull。

**与蓝图/27 冲突**：无。

---

### AI-05 P2 · LP 建模瑕疵：时段 0 充放电不影响 SOC（免费能量窗口）

**类别**：BUG（建模）

**定位**：`crates/ai/energy-lp-model/src/model.rs:127` 附近 —— SOC 动态约束从 t=1 起，`soc[0]` 被 `soc_init` 钉死。

**根因**：时段 0 的 charge/discharge 变量不进入任何 SOC 守恒约束 → 求解器可"免费"充放电（放电不扣 SOC、充电不占 SOC 空间）。另有互斥约束 `charge + discharge <= pcs` 为线性松弛，允许半充半放（同时充放，物理不可能）。

**影响**：目标函数倾向在时段 0 同时最大化充放收益项；实际影响受目标函数系数与边界约束压制，非必然灾难，但最优解含物理不可行分量。

**修复**：
```rust
// SOC 动态从 t=0 起：soc[1] = soc[0] + η_c*charge[0] - discharge[0]/η_d，soc[0]=soc_init
// 时段 0 约束并入循环起点（t in 0..T-1）
// 互斥：引入方向二元变量或分段建模（若求解器支持 MIP）；
// 否则在 safety-validator 后置校验中拒绝 charge>0 && discharge>0 的时段
```

**二次验证**：parse_result 消费方为 fast-path 与 coordinator 慢路径；约束数增加对 HiGHS 求解时长影响可忽略（T=24~96）。次生风险：修复后最优值可能劣化（正确解更贵），属预期。

**host/QEMU 验证**：
```bash
cargo test -p eneros-energy-lp-model soc_conservation
```
判据：任意解满足全时段 SOC 守恒；时段 0 无同时充放。

**与蓝图/27 冲突**：无。

---

### AI-06 P2 · PathSelector.load_change_threshold 声明未使用（文档-实现偏差）

**类别**：代码卫生 / 规格偏差

**定位**：`crates/ai/fast-path/src/selector.rs:32`（声明、:47 初始化），`select()`（:61-100）只检查 price 与 SOC 变化，负荷变化从未参与判断。

**根因**：结构体文档（:20-25）声称按"状态变化阈值"选路，实现缺负荷维度。

**影响**：负荷剧变（如大负载投切）时不会触发慢路径重规划，快路径沿用旧策略表配置。

**修复**：
```rust
// select() 中 SOC 检查后追加：
let load_delta = (state.load_demand.total_kw() - last_state.load_demand.total_kw()).abs();
if load_delta > self.load_change_threshold { /* 走慢路径，同现有模式 */ }
```

**二次验证**：select 调用方仅 coordinator；新增慢路径触发频率上升会加重 LLM 负担——由 min_slow_path_interval（300s）兜底。

**host/QEMU 验证**：`cargo test -p eneros-fast-path selector_load_trigger`；判据：负荷跳变 25kW（>20 阈值）→ SlowPath。

**与蓝图/27 冲突**：无。

---

### AI-07 P2 · runner CHAIN_KEY 硬编码（已注释为 v1.0 交付项）

**类别**：安全（密钥管理）/ 观察项

**定位**：`crates/ai/runner/src/lib.rs:380`
```rust
const CHAIN_KEY: &[u8] = b"eneros-runner-chain-key-000000000000000000";
```
用于 `build_chain()`（run_once 路径）自证链密钥。

**现状**：源码注释声明"生产由运维注入——v1.0 交付项"，且 device-agent 侧链验证密钥独立，实际封焊不依赖此 key 保密性。

**修复**：`run_once` 增参 `chain_key: &[u8]`，CLI 从 `--chain-key-file` 读取；缺省拒绝启动（fail-closed）。

**二次验证**：main.rs CLI 解析处一处改动；测试用固定测试密钥。

**host/QEMU 验证**：`cargo test -p eneros-runner`；判据：无 key 文件 → 退出码非 0。

**与蓝图/27 冲突**：无直接冲突，但蓝图 34 商用验收必须核对此项已兑现，否则标 PENDING。

---

### AG-04 P2 · DecisionAudit（W5.1）零生产消费者

**类别**：缺失功能

**定位**：`crates/ai/dual-brain/src/audit.rs` 与 `tests/gr5_decision_discipline.rs`；runner 与 energy-market-agent 均未接线。

**根因**：审计结构实现+测试在位，但未接入任何生产决策流。W5.1"AI 决策进审计链"承诺未闭环。

**修复**：coordinator 慢路径决策点（sink.write 前）构造 `DecisionAudit` 记录并经 `AuditSink` trait 输出；runner 装配文件型 AuditSink（与 FileCommandSink 同模式）。

**二次验证**：新增只读旁路，不影响控制路径；需给 AuditSink 加有界缓冲（防磁盘满阻塞决策——write 失败计数+丢弃，不阻塞）。

**host/QEMU 验证**：`cargo test -p eneros-dual-brain --features mock audit_wired`；判据：慢路径决策产生审计记录含 path/validation/gate 字段。

**与蓝图/27 冲突**：W5.1 验收按 S9 应标未覆盖。

---

### AI-08 P2 · coordinator.rs:42-43 重复 cfg 属性（表面问题）

**类别**：代码卫生

**定位**：`crates/ai/dual-brain/src/coordinator.rs:42-43` 连续两行相同 `#[cfg(any(test, feature = "mock"))]`。

**修复**：删除重复行。**与蓝图/27 冲突**：无。

**host/QEMU 验证**：`cargo check -p eneros-dual-brain` 通过即可。

---

## 3. 已核查无发现（合格项）

| # | 核查项 | 结论 |
|---|---|---|
| 1 | **控制面封焊面（重点）** | agents 域生产代码零 `ControlCommand` 直接构造；`sink.write` 唯一调用点 coordinator.rs:440 在 Fatal 闸后；device-agent ingress 复验 → write_authorized 链完整。**无绕过路径，无 P0** |
| 2 | ChainCommandSink 自证链（授权→租约→策略→SBO→安全，default-deny，拒绝映射结构化） | 设计正确，测试充分 |
| 3 | EnvelopeDispatcher（握手有界重试 ≤5、fail-closed、篡改拒绝） | 合格，有 host 集成测试 |
| 4 | HiGHS 封装 `solver-core/src/highs.rs` | RAII 释放、参数类型分发、数组长度校验齐全 |
| 5 | llama.cpp 封装 `llm-engine/src/llama_cpp.rs` | RAII 逆序释放、UTF-8 跨 token 累积、argmax 采样；**模型缺失 fail-closed**（load_model → LoadFailed），`test-model.gguf` 仅测试路径引用，生产无硬依赖 |
| 6 | intent-parser/converter | 围栏剥离/别名/字符串数字归一化 + 安全索引；validate_config 校验齐备，LLM 输出解析无越界/注入面 |
| 7 | mock 门控（26-B 回退核查） | 23 个文件含 release `compile_error!` 熔断（llm-engine/solver-core/dual-brain/runner/energy-market-agent 等关键 crate 全覆盖）；`default_with_mock` 等构造均 `#[cfg(any(test, feature="mock"))]` 门控，符合 S2 范式，**未发现回退** |
| 8 | unwrap 热点清点 | autonomous/orchestrator.rs（40 处）、federation/tunnel.rs（45 处）、twin-agent/mirror.rs（45 处）**全部位于 #[cfg(test)] 区**，生产路径无 unwrap 热点；firmware-agent main.rs 的 unwrap 为骨架演示且有注释声明 |
| 9 | firmware-agent（126 行） | 场景骨架，无真实适配器时 fail-closed 退出（exit 2），符合 R2-1 |

---

## 4. 与蓝图/27 冲突声明汇总

| 发现 | 冲突条目 | 性质 |
|---|---|---|
| AI-03 | 27 §4 P0-17（Fatal 阻断） | 快路径未闭环 = P0 项不完整，建议 27 清单标注 |
| AI-02 / AG-04 | S9（skip/PENDING 不计入通过） | W2.3 降级、W5.1 审计的验收测试存在但覆盖路径未实现，应按 PENDING 计 |
| AG-03 | S6（配置 host/装配期生效） | 5 TOML 无消费者，S6 分类不能掩盖能力缺失，蓝图 34 验收需显式核对 |
| AI-01 | 间接削弱 S3 封焊链时效假设 | 非违反，属削弱 |

## 5. 审查效率备注

59 crate 分级处理：核心深读 12 个（dual-brain、runner、energy-market-agent、fast-path、solver-core、llm-engine、infer-scheduler、energy-lp-model、intent-parser、safety-validator、device-agent ingress、firmware-agent）；其余 47 个经 LOC 排名 + unwrap 热点扫描 + 控制写符号 grep 快查，未发现未报告的异常。
