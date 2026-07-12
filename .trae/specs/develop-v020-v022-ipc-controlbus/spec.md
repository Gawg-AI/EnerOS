# v0.20.0 ~ v0.22.0 — IPC 与 Control Bus（P0-G）Spec

## Why

Phase 0 收官需要"基础 OS 服务就绪"——IPC 是 Agent/RTOS/内核间通信的基础，Control Bus 是双平面（快平面 RTOS + 慢平面 Agent）联通的安全闭环。当前 v0.19.0 完成了分区调度，但分区间尚无消息传递机制；Agent 崩溃后 RTOS 无法自动降级到安全默认策略。本 spec 一次性开发 v0.20.0/v0.21.0/v0.22.0 三个版本，完成 P0-G 全部交付物并承载 Phase 0 出口标准验证。

## What Changes

### v0.20.0 — IPC 同步消息传递
- 新增 crate `eneros-ipc`（`crates/kernel/ipc/`），含 `endpoint.rs`/`notification.rs`/`channel.rs`
- 实现 endpoint-based 同步 IPC：`endpoint_create`/`endpoint_destroy`/`send`/`recv`/`call`
- 实现 notification 位图机制：`notify`/`wait_notification`
- **修改** `crates/kernel/sched/src/tcb.rs`：新增 `current_tid()` + `set_current_tid()`（IPC 需要知道当前线程）
- **修改** `crates/kernel/sched/src/lib.rs`：导出 `current_tid`/`set_current_tid`

### v0.21.0 — Lock-free SPSC Ring Buffer
- 同 crate `eneros-ipc` 新增 `spsc_ring.rs`/`shared_mem.rs`
- 实现单生产者单消费者无锁环形缓冲区：`SpscRing::new`/`push`/`pop`/`used`/`free`
- 实现共享内存区域授权：`SharedMemRegion`/`grant_shared_mem`
- 使用 `core::sync::atomic::{AtomicUsize, Ordering}` 的 `Acquire`/`Release` 配对

### v0.22.0 — ControlCommand + TTL + 双平面联调（★ 瓶颈版本，Phase 0 出口验证）
- 新增 crate `eneros-controlbus`（`crates/kernel/controlbus/`），含 `command.rs`/`ttl.rs`/`constraint.rs`/`fallback.rs`/`integration.rs`
- 实现 `ControlCommand` 命令结构（cmd_id/timestamp/ttl_ms/target_device/action/setpoint/constraints/signature）
- 实现 TTL 过期检测：`ttl_check(cmd, now_ns) -> TtlStatus`
- 实现约束包校验（功率/SOC/电压/频率边界截断）：`constraint_check(cmd, state) -> ConstraintResult`
- 实现降级策略：`execute_or_fallback(cmd, now_ns) -> FallbackMode`
- 实现双平面联调：模拟 Agent 崩溃 → TTL 过期 → RTOS 自动降级

### 构建系统更新
- 根 `Cargo.toml` workspace `members` 增加 `crates/kernel/ipc` 和 `crates/kernel/controlbus`
- workspace 版本 `0.19.0` → `0.22.0`
- `Makefile` VERSION `0.19.0` → `0.22.0`
- `.github/workflows/ci.yml` 版本标识更新 + 新增 `eneros-ipc`/`eneros-controlbus` 交叉编译步骤
- `ci/src/gate.rs` 注释追加 v0.20.0~v0.22.0 说明
- `crates/kernel/sched/Cargo.toml` 版本保持 `0.19.0`（sched 本身不变，仅新增 `current_tid`）
- `crates/kernel/ipc/Cargo.toml` 版本 `0.22.0`，依赖 `eneros-sched`
- `crates/kernel/controlbus/Cargo.toml` 版本 `0.22.0`，依赖 `eneros-ipc`

## Impact

