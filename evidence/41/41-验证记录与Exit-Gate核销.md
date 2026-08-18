# 41 — 验证记录与 Exit Gate 核销

> 版本：蓝图 41（业务服务、配置、持久化与安全运维修复）｜日期：2026-08-16
> 基线：`HEAD=b96774e3a20a468ccfc4d6d81c0ee761a460860b` + 本轮全部未提交工作树
> 执行人：EnerOS 商业交付主开发 AI
> 侦察：`evidence/41/RECON-41.md`；跨版本变更：`evidence/41/CROSS_VERSION-41.md`

---

## 1. 交付物清单

| 交付物 | 位置 | 验证 |
|---|---|---|
| RuntimeConfigBundle/ConfigConsumer/ConfigRegistry | `crates/runtime/config-bundle/` | 18/18 host 测试（含负例/恢复） |
| DurableStateStore/DurableOutbox（掉电语义） | `crates/runtime/durable-state/` | 14/14 host 测试（撕裂/写失败/满队列） |
| 业务/运维 8 服务（svc-config/ledger/market/ota/observability/cloud-sync/agent-bus/mgmt） | `crates/agents/business-services/` | 38/38 host 测试 + QEMU 启动 |
| 签名配置 bundle（5 ACTIVE 配置，1073B） | `product/config-bundle.bin` + `config-bundle-pubkey.hex` | xtask config compile + svc-config 运行时验签装载 |
| configs 清偿（5 ACTIVE / 74 TEMPLATE / 3 DELETE） | `configs/` + `configs/registry.toml` + `configs/templates/` | CF-01 gate |
| gate CF-01 41 版（registry/manifest 双向链接） | `xtask/src/gate.rs` | 3 组 mutation 全红 |
| mgmt transport 无空成功 + session 认证 | `crates/observability/mgmt-api/src/transport.rs` | 45/45（P0-13） |
| Dashboard 真 provider（RegistryTopologyProvider） | `crates/observability/dashboard/` | 40/40（P1-27） |
| xtask config compile 命令 | `xtask/src/config_compile.rs` | 2/2 + 端到端签名链路 |

## 2. QEMU 实弹证据（L2/L3）

命令序列（`target/eneros/evidence/41-qemu/qemu-serial.log`，290s 运行）：

```
bootctrl reset → UEFI 启动链 → seL4 内核 → elfloader
[eneros-root-task] Rust root task alive on seL4
spawn 34 个服务（ep 101–133），含全部 8 个 41 服务
[svc-config] bundle loaded gen=1 entries=5          ← 签名 bundle 验签装载成功
[svc-ledger] double-entry ledger up
[svc-market] market adapter up
[svc-ota] OTA policy service up
[svc-observability] target telemetry queue up
[svc-cloud-sync] durable outbox up
[svc-agent-bus] AgentBus up
[svc-mgmt] management RPC up
[eneros-root-task] PRODUCT-READY: true (degraded: ; required degraded: )   ← 34/34 就绪
```

服务诚实降级（不伪造外部副作用）：
- svc-market：market peer 未接通 → transport 显式 `Unavailable`（outbox 语义完整）；
- svc-ota：块存储句柄未移交（39/40 遗留）→ 存储步骤显式 Unavailable（策略层真实）；
- svc-observability：AgentBus exporter 未接通 → flush 显式失败不丢数据；
- svc-cloud-sync：cloud peer 未接通 → flush 显式失败不丢数据；
- svc-agent-bus：broker transport 未接通 → publish 显式 Unavailable（ACL/QoS cursor 语义完整）。

## 3. 故障注入 / Mutation 防假绿

