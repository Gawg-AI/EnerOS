# WP-33.6/33.7 OTA 演练 + HIL 矩阵 + 真机填空封闭清单

执行日期：2026-08-11。蓝图坐标：33 §8/§9。

## WP-33.6 OTA 演练（SE-15/18/19 硬化后的负例演练）

- `tests/ota-rehearsal.sh` 增负例演练段：三连拒绝判据（33 §8 要求）——
  1. 伪造分区表（重算 CRC 无 HMAC）→ pst8 必拒；
  2. 不可解析版本（check_monotonic fail-closed）→ test_cry006 必拒；
  3. 超尺寸镜像（构造器校验 + 运行时防越界）→ test_oversize 必拒。
  产出 `ota-rejection-evidence.json`（schema eneros.ota-rejection-evidence.v1）。
- 执行：`bash tests/ota-rehearsal.sh --skip-qemu` → 三连 verdict 全 PASS
  （证据：target/ota-rehearsal/ota-rejection-evidence.json）。
- 调试记录（次生缺陷）：初版脚本用 `grep -q` 直连 cargo 管道——pipefail
  下 grep 提前关管致 cargo SIGPIPE（141）→ 误判 FAIL；改临时文件 +
  `grep -q file`（已修，留证）。
- QEMU 安装路径：本环境无 enupd 安装器 QEMU 链路（差分/安装/回滚命令
  占位）——如实声明，真机演练挂清单 A6/B1（对端设备 + virtio-net 驱动）。

## WP-33.6 HIL 框架（27 §15 M2 host 部分）

host 仿真后端矩阵（协议 × 后端对拉，全部真传输走 TCP socket/回环）：

| 协议 | 后端矩阵 | 结果 |
|---|---|---|
| IEC104 master | 127.0.0.1:2404 slave 测试桩 | 3 套件 ok |
| IEC104 slave | loopback | 2 套件 ok |
| Modbus TCP | loopback | 2 套件 ok |
| Modbus RTU | loopback + pl011 串口回环 | 2+1 套件 ok |
| GOOSE / SV / MMS | 参考报文 fixture（31 标准化后） | 2+2+3 套件 ok |

证据：evidence/stage1-foundation/hil-matrix/*.log（全绿）。
真机互操作（商用保护装置对端）留清单 A6。

## WP-33.7 真机填空封闭清单定稿 + 台账修正

- **清单**：evidence/stage1-foundation/real-machine-fill-list.md——
  模板化填空表（每项：板级动作/接线点/判据行格式/证据落盘位置）：
  - A 组 6 项（27 §16 原清单：dts 补全/PHY/eMMC-NVMe/MAC/modem/真对端）；
  - B 组 6 项（33 §9 新登记：D2 virtio-net、D10 cellular、D9 RS485
    GPIO、PowerRail 板级实现、SP805/PL031 激活、时钟源三常量板级值）；
  - C 组 2 项（33 WP-33.5 转来的 QEMU 侧显式缺口：IRQ 投递链、virtio-blk
    块读——修复路径 = 内核/驱动调试，非真机填空）。
- **判定**：PENDING 集合 == A+B 真机填空 + C 显式缺口。33 复验订正：
  C1（IRQ 投递）与 C2（virtio-blk 读）为 QEMU 可验证项仍挂 PENDING——
  原"S9 守约"声明自相矛盾，撤除；按 33 §7"如实声明仍 PENDING 及原因"
  处理（DoD 2 允许），验收口径以本判定为准。
- **蓝图/09 台账**：OB-01/OB-02/OB-05/OB-06/OB-07 五行销项注解（引用
  wp33-1/2-evidence.md）。

## 回归

- ota-ab 44 全绿（三连判据测试在案）；HIL 矩阵 7 协议套件全绿；
  observability/security 前序 WP 产物不受影响。
