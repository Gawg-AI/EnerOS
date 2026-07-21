# Tasks — v0.48.0 IEC 104 从站

## Task 1: workspace 版本号与 members 同步
- [x] SubTask 1.1: 修改根 `Cargo.toml`，`version` 从 `0.47.0` → `0.48.0`
- [x] SubTask 1.2: 向 `members` 数组增加 `"crates/protocols/iec104-slave"`

## Task 2: 创建 eneros-iec104-slave crate 骨架
- [x] SubTask 2.1: 创建 `crates/protocols/iec104-slave/Cargo.toml`（workspace 继承，零外部依赖，D8）
- [x] SubTask 2.2: 创建 `crates/protocols/iec104-slave/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + re-export）
- [x] SubTask 2.3: 创建 `crates/protocols/iec104-slave/src/error.rs`（`Iec104Error` 枚举：Encode/Decode/Transport/Sequence/InvalidFrame/Timeout/ConnectionClosed/PointNotFound/InvalidTypeId）

## Task 3: 实现 ASDU 应用层类型（asdu.rs）
- [x] SubTask 3.1: 定义 `TypeId` 枚举（10 变体：SinglePointInformation=1/DoublePointInformation=3/MeasuredValueNormalized=9/MeasuredValueScaled=11/MeasuredValueFloat=13/Counter=15/SingleCommand=45/DoubleCommand=46/InterrogationCommand=100/ClockSyncCommand=103）+ `from_u8`/`to_u8` 转换
- [x] SubTask 3.2: 定义 `Cot` 枚举（9 变体：Periodic=1/Background=2/Spontaneous=3/Initialized=4/Request=5/Activation=6/ActivationConfirm=7/Deactivation=8/InterrogatedByStation=20）+ `from_u8`/`to_u8`
- [x] SubTask 3.3: 定义 `QualityDescriptor`（invalid/not_topical/substituted/blocked/overflow）+ `good()`/`encode()`/`decode()`
- [x] SubTask 3.4: 定义 `SinglePointValue`（Off=0/On=1）和 `DoublePointValue`（Intermediate=0/Off=1/On=2/Bad=3）
- [x] SubTask 3.5: 定义 `Sco`（Single Command：value:bool/qu:u8/select:bool）和 `Dco`（Double Command：value:DoublePointValue/qu:u8/select:bool）
- [x] SubTask 3.6: 定义 `TimeTag`（CP56Time2a：year/month/day/hour/minute/second/iv/su/millis）+ `encode()`（7 字节）/`decode()`（D10）
- [x] SubTask 3.7: 定义 `IoValue` 枚举（Normalized(i16)/Scaled(i16)/Float(f32)/SinglePoint(SinglePointValue)/DoublePoint(DoublePointValue)/SingleCommand(Sco)/DoubleCommand(Dco)/Counter(u32)）
- [x] SubTask 3.8: 定义 `InformationObject`（ioa: u16/value: IoValue/quality: QualityDescriptor/time_tag: Option<TimeTag>）
- [x] SubTask 3.9: 定义 `Asdu`（type_id: TypeId/cause_of_tx: Cot/common_addr: u16/ioas: Vec<InformationObject>）+ `encode()`/`decode()`（SQ=0 非序列模式，每个对象携带自身 IOA）
- [x] SubTask 3.10: 编写 ASDU 单元测试（各 TypeId 编解码 + 浮点小端序 D6 + 边界值）

## Task 4: 实现 APDU 帧结构（apdu.rs）
- [x] SubTask 4.1: 定义 `UFormatFunction` 枚举（StartDtAct/StartDtCon/StopDtAct/StopDtCon/TestFrAct/TestFrCon）
- [x] SubTask 4.2: 定义 `ControlField` 枚举（Information{send_seq, recv_seq}/Numbered{recv_seq}/Unnumbered(UFormatFunction)）
- [x] SubTask 4.3: 定义 `Apdu`（control_field: ControlField/asdu: Option<Asdu>）
- [x] SubTask 4.4: 实现 `Apdu::encode()` -> Vec<u8>（起始字节 0x68 + 长度 + 控制域 4 字节 + ASDU；I 格式 bit0=0/S 格式 bit0=1 bit1=1/U 格式 bit0=1 bit1=0）
- [x] SubTask 4.5: 实现 `Apdu::decode(bytes: &[u8]) -> Result<Apdu, Iec104Error>`（校验起始字节 0x68 + 长度 + 控制域类型判断 + ASDU 可选解析）
- [x] SubTask 4.6: 实现便捷构造方法：`u_format(func)`/`s_format(recv_seq)`/`i_format(send_seq, recv_seq, asdu)`
- [x] SubTask 4.7: 编写 APDU 单元测试（I/S/U 三种格式编解码 + 序列号 15 位回绕 + 边界：空帧/超长帧/错误起始字节）

## Task 5: 实现 Iec104Config 配置（config.rs）
- [x] SubTask 5.1: 定义 `Iec104Config`（common_addr: u16/listen_port: u16/t1_timeout_ms: u32/t2_timeout_ms: u32/t3_timeout_ms: u32/k: u16/w: u16）
- [x] SubTask 5.2: 实现 `Default`（common_addr=1/listen_port=2404/t1=15000/t2=10000/t3=20000/k=12/w=8）
- [x] SubTask 5.3: 编写配置单元测试（Default 值 + 字段访问）

## Task 6: 实现 PointDatabase 点数据库（point.rs）
- [x] SubTask 6.1: 定义 `PointDatabase` trait（get_single_point/get_double_point/get_float/get_all_points/set_float/set_single_point/execute_single_command/execute_double_command）
- [x] SubTask 6.2: 实现 `InMemoryPointDatabase`（用 `BTreeMap<u16, IoValue>` 存储 + `BTreeMap<u16, QualityDescriptor>` 品质）
- [x] SubTask 6.3: 实现 `get_all_points_grouped()`（按 TypeId 分组返回，用于总召唤）
- [x] SubTask 6.4: 编写点数据库单元测试（增删改查 + 分组遍历 + 遥控执行）

## Task 7: 实现 SlaveTransport 传输层抽象（transport.rs）
- [x] SubTask 7.1: 定义 `ConnId` 类型别名（`u32`）
- [x] SubTask 7.2: 定义 `SlaveTransport` trait（accept()->Option<ConnId>/send(ConnId, &[u8])->Result<(), Iec104Error>/recv(ConnId, &mut [u8])->Result<usize, Iec104Error>/close(ConnId)/now_ms()->u64）
- [x] SubTask 7.3: 定义 `SlaveStats`（tx_count/rx_count/tx_error_count/rx_error_count/connections_accepted/connections_closed）+ Default
- [x] SubTask 7.4: 编写 SlaveStats 单元测试

## Task 8: 实现 Iec104Slave 从站（slave.rs）
- [x] SubTask 8.1: 定义 `SlaveState` 枚举（Idle/Connected/StartDtPending/Active/Stopped/Error）
- [x] SubTask 8.2: 定义 `SlaveConnection`（conn_id: ConnId/send_seq: u16/recv_seq: u16/last_rx_time_ms: u64/last_tx_time_ms: u64/pending_acks: u16/state: SlaveState）
- [x] SubTask 8.3: 定义 `Iec104Slave`（config: Iec104Config/point_db: Box<dyn PointDatabase>/transport: Box<dyn SlaveTransport>/connection: Option<SlaveConnection>/stats: SlaveStats/last_testfr_ms: u64）（D4 单连接）
- [x] SubTask 8.4: 实现 `Iec104Slave::new(config, point_db, transport)` 
- [x] SubTask 8.5: 实现 `poll(&mut self, now_ms: u64) -> Result<(), Iec104Error>`（D3 时间注入：accept 新连接/recv 数据/处理 APDU/检查 t3 超时发 TestFrAct）
- [x] SubTask 8.6: 实现 `handle_apdu(&mut self, apdu: &Apdu)`（分发 U/S/I 格式）
- [x] SubTask 8.7: 实现 `handle_u_format`（STARTDT_ACT→STARTDT_CON+state=Active/STOPDT/TESTFR）
- [x] SubTask 8.8: 实现 `handle_i_format`（解析 ASDU → handle_asdu + 更新 recv_seq + 检查 w 阈值发 S 帧）
- [x] SubTask 8.9: 实现 `handle_asdu`（分发 InterrogationCommand/ClockSyncCommand/SingleCommand/DoubleCommand）
- [x] SubTask 8.10: 实现 `handle_interrogation`（激活确认→数据分组发送→激活终止，三步流程）
- [x] SubTask 8.11: 实现 `handle_single_command`/`handle_double_command`（执行遥控→回复确认）
- [x] SubTask 8.12: 实现 `handle_clock_sync`（更新时间→回复确认）
- [x] SubTask 8.13: 实现 `send_apdu(&mut self, apdu: &Apdu)`（encode + transport.send + 更新 send_seq + stats）
- [x] SubTask 8.14: 编写从站单元测试（≥20 个：状态转换/U 格式处理/总召唤完整流程/遥控响应/时钟同步/序列号管理/w 阈值/t3 超时 TestFr/trait object 兼容）

## Task 9: 实现 MockSlaveTransport 测试桩（mock.rs）
- [x] SubTask 9.1: 定义 `MockSlaveTransport`（accepted_conns: VecDeque<ConnId>/rx_data: BTreeMap<ConnId, VecDeque<Vec<u8>>>/tx_frames: Vec<(ConnId, Vec<u8>)>/now_ms_value: u64/closed_conns: Vec<ConnId>）
- [x] SubTask 9.2: 为 `MockSlaveTransport` 实现 `SlaveTransport` trait
- [x] SubTask 9.3: 实现辅助方法（push_rx_data/accept_conn/tx_frames/tx_frames_for/advance_now_ms/clear）
- [x] SubTask 9.4: 编写 mock 单元测试

## Task 10: 集成测试（lib.rs tests mod）
- [x] SubTask 10.1: 编写 APDU 端到端测试（构造 I 帧 → encode → mock recv → decode → 校验）
- [x] SubTask 10.2: 编写总召唤完整流程测试（mock 主站发 InterrogationCommand → 从站 poll → 验证三步响应）
- [x] SubTask 10.3: 编写遥控命令测试（SingleCommand + DoubleCommand 执行确认）
- [x] SubTask 10.4: 编写时钟同步测试（ClockSyncCommand 确认）
- [x] SubTask 10.5: 编写 STARTDT 握手测试（U 格式握手 → state=Active）
- [x] SubTask 10.6: 编写序列号管理测试（多帧收发 + 序列号递增 + 15 位回绕）

## Task 11: 设计文档
- [x] SubTask 11.1: 创建 `docs/protocols/iec104-slave-design.md`，包含：版本目标、前置依赖、交付物清单、详细设计（含偏差声明 D1~D10）、APDU/ASDU 结构、总召唤/遥控/时钟同步流程图、测试计划、验收标准、风险、多角度要求

## Task 12: 构建校验（§2.4 清单）
- [x] SubTask 12.1: `cargo metadata --format-version 1 > NUL`（workspace 成员路径正确）
- [x] SubTask 12.2: `cargo test -p eneros-iec104-slave`（单元 + 集成测试通过，116 测试全绿）
- [x] SubTask 12.3: `cargo build -p eneros-iec104-slave --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（交叉编译通过）
- [x] SubTask 12.4: `cargo fmt --all -- --check`（格式检查）
- [x] SubTask 12.5: `cargo clippy -p eneros-iec104-slave --all-targets -- -D warnings`（lint 无 warning）

# Task Dependencies

- Task 1（workspace 同步）→ 无前置，可独立执行
- Task 2（crate 骨架 + error）→ 依赖 Task 1
- Task 3（ASDU）→ 依赖 Task 2（error.rs）
- Task 4（APDU）→ 依赖 Task 3（Asdu）
- Task 5（Config）→ 依赖 Task 2（可并行于 Task 3/4）
- Task 6（PointDatabase）→ 依赖 Task 3（IoValue/InformationObject）
- Task 7（Transport）→ 依赖 Task 2（error.rs）
- Task 8（Iec104Slave）→ 依赖 Task 2~7 全部
- Task 9（Mock）→ 依赖 Task 7（SlaveTransport trait）
- Task 10（集成测试）→ 依赖 Task 8（Iec104Slave）+ Task 9（mock）
- Task 11（设计文档）→ 可与 Task 3~9 并行
- Task 12（构建校验）→ 依赖 Task 1~10 全部完成

# 可并行执行

- Task 5（Config）+ Task 7（Transport）可与 Task 3（ASDU）并行
- Task 6（PointDatabase）在 Task 3 完成后可与 Task 4（APDU）并行
- Task 11（设计文档）可与 Task 3~9 并行
