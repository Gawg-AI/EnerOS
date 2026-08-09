# WP-27.1 QEMU 13 行启动断言复现记录

**日期**：2026-08-10
**命令**：
```bash
cargo xtask run qemu-prod \
  --image target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  --vars-template target/wp271-evidence-vars.fd \
  --evidence-dir target/wp271-evidence \
  --expect PASS --reset-bootctrl
```
（VARS 模板 = 64 MiB 全零；固件 = /usr/share/AAVMF/AAVMF_CODE.secboot.fd，
edk2-2025.11-3ubuntu7，sha256 aafedb40…，见 qemu-serial.log 首行）

**结果**：bootctrl Genesis 阻塞**解除**（`ENEROS stage=bootctrl-target-written
generation=1` → verified → root-task 常驻运行，200s 超时 kill 为 seL4 无
ACPI 关机的既有预期，CI 以 `|| test -s log` 容忍后由 K3.3 序列门判定）。

## K3.3 序列门逐行（13 行）

| # | 断言 | 结果 |
|---|---|---|
| 1 | `[eneros-root-task] Rust root task alive on seL4` | ✅ |
| 2 | `init_bundle @ paddr=` | ✅ |
| 3 | `spawn plan: root=` | ✅ |
| 4 | `spawn svc-hello entry=` | ✅ |
| 5 | `spawn svc-fault entry=` | ✅ |
| 6 | `EL0 services spawned` | ✅ |
| 7 | `IPC: send hello-req to svc-hello` | ✅ |
| 8 | `[svc-hello] IPC: recv` | ✅ |
| 9 | `IPC: recv hello:eneros:tick=` | ✅ |
| 10 | `IPC: send fault to svc-fault` | ✅ |
| 11 | `[svc-fault] FAULT` | ✅ |
| 12 | `svc-fault suspended (isolated)` | ❌ 差异登记 DIF-27.1-1 |
| 13 | `svc-hello still responsive` | ✅ |

日志：`target/wp271-evidence/qemu-serial.log`（82 KB，tick 至 151，fault 注入
2 次循环）。

## BOOTCTRL 闭环证据（真实字节）

| 阶段 | seq | 双副本 | CRC | 说明 |
|---|---|---|---|---|
| 运行前（脏盘，失败现场） | 1 | 一致 | 正确 | 上次引导遗留 → Genesis 合法拒绝 |
| `--reset-bootctrl` 归零后 | 0 | 一致 | 正确 | 修复版逻辑（decode→seq=0→encode） |
| 运行后 | 1 | 一致 | 正确 | boot-manager prepare_attempt+persist 闭环 |

字节导出：`evidence/boot-unblock/bootctrl-copy-1.bin`（失败现场）、
`bootctrl-post-run-copy1.bin`（运行后）；fixture 入库：
`crates/boot/boot-protocol/tests/fixtures/bootctrl-seq1-*.bin`，防回归测试
`tests/bootctrl_real_bytes.rs` 4 项全绿（含"无最终异或 CRC 必须被拒"）。
