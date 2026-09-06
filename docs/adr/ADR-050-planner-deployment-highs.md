# ADR-050：Planner 部署裁决与求解后端选型（蓝图 61 v0.314.37）

- 状态：已接受（2026-08-31）
- 关联：蓝图 61 [09] §3.2；[16] §3.8/§4；ADR-0048（决策链）
- 验收映射：[09] §3.9 退出 gate——产品 closure 含真实 HiGHS backend；
  mock-leak gate 全绿；CandidatePlan 数值可追溯；无设备副作用
- **状态注记（蓝图 68.5，v0.325.0，2026-09-06）**：裁决 1/2 的**第一形态
  落地**——`crates/ai/planner-peer`（host-only std bin，target 闭包零
  消费）以真 HiGHS（`eneros-solver-core/highs-ffi`，L2 门只收 Optimal）
  经既有 RemoteBrain peer 通道（MQTT QoS1，F26 单值投影）供给提案；
  目标侧决策链消费 + D3 DegradationLease 持签降级（蓝图 68.5 卷册与
  QEMU 证据为锚）。裁决 2 的 mTLS/attestation DecisionRequest 全通道、
  结构化 CertifiedPlan 通道（schema v2）与 certifier 生产接电（ADR-051
  过渡窗）**仍为后续卷**；Phase 5 语义维持。

## 背景

[09] §3.2 要求：优先验证 HiGHS 能否在目标 aarch64/seL4 构建并满足预算；
若目标不具备 C runtime/内存条件，生产 planner-service 运行在受控站内
计算分区，经 mTLS/attestation 接收 DecisionRequest，且不得持有
protocol writer、Gateway key 或 seL4 device capability。

事实核查（2026-08-31）：

- `third_party/highs-1.7.2/build/lib/libhighs.a` 仅有 **x86-64** 构建；
  HiGHS 是 C++（libstdc++/zlib 依赖），seL4 目标侧无 C++ runtime 与
  对应内存预算（当前 root-task 堆 5MiB 量级，HiGHS 静态库 ~数 MiB 且
  求解期堆需求远超）。
- `solver-core` 的 HiGHS FFI 在 host x86-64 全链可测（30 测试绿）。

## 裁决

1. **生产后端 = 真实 HiGHS（`eneros-solver-core/highs-ffi`）**；
   `eneros-planner` 基线依赖即启用（G7：release 不得 mock）。
2. **部署位置 = host 受控计算分区**（std crate）。aarch64/seL4 目标侧
   链接被裁决为不可行（而非"待验证"）——上述事实为证；planner 经
   DecisionRequest wire 接入（Phase 5 起 mTLS/attestation 通道）。
   类型面已隔离：`eneros-planner` 无 protocol writer、无 Gateway、
   无 seL4 capability 依赖。
3. **MockSolver 仅允许 E1/E2 测试**（[09] §3.2）；planner 自身单测用
   `cfg(test)` ScriptedSolver（重放真实 HiGHS 结果做门禁负例），不
   引 solver-core/mock feature。
4. **求解门数值**：ToleranceProfile L2(id=1) 只接受 Optimal；
   L3(id=2) gap ≤ 0.5%。primal ≤ 1e-7×max(1, scale)、integrality
   ≤ 1e-6 进 profile 不散落。gap 数据源 = HiGHS info `mip_gap`
   （`Highs_getDoubleInfoValue`，solver-core FFI 本卷新增）。
5. **超时**：backend 原生 `time_limit` + 外层 Instant 记账；SolveResult
   无 incumbent 字段 → Timeout 一律拒绝（fail-closed，登记）。

## 后果

- 目标侧不运行 planner；目标侧控制链继续消费 CertifiedPlan（.38 认证
  卷裁决见 ADR-051）。
- 多节点拓扑线性化（B-theta）待 catalog 拓扑容量数据齐备（[09] §3.3
  登记），本卷单平衡节点。
- assignments BlobRef 待存储句柄移交（Phase 2/3 已登记 BLOCKED），
  inline 上限 64，超限显式拒绝。
- [09] §3.6 "保存非敏感 MPS/变量目录 BlobRef 用于复验"：模型 digest +
  变量目录已随 BuiltModel/求解出证；**MPS 文件落盘与 BlobRef 寻址依赖
  存储句柄移交**（同上 BLOCKED）——复验面当前为 digest 对账，文件级
  复验登记待存储面就绪。
- Timeout-with-incumbent：SolveResult 无 incumbent 字段，超时一律拒绝
  （fail-closed）；"有 incumbent 接受"路径待 solver-core FFI 扩展
  （Highs_getMipIterationCount/incumbent 接口）后开放。
