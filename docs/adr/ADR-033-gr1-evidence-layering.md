# ADR-033 — G-R1 闸门证据分层与引导层阻断（22 §8）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §8 G-R1 判据 + 红线 8 每步可执行证据）
- **关联**：ADR-025~032、`蓝图/22` §2.W1、§8 G-R1

## 背景

G-R1 判据：W1.1-W1.6 验收全 PASS + QEMU 注入 panic/OOM/服务故障 → 设备
自愈不挂死。QEMU 全系统验证过程中，引导层（boot-manager）出现
`bootctrl-resolve-fail error=Crc`（factory 生成与 reset 脚本写入的
BOOTCTRL 双副本均被拒）——**引导层阻断**，root-task 无法进入
run_ipc_loop 的完整序列。

## 现状侦察（2026-08-09 代码为准）

1. W1.1-W1.6 **全部 host 验收 PASS**（OOM handler/panic 审计+SP805 序列/
   supervisor 状态机 8 测试/可回收堆回收验证/TPM 超时/看门狗条件 base）。
2. **裸机 QEMU 行为验证 PASS**：`oom-probe`（堆耗尽→handler 打印+挂起不
   abort）、`panic-probe`（panic→武装序列执行）。
3. **QEMU 全系统部分验证**：root-task 启动 → spawn **10 个 EL0 服务全部
   成功**（`spawn plan: root=root-task services=10` + 各服务
   `svc_ipc_loop entered`）——隔离/启动验证通过。
4. **阻断**：boot-manager `bootctrl-resolve-fail error=Crc`——factory
   生成（`BootControl::factory().encode()` CRC 有效，python 验证
   stored==calc）的镜像仍被拒——引导层 CRC 校验与镜像生成存在未定位
   的不一致（21-B 遗留或 harness/环境差异；G-KERNEL 旧 evidence 为旧
   流程产物）。QEMU 环境反复问题（端口残留/BOOTCTRL 污染/数据盘状态）
   加剧复现成本。

## 决策

1. **G-R1 证据分层**：
   - **W1 各条验收**：host 测试 + 裸机 QEMU probe（已全绿）——
     判定 PASS；
   - **QEMU 全系统"自愈序列"**（panic→看门狗复位→重启循环、supervisor
     fault→重启→熔断 的完整 QEMU 日志）：**标记 PARTIAL**——spawn 10
     服务验证 PASS；run_ipc_loop 序列被引导层阻断，**列为后续调试项**
     （boot-manager CRC 定位，真机/CI 环境复现）。
2. **不声称**：不声称"QEMU 注入 panic/OOM/服务故障全自愈"已通过
   （红线 8：无证据不声称）。G-R1 的**软件侧验收（W1.1-W1.6 行为）**
   与**系统级自愈序列**分开陈述。
3. **复现工具**：`tools/g1-repro.sh`（杀残留/清盘/归零 VARS/BOOTCTRL 重置
   /跑/摘要）+ `tools/reset-bootctrl.py`——为后续调试提供秒级复现。
4. **后续项**（ADR 记录）：boot-manager CRC 校验定位（对比
   boot-manager-uefi 与 ipc-wire 的 crc 调用链）；QEMU 全系统自愈序列
   完整验证（CI/真机环境）。

## 影响

- G-R1 的 W1 条目验收证据完整（host + 裸机）；系统级 QEMU 自愈序列
  未完成——**"24/7 可运行"声称暂缓**（红线 1：W1 未全通过不声称）。
- 22 后续工作流（W2-W5）不受引导层阻断影响（host 验证为主），继续推进；
  G-R 总出口审查时如实反映 G-R1 状态。

## 备选方案

- **深挖 boot-manager CRC**：预计 ≥1 天（引导层 + QEMU 环境调试），
  超出本会话预算；记录为后续项，不阻塞 W2-W5。
