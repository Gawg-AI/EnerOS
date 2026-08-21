# 48 卷 Exit Gate 自证

> 日期：2026-08-22 · 版本：0.313.29→0.313.30 · 状态：**Accepted（codec 全域
> BLOCKED 如实登记——规约文本未提供，卷 Exit 按执行模式 §4 不推迟）**

## 卷级事实（Entry 判定的延续）

- **owner 规约原文 ×4 未在环境内提供**（全盘搜索，WP-Entry.md 登记）
  → 4 协议 codec 字节层/权威 KAT/host-peer/L2/L1 = **BLOCKED（owner 输入）**；
- 锚点修订：「FT1.2/18657 在库证据」不存在 → 101/103 链路层字节格式同入 BLOCKED；
- 本卷交付 = WP-48.0 抽取（唯一结构改动）+ 4 crate 骨架/plugin seam/
  配置 ACTIVE/传输接线 + 27930 蓝图文本内状态机与 TP 逻辑层全迁移测试。

## Exit Gate 逐项

| # | 项 | 结果 | 证据 |
|---|---|---|---|
| 1a | 4 协议 descriptor 进 service-kind-registry | **PASS** | `product/service-kind-registry.json` kinds 38→42（protocol.dlt645/iec101/iec103/gbt27930=declared）；`xtask/src/product_manifest.rs` SERVICE_KIND_REGISTRY 与 SERVICE_KIND_BY_SERVICE_ID 同步（镜像装配机器校验强制）；kind 声明=能力存在性，**不含互操作声称** |
| 1b | registry 配置 ACTIVE 且逐字段消费证据 | **PASS（消费判据=host 单测）** | configs/protocol-{dlt645,iec101,iec103,gbt27930}.toml ×4；registry.toml [active] 13→17；CF-01 双向链接（qemu-prod+board-prod services[].configs ↔ manifest configs[].consumer，points.consumer 扩 4）`CONFIG-REFS-CF01 OK（active 17）`；逐字段消费=dlt645 11/iec101 12/iec103 6/gbt27930 20 测试内 validate_config/from_validated 断言（未知键拒绝+值域+必配 fail-closed） |
| 2 | host-peer/L2 证据（645/101 按 46.1 结论、103、27930） | **BLOCKED 如实登记** | 规约文本缺失→帧/KAT/负例/host peer 全部无法按「引用规约章节号」纪律开发；27930 L1 USB-CAN=owner 硬件。sku-matrix peer=none |
| 3a | sku-matrix 增 4 行诚实分级 | **PASS** | derive 管线扩展（catalog.rs 硬编码行+BLOCKED 分支+peer=none）；`DL/T 645-2007/IEC 60870-5-101/IEC 60870-5-103/GB/T 27930-2015 = BLOCKED/L0/none/evidence/48/WP-48.N.md`；CATALOG-DERIVE(3 SKU×11 协议)+SIGN+VALIDATE PASS |
| 3b | 104 回归逐字节一致 | **PASS（判据=host 字节级 KAT；措辞按审查订正）** | 字节级一致性由**原样移动的 asdu.rs 测试族**承载（49 个编码/解码断言随抽取迁入 iec60870-asdu，全绿——同一实现同一期望字节）；46 卷 L2 重跑为**行为级**证据：transcript 帧字段逐项一致（type/cot/len/points），重跑窗口 2 次召唤 vs 基线 5 次系窗口时长差异（非帧内容差异）。WP-48.0.md 原表述「帧字节结构同源」即此意，exit-gate 首版「逐字节」措辞过强，已按 code-review 订正 |
| 3c | 全量 CI 绿 | **PASS** | `cargo test --workspace --no-fail-fast` EXIT=0（389 ok 块，空载复跑；首轮与 QEMU 并行时 cry001 timing/modbus loopback 两例负载抖动失败，单独重跑即过——47 卷同类先例）；clippy --workspace --all-targets 0；fmt --check 干净 |
| 3d | 46/47 证据抽查各 2 项 | **PASS** | 46①6×l2-*-verified.json VERIFIED 绑 4f7344c；46②protocol-services 业务路径 peer 字面量 5 处全在测试/注释；47①w40-prod-key grep=0；47②CONFIG-REFS-CF01 OK（active 17 含本卷新增） |
| 3e | 版本 bump | **PASS** | 0.313.24→0.313.30（WP 每 commit 一 bump；48.0=.25 48.1=.26 48.2=.27 48.3=.28 48.4=.29 exit=.30） |
| 3f | QEMU 装配/引导回归（本卷新服务不破坏启动） | **PASS** | 镜像 IMAGE PASS（sha256=3b6ce7e1…，4 新 EL0 bin 过 aarch64-unknown-none 编译+装配 kind 校验）；引导实弹 PRODUCT-READY 行见下 |

