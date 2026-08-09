# PROJECT.md — EnerOS 项目记忆（第一性原则）

> 本文件是每次开发会话的**第一输入**。简洁、只说事实、不定虚目标。
> 领域词汇事实源见 `CONTEXT.md`；架构决策见 `docs/adr/`；差距与规划见 `蓝图/09–17`。

## 1. 项目是什么

**EnerOS：基于 seL4 微内核（ARM64）+ Rust no_std 的能源 Agent OS。**
一句话：把"采集→决策→控制→交易"做成一个可验证、可审计、AI 驱动的操作系统，跑在飞腾/鲲鹏等国产硬件上。

- 内核：seL4 14.0.0（形式化验证微内核），用户态全 Rust。
- 智能体：签名 ELF PIE 插件 → 受限 EL0 进程 → IPC 通信（CONTEXT.md §4）。
- AI 决策链：自然语言意图 → LLM（llama.cpp）→ MILP（HiGHS）→ 安全校验 → 网关裁决 → 设备执行。
- 协议面：Modbus / IEC104 / IEC61850 / MQTT / DDS（电力行业对通）。
- 安全面：国密 SM2/3/4、安全启动、OTA-A/B、远程证明、PKI。

## 2. 第一性原则（不可妥协）

1. **真实大于完整**：宁可能力少而真，不要全而假。Mock 只属于测试，生产路径零假实现。
2. **验证大于声称**：任何能力必须附带可执行证据（测试/QEMU 输出/真机报告）；"自证通过"无效。
3. **安全默认拒绝**：control-gateway default-deny；验签不过不写盘；fatal 违规必阻断。
4. **no_std 是边界**：EL0/目标侧 crate 禁 std；标准库只能出现在 host 工具与声明过的服务。
5. **单一事实源**：领域词汇看 `CONTEXT.md`，差距看 `蓝图/09`，版本计划看 `蓝图/11–14`。

## 3. 目标

**v1.0.0 = 商业交付授权**，判据（蓝图/14 闸门 G1–G7）：

- 真机 72h 全链路浸泡通过（启动→采集→存储→协议→调度→安全→运维）；
- 真实对端互操作报告（104 主站 / Modbus 设备 / MMS 客户端 / MQTT broker）；
- 密码学独立评审通过；渗透测试零 P0/P1；
- 合规证据机器可重算；CI 换机可复现；宣称能力=实测能力。

## 4. 当前状态（诚实基线）

- 版本 v0.192.0，就绪度 **演示级偏上（Demo+）**——表面完整，三条结构性断链未通：
  AI 决策链（release 无可编译组合）、设备硬件链（真机存储/通信断）、设备端安全链（DDS/TLS/PKI 空转）。
- 差距清单：`蓝图/09`（P0×28 / P1×60+ / P2×40+，每条带文件:行号证据）。
- 清偿路线：阶段 A（v0.193–212，P0 清零）→ B（v0.213–242，深度真实化）→ C（v0.243–252，质量加固）→ D（v1.0.0 验收闸门）。

## 5. 架构分层（112 crate）

```
boot/abi        UEFI 引导链、镜像格式、ELF 加载、IPC wire        （真实度：高）
hal/runtime     MMIO 驱动底座、seL4 FFI、root-task、EL0 服务      （断链：spawn/FFI 未接）
drivers         storage/fs/tsdb/net/cellular/rs485/can/...        （断链：真机读写骨架）
protocols       modbus/iec104/iec61850/mqtt/dds/control-gateway   （编解码真实，传输半悬）
security        国密/tls/secure-boot/ota/attestation/...          （真实逻辑多，依赖链断）
ai              dual-brain/solver/llm/intent/safety/...           （FFI 真实，中层接口断）
agents          市场/设备/联邦/结算/租户/... 38 crate             （Mock-only 重灾区）
observability   metrics/logging/tracing/dashboard/mgmt-api/...    （采集真，导出/渲染空）
xtask/ci/fuzz   交付工具链（镜像/签名/QEMU 证据/SBOM）            （真实度：高）
```

## 6. 开发铁律

1. **Mock 纪律**：mock 仅允许 `#[cfg(test)]` 或 release 下 `compile_error!` 熔断的 feature；生产构造一律依赖注入。
2. **测试纪律**：禁改断言迁就实现；防回归测试必须能证明"旧行为错"；测试与实现同源即重写。
3. **错误纪律**：生产路径禁新增 `unwrap/expect`；错误要么处理、要么入指标、要么注释豁免理由。
4. **unsafe 纪律**：每块 SAFETY 注释；禁共享引用造可写指针。
5. **数值纪律**：金额/电量定点化（MilliYuan 先例）；禁 `as` 截断，用 `try_from`/clamp；量纲必标。
6. **版本纪律**：一版本一工作包；验收标准逐条自证后才递增版本号；CI 必须真绿。
7. **术语纪律**：新类型先查 `CONTEXT.md` 与已废弃词汇表，禁发明同义词。
8. **会话纪律**：按 `蓝图/15` 四阶段走（侦察→设计→实现→验收），提示词用 `蓝图/10`。

## 7. 文档地图

| 文件 | 作用 |
|---|---|
| `PROJECT.md`（本文） | 第一性原则与会话入口 |
| `CONTEXT.md` + `docs/adr/` | 领域词汇与架构决策事实源 |
| `蓝图/09` | 差距证据库（全部结论的溯源） |
| `蓝图/10` `15` | 开发会话提示词与四阶段手册 |
| `蓝图/11–14` | 阶段 A/B/C/D 逐版本计划 |
| `蓝图/16` | 烧录工具（eneros-flash）设计 |
| `蓝图/17` | 图形环境（Shell）与 AI 驾驶舱设计 |
| `蓝图/19` `20` | v0.193→v1.0.0 验收审查 + 打回修复技术规格（当前最高优先级） |

## 8. 常用命令

```bash
cargo xtask build all            # 全量构建
cargo xtask image raw|iso|recovery|vars|update   # 镜像产出
cargo xtask run qemu-prod        # QEMU 运行与证据
cargo xtask evidence verify|gate # 证据校验/门禁
cargo test --workspace           # 测试（核心 crate 见 CI 排除项，阶段 A 修复）
```
