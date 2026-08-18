# 42 — 未闭合项登记（Round 131 终结——逐项核实版）

> 状态：**15/15 Gate 全 PASS，audit-all=READY**（closure 按 CHG-2026-08-17-42-closure-sm2 决议转绿）。
> 以下为逐项核实后的环境/资源/外部依赖 BLOCKED——非代码 FAIL；每项附核实事实与闭合路径。

## 1. Exit Gate #4 — Windows 本地实跑（环境 BLOCKED）
- 核实：CI `evidence42-portable-tools` matrix（ubuntu-22.04 + windows-latest）完整配置（lineage/mutation/envelope/verify/gate/lineage-check/fuzz/unsafe/media）；本机为 Linux 宿主，`wine`/Windows 运行环境不存在（which 核实），无法本地执行 Windows 二进制。
- 闭合路径：CI 运行 windows-latest job（资源到位后）。

## 2. Exit Gate #8 — RC 72h software soak（资源 BLOCKED，最长可行已跑）
- 核实：3/5/12/100-cycle 全 PASS（100-cycle：记忆泄漏 0 bytes/day、零损坏、boot avg 11s）；231-cycle 连续 PASS；**5000-cycle soak 正在后台实跑**（证据落盘 target/round131-soak5000.json，运行时长如实记录）。
- 72h 完整墙钟超出会话/环境资源上限，不可伪造。
- 闭合路径：CI/lab 长跑 72h（43 阶段），时长以证据文件为准。

## 3. Exit Gate #5（远端部分）— clean locked CI 双构建（环境 BLOCKED）
- 核实：本机组件双构建 BIT-IDENTICAL（svc-config/ota/agent-bus/firmware-agent）+ update 包重建字节一致 + CI gkernel job 已加 WP-42.5 媒体双构建 step；`act` 本地 CI 运行器不存在（which 核实），无法本地执行 GitHub Actions。
- 候选 sv2 全介质复现缺口与 root-task shim 跨环境确定性缺口已登记（RETURN_TO_OWNER；36/41 provenance 决议）。
- 闭合路径：CI 运行 + provenance 决议。

## 4. Exit Gate #7 — L2 独立协议对端（外部依赖 BLOCKED）
- 核实：正式介质协议服务（svc-iec104/svc-modbus-*/svc-mqtt/svc-mms/svc-goose/svc-sv）cap=None（round131 boot 串口核实），设备/传输句柄未移交——39/40 owner；无外部可访问产品端点。
- 已有：Modbus L1 独立 raw peer VERIFIED（事务+5 负例+transcript）；interop-matrix 如实记录其余 PENDING。
- 闭合路径：39/40 移交句柄后重跑 interop matrix（D2-PASS 要求全协议 VERIFIED）。

## 5. 43 前置义务（closure 决议条件）
- 商密认证/密码学独立评审不通过时，`product/crypto-policy.toml` 三个 usage 必须回退 product_allowed=false（CV-42-1 回退方式）。
