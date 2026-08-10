# WP-33.1 可观测数据完整性（OB-01/02/03/04）修复与证据

执行日期：2026-08-11。蓝图坐标：33 §3。

## OB-01（P1，蓝图/09 未销项确认）
- metrics increment 基数 = store 最新值 + 未 flush pending 累计（heapless
  pending Vec<32>）；maybe_flush 后 pending 清零。
- 判据：test_increment_pending_accumulates（5×3 不 flush → flush 后总值 15）。

## OB-02（P1，蓝图/09 变体仍存）
- logging flush_to 改逐条成功即 remove（按成功进度收缩）——旧实现批量
  刷入失败后 buffer 未收缩，已成功条目下次重复写。
- 判据：既有 flush 测试保持（20 全绿）。

## OB-03
- telemetry-queue QueueSnapshot 增 capacity/policy（旧快照兼容默认
  len×2 + DropOldest）；restore 恢复原配置。
- OverflowPolicy 增 Default（DropOldest）。

## OB-04
- fault-recorder SampleRecord.timestamp_us u32 → u64（>71.6 分钟回卷）；
  COMTRADE binary 布局偏移 +4（cw19~cw23 断言更新）。

## 回归
metrics 16 / logging 20 / telemetry-queue 35 / fault-recorder 32 全绿。
