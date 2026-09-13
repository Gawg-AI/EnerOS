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

## 追加：BOOT-TRIAL 停摆的插桩定位（2026-09-13 第四轮，三窗 G/H/I）

按 `REG-70.12-PROBE-BOOT-TRIAL-STALL` 的定位建议加了两侧插桩（纯观测，不改语义）：

- `svc-ota.rs`（trial 臂）：`BOOT-TRIAL: TX-SENT slot_req=0x09 …` +
  退出臂 `RX-OK resp=… blocked_ms=…` / `RX-ERR …`
- `drv-svc/src/boot_install.rs`：`write0 lba=… len=… rec_lba=…` /
  `write1 lba=…` / `flush` / `done seq=…`
- `drv-svc/src/blk_backend.rs`：`BootSlotBackend::write` 的
  `boot-write lba=… len=… allowed=…` + `inner done`；
  `VirtioBlkBackend::write` 的 `blk-write enter lba=… len=… bs=…`
- `drv-svc/src/block_cmd.rs`：`BOOT-TRIAL: RX req_len=… (slot 0x09)`

三窗（`trial-instr-a/b/c`）输出**逐行一致**，链在第四行截断：

```
[svc-ota] BOOT-TRIAL: begin len=29
[svc-ota] BOOT-TRIAL: TX-SENT slot_req=0x09 t=… body=24
[svc-storage] BOOT-TRIAL: RX req_len=25 (slot 0x09)
[svc-storage] BOOT-TRIAL: write0 lba=18432 len=4096 rec_lba=18432
[svc-storage] boot-write lba=18432 len=4096 allowed=true      ← 窗 B 有（该插桩后加），窗 A/C 无
```

**从此再无任何输出**：驱动面 `blk-write enter` 不打印、`write1/flush/done`
全无、`RX-OK/RX-ERR` 全无、无 PANIC、无 `blk-wait TIMEOUT`。
同期宿主 `%CPU=0.0`（10 次采样，窗 J）⇒ **不是自旋**。

**判定**：trial 的 bootctrl **首写**（lba 18432 扇区 = 字节 9437184，4 KiB，
守卫已放行）卡在"进入驱动写入循环之前的阻塞等待"；`svc-ota` 与 `svc-storage`
都是被动等待者。下一步（10 行内）：在 `BootSlotBackend::write` 的 inner 调用
前后各加一行，区分"调用边界阻塞"与"驱动内部阻塞"。
