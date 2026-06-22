# ADR-0003：v0.28.0 采用 plugin-daemon 进程隔离

- **状态**：Accepted
- **日期**：2026-06-21
- **决策者**：EnerOS 核心团队
- **相关 ADR**：[ADR-0001](./0001-record-architecture-decisions.md)、[ADR-0002](./0002-power-native-agentos.md)

## 上下文

EnerOS v0.27.0 引入了完整的插件框架（eneros-plugin crate），支持第三方协议适配器、Agent 策略、分析模块以动态库形式接入系统。v0.27.0 的插件加载方式为**同进程加载**：插件动态库（.so/.dll/.dylib）通过 libloading 加载到主进程地址空间，直接调用 C ABI 入口函数。

同进程加载存在以下风险：

1. **崩溃传播**：插件代码的 panic、段错误、内存越界会直接导致主进程崩溃。在电力系统中，主进程崩溃意味着 Agent 编排与安全网关中断，可能影响电网安全。

2. **资源隔离不足**：插件可以访问主进程的全部内存空间，恶意或有缺陷的插件可能破坏主进程数据。seccomp 沙箱限制了 syscall，但无法防止内存层面的越界访问。

3. **安全边界模糊**：电力系统对安全性有刚性要求。同进程加载使得插件与内核之间的信任边界不清晰，难以满足电力二次系统安全防护规范。

4. **热加载风险**：卸载动态库时若插件仍持有主进程资源（线程、句柄、回调），可能导致 use-after-free。

考虑到 EnerOS 定位为电力原生 AgentOS（ADR-0002），安全约束为内核法律，插件系统必须提供更强的隔离保障。

## 决策

**v0.28.0 引入 plugin-daemon 独立进程加载插件，通过 IPC 通道与主进程通信，同时保留 Inline（同进程）模式向后兼容。**

具体方案：

1. **plugin-daemon 独立进程**：新增 `crates/eneros-plugin/bins/plugin-daemon` 二进制，作为插件宿主进程。插件动态库加载到 plugin-daemon 的地址空间，而非主进程。

2. **IPC 通信**：plugin-daemon 与主进程通过 IPC 通道（JSON 行协议 over Unix socket / TCP）通信。主进程通过 `PluginDaemonClient`（eneros-plugin/src/ipc.rs）发送 `DaemonRequest`，接收 `DaemonResponse`。

3. **崩溃隔离**：plugin-daemon 崩溃不影响主进程。主进程检测到 plugin-daemon 退出后，可自动重启并重新加载插件。

4. **资源配额**：plugin-daemon 受 cgroups 资源配额约束（CPU/内存），通过 seccomp 限制 syscall，与 v0.27.0 沙箱配置一致。

5. **双模式支持**：
   - **Daemon 模式**（默认，v0.28.0）：插件在 plugin-daemon 进程中加载，通过 IPC 通信。适用于生产环境。
   - **Inline 模式**（v0.27.0 行为）：插件在同进程加载，直接函数调用。适用于开发/测试环境，或对延迟敏感的场景。

6. **模式选择**：通过 `plugin.toml` 中的 `default_mode` 配置项选择，单个插件也可在 manifest 中指定 `mode = "daemon"` 或 `mode = "inline"`。

## 后果

### 正面

- **崩溃隔离**：插件崩溃不影响主进程，保障 Agent 编排与安全网关的连续运行，满足电力系统可靠性要求
- **安全边界清晰**：插件与内核之间通过 IPC 通信，信任边界明确，便于满足电力二次系统安全防护规范
- **资源可控**：plugin-daemon 受 cgroups 约束，插件资源消耗不影响主进程
- **向后兼容**：Inline 模式保留，开发/测试环境可继续使用低延迟的同进程加载

### 负面

- **性能开销**：IPC 通信引入序列化与跨进程开销，相比同进程直接函数调用有延迟增加。对于高频调用的协议插件，需评估延迟影响。
- **复杂度增加**：需要维护 plugin-daemon 进程的生命周期管理、IPC 协议、重连机制
- **调试难度**：跨进程调试比单进程调试复杂，需要同时 attach 多个进程

### 中性

- 生产环境默认使用 Daemon 模式，开发环境可选用 Inline 模式，由部署者根据场景权衡

## 参考

- [CHANGELOG.md](../../CHANGELOG.md) — v0.27.0 插件系统、v0.27.1 插件加固
- [插件开发指南](../plugin-development.md) — 插件部署与 Inline/Daemon 模式
- [ADR-0002](./0002-power-native-agentos.md) — 安全约束为内核法律
