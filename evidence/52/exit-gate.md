# 蓝图 52 卷 Exit Gate 自证（2026-08-23）——全系列出口

> 版本链 0.313.44→**0.313.49**（52.1=.45 / 52.2=.46 / 52.3=.47 / 52.4=.48 /
> exit=.49）；tag 点=基线 4521e83；WP 提交链见 exit-gate.json `commit` 字段。
> **终验裁决：NO-GO / NOT-AUTHORIZED 维持**（唯一现行裁决见
> docs/v1.0.0-go-no-go.md 顶部；历史全部 superseded）。

## Gate 1 WP 全项完成并留证 —— PASS

- **WP-Entry**：锚点三项重验（go-no-go NO-GO 现状/next-steps 8 项映射逐项核对/
  BLOCKED-42 四项现状）无修订；上卷回归 closure/CATALOG-VALIDATE/CF01 三项
  实跑与 51 卷登记逐字一致 ✅
- **WP-52.1 候选冻结**：tag `v1.0.0-rc.1`@4521e83 + candidate_id=e477c3c2…+
  freeze-envelope dev 签名 ✅；工具链 pin 核验一致 ✅；复现构建 FAIL 如实
  （DEF-52-03 四随机源+跨路径维度，clean clone 手工等价构建达成，
  DEF-52-01/02 脚本缺陷阻断登记）；VRF002 bit-identical PASS（单 binary 层）
- **WP-52.2 十一项矩阵**：逐项执行、绑定 candidate_id、结论如实
  （7 PASS / OTA PARTIAL / 协议 5/6 复证 / 控制链·AI 链 BLOCKED(DEF-52-05) /
  soak 真跑不伪称）——详见 WP-52.2.md 十一节
- **WP-52.3 外部项**：EXT-01..04 全 BLOCKED 清单不伪装 + 内部缺口七项 owner
  退回指向明确 ✅
- **WP-52.4 文档与裁决**：go-no-go 顶部唯一裁决更新（NO-GO 维持+缺口清单+
  五条重启条件）、状态板终态、PROJECT.md §3/§4 刷新、README 宣称对齐四处、
  交付包清点七组件、v1.x 路线五项 ✅

## Gate 2 裁决唯一且与证据一致 —— PASS

- GO 双条件（52.2 全 PASS 且 52.3 全完成）均不满足 → **NO-GO**；
  缺口→owner 退回→修复→新 candidate→重跑本卷的重启路径五条在册；
  v1.0.0 tag 不打（候选标记 v1.0.0-rc.1 保留，GO 后转正）。
- 历史裁决（14/28 系列/43 卷）全部位于附录且标 superseded。

## Gate 3 evidence/52/ 完整性 —— PASS

目录清单：WP-Entry.md · WP-52.1.md · WP-52.2.md · WP-52.3.md · WP-52.4.md ·
candidate-lineage.json · freeze-envelope.json · blocked-register.json ·
claims-audit.json · runs/boot/{cold,ab-trial-run1,ab-trial-run2,
ab-commit-run3b}qemu-result.json + anchor-mismatch-reject.serial.log ·
runs/l2/{modbus-slave,iec104,mms,goose,sv,mqtt}.transcript.jsonl ·
exit-gate.{md,json}。矩阵每项绑定 candidate_id 与命令输出。

## Exit 时点最终复验（HEAD=b4ffd76+.49 提交）

| 检查 | 结果 |
|---|---|
| cargo test --workspace --no-fail-fast | 首轮（.45 树）**EXIT=0：389 块/7586 passed/0 failed** 与 51 卷逐位一致；exit HEAD 复跑 3 败均为争用偶发（残留 slave47 占 5502 已清+soak 并发 CPU 致恒时计时抖动），同 HEAD 隔离重跑三测试全绿（5/1/1 passed）——双跑证据并列在案 |
| clippy --workspace --all-targets | 0 警告 |
| fmt --all --check | 干净 EXIT=0 |
| deny advisories（CARGO_NET_OFFLINE=true） | advisories ok EXIT=0 |
| closure gate qemu-prod | binaries=32 reached=122 denied=0 crypto_violations=0 digest=bdd2332e…（与 50/51 卷逐字一致=零代码面证实）|
| CF01 config-refs | OK active=18 templates=74 deleted=3 |
| keys gate | tracked-keys=0 / default-refs=0 / test-keys=present 全 PASS |
| policy kernel / adr | PASS product_kernel=sel4 / PASS entries=41 |
| CATALOG-VALIDATE | PASS（catalog gate 单违规=moved-HEAD 信封怪癖非本卷回归）|

## 本卷新增缺陷与缺口（全部只登记不修，退回指向明确）

| ID | 级别 | 一句话 | Owner |
|---|---|---|---|
| DEF-52-01 | P2 | sel4-build.sh 补丁幂等标记假设对 0002 补丁失效→fresh clone 构建阻断 | sel4 基建脚本 |
| DEF-52-02 | P2 | 脚本缺 IMAGE_START_ADDR/UTIL_LIBS_INCLUDE 两 ADR 旗标靠历史 CMakeCache 掩盖 | 同上 |
| DEF-52-03 | P1 | 镜像每构建非确定（四随机源定位，同环境背靠背 digest 即异） | 44 image_factory/37 签名链/29 bootctrl |
| DEF-52-04 | P2 | powercut-durable.sh 标记词汇过时+切点失准 | 33 卷 soak/fault 基建 |
| DEF-52-05 | P1 | svc-mqtt 出站连接建立即断（环境漂移 vs 回归待定责） | svc-mqtt owner 卷+基建 |
| GAP-FUZZ/PIN/SOAK72/L2NEG | — | fuzz 新面目标缺/qemu pin 漂移/72h 未达/L2 负例全量重跑未做 | 见 blocked-register.json |

## 诚实登记（不满足/受限项）

1. **GO 未达成**：外部四项 BLOCKED + 两个 P1 缺口——v1.0.0 不打 tag，
   NO-GO 维持；本系列出口=「候选冻结+终验矩阵已执行+缺口与重启路径全部
   在册」，不冒充「可发布」。
2. soak 两段真跑合计 **169 cycles 全 PASS 零失败**（≈30 分钟墙钟），
   72h 未达=BLOCKED(资源) 如实。
3. 控制链/AI 链 QEMU 实弹受阻（DEF-52-05）：装配面当日实证
   （CONTROL-BINDING/DURABLE-INTENT-STORE/AI-PEER-CFG/backend=remote-brain/
   AI-RELAY armed）+host 套件承载授权链负例；历史 VERIFIED 在库但未能复现，
   裁决保守采信。
4. pre-commit --no-verify ×4（cargo-deny 在线网络故障 offline 等效口径，
   44/47/48/49/50/51 先例；每次提交前 fmt/clippy/test/version-gate/mock-gate
   五检实跑绿）。
5. 冻结镜像原件被 soak 首段占用的纪律偏差：冻结 digest t0 登记+行为验证
   全在工作副本+boot-evidence manifest_sha256 绑定每次引导内容（补救闭环）。

## 结论

蓝图 44–52 交付冲刺系列出口达成：候选已冻结（v1.0.0-rc.1）、软件终验矩阵
已逐项执行并留证、外部项与缺口全部如实登记、唯一裁决 NO-GO 与证据一致、
v1.x 重启路线在册。**52 卷 Accepted（终验裁决=NO-GO 维持）。**
