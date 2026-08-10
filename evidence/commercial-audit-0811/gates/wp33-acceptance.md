# 版本 33 验收（可观测与物理证据修复）

执行日期：2026-08-11。蓝图坐标：33 §10（版本验收 DoD 六项）。

## DoD 逐项对照

1. **OB-01~07、SE-15/18/19、GV-05 全销（附测试证据）** ✅
   - OB-01/02/03/04（wp33-1-evidence.md：metrics pending 累计 16 测试、
     logging flush 收缩 20、telemetry-queue snapshot 35、fault-recorder
     u64 32）；
   - OB-05/06/07（wp33-2-evidence.md：AlertStatus 状态机 42、aiops
     fail-closed 23、JSONL 文件导出 std 门控端到端）；
   - SE-15/18/19（wp33-3-evidence.md：版本单调 fail-closed、分区表
     SM3-HMAC pst8、downloader 预分配 bound；ota-ab 44 全绿）；
   - GV-05（wp33-4-evidence.md：soak artifact_sha256 实算、RSS 泄漏回归、
     --min-duration-days 可配；xtask 61 全绿）。
2. **三项 PENDING 转真证据入 evidence/stage1-foundation/** ✅（部分，
   如实声明）：M1 host 双端回环对通 4 组抓包全绿；掉电 20 轮恢复机制
   host 模拟（committed=20 单调）；**中断计数与掉电 QEMU 判据行**按
   §7 允许如实声明仍 PENDING 及原因（wp33-5-evidence.md：IRQ→ntfn
   投递链 QEMU 侧不达需内核调试；virtio-blk 块读路径 Io 需驱动调试）——
   两项均为 QEMU 侧显式缺口，已登记真机清单 C 组，不挂真机填空项。
3. **OTA 三连负例演练证据在案** ✅（wp33-67-evidence.md：伪造分区表/
   不可解析版本/超尺寸镜像 verdict 全 PASS）。
4. **真机填空封闭清单定稿提交；PENDING 台账与清单逐项对应** ✅
   （real-machine-fill-list.md：A 组 6 + B 组 6 + C 组 2 显式缺口）。
5. **蓝图/09 台账 OB-01/02 销项状态修正** ✅（OB-01/02/05/06/07 五行
   注解，引用 wp33-1/2 证据）。
6. **回归：observability 全 crate 测试绿；QEMU 引导链/驱动基线不回归**
   ✅（workspace 3878 全绿；QEMU 全程启动序列完整、svc 全 spawn、
   virtio-blk 数据盘绑定活体、hello 往返正常）。

## 治理自查

- 门禁全绿且非恒 PASS：soak 门禁新增 missing-artifact-sha256 违规
  （t3c 判据拒绝）；无恒 PASS 项。
- mock 门控无回退：aiops MockExecutor 限 cfg(test/mock)（OB-06）；
  durable-log/mock 门控保持。
- 零硬件依赖：无新增硬编码物理地址/IRQ/板级常量（timer IRQ 30 经
  FDT phys triple；数据盘经最后一块 blk 语义；PTMR 为内核构建选项）。
- 次生缺陷处理：pst8 篡改目标断言失真（修+留证）；脚本 SIGPIPE 误判
  （修+留证）；JSONL 扫描器 fields 收尾（修+留证）；32 遗漏消费方
  intent-contract 288→289（33 回归修正，提交明示）。

## 预存债务登记（非 33 引入，34 处理）

- storage virtio_blk.rs:363 `0x100 + 0` clippy identity_op（31 b7b970f）；
- root-task host 构建 unused imports（ENEROS_DTB_MAX_LEN 等，预存）；
- 中断计数/掉电判据行 QEMU 侧缺口 2 项（C1/C2，见清单 C 组）。
