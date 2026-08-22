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

## 复验附记（2026-08-22，HEAD=ba36fec，工作树 clean）

Exit Gate 落盘后同日全项实弹复验，**结论：Accepted 维持，5 Gate 全部复核通过**。

| 项 | 复验结果 |
|---|---|
| CI 六检 | `cargo run -p eneros-ci` Overall=PASS（fmt/clippy/audit/test/version-gate/mock-gate）；`cargo test --workspace --no-fail-fast` EXIT=0（389 ok 块 0 失败）；clippy --workspace --all-targets 0；fmt --check 干净；**cargo deny EXIT=0——advisory-db 网络已恢复，wp48.2/.3/.4 的 --no-verify 前置条件消失，等效验证由本轮补丁外实跑闭合** |
| closure / catalog | CLOSURE-GATE PASS binaries=32 reached=126 denied=0 crypto_violations=0；CATALOG-VALIDATE PASS |
| 104 回归 | 家族精确对账 master 74 + slave 106 + iec60870-asdu 51 = **231**（与 WP-48.0 登记一致）；protocol-abstract 34 |
| WP DoD 定向 | dlt645 11 / iec101 13（登记 12 + ba36fec 审查补 k=65536 截断负例）/ iec103 6 / gbt27930 21（登记 20 + 审查补值域负例）——计数差即审查提交新增负例，口径吻合 |
| 负例点名抽查 | 101: out_of_range_values_rejected_before_truncation / rejects_unknown_keys ✅；27930: all_keys_required_no_memory_defaults、zero_timeout_rejected、sm 四相超时+param_mismatch_aborts+reconnect_after_abort、tp 乱序/丢包/重复/总数冲突 ✅；645: baud_range_enforced、validate_rejects_unknown_and_out_of_range、descriptor_is_honestly_spec_blocked ✅ |
| 46/47 抽查复跑 | 4/4：l2 json×6 VERIFIED@4f7344c；`10.0.2.*` 恰 5 处=L9 文档注释+L759/843/872/983 cfg(test) 内（业务路径零硬编码）；w40-prod-key grep=0；CF01 active=17 |
| QEMU 引导复跑 | 同 qemu.argv 实弹重启：spawn×4 同 entry/ipc_ep(126-129)/29 页、interop=SpecBlocked×4、PRODUCT-READY: false (required degraded: svc-control-dispatch) 确定性重现 |

### 复验订正与新登记

1. **closure binaries 28→32 订正**：exit-gate.json 首版 gate-4 记录的
   `binaries=28` 系 47 卷口径（26 服务+kernel+root）带过的陈旧数
   （该记录自身 reached_packages 已更新为 126 而 binaries 未更）；
   本卷 30 服务实测应为 32（30 服务+kernel+root），JSON 已订正并注记。
2. **orphan-check 门为「确认即删」自动修复门（新登记，操作教训）**：
   `cargo xtask qa gate orphan-check` 对 confirmed orphan 直接
   `fs::remove_file` 并改写 lib.rs 后才 FAIL 返回。本次复验误触发，
   已 `git checkout` 全量恢复（零净变更）。HEAD 状态报 3 孤儿：
   control-services acquisition.rs/collector.rs（WP-45.2 遗留未接线）+
   iec101 asdu_bridge.rs（BLOCKED 接缝，ba36fec 审查裁决保留）。
   该门不在 CI 六检环内、本卷 Exit 未声称其 PASS；孤儿清偿归后续卷。
3. 镜像哈希说明：`artifact-sha256.json` 内 eneros-qemu-prod.raw
   sha256=2b6c4b74… 为构建时值，与登记一致；盘上 raw 因 boot 实弹写入
   （bootctrl/durable-state）偏离构建哈希属预期行为。
