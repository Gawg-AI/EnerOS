# SKU 选板判据与候选评估表（蓝图 51 · WP-51.1）

> schema: `eneros.board-selection.v1` · 状态：**Candidate（待 owner 裁决签字）**
> 建立：2026-08-23 · 基线 commit：9d4144c · 版本 0.313.40→0.313.41（WP Exit bump）
> 上位裁决：44-00 §1 **D3（板卡待定/考虑换板）**——本文件是 owner 决策输入，
> 不是选板结论。**最终 SKU 冻结=owner 签字**（§5 决策记录）。
> ADR 关联：ADR-0045 方案 C（板载 NPU 协处理器）转为本卷可选输入；
> WP-49.3 本地推理 spike 已 **NO-GO**（evidence/49/exit-gate.json exit-2-spike-report）。

## 0. 事实纪律（本卷红线，适用于全表）

1. 板级事实逐项 `verified` + `source`；**禁默认容量填空、禁 QEMU 地址填空**
   （43 卷 `product/skus/*.json` 模板纪律延续）。
2. 本表事实分三级：
   - ✅ **repo 内证据**（代码/dts/config，verified=true，source=仓库路径）；
   - ✅ **一手来源已核验**（本卷执行时实际抓取的官方文档，verified=true，
     source=URL+日期；当前网络环境仅 TF-A 文档可达）；
   - ⚠️ **公开规格·未经一手来源核验**（verified=false；执行环境网络受限，
     厂商站点不可达）——一律标注「待 B0 核」，冻结前必须按 §4 流程转为
     真机/手册实证。
3. 任何「初判 PASS」在 SKU 冻结前不产生资格效果；catalog 维持
   not-qualified（WP-51.3 一致性检查项）。

## 1. 选板判据表（按 44–50 卷软件需求倒推，卷蓝图判据逐项展开）

判定列含义：**硬门**=一票否决（FAIL 即出局）；**加权**=硬门全过后的排序分。
权重用于多候选同过硬门时的排序（§3）。

| # | 类别 | 判据 | 判定 | 权重 | 软件需求依据（44–50 卷事实） |
|---|---|---|---|---|---|
| C1 | 基础 | aarch64，≥4 核 Cortex-A53 或以上 | 硬门 | — | 38 卷 ARM 平台 HAL/aarch64 目标闭环；hal PlatformConfig 多平台模型 |
| C2 | 基础 | GICv3 及以上中断控制器 | 硬门 | — | hal interrupt 抽象按 GIC 版本分层（38 卷平台适配器合同） |
| C3 | 基础 | 内存 ≥2G；（条件项）≥4G 且带 NPU | 硬门 / 加权 W2 | W2=2 | 装配内存基线 QEMU 1G、规划 2G（49 卷 spike 内存审计：host RSS 574MiB vs 装配 128MiB→NO-GO）；49 卷内核映射窗已扩 8MiB 承载 bundle。NPU 仅作 ADR-0045 方案 A/C 重启条件输入（spike NO-GO 后换板带 NPU=A 方案重启条件之一），**v1.0 不需要 NPU** |
| C4 | 存储 | eMMC ≥8G（或 NVMe/SATA） | 硬门 | — | svc-storage 块链路（41 卷）；OTA1 A/B 双槽+trial 需双份镜像空间（50 卷升级回滚手册）；factory provision 整盘镜像（50 卷） |
| C5 | 存储 | 掉电安全电容或 PSU 掉电通知（OTA 断电语义） | 硬门 | — | durable-state/掉电场景（41 卷）、A/B trial 断电续审（50 卷手册断电场景行）；33 卷 durable-log 掉电语义 |
| C6 | 网络 | ≥1× 千兆以太网 | 硬门 | — | 46 卷 MMS/GOOSE/SV/104 L2 对端性能前提（61850 需要性能） |
| C7 | 网络 | MAC 硬件时间戳（gPTP/PTP）优先 | 加权 W1 | W1=3 | SV/GOOSE 时标质量（46 卷 L2 VERIFIED 协议族）；无硬件时戳只能软件对时降级 |
| C8 | 总线 | ≥2× UART（控制台+485 隔离）+ 485 收发器 | 硬门 | — | PL011 真寄存器驱动就绪未上硬件（锚点④）；modbus-rtu L2 BLOCKED 待真机（evidence/46/l2-modbus-rtu-blocked.json）；48 卷 645/101 串口协议 |
| C9 | 总线 | CAN 控制器（SoC 原生或板载 SPI SJA1000）+ 收发器 | 硬门 | — | gbt27930 plugin：SJA1000+TP 真机路径=51 卷（锚点⑤）；EnerOS 既有 SJA1000 驱动面（crates/drivers/can） |
| C10 | 安全 | TRNG 可编程熵源且**寄存器手册可得** | 硬门 | — | crypto entropy fail-closed：Phytium/KunpengTrng 占位恒 Unavailable（锚点③）；无 TRNG→实机密码全灭，bring-up 第一优先（模板 B3） |
| C11 | 安全 | 安全启动链：ROTPK 熔丝/安全启动模式 | 硬门 | — | 37 卷签名链 + boot-manager-uefi（Ed25519/SHA-256 manifest）；catalog firmware 字段契约 |
| C12 | 安全 | 防回滚单调计数器（eFuse/RPMB） | 硬门 | — | boot-protocol `security_version < recorded → SecurityVersionDowngrade` 拒绝（signature.rs:110-115 集成层供给点，锚点登记）；单调计数器=B8 集成前置 |
| C13 | 生态 | UEFI/TF-A 支持成熟度 | 硬门 | — | firmware 契约=AAVMF UEFI + boot-manager-uefi；TF-A 为 BL31 供给 |
| C14 | 生态 | 设备树/寄存器手册可得性 | 硬门 | — | hal 接入点=FdtDescriptionProvider（38 卷合同：新板=新 PlatformAdapter 实现+FDT 描述）；BSP-001 设备树基址约定 |
| C15 | 生态 | 供货与 BOM 稳定（工业温度等级/生命周期） | 硬门（owner 输入） | W5=2 | 商用交付前提（43 卷终验 NO-GO 清单：板卡待定） |
| C16 | 生态 | 二次开发文档（BSP 文档可得性） | 硬门 | — | B0 板级事实采集依赖手册；TRNG/看门狗/电源管理寄存器均需文档 |

