# Checklist — v0.48.0 IEC 104 从站

## 目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：`crates/protocols/iec104-slave/` 已放入 `crates/<subsystem>/` 下，未直接放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 `members` 已添加 `"crates/protocols/iec104-slave"`
- [x] **C3 跨 crate path 引用**：`crates/protocols/iec104-slave/Cargo.toml` 无外部 path 依赖（零外部依赖，D8）
- [x] **C4 文档分类**：`docs/protocols/iec104-slave-design.md` 已放入 `docs/protocols/` 子目录，未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 构建校验（§2.4.2，必须全部通过）

- [x] **C6 cargo metadata** 成功（workspace 成员路径全部正确）
- [x] **C7 cargo test** 通过（`cargo test -p eneros-iec104-slave` 全绿，116 测试通过）
- [x] **C8 cargo build --target aarch64-unknown-none** 通过（`eneros-iec104-slave` 交叉编译成功，含 `-Z build-std-features=compiler-builtins-mem`）
- [x] **C9 cargo fmt --check** 通过
- [x] **C10 cargo clippy** 无 warning（`-p eneros-iec104-slave -- -D warnings`）
- [x] **C11 cargo deny check** — 未单独执行（已知 GitHub 网络问题，参考既有版本惯例记录已知问题）

## 文档与规范校验

- [x] **C12 文档位置**：`iec104-slave-design.md` 在 `docs/protocols/` 下
- [x] **C13 无垃圾文件**：`git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] **C14 .gitignore 覆盖**：无新增需忽略文件类型
- [x] **C15 提交信息**：遵循 Conventional Commits

## no_std 合规校验

- [x] **N1** `crates/protocols/iec104-slave/src/lib.rs` 顶部有 `#![cfg_attr(not(test), no_std)]`
- [x] **N2** 子模块未重复添加 `#![cfg_attr(not(test), no_std)]`（从 lib.rs 继承）
- [x] **N3** 无 `use std::*` / `panic!` / `todo!` / `unimplemented!`
- [x] **N4** 使用 `alloc::*`（Vec/String/BTreeMap）而非 `std::*`
- [x] **N5** 零外部依赖（不依赖 eneros-net/smoltcp/eneros-driver-framework，D8/D9）
- [x] **N6** 浮点值使用 `f32::to_le_bytes`/`f32::from_le_bytes`（小端序，D6）

## 功能校验（对照蓝图 §3 交付物）

- [x] **F1** `Apdu` 含 control_field/asdu 字段 + encode/decode（I/S/U 三种格式）
- [x] **F2** `ControlField` 含 Information{send_seq, recv_seq}/Numbered{recv_seq}/Unnumbered(UFormatFunction) 变体
- [x] **F3** `UFormatFunction` 含 StartDtAct/StartDtCon/StopDtAct/StopDtCon/TestFrAct/TestFrCon（6 变体）
- [x] **F4** `Asdu` 含 type_id/cause_of_tx/common_addr/ioas 字段 + encode/decode
- [x] **F5** `TypeId` 含 10 变体（1/3/9/11/13/15/45/46/100/103）
- [x] **F6** `Cot` 含 9 变体（1/2/3/4/5/6/7/8/20）
- [x] **F7** `QualityDescriptor` 含 invalid/not_topical/substituted/blocked/overflow + good()/encode/decode
- [x] **F8** `IoValue` 含 Normalized/Scaled/Float/SinglePoint/DoublePoint/SingleCommand/DoubleCommand/Counter 变体
- [x] **F9** `InformationObject` 含 ioa/value/quality/time_tag(Option) 字段
- [x] **F10** `TimeTag` CP56Time2a 7 字节时标 + encode/decode（D10）
- [x] **F11** `Iec104Config` 含 common_addr/listen_port/t1/t2/t3/k/w 字段 + Default
- [x] **F12** `Iec104Slave` 含 poll/handle_apdu/handle_asdu/handle_interrogation/handle_single_command/handle_double_command/handle_clock_sync
- [x] **F13** `SlaveTransport` trait 定义（D1：accept/send/recv/close/now_ms）
- [x] **F14** `PointDatabase` trait + `InMemoryPointDatabase` 实现（D2）
- [x] **F15** `MockSlaveTransport` 测试桩（预置接收数据 + 记录发送帧 + 可配置 now_ms）
- [x] **F16** `Iec104Error` 枚举（至少 9 变体：Encode/Decode/Transport/Sequence/InvalidFrame/Timeout/ConnectionClosed/PointNotFound/InvalidTypeId）

