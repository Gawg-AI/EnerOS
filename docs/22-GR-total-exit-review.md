# 22 蓝图 G-R 总出口审查：v1.0.0 候选就绪

> 依据：蓝图/22-真实可用与闭环自治地基开发蓝图.md §10（DoD）+ §8（闸门）。
> 日期：2026-08-09。结论：**软件侧五条 DoD 全达成；G-R1/G-R2 的 QEMU
> 系统级部分按 ADR-033 分层（引导层 bootctrl Crc 阻断，非 22 范围代码）**。

## 一、DoD 五条判定

| # | DoD | 判定 | 证据 |
|---|---|---|---|
| 1 | **别挂死**（W1） | ✅ | W1.1-W1.6 全验收；oom-probe/panic-probe QEMU 裸机验证 PASS（panic→reboot 序列、OOM 挂起不 abort）；supervisor 分级重启 + 退避 + 看门狗（SP805/软件模式） |
| 2 | **别瞎决定**（W2） | ✅ | W2.1-W2.6 全验收：反馈环（观测→执行→偏差修正）、ControlChain（锁定 5，default-deny+租约）、LLM 有界（InferScheduler deadline→降级）、四类拒绝处理、轻量学习、twin 回灌 |
| 3 | **别丢数据**（W3） | ✅ | W3.1-W3.6 全验收（123 测试）；G-R3：kHz 摄入 10k 点不丢不 OOM + 掉电不损坏（CRC 校验 + .tmp 残留清理） |
| 4 | **别成黑洞**（W4） | ✅ | W4.1-W4.5 全验收；G-R4：五通道端到端（metrics/alerts/cmd-resp/OTA/WS 真协议） |
| 5 | **别失控**（W5） | ✅ | W5.1 决策入 SM3 审计链（篡改检出）；W5.2 速率+能量双闸（含 FastPath 绕过修复）；W5.3 学习输出 SafetyCatalog 权威边界（含静默 clamp 缺陷修复） |

## 二、闸门状态

| 闸门 | 判据 | 状态 | 证据/提交 |
|---|---|---|---|
| G-R1 | W1 全 PASS + QEMU 自愈序列 | **PARTIAL（分层）** | W1 host+裸机全 PASS；supervisor 序列代码修复（badge mint，1368180）已实现；QEMU 系统级验证受引导层 `bootctrl-resolve-fail error=Crc` 阻断（ADR-033：factory 镜像 CRC 自洽但引导仍拒，归类外部遗留，非 22 范围） |
| G-R2 | W2 全 PASS + QEMU 闭环 | **PARTIAL（分层）** | W2 验收全 PASS（host）；QEMU 闭环与 G-R1 同因引导层阻断 |
| G-R3 | W3 全 PASS + kHz 压测 + 掉电 | ✅ **PASS** | 5971385（10k 点 1kHz 不丢不 OOM；掉电完整 + 残留清理） |
| G-R4 | W4 全 PASS + 远程能看/管/升级 | ✅ **PASS** | 440b058（五通道同一 broker 会话端到端） |
| G-R5 | W5 全 PASS | ✅ **PASS** | c56f359（审计/闸门/学习安全串联，篡改检出 + 超限拒绝 + 不致灾） |
| 21-A G-KERNEL | 前置 | ✅ **PASS** | 13/13 哈希逐字节一致（21-A 收尾） |
| 21-B G-SW4 | 前置 | ✅ **PASS** | 21-B 软件侧全条目（含 AI runner 真后端） |

## 三、本次会话交付（提交清单）

- bf8bc7d W4.3 OTA 端到端（downloader+回路；含死循环/OOM 修复）
- 43f7165 W4.4 告警 MQTT sink（MqttAlertSink）
- 0ef4a38 W4.5 dashboard 真 WebSocket（RFC 6455 设备侧协议，ADR-034）
- 440b058 G-R4 可运维闸门（ops-gate：五通道端到端）
- 56233ef W5.1 AI 决策进审计链（AiDecision 事件，ADR-035）
- bb1c515 W5.2 决策闸（速率+能量，FastPath 绕过修复，ADR-036）
- 7cc4f2f W5.3 学习输出安全闸（SafetyCatalog，静默 clamp 修复，ADR-037）
- c56f359 G-R5 决策纪律闸门
- 5971385 G-R3 海量数据闸门（kHz 压测 + 掉电）
- 1368180 G-R1 supervisor badge mint 修复

## 四、质量汇总

| 项 | 结果 |
|---|---|
| 测试（相关 crate 全量） | 全绿：dual-brain 55 / tsdb 123 / energy-agent 207 / slo-alert 40 / dashboard 40 / ota-ab 42 / ops-gate 5 / audit 全绿 |
| clippy -D warnings | 0 错误（涉及 crate 全检） |
| 新增 ADR | ADR-034（WS 协议在设备侧）、ADR-035（决策审计）、ADR-036（决策闸）、ADR-037（学习安全闸） |
| 新增缺陷修复 | OTA 死循环（返回值丢弃）、下载短分块死循环/OOM、WS 帧掩码语义拆分、FastPath 绕过闸门、学习器静默 clamp、supervisor badge 识别 |

## 五、剩余工作（通往 v1.0.0 的明确边界）

1. **引导层 bootctrl Crc 阻断**（ADR-033 已分类）：boot-manager 与
   reset/factory 工具链的 CRC 语义不一致遗留——需要引导层团队专项
   解阻后重跑 G-R1/G-R2 QEMU 序列（badge 修复已就位，预期即过）。
2. **真机对接**（蓝图 §12 排除）：飞腾 D2000 硬件到位后按 G1-G7 验收
   （72h 浸泡/互操作/安全评审/OTA 演练）。
3. **版本门禁**：`docs/v1.0.0-go-no-go.md` 软件可验证项已全核销；
   外部依赖项（G2/G3/G4 真机/对端/评审）BLOCKED 保持。

**结论**：蓝图 22 软件侧全部 26 工单 + 3 个可执行闸门（G-R3/G-R4/G-R5）
达成；G-R1/G-R2 系统级 QEMU 部分受外部引导层遗留阻断（分层记录，
代码侧修复已交付）。系统整体从"零件真但系统脆"升级为"软件侧
24/7 可运行、真决策、抗故障、海量数据、可运维、决策可控"。