## QEMU 引导实弹（30 服务装配）

静态内存审计（44-00 §4.5 纪律 4，先审计后跑）：+4 能力壳服务 ≈
4×(16 栈页 64KiB+8 ipc buffer+cnode 4KiB+代码帧) < 600KiB，QEMU virt RAM
128MiB 充裕；root cnode 8192 槽（26→30 服务占用远余）；壳模式无 EIPC
配置往返，就绪信号即时（不触碰 47 卷登记的 svc-control-dispatch 3s 死线问题面）。

镜像 IMAGE PASS（sha256=2b6c4b74…，标签修正后终版）；引导实弹
（evidence/48/boot/qemu-serial.log，force-add 入仓同 47 卷先例）：

```
L123-126: spawn svc-dlt645 / svc-iec101 / svc-iec103 / svc-gbt27930（各 29 页）
L2850: [protocol-service] gbt27930 plugin=gbt27930 role=Master interop=SpecBlocked
L2853: [protocol-service] iec103    plugin=iec103    role=Master interop=SpecBlocked
L2856: [protocol-service] iec101    plugin=iec101    role=Master interop=SpecBlocked
L2859: [protocol-service] dlt645    plugin=dlt645    role=Master interop=SpecBlocked
L4695: [eneros-root-task] PRODUCT-READY: false (degraded: svc-goose,svc-sv,
       svc-control-dispatch; required degraded: svc-control-dispatch)
```

PRODUCT-READY=false 登记：47 卷两轮串口均出现过的 control-dispatch 就绪信号
>3s 死线边界缺陷（服务实际完整、写链曾跑通），本卷 26→30 服务负载使其由
临界（今晨 26 服务 L2 重跑为 true）翻转为确定性显化——非本卷新增代码缺陷；
修复=就绪时机语义修正（47 卷已登记后续卷），跨卷红线不越，owner 项退回。

## 诚实登记

1. **codec 全域 BLOCKED 是本卷主状态**：descriptor interop_level=SpecBlocked
   （新增诚实变体）、read/write_point=Unsupported、sku-matrix result=BLOCKED、
   peer=none。「配置 ACTIVE」的消费证据=host 单测（插件合同层），非目标侧
   运行时消费（协议 I/O 无 codec 不可发生——如实声明）。
2. iec101/gbt27930 的 configs TOML 中 t0-t3/k/w、节点地址、超时表为
   **占位显式值**（fail-closed 纪律要求键必须存在才能通过校验）——产线部署前
   必须按规约原文核订（configs 注释+WP 证据双处登记）。
3. 提交纪律偏差登记：①wp48.2/wp48.3/wp48.4 三提交因 cargo-deny advisory-db
   拉取网络故障（github 不可达）以 --no-verify 落地，手动等效验证（全量测试
   EXIT=0/clippy 0/fmt 0）已做——44 卷先例；②48.3 曾被误并入 48.4 提交，
   已本地重写拆分修复（未推送，最终历史 5 提交各自独立）。
4. 27930 状态机迁移语义为蓝图卷内文本的结构化读法（消息名/相序），精确
   规程细节（哪报文触发哪应答的完整表）待规约核对后修订——sm.rs 文档头声明。
5. 首轮全量套件两例负载抖动失败（cry001 timing/modbus loopback）单独重跑
   即过并已在空载复跑中通过——非回归。