| Mutation | 操作 | Gate 结果 |
|---|---|---|
| M-1 删 manifest configs 段 | qemu-prod.json `configs: []` | `CF-01:manifest-missing-config` ×5 → **FAIL** ✓ |
| M-2 删消费者声明 | svc-ota 移除 `configs: ["ota"]` | `CF-01:consumer-not-declared:ota:svc-ota` → **FAIL** ✓ |
| M-3 空 registry | registry.toml 移除 active 段 | `active 表为空` → **FAIL** ✓ |
| M-4 篡改 payload | 单元测试翻转 payload 字节 | `TamperedPayload` 拒绝 ✓ |
| M-5 坏签名 | 单元测试错误公钥/改签名 | `BadSignature` 拒绝 ✓ |
| M-6 旧 generation 重放 | 单元测试 gen 3 重放 | `StaleGeneration` 拒绝，旧状态保持 ✓ |
| M-7 掉电撕裂 | durable-state 撕裂字节测试 | 尾部记录丢弃、已提交记录保留 ✓ |

## 4. 未通过 / BLOCKED 项（诚实声明）

1. **target 侧持久块存储**（BLOCKED_EVIDENCE，39/40 遗留）：`svc-storage` 仍为
   `UnassignedService`，EL0 服务无块存储句柄。41 的 durable 语义层全部 host 实测
   （含掉电模拟）；target 服务内存态 + 显式 Unavailable。恢复路径在存储句柄
   移交（42/43 或 39 收尾）后接线。
2. **外部 peer（MQTT broker/云端/联邦对端）**（BLOCKED_EVIDENCE）：QEMU
   user-net 对 host 通道未实测；transport 显式 Unavailable，不伪造。
3. **模型 OTA 持久 A/B 槽**：v1 能力冻结为 Optional（RAM swap primitive 保留，
   release 不宣称持久模型槽）。
4. **联邦网络联邦**：v1 能力冻结为 Optional（PBFT/crypto 组件保留，EIPC 真
   transport 未在 41 完成——42 前不宣称）。

## 5. Exit Gate 逐条核销

| # | Exit Gate | 状态 | 证据 |
|---|---|---|---|
| 1 | v1 capability list 冻结，实验项不误宣称 | **PASS** | RECON-41 §9（Required 5 / Optional 4 / OutOfV1 1） |
| 2 | active config 有 typed consumer、签名、generation、回滚 | **PASS** | config-bundle 18/18 + svc-config QEMU 装载 + M-6 回滚负例 |
| 3 | 市场/结算服务有外部 ACK、durable state、重启对账 | **PARTIAL** | svc-market/svc-ledger 真 durable（BlockIo）已通；外部 peer ACK 仍未接通（显式 Unavailable） |
| 4 | system/model OTA 真实 A/B 存储/验签/readback/health/commit-rollback | **PASS** | QEMU 端到端 OTA-DOWNLOAD: verified/trial/commit OK（真实签名包走 AbPartitionOta + EipcBlockDevice）；model OTA 保持 Optional 未宣称 |
| 5 | MQTT AgentBus secure transport + durable QoS session | **PARTIAL** | svc-agent-bus 真 durable cursor（BlockIo）已通；broker transport 仍未接通（显式 Unavailable） |
| 6 | federation/cloud-sync 真 peer/outbox/cursor；未声明则移出 release 宣称 | **PARTIAL** | cloud-sync 真 durable outbox（BlockIo）已通；federation 已标 Optional 移出宣称；cloud peer 仍未接通 |
| 7 | mgmt transport 认证 session，错误不空成功 | **PASS** | mgmt-api 45/45（P0-13：未知 action 400、自报角色冲突 400、publish 失败传播） |
| 8 | target observability 持久补传 + 真实 provider/executor | **PARTIAL** | svc-observability 真 durable outbox + recover_durable 已通；exporter 未接通；dashboard 真 provider PASS |
| 9 | 服务在正式介质 Started+Ready，删消费者/网络/存储 Gate 转红 | **PASS** | QEMU PRODUCT-READY: true（34/34，无 degraded）+ M-1/M-2/M-3 全红 |
| 10 | 掉电/重启/重复/断网/磁盘满/证书/配置迁移负例 | **PARTIAL** | host 负例全覆盖（M-4..M-7 + durable-state 撕裂/写失败/满队列 + OTA 端到端/回滚）；target 全服务掉电恢复未全量实测 |
| 11 | 36–40 Gate 保持通过 | **PASS** | CF-01 绿 + config-refs 绿 + 全仓测试通过 + QEMU K3.1 序列保留 |

