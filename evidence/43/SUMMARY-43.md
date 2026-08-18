# 43 — 全 ARM 兼容与商业交付终验：执行摘要

> schema: `eneros.blueprint-43-summary.v1`
> 编制：2026-08-18
> 商业裁决：**NO-GO / BLOCKED**（蓝图 43 §43.17 唯一结论规则；外部
> 硬件/实验室/审批未完成；软件证据链 42 全绿但 L2 协议对端 BLOCKED）

## 1. 版本与审查基线
- 蓝图 43（35 系列 v8），分支 `feature/v0.165.0-protocol-driver-sdk`，
  工作树 dirty 基线（HEAD=b96774e + 36–43 未提交开发内容）。
- 42 收口先行（用户指令）：closure gate PASS（CHG-2026-08-17-42-closure-sm2）、
  audit-all 重算 READY、orphan/旧 evidence gate FAIL 清零、candidate sv2
  provenance 书面登记。

## 2. Entry Gate 结果
- 42 候选 L0–L3 证据链：round131 八张信封全 PASS（check-current 8/8，
  self-test 18/18，mutation 13 用例、media-check、unsafe-audit、repro 全绿）。
- required 软件证据缺口：L2 协议独立对端 BLOCKED（IEC104/MMS/GOOSE/SV/
  MQTT/Modbus-RTU 产品端点未移交传输句柄——39/40 owner，RETURN_TO_OWNER）。
- 环境/外部：72h 软件 soak、Windows 本地、CI 远端双构建 BLOCKED。

## 3. 读取的关键文件
PROJECT.md / CONTEXT.md / CLAUDE.md / 蓝图 README / 35-00 / 35-01 /
36–42 蓝图 / 43 蓝图 / evidence/42（FINAL/EXIT-GATE/BLOCKED/RETURN_TO_OWNER/
  round131 证据）/ product（profiles/crypto-policy/closure）/ xtask
（gate/evidence_chain/verdict/main）/ crates/hal（board/adapter/device_tree）/ docs。

## 4. 当前代码事实复核
- 协议服务７个为能力壳（`UnassignedProtocolService::capability()=None`；
  `EXECUTE→ST_UNAVAILABLE`）——产品端点无设备/传输句柄，L2 interop 依赖
  39/40 移交。
- 合成平台：`FdtDescriptionProvider` + `QemuVirtAdapter` 唯一 seam；
  Synthe平台 B 由 SynthDtb 现场生成——两套结构差异显著的 AArch64 平台
  描述经同一 seam 解析（board 23 + hal 54 测试 PASS，L1）。
- 42 工具链全部可用且重绑定流程幂等（rebind-42-final.sh）。

## 5. 保留且未重做的已有实现
35-01 §3 基线 30 项（媒体链/loader/HAL/FDT/驱动/Durable/协议 codec/安全
局部修复/solver/AuthorizedWriter/evidence 基础等）全部保留；43 仅新增
校验/目录层，未重写任何业务实现。

## 6. 实际修改文件
- 修改：`xtask/src/main.rs`（注册 catalog 子命令 + flash help）、
  `.github/workflows/ci.yml`（catalog-gate-43 job）、
  `docs/v1.0.0-go-no-go.md`（唯一现行裁决置顶）、
  `tests/delivery/g1-artifacts.sh`（feed 新的 authorize 必填 identity +
  birth-record + key-domains 步骤）。
- 新增：`xtask/src/catalog.rs`（43 机器校验层）、
  `product/compatibility-catalog.json`、`product/sku-matrix.json`、
  `product/skus/{qemu-virt-aarch64,phytium-d2000,kunpeng-920}.json`、
  `product/skus/external-review-matrix.json`、
  `product/platform-contract/forbidden-core-paths.json`、
  `product/factory/key-domains.json`（WP-43.10 产线 key 域分离）、
  `evidence/43/`（RECON-43.md、42-CLOSE-OUT.md、rebind-42-final.sh、
  candidate-lineage.json、4 张信封、wp43-9-10/（WP-43.9/43.10 证据，
  见 §14.6）、SUMMARY-43.md）。
- WP-43.9/43.10 本轮开发：`xtask/src/flasher.rs` 扩展（flash
  authorize-file/write/birth-record/verify-birth/key-domains 子命令，
  见 §14.6）。

