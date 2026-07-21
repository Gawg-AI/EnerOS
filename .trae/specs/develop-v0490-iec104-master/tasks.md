# Tasks

- [x] Task 1: 同步 workspace 版本号与 members 列表
  - [x] 修改 `e:\eneros\Cargo.toml`：`version = "0.48.0"` → `version = "0.49.0"`
  - [x] 在 members 数组中 `"crates/protocols/iec104-slave"` 之后增加 `"crates/protocols/iec104-master"`
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 crate 骨架（Cargo.toml + lib.rs）
  - [x] 创建 `e:\eneros\crates\protocols\iec104-master\Cargo.toml`
    - package name = `eneros-iec104-master`，workspace 继承
    - dependencies: `eneros-iec104-slave = { path = "../iec104-slave" }`
  - [x] 创建 `e:\eneros\crates\protocols\iec104-master\src\lib.rs`
    - `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
    - 模块声明：error / config / transport / device / poll / connection / master / mock
    - D1~D11 偏差声明表（doc comment）
    - 重导出公共 API
  - 验证：`cargo build -p eneros-iec104-master` 编译通过

- [x] Task 3: 实现 error 模块
  - [x] 创建 `src/error.rs`
    - 重导出 `eneros_iec104_slave::Iec104Error`（D5 复用）
    - 定义 `MasterError` 枚举（NotConnected / ConnectFailed / SendFailed / RecvFailed / StateError / Timeout）
  - 验证：`cargo build -p eneros-iec104-master` 编译通过

- [x] Task 4: 实现 config 模块
  - [x] 创建 `src/config.rs`
    - `MasterConfig` 结构体（clock_sync_interval_ms: u32, t3_timeout_ms: u32, poll_interval_ms: u32, default_port: u16=2404）
    - `Default` 实现：clock_sync=600000(10min), t3=20000, poll=30000
  - 验证：编译通过

- [x] Task 5: 实现 device 模块（RemoteDevice + ConnState）
  - [x] 创建 `src/device.rs`
    - `RemoteDevice` 结构体（ip: [u8;4], port: u16, common_addr: u16, poll_interval_ms: u32）
    - `RemoteDevice::new(ip, port, common_addr, poll_interval_ms)` 构造函数
    - `ConnState` 枚举（Idle / Connecting / StartDtPending / Connected / Interrogating / Error）
    - 派生 Debug/Clone/Copy/PartialEq/Eq
  - 验证：编译通过

- [x] Task 6: 实现 transport 模块（MasterTransport trait + MasterStats）
  - [x] 创建 `src/transport.rs`
    - `ConnId = u32` 类型别名（D9）
    - `MasterTransport` trait（D1）：
      - `connect(&mut self, ip: [u8;4], port: u16) -> Result<ConnId, MasterError>`
      - `send(&mut self, conn: ConnId, data: &[u8]) -> Result<(), MasterError>`
      - `recv(&mut self, conn: ConnId) -> Result<Option<Vec<u8>>, MasterError>`
      - `close(&mut self, conn: ConnId) -> Result<(), MasterError>`
      - `now_ms(&self) -> u64`
    - `MasterStats` 结构体（tx_count / rx_count / tx_error_count / rx_error_count / connect_count / disconnect_count / interrogation_count / command_count / clock_sync_count）
  - 验证：编译通过

- [x] Task 7: 实现 poll 模块（PollTask + PollScheduler）
  - [x] 创建 `src/poll.rs`
    - `PollTask` 结构体（common_addr: u16, next_poll_ms: u64, interval_ms: u32）
    - `PollScheduler` 结构体（tasks: BTreeMap<u16, PollTask>）
    - `PollScheduler::new()` / `add_task(common_addr, interval_ms)` / `remove_task(common_addr)`
    - `PollScheduler::due_tasks(now_ms) -> Vec<u16>` 返回到期任务
    - `PollScheduler::update_next(common_addr, now_ms)` 更新下次执行时间
  - 验证：编译通过 + 单元测试

- [x] Task 8: 实现 connection 模块（MasterConnection）
  - [x] 创建 `src/connection.rs`
    - `MasterConnection` 结构体：
      - remote: RemoteDevice
      - conn_id: ConnId
      - send_seq: u16
      - recv_seq: u16
      - last_interrogation_ms: u64
      - last_clock_sync_ms: u64
      - last_activity_ms: u64
      - state: ConnState
      - pending_acks: u16
    - 方法：`new(remote, conn_id, now_ms)` / `next_send_seq()` / `next_recv_seq()` / `touch(now_ms)`
  - 验证：编译通过

- [x] Task 9: 实现 master 模块（Iec104Master 核心）
  - [x] 创建 `src/master.rs`
    - `Iec104Master` 结构体：
      - devices: BTreeMap<u16, MasterConnection>
      - scheduler: PollScheduler
      - config: MasterConfig
      - stats: MasterStats
      - transport: Box<dyn MasterTransport>
    - 方法实现：
      - `new(config, transport) -> Self`
      - `connect(&mut self, device: &RemoteDevice) -> Result<(), MasterError>` — 传输层连接 + 发送 STARTDT_ACT
      - `interrogation(&mut self, common_addr: u16, now_ms: u64) -> Result<(), MasterError>` — 发送总召唤 ASDU
      - `clock_sync(&mut self, common_addr: u16, now_ms: u64) -> Result<(), MasterError>` — 发送时钟同步 ASDU
      - `send_single_command(&mut self, common_addr: u16, ioa: u16, value: SinglePointValue) -> Result<(), MasterError>`
      - `send_double_command(&mut self, common_addr: u16, ioa: u16, value: DoublePointValue) -> Result<(), MasterError>`
      - `poll(&mut self, now_ms: u64)` — 周期总召唤 + 时钟同步 + 接收处理 + t3 保活
      - `process_rx(&mut self, conn) -> Result<(), MasterError>` — 处理接收 I/S/U 帧（每次处理一帧，非阻塞）
      - `handle_u_format(&mut self, conn, func)` — STARTDT_CON/STOPDT_CON/TESTFR_CON 处理
      - `handle_i_format(&mut self, conn, asdu)` — ASDU 接收处理
      - `send_i_format(&mut self, conn, asdu) -> Result<(), MasterError>` — I 帧发送
      - `send_testfr(&mut self, conn) -> Result<(), MasterError>` — TESTFR_ACT 发送
      - `stats(&self) -> &MasterStats`
      - `device_state(&self, common_addr: u16) -> Option<ConnState>`
    - STARTDT 流程：connect 后 state=StartDtPending，收到 STARTDT_CON 转 Connected
    - 序列号管理：send_seq/recv_seq 15 位回绕（& 0x7FFF）
  - 验证：编译通过

- [x] Task 10: 实现 mock 模块
  - [x] 创建 `src/mock.rs`（`#[cfg(test)]`）
    - `MockMasterTransport` 实现 `MasterTransport` trait
    - 内部状态：
      - next_conn_id: ConnId（自增）
      - connections: BTreeMap<ConnId, ([u8;4], u16)>
      - rx_data: BTreeMap<ConnId, VecDeque<Vec<u8>>>（预置接收数据）
      - tx_frames: Vec<(ConnId, Vec<u8>)>（已发送帧记录）
      - current_time_ms: u64
    - 方法：
      - `new() -> Self`
      - `push_rx(conn, data)` 预置接收数据
      - `tx_frames() -> &[(ConnId, Vec<u8>)]` 获取已发送帧
      - `advance_time(ms)` 推进时间
      - 实现 trait 所有方法
  - 验证：编译通过