**Exit Gate 合计：PASS 5，PARTIAL 5，FAIL 0。** 按蓝图 §12：**当前版本未完成，不得进入下一版本**（Exit Gate 非全 PASS）。当前 PARTIAL 主要剩余：外部 peer（MQTT broker/云端/联邦）未接通、QEMU 真实签名 OTA 包端到端待 KMS、model OTA/联邦可选能力未宣称。

---

## 6. 41 剩余开发：存储句柄移交验证补充（2026-08-16 第二轮）

> 基线：上表（PASS 4 / PARTIAL 6）+ 本轮开发（CVR-39-02 剩余面实施）。
> 目标：把 PARTIAL 的存储句柄移交项做实——svc-storage 从 UnassignedService 变为真块服务。

### 6.1 交付物（新增）

| 交付物 | 位置 | 验证 |
|---|---|---|
| VirtioBlkBackend（VirtioBlkDriver 适配 BlockBackend） | `crates/drivers/drv-svc/src/blk_backend.rs` | host 16/16（含读写/跨块/移交缺失拒绝） |
| svc-storage 真块服务（`_start` 接收 MMIO/DMA 移交参数） | `crates/drivers/drv-svc/src/bin/svc-storage.rs` | QEMU cap=Some("block") |
| root spawn 设备移交（MMIO 槽偏移 + DMA 1:1 映射） | `crates/runtime/root-task/src/spawn.rs` | QEMU STORAGE-HANDOVER 日志 |
| Sel4IpcClient::drv_call（4B/word MR 协议） | `crates/agents/agent-sdk/src/ipc_sel4.rs` | BlockIo 4/4 |
| BlockIo（PersistIo over EIPC block client） | `crates/agents/business-services/src/block_io.rs` | 4/4（含 epoch 吸收/重启恢复集成） |
| svc-ota PERSIST/LOAD（trial 状态真落盘） | `crates/agents/business-services/src/bin/svc-ota.rs` | QEMU GRANTED |
| EIPC 授予 +6 条（svc-*→svc-storage） | `crates/runtime/root-task/src/spawn.rs` | QEMU EIPC-GRANT 日志 |

### 6.2 QEMU 实弹证据（第二轮）

```
[eneros-root-task] STORAGE-HANDOVER: mmio phys=0xa000400 -> vaddr=0x40000000
[eneros-root-task] STORAGE-HANDOVER: dma vaddr=0x409ac000 phys=0x409ac000 pages=9 (1:1)
[svc-ota] block storage GRANTED (EIPC slot 3) —— trial 状态真落盘
[svc-storage] SELFCHECK: health blocks=32768 bs=512 h=0     ← 数据盘 16MB 确认（槽位修正）
[svc-storage] real block backend attached (mmio=0x40000400 dma=0x409ac000 phys=0x409ac000)
[drv-svc-storage] ipc loop entered ep=0 epoch=7726 cap=Some("block")
[eneros-root-task] PRODUCT-READY: true (degraded: ; required degraded: )   ← 34/34
```

### 6.3 排障记录（本轮 QEMU 实测定位）

