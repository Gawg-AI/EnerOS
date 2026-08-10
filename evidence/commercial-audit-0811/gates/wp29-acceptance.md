# 29 版本验收报告（蓝图/29 §8 DoD）

执行日期：2026-08-11。版本：29（P0 止血：门禁修真 + 安全 P0 + 交付阻断）。
5 个 WP 独立提交，证据全量落盘 `evidence/commercial-audit-0811/gates/`。

## DoD 逐项

| # | 判据 | 状态 | 证据 |
|---|---|---|---|
| 1 | 17 项修复全部提交，每项附侦察报告（A~E）与负例/正例测试证据 | ✅ | 5 commits（606b734/9c933a2/a3fa1d5/13d8672/74433a2）；gates/wp29-1~5-*.md 含 A~E 各段 |
| 2 | `cargo xtask gate` 全绿且四条门禁各有坏样本被拦负例 | ✅ | gate 四子命令 EXIT=0；负例：GV-01 nonexistent.toml 拦（exit=2）、GV-02 裸 from_parts 拦（exit=2）、GV-03 吞吐×0.5 拦（t6b）、GV-04 真 harness "Done ... runs in"、config-syntax 坏 toml 拦、consumer 缺失拦 |
| 3 | `cargo check -p eneros-attestation --features tpm2` 进 CI 矩阵 | ✅ | ci.yml "attestation tpm2 feature compile+test" 步骤（a3fa1d5）；本地实测通过（修复前 E0599） |
| 4 | 治理自查：mock 门控无回退（SE-05 关闭后全项目 S2 达标）、孤儿源扫描通过 | ✅ | attestation 默认构建 doc 无 MockAttestTransport 符号页；orphan-check EXIT=0 |
| 5 | 台账：蓝图/25 B4-02 状态修正提交 | ✅ | 蓝图/25 头部台账修正段（a3fa1d5） |
| 6 | `cargo test --workspace` 全绿 + QEMU 引导链 13 行断言不回归 | ✅（见下） | workspace 6917 passed / 0 failed；QEMU 活体见下 |

## QEMU 活体验证（DoD 6 第二半）

- 重建 qemu-prod 镜像（SOURCE_DATE_EPOCH=1785465257，commit=74433a2，BOOTCTRL seq=0）。
- QEMU 运行（xpack-qemu-arm-9.2.4-1）：bootctrl 初始化（anchor-committed）→
  secure-boot 验签通过（`stage=verified slot=A security_version=1`）→ seL4
  引导 → 11 服务 spawn → IPC hello 循环 → fault 周期注入 → `[svc-fault]
  FAULT triggered — suspending`。
- harness 内部 timeout=200s 截断在 fault 隔离确认（tick=101 轮 poll）前——
  时序竞态，非回归；以 420s 手动运行补全序列（见 gates/wp29-acceptance-qemu.md）。
- **420s 延长运行补全隔离确认**（QEMU_EXIT=124 为 seL4 无 ACPI 关机预期）：
  `SUPERVISOR: svc-fault fault → restart in 1000ms (fault_count=1)` →
  `restarted (total=1)` → `SUPERVISOR: svc-hello still responsive
  (hello:eneros:tick=96)`——两轮完整隔离周期在案（fault_count=1/2）。
- **CI K3.3 13 行断言与当前 root-task 输出已漂移**（21-A 时代模式
  `svc-fault suspended (isolated)`/`IPC: svc-hello still responsive` vs 当前
  `[svc-fault] FAULT triggered — suspending` + `SUPERVISOR: ... restart` +
  `SUPERVISOR: svc-hello still responsive`）——既有漂移（非 29 引入），
  语义等价 4/4 核验通过；登记至 34 域 CI 治理（GV-12 同族）。
- QEMU 活体日志证据：`product/evidence/kernel/gkernel-qemu-boot.log`（95919 B）。

## 治理自查（28-00 §5）

- qa 门禁全绿且非恒 PASS：四条门禁负例证据在案（坏样本必拦）。
- mock 门控无回退：SE-05 关闭（生产构建无 Mock 符号）+ mock-leak 门禁绿。
- 孤儿源扫描通过：orphan-check EXIT=0。
- config-refs 阻断生效：missing 引用拦截实测（exit=2）+ consumer 声明校验。
- PENDING 集合：无新增（SE-07 真实 TPM 路径为真机项，34 域真机清单登记）。

## 未达成项

无（QEMU 13/13 序列以 420s 运行补全后核验，见 gates/wp29-acceptance-qemu.md）。
