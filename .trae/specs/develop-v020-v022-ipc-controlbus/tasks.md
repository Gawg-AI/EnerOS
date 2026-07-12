# Tasks — v0.20.0 ~ v0.22.0 IPC 与 Control Bus

- [x] Task 1: 升级 sched crate 新增 `current_tid` / `set_current_tid`
  - [x] SubTask 1.1: 在 `crates/kernel/sched/src/tcb.rs` 新增 `current_tid() -> Tid` 与 `set_current_tid(tid: Tid)`，使用 `Spinlock + UnsafeCell<Tid>` 保护全局 `CURRENT_TID`（初值 `Tid(0)`）
  - [x] SubTask 1.2: 在 `crates/kernel/sched/src/lib.rs` 的 `pub use tcb::{...}` 中追加 `current_tid, set_current_tid`
  - [x] SubTask 1.3: 编写单元测试：set/get 往返、默认值 `Tid(0)`
  - [x] SubTask 1.4: 验证 `cargo test -p eneros-sched` 通过（v0.18.0/v0.19.0 回归 + 新增 2 测试）

- [x] Task 2: 创建 `eneros-ipc` crate 骨架与构建配置
  - [x] SubTask 2.1: 创建 `crates/kernel/ipc/Cargo.toml`（name=eneros-ipc, version=0.22.0, 依赖 eneros-sched path="../../sched"）
  - [x] SubTask 2.2: 创建 `crates/kernel/ipc/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明占位）
  - [x] SubTask 2.3: 根 `Cargo.toml` workspace members 增加 `"crates/kernel/ipc"`
  - [x] SubTask 2.4: workspace 版本 `0.19.0` → `0.22.0`
  - [x] SubTask 2.5: `Makefile` VERSION `0.19.0` → `0.22.0`
  - [x] SubTask 2.6: `.github/workflows/ci.yml` 版本标识更新 + 新增 `eneros-ipc` 交叉编译步骤
  - [x] SubTask 2.7: `ci/src/gate.rs` 注释追加 v0.20.0~v0.22.0 说明

- [x] Task 3: 实现 `crates/kernel/ipc/src/endpoint.rs`（v0.20.0 IPC 端点）
  - [x] SubTask 3.1: 定义 `EndpointId(pub u32)` 新类型，derive `Clone, Copy, Debug, PartialEq, Eq`
  - [x] SubTask 3.2: 定义 `Message { label: u64, payload: [u8; 120] }`（MSG_SIZE=128），derive `Clone, Copy`，impl `Default`
  - [x] SubTask 3.3: 定义 `Endpoint { id, waiting_sender: Option<Tid>, waiting_receiver: Option<Tid>, msg: Message }`
  - [x] SubTask 3.4: 定义 `IpcError { InvalidEndpoint, Timeout, Disconnected }`，derive `Debug`
  - [x] SubTask 3.5: 定义 `static ENDPOINTS_TABLE: Spinlock + UnsafeCell<[Option<Endpoint>; MAX_ENDPOINTS=256]>`，`static NEXT_EP_ID: Spinlock + UnsafeCell<u32>`（初值 1）
  - [x] SubTask 3.6: 实现 `endpoint_create() -> EndpointId`：扫描空槽，分配 ID，返回 `EndpointId(0)` 表示失败
  - [x] SubTask 3.7: 实现 `endpoint_destroy(ep: EndpointId)`：清空槽位
  - [x] SubTask 3.8: 实现 `send(ep, msg: &Message) -> Result<(), IpcError>`：有接收方→拷贝+resume；无→设 waiting_sender + block
  - [x] SubTask 3.9: 实现 `recv(ep) -> Result<Message, IpcError>`：有发送方→拷贝+resume；无→设 waiting_receiver + block
  - [x] SubTask 3.10: 编写单元测试：endpoint 增删、send/recv 会合逻辑（先 send 后 recv / 先 recv 后 send）、无效端点、ID 分配递增

- [x] Task 4: 实现 `crates/kernel/ipc/src/notification.rs`（v0.20.0 通知机制）
  - [x] SubTask 4.1: 定义 `Notification { bits: AtomicU64, waiter: Option<Tid> }`
  - [x] SubTask 4.2: 定义 `static NOTIFICATIONS: Spinlock + UnsafeCell<[Notification; MAX_THREADS]>`（借用 sched 的 `MAX_THREADS`，或定义本地 `MAX_NOTIFY=256`）
  - [x] SubTask 4.3: 实现 `notify(target: Tid, bit: u32)`：`bits.fetch_or(1<<bit, Release)` + `thread_resume(target)`
  - [x] SubTask 4.4: 实现 `wait_notification() -> u64`：`bits.swap(0, Acquire)`，为 0 则 block 当前线程（host 测试用 `set_current_tid` 设置后验证）
  - [x] SubTask 4.5: 编写单元测试：notify 设置位、wait_notification 读取并清零、多 bit 叠加

- [x] Task 5: 实现 `crates/kernel/ipc/src/channel.rs`（v0.20.0 call 封装）
  - [x] SubTask 5.1: 实现 `call(ep, req: &Message) -> Result<Message, IpcError>`：先 `send(ep, req)`，成功后 `recv(ep)` 取回复
  - [x] SubTask 5.2: 编写单元测试：call 等价 send+recv、call 无效端点返回 Err

- [x] Task 6: 实现 `crates/kernel/ipc/src/spsc_ring.rs`（v0.21.0 无锁环形缓冲区）
  - [x] SubTask 6.1: 定义 `SpscRing { buffer: *mut u8, capacity, slot_size, slot_count, head: AtomicUsize, tail: AtomicUsize }`，手动 impl Send+Sync
  - [x] SubTask 6.2: 定义 `RingError { Full, Empty, InvalidSize }`，derive `Debug`
  - [x] SubTask 6.3: 实现 `SpscRing::new(buf: &mut [u8], slot_size, slot_count) -> Self`
  - [x] SubTask 6.4: 实现 `push(&self, data: &[u8]) -> Result<(), RingError>`：满判定（next==head）→ Full；写槽位 + `tail.store(next, Release)`
  - [x] SubTask 6.5: 实现 `pop(&self, out: &mut [u8]) -> Result<usize, RingError>`：空判定（head==tail）→ Empty；读槽位 + `head.store(next, Release)`
  - [x] SubTask 6.6: 实现 `used(&self) -> usize` 与 `free(&self) -> usize`
  - [x] SubTask 6.7: 编写单元测试：push/pop 往返、满/空判定、环形回绕、used/free 计算、InvalidSize

- [x] Task 7: 实现 `crates/kernel/ipc/src/shared_mem.rs`（v0.21.0 共享内存授权）
  - [x] SubTask 7.1: 定义 `SharedMemRegion { phys: u64, size: usize, owner: u32, consumer: u32 }`，derive `Clone, Copy, Debug`
  - [x] SubTask 7.2: 实现 `grant_shared_mem(owner: u32, consumer: u32, size: usize) -> Option<SharedMemRegion>`：Phase 0 stub，返回硬编码物理地址 `0x8000_0000`
  - [x] SubTask 7.3: 编写单元测试：grant 返回区域字段正确、owner/consumer 标识

- [x] Task 8: 更新 `crates/kernel/ipc/src/lib.rs` 导出全部模块
  - [x] SubTask 8.1: 添加 `pub mod endpoint/notification/channel/spsc_ring/shared_mem`
  - [x] SubTask 8.2: 添加 `pub use` 导出：`EndpointId`/`Message`/`MSG_SIZE`/`Endpoint`/`IpcError`/`endpoint_create`/`endpoint_destroy`/`send`/`recv`/`call`/`notify`/`wait_notification`/`SpscRing`/`RingError`/`SharedMemRegion`/`grant_shared_mem`
  - [x] SubTask 8.3: 文档注释说明 v0.20.0/v0.21.0 交付内容

- [x] Task 9: 创建 `eneros-controlbus` crate 骨架与构建配置
  - [x] SubTask 9.1: 创建 `crates/kernel/controlbus/Cargo.toml`（name=eneros-controlbus, version=0.22.0, 依赖 eneros-ipc path="../ipc"）
  - [x] SubTask 9.2: 创建 `crates/kernel/controlbus/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + 模块声明占位）
  - [x] SubTask 9.3: 根 `Cargo.toml` workspace members 增加 `"crates/kernel/controlbus"`
  - [x] SubTask 9.4: `.github/workflows/ci.yml` 新增 `eneros-controlbus` 交叉编译步骤