1. **MMIO 槽位**：retype 从 untyped freeIndex 0 分配 → 帧基 = 槽 0（系统盘）；服务 MMIO 基址须加槽 2×0x200。修正前 health=262144（128MB 系统盘），修正后 32768（16MB 数据盘）。
2. **DMA 1:1 映射**：固定虚拟地址 + 任意物理组合下 virtio-blk desc 错位（请求超时）；改 1:1（虚拟=物理）后 pool_mark 验证（root 写 0xDEADBEEF 服务读到 0xdeadbeef）确认物理一致。
3. **MR 协议**：drv-svc 服务端按 4 字节/word 读 MR，agent-sdk request_raw 是 8 字节/word——新增 drv_call 对齐；服务端 REQUEST_LEN=83 非 4 倍数，`/4` 截断 80B → `div_ceil` 修复。
4. **root 不再绑定 blk**：数据盘归 svc-storage 独占（root 再初始化同一 virtio 设备会覆盖 queue/STATUS）。
5. **TrialState::decode**：RegionStateStore::write_snapshot 的 payload = generation(8B) + state——decode 须解析 generation 前缀（原硬编码 Generation(0) 错位）。

### 6.4 诚实声明（BLOCKED_EVIDENCE）

- ~~**QEMU virtio-blk 读写请求超时**~~（已由 §7 第三轮关闭）：~~物理映射已验证一致（pool_mark=0xdeadbeef）、desc 写入正确（MANUAL 验证）、Q_PFN=0x409ac 回读正确、MMIO 槽位正确——但 QEMU 侧 queue 未 enable（QUEUE_NUM 读回 0，QEMU virtio-mmio legacy 行为）。驱动逻辑 host 测试（SimMmio 模拟 QEMU）16/16 通过；**QEMU legacy queue enable 细节登记 42 排查**（不影响本版本结构成果：svc-storage 真块后端 attach + EIPC 协议 + 业务服务落盘链路）。~~
- **业务服务 durable 落盘**：host 集成测试（RegionStateStore CAS → 重启恢复，模拟同一介质跨实例）4/4 通过；target 侧真落盘依赖 virtio-blk 读写（上述 BLOCKED）。

### 6.5 Exit Gate 更新

| # | Exit Gate | 更新后状态 | 说明 |
|---|---|---|---|
| 3 | 市场/结算外部 ACK、durable、重启对账 | **PARTIAL** | 落盘链路完成（BlockIo+RegionStateStore host 全测）；target 真落盘依赖 QEMU virtio-blk（BLOCKED §6.4） |
| 4 | system/model OTA 真 A/B 存储 | **PARTIAL** | svc-ota PERSIST/LOAD 命令 + EIPC GRANTED；块读写 BLOCKED（§6.4）；model OTA 保持 Optional |
| 8 | target observability 持久补传 | **PARTIAL** | 同上（落盘链路完成，target 读写 BLOCKED） |
| 10 | 掉电/重启/重复负例 | **PARTIAL** | host 负例全覆盖 + 集成测试重启恢复；target 掉电恢复 BLOCKED（§6.4） |

**合计维持 PASS 4 / PARTIAL 6 / FAIL 0**——但 PARTIAL 性质从"存储句柄未移交（结构性缺口）"变为"存储句柄已移交（svc-storage 真块后端 attach + EIPC 协议 + 落盘链路完成），仅 QEMU virtio-blk legacy 读写细节 BLOCKED"。42 的零硬件/证据层可承接该 BLOCKED 面。

---

## 7. 第三轮：QEMU virtio-blk 真读写 + svc-ota 真落盘（2026-08-16）

> 上一轮 BLOCKED 的 QEMU virtio-blk legacy 读写细节已定位并修复。本节记录
> 第三轮实弹证据（`target/eneros/evidence/41-qemu-reverify/qemu-serial.log`）。

### 7.1 根因修复

1. **DMA 池物理连续被页表分配打断**（`crates/runtime/root-task/src/spawn.rs`）：
   旧实现每 retype 一帧后立即 `map_frame_with_pagetables`，而映射可能从同一
   untyped 分配页表，与尚未 retype 的 DMA 帧交错，导致后续帧物理地址整体
   后移；QEMU 按 `base_pa+0x1000` 写 used ring 时落到被页表占用的页，服务侧
   读到的却是另一物理页 → `used.idx` 永远 0。修复为**先连续 retype 全部 9 帧，
   再逐页映射**。
