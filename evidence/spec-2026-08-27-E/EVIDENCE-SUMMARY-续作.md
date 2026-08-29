# SPEC-SERIES-2026-08-27/E · 证据汇总——续作篇（2026-08-28）

> 本篇承接 2026-08-27 的 [EVIDENCE-SUMMARY.md](EVIDENCE-SUMMARY.md)：记录"多分块
> INGEST Transport"缺陷的根因链、四层修复与 target 实弹复验。上篇的 host 判据
> 全部继续有效（本轮全量 **7779 绿 / 0 失败**，基线 7777 只升不降）。

## 〇、结论速览

| 事项 | 状态 |
|------|------|
| 多分块 TEL_INGEST → svc-telemetry Transport（上篇缺陷①） | ✅ **根因修复并实弹打穿**（三层根因，见 §一） |
| E.3-C3 学习闭环（qemu-test + synth） | ✅ C3①LEARN-APPLY 序列 + ②强杀重启恢复逐字节一致（win4） |
| 根环编排节奏塌陷（上篇缺陷②） | ⚠ **未修**——prod 编队启动吞窗（见 §四），归调度·驱动层卷 |
| E.2-C2/C3、E.1-C2（依赖意图流） | ⏳ 取决于节奏塌陷，win5 最终验证结果见 §三 |
| E-DEF-01：littlefs 磁盘 TSDB 写路径 target 不可用（C 卷域） | 🆕 本轮发现并显式停用（§二） |
| E-DEF-02：RegionStateStore 单槽原地写不抗撕裂 | 🆕 本轮修复（双槽代次切换，§二） |

## 一、Transport 根因链（三层叠加，全部实弹定位）

```
[svc-modbus-tcp] INGEST eipc=Transport        ← 表象（E 卷上篇登记）
  ① svc-telemetry 默认栈 16 KiB，TSDB 首写链（LFS mount_and_then +
     checkpoint 深帧）栈溢出（host 同构复现：16-28 KiB 溢出 / 32 KiB
     存活，守门测试 telemetry_history::diag_stack_write_path_survives）
     → 线程死；panic handler 是静默自旋 → 零痕迹
     → INGEST 无回执 → root 卡死 EXECUTE Call → 全系统静默冻结
     （win1-t2 串口：mount 后零块 IO、零打印、日志戛然而止）
  ② 磁盘 TSDB 写路径架构性慢（修①后暴露，面包屑插桩 win4-diag 实测：
     单 LFS 操作 2k-13k 次块命令往返 = 串口 6k-40k 行，单 checkpoint
     分钟级）——TEL_INGEST 5s deadline 物理不可满足 → E-DEF-01
  ③ 学习快照 RegionStateStore 单槽原地覆写：硬杀落进写窗口即撕裂唯一
     记录 → Corrupt → 学习器 fail-closed（win4 Run2 盘上实证：payload
     逐字段完好而 CRC 撕裂，gen=901/current=0.13951424/samples=900 与
     Run1 串口逐位吻合）→ E-DEF-02
```

## 二、修复清单（本会话，全部 host 测试在守）

| # | 修复 | 锚点 |
|---|------|------|
| 1 | svc-telemetry 栈覆盖 16→64 KiB（按名覆盖机制，BP-40 先例；≥2× 实测余量） | `root-task/src/main.rs` `SERVICE_STACK_OVERRIDES` |
| 2 | 4 个服务 bin panic handler 静默自旋 → 打印后自旋（死亡可见；svc-device-agent 词形先例；其余 ~20 bin 同病待后续卷） | `svc-{telemetry,energy-agent,ai-worker,control-dispatch}.rs` |
| 3 | 磁盘 TSDB target 装配显式停用（**E-DEF-01**）：内存环接替（TSDB-COMMIT 基线词形），TELH 回 HIST_UNAVAILABLE（既有降级词形）；接缝代码+读面+host FakeStorage 全链测试保留，修复后单点恢复 | `svc-telemetry.rs` `_start` 装配段注释 |
| 4 | RegionStateStore 双槽代次切换（**E-DEF-02**）：写恒落非最新槽，open 取 CRC 合法最大代次，撕裂槽天然忽略；撕裂最新写恢复上一完整代次的回归测试在守 | `durable-state/src/lib.rs` + `tests/durable.rs`（17 绿） |
| 5 | BP-50 遗留强制 `storage/diag` 的**摘除尝试失败并回滚**：摘除态与保留态各复现一次 prod fleet init 中途串口冻结——BP-50"环境性启动冻结"=**间歇性** virtio 时序竞态，与 diag 无必然因果；diag 维持保留（摘除无收益徒增变量），31× UART 税（BP-50 登记）与间歇冻结一并归调度·驱动层卷 | `drv-svc/Cargo.toml` 注释定案 |
| 6 | closure gate 转红订正：littlefs2 `c-stubs` 被 deny 子串 "stub" 误伤（C 卷接线引入的预存红灯）；走 allowlist 正途登记（非 mock 设施，2026-12-31 到期） | `product/closure/feature-allowlist.toml` |
| 7 | 守门测试：栈预算（`diag_stack_write_path_survives`，`STACK_DIAG_BYTES` 可参二分） | `control-services/src/telemetry_history.rs` |