- [x] Task 10: 实现 `crates/kernel/controlbus/src/command.rs`（v0.22.0 命令结构）
  - [x] SubTask 10.1: 定义 `ControlCommand { cmd_id: [u8;16], timestamp: u64, ttl_ms: u32, target_device: DeviceId, action: ControlAction, setpoint: f32, constraints: ConstraintPack, signature: [u8;64] }`，derive `Clone, Copy`
  - [x] SubTask 10.2: 定义 `ControlAction { Charge, Discharge, Idle, Emergency }`，derive `Clone, Copy, Debug`
  - [x] SubTask 10.3: 定义 `DeviceId(pub u32)`，derive `Clone, Copy, Debug`
  - [x] SubTask 10.4: 定义 `ConstraintPack { max_power, min_power, soc_limit, voltage_limit, frequency_limit }`，derive `Clone, Copy, Default`
  - [x] SubTask 10.5: 定义 `CbError { NotInitialized, RingFull, RingEmpty, InvalidCommand, SignatureFailed }`，derive `Debug`
  - [x] SubTask 10.6: 定义 `static CMD_RING: Spinlock + UnsafeCell<Option<SpscRing>>`，`static LAST_CMD: Spinlock + UnsafeCell<Option<ControlCommand>>`
  - [x] SubTask 10.7: 实现 `control_bus_init(ring: SpscRing)`：写入 CMD_RING
  - [x] SubTask 10.8: 实现 `command_send(cmd: &ControlCommand) -> Result<(), CbError>`：encode + push
  - [x] SubTask 10.9: 实现 `command_consume() -> Option<ControlCommand>`：pop + decode + 更新 LAST_CMD
  - [x] SubTask 10.10: 实现 `encode_command(cmd, buf) -> usize` 与 `decode_command(buf) -> ControlCommand`（`ptr::copy_nonoverlapping`）
  - [x] SubTask 10.11: 编写单元测试：command_send/consume 往返、未初始化返回 Err、RingFull 返回 Err、encode/decode 对称