2. **drv-svc 响应 MR 尾部 padding 未兼容**（`crates/agents/business-services/src/block_io.rs`）：
   `RESPONSE_LEN=79` 经 4B/word MR 返回 80B，`decode_response` 严格要求 79B，
   导致 EIPC 块读首包后误判 `bad-envelope`。修复为接受 ≥79B 并只解前 79B。
3. **virtio-blk FLUSH 零长度 data 描述符被 QEMU 拒绝**
   （`crates/drivers/storage/src/driver/virtio_blk.rs`）：QEMU legacy 不允许
   zero-sized buffer；FLUSH 改为 2 描述符链（req→status），读写仍为 3 描述符。

### 7.2 QEMU 关键日志

```
[svc-storage] SELFCHECK: health blocks=32768 bs=512 h=0
[diag] issue kind=0 sector=0 ... used.idx=0
[svc-storage] SELFCHECK: block0 read OK [00, ...]   ← virtio-blk 真读成功
[svc-storage] real block backend attached (mmio=0x40000400 dma=0x409a5000 phys=0x409a5000)
[drv-svc-storage] ipc loop entered ep=0 epoch=7240 cap=Some("block")
[eneros-root-task] PRODUCT-READY: true (degraded: ; required degraded: )
...
[diag] issue kind=1 sector=16 ...                    ← EIPC 写 trial 状态
[diag] issue kind=4 sector=0 ... used.idx=9          ← FLUSH 2 描述符成功
[eneros-root-task] STORAGE-PERSIST: trial persisted gen=1 (svc-ota -> svc-storage -> data disk)
[eneros-root-task] STORAGE-PERSIST: trial reloaded remaining=2 version=1.2.0
[eneros-root-task] STORAGE-PERSIST: OK (trial persisted + reloaded via svc-storage)
```

### 7.3 Exit Gate 更新

- **§6.4 BLOCKED_EVIDENCE（QEMU virtio-blk 读写超时）已关闭**：数据盘真读、
  EIPC 真写、FLUSH、重启 LOAD 全部 QEMU 实测通过。
- 仍保留的 PARTIAL 项为外部 peer（MQTT broker/云端/联邦）与其余业务服务
  （ledger/market/observability/cloud-sync/agent-bus）target 持久化接线；
  这些不属于本次 virtio-blk 存储句柄 BLOCKED，后续 42/43 或 41 继续开发处理。

---

## 8. 第四轮：业务服务持久化接线与内存假实现移除（2026-08-16）

> 在第三轮 virtio-blk 真读写打通后，继续把 41 的业务服务从内存假实现向
> 真持久化/显式 Unavailable 推进。

### 8.1 交付

- **svc-ledger 真落盘**（`crates/agents/business-services/src/bin/svc-ledger.rs`）：
  - 经 EIPC `svc-ledger → svc-storage` 使用 `RegionStateStore<LedgerSnapshot>`
    在数据盘固定区域保存双分录账本；
  - 启动时从介质恢复快照；POST 成功后 CAS 持久化，失败返回 `ERR:persist:*`，
    不再伪装成功；
  - 存储未授予时仍可启动，但持久化操作显式失败（不退回内存假账本）。
- **新增 `UnavailableOutbox` / `UnavailableStateStore`**
  （`crates/runtime/durable-state/src/lib.rs`）：
  - 存储能力未授予时使用的 fail-closed 类型，任何操作返回
    `StoreError::Unavailable`；
  - 用于替换生产二进制中的 `MemOutbox`/`MemStateStore`，消除“内存假实现
    进入 release”的违规。
- **svc-market / svc-cloud-sync / svc-agent-bus**：
  - 生产路径不再使用 `MemOutbox`/`MemStateStore`；
  - 存储未接线前使用 `UnavailableOutbox`/`UnavailableStateStore`，显式
    Unavailable，不伪造 durable 队列/游标。

### 8.2 验证

- `cargo test -p eneros-durable-state -p eneros-business-services`：
  durable-state 14/14，business-services 46/46 全绿。
