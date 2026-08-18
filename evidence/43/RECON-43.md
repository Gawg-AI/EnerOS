# 43 — Entry Gate 审计（RECON-43）

> schema: `eneros.blueprint-43-recon.v1`
> 编制：2026-08-18（蓝图 43 执行）
> 分支：`feature/v0.165.0-protocol-driver-sdk`
> 42 状态：Exit Gate 15/15 PASS（evidence/42/EXIT-GATE-STATUS.md），
> evidence42 audit-all 重算 conclusion=READY（本卷执行时复核，
> `evidence/42/audit-all.json`），closure gate PASS（43 收口核实）。

---

## 1. Entry Gate 判定（蓝图 43 §1.1）

| 条件 | 结果 | 说明 |
|---|---|---|
| 42 candidate lineage 全部 L0–L3 PASS | **PASS** | round131 八张信封（formal-boot/install/recovery/update/ab-update/soak-smoke/soak-12x/soak-100x）绑定当前候选；check-current + self-test 18/18 PASS；本卷工作树变更后按 42 重绑定流程重新绑定（见 42-CLOSE-OUT.md） |
| 无 BLOCKED/PENDING required software evidence | **PARTIAL** | 42 Exit #7 协议独立对端：Modbus TCP L1 独立 raw peer VERIFIED（evidence/42/modbus-result.json）；IEC104/MMS/GOOSE/SV/MQTT/Modbus-RTU 的 L2 独立对端 **BLOCKED**——正式介质协议服务（svc-iec104 等 7 个）为能力壳（`UnassignedProtocolService::capability()=None`，`EXECUTE→ST_UNAVAILABLE`），产品端点未移交设备/传输句柄（39/40 owner，BLOCKED-42.md #4）。按蓝图 §1.1 该 required 软件项未闭合 → 43 不得以真机"顺便证明"；已登记 RETURN_TO_OWNER（39/40）并在 Go/No-Go 中如实计为 BLOCKED |
| 环境/外部资源（72h soak、Windows 本地、CI 远端双构建） | **BLOCKED（外部）** | 时长/宿主/CI 资源缺口；95-cycle 与 100-cycle endurance PASS（0 bytes/day 泄漏、零损坏）、5000-cycle 后台实跑记录；Windows 由 CI matrix 覆盖未本地实跑；CI 双构建 step 已配（gkernel job），本地无 act |
| 当前仓库 dirty 状态 | **DIRTY（开发基线）** | 36–42 未提交开发代码 + 本项目 43 资产；本轮证据为开发证据，不是 clean commercial release 签署依据（WP-43.14 由 owner 建 clean tag 后另行签署） |

**结论**：Entry Gate 在"42 软件 L0–L3 证据链"上 **PASS**；L2 协议独立对端为
required 软件证据但 **BLOCKED（39/40 句柄移交）** ——按蓝图必须退回 owner，
本卷将其记录为唯一结构性 BLOCKED，不借真机/外部项掩盖（43 的唯一结论
仍为 NO-GO，见 43.17 与 docs/v1.0.0-go-no-go.md）。

## 2. 唯一目标与当前裁决

43 唯一目标：验证开放式 AArch64 适配合同 + 每个拟销售盒子/SKU/协议组合
的 L4 资格与商业 Go/No-Go。本卷完成可机器复核的软件侧证据基建（目录/矩阵/
板级事实/平台准入 Gate/机器 Gate/唯一裁决文档），并**完成了 Modbus-TCP 的
L2 独立 peer 对通（svc-modbus-tcp 产品端点真实 I/O，见下）**，同时如实登记
其余 L4 与外部 BLOCKED。**当前商业裁决：NO-GO / BLOCKED**（外部硬件/实验室/
审批未完成，蓝图 43 §43.17 唯一结论规则）。

### 2.1 L2 协议互操作收口（本卷新增，43 执行期完成）

- **Modbus TCP（L2 VERIFIED）**：正式介质 `svc-modbus-tcp` 经 root 移交
  virtio-net 槽 8（`NET-HANDOVER mmio phys=0xa001000→vaddr=0x40020000`
  + 9 页连续 DMA 池 1:1）→ VirtioNetDriver → smoltcp（10.0.2.15/24，网关
  10.0.2.2）→ ModbusTcpMaster，与独立 raw 从站（`evidence/43/modbus_l2_slave.py`，
  零 EnerOS 代码）经 QEMU user-mode（slirp）对通：3 笔
  read-holding-registers（0x1111/0x2222/0x3333）由外部对端记录并应答，
  读回值一致。证据：`evidence/43/l2-modbus-tcp-verified.json`（INTEROP/PASS，
  verify+gate 通过）+ `target/evidence43/l2-modbus-tcp/`（串口 + peer
  transcript）。
- 剩余协议（IEC104/MMS/GOOSE/SV/MQTT/Modbus-RTU）L2 对端：仍 BLOCKED
  （各自产品端点传输接线未做——同 svc-modbus-tcp 路径可推广，属 39/40
  后续接线工作）。

## 3. 拟销售 SKU / 协议清单（catalog 与矩阵事实源）

| SKU | 类型 | BSP/Adapter | 协议 | 状态 |
|---|---|---|---|---|
| qemu-virt-aarch64 | development（不可销售） | `QemuVirtAdapter` | modbus-rtu/tcp, iec104, mqtt, mms, goose, sv | not-qualified（无 L4） |
| phytium-d2000 | product（首个生产平台） | FdtDescriptionProvider + TBD adapter | （profile 未装配） | not-qualified（硬件 BLOCKED、事实未冻结） |
| kunpeng-920 | product | FdtDescriptionProvider + TBD adapter | （profile 未装配） | not-qualified（硬件 BLOCKED、事实未冻结） |