评分制：硬门全部 PASS（含 owner 输入项认可）→ 进入加权排序；
加权总分 = Σ(得分 0–5 × 权重)。C10（TRNG 手册可得）与 C14（手册可得）
为历史风险最高两项（国内厂商文档获取难），评估表单独加粗标注。

## 2. 候选评估表

### 2.0 候选清单

| 候选 | 来源 | repo 起点 |
|---|---|---|
| 飞腾 D2000 | 卷蓝图候选（owner 既定方向之一） | configs/phytium-d2000.dts 占位骨架 + hal PHYTIUM_D2000 PlatformConfig + catalog/sku JSON 行 |
| 飞腾 E2000 | 卷蓝图候选 | 无（dts/PlatformConfig/catalog 全新接入面） |
| RK3588 | 卷蓝图候选（NPU 有利方案 C） | 无（同上全新接入面） |
| （owner 提名槽位） | 待 owner 填写 | — |
| kunpeng-920 | catalog 存量行（非本卷候选清单成员） | 维持 not-qualified 现状，owner 冻结裁决时一并处置（保留/除名） |

### 2.1 飞腾 D2000

| 判据 | 事实 | source | verified | 初判 | B0 必核项 |
|---|---|---|---|---|---|
| C1 核数 | repo PlatformConfig 声明 8 cores（占位，未真机验证） | crates/hal/hal/src/arm64/platform.rs:40 | ✅ repo | PASS* | 真机核数/型号/主频 |
| C2 GIC | 公开规格称 GICv3（未经一手核验） | 公开规格·待核 | ⚠️ | 待 B0 | GIC 版本与 distributor/redistributor 基址 |
| C3 内存 | repo 无 RAM map（sku JSON missing 登记） | product/skus/phytium-d2000.json missing[] | ✅ repo(缺失事实) | 待 B0 | RAM map/容量/保留区 |
| C4 存储 | 公开规格支持 SATA/PCIe（NVMe 可选）（未经一手核验）；eMMC 控制器有无待核 | 公开规格·待核 | ⚠️ | 待 B0 | 存储控制器类型/介质/容量 |
| C5 掉电 | 无公开资料佐证板级掉电安全设计 | — | ⚠️ | 待 B0 | 电容/PSU 通知 GPIO |
| C6 千兆 | 公开规格集成 2× GMAC（未经一手核验，PHY 板级） | 公开规格·待核 | ⚠️ | PASS* | MAC 数量/速率/PHY 型号 |
| C7 PTP 时戳 | 无公开资料确认 MAC 支持 IEEE1588 硬件时戳 | — | ⚠️ | 待 B0 | 数据手册 MAC 特性章节 |
| C8 UART | repo dts 占位含 UART 节点（compatible 级别）；真实地址/IRQ 未证实 | configs/phytium-d2000.dts（sku JSON uart=未知 verified:false） | ✅ repo | PASS*(≥2 路，PL011 兼容假设**未证实**) | UART 地址/IRQ/兼容性实测 |
| C9 CAN | 无公开资料确认 SoC 原生 CAN；SPI/USB 扩展板级可行 | — | ⚠️ | 待 B0 | 原生 CAN 或板载 SJA1000 方案 |
| C10 TRNG | **关键缺口**：crypto PhytiumTrng 占位 Unavailable（fail-closed）；寄存器手册可得性未知 | crates/security/crypto/src/rng/entropy.rs:44-52 | ✅ repo(占位事实) | **高风险待 B0** | **TRNG 寄存器手册获取+上电实测** |
| C11 安全启动 | 公开规格有可信启动相关特性（未经一手核验，细节不明） | 公开规格·待核 | ⚠️ | 待 B0 | ROTPK 熔丝/启动模式实测 |
| C12 防回滚 | eFuse 单调计数器可用性未知 | — | ⚠️ | 待 B0 | eFuse/RPMB 实证 |
| C13 TF-A/UEFI | 上游 TF-A 文档无 Phytium 平台页（rockchip.html 存在而 phytium.html 404，2026-08-23 抓取）；国产 UEFI 固件生态存在但成熟度未核 | trustedfirmware-a.readthedocs.io plat/ 索引+plat/phytium.html(404) | ✅ 一手(负面结论) | **风险** | 固件供应商/版本/源码可得 |
| C14 手册 | dts 占位骨架仅 compatible 级别；完整寄存器手册获取渠道未验证 | configs/phytium-d2000.dts（159 行最小骨架） | ✅ repo | **高风险待 B0** | BSP 文档全集 |
| C15 供货 | 信创渠道供货（owner 业务判断） | owner 输入 | — | owner | 工业温度/生命周期承诺 |
| C16 二开文档 | 同 C14 | — | ⚠️ | 高风险待 B0 | — |

