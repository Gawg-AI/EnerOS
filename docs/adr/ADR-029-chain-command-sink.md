# ADR-029 — ChainCommandSink：AI→设备必经 ControlChain（22 W2.2）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W2.2 强约束 + §1 锁定 5）
- **关联**：ADR-021（DDS/MQTT）、`蓝图/22` §2.W2.2/W2.4、§8 G-R2

## 背景

22 §2.W2.2：`CommandSink` 唯一非测试实现是 `MockCommandSink`（永远 Ok）；
`ControlGateway`/`ControlChain` 在生产 crate 零实例化。要求：
`ChainCommandSink`（`write` 走 reconcile→授权→设备写入→回读）；设备拒绝
映射 `DualBrainError`（非 Ok）；`MockCommandSink` 降级
`#[cfg(any(test, feature="mock"))]`；`eneros-ai-runner` 改用之。

## 现状侦察（2026-08-09 代码为准）

1. `ControlChain::execute(buf, now, feedback) -> ChainOutcome`（gateway HMAC
   授权 → 租约 → 策略 → SBO → 安全校验，default-deny）；`EnvelopeBuilder`
   构造签名信封（key/lease/policy_version/actor_kind 必填）。
2. `ControlCommand` 仅开关语义（Single On/Off）——无功率设定字段。
3. `DualBrainError::DispatchError(String)` 可承载拒绝原因。
4. runner 的 `FileCommandSink`（JSON Lines 文件总线）是真实设备通道
   （21-B A1 生产实现）。

## 决策

1. **`ChainCommandSink`（dual-brain/chain_sink.rs）**：`CommandSink` 生产
   实现，内部持有 `ControlChain` + `DeviceExecutor`。`write` 序列：
   SBO 选择（AI 写命令 = 选择+执行原子语义）→ 签名信封 → `execute` →
   `Executed` → executor 真实设备写入 → 拒绝（Gateway/Lease/Policy/SBO/
   Unsafe/Feedback）映射 `DualBrainError::DispatchError`（非 Ok，W2.4）。
2. **功率语义**：`ControlCommand` 无功率字段——`power_kw > 0 → On`（放电），
   `≤0 → Off`（充电）；功率作为**执行载荷**传给 `DeviceExecutor`（开关
   授权由链保证，功率执行由设备层保证）。ControlChain 升级支持功率设定
   为后续协议演进（W4.2 远程通道可扩展 `ControlCommand`）。
3. **SBO 内化**：sink 内部 `sbo_select`（AI 无"选点"概念，命令即选择+执行）。
4. **runner 装配**：`ChainCommandSink`（build_chain：策略放行 Ai→dev1/pt10、
   租约 3600s、安全点 PccBreaker）+ `FileExecutor`（授权命令经文件总线
   落盘）。`MockCommandSink` 保持 `#[cfg(any(test, feature="mock"))]`
   （21-B mock 纪律已满足——release 不可见）。
5. **key 注入**：`CHAIN_KEY` 编译期常量（v1.0 前由运维通道注入
   ——runner 无配置管理，W4.2 远程配置通道落地后迁移）。

## 影响

- AI 旁路设备写入的路径**不存在**（唯一生产 sink 必经 ControlChain）；
  default-deny 链拒绝 → 决策系统感知（W2.4 输入）。
- runner 证据（sink JSONL）不变（FileExecutor 复用 FileCommandSink 格式）。
- `dual-brain`/`runner` 新增依赖：control-gateway + telemetry-model
  （workspace 内，无新外部依赖）。

## 备选方案

- **runner 直接换 MockCommandSink 替身**：违反锁定 5，否决。
- **ControlCommand 扩展功率字段**：改 telemetry-model + gateway 信封格式
  （协议破坏面大），留待协议演进，ADR 记录。
