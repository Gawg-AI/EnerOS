# 集成 cnpower/pandapower 到仓库 + Phase 5 基础设施适配器 Spec

## Why
cnpower 和 pandapower 目前作为外部 Python 依赖存在，未纳入 EnerOS 仓库管理，导致新开发者无法一键获取完整开发环境。同时 Phase 1-4.5 已完成，需要推进 Phase 5 基础设施适配器（IEC 61850/104/MQTT 生产级实现）。

## What Changes
- 将 cnpower 作为 git submodule 集成到 `third_party/cnpower`
- 将 pandapower 作为 git submodule 集成到 `third_party/pandapower`
- 更新 .gitignore 添加 Python 相关忽略规则
- 更新 requirements.txt 引用本地 third_party 路径
- Phase 5: IEC 61850 生产级适配器实现
- Phase 5: IEC 104 生产级适配器实现
- Phase 5: MQTT 生产级适配器实现

## Impact
- Affected infra: git submodules, .gitignore, requirements.txt
- Affected code: eneros-device (IEC 61850/104/MQTT 适配器), eneros-bridge (路径引用)

## ADDED Requirements

### Requirement: Git Submodule 集成
系统 SHALL 将 cnpower 和 pandapower 作为 git submodule 集成到 `third_party/` 目录，确保克隆 EnerOS 仓库时自动获取这两个依赖。

#### Scenario: 克隆仓库后获取完整依赖
- **WHEN** 执行 `git clone --recurse-submodules` 克隆 EnerOS
- **THEN** `third_party/cnpower` 和 `third_party/pandapower` 目录包含完整源码

### Requirement: IEC 61850 生产级适配器
系统 SHALL 提供生产级 IEC 61850 协议适配器，支持 MMS 连接、数据集订阅、报告控制块、GOOSE 发布。

#### Scenario: 连接 IEC 61850 服务器
- **WHEN** 调用 `Iec61850Adapter::connect(addr)`
- **THEN** 建立 MMS 关联，可读写数据模型节点

### Requirement: IEC 104 生产级适配器
系统 SHALL 提供生产级 IEC 104 协议适配器，支持 ASDU 收发、总召唤、时钟同步、遥测遥信。

#### Scenario: 接收遥测数据
- **WHEN** IEC 104 主站发送 M_ME_NC_1 ASDU
- **THEN** 适配器解析为 MeasurementEvent 并发布到 EventBus

### Requirement: MQTT 生产级适配器
系统 SHALL 提供生产级 MQTT 适配器，支持 QoS 1/2、遗嘱消息、主题过滤。

#### Scenario: 订阅遥测主题
- **WHEN** 调用 `MqttAdapter::subscribe("grid/+/measurement")`
- **THEN** 收到匹配主题的消息后触发 EventHandler

## MODIFIED Requirements

### Requirement: Bridge 路径引用更新
bridge_http_server.py 和 requirements.txt 更新为引用 `third_party/cnpower` 本地路径。
