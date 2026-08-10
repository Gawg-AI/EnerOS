# WP-33.4 soak 证据自洽（GV-05）修复与证据

执行日期：2026-08-11。蓝图坐标：33 §6。

## 0. 侦察报告（开工前落盘）

- **波及面自证**：仅 `xtask/src/qa.rs` 的 soak-run 段（command_soak_run /
  command_soak 门禁 / SoakCampaign）与 soak 相关测试——符合 33 §1 排斥
  矩阵"xtask/src/qa.rs（仅 soak-run 段 GV-05）"。
- **依赖方向**：sha2 为 xtask 既有依赖（artifact_inspector 已用），零新增。
- **消费方清点**：SoakCampaign 消费方仅 `qa soak`（command_soak 门禁）与
  自身测试；boot-times.csv / soak-campaign.json 由 soak-report 消费（字段
  兼容：未增删字段）。CI 无 soak-run 调用（grep 实证）——新增参数无外部
  调用点需适配。
- **二次影响对照**：`--image` 由可选改强制——qemu-prod 的 --image 本就
  强制（"mandatory"），soak-run 缺 --image 原会在第一轮 spawn 时失败，
  改为构造期显式报错是收紧而非破坏；t5 测试同步补 --image 与
  --min-duration-days 0。
- **测试基线**：xtask 60 测试全绿（开工前）。

## 修复（对照蓝图 §6 定位三连）

1. **artifact_sha256 实算**（旧：硬编码 `""`，自证链断裂）：
   - soak-run 对 `--image` 产物实算 SHA-256（`hex_encode(Sha256::digest(
     read(image)))`）。
   - 决策明示：蓝图正文写"crypto crate SM3"但验证判据写"等于产物
     sha256sum"（SHA-256）——字段名与验证判据一致，采用 sha2（xtask
     既有依赖）实算 SHA-256。
   - 门禁同步：`qa soak` 对空 artifact_sha256 判违规
     `missing-artifact-sha256`（旧门禁不校验，硬编码空串可过门）。
2. **memory_leak 实算**（旧：硬编码 0）：宿主 RSS 采样（/proc/self/status
   VmRSS，非 Linux 返回 None 并显式警告）→ 每轮结束采样
   (累计秒, RSS 字节) → 线性回归斜率 → 字节/天（负斜率钳 0）；
   阈值 `--leak-threshold` 可配（默认 64 MiB/天，旧 0 阈值与实测互斥）。
3. **duration 与门禁阈值**（旧：duration_days 实算但 min_duration_days
   硬编码 3 → 短时 CI 永远不过门）：`--min-duration-days` 可配（默认 3
   = 14 号文 D1 72h；CI 短周期显式传 0 且打印 WARNING 标注"非 72h 浸泡
   证据，不得静默"）。
4. **data_corruption**（旧：硬编码 false）：QEMU 浸泡仅验证引导链（无
   数据负载写入），腐化检测不适用（真机 72h 浸泡脚本负责数据面）——
   false 为如实声明，注释明示，非静默造假。

## 证据

- `cargo test -p eneros-xtask` → 61 passed（旧 60 + t5b/t3c 新增，t5 更新）：
  - t5：soak-run（--image 假镜像 + --min-duration-days 0）→
    artifact_sha256 实算 == `hex(Sha256::digest(image))`；campaign 产出。
  - t5b：缺 --image → 显式报错（fail-closed）。
  - t3c：空 artifact_sha256 的 campaign → 门禁拒绝。
- 蓝图验证命令 `cargo xtask qa soak-run` 的完整 QEMU 实跑（真镜像 +
  门禁通过）随 WP-33.5 QEMU 证据链一并执行（同镜像产物）。
- clippy `-p eneros-xtask --all-targets --no-deps -D warnings` clean；
  顺手修复 qa.rs 既有 `unnecessary_to_owned` lint 一处（perf-gate 测试，
  文件在矩阵内一行修复，明示）。

## 回归

- xtask 61 测试全绿；observability/ota-ab 等 33 前序 WP 产物不受影响
  （qa.rs 仅 soak 段改动，proof/compliance/gng 段未触碰——34 域）。
