# 硬件 bring-up 工作包模板 B0–B9（蓝图 51 · WP-51.2）

> schema: `eneros.board-bringup.v1` · 状态：**全步骤 BLOCKED（板卡未到位=owner 动作）**
> 建立：2026-08-23 · 基线 commit：51 卷 WP-51.1 提交 · 版本 0.313.41→0.313.42（WP Exit bump）
> 用途：板卡到位后，本模板逐项「转正」为正式 WP 执行（每步一个独立 WP+独立证据卷）。
> 前置链：owner 在 board-selection-criteria.md §5 签字 → 到货 → B0 起按依赖序执行。
> 判据映射：每步标注其验证的选板判据编号（C1–C16，见 board-selection-criteria.md §1）。

## 0. 通用验收纪律（每步 DoD 的组成部分，缺一即 NO_RESULT）

1. **防伪四要素**（本卷红线，真机声称的必要条件）：串口原始 log（含 board
   身份行）+ 时间戳 + 对端 transcript + 执行人登记。缺任一按 **NO_RESULT**。
2. **证据命名**：`evidence/<卷号>/board/<B 步>-<项目>.{log,json}`；串口 log
   注意 `.gitignore` 对 `*.log` 的忽略——入库须 `git add -f`（47/49 卷先例）。
3. **内存优先铁律**：新增目标侧负载前先静态内存审计（root 栈 256K/窗口
   4M→49 卷已扩 8MiB/堆 3M 基线），超预算先扩预算再跑（44-00 §4.4）。
4. **修复纪律**：每个缺陷先写能证明旧行为错的失败测试再修（44-00 §4.2）。
5. 每步完成须回填 `product/skus/<board>.json` 对应 facts 项
   （verified:true + source=实测命令或手册页码），并复跑 catalog 三连：
   `derive/sign/validate`。
6. **QEMU 等价物引用不是资格证据**：仅用于说明「该机制在合成平台上已被验
   证到什么程度」；真机差异风险列必须逐条处置后才算该步闭环。

## B0 板级事实采集（判据 C1–C6、C8、C14–C16）

**目标**：从真板采集串口/内存映射/中断拓扑/时钟事实 → 冻结 SKU json。

**工作包**：
1. 上电前核对：供电档位、启动介质跳线、调试 UART 接线与波特率；
2. 固件侧 FDT 导出（UEFI `dtbdump` 或 TF-A 阶段打印），与厂商手册对照；
3. RAM map / MMIO 窗口 / 中断路由 / 时钟树逐项登记；
4. 回填 `product/skus/<board>.json`（43 卷 `eneros.board-fact-freeze.v1`
   模板）：逐项 facts{value, verified, source}，未知进 missing[]——
   **禁 QEMU 地址/默认容量填空**。

**验收命令**（真机段）：
```bash
# ① 串口原始 log（含 board 身份行）——防伪要素①②
picocom -b 115200 /dev/ttyUSB0 | tee evidence/<vol>/board/b0-boot.log   # 执行人登记于 WP 文档
# ② FDT 事实导出与摘要
fdtdump <board.dtb > evidence/<vol>/board/b0-fdt.txt
sha256sum evidence/<vol>/board/b0-fdt.txt
# ③ SKU json 冻结后机器校验三连
cargo xtask catalog derive && cargo xtask catalog sign && cargo xtask catalog validate
```

**QEMU 侧已验证等价物**：`product/skus/qemu-virt-aarch64.json` 冻结事实 +
`configs/qemu-virt.dts`（358 行真实设备树）+ 43 卷 sku-matrix 流水线
（evidence/43/sku-matrix-status.json）。
**差异风险**：QEMU 地址/拓扑与任何真板都不同（复制即触红线）；真机 FDT 由
固件供给而非 repo 编译产物，digest 口径不同。

## B1 hal/board adapter（判据 C2、C14）

**目标**：新增 `<Board>Adapter`（对齐 QemuVirtAdapter 模式：FDT 解析+
MMIO 绑定表+cache 一致性判定）；产出平台契约 JSON。