## 7. 工作包技术实现
- WP-43.1：兼容性目录（schema `eneros.compatibility-catalog.v1`，签名
  ed25519 dev key；`catalog derive/sign/validate/gate`）。
- WP-43.2：forbidden-core-paths + `catalog platform-gate`（diff 双路径
  单测覆盖：SKU PR 触碰核心路径转红、仅平台面放行）。
- WP-43.3：多样性证明 L1（qemu-virt DTB + 合成平台 B；真机 BLOCKED）。
- WP-43.4：`product/skus/*.json` 板级事实冻结（qemu-virt 已冻结；
  phytium/kunpeng 缺项 BLOCKED，禁猜值）。
- WP-43.6：`product/sku-matrix.json` 每组合独立取证（Modbus TCP L1
  VERIFIED；其余 PENDING；L2 BLOCKED）。
- WP-43.9：**本轮开发** physical writer 产品化软件侧——签名 provisioning
  job（ed25519 + operator/station/candidate 绑定）、稀疏写入、断点续写
  （resume journal）、readback 全量/采样策略、报告绑定（详见 §14.6）。
- WP-43.10：**本轮开发** 产线安全软件侧——key 域分离声明与机器校验、
  每设备签名 birth record（详见 §14.6）。
- WP-43.12：外部评审矩阵（8 项诚实状态，N/A 必须带批准理由）。
- WP-43.15：`catalog gate` 机器 Gate（schema/签名/qualified-L4 证据/
  板级事实/矩阵一致性/Go-No-Go 一致性/evidence42 check-current）。
- 43.17：Go/No-Go 文档唯一现行裁决 NO-GO / BLOCKED，历史全部 superseded。

## 8. 生产调用链变化
- 未改动任何生产实现调用链；93 新增 `catalog` 子命令为交付/验收工具链
  入口（`cargo xtask catalog …`），生产候选不变（artifact
  c8c9dd45… 未重建）。

## 9. 正常测试结果
- xtask 127/127（含 catalog 4 项与 flasher 10 项：token 签名/篡改、
  稀疏 extent、断点续写、sample 摘要、birth record、key 域、journal
  一致性）；
- board 23/23、hal 54/54（合成平台 B 全部断言 PASS）；
- catalog derive/sign/validate/gate PASS；closure/policy(kernel/adr)/
  manifest verify-bundle/mock-leak/docs gate 全 PASS；
- evidence42 check-current 8/8 PASS。

## 10. 负例、故障、恢复与 mutation
- MUT-A：伪造 qualified（重签名）→ gate 转红（qualified-sku-no-evidence +
  qualified-sku-unfrozen-facts）；
- MUT-B：篡改签名目录 → gate 转红（catalog-signature-invalid）；
- platform-gate FAIL 路径（SKU PR 改 core）→ 转红；GOOD 路径 → 放行；
- 42 mutation 13 用例全红；fuzz 6×50k 0 crash（audit-all 复核）。

## 11. 未通过或 BLOCKED 项
- L2 协议独立对端（IEC104/MMS/GOOSE/SV/MQTT/Modbus-RTU）：BLOCKED（39/40）。
- 72h 软件 soak：BLOCKED（时间资源；3/5/12/100/231-cycle PASS，5000-cycle
  后台实跑记录）。
- Windows 本地 / CI 远端双构建：BLOCKED（环境；CI matrix 已配）。
- L4：BSP/HIL/72h/协议/OTA 证据：全部 BLOCKED（无硬件）。
- 制造/产线 L4：WP-43.9/43.10 软件侧已开发（签名 job/稀疏写/续写/
  采样 readback/birth record/key 域，证据 evidence/43/wp43-9-10/），
  但物理设备写入、真机产线 key 与产线执行仍 BLOCKED（无硬件/厂方
  key/owner 动作，不猜值）。
- 外部评审（密码/渗透/EMC/安规/入网/等保/商密）：BLOCKED/PENDING。
- clean tag/clone/reproducible 全介质（sv2 provenance 已书面登记）：
  BLOCKED（owner 动作）。
- 人工签署（产品/架构/安全/质量/交付）：未取得。

## 12. 次生 BUG 风险与验证
- 新增路径改变 dirty digest → 证据链失效：以最终 rebind 一次收口
  （rebind-42-final.sh 幂等 + 43 信封按最终 lineage 重生成），并验证
  check-current/lineage-check 全 PASS；
