# WP-31.5 D 矩阵补全（D5/D6/D8/D9 + D10 机制）修复与证据

执行日期：2026-08-11。蓝图坐标：31 §7。

## D5 RTC（DR-11）
- 绑定表增 arm,pl031 条目（rtc_factory：FDT reg 注入 → register_rtc_base）；
  main.rs time_init 占位 0 的 RTC 注入接线随 33 域装配（基址已可注册）。

## D6 WDT（DR-10）
- 绑定表增 arm,sp805 条目（wdt_factory：FDT reg 注入 → register_wdt_base）；
  panic 路径 WDT_BASE 改 AtomicU64（注册基址读取；默认 QEMU 兜底）。
  软件模式语义不变（30 RT-001 守约）。

## D8 timer
- 30 的 RT-008（timer_irq_from_dtb + 绑定表登记）已完成——D8 关闭 ✓。

## D9 RS485（DR-07）
- divisor u64 中间量（1.2 GHz 无回绕）+ GpioPin trait 注入（RamGpioPin 桩，
  对齐 PowerRail 模式）+ inject_gpio 装配方法。GPIO 真接线登记 33 §真机填空。
- 判据：test_divisor_no_overflow + test_ram_gpio_pin_level。

## D10 cellular（DR-04/05，R31-2 拆分）
- **DR-05 完成**：fcs16（RFC 1662 CRC-16-CCITT 补码）encode 覆盖 protocol+data、
  decode 校验；判据：RFC 1662 向量 "123456789"→0x906E。
- **DR-04 部分**：AT+CGDCONT 下发（ATD 前配置 APN，判据 MockSerial 收到）；
  PPP TX 协商（send_lcp_conf_req/IPCP）与 PAP 登记 33 §真机填空（R31-2
  机制 host 验证 + 真机填空拆分销账，不宣称全通）。

## D2 virtio-net（DR-12/DR-08）
- 完整 virtio-net 设备驱动（含 DR-08 cache 一致性 CacheOps 注入）登记 33
  域真机清单（原 WP-27.3 已登记缺口；31 未新写完整驱动，如实声明）。

## 回归
root-task 61 / rs485 50 / cellular 144 全绿。