- [x] Task 11: 实现 `crates/kernel/controlbus/src/ttl.rs`（v0.22.0 TTL 逻辑）
  - [x] SubTask 11.1: 定义 `TtlStatus { Valid, Expired }`，derive `Clone, Copy, Debug, PartialEq, Eq`
  - [x] SubTask 11.2: 实现 `ttl_check(cmd: &ControlCommand, now_ns: u64) -> TtlStatus`：`elapsed_ms = (now_ns - cmd.timestamp) / 1_000_000`，≥ ttl_ms → Expired
  - [x] SubTask 11.3: 编写单元测试：未过期 Valid、恰好过期 Expired、ttl_ms=0 立即过期、timestamp=0 边界

- [x] Task 12: 实现 `crates/kernel/controlbus/src/constraint.rs`（v0.22.0 约束包校验）
  - [x] SubTask 12.1: 定义 `DeviceState { soc, voltage, frequency, current_power }`，derive `Clone, Copy, Default`
  - [x] SubTask 12.2: 定义 `ConstraintResult { Ok, Truncated(f32), Rejected }`，derive `Clone, Copy, Debug`
  - [x] SubTask 12.3: 实现 `constraint_check(cmd: &ControlCommand, state: &DeviceState) -> ConstraintResult`：SOC 越限→Rejected；功率越限→Truncated；正常→Ok
  - [x] SubTask 12.4: 编写单元测试：正常 Ok、功率上限截断、功率下限截断、SOC 越限 Rejected、边界值

- [x] Task 13: 实现 `crates/kernel/controlbus/src/fallback.rs`（v0.22.0 降级策略）
  - [x] SubTask 13.1: 定义 `FallbackMode { Normal, WaitForCommand, SafeDefault, Emergency }`，derive `Clone, Copy, Debug, PartialEq, Eq`
  - [x] SubTask 13.2: 实现 `execute_or_fallback(cmd: Option<&ControlCommand>, now_ns: u64) -> FallbackMode`：有命令且未过期→Normal；有命令但过期→SafeDefault；无命令且上次未过期→WaitForCommand；无命令且上次过期→SafeDefault
  - [x] SubTask 13.3: 编写单元测试：Normal/SafeDefault/WaitForCommand 三路径、TTL 过期切换

- [x] Task 14: 实现 `crates/kernel/controlbus/src/integration.rs`（v0.22.0 双平面联调）
  - [x] SubTask 14.1: 定义 `IntegrationState { agent_alive: bool, last_cmd_time: u64, current_mode: FallbackMode }`
  - [x] SubTask 14.2: 实现 `integration_step(state: &mut IntegrationState, now_ns: u64) -> FallbackMode`：模拟 RTOS 周期消费 + Agent 崩溃 + TTL 过期降级
  - [x] SubTask 14.3: 实现 `simulate_agent_crash(state: &mut IntegrationState)`：设置 agent_alive=false
  - [x] SubTask 14.4: 编写单元测试：Agent 正常→Normal、Agent 崩溃后 TTL 过期→SafeDefault、Agent 恢复→Normal