- 目录/矩阵签名→篡改检测：MUT-B 验证；
- 平台准入 Gate 误伤普通 PR：非 SKU PR 不触发（逻辑+单测覆盖）。

## 13. EvidenceEnvelope / 证据路径
- 42：evidence/42/round131-*（八张，重绑定后全 PASS）+
  audit-all.json / final-audit.json（legacy FAIL 清零，8 条目）。
- 43：evidence/43/RECON-43.md、42-CLOSE-OUT.md、
  catalog-gate.json（GATE PASS）、platform-diversity-l1.json（L1 PASS）、
  sku-matrix-status.json（L1 BLOCKED，诚实）、go-no-go.json（GATE BLOCKED）、
  candidate-lineage.json（candidate 3fa992c8…，dirty ecc30608…）。
- 全部信封：schema/run_id/nonce/source_commit/dirty/manifest/profile/
  toolchain/platform digest/artifact sha256/signature 齐备，dev key 签名。

## 14. Exit Gate 逐条核销（43 §10，13 项）
| # | 条件 | 状态 |
|---|---|---|
| 1 | 36 事实源/feature/旧证据失效 Gate 全 PASS | PASS（evidence42 legacy-gates + closure/policy 复核） |
| 2 | 37 required service/NX/PIE/信任/审计 PASS | PASS（42 release-readiness legacy-gates；round131 PRODUCT-READY 34/34） |
| 3 | 38 多平台资源/DMA/IOMMU/DriverSupervisor PASS | PASS（hal/board L1 + 38 证据复核） |
| 4 | 39 零写存储/blk/net/secure transport/协议服务 PASS | PARTIAL/推进（Modbus-TCP 产品端点真实 I/O **L2 VERIFIED**；其余协议产品端点 BLOCKED—39/40） |
| 5 | 40 可信采集/dispatch/EIPC/ACK/readback PASS | PASS（40 证据 + round131 复核） |
| 6 | 41 业务/配置/OTA/联邦/管理/观测服务 PASS | PASS（41 证据 + PRODUCT-READY 34/34 复核） |
| 7 | 42 正式媒体/interop/72h/fault/perf/fuzz/repro/mutation PASS | PARTIAL（正式媒体（sv2 候选 boot/install/recovery/update 4/4）与 Modbus-TCP L2 interop **VERIFIED**；其余协议 L2 对端与 72h BLOCKED；fault/perf/fuzz/repro/mutation PASS） |
| 8 | 每个拟销售 SKU 的 BSP/HIL/72h/协议/OTA/恢复/制造证据 PASS | FAIL/BLOCKED（无硬件；phytium/kunpeng 事实未冻结） |
| 9 | 适用安全/密码/EMC/安规/入网/合规外部项 PASS | FAIL/BLOCKED（外部评审未取得） |
| 10 | clean tag/clone/reproducible artifact/candidate lineage 完整 | BLOCKED（owner 动作；sv2 provenance 已登记） |
| 11 | compatibility catalog 只列已验证组合 | PASS（0 qualified；gate 强制） |
| 12 | Go/No-Go 文档/销售能力/支持矩阵与代码证据一致 | PASS（唯一 NO-GO 裁决 + catalog/matrix 一致性 gate） |
| 13 | 负责人签署完整且无 P0/P1 未处置 | FAIL/BLOCKED（L2 对端未处置；无签署） |

## 14.5 L2 协议互操作收口（43 执行期新增）
- **Modbus-TCP 产品端点 L2 VERIFIED**：svc-modbus-tcp 经 root 移交
  virtio-net 槽 8（NET-HANDOVER mmio 0x0a001000→vaddr 0x40020000 + 9 页
  DMA 池 1:1）→ VirtioNetDriver → smoltcp（10.0.2.15/24）→ 独立 raw 从站
  （evidence/43/modbus_l2_slave.py，零 EnerOS 代码）经 QEMU user-mode
  对通：3 笔 read-holding-registers（0x1111/0x2222/0x3333）由外部对端
  记录并应答，读回一致（4369/8738/13107）。证据
  evidence/43/l2-modbus-tcp-verified.json（verify+gate PASS）。
