# ADR-0045：System2 供给架构——远程 peer 主线 + 本地推理 spike 带杀死判据

- 状态：**Draft（依据 owner D1 裁决先行实施；owner 签字 = 49 卷 Exit 登记项，未签不阻塞）**
- 日期：2026-08-22
- 关联：蓝图/49 卷（本 ADR = WP-49.1 第一交付物）；D1 裁决（44-00 §1：保留
  LLM 路线，任何卷不得擅自砍 LLM/AI crate，不得把 host llama.cpp 声称为目标侧
  能力）；ADR-0036（491MB 模型不可入 init bundle）；ADR-0035/0042（AI 决策
  审计与角色边界）；蓝图/51（选板判据）

## 问题

v1.0 产品线中 System2（LLM 慢路径决策）的算力供给位置未裁决。代码事实
（2026-08-22 Entry 重验）：

1. 目标侧后端只有 `UnavailableBackend`（默认）与 `HeuristicBackend`
   （SOC 死区启发式，±5kW 有界规则）；
2. llama.cpp 为 **x86-64 host 构建**（`third_party/llama.cpp/build/src/libllama.a`
   readelf 实证 Machine=AMD X86-64），FFI 仅 host feature 门控可用；
   `test-model.gguf` 491MB 进不了 init bundle（ADR-0036）；
3. `DualBrainCoordinator`（7 步编排）target 零调用；dual-brain 仅
   类型+DecisionGate+DecisionAudit 进入 dispatch 装配；
4. intent-parser/fast-path/infer-scheduler/prompt-template 被
   control-services→dual-brain 类型依赖拖进 target 闭包但零调用。

不裁决则：AI 宣称无供给事实支撑（虚假宣称风险），或被迫砍 LLM 路线
（违反 D1）。

## 备选方案与裁决

| 方案 | 内容 | 判定 |
|---|---|---|
| **A 本地推理** | target 侧 GGUF 推理器（自研 no_std 或裁剪 ggml 核心）+ 数据盘模型加载 | **spike 验证（WP-49.3），不预设结论**。三关卡顺序执行、任一失败即 NO-GO 终止：①静态内存审计先行（内存优先铁律）：权重常驻+KV cache+激活 vs 板内存预算（QEMU 1G/按 2G 规划）；②aarch64 交叉编译可行性（ggml 对 pthread/文件 IO/动态分配的依赖逐项清点替换方案）；③host 原型延迟测量推算 target WCET。NO-GO 重启条件（换板带 NPU/模型 ≤0.5B Q4 出新压缩格式等）写入 spike 报告 |
| **B 远程 peer（✅ 采纳为主线）** | System2 = 站控机/边缘服务器上的 LLM 服务（runner 或任意 OpenAI 兼容推理服务），经 MQTT/TLS 与 target 交互。target 侧保留：intent 序列化（intent-contract）、超时降级、DecisionGate、safety-validator trusted 全量校验、dispatch 授权链 | 复用既有组件最多（MQTT 46 卷链/intent-contract/safety-validator/model-ota 全在库且经 46/47 实弹验证）；网络断→HeuristicBackend 降级（**fail-operational 而非 fail-dead**）；无新硬件依赖；远程 peer 定性为**不受信输入**——其应答必须过 trusted 路径全量校验方可进入决策 |
| **C 协处理器** | 板载 NPU/加速模块 | 硬件依赖，v1.0 不阻塞；转为 51 卷选板判据的可选输入（若 49.3 spike NO-GO，NPU 板卡即 A 方案重启条件之一） |

## 决策细则（方案 B）

1. **信任边界**：peer 应答 = 不受信外部输入。target 侧校验全量执行：
   schema 校验/枚举闭集/数值域（功率 ≤ 预算、TTL 上限）/sequence 单调/
   请求相关性（req_id 配对）/新鲜度；坏应答→丢弃计数→本次 tick 降级，
   决不直接采信。
2. **降级语义**：超时（可配，默认 5s）→ 本次 tick 由 HeuristicBackend
   （System1）接管；连续 N 次（可配，默认 3）→ peer 标记 Unhealthy
   （遥测/串口可见）；恢复探测成功→自动回 System2。切换事件全部入
   审计链（DecisionAudit，ADR-0042 角色边界不变：dispatch 仍是唯一投递
   与最终授权点）。
3. **决策留痕**：每次 AI 参与决策将 prompt 摘要/应答摘要/最终裁决写入
   audit 链；**截断策略**（防审计膨胀）：prompt 摘要 ≤48 字符、应答摘要
   ≤64 字符、只留结构化要素（req_id/功率/理由码），不落全文。
4. **宣称纪律（硬约束）**：产品宣称只允许「**AI 辅助决策（需配套站端
   服务）**」；禁止宣称「设备端本地大模型」；禁止把 host llama.cpp /
   host runner 声称为目标侧能力。sku-matrix/README/交付文档措辞受此条
   约束。
5. **传输**：复用 46 卷 MQTT 链（QoS1），topic 约定见 WP-49.2 证据；
   TLS 化沿用 47 卷 SecureChannelFactory 策略面（产品部署项，QEMU 证据
   以 slirp 明文链路为界如实登记）。

## 影响

- WP-49.2 直接按 B 实施（卷执行模式：不停等签字）；
- WP-49.3 spike 只出报告不做产品接线、不向 target 闭包加入推理代码；
- WP-49.4 闭包死重治理以「消费或移出」逐一判定 dual-brain 拖入的 AI crate；
- 若 A 方案未来翻案（GO），须新 ADR 取代本 ADR 的方案裁决部分并重走
  内存/实时性门禁。

## owner 签字栏

- [ ] 签字（姓名/日期）：__________ —— 未签状态如实登记于 49 卷 Exit
      Gate（BLOCKED，owner 动作）；不影响 B 主线的实施与证据效力，
      但 v1.0 宣称冻结前必须补签。
