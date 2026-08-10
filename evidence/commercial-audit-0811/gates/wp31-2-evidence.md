# WP-31.2 IEC104 slave 正确性（PR-05/06/07/08）修复与证据

执行日期：2026-08-11。蓝图坐标：31 §4。

## PR-05（P1，t1 超时重传循环）
- SlaveConnection 增 t1_retry_count；t1 超时分支：重发 U 帧 + 计数 +
  last_tx_time_ms 重置；≥3 次断连（IEC 104 §5.3 断连保底；旧实现无限
  重传忙循环）。
- 判据：test_t1_retry_bounded_then_close（TestFrAct 场景 4×t1 推进 →
  重传 ≤3 + state Idle）。

## PR-06（P2，APDU 长度 u8 截断）
- `Apdu::encode() -> Result<Vec<u8>, Iec104Error>`；ASDU 超 249 字节
  （APDU >253）→ Err(ApduTooLong)。全仓调用方同步（slave 生产 1 处 +
  测试多处于 unwrap）。
- 判据：test_encode_rejects_oversized_asdu（64 IO 超长 → Err）。

## PR-07（P2，时钟同步无授权门）
- Iec104Config.allow_clock_sync（默认 false）+ set_allow_clock_sync 方法；
  handle_clock_sync 未授权直接返回（不写 RTC、不回确认）。
- 判据：test_clock_sync_denied_without_policy（默认拒绝 tx 不变）；
  既有确认测试注入放行策略。

## PR-08（P2，双点遥控无 SBO）
- handle_double_command 补齐 SBO：select 阶段记录 selected + 回确认；
  execute 必须与 select 精确匹配（对照单点 PTC-003 语义）。
- 判据：test_double_command_executes_when_authorized（select→execute
  confirm）+ test_double_command_direct_execute_rejected（直接 execute →
  Deactivation）。

## 回归
iec104-slave 149（+3 判据）+ loopback 集成全绿。