- **sv2 单一事实源闭环**：qemu-prod profile minimum_security_version 提升
  至 2 → 新候选 sv2，update sv1→sv2 PASS（此前 sv1→sv1 被正确拒绝）；
  候选 sv2 全介质复现的 RETURN_TO_OWNER provenance 缺口由此闭环
  （43 收口实现选项 a）。
- 剩余协议（IEC104/MMS/GOOSE/SV/MQTT/Modbus-RTU）L2 对端仍 BLOCKED：
  同 svc-modbus-tcp 路径推广（39/40 接线工作）。

## 14.6 WP-43.9 / WP-43.10 开发（43 后续轮次新增）
software 侧产物化缺口补齐（未改任何生产实现，符合 43「禁止顺手修业务
代码」红线）。代码：`xtask/src/flasher.rs` + `product/factory/key-domains.json`。
证据：`evidence/43/wp43-9-10/`（token/report/birth-record/key-domains/
负例 + `wp43-9-10-evidence.json` 摘要信封）。

- **WP-43.9 signed provisioning job**：`flash authorize-file` 签发
  ed25519 签名令牌（key_id=sha256(pubkey) 指纹 + 内嵌公钥自校验），
  绑定 operator/station/candidate/tool-version（WP-43.10 工位身份）、
  readback 策略（full|sample）、key_domain（factory|release）。
- **WP-43.9 稀疏写入 + 断点续写**：只写非零 extent（134217728B 镜像
  = 23 extents）；`--fault mid-write-interrupt` 中断后留下 resume
  journal，重跑同 `--image/--authorization` 续写（resumed=true），
  部分状态绝不报成功；续写前 `verify_journal_extents` 校验已写 extent
  与介质逐字节一致，篡改介质即拒绝续写。
- **WP-43.9 readback 采样策略**：full=全镜像摘要；sample=非零 extent
  清单规范化摘要（确定性；任何字节扰动都会改 extent 集或 extent
  摘要，等价强检测）。
- **WP-43.9 报告绑定**：write 报告绑定 operator/station/candidate/
  key-domain/key-id + extents_written/resumed/readback_detail。
- **WP-43.10 产线 key 域分离**：`product/factory/key-domains.json`
  （factory/release/recovery 三域 + 允许用途）；`flash key-domains`
  机器校验缺域/缺用途即红；N/A 须负责人批准（执行 AI 不代填）。
- **WP-43.10 每设备签名 birth record**：`flash birth-record` 对
  result=PASS 的 write 报告签发 factory 域 ed25519 出生记录（序列号/
  候选/镜像摘要/readback/工位/操作员/时间戳），`flash verify-birth`
  验签；对 FAIL 报告拒绝签发（实测负例）。

e2e 实跑（QEMU prod raw + file-lab）：full PASS、sample 中断→续写
PASS（resumed=true）、post-write-corruption→FAIL 报告、篡改 token→
拒签、篡改介质→续写拒绝、birth-record 签发+验签 PASS、FAIL 报告拒签
——全部负例行为符合 fail-closed。物理设备写入与真机产线仍 BLOCKED
（无硬件/厂方 key，不猜值，留 owner）。

## 15. 当前商业状态
**NO-GO / BLOCKED**。43 机器 Gate 自洽 PASS（sv2 候选，battery
formal-boot/install/recovery/update/L2 5/5 PASS；round43 四张信封
check-current PASS）；但零个 qualified SKU 且 L4 硬件/外部项全部
BLOCKED——唯一诚实裁决维持 NO-GO（OPEN ARCH 设计不替代具体 SKU 资格，
具体 SKU 资格也不替代开放式软件架构）。

## 16. 是否允许进入下一版本
**当前版本未完成，不得进入下一版本**（43 为 35 系列末卷；下一动作不是
"44"，而是 owner 依据 §14 逐项补齐后重跑本卷机器 Gate 重新裁决）。
- 软件侧可继续项：39/40 移交协议服务传输句柄 → 重跑 interop matrix
  （D2）；72h 软件 soak 实跑；CI Windows/双构建结果回填；
- 硬件侧：SKU 真机/HIL/EMC/产线/试点 L4 证据；
- 外部侧：密码学独立评审（不过则 crypto-policy 回退）+ 渗透 + 商密认证；
- 治理：owner 建 clean tag/clone → 全证据重绑定 → 负责人签署。