### 2.2 飞腾 E2000

| 判据 | 事实 | source | verified | 初判 | B0 必核项 |
|---|---|---|---|---|---|
| C1 核数 | 公开规格嵌入式系列（Q/H 变体 ≤4 核，具体核型未经一手核验） | 公开规格·待核 | ⚠️ | PASS*(若 ≥4 核变体) | 变体选择与核数实证 |
| C2 GIC | 未经一手核验 | 公开规格·待核 | ⚠️ | 待 B0 | GIC 版本 |
| C3 内存 | 未经一手核验（嵌入式档位容量上限风险） | — | ⚠️ | 待 B0 | RAM map（≥2G 硬门复核） |
| C4 存储 | 公开规格含 eMMC/SATA 支持（未经一手核验） | 公开规格·待核 | ⚠️ | PASS* | 控制器/容量实测 |
| C5 掉电 | 无佐证 | — | ⚠️ | 待 B0 | 板级设计 |
| C6 千兆 | 公开规格集成 GMAC（数量/速率未经一手核验） | 公开规格·待核 | ⚠️ | 待 B0 | MAC/PHY 实测 |
| C7 PTP 时戳 | 无公开资料确认 | — | ⚠️ | 待 B0 | 数据手册 |
| C8 UART | 公开规格多路 UART（未经一手核验）；工业串口资源通常充裕 | 公开规格·待核 | ⚠️ | PASS* | 地址/IRQ/隔离 485 收发器 |
| C9 CAN | 公开资料提及 CAN 能力（**未经一手核验，不得作为判 PASS 依据**） | 公开规格·待核 | ⚠️ | 待 B0 | CAN 控制器型号/寄存器手册 |
| C10 TRNG | **同 D2000：手册可得性未知**（repo 无该平台熵源占位，接入面更新） | crypto entropy 平台配置点 | ✅ repo(无占位事实) | **高风险待 B0** | TRNG 手册+实测 |
| C11–C12 安全启动/防回滚 | 未经一手核验 | — | ⚠️ | 待 B0 | ROTPK/eFuse/RPMB 实证 |
| C13 TF-A/UEFI | 上游 TF-A 无 Phytium 平台页（同 D2000 负面结论，2026-08-23） | trustedfirmware-a.readthedocs.io | ✅ 一手(负面结论) | **风险** | 固件可得性 |
| C14–C16 生态 | repo 零起点（无 dts/PlatformConfig/catalog 行）；手册渠道未验证 | repo 全库 grep 无 E2000 引用 | ✅ repo(零起点事实) | 接入成本高于 D2000 | BSP 全集 |

