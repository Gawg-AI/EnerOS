# VOL68.4-GATE 证据摘要（run5 终判，2026-09-06）

- 判定：VOL68.4-GATE PASS（双窗全判据绿）
- 完整扫描 = 本目录 gate-run3-scan.log（run5 终判留痕）；串口大日志（15/16MiB）按 gitignore 政策本地归档于本目录

## 双窗关键数字

| 判据 | 窗A normal | 窗B restart |
|---|---|---|
| PRODUCT-READY | true | true |
| PANIC 净值（豁免 REG-DISPATCH-HEAP-HIGHROUNDS 签名后） | 0 | 0 |
| READINESS | 29 | 29 |
| TSDB-DISK-COMMIT（68.3 不回归） | 18 | 19 |
| TSDB-CHECKPOINT-DUR max | 31.7s | 30.4s |
| EVENT-STORE append | 18 批 fail=0 | 20 批 fail=0 |
| EVENT-REPLAY | recovered=0 consistent=true（fresh） | **recovered=112 consistent=true** |
| SNAP-BUILT / SNAP-STORED / SNAP-SKIP | 2 / 2 / 0 | 2 / 2 / 0 |
| SNAP-RECOVERED blobs | —（fresh 0） | 2 |
| LEARN-STATE restored | — | 1（68.3 学习态恢复不回归） |
| modbus ACQ 行 | 105 | 98 |
| FACT-FACE-OPEN | consistent=true wm=true | 同左 |
| agent=OK | 463（=vol3 基线 463） | 476 |
| 跨窗 RPO=0 对账 | 窗 A 追加 112 → 窗 B 恢复 112（**精确零丢失**） | 同左 |

## 变异验证（host）

| 变异 | 目标 | 结果 |
|---|---|---|
| RegionScope 基址置 0（去钳制） | t3 | 红 ✓ |
| append 假 ACK（去持久化） | t1 | 红 ✓ |
| 重放 FrameCollector 丢帧 | t4 | 红 ✓ |
| SnapshotWatermark 不推进 event_time | t4 | 不咬（两侧同代码路径自洽）——水位逻辑由 snapshot-service 单测锁，t4 锁重放一致机制；登记为测试设计事实 |

复现命令（变更后重验须重做）：
```bash
# 去钳制：event_face.rs ensure_open 的 open_region_scoped(..., base_4k, blocks_4k, ...) → (..., 0, 全盘块数, ...)
cargo test -p eneros-control-services --lib t3_region_scoped   # 预期 FAILED
# 假 ACK：store_fact_sink.rs append 的 store.append(...) → let offset: LogOffset = 0;
cargo test -p eneros-business-services --lib t1_fact_face      # 预期 FAILED
# 重放丢帧：event_face.rs tests FrameCollector::record 尾部加 self.frames.pop();
cargo test -p eneros-control-services --lib t4_snapshot        # 预期 FAILED
```

## host 基线

8663 测试全绿（卷前 8648 → +15）+ clippy 0 warning + fmt clean。

- RegionScope 去钳制 → t3 红 ✓；append 假 ACK → t1 红 ✓；重放丢帧 → t4 红 ✓
- 水位判据变异不咬（t4 两侧同代码路径自洽）——登记为测试设计事实
