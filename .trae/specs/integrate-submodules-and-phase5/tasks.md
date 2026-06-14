# Tasks

## 层级 1: Git Submodule 集成

- [x] Task 1: 集成 cnpower/pandapower 为 git submodule
  - [x] 1.1: 创建 `third_party/` 目录
  - [x] 1.2: 添加 cnpower submodule: `git submodule add https://github.com/Gawg-AI/cnpower.git third_party/cnpower`
  - [x] 1.3: 添加 pandapower submodule: `git submodule add https://github.com/e2nIEE/pandapower.git third_party/pandapower`
  - [x] 1.4: 更新 `.gitignore` 添加 Python 缓存忽略规则（__pycache__, *.pyc, .venv/）
  - [x] 1.5: 更新 `crates/eneros-bridge/python/requirements.txt` 添加本地安装说明

## 层级 2: IEC 61850 生产级适配器

- [x] Task 2: IEC 61850 适配器核心实现
  - [x] 2.1: 在 `crates/eneros-device/src/adapters/` 创建 `iec61850.rs`
  - [x] 2.2: 定义 `Iec61850Config` 结构体（host, port, ied_name, dataset_ref）
  - [x] 2.3: 实现 `Iec61850Adapter` 结构体，实现 `ProtocolAdapter` trait
  - [x] 2.4: 实现 MMS 连接管理（connect/disconnect/reconnect）
  - [x] 2.5: 实现数据模型读写（read_node/write_node）
  - [x] 2.6: 实现报告控制块订阅（subscribe_reports）
  - [x] 2.7: 添加测试

## 层级 3: IEC 104 生产级适配器

- [x] Task 3: IEC 104 适配器核心实现
  - [x] 3.1: 在 `crates/eneros-device/src/adapters/` 创建 `iec104.rs`
  - [x] 3.2: 定义 `Iec104Config` 结构体（host, port, common_address, asdu_size）
  - [x] 3.3: 实现 `Iec104Adapter` 结构体，实现 `ProtocolAdapter` trait
  - [x] 3.4: 实现 ASDU 解析（M_ME_NC_1 遥测、M_SP_NA_1 遥信、C_SC_NA_1 遥控）
  - [x] 3.5: 实现总召唤（general_interrogation）和时钟同步
  - [x] 3.6: 实现事件驱动：ASDU → EventBus 事件发布
  - [x] 3.7: 添加测试

## 层级 4: MQTT 生产级适配器

- [x] Task 4: MQTT 适配器核心实现
  - [x] 4.1: 在 `crates/eneros-device/src/adapters/` 创建 `mqtt.rs`
  - [x] 4.2: 定义 `MqttConfig` 结构体（broker_url, client_id, qos, will_topic）
  - [x] 4.3: 实现 `MqttAdapter` 结构体，实现 `ProtocolAdapter` trait
  - [x] 4.4: 实现 QoS 1/2 发布和订阅
  - [x] 4.5: 实现遗嘱消息和主题过滤（通配符 +/#）
  - [x] 4.6: 实现消息回调 → EventBus 事件发布
  - [x] 4.7: 添加测试

## 层级 5: 验证

- [x] Task 5: 全局验证
  - [x] 5.1: cargo test --workspace 全部通过
  - [x] 5.2: cargo clippy --workspace 无错误
  - [x] 5.3: 更新 DEVGUIDE.md Phase 5 完成度

# Task Dependencies
- [Task 2, 3, 4] 可并行执行
- [Task 5] depends on [Task 1, 2, 3, 4]
