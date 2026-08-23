# EnerOS 故障处置手册（v1.0 候选版）

> 蓝图 50（WP-50.4）手册四件套之一。排障依据 = 50.1 结构化诊断出口
> （MQTT OBSF 遥测帧 + 串口结构化行）+ 审计入链（DecisionAudit/管理审计）。

## 1. 排障决策树

```text
设备无输出？
├─ 无串口输出 → 供电/引导链问题 → 《升级回滚手册》§4 BOOTCTRL 检查
├─ 有串口、卡在 spawn
│   ├─ 某 svc 行后停 → 该服务 fail-closed（查对应服务串口行）
│   └─ NoCapSlot / DT_LAYOUT → 内核资源窗口问题（收集串口报支持）
└─ PRODUCT-READY: false (degraded: X,Y)
    ├─ degraded 含 svc-mqtt
    │   ├─ NET-TRANSPORT READY 缺失 → 网卡句柄未移交（重启）
    │   ├─ protocol config unavailable → config bundle 缺 protocol-mqtt
    │   │   （重跑 cargo xtask config compile 并重建镜像）
    │   └─ TEL-FRAME-REJECT broker 断连 → broker/网络不通：
    │       数据自动进 durable outbox，恢复后按帧序补传（不丢）；
    │       恢复判据 = EXPORT-FRAME frame_seq 继续递增
    ├─ degraded 含 svc-ai-worker
    │   └─ REMOTE-BRAIN peer-unhealthy → 站端 LLM peer 断链；
    │       控制链不受影响（System1 启发式接管），恢复后自动回 AI
    └─ degraded 含协议服务 → 核对该协议对端（从站/broker）在线与点表配置
```

## 2. 关键计数与审计链

- **遥测队列**（OBS1 STATS）：enqueued / dropped / exported /
  export_failed——`export_failed` 持续增长 = 出口持续断连。
- **控制链**：`CONTROL-CHAIN … dispatch=Committed` 连续 = 写链健康；
  `Rejected` = 授权拒绝（核对点表绑定/功率预算/限流 rate gate）。
- **审计入链**：`AUDIT-CHAIN appended=N dropped=0`——dropped>0 即异常，
  立即收集诊断包。
- **管理审计**：每条 MGT1 命令在 svc-mgmt 审计留痕
  （principal/action/result/denied）。

## 3. 只读诊断包触发（管理通道）

外部运维客户端经 MQTT 发布 MGT1 帧（固定二进制布局：

```text
"MGT1"(4) ‖ cmd(1) ‖ principal(16) ‖ roles(4) ‖ tenant u32 LE ‖
channel(16) ‖ expiry u64 LE ‖ request_id u64 LE ‖ action_len u8 ‖
action ‖ arg
```

roles 字段 = mgmt-api 角色码（u32 LE：viewer=0 / operator=1 / admin=2 /
superuser=3；余位补零）。

白名单动作与角色要求（越权 → Failed + 审计 denied）：

| action | 角色 | 说明 |
|---|---|---|
| `status` | viewer | 服务状态查询 |
| `log-query` | viewer | 日志级别查询 |
| `config-get` | operator | 配置只读查询（arg=config_id） |
| `support-bundle` | operator | 只读诊断包触发 |
| `agent-restart` | admin | 受控重启请求（经 root supervisor 执行） |

参考实现：`evidence/50/ops_mgmt_client.py`（独立 python，零 EnerOS 依赖）。
应答帧 = `[status 1B][detail]`（status: 0=Failed, 1=Completed, 2=Accepted），
发布回 `eneros/mgmt/response`。

## 4. 恢复模式（47.6 探针集）

恢复探针为 qemu-test 装配的编排动作集（storage persist 往返、OTA trial
状态落盘/读回、hang 注入心跳隔离）。现场处置原则：

1. 先以只读动作取证（status / support-bundle）；
2. 单服务异常 → admin 发起 `agent-restart`（受控重启，supervisor 执行）;
3. 引导层损坏 → 按《制造手册》重新 provision（Genesis 归零）。

## 5. 支持联系方式模板

```text
[支持请求模板]
设备序列号：ENP-YYYY-NNNNNN        （见制造记录）
固件镜像 sha256：<record.image_sha256 前 16 位>
点表 digest：<record.points_sha256 前 16 位>
现象描述：<一句话>
关键串口行（≤20 行）：<PRODUCT-READY / 异常行>
诊断包编号：<support-bundle 应答 detail>
发生时间与频率：<…>
```
