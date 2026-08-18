# PROJECT.md — EnerOS 项目记忆（第一性原则）

> **状态纠偏（2026-08-12）**：本文的旧版本号、crate 数、DDS/产品能力、`蓝图/09–20` 入口和阶段状态已经漂移。当前开发入口改为 `蓝图/35-00`、`35-01` 与唯一待执行的 36–43 版本；已提交和未提交代码都按 `35-01` 判定。本文第一性原则仍有效，旧进度/路线不得作为验收事实。

> 本文件是每次开发会话的**第一输入**。简洁、只说事实、不定虚目标。
> 领域词汇事实源见 `CONTEXT.md`；架构决策冲突先由蓝图 36 裁决；当前差距与规划见 `蓝图/35-00`、`35-01`、`36–43`。

## 1. 项目是什么

**EnerOS：基于 seL4 微内核（ARM64）+ Rust no_std 的能源 Agent OS。**
一句话：把"采集→决策→控制→交易"做成一个可验证、可审计、AI 驱动的操作系统，跑在飞腾/鲲鹏等国产硬件上。

- 内核：seL4 14.0.0（形式化验证微内核），用户态全 Rust。
- 已有地基：签名介质、seL4 root-task、EL0 spawn/IPC、协议与 AI/安全组件；其产品闭环状态逐项见 `蓝图/35-01`。
- 目标 AI 链：意图/solver/LLM 组件 → 可信采集 → 唯一 ControlDispatch → 设备 ACK/readback；当前尚未产品闭环。
- 协议组件：Modbus / IEC104 / IEC61850 / MQTT；DDS 路线正在退出，协议服务是否进入产品只认 ProductManifest。
- 安全组件：SM2/3/4、TLS、启动、OTA、证明、PKI；具体产品算法/provider 与商业授权尚须蓝图 36 裁决。

## 2. 第一性原则（不可妥协）

1. **真实大于完整**：宁可能力少而真，不要全而假。Mock 只属于测试，生产路径零假实现。
2. **验证大于声称**：任何能力必须附带可执行证据（测试/QEMU 输出/真机报告）；"自证通过"无效。
3. **安全默认拒绝**：control-gateway default-deny；验签不过不写盘；fatal 违规必阻断。
4. **no_std 是边界**：EL0/目标侧 crate 禁 std；标准库只能出现在 host 工具与声明过的服务。
5. **单一事实源**：领域词汇看 `CONTEXT.md`；当前代码/缺口看 `蓝图/35-00`、`35-01`；执行只看唯一一卷 `36–43`。

## 3. 目标

**v1.0.0 = 商业交付授权**，唯一现行判据见蓝图 43：

- 真机 72h 全链路浸泡通过（启动→采集→存储→协议→调度→安全→运维）；
- 真实对端互操作报告（104 主站 / Modbus 设备 / MMS 客户端 / MQTT broker）；
- 密码学独立评审通过；渗透测试零 P0/P1；
- 合规证据机器可重算；CI 换机可复现；宣称能力=实测能力。

## 4. 当前状态（诚实基线）

- workspace 版本字段为 `0.313.0`（44-00 §3 纪律：未过终验不挂 rc/1.0 标记；每 WP Exit 递增 patch）；版本号不代表验收；当前商业裁决是 **NO-GO / NOT-READY**。
- 当前审查基线为 `HEAD=473b1e26681be33acc654f32e88251d8c19ad3cf` 加全部未提交/未跟踪开发代码。
- 正式介质当前主要是 boot/kernel/root/demo/占位驱动服务；Agent、AI、协议、OTA、联邦、管理和可观测尚未形成完整产品装配。
- 当前共 16 个 P0、30 个 P1、5 个 P2（审查快照）；详见 `蓝图/35-00`，代码事实与旧规划抹平见 `35-01`。
- 唯一路线：36 事实源 → 37 runtime/trust → 38 ARM/HAL/driver → 39 storage/net/protocol → 40 Agent/AI/control → 41 business/ops → 42 L0–L3 evidence → 43 L4/Go-No-Go。

## 5. 架构分层（crate 数以 `cargo metadata` 当前结果为准）

```
boot/abi        UEFI 引导链、镜像格式、ELF 加载、IPC wire        （真实度：高）
hal/runtime     seL4 FFI、root-task、EL0 spawn/IPC                 （基础接入；NX/PIE/严格装配待37）
drivers         storage/fs/tsdb/net/cellular/rs485/can/...         （组件多；生命周期/DMA/产品服务待38–39）
protocols       modbus/iec104/iec61850/mqtt/control-gateway        （codec多；产品transport/service待39）
security        crypto/tls/secure-boot/ota/attestation/audit       （组件多；唯一信任/授权/产品接线待36–41）
ai              dual-brain/solver/llm/intent/safety/...            （算法多；可信输入/dispatch/EIPC待40）
agents          市场/设备/联邦/结算/租户/...                       （状态机多；正式装配/持久恢复待40–41）
observability   metrics/logging/tracing/dashboard/mgmt-api/...     （组件多；target provider/exporter/auth待41）
xtask/ci/fuzz   介质/签名/QEMU/SBOM/evidence                       （基础多；假绿与可移植性待36/42）
```

## 6. 开发铁律

1. **Mock 纪律**：mock 仅允许 `#[cfg(test)]` 或 release 下 `compile_error!` 熔断的 feature；生产构造一律依赖注入。
2. **测试纪律**：禁改断言迁就实现；防回归测试必须能证明"旧行为错"；测试与实现同源即重写。
3. **错误纪律**：生产路径禁新增 `unwrap/expect`；错误要么处理、要么入指标、要么注释豁免理由。
4. **unsafe 纪律**：每块 SAFETY 注释；禁共享引用造可写指针。
5. **数值纪律**：金额/电量定点化（MilliYuan 先例）；禁 `as` 截断，用 `try_from`/clamp；量纲必标。
6. **版本纪律**：一版本一工作包；验收标准逐条自证后才递增版本号；CI 必须真绿。
7. **术语纪律**：新类型先查 `CONTEXT.md` 与已废弃词汇表，禁发明同义词。
8. **会话纪律**：完整读取 `35-00`、`35-01` 和当前唯一版本；按该卷 Entry/侦察/实现/二次验证/Exit Gate 执行。

## 7. 文档地图

| 文件 | 作用 |
|---|---|
| `PROJECT.md`（本文） | 第一性原则与会话入口 |
| `CONTEXT.md` + `docs/adr/` | 领域词汇；ADR 冲突先由 36 registry/policy 裁决 |
| `蓝图/35-00` | 当前工作树商业代码审查与 NO-GO 总报告 |
| `蓝图/35-01` | 已提交/未提交代码事实、已实现勿重做、旧规划作废迁移总账 |
| `蓝图/36–43` | 唯一串行修复与商业终验路线 |
| `蓝图/00–34` | 历史设计/需求/修复来源，无执行与验收权 |

## 8. 常用命令

```bash
cargo xtask build all            # 目标命令；当前环境/依赖失败不得记 PASS
cargo xtask image raw|iso|recovery|vars|update
cargo xtask run qemu-prod
cargo xtask evidence verify|gate # release 只接受 gate PASS
cargo test --workspace           # 当前并非全绿，执行结果按35-00记录
```