协议×角色矩阵（`product/sku-matrix.json`，每组合独立取证）：

| SKU | Interface | Protocol | Role | Security profile | Peer | Result |
|---|---|---|---|---|---|---|
| qemu-virt | RS485 | Modbus RTU | master | physical/local | 独立 raw peer | PENDING（L0） |
| qemu-virt | Ethernet | Modbus TCP | master | physical/local | 独立 raw peer | **VERIFIED（L2 产品端点，evidence/43/l2-modbus-tcp-verified.json：virtio-net 槽8 移交 + smoltcp + 独立 raw 从站，3 事务读回 0x1111/0x2222/0x3333）** |
| qemu-virt | Ethernet | IEC104 | master/slave | TLS/IEC62351 policy | 独立 peer | PENDING（L0；L2 对端 BLOCKED） |
| qemu-virt | Ethernet | MMS | client | product policy | relay/simulator | PENDING（L0） |
| qemu-virt | Ethernet | GOOSE | pub/sub | product policy | 独立 peer | PENDING（L0） |
| qemu-virt | Ethernet | SV | pub/sub | product policy | 独立 peer | PENDING（L0） |
| qemu-virt | Ethernet/Cellular | MQTT | client | mTLS | broker | PENDING（L0） |

未测角色/安全 profile 不从其它组合推导（矩阵逐行独立 evidence 字段）。

## 4. 可用资源清单

- 实验室：无（本环境 Linux 宿主，无实体 ARM 盒子）。
- HIL：无；`docs/platform-contract/d2000-hil-spec.md` 与
  `g10-hardware-qualification-spec.md`（HW-001~018 契约）已冻结规格但
  全部"无硬件"阻塞（docs/platform-contract/hw-task-cards.md）。
- 独立 peer：Modbus TCP L1 raw peer（evidence/42/modbus-result.json，
  独立 spec 级帧，无 EnerOS 代码）；iec104/mms/mqtt L2 peer 未配置。
- EMC/安规/密码/入网：无外部评审资源；密码学独立评审与商密认证为 43
  前置（product/crypto-policy.toml CHG-2026-08-17-42-closure-sm2 绑定条件）。
- recovery/factory/provisioning/烧录/密钥注入 SOP：媒体链（raw/ISO/
  recovery/update/SBOM）与 file-lab 写入/readback 真实存在（35-01 §3 #27）；
  产线 SOP（WP-43.10）未建立（无产线）。

## 5. owner / 审批 / 风险接受

- 43 软件侧 owner：本卷（兼容目录/矩阵/板级事实/机器 Gate/唯一裁决）。
- 39/40 owner：协议服务设备/传输句柄移交（L2 对端 BLOCKED 的唯一退口）。
- 36/41 owner：candidate sv2 全介质复现与 root-task shim 跨环境确定性
  （RETURN_TO_OWNER-42；本卷已在 42-CLOSE-OUT.md 书面登记 provenance）。
- 36 owner：profile security_version 单一事实源（sv2 候选 provenance）。
- 产品/架构/安全/质量/交付负责人签署：未取得（43.16 人工签署 BLOCKED）。
- 风险接受：本卷不接受任何 P0/安全 fail-open；closure 决议已按
  CHG-2026-08-17-42-closure-sm2 登记（商密认证不通过时必须回退）。

## 6. release clean baseline 计划

- 当前全部证据为 dirty 开发基线（未提交工作流）；按 WP-43.14，正式
  授权要求 owner 建 clean commit/tag → clean clone → 锁定工具链/seL4/BSP/
  固件/密钥政策 → 42 reproducible build → 唯一 candidate_id → 全部
  L0–L4 证据绑定同一 candidate。本卷未获授权自动提交/打 tag，故 clean
  baseline 为 **BLOCKED（owner 动作）**，与本卷证据严格区分。

## 7. WP-43.9 / WP-43.10 开发登记（后续轮次）

43 执行期唯一裁决 NO-GO 后，按「检查未开发工作包并开发」补齐两块
**software 侧产物化缺口**（未改任何生产实现）：

- **WP-43.9 physical writer 产品化（软件侧）**：
  - 签名 provisioning job：`flash authorize-file` 现签发 ed25519 签名
    令牌，绑定 operator/station/candidate/tool-version/readback 策略/
    key 域（key_id 指纹 + 内嵌公钥自校验）；
  - 稀疏写入：只写非零 extent（e2e：134MB 镜像 = 23 extents）；
  - 断点续写：`--fault mid-write-interrupt` 中断留 resume journal，
    重跑续写（介质无篡改时 resumed=true），部分状态绝不报成功；
  - readback 全量/采样策略（sample = 非零 extent 清单摘要，确定性）；
  - 报告绑定 operator/station/candidate/key 域/extent/续写信息。
- **WP-43.10 产线安全（软件侧）**：
  - 产线 key 域分离：`product/factory/key-domains.json`
    （factory/release/recovery 三域 + 允许用途，`flash key-domains`
    机器校验）；N/A 须负责人批准，执行 AI 不代填；
  - 每设备签名出生记录：`flash birth-record`（factory 域 ed25519，
    只接受 PASS write 报告）+ `flash verify-birth` 验签。
- 证据：`evidence/43/wp43-9-10/`（token/report/birth-record/key-domains/
  负例 + 摘要信封）；flasher 单测 10/10，全 xtask 127/127。
- **仍 BLOCKED（不属软件侧）**：物理设备写入需真机/厂方 key；真机产线、
  HIL/EMC/制造执行均为 owner/硬件/外部动作。