- **Affected specs**: Phase 0 P0-G（v0.20.0~v0.22.0）；Phase 0 出口标准验证报告
- **Affected code**:
  - 新增 `crates/kernel/ipc/`（endpoint/notification/channel/spsc_ring/shared_mem）
  - 新增 `crates/kernel/controlbus/`（command/ttl/constraint/fallback/integration）
  - 修改 `crates/kernel/sched/src/tcb.rs`（新增 current_tid/set_current_tid）
  - 修改 `crates/kernel/sched/src/lib.rs`（导出 current_tid/set_current_tid）
  - 修改根 `Cargo.toml`/`Makefile`/`ci.yml`/`gate.rs`
- **Affected docs**: 新增 `docs/kernel/ipc-design.md`、`docs/kernel/spsc-ring-design.md`、`docs/kernel/control-bus-design.md`、`docs/kernel/ttl-safety-mechanism.md`、`docs/kernel/phase0-exit-verification.md`

## ADDED Requirements

### Requirement: IPC 同步消息传递（v0.20.0）

系统 SHALL 提供基于 endpoint 的同步阻塞 IPC 机制，支持跨分区消息传递。

#### Scenario: 发送方先到达，接收方后到达
- **WHEN** 线程 A 调用 `send(ep, msg)` 且无接收方等待
- **THEN** A 被标记为 `waiting_sender` 并阻塞（状态转为 Blocked）
- **WHEN** 线程 B 随后调用 `recv(ep)`
- **THEN** 消息被拷贝给 B，A 被唤醒（状态转为 Ready）

#### Scenario: 接收方先到达，发送方后到达
- **WHEN** 线程 B 调用 `recv(ep)` 且无发送方等待
- **THEN** B 被标记为 `waiting_receiver` 并阻塞
- **WHEN** 线程 A 随后调用 `send(ep, msg)`
- **THEN** 消息被拷贝给 B，A 立即返回，B 被唤醒

#### Scenario: call 语义（send + recv）
- **WHEN** 客户端调用 `call(ep, req)`
- **THEN** 等价于先 `send(ep, req)` 再 `recv(ep)` 获取回复

#### Scenario: notification 位图唤醒
- **WHEN** 线程 A 调用 `notify(target_tid, bit=3)`
- **THEN** target 的 notification 位图 bit 3 被置位，target 被唤醒
- **WHEN** target 调用 `wait_notification()`
- **THEN** 返回当前位图值并清零

### Requirement: Lock-free SPSC Ring Buffer（v0.21.0）

系统 SHALL 提供基于共享内存的无锁单生产者单消费者环形缓冲区。

#### Scenario: 正常 push/pop
- **WHEN** 生产者调用 `ring.push(data)` 且环形未满
- **THEN** 数据被写入 tail 槽位，tail 推进（Release 序）
- **WHEN** 消费者调用 `ring.pop(out)` 且环形非空
- **THEN** 数据从 head 槽位读出，head 推进（Release 序）

#### Scenario: 环形满/空判定
- **WHEN** `next_tail == head`（环形满）
- **THEN** `push` 返回 `Err(RingError::Full)`
- **WHEN** `head == tail`（环形空）
- **THEN** `pop` 返回 `Err(RingError::Empty)`

#### Scenario: 共享内存授权
- **WHEN** 调用 `grant_shared_mem(owner, consumer, size)`
- **THEN** 返回 `SharedMemRegion` 描述物理区间与归属，仅 owner/consumer 分区可访问

### Requirement: ControlCommand 命令结构与 TTL（v0.22.0）

系统 SHALL 提供带 TTL 的控制命令结构，Agent 崩溃后 RTOS 自动降级到安全默认策略。

#### Scenario: TTL 过期检测
- **WHEN** `ttl_check(cmd, now_ns)` 且 `now_ns - cmd.timestamp >= cmd.ttl_ms * 1_000_000`
- **THEN** 返回 `TtlStatus::Expired`