- [x] Task 15: 更新 `crates/kernel/controlbus/src/lib.rs` 导出全部模块
  - [x] SubTask 15.1: 添加 `pub mod command/ttl/constraint/fallback/integration`
  - [x] SubTask 15.2: 添加 `pub use` 导出全部公开类型与函数
  - [x] SubTask 15.3: 文档注释说明 v0.22.0 交付内容 + Phase 0 出口验证

- [x] Task 16: 创建文档（docs/kernel/ 子目录）
  - [x] SubTask 16.1: 创建 `docs/kernel/ipc-design.md`（~350 行）：endpoint 会合模型、send/recv 阻塞语义、notification 位图、与 sched 调度器的交互
  - [x] SubTask 16.2: 创建 `docs/kernel/spsc-ring-design.md`（~300 行）：无锁算法原理、Acquire/Release 内存序、ARMv8 弱内存模型、与共享内存的关系
  - [x] SubTask 16.3: 创建 `docs/kernel/control-bus-design.md`（~400 行）：ControlCommand 结构、TTL 安全闭环、双平面架构、Agent 崩溃降级流程
  - [x] SubTask 16.4: 创建 `docs/kernel/ttl-safety-mechanism.md`（~250 行）：TTL 概念、单调时钟依赖、过期检测算法、与看门狗的关系
  - [x] SubTask 16.5: 创建 `docs/kernel/phase0-exit-verification.md`（~300 行）：Phase 0 四大出口标准逐项验证报告

- [x] Task 17: 验证与回归
  - [x] SubTask 17.1: `cargo fmt --all -- --check`
  - [x] SubTask 17.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
  - [x] SubTask 17.3: `cargo test -p eneros-sched`（current_tid 新增测试 + v0.18.0/v0.19.0 回归）
  - [x] SubTask 17.4: `cargo test -p eneros-ipc`（endpoint/notification/channel/spsc_ring/shared_mem 全部测试）
  - [x] SubTask 17.5: `cargo test -p eneros-controlbus`（command/ttl/constraint/fallback/integration 全部测试）
  - [x] SubTask 17.6: `cargo build -p eneros-ipc --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - [x] SubTask 17.7: `cargo build -p eneros-controlbus --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - [x] SubTask 17.8: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`（v0.19.0 不退化）
  - [x] SubTask 17.9: `cargo deny check advisories licenses bans sources`
  - [x] SubTask 17.10: `git status` 确认无垃圾文件（无 `target/`、`*.elf`、`*.bin`、`*.dtb`）

# Task Dependencies

- Task 1（sched current_tid）独立，可与 Task 2（ipc 骨架）并行
- Task 3/4/5（endpoint/notification/channel）依赖 Task 1（current_tid）+ Task 2（ipc 骨架）
- Task 6/7（spsc_ring/shared_mem）依赖 Task 2（ipc 骨架）
- Task 8（ipc lib.rs 导出）依赖 Task 3/4/5/6/7 完成
- Task 9（controlbus 骨架）依赖 Task 8（ipc 导出就绪）
- Task 10/11/12/13/14（command/ttl/constraint/fallback/integration）依赖 Task 9
- Task 15（controlbus lib.rs 导出）依赖 Task 10/11/12/13/14 完成
- Task 16（文档）可与 Task 3~14 并行（文档描述实现，但可提前起草）
- Task 17（验证）依赖所有前序任务完成

# Notes

- v0.22.0 是瓶颈版本（★），代码必须"骨架可用"——算法完整无 stub，性能 < 50μs 延后 QEMU
- v0.20.0/v0.21.0 非瓶颈版本，算法完整即可
- `eneros-ipc` 依赖 `eneros-sched`（用 Tid/thread_block/thread_resume/current_tid）
- `eneros-controlbus` 依赖 `eneros-ipc`（用 SpscRing）
- `eneros-controlbus` 不依赖 `eneros-time`——`ttl_check(cmd, now_ns)` 接受时间参数
- host 测试用 `set_current_tid` 设置当前线程，避免实际阻塞
- 测试预估：sched +2 + ipc ~35 + controlbus ~30 = ~67 个新测试，加 v0.19.0 原 99 个 = ~166 总测试