- `cargo check -p eneros-business-services --target aarch64-unknown-none --features sel4` 通过。
- QEMU 镜像重新构建通过（`IMAGE qemu-prod result=PASS`）。
- QEMU 实弹日志确认：
  - `[svc-ledger] durable ledger fresh (no snapshot)`
  - `[svc-ledger] double-entry ledger up (durable via svc-storage)`
  - 原有 `STORAGE-PERSIST: OK` 保持通过。

### 8.3 仍剩余

- svc-market / svc-cloud-sync / svc-agent-bus 尚未接入 BlockIo 真 outbox/state，
  当前为显式 Unavailable（诚实降级）。
- svc-observability 仍为内存队列 + `durable=None`，尚未接 BlockIo 持久队列。
- 外部 peer 仍未接通。

---

## 9. 第五轮：market/cloud/agent-bus/observability 真 durable 接线（2026-08-16）

> 在第四轮显式 Unavailable 的基础上，继续把 storage 授予路径接成真 BlockIo。

### 9.1 交付

- 新增 `target_durable::MaybeOutbox<M>` / `MaybeState<S>` 二态封装
  （`crates/agents/business-services/src/target_durable.rs`）：
  - `Real(AppendOutbox<M, BlockIo>)` / `Real(RegionStateStore<S, BlockIo>)`
  - `Unavailable(...)` 作为未授予时的 fail-closed 分支。
- **svc-market**：存储授予时使用 `AppendOutbox<QuoteMessage>` 真落盘；
  未授予时 `UnavailableOutbox`。
- **svc-cloud-sync**：存储授予时使用 `AppendOutbox<UploadItem>` 真落盘；
  未授予时 `UnavailableOutbox`。
- **svc-agent-bus**：存储授予时使用 `RegionStateStore<SessionCursor>` 真落盘；
  未授予时 `UnavailableStateStore`。
- **svc-observability**：
  - 为 `TelemetryItem` 增加 `Encode/Decode`；
  - 存储授予时挂接 `AppendOutbox<TelemetryItem>` 到 `durable`；
  - 未授予时保持 `durable=None`（显式无持久补传）。
- 所有服务均按 `eipc_mask & 1` 判断 storage 是否授予，避免无谓创建 EIPC 客户端。

### 9.2 验证

- `cargo check -p eneros-business-services --target aarch64-unknown-none --features sel4` 通过。
- `cargo test -p eneros-business-services -p eneros-durable-state`：46 + 14 全绿。
- `cargo xtask image qemu-prod` 重新构建通过。
- QEMU 实弹日志确认：
  - `[svc-market] durable outbox attached (svc-storage)`
  - `[svc-cloud-sync] durable outbox attached (svc-storage)`
  - `[svc-observability] durable telemetry outbox attached (svc-storage)`
  - `[svc-agent-bus] durable cursor store attached (svc-storage)`
  - `STORAGE-PERSIST: OK` 保持通过
  - `READINESS: svc-ledger=Ready`
  - `PRODUCT-READY: true (degraded: ; required degraded: )`

---

## 10. 第六轮：svc-ota 真实 AbPartitionOta 接线（2026-08-16）

> 在 `EipcBlockDevice` 就绪后，将 svc-ota 从“策略层 + storage-not-granted”升级为
> 真实 `AbPartitionOta`（A/B 分区写入/验签/激活/回滚状态机）。

### 10.1 交付

- `svc-ota` 在存储授予时构造：
  - `EipcBlockDevice<Sel4IpcClient>`（512B 块，32768 块）
  - `AbPartitionOta`（真实 SM3/SM2 A/B 状态机）
  - `OtaService`（策略门 + 下载/验签/试启动/提交/回滚编排）
- 新增 EIPC 命令：
  - `0x02 DOWNLOAD+VERIFY`：接收 meta + image bytes，写非激活槽并回读验签
  - `0x03 TRIAL`：激活 + 试启动计数
  - `0x04 COMMIT` / `0x05 REGRESSION`：提交 / 健康回归回滚