### 2.3 RK3588

| 判据 | 事实 | source | verified | 初判 | B0 必核项 |
|---|---|---|---|---|---|
| C1 核数 | Octa-Core Cortex-A55/A76（4×A76+4×A55） | trustedfirmware-a.readthedocs.io/en/latest/plat/rockchip.html（2026-08-23 抓取） | ✅ 一手 | PASS | 主频/工艺实测核对 |
| C2 GIC | 公开规格 ARMv8.2-A + GICv3（未经一手核验，与 TF-A 端口存在性互洽） | 公开规格·待核 | ⚠️ | PASS* | GIC 版本/基址 |
| C3 内存 | SoC 支持 LPDDR4/LPDDR5（公开规格）；板级容量 2G–16G 档位丰富；**NPU≈6TOPS（公开规格·未经一手核验）**——ADR-0045 方案 A/C 重启条件加分项 | 公开规格·待核 | ⚠️ | PASS* | 板级 RAM map/NPU SDK 可得性 |
| C4 存储 | SoC 支持 eMMC/UFS/SPI NOR + PCIe 3.0（NVMe）（公开规格·待核）；nvme.rs 驱动面已有 299 行待真机验证 | configs 与 drivers/storage 锚点④ | ✅ repo(驱动面) | PASS* | 容量/控制器实测 |
| C5 掉电 | SoC 无直接判据；主流 SBC 板级电源设计参差——**板卡选型必查项** | — | ⚠️ | 待 B0（板级） | 电容/PSU 通知 |
| C6 千兆 | SoC 集成 GMAC（公开规格 2× 千兆级，速率待核） | 公开规格·待核 | ⚠️ | PASS* | MAC/PHY 实测 |
| C7 PTP 时戳 | 若 GMAC 为 DesignWare EQOS 类 IP 则支持 IEEE1588 硬件时戳（**未经一手核验**） | 公开规格·待核 | ⚠️ | 加分待 B0 | ethtool -T 实测 |
| C8 UART | 公开规格多路 UART（未经一手核验）；485 收发器=板级选配 | 公开规格·待核 | ⚠️ | PASS* | 地址/IRQ/收发器 |
| C9 CAN | **SoC 无原生 CAN（公开资料普遍口径·待核）**；判据允许板载 SPI SJA1000+收发器补足（与 EnerOS SJA1000 驱动面对接） | 公开规格·待核 | ⚠️ | 条件 PASS*（板载 CAN 必配） | SPI CAN 模块型号/回环 |
| C10 TRNG | SoC 含密码引擎/TRNG（公开资料口径·**未经一手核验**）；寄存器手册经社区/BSP 可得的概率高于封闭平台 | 公开规格·待核 | ⚠️ | 中风险待 B0 | TRNG 寄存器手册+B0 实测（bring-up 第一优先） |
| C11 安全启动 | eFuse/OTP + 启动校验链（公开资料口径·待核）；TF-A 上游端口为一手确认 | rockchip.html（一手）+公开规格·待核 | 部分✅一手 | PASS*(机制存在性)；细节待 B0 | ROTPK 烧录流程实测 |
| C12 防回滚 | eFuse 单调计数/RPMB（eMMC 介质）可行性较高（未经一手核验） | — | ⚠️ | 待 B0 | 计数器烧录/递增实测 |
| C13 TF-A/UEFI | **上游 TF-A rk3588 标准端口存在**（Octa-Core A55/A76，2026-08-23 一手抓取）；U-Boot 上游社区支持广泛（未经一手核验）；EDK2 侧社区移植存在（待核） | trustedfirmware-a.readthedocs.io/en/latest/plat/rockchip.html | ✅ 一手(TF-A) | PASS | BL31 构建+UEFI 链实测 |
| C14 手册 | TRM/原理图社区可得度高（Rockchip 开放程度公开口碑·待核）；主线 Linux DT 覆盖进行中（部分 IP 仍 vendor BSP） | — | ⚠️ | PASS*(相对最优) | 目标外设 DT 覆盖清单 |
| C15 供货 | SBC/核心板生态丰富、多厂商可替代；工业温度等级与长生命周期承诺需选板时锁定（消费级出身风险） | owner 输入+公开规格·待核 | ⚠️ | 中 | 工业 SKU/供货协议 |
| C16 二开文档 | 社区文档丰富但质量参差；官方 TRM 获取流程待走 | — | ⚠️ | 中 | TRM 正式授权副本 |

