# WP-30.6 elf-loader 重定位（WP-27.7 吸收，S7 前置已关闭）修复与证据

执行日期：2026-08-11。蓝图坐标：30 §8（依赖 WP-30.2 完成——四项校验洞已关）。

## 实现（蓝图/27 §11 规格）

1. `LoadedElf::parse_dynamic(data) -> Result<DynamicInfo, ElfError>`：PT_DYNAMIC
   定位（缺失 → BadDynamic）+ .dynamic 表遍历（DT_RELA/DT_RELASZ/DT_RELAENT，
   DT_NULL 终止；异常 → BadRelaTable）。
2. `LoadedElf::apply_relocations(img, base, dyn_, segments) -> Result<(), ElfError>`：
   - Rela 表边界校验（rela_ent ≥ 24 且 8 对齐；表区间在 img 内）；
   - 逐条：写入目标（base + r_offset，8 字节）必须完全落在**可写段**内
     （防恶意 Rela 越界写 → RelocOutOfBounds）；
   - R_AARCH64_RELATIVE(1027)：`img[r_offset..+8] = base + r_addend`；
   - 其他类型 → UnsupportedReloc（fail-closed 不静默跳过）。
3. 新增常量：PT_DYNAMIC/DT_NULL/DT_RELA/DT_RELASZ/DT_RELAENT/
   R_AARCH64_RELATIVE/ELF64_RELA_SIZE；ElfError 4 新变体 + Display。

## 安全顺序（S7）
验签（对象=文件字节，RT-004 同源）→ 映射段 → 重定位 → 跳转——重定位
接口独立于 parse，由装配层按序调用；parse 阶段无重定位副作用。

## 判据测试（黄金向量）
- apply_relocations_golden_vector：3 条 RELATIVE（静态数据/函数指针/字符串
  语义）在 base=0x4000_0000 全部解析为 base + r_addend；
- apply_relocations_rejects_out_of_bounds：段外写入 → RelocOutOfBounds；
  段内只读区 → 拒绝；
- apply_relocations_rejects_unsupported_type：R_AARCH64_GLOB_DAT(0x401) →
  UnsupportedReloc；
- parse_dynamic_rejects_missing：无 PT_DYNAMIC → BadDynamic。

## 回归
elf-loader 17（+4 判据）全绿。哈希资产与 WP-30.2 的 trait 注入分离（本 WP
为纯计算，无哈希依赖——S7"禁各写一套"指验签资产，重定位不涉哈希）。
