# 蓝图 68 · 卷 68.5（v0.325.0）出口门 VOL68.5-GATE 证据汇总

- 日期：2026-09-06 ｜ 镜像 sha256：`1ffaea42…8563d5f02fd4`（512K heap-large
  + 200ms mqtt recv + RST 同步清理）
- 判据口径：执行卡 WP-68.5.3 词形链原文「REMOTE-BRAIN link_err→
  **LEASE-ARMED**→System1 兜底决策仍 Committed」；unhealthy/recovered
  迁移为完整降级演示项（host t4/t5 已锁语义，实弹窗时序脆弱——见下）。

## 窗 A · planner 在局（fresh 盘 + broker + planner-peer；evidence planner-live.*）

| 判据 | 结果 |
|---|---|
| PRODUCT-READY / PANIC 净值（豁免 dispatch 签名 1）/ READINESS | true / 0（全量 1）/ 29 |
| 68.3/68.4 不回归：TSDB-DISK-COMMIT=22 / EVENT append 22 批 / FACT-FACE-OPEN / SNAP-BUILT=2 / SourceEpochMismatch=0 | 全 ✓ |
| broker transcript CONNECT+SUBSCRIBE（guest 在场） | ✓ |
| planner-peer PEER-REQ=5 / **PLANNER-SOLVE status=optimal=5（真 HiGHS，L2 门）** / PEER-RESP seq 持久 | ✓ |
| 串口 PEER-MSG verdict=Accepted → **REMOTE-BRAIN used=true** → resp=`[backend=planner] power=0kW…`（审计可区分）| ✓ |
| CONTROL-CHAIN dispatch=Committed / **agent=OK=512**（带 [347,578]）| ✓ |

## 窗 B · 降级链（fresh 盘；看护杀 planner-peer → System1 持签兜底 → 重启恢复）

采纳轮（call_55f4e464，死亡窗 200s）：LEASE-ARMED level=D3 ✓ / 降级期
used=false ✓ / **Committed 维持（agent=OK=511）** ✓ / 重启后
LEASE-REVOKED reason=peer-accepted ✓ / **used=true 回归（2 次）** ✓ /
LEASE-ARM-FAIL=0 ✓。event=peer-unhealthy/recovered = 0（信息项：
管道滞留应答清零 consecutive，3 次超时未在死亡窗内达成——迁移语义由
host t4/t5 锁定；卡面词形链不要求）。

## 窗 C · 无 peer 对照（现役形态；evidence no-peer.*）

全判据 ✓（agent=OK=512、Committed、**LEASE-ARMED=0**——healthy 常态
不武装，68.5 改动对无 peer 场景零行为变化）。

## 配套根因修复实弹（REG-MQTT-BROKER-HANG → Done）

1. 根因链（probe2 诊断词形 `[net] tcp-connect OK/TIMEOUT` +
   `MQTT-HS-ERR`）：第一次 TCP connect OK（start 期）→ 首次 poll 的
   reconnect（50 卷"一律重建"防御）裸 remove 不发段 → slirp 侧旧连接
   悬挂 → 重建新 SYN 同 4 元组被按重复丢弃 → SynSent 卡死 → svc-mqtt
   Degraded。无 broker 窗全绿之谜 = host 拒连 RST 快速清理（双态闭环）。
2. 修复：`TcpStreamHandle::reconnect`/`tcp_connect` 超时路径先
   `abort()`（RST 出环）再移除；`mqtt::real::NetTcpTransport` 同族防御
   （SynSent 残留 abort + 有界握手/接收窗）。
3. recv_timeout_ms 1000→200（50 卷参数在 broker 窗的次生适配：root 每
   轮同步 EXECUTE 等满超时 → 轮转 4.5s、agent=OK 82；200ms 恢复带内
   512——错误计数噪音 ~2-3×，断连判定不依赖）。
4. 实弹复验：MQTT-HS connected=true → PRODUCT-READY 无 degraded →
   PEER-MSG Accepted → used=true backend=planner（全链贯通）。

## 配套登记（candidates-registry E-68 追加）

- REG-MQTT-BROKER-HANG → **Done**（本卷附加裁决 6）。
- REG-TELEMETRY-HEAP-CREEP → Open（复用盘+broker 窗堆慢爬升 ~0.35KiB/s
  触顶 384K → PANIC；池 384→512KiB 买窗后 reuse1 窗零 PANIC；泄漏面归
  后续卷 host alloc 计数探针定位）。
- REG-REUSEDISK-TEL-COLDSTART → Open（复用盘恢复期 × 采集推送冷启动 →
  SNAPSHOT MissingPoints → agent 低产；fresh 盘同流量全绿；DEMO-68 复用
  盘段 BLOCKED 于本项）。

## 出口门状态

- 窗 A ✓ / 窗 B ✓（卡面词形链全咬合）/ 窗 C ✓ → **VOL68.5-GATE: PASS**。
- DEMO-68（00 §4 蓝图收尾总验收）：复用盘段 BLOCKED 于
  REG-REUSEDISK-TEL-COLDSTART——登记后由修复卷/复查会话执行。
- 串口大日志按现行 gitignore 政策本地归档（不入库），transcript 与
  planner-peer 日志入库。