**工作包**：
1. `crates/hal/board` 新增 `<Board>Adapter implements PlatformAdapter`
   （discover/inventory/DMA 一致性/时钟/复位句柄集，参照
   crates/hal/board/src/adapter.rs:38-60 的 QemuVirtAdapter 结构）；
2. cache 一致性判定：FDT `dma-coherent` 属性逐设备落实——真机上非一致
   设备必须走真实 cache 维护路径（38 卷合同：核心业务不分支板卡名）；
3. `configs/platform-contracts/<board>.json` 按 schema.json 登记
   （现有样例：phytium-d2000.json/qemu-virt-aarch64.json 占位待 B0 事实回填）；
4. host 单测：inventory 校验/一致性标注/中断句柄解析负例。

**验收命令**：
```bash
cargo test -p eneros-hal-board adapter          # host 单测
third_party/sel4-build-rust.sh                   # rootserver 重链接（改 root-task 后必须）
cargo xtask image --profile qemu-prod            # 装配校验
# 目标侧：串口出现 inventory 打印行（b1-inventory.log，含 board 身份行）
```

**QEMU 侧已验证等价物**：QemuVirtAdapter 全链（38 卷 WP-38.4：FDT 解析+
绑定表+Coherent 标注；33 卷 durable-log 掉电证据=一致性声明的验证依据）。
**差异风险**：QEMU virt 无 cache 模型（全一致假设在真机首次失效）；真机
GIC 版本/ITS 与 virt 模型不同；MMIO 槽偏移推导（39/41 卷 DMA 1:1 教训）在
非身份映射的真机 VA≠PA 下重做。

## B2 控制台+时钟（判据 C8 部分）

**目标**：UART 真 MMIO 跑通 println → time_init（跨进程时钟基准纪律）。

**工作包**：
1. PL011（或板级兼容控制器）以 B0 实测地址/IRQ 绑定（驱动面已有：
   crates/hal/hal/src/arm64/uart_pl011.rs——真寄存器实现，首次上硬件）；
2. 波特率/分频器按板级时钟计算并与实测对账；
3. time_init：跨进程时钟同基准（40 卷纪律：root 相对 vs 服务端绝对基准
   统一；EIPC 时间语义不得混用）。

**验收命令**：
```bash
# ① 启动横幅与自检行入串口 log（防伪四要素齐全）
picocom -b 115200 /dev/ttyUSB0 | tee evidence/<vol>/board/b2-console.log
# ② host 回归：时钟相关套件不回归
cargo test --workspace time        # 抽查时钟语义套件
# ③ 目标侧：双服务时间戳对账行（svc 心跳 vs root tick 同基准）入 b2-clock.log
```

