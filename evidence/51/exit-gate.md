# 蓝图 51 卷 Exit Gate 自证（2026-08-23）

> 版本链 0.313.40→**0.313.44**（WP-51.1=.41 / WP-51.2=.42 / WP-51.3=.43 /
> exit=.44）；基线 commit 9d4144c；WP 提交链见 exit-gate.json `commit` 字段。

## Gate 1 选板判据+评估表+owner 决策记录 —— PASS

- `product/skus/board-selection-criteria.md`：判据表 **C1–C16** 按卷蓝图表格
  逐项展开（基础/存储/网络/总线/安全/生态六类；硬门一票否决+加权 W1/W2/W5；
  每条带 44–50 卷软件需求依据列）✅
- 三候选评估表（飞腾 D2000/E2000/RK3588）逐判据事实+source+verified+初判+
  B0 必核项 ✅；owner 提名空槽 ✅；kunpeng-920 存量行处置说明 ✅
- 评分汇总乐观\*/保守双口径+结构性风险列；初判排序 RK3588＞D2000＞E2000，
  显式声明不预决 owner 信创业务权重 ✅
- **owner 决策记录=「待 owner 裁决」登记**（卷首执行模式条款：不停等答复，
  签字栏已备）✅
- 事实纪律：三级标注（repo 内证据/一手来源/公开规格·待 B0 核）；一手来源
  实录=TF-A 文档 2026-08-23 抓取（rk3588 标准端口存在 Octa-Core A55/A76；
  phytium.html 404=上游无 Phytium 平台页负面结论）；厂商站直取失败实录登记；
  **零默认容量填空、零编造数值** ✅
- NPU 定位如实：ADR-0045 方案 C 可选输入+WP-49.3 spike NO-GO 后的 A 方案
  重启条件（evidence/49/exit-gate.json exit-2），非 v1.0 需求 ✅

## Gate 2 bring-up 模板 B0–B9 完整 —— PASS

- `product/skus/bring-up-template.md` 十步齐全；**每步含验收命令**（真机段
  具体到设备节点/脚本路径/产物命名）+ QEMU 侧等价证据引用（全部指向在库：
  38 adapter 模式/47–50 boot 串口/40 时钟基准/21-B KAT/41 块链路/50 factory/
  46 L2×6 绑 4f7344c/37 签名链/29 A/B 三态/42 harness）+ 差异风险 +
  判据映射 C1–C16 ✅
- CAN 步骤如实写「QEMU 无等价物」（锚点⑤）；B6-RS485 登记为 46 卷
  modbus-rtu blocked 的清偿点 ✅
- §0 通用纪律：防伪四要素（缺任一 NO_RESULT）+串口 log gitignore 需
  `git add -f` 先例提醒+内存优先铁律+B3 第一优先强制前置（B3 完成前
  B4–B9 不得开始）✅
- B8 ROTPK 熔丝不可逆动作 dry-run+二次确认+owner 在场纪律 ✅
- 引用路径真实性逐一核查（platform-contracts/boot-manager-uefi/tests-soak/
  fault-catalog 等）见 WP-51.2.md ✅

## Gate 3 BLOCKED 登记表与 catalog 一致 —— PASS

- `evidence/51/blocked-register.json`（eneros.blocked-register.v1）：15 项
  三类 blocked_kind（owner_action ×3 / board_absent ×10 /
  owner_action_then_board ×2）；每项 dependency+expected_outputs+template_ref
  锚点+criteria 映射 ✅
- 对齐实测（2026-08-23）：`cargo xtask catalog validate` → **PASS**；
  三 SKU 状态与 statuses_expected 全等（kunpeng-920/phytium-d2000/
  qemu-virt-aarch64 均 not-qualified）**ALIGN PASS** ✅
- `cargo xtask catalog gate` 单违规 `evidence42-check-current-not-all-pass`
  **如实登记为既有怪癖而非本卷回归**：唯一来源 run_check_current
  （xtask/src/catalog.rs:1035-1037）；44 卷已登记 moved-HEAD 恒 NOT-ALL-PASS
  怪癖；50 卷 Exit 即以 CATALOG-VALIDATE 为 catalog DoD 口径，本卷沿用 ✅

## Gate 4 CI 绿与回归 —— PASS（deny=offline 等效如实登记）

| 检查 | 结果 |
|---|---|
| `cargo test --workspace --no-fail-fast` | **7586 passed / 0 failed，ok 块 389**（与 50 卷口径一致）|
| `cargo fmt --all -- --check` | 干净 EXIT=0 |
| `cargo clippy --workspace --all-targets` | 0 warning/error |
| `CARGO_NET_OFFLINE=true cargo deny check advisories` | **advisories ok**（在线 deny 网络故障，44/47/48/49/50 先例等效口径）|
| closure gate qemu-prod | binaries=32 reached=122 denied=0 PASS |
| CF01 config-refs | OK（active 18，templates 74，deleted 3）|
| keys gate | tracked-keys=0 / default-refs=0 / test-keys=present / production-injection=fail-closed 全 PASS |
| CATALOG-VALIDATE / policy adr | PASS / ADR-REGISTRY-GATE PASS entries=41 |

- Entry 上卷回归（§4.5.2 ≥2 项照抄）：closure gate/catalog validate/CF01
  三项实跑全 PASS 且数值与 50 卷登记一致 ✅
- 上卷抽查：①49 卷 boot 串口日志 ×3（exit/run15/rereverify）git ls-files
  在库（补正项维持）✅ ②50 卷 factory 四测试实录 wp503-tests.txt 在库 ✅

## Gate 5 版本与证据落盘 —— PASS

- 版本链 0.313.40→0.313.41→0.313.42→0.313.43→0.313.44（bump-version 工具
  四次，Cargo.lock 同步）✅
- evidence/51/：WP-Entry.md / WP-51.1.md / WP-51.2.md / WP-51.3.md /
  blocked-register.json / exit-gate.{md,json} ✅
- 44-00 §6 状态板 51 行置 **Accepted** ✅

## 诚实登记（不满足/受限项）

1. **真机执行项零执行**：全 BLOCKED（owner 动作）——本卷无任何真机声称，
   无 NO_RESULT 触发；不得视为「硬件就绪」任何形式的进展宣称。
2. **候选规格外部核验受网络限制**：phytium.com.cn 两页 404、rock-chips
   产品页/Wikipedia/U-Boot docs 超时（2026-08-23 实录）→ 三候选除 TF-A
   一手确认外的规格全部 verified=false 待 B0 核；RK3588 初判优势建立在
   「上游生态存在性」这一可核事实上，细节规格仍须 B0 全量实证。
3. **catalog gate FAIL（信封怪癖）**：如 Gate 3 所述，evidence42
   check-current moved-HEAD 恒 FAIL，与本卷无关且非本卷可修（重绑信封=
   CI 流程职责）。
4. **pre-commit --no-verify ×3**（51.1/51.2/51.3 提交）：cargo-deny 在线
   网络故障（audit 200s 超时特征）；每次提交前 CARGO_NET_OFFLINE=true
   advisories ok 等效验证；51.1 提交时 fmt/clippy/test/version-gate/
   mock-gate 五检实跑全绿；Exit 时点全量六检等效复跑（本 Gate 4 表）。
5. **owner 决策未决**：SKU 冻结签字、TRNG/BSP 手册获取、规约原文×4、
   CAN 硬件采购、环境舱/EMC 外部资源均登记 owner 动作；kunpeng-920 存量行
   处置（保留/除名）随冻结裁决一并。
