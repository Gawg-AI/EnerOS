# ADR-021：DDS agent 总线降级替换为 MQTT（21-B B2 方案 2）

- 状态：已接受（2026-08-09，21-B §5 预设裁决 1，用户已批准）
- 关联：蓝图/21-B §2.B2；ADR-022（国密 vetted 库）；ADR-023（QEMU-first transport）
- 背景蓝图：`蓝图/21-B-商业化解阻塞-P0软件侧清偿开发蓝图.md`

## 问题

`agent-bus-dds` 的 release 默认只有 `MockDdsNode`——"agent 总线在 release
是 mock"是断链 B 的核心。两个候选：

- **方案 1（接 C 库）**：cyclone-dds feature 走通 `CycloneDdsNode`
  （真实链接 third_party/CycloneDDS，release 默认启用）。
- **方案 2（降级替换）**：v1.0.0 agent 总线改用 MQTT（`mqtt/src/real.rs`
  已有 `NetTcpTransport` 真实实现），agent-bus-dds 标 out-of-v1.0。

## 决策

**选方案 2（降级替换）**——2026-08-09 用户预设裁决生效。

理由：
1. **MQTT 真实现已就绪**：`eneros-mqtt` 含 MQTT 3.1.1 完整协议层
   （CONNECT/CONNACK/SUBSCRIBE/SUBACK/PUBLISH 握手）+ `NetTcpTransport`
   （eneros-net/smoltcp）；model-ota 已作为 agent 经 MQTT 收发
   （transport_mqtt.rs，R2-4 验收：61 测试，含验签禁落盘）。
2. **DDS 接 C 库工程量大且未通**：`CycloneDdsNode` 当前返回 `Ffi(-1)`，
   交叉编译 + 协议调试周期不可控，v1.0.0 周期内风险高。
3. **真实可用的 MQTT 总线 > 未接通的 DDS 桩**（第一性原理：真实大于完整）。

## 实现（已随 B2 落地）

1. `agent-bus-dds`：`[package.metadata.eneros]` 声明
   `commercial_scope = "out-of-v1.0"` + `agent_bus_role = "replaced-by-mqtt"`；
   顶层 doc 双处声明（ADR-021）。
2. **mock 收编**：`MockDdsNode` / `MockSigner` / `MockCapabilityVerifier`
   全部 `#[cfg(any(test, feature = "mock"))]`——release 构建不可见
   （probe 验证：E0433 编译失败）；`mock` feature 显式声明。
3. **依赖方**：grid_agent / twin-agent 的测试经 dev-dependencies 显式
   启用 mock feature；生产代码零 mock 引用。
4. **firmware profile 一致**：`product/profiles/qemu-prod.json` binaries
   列表无 agent-bus-dds（现状已一致，无需改动）；deny.toml 无特批条目。
5. **MQTT 总线证据**：model-ota 真实 MQTT 协议层收发（R2-4，61 测试
   全过）——"至少一个 agent 通过 MQTT 总线收发消息"。

## 替代方案评估

- 方案 1（Cyclone DDS）：协议语义更贴近 DDS 标准，但 C 库集成未通 +
  交叉编译成本高——v1.0.0 后按市场需求恢复（P1）。

## 风险

- MQTT 总线缺少 DDS 的 QoS/发现语义：v1.0.0 的 agent 拓扑为已知
  节点集合（配置注入 topic 路由），MQTT topic 层级可覆盖；若未来需要
  动态发现，再评估 DDS（P1）。
- agent-bus-dds 降级后，依赖它的代码路径冻结：不允许新增生产依赖
  （CI 审计关注）。
