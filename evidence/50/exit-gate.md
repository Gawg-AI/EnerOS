# 蓝图 50 卷 Exit Gate（可观测运维出口与交付工具）

> 日期：2026-08-23 · 版本：0.313.40（见 §5 bump 链）· 状态板随本 gate 置 Accepted

## Gate 自证

### 1. MQTT exporter 补传/断网/schema 负例 + mgmt 白名单/RBAC/审计

| 项 | 结果 | 证据 |
|---|---|---|
| exporter 真通道（v1.0 单选 MQTT） | ✅ QEMU 实弹 | svc-observability →（OBSF 帧分片 EXECUTE）→ svc-mqtt → QoS1 → broker；独立 python broker 订阅观测 TELFRAME 帧序列 frame_seq 连续（telemetry.transcript.jsonl） |
| 固定二进制帧 + schema 版本化 | ✅ host+QEMU | OBSF v1（agent-sdk::telemetry_relay 单一事实源）；负例 `negative_schema_version_mismatch_rejected`（version 0/2 拒绝解析）+ svc-mqtt 边界 check_frame_header fail-closed |
| 断网 durable outbox 兜底 | ✅ QEMU+host | broker kill 秒级停帧（transcript 时序对齐编排相位）；恢复后补传；host 确定性测试 `补传_exports_each_item_exactly_once_after_repeated_failures`（幂等键稳定——不丢不重） |
| **实弹修复×6**（红→绿或实弹根因） | ✅ | ①补传重复导出缺陷（稳定幂等键）；②OBS outbox 4 槽 < batch8（→16）；③半开 TCP 断连不可检（slirp 不传播 FIN → keepalive PING 无应答 mark_disconnected，标准 MQTT 语义）；④connect 一律重建 TCP（信任 is_connected 会把 CONNECT 写进死链）；⑤MQTT 定长头逐帧解码（TCP 合并段丢包根因）；⑥config bundle 陈旧产物流程登记 |
| mgmt 白名单动作 ×5 真实 executor | ✅ QEMU 往返 | status/config-get/log-query/support-bundle/agent-restart 经 MQTT MGT1 全链（svc-mqtt → EIPC → svc-mgmt → 应答回发布）；`MGMT-FWD` 串口行 + ops 客户端 MGMT-RESP 在案 |
| 每动作独立 RBAC + 越权负例 + 审计 | ✅ host+QEMU | role_for() viewer/operator/admin 映射 + host 越权负例×3；QEMU `restart-viewer-denied → insufficient-role:admin`（Failed + 审计 denied）在案 |
| 受控重启经 root supervisor | ✅（seam 登记） | admin restart → Accepted `restart-request-logged`（root supervisor 心跳/故障路径执行；service→root 重启用 RPC seam 登记 v1.x——诚实边界） |

### 2. factory provision 全流程证据 + 制造记录 schema

| 项 | 结果 | 证据 |
|---|---|---|
| 缺密钥 fail-closed | ✅ | TEST A：无键 → 非零退出「产线签章键未提供…拒绝」 |
| provision 正例 | ✅ | TEST B：Genesis=true、readback=verified、FACTORY-PROVISION PASS |
| 制造记录 schema + 签章 + 对账 | ✅ | TEST C verify-record schema=OK signature=VALID；TEST D 篡改 → signature INVALID / digest mismatch |
| 空盘 → provision → 启动冒烟 | ✅ | provisioned 盘原样引导（无 reset-bootctrl）→ PRODUCT-READY: true（factory/provision-smoke.md） |

### 3. 四手册演练记录（≥10 条命令可执行抽查）

四手册落库 docs/manuals/；演练映射见 evidence/50/WP-50.4.md。抽查命令
（≥10 条全部在本卷实弹执行过）：image qemu-prod / run qemu-prod / config
compile / factory provision / factory verify-record / telemetry OBSF 帧布局
对照 agent-sdk 常量 / MGT1 帧布局对照 svc-mgmt 解析偏移 / OTA1 0x01–0x07
对照 svc-ota 协议注释 / qa gate config-refs / closure gate / qa gate keys /
bump-version。手册与代码不一致处已按代码修订（MGT1 cmd 字节初稿遗漏 →
修订手册）。

### 4. 全量 CI 绿 + 45–47 抽查

- cargo test --workspace --no-fail-fast：**全绿 0 失败**（ok 块数见
  exit-gate.json，含本卷新增 ≥25 测试）
- clippy --workspace 0 警告 / fmt --all 干净 / closure gate PASS（binaries=32
  reached=122 denied=0）/ CF01 OK active=18 / keys gate 4 项 PASS /
  CATALOG-VALIDATE PASS / policy adr PASS
- 45 抽查：①plugin.rs `_points`=0 ✓ ②closure denied=0 零合成遥测 ✓
- 46 抽查：①l2-*-verified.json ×6 VERIFIED 绑 4f7344c ✓ ②CATALOG-VALIDATE
  PASS ✓
- 47 抽查：①w40-prod-key grep=0 ✓ ②DURABLE-INTENT-STORE 实弹行在案
  （49 exit 串口 + 47 write 串口）✓

### 5. 版本 bump 链

0.313.35 →(50.1)→ 0.313.36 →(50.2)→ 0.313.37 →(50.3)→ 0.313.38 →(50.4)
→ 0.313.39 →(Exit)→ **0.313.40**；Cargo.lock 同步。

## 诚实登记

1. **QEMU 断网→恢复→补传「恢复段」可视化 = PARTIAL（演示窗限制）**：
   在线段（frame_seq 连续观测）与断网段（broker kill 秒级停帧）多次实弹；
   恢复段受三重演示窗叠加限制——harness Linux 路径硬性
   `timeout --signal=KILL 200s`、slirp 对 guest 半开 TCP（host 侧关闭不传播，
   smoltcp state 恒 ESTABLISHED）、断连轮询期墙钟放大。机制层由 host 确定性
   测试承载（cycle test + 补传 exactly-once），产品语义（30min 补传窗、自动
   重连）由配置与已测代码承载。非产品缺陷登记。
2. **二分实验残留**：定位过程中曾临时停用 OBS 分支/mgmt 授予/diag 特性
   并复跑对照（均不能消除冻结）——已全部恢复激活态；最终归档 run 于全部
   功能激活镜像上执行。
3. **[diag] UART 体积**：drv-svc 对 storage 的 diag 特性依赖保持原状
   （二分证明其非冻结根因）；UART 体积对 guest 时钟的拖慢为既有行为，
   优化登记后续卷。
4. **受控重启 service→root 重启用 RPC seam** 登记 v1.x（mgmt Accepted +
   root supervisor 执行的既有心跳/故障路径承载）。
5. **每设备身份密钥注入面**（产线）登记 owner/KMS 后续项；v1.0 信任锚
   经签名 config bundle 承载。
