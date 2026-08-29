# SPEC-SERIES-2026-08-28 · A/B/C 收口整验窗口留证（2026-08-28 晚）

## ⚠ mw-freeze/ · NEW P1：qemu-prod 启动确定性冻结（本整验抓出）

`mw-freeze/qemu-serial.{420s,720s}.log`：两次独立窗口（不同时长）串口**逐行一致**（13446 行止于 `[svc-control-dispatch] AUDIT-CHAIN-DURABLE fresh`，storage corr=45324 恒定）= 确定性冻结非慢启动。B.2 B1 改造前媒体（18:34 sha 4e82bca1）同样冻结 ⇒ 病灶在 A/B/C WIP 增量（E.2 dispatch 持久审计 × qemu-prod 线组合从未实弹过；2026-08-27 系列的 PRODUCT-READY 双复现均在 qemu-test 线；D 卷 window2（E.2 合入前）qemu-prod 可达 PRODUCT-READY）。qemu-test 线同代码全链正常（见下 win4）。冻结后全局静默（含 storage drv），候选面：storage EIPC 并发、审计区段扫描时序、readiness 信号丢失。**MASTER-WRITE 全链重放（B.2 DoD B3）被此冻结阻断。**



## 窗口

- 命令：`bash evidence/spec-2026-08-27-C/run_window.sh /tmp/abc-window-q1 target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw 420` + `verify.sh /tmp/abc-window-q1`
- 媒体：本会话 `xtask image qemu-prod` 全链重建（root-task ELF 含 SPEC-C.3 spawn 改动 + SPEC-B.3 svc-ai-worker 下沉首飞；sha256=4e82bca1d6e8984e730f82c91132d2e262b6c985f4608c9e84dbc2d38e970501，SOURCE_DATE_EPOCH=1785465257 + CI 三环境变量 + dev-signing.key）
- qemu_rc=124 = 420s 截断基线（c45 先例，周期演练不退出，以串口行为裁决）

## 判定（VERIFY.txt）

| 判据 | 值 | 说明 |
|------|----|------|
| storage_granted | 1 | EIPC-GRANT: svc-telemetry -> svc-storage slot=3 |
| mgmt_history_bound | 1 | `history EIPC bound (svc-telemetry)` |
| product_ready | ⚠ 见订正 | **初值混淆订正（2026-08-28 深夜）**：verify.sh 将 product_ready 初始化为 1，420s 窗串口实际**无** PRODUCT-READY 行（截断时 boot 未走完全队列——本机此媒体全就绪 >420s，29 卷"截断手动补全"先例）。720s 复跑窗为准，见 MW-VERIFY |
| panic | 1（无 PANIC） | 全队 420s 零 PANIC |
| no_grant_line | 1 | 无 TSDB-DISK-OPEN FAILED |
| tsdb_disk_open | 0 | **E-DEF-01 在案停用**（`TSDB-DISK DISABLED (E-DEF-01)` 诚实降级词形在串口）——磁盘时代判据，非回归 |
| hist_neg_frame | 0 | **DEF-52-05 在案出站缺陷**（broker 应答不可达，verdict=TIMEOUT；verify.sh 自注 BLOCKED-BY-DEF-5205） |

C-SMOKE-VERDICT: REVIEW——初判"可判据 5/5"有误：product_ready 1 为初值非实测。实测 = storage_granted/mgmt_history_bound/panic/no_grant_line 4 项真过 + product_ready 待 720s 窗。

## 目标侧新证据（本窗口首飞）

- SPEC-B.3 下沉后的 svc-ai-worker（137 行 bin）：`AI-PEER-CFG loaded … learn=true` / `backend=remote-brain` / `LEARN-STATE restored=1 current=0.100000 min=0.060000 max=0.140000` / `LEARN-THRESHOLDS charge=0.300000 discharge=0.500000`——lib 侧 build_learner/run_remote 目标侧真实执行。
- SPEC-B.2 dispatch_assembly：`RATE-GATE` / `protocol-outbound channel bound: slot=4 target=svc-modbus-tcp (OP_MASTER_WRITE)` / `DURABLE-INTENT-STORE open OK` / `AUDIT-CHAIN-DURABLE fresh base_block=24576 slots=64`。
- SPEC-C.3 spawn（handover-abi 收编）：`EIPC-GRANT-TABLE edges=33 base_slot=3`，授予链完整。

## 同会话 host 守门

- 全量 `cargo test --workspace --no-fail-fast` = 7801 passed / 0 failed（整验前基线 7795 + B.3 新测 6）
- `xtask build all` ×8 profiles PASS；clippy/fmt 净；lock 同步 152×0.314.20
- 版本 bump：0.314.19 → **0.314.20**
