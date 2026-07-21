# Checklist

## 目录结构校验

- [x] C1 新 crate 位置：`crates/protocols/iec104-master/` 在 `crates/protocols/` 下，未放根目录
- [x] C2 workspace members：根 Cargo.toml 的 members 已添加 `"crates/protocols/iec104-master"`
- [x] C3 跨 crate path 引用：`crates/protocols/iec104-master/Cargo.toml` 的 `eneros-iec104-slave = { path = "../iec104-slave" }` 路径正确
- [x] C4 文档分类：设计文档在 `docs/protocols/iec104-master-design.md`，未平面化放 docs/ 根
- [x] C5 无根目录 crate：仓库根目录下无新增 Rust crate 文件夹

## 构建校验

- [x] C6 cargo metadata 成功（workspace 成员路径全部正确）
- [x] C7 cargo test 通过（58 个测试全绿）
- [x] C8 cargo build --target aarch64-unknown-none 通过（no_std 交叉编译）
- [x] C9 cargo fmt --check 通过
- [x] C10 cargo clippy 无 warning
- [x] C11 cargo deny check 通过（已知 GitHub 网络问题：advisory-db 无法获取）

## 文档与规范校验

- [x] C12 文档位置：设计文档在 `docs/protocols/` 下
- [x] C13 无垃圾文件：git status 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C14 .gitignore 覆盖：无新产生的未忽略文件类型
- [x] C15 提交信息：遵循 Conventional Commits

## no_std 合规校验

- [x] N1 `#![cfg_attr(not(test), no_std)]` 在 lib.rs 顶部
- [x] N2 `extern crate alloc` 声明
- [x] N3 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] N4 无 `panic!` / `todo!` / `unimplemented!`（用 `assert!` 或返回 Result）
- [x] N5 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）

## 功能性校验

- [x] F1 Iec104Master.connect() 能通过 MasterTransport 建立连接并发送 STARTDT_ACT
- [x] F2 收到 STARTDT_CON 后连接状态转为 Connected
- [x] F3 interrogation() 能发送总召唤 ASDU（TypeId::InterrogationCommand, COT=Activation, QOI=20）
- [x] F4 clock_sync() 能发送时钟同步 ASDU（TypeId::ClockSyncCommand, 带 CP56Time2a 时标）
- [x] F5 send_single_command() 能发送单点遥控 ASDU（TypeId::SingleCommand）
- [x] F6 send_double_command() 能发送双点遥控 ASDU（TypeId::DoubleCommand）
- [x] F7 poll(now_ms) 能根据 poll_interval_ms 周期触发总召唤
- [x] F8 poll(now_ms) 能根据 clock_sync_interval_ms 周期触发时钟同步
- [x] F9 poll(now_ms) 能处理接收 I 帧（解析 ASDU 并更新统计）
- [x] F10 poll(now_ms) 能处理 t3 超时发送 TESTFR_ACT 保活
- [x] F11 多设备并发：BTreeMap 支持多个 MasterConnection 独立管理
- [x] F12 序列号 15 位回绕（send_seq/recv_seq & 0x7FFF）

## 测试覆盖校验

- [x] T1 主站连接从站 + STARTDT 握手测试
- [x] T2 总召唤命令发送 + 响应接收测试
- [x] T3 单点遥控命令发送测试
- [x] T4 双点遥控命令发送测试
- [x] T5 时钟同步命令发送 + TimeTag 构造测试
- [x] T6 多设备并发连接 + 轮询测试
- [x] T7 t3 超时保活 TESTFR 发送测试
- [x] T8 序列号递增与 15 位回绕测试
- [x] T9 poll 周期触发总召唤测试
- [x] T10 连接状态机转换测试（Idle→Connecting→StartDtPending→Connected→Interrogating）

## 验收标准校验

- [x] A1 主站能连接 IEC 104 从站并完成 STARTDT 流程
- [x] A2 总召唤获取全部遥测遥信数据（ASDU 解析正确）
- [x] A3 遥控命令下发并收到确认
- [x] A4 时钟同步命令生效（CP56Time2a 时标正确）
- [x] A5 支持多设备并发轮询

## 偏差声明校验

- [x] D1 MasterTransport trait 定义（connect/send/recv/close/now_ms）
- [x] D2 时间通过 now_ms: u64 参数注入
- [x] D3 超时/间隔使用 u32 毫秒
- [x] D4 不依赖 eneros-net/smoltcp
- [x] D5 复用 eneros-iec104-slave 类型
- [x] D6 crate 在 crates/protocols/iec104-master/
- [x] D7 不实现 DeviceDriver trait
- [x] D8 IP 地址用 [u8; 4] 表示
- [x] D9 ConnId = u32 类型别名
- [x] D10 PollScheduler 基于 now_ms 时间戳比较
- [x] D11 时钟同步时标由 now_ms 参数构造
