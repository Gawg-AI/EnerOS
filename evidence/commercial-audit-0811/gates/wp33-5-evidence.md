# WP-33.5 三项 PENDING 转真证据（吸收 WP-27.4）——执行与缺口登记

执行日期：2026-08-11。蓝图坐标：33 §7（按蓝图/27 §8 规格执行）。

## 0. 侦察报告（开工前落盘）

- **波及面自证**：root-task（irq.rs/irq_timer.rs/main.rs/driver_probe.rs/
  service.rs/Cargo.toml——33 §7 掉电判据行要求的 30 WP-30.7 接线遗留，
  30 evidence 明示"root-task 持久化接线随 33 域 WP-27.4 掉电证据出证"）、
  durable-log（RefMutDevice 借用式包装）、driver-framework
  （BoundEntry.irq_badge 接缝填充——30 RT-009 索引配对缺陷修复所必需）、
  sel4-build-rust.sh（-DKernelArmExportPTMRUser=ON——seL4 aarch64 用户态
  在 EL0，CNTP_CTL 需 EL0PTEN 导出）、tests/（powercut-durable.sh /
  uart-irq-client.py——证据脚本，矩阵内）。
- **依赖方向**：root-task 新增 eneros-durable-log 依赖（30 机制 crate）；
  无其他新增。
- **测试基线**：root-task 61 / durable-log 4 / framework 42 全绿（开工前）。

## M1 对通（转真 ✅，host 双端回环）

- 104 回环：iec104-master loopback（127.0.0.1:2404 对拉，总召→遥测回包）
  + iec104-slave loopback，host 抓包日志入
  `evidence/stage1-foundation/m1-loopback/`（1+1 全绿）。
- Modbus 对通：modbus-tcp loopback + modbus-rtu loopback（含 pl011 串口
  回环），抓包日志同上（1+2 全绿）。
- **D2 virtio-net 依赖部分**（Modbus-TCP-over-virtio-net 的 QEMU 侧）：
  R31-2 已登记转 33 真机填空清单——此处如实声明：host 双端回环对通
  完成，QEMU 网络面挂真机清单（D2）。

## 中断计数（部分转真 ⚠️，四项根因已修，投递链一处在 QEMU 侧断裂）

**修复落地（QEMU 活体验证的根因链）**：
1. **IRQControl_Get dest_depth 0→64**（irq.rs）：root cnode levelBits=64
   （spawn.rs 23 G-I 同款注释）；0 使 lookupSlotForCNodeOp 判 depth<1 →
   RangeError（QEMU 实测 `Target slot for new IRQ Handler cap invalid`）。
2. **cnode_mint dest/src depth 0→64**（irq.rs）：同上（Retype 有 depth=0
   特例而 CNode_Mint 没有）。
3. **virtio 区域 2M→4×4K 帧映射**（driver_probe.rs）：QEMU 实测单次 2M
   帧映射仅首 4K 生效（槽 8 起 0xa001000 translation fault）；4K 帧路径
   已被 D1 pl011/D4 槽 0 验证。
4. **IRQ 接线索引→badge 配对**（framework take_last_badge 接缝 +
   probe_and_report）：30 RT-009 的索引配对在"未绑 IRQ 的驱动"存在时
   错位——QEMU 活体实测 pl011 的 IRQ 33 被接到 virtio-blk0（bound[0]
   未绑 IRQ）。修复后 `probe irq-wired driver=pl011-uart0 irq=33`。
5. **EL0 物理定时器导出**（sel4-build-rust.sh -DKernelArmExportPTMRUser
   =ON）：seL4 aarch64 用户态在 EL0；缺省时 root-task `msr cntp_ctl_el0`
   触发 user exception EC=0x18（QEMU 实测）；导出后无异常。定时器 IRQ
   选 30（phys）而非 27（virt）——27 是 KERNEL_TIMER_IRQ（RevokeFirst）。
6. **驱动 IRQ 计数判据行**（service.rs DRIVER-IRQ 打印）+ socket 串口
   注入脚本（tests/uart-irq-client.py）——UART RX 注入到达 pl011
   （串口流可见注入字节，判据行在 bind 后注入）。

**仍 PENDING（如实声明）**：IRQ→badge ntfn→poll 投递链在 QEMU 侧不达
——定时器 IRQ 30 与 pl011 IRQ 33 均未触发计数（注入时序 20s/55s/120s
三组实验均 0 计数；内核自身定时器 IRQ 27 工作正常，GIC/内核路径对
用户绑定的 IRQ 不投递）。需内核级调试（GIC 使能/IRQ 状态机），超出
本会话可验证边界——登记为 QEMU 侧 PENDING（不挂真机清单，属于内核
集成问题，34 或独立内核会话处理）。**判据**：串口 `IRQ: irq=` /
`DRIVER-IRQ:` 计数递增行。

## 掉电 ≥20 次（部分转真 ⚠️）

- **机制证据（host，✅）**：durable-log 新增 `powercut_20_cycles_
  monotonic_recovery`——20 轮"断电+恢复"：append 序号单调、每 5 轮注入
  记录区半写脏字节（断电窗口等价形态）、recover 以已提交序号截断且
  单调不减，20 轮后 committed=20。5 测试全绿。
- **QEMU 接线（⚠️ 部分）**：root-task 掉电自检接线完成——virtio-blk
  数据盘（槽 30）绑定成功（4K 映射修复后 `probe bound virtio-blk0`），
  durable format/recover/append 判据行打印在案；**但 format 返回 Io**——
  virtio-blk **块读路径**在 QEMU 无活体先例（D4 证据仅 config space
  容量读），read_block 返回 Io（候选根因：DR-09 used_elem.id 校验或
  设备响应路径）——登记为 QEMU 侧缺口，真机填空清单同步登记（块读
  路径需 QEMU 调试或真机验证）。
- **判据**：每次掉电后下次启动串口 `durable-log: recover committed=N`
  单调续增（N=1..20）。

## 回归

- root-task 61 / durable-log 5 / framework 42 / storage 128 全绿；
  QEMU 引导链 13 行基线不回归（run6/run11 全程启动序列完整、hello
  往返正常、svc 全部 spawn）。
- 无新增硬编码物理地址/IRQ/板级常量（timer IRQ 30 经 FDT phys
  triple 提取；数据盘经最后一块 blk 语义；PTMR 为内核构建选项）。