### 2.4 owner 提名槽位

> 待 owner 填写后按 §1 判据表逐项套用本模板评估。（空槽不阻塞本卷 Exit——
> 卷内明标最终选板=owner 事项，评估框架已备好即为本卷 DoD。）

## 3. 评分汇总（初判，决策输入非裁决）

硬门「待 B0」项按公开规格乐观口径计 PASS*、保守口径计入风险扣分；
两口径并列供 owner 读表。

| 候选 | 硬门（乐观*/保守） | 加权 W1 时戳 | W2 NPU | W5 供货 | 总分（乐观*/保守） | 结构性风险 |
|---|---|---|---|---|---|---|
| RK3588 | 全过*/C5·C9·C10·C15 待核 | 4（待核） | 5 | 3 | 23*/17 | 消费级出身（温度等级/生命周期）；CAN 须板载补；vendor BSP 依赖面 |
| 飞腾 D2000 | 待核为主/C10·C13·C14 高风险 | 2（未知） | 0 | 4（信创渠道） | 12*/9 | TRNG/手册/固件三重文档风险；上游固件生态空白 |
| 飞腾 E2000 | 待核为主/同上+容量档位风险 | 2（未知） | 0 | 4（信创渠道） | 11*/8 | 同 D2000 文档风险 + repo 零起点接入成本 |

初判排序：**RK3588 ＞ 飞腾 D2000 ＞ 飞腾 E2000**。
排序依据：生态成熟度（唯一有一手确认的上游 TF-A 端口）+ 文档可得性 +
A/C 方案重启条件加分。**该排序不预决 owner 业务约束**（信创/国产化合规
要求可能使 D2000/E2000 成为硬性优先——此权重归 owner，本表如实留白）。
飞腾系两候选的 C10/C14 双高风险是共性阻断项：**若 owner 不能取得 TRNG 与
BSP 寄存器手册，则按 C10/C14 硬门直接出局**，与技术分无关。

## 4. 冻结流程（owner 签字后动作序列）

1. owner 在 §5 决策记录签字选定 SKU（可附条件，如「先取 TRNG 手册再定」）；
2. 按 `eneros.board-fact-freeze.v1` 填 `product/skus/<board>.json`：
   逐项 facts{value, verified:true, source:<手册页码/实测命令>}，
   缺真机事实的项进 missing[]——**禁 QEMU 地址/默认容量填空**；
3. 到货后按 bring-up 模板（WP-51.2）B0 板级事实采集产出实证，
   回填 sku JSON 并复跑 catalog derive/sign/validate；
4. catalog gate 维持 not-qualified 直至 B9 证据齐全（qualified 升格条件见
   模板 B9 与 43 卷资格矩阵）。

## 5. owner 决策记录

| 项 | 内容 |
|---|---|
| 裁决事项 | v1.0 首个目标板卡（SKU 冻结） |
| 候选输入 | 本文件 §2 评估表 + §3 初判排序 |
| 状态 | **待 owner 裁决**（2026-08-23 登记于 51 卷 WP-51.1；不阻塞后续 WP） |
| 裁决人 / 日期 / 签字 | ＿＿＿＿＿＿＿＿ / ＿＿＿＿ / ＿＿＿＿ |
| 附带条件（如有） | ＿＿＿＿＿＿＿＿ |