## 三、target 实弹复验（镜像 sha256 见各 window 目录 run 日志）

### win4 —— qemu-test + qemu-test-synth（`window4-synth.sh`，数据盘同目录复用=重启续链）

| 判据 | Run1（fresh，420s） | Run2（同盘重启，240s） |
|------|--------------------|-----------------------|
| INGEST Transport（须 0） | **0** ✓ | 0 ✓ |
| CONTROL-CHAIN 轮数 | **1653**（ingest=6/轮） | 1670（继续） |
| TSDB-COMMIT（内存环词形） | 1655 | — |
| PANIC（须 0） | **0** ✓ | 0 ✓ |
| LEARN-APPLY 序列 | **7**：0.110000 → … → 0.139514（EMA 收敛至 catalog 上界 0.14，与设计预测 ~0.1396 一致） | 0（已在上界，继续拒绝） |
| LEARN-REJECT-CAP（安全阀） | 1646 | 1670 |
| **E.3-C3② 重启恢复** | boot：`LEARN-STATE restored=1 current=0.100000`（出厂） | **`LEARN-STATE restored=1 current=0.139514 … samples=1653 rejected=1646`——与 Run1 末次 APPLY 逐字节一致** ✓ |

注：E.3-C3③"压在旧死区外/新死区内的提案对照帧"未在 target 构造——synth 振荡
（0.475↔0.525）全程落在学习后带内（agent=NOACTION），边界钳制由 REJECT-CAP
序列（cap 0.14 生效）+ ai_worker e3/e4 host 判据共同证明；精确边界提案构造留
prod 窗（SCADA SET 载荷可编程）。

### win5 —— qemu-prod 全链（`window5-audit.sh`：SCADA 意图 + 振荡从站 + peer 围栏）

**未打穿，如实登记**（`window5/` 留存冻结现场）。三次尝试：
1. diag-on 540s：编队启动吞窗——串口止于 dispatch 审计区扫描，root tick 仅 ~1 次，意图期未达；
2. diag 摘除 540s：fleet init 中途串口**冻结**（~13.4k 行）；
3. diag 恢复 1500s + SCADA 延迟 700s：再次**间歇冻结**于同一点（`DURABLE-INTENT-STORE open` 后，`[drv-svc-storage]` IPC 风暴中，串口 9+ 分钟零增长，QEMU 存活）。

结论：E.2-C2/C3、E.1-C2 的 prod 证据被**间歇性启动冻结**（BP-50 登记项，virtio
时序竞态，本次新增两次复现样本：摘除态/保留态各一）+ 编队启动吞窗双重阻断。
冻结现场 = `target/evidence-e/window5/qemu-serial.log`（尾部：audit 区挂载后
storage IPC 流中）。修复归属调度·驱动层卷（与节奏塌陷同根）。

## 四、遗留（登记不顺手修）

| id | 事项 | 归属 |
|----|------|------|
| E-DEF-01 | littlefs2 每操作 mount_and_then × BlockIo 52B 分片：单 LFS 操作 2k-13k 块命令往返，checkpoint 分钟级——磁盘 TSDB 在 target 不可用（代码保留待修；方向：burst 块协议 / PAYLOAD_CAP 扩容+全量内存重审 / eneros-fs 持久 mount） | C 卷 owner |
| E-DEF-02 残留 | 双槽仍存"连续两次写均撕裂"（概率²）→ Corrupt 的理论残余；4KiB 区仅 2 槽 | 后续卷（可扩槽） |
| 节奏塌陷 | prod 编队 fleet init 的块 IO 风暴 + diag UART 31× 税 + virtio 时序敏感（摘 diag 即冻结）三合一——根环 tick 冻结于此，E.2-C2/C3、E.1-C2 的 prod 窗口依赖其根治 | 调度·驱动层卷（BP-50 冻结登记的收口卷） |
| panic handler | ~20 个服务 bin 仍静默自旋（本会话修 4 个窗口链路上的） | 后续卷批量 |

## 五、复现指令

```bash
# host 全量（7779 绿）
cargo test --workspace --no-fail-fast
# 栈守门（宿主同构复现 16-28KiB 溢出 / 32KiB 存活）
STACK_DIAG_BYTES=16384 cargo test -p eneros-control-services --lib diag_stack_write_path
# 镜像（qemu-test 自动启 synth feature；qemu-prod 需 SIGN_KEY/KMS_DIR）
SOURCE_DATE_EPOCH=1755523200 PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$PATH" \
  cargo xtask image raw qemu-test --signing-key product/security/keys-local/dev-signing.key
# win4（synth 正例+重启续链）/ win5（prod 三阶段）
bash evidence/spec-2026-08-27-E/window4-synth.sh target/evidence-e/window4 420 240
SCADA_DELAY=700 SCADA_RETRIES=10 SCADA_INTERVAL=80 \
  bash evidence/spec-2026-08-27-E/window5-audit.sh target/evidence-e/window5 1500
```
