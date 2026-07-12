# Tasks

## Phase 1 蓝图生成任务（v0.23.0 ~ v0.74.0，共 52 个版本）

- [x] Task 1: P1-A 存储与文件系统（v0.23.0~v0.26.0，4 版）
  - [x] SubTask 1.1: v0.23.0 存储驱动（eMMC/NVMe 块设备驱动、DMA 传输、坏块管理）
  - [x] SubTask 1.2: v0.24.0 日志结构文件系统（no_std FS、参考 littlefs、掉电安全、磨损均衡）
  - [x] SubTask 1.3: v0.25.0 时序数据存储引擎（列式存储、时间索引、Snappy 压缩、TTL 清理）
  - [x] SubTask 1.4: v0.26.0 配置管理系统（TOML/JSON、热加载通知、版本管理、默认值）

- [x] Task 2: P1-B 网络协议栈（v0.27.0~v0.30.0，4 版）
  - [x] SubTask 2.1: v0.27.0 以太网网卡驱动（MAC 驱动、DMA 收发环形缓冲、PHY 配置）
  - [x] SubTask 2.2: v0.28.0 TCP/IP 协议栈集成（smoltcp 或自研、ARP/IPv4/TCP/UDP、DHCP）
  - [x] SubTask 2.3: v0.29.0 Socket 抽象层（统一 API、非阻塞 IO、select/poll、连接管理）
  - [x] SubTask 2.4: v0.30.0 网络栈安全与性能（防火墙规则、连接数限制、DDoS 防护）

- [x] Task 3: P1-C 密码学与安全基础（v0.31.0~v0.32.0，2 版）
  - [x] SubTask 3.1: v0.31.0 国密算法库（纯 Rust SM2/SM3/SM4、CSRNG、签名/哈希/加解密）
  - [x] SubTask 3.2: v0.32.0 PKI 证书基础（X.509 解析/签发、CA 证书链验证、CRL 吊销）

- [x] Task 4: P1-D Agent Runtime 基础 上半部分（v0.33.0~v0.36.0，4 版）
  - [x] SubTask 4.1: v0.33.0 Agent 抽象与描述符（AgentDescriptor：ID/类型/能力/状态/优先级/配额/信任等级）
  - [x] SubTask 4.2: v0.34.0 Agent 注册表与发现（全局注册表、按 ID/类型查找、枚举）
  - [x] SubTask 4.3: v0.35.0 Agent 生命周期状态机（Created→Ready→Running→Suspended→Error→Recovering→Dead）
  - [x] SubTask 4.4: v0.36.0 Agent 启动与初始化（代码加载、状态初始化、连接初始化）

- [x] Task 5: P1-D Agent Runtime 基础 下半部分 + P1-E System Agent（v0.37.0~v0.42.0，6 版）
  - [x] SubTask 5.1: v0.37.0 Agent 心跳与健康检查（1s 周期、3 次超时=故障）
  - [x] SubTask 5.2: v0.38.0 Agent 崩溃自动重启（最多 3 次、检查点恢复、3 次失败→Dead）
  - [x] SubTask 5.3: v0.39.0 能力模型 - Capability Token（令牌结构：owner/target/permission/constraints/signature）
  - [x] SubTask 5.4: v0.40.0 能力模型 - 签发/校验/撤销（能力管理器、运行时校验、故障冻结）
  - [x] SubTask 5.5: v0.41.0 System Agent 核心（资源监控 CPU/内存/温度、Agent 启停管理）
  - [x] SubTask 5.6: v0.42.0 System Agent 故障恢复编排（多 Agent 协调恢复、依赖顺序处理）

- [x] Task 6: P1-F 设备协议栈 上半部分（v0.43.0~v0.47.0，5 版）
  - [x] SubTask 6.1: v0.43.0 用户态驱动框架（DeviceDriver trait、注册/发现/隔离/生命周期）
  - [x] SubTask 6.2: v0.44.0 RS485 串口驱动（基于 HAL、数据帧收发、超时重传）
  - [x] SubTask 6.3: v0.45.0 Modbus RTU 主站（协议帧解析、功能码 03/06/10、点表映射）
  - [x] SubTask 6.4: v0.46.0 Modbus TCP（Modbus over TCP、多设备轮询）
  - [x] SubTask 6.5: v0.47.0 CAN 驱动（CAN 帧收发、ID 过滤、基础 CAN 协议）

- [x] Task 7: P1-F 设备协议栈 下半部分 + P1-G 四遥与SOE（v0.48.0~v0.53.0，6 版）
  - [x] SubTask 7.1: v0.48.0 IEC 104 从站（协议栈、ASDU 处理、遥测/遥信/遥控响应）
  - [x] SubTask 7.2: v0.49.0 IEC 104 主站（主动轮询、总召唤、时钟同步命令）
  - [x] SubTask 7.3: v0.50.0 统一点表模型 UPA（DataPoint：point_id/device_id/name/type/value/quality/timestamp）
  - [x] SubTask 7.4: v0.51.0 协议抽象层（统一 PointAccess trait、协议适配器、多协议共存）
  - [x] SubTask 7.5: v0.52.0 四遥标准数据模型（遥测/遥信/遥控/遥调统一模型、品质标志、死区过滤、变化上报）
  - [x] SubTask 7.6: v0.53.0 SOE 事件顺序记录引擎（ms 级时标事件队列、不乱序保证、持久化、上传）

