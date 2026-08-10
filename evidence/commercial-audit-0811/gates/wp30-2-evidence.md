# WP-30.2 ELF/spawn/bundle 校验洞（RT-004/005/006/007，S7）修复与证据

执行日期：2026-08-11。蓝图坐标：30 §4（S7：先于 WP-27.7 重定位）。

## RT-004（P1，bundle 模块 sha256 从不校验）
- 打包侧（xtask compose_init_bundle）已写真实 Sha256 摘要（grep 核实）——
  缺口仅在解析侧。
- 修复：bundle.rs 增 `parse_verified(raw, expect_sha256: fn(&[u8]) -> [u8;32])`
  （每个模块 blob 摘要 vs 表项 sha256，不符 Err(DigestMismatch)）；哈希经
  函数注入（root-task 依赖 sha2 0.10 no_std，与打包侧同算法）；main.rs
  L143 唯一消费点替换为 parse_verified（DigestMismatch 即挂起 fail-closed）。
- 判据：test_parse_verified_accepts_matching_digest（正例）+ 
  test_parse_verified_rejects_tampered_blob（篡改 1 字节 → DigestMismatch）。
- 测试夹具同步写真实摘要（原"sha256 占位（全零；解析器不校验摘要）"注释
  删除）。

## RT-005（P1，elf-loader 入口校验加法回绕）
- lib.rs:278 `e_entry < s.vaddr + s.memsz` → checked_add（恶意段头
  vaddr=0xFFFF_FFFF_FFFF_F000+memsz=0x2000 回绕绕过校验）。
- 判据：test_validate_segments_rejects_wrapping_range（回绕 → Err）。

## RT-006（P1，段 vaddr 重叠）
- 落点统一（禁止两处各写）：elf-loader 新增 `validate_segments(segs)` 纯函数
  （边界 checked_add + 排序后 windows 两两比较）；`LoadedElf::parse` 尾部
  调用；spawn.rs plan_one 复用（SpawnError::SegmentOverlap 透传）。
- 判据：test_validate_segments_rejects_overlap（重叠 → Err；相邻合法）+ 
  test_plan_one_rejects_overlapping_segments（校验来源核验）。

## RT-007（P1，debug_assert release 失效）
- copy_segment_via_window：debug_assert → 显式 `if len > PAGE_SIZE →
  Err(SpawnError::WindowOverflow)`（release 生效）。
- 判据：test_copy_segment_via_window_rejects_oversized_len（检查路径存在
  防回退）+ 函数首行显式检查走查。

## 回归
- elf-loader 13（+2 判据）/ root-task 59（+4 判据）全绿。
- S7 前置达成：WP-30.6（elf-loader 重定位）现在可开工。

## 消费方清点
- parse_verified：main.rs 唯一消费点（grep InitBundle::parse 仅 main.rs:143）。
- validate_segments：elf-loader parse + spawn plan_one 两处（统一入口）。
- copy_segment_via_window：spawn.rs 内部逐页循环（唯一）。
