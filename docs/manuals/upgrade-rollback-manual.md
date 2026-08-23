# EnerOS 升级与回滚手册（v1.0 候选版）

> 蓝图 50（WP-50.4）手册四件套之一。A/B 双槽 + trial 语义（bootctrl
> `ENERBC01`，记录布局见 `boot-protocol::boot_control`）。

## 1. A/B/trial 语义速览

| 概念 | 含义 |
|---|---|
| BOOT_A / BOOT_B | 两个完整系统槽；BOOTCTRL 分区保存两份互为镜像的控制记录 |
| sequence | 单调防回滚代数；**Genesis = 0**（产线 provision 归零） |
| trial | 新槽以 trial 态启动（tries_remaining 有限次），确认成功前旧槽仍可回退 |
| anchor | 受保护的锚记录（seq + record digest）；与盘上记录不符 → fail-closed Genesis 拒绝 |
| security_floor | 安全版本下限；低于下限的槽拒绝引导 |

## 2. OTA 流程

设备侧服务：svc-ota（EIPC `OTA1`）。管理入口经管理通道（MGT1
`update-check` / `update-apply` / `update-rollback`，需 admin 角色）。

```text
0x01 CHECK        目标版本/SKU/安全版本核验 → verdict
0x02 DOWNLOAD+VERIFY  包下载 + 签名验签（Ed25519）→ 分步状态
0x03 TRIAL        写入对侧槽并置 trial 引导（tries_remaining 递减）
0x04 COMMIT       试运行成功后提交（successful 置位，sequence 推进）
0x05 REGRESSION   显式回退到旧槽
0x06 PERSIST      trial 状态落盘（remaining‖version → svc-storage）
0x07 LOAD         重启后恢复 trial 状态（断电续审）
```

## 3. 失败处置

| 场景 | 系统行为 | 运维动作 |
|---|---|---|
| 新槽启动失败（trial 耗尽） | bootctrl 回退旧槽（priority 比较） | 核查新包签名/版本；重新 CHECK |
| 新包验签失败 | DOWNLOAD+VERIFY 阶段拒绝，不写槽 | 换正确签章的包 |
| 目标版本 ≤ 当前 sequence | REGRESSION 保护拒绝升级 | 如确需降级走显式回退流程（owner 批准） |
| trial 期间业务异常 | 未 COMMIT 前可 `update-rollback` | 记录故障现象后再回退 |

## 4. 断电场景

- **trial 中掉电**：trial 状态已 PERSIST 落盘；重启后 LOAD 恢复剩余尝试
  次数继续审——不会重复扣减也不会跳过审查。
- **COMMIT 后掉电**：sequence 已推进、双副本一致写入（原子双写）；重启
  直接进新系统。
- **BOOTCTRL 双副本不一致**（异常写入中断）：boot manager 判损坏 →
  fail-closed 拒绝引导（`error=Genesis` 类串口行）→ 按《制造手册》重做
  provision（Genesis 归零）。

## 5. 验证方法（QEMU/测试环境）

三态引导测试法（42 卷定型）：fresh VARS + seq=0 staged 盘起步验证
Identity 态；复用 run.vars 验证 trial/回退态。命令模板：

```bash
cargo xtask run qemu-prod --image <img> --vars-template <fd> \
  --reset-bootctrl --expect PASS --evidence-dir <dir> \
  --ready-marker product-ready --qmp-deadline-secs 300
```
