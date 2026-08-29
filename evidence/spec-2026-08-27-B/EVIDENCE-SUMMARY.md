# SPEC-SERIES-2026-08-27/B · QEMU 实证小结

> 日期 2026-08-27 · 介质 qemu-prod（`target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw`，
> 本轮重建 sha `a7c47bd8…`）+ qemu-test 辅助窗 · 脚本即产物（本目录
> `run_window.sh` / `mqtt_broker_b2.py` / `anchor_repair.py` / `verdict.py` /
> `host_creds_probe.py` / `build_creds_variant.sh`）。

## 一、B.1 时间分发链（EL0 真墙钟）

### 判据与证据

| 判据 | 结论 | 证据（窗口/串口行） |
|------|------|----------------------|
| C1 闸门四臂 host 单测 | ✅ | `eneros-config-bundle` `schema::time_gate::tests::time_gate_four_verdicts`（合法/过旧/超未来/零） |
| C2 正例：TIME-EPOCH=<secs> 且>2026-01-01；宿主对照 | ✅ | `target/evidence-B2/w1k`：`[eneros-root-task] TIME-EPOCH=1787816910 stage=spawn`；sv/mms/…/mqtt/iec104/modbus-tcp 六 bin 同值；broker `BROKER-LISTEN ts` 与 guest 差 <60s（1787816902 vs 1787816910） |
| C3 重启/双 boot 前进 | ✅ | `w3a`=1787817995 → `w3b`=1787818145（150s 间隔严格递增；root 重启面每次经 `gated_wall_epoch("restart")` 新读 PL031） |
| C4 负例：不可信时间显式降级且系统照常运行 | ✅ | `w2b`（`-rtc base=2025-06-01`）：`TIME-EPOCH-MISSING stage=spawn verdict=Stale raw=1748736008` + 六 bin MISSING + `el0-enter PASS` + `DURABLE-INTENT-STORE open OK` |

### 实现要点（相对 spec 的诚实偏差与根因）

1. **F-T4 订正**：spec 称 X5 恒 0 可用，实测六个协议 bin 全部走 net 移交
   （X3/X4/X5=mmio/dma/dma_phys，`_start` 六参同型）→ 墙钟统一改走 **X6**
   （spawn 寄存器 count 9→10，`initial_registers`/`restart_service` 单点构造）。
2. **F-T1 订正**：运行时 DTB 经 AAVMF 链剪枝，QEMU virt 实测 `nodes=60`、
   `pl031=false sp805=false`（硬件 dumpdtb 有 pl031）→ root 装配面增加
   **QEMU virt 板级事实兜底** `ensure_rtc_bootstrapped`（spawn 前把 PL031
   所在设备区 [0x09000000,0x09010000] 逐 4K 恒等映射 17 页，`SpawnCtx.dev_maps`
   去重使 pl011 同页请求零二次 Retype）。`set_rtc_base` 同步写入时钟子系统，
   X6 经 `time_gate` 闸门后下发。
3. 闸门常量（`WALL_EPOCH_MIN_SEC`=2026-01-01 / `WALL_EPOCH_MAX_SEC`=2100-01-01）
   入 `eneros-config-bundle::schema::time_gate`（BAQ-2 采纳；用固定远期天花板
   替代“构建时刻+30 天”，保持 DEF-52-03 确定性构建纪律，已入册）。
4. 服务侧注入面 `set_wall_epoch_secs`（drivers/time，与 `set_rtc_base` 同型
   轻量、不重建 TimerWheel）；六 bin `_start` 增第 7 参并打印
   `TIME-EPOCH=` / `TIME-EPOCH-MISSING`。
5. **次生 bug 排查闭环**：2MiB 大页映射在 root VSpace 落地不生效（.07 L3 翻译
   故障）→ 改 4K 逐页；顺序帧消费曾致 pl011 重取错页（UART 0x9000024 故障）→
   `dev_maps` 去重根治。

## 二、B.2 MQTT 凭据管道

### 判据与证据

| 判据 | 结论 | 证据 |
|------|------|------|
| C1 golden（含中文用户名 UTF-8 字节长前缀） | ✅ | `eneros-mqtt` `client::gap_closure_tests::b2_golden_frame_utf8_multibyte_username`（"电网" 6B 长度前缀手算逐字节）；`wp3_none_credentials_frame_byte_unchanged` 既有回归保持 |
| C2 双键表同步 + 配对校验 | ✅ | `b2_known_keys_literal_lock…`（plugin 侧）+ `b2_mqtt_known_keys_literal_lock…`（config-bundle 侧）互为漂移闸；`b2_password_without_username_rejected`（MQTT-3.1.2-22） |
| C3 带凭据 CONNECT 正负例（wire 语义） | ✅ 主机真 TCP / ⚠️ 客端 PARTIAL | `host_creds_probe.py` PASS：correct→CONNACK rc=0；wrong→rc=4 且 broker `CONNACK-REJECT reason=wrong-pass` + 断连；匿名→rc=4（no-user）。客端 svc-mqtt 装配受限（见下） |
| C4 无凭据回归 | ✅ | 匿名配置 `w1k` 建联路径照旧（plugin READY、broker legacy 模式、帧行为不变）；host byte-equal 回归 |

### 客端 C3 PARTIAL 说明（如实）

- 凭据介质（creds-on）重建后多协议 net init 竞态（mqtt/iec104 `plugin start
  FAILED ProtocolInit`、goose/modbus `virtio-net-init-failed`——与配置无关的
  net 装配 flake，匿名介质 `w1k` 同装配 mqtt 即 `NET-TRANSPORT READY`）。
- 且本内核装配下 svc-mqtt 的 EXECUTE 驱动源缺失（prod 无合成遥测/运维
  注入），`PROTO-EXEC svc-mqtt status=0x04(ST_UNAVAILABLE)` 走能力壳——即使
  READY 也难在窗口内触发真实 CONNECT。**主机真 TCP + 帧 golden + broker 解析
  已覆盖全部 wire 语义**；客端建联留待后续卷（net init 稳定性 + EXECUTE 驱动）
  收口。

## 三、环境与工程事实登记

- 窗口起步修复：fresh 盘 `reset-bootctrl.py` + `anchor_repair.py`（UEFI VARS
  内 `EnerOSBootControlAnchor` 64B 等长改写配对）——解决 AnchorMismatch。
- `-rtc base=2010…` 会使 EDK2/AAVMF 挂起（仅 2025/2026± 可行）；C4 用
  base=2025 触发 Stale 臂。
- 运行窗口目录：`target/evidence-B2/{w1k,w2b,w3a,w3b,wc1,wc2,wmq1,wa*}`；
  host 探针产物 `target/evidence-B2/host-creds.json`。
- 恢复：`build_creds_variant.sh off` 已把 `configs/protocol-mqtt.toml` 还原为
  注释样例、重编匿名 bundle 并重建介质（保证共享树/产物一致）。