- [x] Task 8: P1-H RTOS 组件（v0.54.0~v0.58.0，5 版）
  - [x] SubTask 8.1: v0.54.0 RTOS 控制闭环引擎（周期 10ms、PID 基础算法、设定值跟踪）
  - [x] SubTask 8.2: v0.55.0 高频采样服务（设备状态采集、状态快照写入共享内存）
  - [x] SubTask 8.3: v0.56.0 命令消费与执行（从 Control Bus 读命令、TTL 检查、约束包检查、协议下发）
  - [x] SubTask 8.4: v0.57.0 降级规则引擎（储能停充放、维持出力、安全默认策略）
  - [x] SubTask 8.5: v0.58.0 看门狗与端到端降级流程（Agent 心跳监控、TTL 过期触发、降级切换、恢复回切）

- [x] Task 9: P1-I AI Runtime — LLM（v0.59.0~v0.63.0，5 版）
  - [x] SubTask 9.1: v0.59.0 LLM 推理引擎选型与 FFI 封装（llama.cpp、Rust 封装层、LlmEngine trait）
  - [x] SubTask 9.2: v0.60.0 模型加载与内存管理（GGUF 模型文件加载、内存分配、卸载）
  - [x] SubTask 9.3: v0.61.0 7B INT4 量化模型部署（量化配置、推理正确性验证）
  - [x] SubTask 9.4: v0.62.0 推理调度与并发控制（请求队列、并发 ≤ 2、KV Cache 管理）
  - [x] SubTask 9.5: v0.63.0 Prompt 模板系统 + JSON 输出约束（电力专用模板、JSON Schema 校验）

- [x] Task 10: P1-J AI Runtime — Solver（v0.64.0~v0.68.0，5 版）
  - [x] SubTask 10.1: v0.64.0 LP 求解器集成（HiGHS via FFI、Rust 封装、Solver trait）
  - [x] SubTask 10.2: v0.65.0 优化问题建模框架（目标函数/决策变量/约束的 Rust DSL、OptProblem builder）
  - [x] SubTask 10.3: v0.66.0 能源调度 LP 模型（功率平衡/设备容量/SOC/爬坡约束）
  - [x] SubTask 10.4: v0.67.0 安全校验器（电气安全校验、保护配合校验、约束包一致性校验、截断到安全边界）
  - [x] SubTask 10.5: v0.68.0 意图解析器（LLM JSON 意图 → 优化问题参数转换、IntentParser）

- [x] Task 11: P1-K 双脑协同 + P1-L MVP 集成（v0.69.0~v0.74.0，6 版）
  - [x] SubTask 11.1: v0.69.0 LLM → Solver 意图契约（统一 JSON schema、双向转换逻辑、版本化）
  - [x] SubTask 11.2: v0.70.0 实时路径 - Solver only（神经部分快速前馈→符号求解、不调用 LLM、< 500ms）
  - [x] SubTask 11.3: v0.71.0 双脑协同联调（端到端：感知→意图→求解→校验、延迟分解测量、< 2s）
  - [x] SubTask 11.4: v0.72.0 Energy Agent + Market Agent（能源调度核心 + 市场数据接收）
  - [x] SubTask 11.5: v0.73.0 Device Agent（设备管理：状态采集、命令执行、多设备）
  - [x] SubTask 11.6: v0.74.0 ★ MVP 端到端集成 - 储能自治场景（电价→LLM→Solver→Control Bus→RTOS→储能充放电、收益 ≥ 10%）

# Task Dependencies

- [Task 2] depends on [Task 1] （网络栈依赖文件系统配置管理）
- [Task 3] depends on [Task 1] （国密依赖用户堆 v0.11.0，独立于 Task 1/2，但逻辑上在 P1-C）
- [Task 4] depends on [Task 3] （Agent Runtime 依赖能力模型，v0.33.0 依赖 v0.22.0+v0.11.0，v0.39.0 依赖 v0.31.0）
- [Task 5] depends on [Task 4] （P1-D 下半部分依赖上半部分）
- [Task 6] depends on [Task 1] （设备协议栈依赖存储驱动 v0.23.0 和 HAL v0.7.0）
- [Task 7] depends on [Task 6] （P1-F 下半部分依赖上半部分，P1-G 依赖 P1-F）
- [Task 8] depends on [Task 7] （RTOS 组件依赖设备协议和四遥）
- [Task 9] depends on [Task 1] （LLM 依赖文件系统模型加载 v0.24.0）
- [Task 10] depends on [Task 9] （Solver 依赖 LLM 意图解析 v0.63.0 和 v0.66.0）
- [Task 11] depends on [Task 8, Task 10] （双脑协同+MVP 依赖 RTOS 组件和 Solver）

## 并行执行策略

以下 Task 组可并行执行：
- Task 1（存储）与 Task 3（密码学）可并行（无相互依赖）
- Task 6（设备协议上半）与 Task 9（LLM）可并行（无相互依赖）
- Task 8（RTOS）与 Task 10（Solver）可并行（无相互依赖，都依赖 Task 7/9 分别完成）