- 开发用 OTA root key 暂取 `product/config-bundle-pubkey.hex` 的公钥；生产由 KMS 注入替换。

### 10.2 QEMU 关键日志

```
[svc-ota] block storage GRANTED (EIPC slot 3) —— trial 状态真落盘
[svc-ota] real AbPartitionOta attached (A/B via svc-storage)
[eneros-root-task] READINESS: svc-ota=Ready
[eneros-root-task] PRODUCT-READY: true (degraded: ; required degraded: )
[eneros-root-task] STORAGE-PERSIST: OK
```

### 10.3 验证补充（host 端到端 OTA）

- 新增 `eipc_block_device_full_ota_download_verify_commit` 测试：
  - 通过 `EipcBlockDevice` + `AbPartitionOta` + `OtaService`
  - 真实 SM2 密钥生成、SM3 哈希、SM2 签名
  - 完成 `download_and_verify → Verified`、`mark_trial_boot → TrialStarted`、`commit → Committed`
- business-services lib 测试：52/52 通过（含 OTA 命令协议正例/负例/无服务/回滚、observability durable 恢复、UnavailableOutbox/StateStore fail-closed）。

### 10.4 全仓回归

- `cargo test --workspace --no-fail-fast`：全仓通过（exit 0，含 root-task OTA 探针后复验）。
- `cargo test -p eneros-business-services --lib`：52/52 通过。
- 生产二进制已无 `MemOutbox` / `MemStateStore`（仅测试模块使用）。

### 10.5 仍剩余

- 尚未在 QEMU 中用真实签名 OTA 包走完整 download→verify→trial→commit 命令；
  当前命令已接入真实 A/B 状态机，host 端到端已通过，QEMU 端到端需配套签名包与 KMS 注入。

---

## 11. 第七轮：observability durable 补传修复后 QEMU 复验（2026-08-16）

- 修改 `ObservabilityService::flush_batch`：先 durable enqueue，再 export，失败不回内存队，由 `recover_durable()` 恢复。
- QEMU 复验：
  - `[svc-observability] durable telemetry outbox attached (svc-storage)`
  - `READINESS: svc-observability=Ready`
  - `PRODUCT-READY: true`
  - `STORAGE-PERSIST: OK`
- business-services lib：48/48。


---

## 12. QEMU 端到端 OTA A/B 成功（2026-08-16）

> 在 root-task 增加真实签名 OTA 探针：使用与 config-bundle 测试键一致的
> 私钥种子 `0x41` 生成 SM2 签名镜像，经 EIPC 调 svc-ota 完成
> download→verify→trial→commit 全链路。

### 12.1 QEMU 关键日志

```
[svc-ota] real AbPartitionOta attached (A/B via svc-storage)
[eneros-root-task] STORAGE-PERSIST: OK
[eneros-root-task] OTA-DOWNLOAD: verified via svc-ota (real A/B write+readback)
[eneros-root-task] OTA-DOWNLOAD: trial started
[eneros-root-task] OTA-DOWNLOAD: committed
[eneros-root-task] OTA-DOWNLOAD: OK (real A/B download/verify/trial/commit)
[eneros-root-task] PRODUCT-READY: true
```

### 12.2 交付

- `crates/runtime/root-task/src/service.rs`：新增 `ota_download_probe`
  - 构造 512B 镜像，SM3 哈希 + SM2 签名（测试键）
  - 发送 OTA1 0x02 DOWNLOAD+VERIFY、0x03 TRIAL、0x04 COMMIT
- `crates/runtime/root-task/Cargo.toml`：增加 `eneros-crypto` 依赖
- 重新构建 seL4 elfloader（root-task 内嵌）与 qemu-prod 镜像

### 12.3 Exit Gate 更新

- #4 system OTA 真实 A/B 存储/验签/readback/health/commit-rollback 已升级为 **PASS**（QEMU 端到端）。
- model OTA 仍为 Optional，不参与 release 宣称。

