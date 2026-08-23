# EnerOS 用户手册（v1.0 候选版）

> 蓝图 50（WP-50.4）手册四件套之一。适用版本：0.313.x（v1.0 候选冻结前）。
> 与代码不一致处以代码为准（50 卷 Exit 抽查 ≥10 条命令可执行）。

## 1. 产品能力边界（宣称纪律，D1–D4 裁决）

本手册所述能力以 44-00 总纲 D1–D4 owner 裁决为准；超出下表的能力**不宣称**：

| 能力 | v1.0 宣称 | 边界说明 |
|---|---|---|
| 工业协议采集/控制 | ✅ | Modbus RTU/TCP、IEC 104、MMS/GOOSE/SV、MQTT、DL/T 645、IEC 101/103、GB/T 27930（后四者为 48 卷接入面；规约原文核订前配置值须按现场规约复核） |
| AI 辅助决策 | ✅（需站端服务） | System2 供给 = **站端 LLM peer**（ADR-0045 方案 B）；设备端**无本地大模型**（49 卷 spike NO-GO）；peer 不可达自动降级本地启发式，不中断控制链 |
| 商用密码 | ❌ 不宣称 | 产品密码面 = Ed25519/SHA-256/rustls(P-256)；国密 SM2/3/4 为实验性能力（D4），不进产品宣称、不进送检 |
| 硬件 | QEMU 参考平台 | 飞腾 D2000 等真机 bring-up 未完成（51 卷 BLOCKED 登记）；真机证据为零（D3） |
| 遥测出口 | ✅ | MQTT 单通道（v1.0 单选）；固定二进制 OBSF 帧（§4） |

## 2. 部署

### 2.1 参考部署（QEMU）

```bash
# 构建产品镜像（qemu-prod profile；需显式签章键——无键 fail-closed）
export ENEROS_IMAGE_SIGNING_KEY=/path/to/signing.key
cargo xtask image qemu-prod
# 运行（外部 broker 在 host 127.0.0.1:1883）
cargo xtask run qemu-prod --image target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  --vars-template <template.fd> --expect PASS --evidence-dir <dir> \
  --net-mode present --net-user --net-protocols present --reset-bootctrl \
  --ready-marker product-ready
```

### 2.2 产线部署

整盘写入 + 设备身份 + 制造记录由产线工具完成——见《制造手册》
（`docs/manuals/manufacturing-manual.md`）。

## 3. 点表配置方法

点表事实源为 `configs/points.toml`（编译进签名 config bundle，经 svc-config
分发到各协议服务；45 卷起 release 装配**零合成遥测**——所有点位读数来自
真实协议采集）。

- 点位四元组：`device_id` / `point_id` / 协议地址 / 工程量变换（斜率+偏移）。
- 控制点位须显式声明（47 卷 CONTROL-BINDING；写链授权按点表绑定校验）。
- 修改点表后必须重新执行 `cargo xtask config compile --profile qemu-prod …`
  并重建镜像（bundle 签名随点表 digest 变化；制造记录含 points_sha256）。

## 4. 遥测观测（MQTT exporter）

| 主题 | 方向 | 载荷 |
|---|---|---|
| `eneros/<client-id>/telemetry` | 设备 → 站端 | OBSF 固定二进制帧（下） |
| `eneros/control` | 站端 → 设备 | `SET <device> <point> <value>`（经授权链） |
| `eneros/brain/request` / `response` | 双向 | AI peer JSON（QoS1） |
| `eneros/mgmt/request` / `response` | 双向 | MGT1 管理帧（见《故障处置手册》§3） |

OBSF 帧布局（schema v1，向后兼容规则见 `agent-sdk::telemetry_relay`）：

```text
偏移 0   magic "OBSF"(4B)
偏移 4   schema 版本 u8（当前 =1；未知版本 → 订阅端必须拒绝解析）
偏移 5   batch_seq u64 LE（帧序号，连续 = 无丢失）
偏移 13  count u16 LE
其后     count × 条目（class u8 ‖ service_len ‖ service ‖ schema_len ‖
         schema ‖ created_ms u64 ‖ boot_id u64 ‖ config_gen u64 ‖
         payload_len u32 ‖ payload）
```

断网行为：broker 不可达期间设备侧 durable outbox 兜底累积（产品缺省补传
窗口 30min，配置承载）；恢复后按帧序连续补传，**不丢不重**（幂等键稳定）。

## 5. 告警含义

| 告警/串口行 | 含义 | 处置 |
|---|---|---|
| `PRODUCT-READY: true` | 全部必需服务就绪 | — |
| `PRODUCT-READY: false (degraded: …)` | 列出的服务降级 | 见《故障处置手册》决策树 |
| `HEARTBEAT-KILL` | 服务无心跳被 supervisor 强制重启 | 观察重启后 readiness；频发→抓诊断包 |
| `peer-unhealthy` / `peer-recovered` | AI 站端 peer 断链/恢复（System1↔System2 切换） | 检查站端 LLM 服务与 broker |
| `TEL-FRAME-REJECT broker 断连` | 遥测出口断连（数据进 outbox 待补传） | 检查 broker/网络；恢复后自动补传 |
| `INTENT-FWD rejected` | 控制写意图被授权链拒绝 | 核对点表绑定与功率预算 |

## 6. 支持与反馈

见《故障处置手册》§5 支持联系方式模板。
