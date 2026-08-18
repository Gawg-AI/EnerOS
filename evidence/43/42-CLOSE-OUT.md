# 42 收口记录（蓝图 43 执行时完成）

> 蓝图 43 执行前按用户指令"对 42 的部分功能进行收口"，对本卷可闭合的
> 42 未闭合项逐项核实并闭环；不可闭合项如实登记且不折算 PASS。

## 1. closure gate（Exit Gate #13 唯一 FAIL）— 已闭合

- **核实（本卷实跑）**：`cargo xtask closure gate --profile qemu-prod`
  → `CLOSURE-GATE PASS binaries=36 reached_packages=127 denied=0
  crypto_violations=0`。
- **决议**：CHG-2026-08-17-42-closure-sm2（负责人三选一之选项 1）登记于
  `product/crypto-policy.toml`（usages.device_identity / ota_signature /
  attestation：eneros-crypto SM2/SM3 入闭包，绑定 C1/C2 KAT + 10 组
  OpenSSL 互操作向量；`impl_dependencies=["ecdsa"]` 随主实现登记）。
- **绑定条件**：商密认证仍为 43 前置；评审不通过时三个 usage 必须回退
  `product_allowed=false`（可逆决策，防伪批准机制保留）。

## 2. orphan-check 与旧 evidence gate — 已闭合

- orphan-check：Round 131 已修复（文本候选 + 删声明编译验证双判据；
  20 个真死文件清偿；runtime_spec.rs 未跟踪豁免）；EXIT-GATE-STATUS #13
  记录转 PASS。
- 旧 G0/G1 evidence gate：release 证据权威移交 42 EvidenceEnvelope 层
  （check-current + self-test），任务卡降为信息报告。

## 3. evidence42 audit-all 重算 — 已闭合

- 本卷重跑 `cargo xtask evidence42 audit-all`：
  - `evidence/42/audit-all.json` conclusion=**READY**；
  - self-test 18/18 PASS（`target/eneros/evidence/42/self-test.json`）；
  - release-readiness PASS（mutation 13 用例、media-check、unsafe-audit
    89 文件/478 站点、repro、legacy-gates 全绿）。
- 旧 `final-audit.json` 中"failing legacy gates（closure/orphan/evidence）"
  已被重算取代；本轮收口后按最终状态重新生成 final-audit/gate-status。

## 4. candidate sv2 全介质复现缺口 — 书面 provenance 登记（RETURN_TO_OWNER 决议 b）

- **缺口事实**：当前候选（artifact c8c9dd45…，manifest security_version=2）
  由 42 round131 时的 profile 状态构建；当前工作树
  `product/profiles/qemu-prod.json` `minimum_security_version=1`，重建
  只能产出 sv1 候选（5d7f50af…）。分区级对比：SYS_A/SYS_B/CONFIG/DATA/
  LOGS/MODELS 字节一致；BOOTCTRL/BOOT_A/BOOT_B/PLATFORM_BOOT/RECOVERY
  不一致（security_version 差异所致）。
- **登记**：候选 provenance 记录于本文件（42-43 交接事实）：候选
  `0f75677e…/f7ec2539…/3b94d483…`（各轮 lineage）由 2026-08-17 round131
  期间含未提交 profile 状态（sv2）构建；该 profile 状态已不在工作树。
- **闭合路径**：36/41 owner 二选一——(a) 提升 qemu-prod profile 到
  minimum_security_version=2 并统一重生成候选与全部证据；(b) 维持本
  书面 provenance 登记。本卷采用 (b)（不越权改 profile；不改旧证据）。
- **root-task shim 跨环境确定性**：相同固定环境（xpack 交叉编译器显式
  PATH）双构建 BIT-IDENTICAL；跨环境 shim 字节不保证。
  `tools/setup-toolchain.sh`/CI 建议把 xpack 纳入 pinned-tools 显式 PATH
  （42 建议，本卷不越权实现）。

## 5. 剩余 BLOCKED（环境/外部，不折算 PASS）

| 项 | 状态 | 闭合路径 |
|---|---|---|
| Exit #4 Windows 本地实跑 | BLOCKED（无 wine/Windows 宿主） | CI matrix windows-latest（已配） |
| Exit #8 72h 软件 soak | BLOCKED（墙钟资源；3/5/12/100/231-cycle PASS、5000-cycle 后台实跑） | CI/lab 长跑 |
| Exit #5 CI 远端双构建 | BLOCKED（无 act；gkernel job 已加 WP-42.5 step） | CI 运行 |
| Exit #7 L2 协议独立对端 | **Modbus TCP 已闭合（本卷 43 收口）**：svc-modbus-tcp 产品端点 + virtio-net 槽8 移交 + smoltcp ↔ 独立 raw 从站 VERIFIED（evidence/43/l2-modbus-tcp-verified.json）；IEC104/MMS/GOOSE/SV/MQTT/Modbus-RTU 仍 BLOCKED（同类接线推广属 39/40） | 已闭合路径即同构复制到其余协议 |

## 6. 43 前置义务提醒

- `product/crypto-policy.toml` 的 eneros-crypto 三个 usage：43 密码学
  独立评审不通过时须回退 product_allowed=false（CV-42-1 回退方式）。
- Modbus L1 raw peer 证据（evidence/42/modbus-result.json）与 interop
  matrix 其余 PENDING 状态如实保留，不上推。.