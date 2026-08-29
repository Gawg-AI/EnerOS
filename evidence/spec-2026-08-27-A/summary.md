# SPEC-SERIES-2026-08-27/A · 卷执行摘要

- 代码基线 b5d939b（0.314.14）→ 本卷工作树（版本递增至 0.314.15，lock 已同步）。
- 全量测试最终计数见卷尾执行状态表；触达包 clippy --all-targets 零警告；fmt 已应用。
- 目标侧构建：`xtask build all` ×8 profiles PASS（需 CI 同款三环境变量 ENEROS_RELEASE_PUBLIC_KEY_HEX/KEY_ID/PLATFORM_ID）；真实 no_std 编译闸 = `xtask image raw qemu-test`（build all 不编 qemu 变体 bins——据此抓出两处区域重写笔误并修复）。
- QEMU 证据（R9 全部 target 侧串口/broker 行）：
  - 正例：MASTER-WRITE=171 / Committed=171 / TSDB-COMMIT=175 / PRODUCT-READY=1 / CHAIN-KEY-MISSING=0 / SAFETY-REJECTED=0 / PANIC=0
  - 负例（缺键节 bundle）：CHAIN-KEY-MISSING=2 / Committed=0 / PANIC=0
- 环境受限登记（owner 可解锁后补证）：A.2-C3 越窗拒 target 侧——本环境无遥测注入面，宿主侧 8 例覆盖全部拒路径，SAFETY-REJECTED 串口行接线已在 image 内编译实证。
- 关键坑：svc-config registry 双向完备性（security 条目无 consumer → unconsumed-config 整包拒载）；qemu-dev/qemu-test profile 清单缺 ai-peer/dlt645 属预存 CF-01 红（历史 flows 均用 qemu-prod 清单编 bundle）；mqtt net 通道 virtio-net-init-failed 属预存环境事实（w59 基线同款），正例写链由能量 Agent 自主 5kW 提案驱动（EXECUTE 中继面）。
- 未提交（67 文件改动 + 新文件在案）；提交与否待 owner 意向。

## 复核轮（2026-08-27，同日）

复审全部 diff hunk + 机械卫生检查（bash -n 三脚本 OK / fmt 触达包零漂 / workspace-lock 版本同步 / 生产路径无新增 unwrap），发现并修复：

| # | 发现 | 处置 |
|---|------|------|
| 1 | svc-control-dispatch `new()` 残留无用 `let profile` 与 `prod_chain` 导入（target-only 编译 2 警告——host clippy 看不见 target_os=none 体） | 删除；重建镜像确认 bin 零警告 |
| 2 | engine.rs 插桩时把 execute_core 的合核 doc 注释块错误接到新 helper 文档上 | 两 helper 迁至合核注释之前，doc 归属复原 |
| 3 | run-a-negative 脚本失败退出路径跳过 registry/bundle 还原（曾致 working tree 泄漏已摘除态） | 手工还原 + 增加 EXIT trap 兜底 |
| 4 | business-services 夹具双重循环体残留（编辑竞争） | 合并为单循环 |

修复后**终态复证**（image sha256=7a378ac8…，positive+negative 双窗依序重跑）：
- gate-final：`MASTER-WRITE=1 / Committed=1 / TSDB-COMMIT=3 / PRODUCT-READY=1 / CHAIN-KEY-MISSING=0 / SAFETY-REJECTED=0 / REGISTER-REJECTED=0 / PANIC=0`
- neg-final：`CHAIN-KEY-MISSING=2 / Committed=0 / PANIC=0`（virtual-pcs registered=1 = 设备注册与信封拒载解耦的预期形态）
- 终局测试全量 7704 passed/0 failed；脚本 bash -n 全过。

## 内存姿态（R2）

QEMU 窗口单实例顺序执行（正例→负例各一 TCG 实例，峰值 RAM≈1.2GB）；host 测试套件常规内存量级；未做并行压力。

## 环境事实登记（沿用既有卷口径）

- mqtt net 通道 `virtio-net-init-failed` 为预存基线现象（w59 full 串口同款）——SCADA-broker 下行写路径 target 不可用；控制写由能量 Agent 自主提案驱动。
- qemu-dev / qemu-test profile 清单对 ACTIVE registry 的 CF-01 红（缺 ai-peer / dlt645 等）为预存事实——bundle 编译标准流程走 qemu-prod 清单。
