# WP-30.1 侦察报告（RT-002/003/010/021/022 启动契约链单一事实源）

执行日期：2026-08-11。蓝图坐标：30 §3（WP-30.1，5 项一次 PR 全关）。

## A. 波及面自证（逐坐标 grep 复核）

| 坐标 | 现状确认 |
|---|---|
| boot-protocol/src/boot_info.rs:20 | `MAX_INIT_BUNDLE_LEN=256MiB`（validate 用）确认；:195 ENEROS_INIT_BUNDLE_PHYS；:199 ENEROS_MAPPED_WINDOW_LEN=2MiB 已有 |
| boot-manager-uefi/src/lib.rs:16-17 | INIT_LOAD_START=0x4200_0000 / INIT_LOAD_END=0x4280_0000 浮动区间；validate（~200）允许 load_address 在区间内 |
| boot-manager-uefi/src/main.rs:510 | `info.init_bundle_phys = init.load_address`（写入端就绪）确认 |
| root-task/src/main.rs:54,303 | INIT_BUNDLE_PHYS=0x4200_0000 硬编码；MAX_BUNDLE_LEN=64MiB 确认 |
| root-task/src/main.rs:317 | `total < 64 || total > MAX_BUNDLE_LEN` 确认 |

## B. 依赖方向检查

boot-protocol 是叶子契约 crate（无反向依赖）；boot-manager/root-task 已依赖 boot-protocol（grep：main.rs:24 已 use ENEROS_INIT_BUNDLE_PHYS）。改动不引入新依赖。

## C. 消费方清点

| 符号 | 实现者 | 调用方 | 处置 |
|---|---|---|---|
| MAX_INIT_BUNDLE_LEN | boot_info.rs:20 | boot_info.rs:333 validate | 删除（替换为导出常量 ENEROS_INIT_BUNDLE_MAX_LEN） |
| INIT_LOAD_START/END | boot-manager lib.rs | lib.rs validate + main.rs | 改引用 boot-protocol 常量；validate 收紧 |
| MAX_BUNDLE_LEN | root-task main.rs | read_bundle_from_phys | 替换为 ENEROS_INIT_BUNDLE_MAX_LEN |
| INIT_BUNDLE_PHYS | root-task main.rs:54 | :117/:134/:141/:158 | 替换为 boot-protocol ENEROS_INIT_BUNDLE_PHYS（消除魔法数）+ 一致性断言 |

## D. 二次影响对照（S 条目）

- RT-002/021 完整修复（rust_entry 消费 BootInfo.init_bundle_phys）需 elfloader/
  内核传参通道（x1）——elfloader 为第三方构建产物（源码不在仓库，
  third_party/sel4-build/top/elfloader 仅产物）→ 按蓝图 RT-002 修复方案过渡
  路径：boot-manager validate 收紧 `load_address == ENEROS_INIT_BUNDLE_PHYS`
  （写端保证），root-task 读固定常量 + 打印一致性断言（读端校验），两端由
  同一常量闭合。豁免登记：完整通道改造依赖第三方 ABI，登记 34 域。
- R30-1 登记：bundle >2MiB 需求需先扩内核窗口（BOOT-007 证据重出）。
- 27.1 既定窗口事实（0x4200_0000/2MiB）不回退——本组为回灌。

## E. 测试基线快照

- `cargo test -p eneros-boot-protocol -p eneros-boot-manager-uefi -p eneros-root-task`（改动前，host）。