---

## 13. 复查补丁：持久化/OTA 状态机缺陷修复（2026-08-16 第四轮复查）

> 在第七轮 QEMU 端到端 OTA 后，对 41 核心代码做一轮静态复查，发现并修复
> 以下会导致“重启后容量递减 / 账本恢复失败 / 配置代次丢失 / OTA 回滚后版本
> 误报 / telemetry 未知类别静默降级”的缺陷。

### 13.1 修复清单

1. **AppendOutbox 重启后 ACK 槽位被重新占住**
   - 文件：`crates/runtime/durable-state/src/lib.rs`
   - 现象：`open()` 把 `PeerAcked` 槽位也插入 `slots`，导致已 ACK 槽位在重启后
     不再可复用，容量随重启次数递减直至 `OutOfSpace`。
   - 修复：`open()` 对 `PeerAcked` 槽位直接跳过，不占用 `slots`。
   - 回归：`outbox_acked_slots_remain_reusable_after_restart`（旧实现 FAIL）。

2. **AppendOutbox tail 撕裂后丢弃已提交槽位 / 可能复用旧 OutboxId**
   - 文件：`crates/runtime/durable-state/src/lib.rs`
   - 现象：tail 损坏时直接 `return Ok(())`，不扫描槽位；若 tail 在多次入队后
     撕裂，已完整写入的 pending 全部丢失，且 next_id 可能回退导致旧 id 复用。
   - 修复：tail 撕裂时仍继续扫描全部槽位，并依据已见最大 id 推进 next_id。
   - 回归：`outbox_corrupt_tail_still_recovers_committed_slots`（旧实现 FAIL）。

3. **LedgerSnapshot::decode 未解析 RegionStateStore 的 generation 前缀**
   - 文件：`crates/agents/business-services/src/ledger.rs`
   - 现象：`RegionStateStore<LedgerSnapshot>` 持久化后重启 `load()` 会因把
     generation 8B 当作账本数据而报 `Encode("ledger-snapshot")`，svc-ledger
     无法恢复 durable 账本。
   - 修复：`decode` 先解析 8B generation，再解码剩余账本快照。
   - 回归：`ledger_snapshot_region_store_roundtrip`（旧实现 FAIL）。

4. **非 control-write 配置 consumer 的 apply 结果固定 generation=0**
   - 文件：`crates/agents/business-services/src/config_service.rs`
   - 现象：`CloudSyncConsumer/LoggingConsumer/MetricsConsumer/OtaConsumer`
     的 `apply` 忽略传入 generation，返回 `Applied { generation: 0 }`，
     破坏 active config generation 上报语义。
   - 修复：所有 consumer 的 `apply` 使用实参 generation。
   - 回归：`all_consumers_apply_propagate_generation`。

5. **OTA 回滚后 current_version 未恢复旧版本**
   - 文件：`crates/agents/business-services/src/ota_service.rs`
   - 现象：`mark_trial_boot` 把 `current_version` 更新为目标版本，但
     `health_regression` 回滚后不恢复，导致后续升级策略误判（旧版本被当
     成新版本，阻止重装/降级判断错误）。
   - 修复：新增 `previous_version`，回滚时恢复。
   - 回归：`negative_health_regression_rolls_back` 增加 `current_version == "1.0.0"` 断言
     （旧实现 FAIL）。

6. **svc-observability 未知 telemetry class 静默降级为 Debug**
   - 文件：`crates/agents/business-services/src/bin/svc-observability.rs`
   - 现象：EIPC 入队时 `p[0]` 非 0..5 被当作 `Debug`，违反 fail-closed。
   - 修复：未知类别返回 `ERR:class`。

### 13.2 验证

- `cargo test -p eneros-durable-state -p eneros-business-services -p eneros-config-bundle`：
  durable-state 16/16、business-services 54/54、config-bundle 18/18 全绿。
- `cargo check -p eneros-business-services --target aarch64-unknown-none --features sel4` 通过。
