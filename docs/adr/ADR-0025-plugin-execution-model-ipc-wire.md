# ADR-0025：协议/驱动插件执行模型 = 签名 ELF 进程 + 版本化 IPC wire，删除 cross-binary trait-object 导出

> 状态：Accepted（2026-08-04）
> 决策者：产品负责人 + 架构收敛实施（07 蓝图 ARCH-D01 / ARCH-S01）
> 前置：MASTER §G6 AGT-003（禁止跨二进制 Rust ABI / 动态插件加载）；07 蓝图 §1.1
> 审计证据：`crates/drivers/driver-sdk/src/lib.rs:321/337` 与 `crates/protocols/protocol-sdk/src/lib.rs:303/319`
> 的 `register_driver!`/`register_protocol!` 宏仍导出 `extern "C" fn -> *mut dyn`；两者零消费者。

## 1. 背景

MASTER §10.1 已冻结 Agent 执行模型为「一个 Agent package → 签名静态 ELF PIE → 受限
EL0 process → 版本化 IPC」；`agent-sdk` 已按 AGT-003 删除 `register_agent!`（
`extern "C" -> *mut dyn Agent`）宏（`crates/agents/agent-sdk/src/lib.rs:90` 注释为证）。

但同一契约下，driver-sdk 与 protocol-sdk 仍在导出 Rust trait object 的裸指针跨 C ABI
边界：`extern "C" fn eneros_driver_create() -> *mut dyn DriverPlugin` 等 4 处
（evidence/arch-convergence/baseline-before.md 第 1-11 行）。Rust trait object vtable
布局不是稳定 ABI，跨二进制传递属未定义行为；若随 SDK 发布，修复即 breaking change。
07 蓝图 ARCH-D01 将此项列为当前分支合入前的**阻断项**。

## 2. 决策

1. **v1 插件执行模型唯一化**：协议与驱动插件与 Agent 相同——签名静态 ELF PIE →
   受限 EL0 进程 → 版本化 IPC wire ABI。不承诺第二条执行路径。
2. **删除 cross-binary trait-object 导出**：`register_driver!`/`register_protocol!`
   宏及其 `extern "C" -> *mut dyn` 导出函数整体删除；`#[no_mangle]` 导出清零。
3. **SDK 形态（ARCH-S01 方向，用户已确认）**：driver-sdk / protocol-sdk 改造为
   **IPC wire codec 客户端**——定义与 Agent 一致的二进制 wire 消息格式（固定头 +
   有界长度 + CRC32，对齐 `agent-sdk/src/wire.rs` 的 EIPC 规范），插件/宿主两侧经
   wire codec 交换请求与响应；不再以 trait object 跨进程传递。
4. **trait 定位**：`DriverPlugin`/`ProtocolPlugin` 仅作为宿主进程内（编译期组合）
   的服务端实现接口保留；跨进程一律走 wire。SDK 内任何 `Box<dyn Plugin>` 仅允许
   存在于 `#[cfg(test)]`。
5. **release profile 约束**：release 闭包内禁止出现 `*mut dyn` 跨二进制导出；
   由出口扫描（`rg "extern \"C\" fn.*\*mut dyn"`）自动验证。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 保留 `*mut dyn` 导出，仅加文档警告 | 未定义行为不因文档免责；AGT-003 对三 SDK 必须一致生效 |
| 用 `#[repr(C)]` struct + vtable 指针替代 trait object | 仍是自定义 ABI，需冻结 vtable 布局且无消费者验证；wire codec 是更标准、可 fuzz、可版本化的路径 |
| 静态链接源码级框架（编译期组合） | 07 蓝图官方选项之一；用户决策选 IPC wire codec 客户端，为 ELF 进程插件化直接铺路，与 agent-sdk wire 一致 |
| 延迟到发布后处理 | 两 SDK 零消费者、未发布，现在改成本最低；发布后即 breaking change（07 §0） |

## 4. 影响面

- `crates/drivers/driver-sdk/`：删除注册宏导出；`DriverPlugin` trait 保留为宿主内
  接口；引入 wire codec 模块（若 ARCH-S04 抽共享 crate 则复用）。
- `crates/protocols/protocol-sdk/`：同上；同时点类型并入 upa-model（ARCH-S02）。
- `agent-sdk`：不受影响（已合规），其 wire.rs 成为 wire 格式的对齐基准。
- 测试：现有 `#[cfg(test)]` 内 `Box<dyn ...>` 使用不违反决策（非跨二进制）；
  新增 wire round-trip 属性测试。
- 出口：07 §5 第一条「release profile 闭包内不存在 `*mut dyn` 跨二进制导出」。

## 5. 禁止捷径

- 给 `*mut dyn` 导出包一层"暂时安全"封装继续发布（07 §6 明令禁止）。
- 用版本号递增标记完成。
- 把 wire codec 写成无真实编解码的空壳；必须通过 round-trip 与畸形输入测试。

## 6. 追踪

- 蓝图：07 §1.1、§2.1、§2.2；MASTER §10.1、AGT-003。
- 实施：ARCH-S01（删除导出 + wire codec 客户端）、ARCH-S04（共享 crate）。
