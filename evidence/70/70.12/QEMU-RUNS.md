# 70.12 QEMU 目标实弹记录（2026-09-13）

## 每次运行实际启动的镜像（**权威值 = 各窗 qemu.argv / 运行日志的 `before_sha256`**）

| 窗 | 证据文件 | 启动镜像 sha256（前 16） | harness 记录 |
|---|---|---|---|
| A `probe-window` | `probe-window.qemu-serial.log` / `.qemu.argv` | `3ea55ee28f3053b1` | NO_RESULT（marker 未达） |
| B `boot-window` | `boot-window.qemu-serial.log` / `.qemu.argv` / `.qemu-result.json` | `a2fff2279284f377`（result 内 `image_initial_sha256=09d1fdc2…`） | **PASS** |
| D `fix-empty-bundle` | `fix-empty-bundle.qemu-serial.log` / `.qemu.argv` | `c0dd80f02b3022c9` | NO_RESULT（尾部停顿） |
| E `injected-probe` | `injected-probe.qemu-serial.log` / `.qemu.argv` | `dfc0d6b9019ca44c` | NO_RESULT（尾部停顿） |

> **不要用 `target/eneros/artifacts/**` 的当前字节去核对本目录日志**：本卷期间镜像
> 被重构多次（每次 `image raw` 会重写该路径），当前盘上字节已与任一窗无关。
> 本卷未存整盘镜像副本（130MiB × 4 不落仓）。**判据以外**的核对路径：
> ①窗内日志首尾的 harness 打印（`bootctrl reset before_sha256=… after_sha256=…`）；
> ②`*.qemu.argv`（含 `file=` 指向与设备参数）；③`boot-window.qemu-result.json`
> 的 `image_initial_sha256`/`image_final_sha256`。
> 已知事实：同一源码多次 `image raw` 摘要**非确定**（70.7 卷已登记该性质），故
> 本表逐一登记每窗启动值，而不是强行对齐。

## 构建（每窗之前）

```
ROOT_TASK_FEATURES="sel4-eipc,qemu-test-synth" bash third_party/sel4-build-rust.sh
SOURCE_DATE_EPOCH=1755523200 cargo xtask image raw qemu-test \
  --signing-key product/security/keys-local/dev-signing.key \
  --uefi-key product/security/keys-local/uefi-test.key \
  --uefi-cert product/security/keys-local/uefi-test.pem
```

`ENEROS_MGT_PROBE_{TOKEN,SIG,BUNDLE}` 是**构建期**注入（`root-task/build.rs` →
`include_bytes!`）：运行期设置**不改变**镜像内载荷（窗 C 实测无效）。

## 运行

```
cargo xtask run qemu-test --image <raw> --vars-template target/evidence61/template.fd \
  --data-disk <16MiB raw> --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --qmp-shutdown [--ready-marker <marker>] [--qmp-deadline-secs <secs>]
```

## 判据提取

```
grep -a "70.12 BOARD" <log>          # 板级装配绑定证据行
grep -ac PANIC <log>                 # 零泛豁免
grep -a "MGT-PROBE" <log>            # 探针链逐步（链式另存 *.probe-chain.txt）
grep -a "CONTROL-CHAIN" <log> | tail -1
```

## 未闭合（登记在册）

- `MGT-PROBE: sequence complete` 未取得：两窗（D/E）均停在
  `[svc-ota] BOOT-TRIAL: begin len=29`（`REG-70.12-PROBE-BOOT-TRIAL-STALL`）。
- 运行时 DTB 字节非确定 ⇒ `desc_digest` 逐次不同（`REG-70.12-DESC-DIGEST-VOLATILE`）：
  构建期 `ENEROS_BOARD_DESCRIPTION_DIGEST` 绑定会误拒，需改启动期比对或规范化摘要。
- 空载荷越界 panic 已修（`REG-70.12-PROBE-EMPTY-BUNDLE-PANIC` → Done）。