- [x] Task 11: 集成测试
  - [x] 在 `src/lib.rs` 的 `#[cfg(test)] mod tests` 中编写跨模块集成测试：
    - 测试 1：主站连接从站 + STARTDT 握手（mock 推 STARTDT_CON）
    - 测试 2：总召唤命令发送 + 接收响应数据
    - 测试 3：单点遥控命令发送
    - 测试 4：双点遥控命令发送
    - 测试 5：时钟同步命令发送（验证 TimeTag 构造）
    - 测试 6：多设备并发连接 + 轮询
    - 测试 7：t3 超时保活（TESTFR 发送）
    - 测试 8：序列号递增与 15 位回绕
    - 测试 9：poll 周期触发总召唤
    - 测试 10：连接状态机转换
  - 验证：`cargo test -p eneros-iec104-master` 全部通过（58 tests passed）

- [x] Task 12: 编写设计文档
  - [x] 创建 `e:\eneros\docs\protocols\iec104-master-design.md`
    - 章节：1.概述 / 2.架构 / 3.核心类型 / 4.主站状态机 / 5.通信流程 / 6.多设备管理 / 7.轮询调度 / 8.错误处理 / 9.no_std合规 / 10.测试策略 / 11.与v0.48.0的关系 / 12.偏差声明
    - 包含 Mermaid 状态机图 + 通信时序图（909 行）
  - 验证：文档位置在 `docs/protocols/` 下（C4 校验）

- [x] Task 13: 更新 Makefile / ci.yml / gate.rs 版本号
  - [x] `e:\eneros\Makefile`：0.43.0 → 0.49.0
  - [x] `e:\eneros\.github\workflows\ci.yml`：0.43.0 → 0.49.0
  - [x] `e:\eneros\ci\src\gate.rs`：补充 v0.44.0~v0.49.0 新增 crate 注释
  - 验证：版本号已同步

- [x] Task 14: 构建校验（C6~C11）
  - [x] `cargo metadata --format-version 1` — workspace 解析成功
  - [x] `cargo test -p eneros-iec104-master` — 58 测试全部通过
  - [x] `cargo build -p eneros-iec104-master --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` — 交叉编译通过
  - [x] `cargo fmt -p eneros-iec104-master -- --check` — 格式检查通过
  - [x] `cargo clippy -p eneros-iec104-master --all-targets -- -D warnings` — lint 通过
  - [x] `cargo deny check advisories licenses bans sources` — 已知 GitHub 网络问题（advisory-db 无法获取）

# Task Dependencies
- Task 1 独立（workspace 准备）
- Task 2 依赖 Task 1（crate 骨架）
- Task 3~8 依赖 Task 2，相互独立可并行
- Task 9 依赖 Task 3~8（master 核心依赖所有类型）
- Task 10 依赖 Task 6（mock 实现 transport trait）
- Task 11 依赖 Task 9 + Task 10（测试依赖 master + mock）
- Task 12 依赖 Task 9（文档依赖实现完成）
- Task 13 独立（版本号同步）
- Task 14 依赖全部完成
