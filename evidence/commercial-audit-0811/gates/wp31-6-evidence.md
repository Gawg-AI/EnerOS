# WP-31.6 地雷拆除与存储精修（DR-06/02/03/09）修复与证据

执行日期：2026-08-11。蓝图坐标：31 §8。

## DR-06（P1，零硬件依赖红线）
- power detect.rs aarch64 虚构 ADC/GPIO 地址（0x0906_0000/0x0907_0000）全段
  删除；`is_main_power_ok` 统一单一实现（PowerRail 注入；未注入 fail-safe
  回退缓存状态）。POWER_OK_THRESHOLD_MV 提升为无条件常量。
- grep 判据：0x0906_0000|0x0907_0000 生产路径归零（仅注释保留）。

## DR-02（P2，tsdb 清理路径）
- cleanup_tmp_residue 删除拼完整路径 `{metric_path}/{name}`（旧实现只删
  基名必失败，残留 .tmp 永久堆积）。

## DR-03（P2，tsdb 子串误匹配）
- compact_chunks 两处 contains 子串匹配 → reader::path_matches（提升
  pub(crate)）精确分段（设备 "2" 不再命中 "12/"）。
- 附带：tsdb Cargo.toml 补 storage mock dev-dependency（MockBlockDevice
  门控不跨 crate——同 fs crate 模式，123 测试首跑通过）。

## DR-09（P2，virtio_blk 三处）
1. capacity 读 config space offset 0（legacy 8B LE；mock regs 越界防护
   unwrap_or(0) → 回退占位）；
2. block_size > 4096 → Err(UnsupportedBlockSize)（栈缓冲越界 panic 面）；
3. wait_used id 校验：**注释诚实化**（蓝图二选一选项）——精确 head 一致性
   依赖 avail 快照同步（多缓冲在飞），落地范围防御（id ≥ QUEUE_MAX →
   Err）+ 精确校验登记 33 §真机填空。

## 回归
power 32 / tsdb 123 / storage 128 全绿。