#### Scenario: 约束截断
- **WHEN** `constraint_check(cmd, state)` 且 `cmd.setpoint > cmd.constraints.max_power`
- **THEN** 返回 `ConstraintResult::Truncated(max_power)`
- **WHEN** `state.soc` 超出 `cmd.constraints.soc_limit` 范围
- **THEN** 返回 `ConstraintResult::Rejected`

#### Scenario: Agent 崩溃降级
- **WHEN** `command_consume()` 返回 `None`（无新命令）且上次命令 TTL 已过期
- **THEN** `execute_or_fallback(None, now_ns)` 返回 `FallbackMode::SafeDefault`

#### Scenario: 双平面联调（出口验证）
- **WHEN** 模拟 Agent 崩溃（停止发送命令）
- **THEN** RTOS 在 TTL 过期后自动切换到 `SafeDefault`，控制不中断

## MODIFIED Requirements

### Requirement: sched 线程管理 API

v0.18.0 的 `thread_block`/`thread_resume` 已支持状态转换。v0.20.0 新增 `current_tid() -> Tid` 与 `set_current_tid(tid: Tid)`：

- `current_tid()` 返回当前线程的 Tid（host: 从可设置静态变量读取；aarch64: 预留 TPIDR_EL0 接口，Phase 0 返回静态变量）
- `set_current_tid(tid)` 设置当前线程 Tid（用于 host 测试与初始化）

### Requirement: Phase 0 出口标准验证

v0.22.0 承载 Phase 0 全部出口标准验证：
1. 双分区隔离（v0.9.0 + v0.21.0 共享内存隔离）— 已验证，v0.22.0 补 Control Bus 隔离
2. 实时性（分区抖动 < 1ms）— v0.19.0 host 验证，QEMU 实机延后
3. 多核（所有核启动 + RTOS 绑核）— v0.15.0~v0.17.0 已验证
4. 基础 OS 服务就绪 — v0.22.0 补 IPC + Control Bus 验证

## REMOVED Requirements

无。

## Design Decisions

| 编号 | 决策 | 理由 |
|------|------|------|
| D1 | 新 crate 放 `crates/kernel/ipc/` 与 `crates/kernel/controlbus/` | IPC/ControlBus 是内核级服务，归 kernel 子系统（§2.3.2） |
| D2 | 全局状态用 `Spinlock + UnsafeCell`（非 `static mut`） | 遵循 v0.18.0/v0.19.0 既有模式，避免 unsafe 静态可变 |
| D3 | `current_tid()`/`set_current_tid()` 加入 sched tcb.rs | IPC 需要知道当前线程；最小手术式修改，不破坏既有 API |
| D4 | host 测试仅验证状态机，不实际阻塞 | 单线程测试无法模拟阻塞；`thread_block` 在 host 仅转状态 |
| D5 | `SpscRing` 持外部 buffer 原始指针，手动 impl Send+Sync | Ring 不拥有内存，外部 buffer 生命周期由调用方管理 |
| D6 | `ttl_check`/`execute_or_fallback` 接受 `now_ns` 参数 | 调用方（RTOS）提供时间，controlbus 不依赖 eneros-time |
| D7 | v0.22.0 瓶颈版本：算法完整无 stub，性能 < 50μs 延后 QEMU | 蓝图 §4.4 要求瓶颈版本"骨架可用" |
| D8 | `Message` 大小 128 字节（label 8 + payload 120） | 蓝图 §4.5 指定，对齐缓存行 |
| D9 | 命令 encode/decode 用 `ptr::copy_nonoverlapping` | Phase 0 简化；未来 Phase 1 改正式序列化 |
| D10 | `shared_mem.rs` 实现 Struct + 简单 grant 函数 | 真实共享内存管理需 v0.8.0 vspace，Phase 0 用 stub 物理地址 |
| D11 | `integration.rs` 是模拟函数，驱动状态机 | 端到端真机验证延后 QEMU；host 验证降级时序逻辑 |