**QEMU 侧已验证等价物**：QEMU virt PL011 全部 boot 串口证据
（evidence/47|48|49|50 各卷 boot/*.log）+ time_init 四修复（22 G-R1 系列）
+ 40 卷跨进程时钟同基准实弹。
**差异风险**：分频器源时钟不同（QEMU 固定 clock 频率）；16550 兼容口与
PL011 寄存器差异；流控引脚。

## B3 熵源【bring-up 第一优先】（判据 C10）

**目标**：TRNG 驱动接 `platform_entropy_source()`；health test 通过；
csrng 可构造。**失败=实机密码全灭，优先级高于一切后续步。**

**工作包**：
1. TRNG 寄存器驱动（BSP 手册为 source of truth——手册不可得=C10 硬门
   FAIL，退回 owner 重选板）；
2. health test：上电连续性/重复检测（SP800-90B 启发型测试），fail-closed
   保持（错误路径不得回退固定种子——crypto entropy 合同）；
3. 替换占位：PhytiumTrng/KunpengTrng 式占位改为真实现（现锚点：
   crates/security/crypto/src/rng/entropy.rs:44-58 恒 Unavailable）;
4. csrng 构造后跑 crypto KAT 全家（C3/C4 已有 SP800-90A/RFC6979 向量）。

**验收命令**：
```bash
# ① 目标侧熵自检行（health test PASS + 32B 采样非零/无重复块）→ b3-entropy.log
# ② KAT 报告（target 侧输出转储 + host 对照）
cargo xtask qa kat-report
# ③ host 回归不受影响
cargo test -p eneros-crypto rng
# ④ fail-closed 负例：人为屏蔽噪声源（跳线/寄存器封锁）→ 自检必须 FAIL 而非静默
```

**QEMU 侧已验证等价物**：virtio-rng 真队列 I/O（v0.209.0 P0-15；
crypto/src/rng/virtio_rng.rs）+ crypto-gmssl/openssl 互操作向量（21-B C1/C2）
+ SP800-90A/RFC6979 KAT（C3/C4）。
**差异风险**：virtio-rng 是宿主喂熵（无噪声源质量语义）；真机 health test
边界条件（上电冷启动/高低温漂移）无法在 QEMU 复现；寄存器手册可得性是
选板级风险（criteria C10 高风险项）。

## B4 存储（判据 C4、C5 部分）

**目标**：eMMC/NVMe 真驱动 → svc-storage 块链路 → 断电安全实测；
emmc.rs RAM 后端退役为纯测试件。

**工作包**：
1. eMMC（crates/drivers/storage/src/driver/emmc.rs 837 行骨架，RAM 后端
   :171-183 退役为 test-only）或 NVMe（nvme.rs 299 行未验证代码首次上真机）
   以 B0 事实绑定；
2. svc-storage 块链路贯通（41 卷移交模式：MMIO 槽偏移/DMA 1:1/drv_call
   4B-word/div_ceil 教训清单先行）；
3. 断电安全实测：写入中切电 ×N → durable-state/durable-log 一致恢复；
4. factory provision 真盘写入 + verify-record（50 卷工具首次真机化）。

**验收命令**：
```bash
# ① 真盘 provision（host 写整盘镜像→目标盘）
cargo xtask factory provision --image target/.../disk.img --device /dev/sdX
cargo xtask factory verify-record --record manufacturing-record.json
# ② 目标侧块读写回路行 → b4-blockio.log（读回 digest 对账）
# ③ 断电循环×10：写中切电→重启→durable 校验行 → b4-powerloss.log（继电器控电记录在对端 transcript）
# ④ OTA A/B trial 真机首跑（50 卷升级手册断电场景行）
```

**QEMU 侧已验证等价物**：emmc RAM 后端全命令面单测 + svc-storage 真块服务
装配（41 卷：MMIO 槽/DMA/4B-word）+ factory provision 四测试（50 卷，
host 盘镜像层）+ Genesis 双副本语义（harness reset 同源字节）。
**差异风险**：RAM 后端零掉电语义/零时序（真机 HS200/DDR 时序、坏块管理、
RPMB 分区全新）；virtio-blk guest 读写曾在 42 卷排查 qnum=0 BLOCKED——
真机不走 virtio 但同属块栈，接线缺陷清单需复查。

## B5 网络（判据 C6、C7）

**目标**：PHY+MAC 真驱动 → virtio 退役 → smoltcp 通 → Modbus L2 真机复测。

**工作包**：
1. MAC 驱动（新写；DMA 描述符环按 B1 一致性结论走对应路径）+ PHY MDIO
   协商；virtio-net 路径退役为开发平台专用；
2. smoltcp 栈在真 NIC 上连通（ARP/TCP/UDP）；
3. Modbus TCP L2 对端复测：46 卷独立 python peer 方法论原样搬真机；
4. （若 C7 加分兑现）PTP/gPTP 硬件时戳 ethtool 实证。

**验收命令**：
```bash
# ① 链路与栈：ping + TCP 回环 transcript → b5-link.log
# ② Modbus L2 独立对端（oracle=非 EnerOS 进程，46 卷四判据不变）
python3 evidence/46/modbus_slave.py &   # 或 45 卷 modbus_slave.py 按协议选用
cargo xtask run --profile qemu-prod ... # 真机版替换 qemu 参数；transcript → b5-l2-modbus-tcp/
# ③ 时戳实证（若适用）：ethtool -T <if> → b5-ptp.log
```

**QEMU 侧已验证等价物**：6 协议 L2 VERIFIED（evidence/46/l2-*-verified.json
绑 4f7344c）+ 45 卷采集数据面 QEMU 实弹。
**差异风险**：slirp 语义残留假设全部作废（半开 TCP 检测逻辑保留但触发条件
不同；ARP/广播/MTU/校验和卸载全新）；virtio legacy 头（RX 跳 10B/TX 前置）
退役时勿把补丁带进真 MAC 驱动。

## B6 总线（判据 C8、C9）

**目标**：485（含隔离与 DE-RE 方向实测）→ 645/101 真机对端；CAN（SJA1000
收发器回环 → 27930 BMS 模拟器 L1 → 真车/BMS 若可得）。

**工作包**：
1. RS485：DE-RE 方向切换实测（示波器/逻辑分析仪照片入证）+ 隔离耐压
   记录；modbus-rtu L2 真机复测（清偿 46 卷 blocked 项）；
2. DL/T 645-2007 / IEC101 真机对端——**前置依赖：48 卷 codec 字节层仍
   SpecBlocked（owner 规约原文未提供）→ 该子项 BLOCKED(owner) 先于
   BLOCKED(板卡)**；
3. CAN：SJA1000 驱动上板载控制器/模块 → 收发器回环 → gbt27930 plugin
   transport_kinds 填充（现 `&[]`，crates/protocols/gbt27930/src/plugin.rs:104,120）
   → BMS 模拟器 L1 → 真车/BMS（owner 资源）。

**验收命令**：
```bash
# ① 485 方向实测：DE-RE 波形图 + modbus-rtu L2 verdict → b6-rs485/
python3 tools/device_simulator/...    # 独立 rtu 对端
# ② CAN 回环：发帧=收帧 transcript → b6-can-loopback.log
# ③ 27930 L1：充电机侧报文注入 seam（CsmHost）对 BMS 模拟器 → b6-27930-l1/
```

**QEMU 侧已验证等价物**：**无 CAN 等价物**（QEMU virt 无 CAN——锚点⑤，
27930 真机验证挂本步）；modbus-rtu L2 在 46 卷如实 BLOCKED
（l2-modbus-rtu-blocked.json）——本步是它的唯一清偿路径；gbt27930
CsmHost 注入 seam 有 host 测试（字节解码 SpecBlocked 如实）。
**差异风险**：隔离/布线错误可损坏硬件（先假负载）；DE-RE 时序竞态在
QEMU 不存在；规约原文缺失使 645/101 子项即使板到位也无法转正。

## B7 看门狗/电源（判据 C5 部分）

**目标**：SP805 或板载 WDT 真喂狗+复位实测；掉电检测→优雅停机。

**工作包**：
1. watchdog crate（crates/drivers/watchdog/src/wdt.rs SP805 实现）上真机；
   停止喂狗→强制复位→复位原因寄存器读取；
2. 掉电检测 GPIO/PSU 通知 → 优雅停机路径（durable flush 后 halt）；
3. 复位原因进入审计/诊断流（50 卷故障手册排障树补充真机分支）。

**验收命令**：
```bash
# ① 喂狗正常期：运行 ≥10min 无复位 → b7-wdt-feed.log
# ② 停喂负例：kill 喂狗任务→T 秒内复位串口可见 + 复位原因行 → b7-wdt-reset.log
# ③ 掉电正例：PSU 通知→优雅停机行（flush 完成）→ 再上电 durable 校验 PASS → b7-powerloss.log
```

**QEMU 侧已验证等价物**：wdt.rs 单测 + root 探针可选机制（44 卷）+
`--reset-bootctrl`/VARS anchor 复用（29 卷 A/B 三态）。
**差异风险**：QEMU WDT 模型时序宽松（真机超时窗口/预缩放配置全新）；
复位原因寄存器 vendor-specific；PSU 通知硬件依赖板级设计（C5 选板必查项）。

## B8 安全启动（判据 C11、C12）

**目标**：ROTPK 熔丝烧录 → 安全启动全链（UEFI db/Ed25519 manifest）→
防回滚计数器接入 security_version
（boot-protocol/src/signature.rs:110-115 集成层供给点）。

**工作包**：
1. 固件信任链：TF-A/UEFI 侧 ROTPK 哈希熔丝烧录（**不可逆动作——先 dry-run
   校验脚本+二次确认**，owner 在场）；
2. boot-manager-uefi（crates/boot/boot-manager-uefi，Ed25519/SHA-256
   manifest）真机验证：正确签名放行/错签拒启/无签拒启三态；
3. 防回滚：单调计数器（eFuse/RPMB 按 B0 结论选型）落地 security_version
   存取；降级镜像端到端拒绝实测；
4. dev key（key_id=0xDD）release 拒绝在真机复现。

**验收命令**：
```bash
# ① 三态启动矩阵（fresh VARS 思想移植：每次试验前固化已知状态）
#    正签名 Accepted / 错签 Rejected / 无签 Rejected → b8-boot-matrix.log（含 board 身份行）
# ② 防回滚：security_version=N 放行→刷 N-1 镜像→SecurityVersionDowngrade 拒绝 → b8-rollback.log
# ③ keys gate 维绿（host）
cargo xtask qa gate keys
```

**QEMU 侧已验证等价物**：37 卷签名链+PIE 全链（PRODUCT-READY:true）+
signature.rs 全负例 host 测试（长度/域/降级/dev-key）+ 47 卷无键构建
fail-closed（exit=2）+ 42 卷信封重绑定 lineage。
**差异风险**：熔丝烧录不可逆且 QEMU 无法演练；UEFI db/变量存储真机持久
语义与 OVMF 不同；eFuse 编程电压/流程 vendor-specific。

## B9 soak/HIL（判据 C13、C15 及全表）

**目标**：72h soak（42 卷 harness 真机模式）、HIL 故障注入、掉电循环、
协议互操作真机复测 → catalog 该 SKU 升 qualified（证据齐全时）。

**工作包**：
1. tests/soak/soak72.sh + collect-evidence.sh 真机模式跑通（42 卷 harness
   扩展：qemu 基线换真机串口/电源控制通道）；
2. HIL 故障注入按 fault-catalog（evidence/42/fault-catalog.json）扩展真机
   注入器（电源/总线/网络三类通道）；
3. 掉电循环（≥100 次，B4/B7 结论的批量复核）；
4. 协议互操作全量真机复测（Modbus RTU/TCP、104、MMS/GOOSE/SV、MQTT；
   645/101/103/27930 视 B6/48 卷解封进度）；
5. qualification_evidence 逐项填充 → catalog gate 复核 → owner 签字升
   qualified。

**验收命令**：
```bash
# ① soak：72h 无值守（看门狗兜底开启）→ b9-soak/{serial,metrics,report}
tests/soak/soak72.sh --target hw --port /dev/ttyUSB0
tests/soak/collect-evidence.sh evidence/<vol>/board/b9-soak/
# ② 故障注入矩阵逐格 verdict → b9-fault-matrix.json
# ③ 资格升格机器校验
cargo xtask catalog derive && cargo xtask catalog sign && cargo xtask catalog validate
```

**QEMU 侧已验证等价物**：qa soak QEMU 基线（xtask qa soak，BLOCKED 语义
诚实分级：missing-report=真机浸泡未执行）+ 29 卷 A/B 三态引导矩阵 + 42 卷
harness PRODUCT-READY 修复链。
**差异风险**：QEMU 无热/EMC/长期时序漂移维度（环境舱/EMC 实验室=owner
外部资源）；soak 中的 flaky 判定需持久化串口为准（49 卷教训）；qualified
升格还需 L4 清单其余项（EMC/产线/认证/试点）——非本模板单方面可达。

## 转正流程（板到位后）

1. owner 宣布板卡与到货批次 → 本模板复制为当卷执行蓝本（B 步→WP 编号）；
2. 每 B 步独立 WP：测试先行+surgical 实施+DoD 实录+版本 bump+提交；
3. BLOCKED 转 ACTIVE 时在本卷 blocked-register（evidence/51/blocked-register.json）
   登记去向卷号，禁止静默消失；
4. 依赖序强制：B0→B1→B2 可并行度低依序执行；B3 完成前 B4–B9 不得开始
   （密码面全灭状态下的一切「通过」均无效）。
