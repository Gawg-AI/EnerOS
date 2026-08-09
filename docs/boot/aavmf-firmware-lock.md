# AAVMF 固件锁定声明（WP-27.1，R27-2 供应链缓解）

> 依据：蓝图/27 §5.3 验收「AAVMF 版本+哈希锁定声明入 xtask 文档」。
> 本文件与 `xtask/src/qemu_harness.rs` 中 `firmware_path` 上方的注释块为同一
> 声明的两个载体；更换固件前必须重跑 qemu-prod 全链证据并同步更新两处。

## 锁定记录（2026-08-10 实测）

| 项 | 值 |
|---|---|
| 固件路径 | `/usr/share/AAVMF/AAVMF_CODE.secboot.fd` |
| 包 | Ubuntu `qemu-efi-aarch64`（`AAVMF_CODE.secboot.fd`） |
| 版本字符串 | `2025.11-3ubuntu7`（edk2-2025.11） |
| 构建时间 | 2026-03-31 11:11:58（固件内 `Sec.dll` 构建路径 + 启动日志首行） |
| SHA256 | `aafedb40e155a33ef37da3f94169352c111356c6c510359dd25fdfb4817bc2d6` |
| VARS 模板 | `/usr/share/AAVMF/AAVMF_VARS.fd`（64 MiB；qemu-prod 使用全零化副本，确保无历史 Boot 项/anchor 变量） |
| 替代路径 | `ENEROS_QEMU_FIRMWARE` 环境变量（xPack edk2-arm-code.fd 等测试环境） |

每次 qemu-prod 运行的证据文件（`qemu-result.json` / `qemu-install-result.json`）
均记录 `firmware_path` 与 `firmware_sha256`，可机器核对。

## 为什么这不是"固件代差"故障（H1 排除）

2025.11 固件下 G1 日志（2026-08-09 21:31，`target/g1-evidence/qemu-serial.log`）
完整复现了 13 行启动断言（root-task alive → spawn → IPC → 故障隔离）。BOOTCTRL
分区内容由 xtask 镜像工厂（`image_factory.rs`）与 boot-manager 自身写入，UEFI
固件不写该分区。因此 `bootctrl-resolve-fail error=Genesis` 与固件版本无因果。

## 真正的根因与修复（H3）

- 现象：`cargo xtask run qemu-prod` 同盘重跑 → `bootctrl-resolve-fail
  error=Genesis` → `REJECT BOOTCTRL`。
- 机制：QEMU `cache=none` 直写镜像，上次成功引导会把 BOOTCTRL 双副本写为
  `seq=1`（合法状态，CRC 正确）；而 harness 每次运行都从模板复制全新 VARS
  （无保护锚点）→ `resolve_boot_control` 无锚点路径要求 `seq==0` → fail-closed
  Genesis。**这是安全语义的正确拒绝，不是 bug**（红线：不得放宽判定）。
- 修复：
  1. `tools/reset-bootctrl.py`：CRC32 补最终异或（旧版与
     `eneros_ipc_wire::crc32` 不一致，写出的副本被 decode 判 `Crc`——"重置
     脚本与 boot-manager 的 CRC 不一致"风险的根因）。
  2. `cargo xtask run qemu-prod --reset-bootctrl`：运行前把 BOOTCTRL 双副本
     归零（decode→seq=0→encode 重算 CRC，与 boot-manager 同源，非 python
     重复实现）；显式开关，默认关，不破坏 Reject/CUT/fault 注入场景。
- 证据：`evidence/boot-unblock/`（失败现场导出字节 + 解码报告 + 复位验证）。

## 升级/更换流程

1. 替换固件文件（或设置 `ENEROS_QEMU_FIRMWARE`）；
2. 更新上表版本字符串与 SHA256（`sha256sum` 实测）；
3. 重跑 `cargo xtask image qemu-prod && cargo xtask run qemu-prod --reset-bootctrl`
   并确认 13 行启动断言与全部 bootctrl 证据 PASS；
4. 同步更新 `qemu_harness.rs` 注释块与本文件，提交。
