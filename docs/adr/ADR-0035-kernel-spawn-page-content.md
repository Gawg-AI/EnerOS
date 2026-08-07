# ADR-0035：K1.3 spawn 页内容拷贝方案 A（root-task 临时映射）

- 状态：已接受（2026-08-08，21-A K1.3；用户已确认方案选择）
- 关联：蓝图/21-A §2.K1.3；ADR-0034（spawn FFI 缺口）；ADR-0023（seL4 基线）
- 背景蓝图：`蓝图/21-A-内核地基-G-KERNEL闸门-裸机OS真闭环开发蓝图.md`

## 问题

spawn 时 retype 出的页是**空白物理页**，需把 init bundle 中服务 ELF 的段数据
写入页物理内存，再映射进服务 VSpace。21-A K1.3 给出两个候选：

- **方案 A（root-task 临时映射）**：retype 新页后，root-task 把它 map 到自己
  VSpace 的临时窗口地址，拷贝 ELF 段数据，unmap，再映射进服务 VSpace。
- **方案 B（elfloader 预加载）**：改 init bundle 构建流程，让 elfloader 预先把
  服务 ELF 段放到独立物理页，spawn 时只映射不拷贝。

## 决策

**选方案 A**（21-A 建议方案；2026-08-08 用户明确确认）。

理由：
1. boot 链（UEFI 验签 + A/B + measurement + elfloader）已足够复杂，方案 B
   动启动链，风险传导到 ①启动判据（21-A 已论证）。
2. root-task 是唯一特权进程（路线锁定 4），拥有全部 untyped 与 root VSpace，
   做临时映射是其本职；方案 A 不改变任何既有链路。
3. 方案 A 的通用性：未来动态加载服务（OTA 后）同样需要"拷内容进新页"，
   方案 A 一次解决，方案 B 则把问题推回构建期。

## 实现（已随 K1.2 落地，spawn.rs `copy_segment_via_window`）

页拷贝调用序列（每段每页一次）：

```
1. retype untyped → SmallPage frame（空白页，BSS 由 retype 清零保证）
2. arm_page_map(frame → root VSpace @ WINDOW_BASE)      // 临时窗口映射
3. memcpy(WINDOW_BASE, blob[seg.offset + 页内偏移], chunk) // 拷 ≤4 KiB
4. arm_page_unmap(frame)                                  // 解除（ADR-0034：
   // frame 只能映射一个地址，unmap 前不可再映射进服务 VSpace）
5. arm_page_map(frame → 服务 VSpace @ 段映射 vaddr)
```

关键参数（ADR-0034 同源）：
- 窗口地址 `WINDOW_BASE = 0x8000_0000`（root VSpace，1 GiB 区域；与 root
  镜像 0x4000_0000 区域不冲突；L0/L1 由 elfloader 提供，spawn 补 L2/L3 两个页表）。
- 窗口固定复用：unmap 后内核允许 frame 重新映射到同一窗口地址，多页串行
  拷贝无冲突。
- 多页段：`align_up(memsz)/PAGE_SIZE` 页循环，每页独立 frame，分块拷贝
  （`filesz` 跨页边界正确处理，BSS 部分跳过拷贝）。
- 段 vaddr 非页对齐 → `PlanMismatch` fail-closed（链接器默认页对齐）。

## 影响

- 代码：全部随 K1.2 提交（spawn.rs 窗口拷贝 + 页表建立 + main.rs 接线）。
- 性能：每页 1 次 map + 1 次 memcpy + 1 次 unmap，spawn 期一次性开销，
  运行时零开销（服务页最终只映射在服务 VSpace）。
- 安全：拷贝窗口仅 root-task（特权进程）可见；服务间互不可见（独立 VSpace）。

## 备选（否决）

- 方案 B（elfloader 预加载）：动启动链，否决理由见上。
- frame 双映射（窗口 + 服务同时映射）免 unmap：内核 `capFMappedASID` 校验
  明确拒绝（ADR-0034 证据），不可行。