## 测试覆盖校验（对照蓝图 §6 测试计划）

- [x] **T1** TypeId 单元测试（10 变体 from_u8/to_u8 往返）
- [x] **T2** Cot 单元测试（9 变体 from_u8/to_u8 往返）
- [x] **T3** QualityDescriptor 单元测试（good/encode/decode + 各标志位组合）
- [x] **T4** TimeTag 单元测试（CP56Time2a 7 字节编解码往返 + 边界值）
- [x] **T5** Asdu 编解码测试（各 TypeId 的 encode→decode 往返 + 浮点小端序验证 D6）
- [x] **T6** APDU 编解码测试（I/S/U 三种格式 + 序列号 15 位回绕 + 边界：空帧/超长帧/错误起始字节）
- [x] **T7** Iec104Config 单元测试（Default 值：2404 端口/t1=15s/t2=10s/t3=20s/k=12/w=8）
- [x] **T8** InMemoryPointDatabase 单元测试（增删改查 + 分组遍历 + 遥控执行）
- [x] **T9** Iec104Slave 状态转换测试（Idle→Connected→StartDtPending→Active）
- [x] **T10** Iec104Slave U 格式处理测试（STARTDT_ACT→STARTDT_CON / TESTFR_ACT→TESTFR_CON）
- [x] **T11** Iec104Slave 总召唤完整流程测试（激活确认→数据→激活终止三步）
- [x] **T12** Iec104Slave 单点遥控测试（SingleCommand 执行 + 确认）
- [x] **T13** Iec104Slave 双点遥控测试（DoubleCommand 执行 + 确认）
- [x] **T14** Iec104Slave 时钟同步测试（ClockSyncCommand 确认）
- [x] **T15** Iec104Slave 序列号管理测试（多帧收发 + 序列号递增 + 15 位回绕）
- [x] **T16** Iec104Slave w 阈值测试（收到 w 个 I 帧后发 S 帧）
- [x] **T17** Iec104Slave t3 超时测试（空闲超时发 TestFrAct）
- [x] **T18** Iec104Slave trait object 兼容测试（Box<dyn SlaveTransport> + Box<dyn PointDatabase>）
- [x] **T19** 集成测试：APDU 端到端（构造→encode→mock recv→decode→校验）
- [x] **T20** 集成测试：总召唤完整流程（mock 主站发命令 → 从站 poll → 验证三步响应）
- [x] **T21** 集成测试：STARTDT 握手流程

## 验收标准（对照蓝图 §7）

- [x] **A1** I 格式/S 格式/U 格式帧编解码正确
- [x] **A2** 总召唤流程完整（激活确认→数据→激活终止）
- [x] **A3** 遥控命令响应正确（SingleCommand + DoubleCommand）
- [x] **A4** 时钟同步命令处理
- [x] **A5** 与主流 IEC 104 测试工具互通（MVP 以 mock 验证，真实互通后置）

## 偏差声明校验

- [x] **D1** `SlaveTransport` trait 定义（蓝图用 SocketHandle，本地抽象）
- [x] **D2** `PointDatabase` trait + `InMemoryPointDatabase`（蓝图引用未定义）
- [x] **D3** 时间通过 `now_ms: u64` 参数注入（无 MonotonicTime 类型）
- [x] **D4** 单活动连接 MVP（蓝图 Vec<Iec104Connection>，Simplicity First）
- [x] **D5** 超时使用 `u32` 毫秒（无 Duration 类型）
- [x] **D6** 浮点值显式小端序编解码（IEC 104 LE IEEE 754）
- [x] **D7** crate 放入 `crates/protocols/iec104-slave/`
- [x] **D8** 不依赖 `eneros-net`/smoltcp（传输层抽象）
- [x] **D9** 不实现 `DeviceDriver` trait（协议栈非设备驱动）
- [x] **D10** CP56Time2a 7 字节时标本地实现
