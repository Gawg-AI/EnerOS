# Python 环境规范化 + Bridge 端到端验证 Spec

## Why
cnpower 和 pandapower 已安装但 EnerOS 项目内缺少 Python 依赖管理文件，aiohttp 未声明，桥接脚本路径硬编码依赖工作目录，且从未实际运行端到端验证。需要规范化 Python 环境、添加依赖管理、修复路径问题，并验证 Bridge 端到端可用。

## What Changes
- 添加 Python 依赖管理文件（requirements.txt）
- 修复桥接脚本路径查找逻辑（从相对路径改为基于 crate 目录的绝对路径）
- 添加 Python 环境初始化脚本
- 端到端验证：Rust → HTTP Bridge → cnpower → pandapower → 结果回传
- 修复 bridge_http_server.py 中 aiohttp 缺失问题

## Impact
- Affected code: eneros-bridge (python_bridge.rs, bridge_client.rs, bridge_http_server.py, 新增 requirements.txt)
- Affected infra: Python 环境配置

## ADDED Requirements

### Requirement: Python 依赖管理
系统 SHALL 提供 `requirements.txt` 文件声明所有 Python 依赖（cnpower, pandapower, aiohttp），并支持一键安装。

#### Scenario: 新开发者安装 Python 依赖
- **WHEN** 运行 `pip install -r crates/eneros-bridge/python/requirements.txt`
- **THEN** 所有 Python 依赖（cnpower, pandapower, aiohttp）被安装

### Requirement: 桥接脚本路径解析
系统 SHALL 从 crate 目录（CARGO_MANIFEST_DIR）解析桥接脚本路径，而非依赖当前工作目录。

#### Scenario: 从任意工作目录启动 Bridge
- **WHEN** 从项目根目录以外的位置运行 EnerOS
- **THEN** BridgeClient 仍能找到并启动 bridge_http_server.py

### Requirement: Bridge 端到端验证
系统 SHALL 提供端到端集成测试，验证 Rust → HTTP Bridge → cnpower → pandapower 完整链路。

#### Scenario: 通过 Bridge 获取 cnpower 设备数据
- **WHEN** 调用 BridgeClient::start() + call("list_transformers", {})
- **THEN** 返回非空变压器列表，包含 sn_kva/vn_hv_kv 等字段

#### Scenario: 通过 Bridge 运行 pandapower 潮流
- **WHEN** 调用 call("run_powerflow", {assets: ...})
- **THEN** 返回 PandapowerResult，converged=true，buses 包含 vm_pu/va_degree

## MODIFIED Requirements

### Requirement: BridgeClient 路径解析
BridgeClient 和 PythonBridge 的脚本查找逻辑改为基于 CARGO_MANIFEST_DIR 环境变量，编译时注入 crate 目录路径。
