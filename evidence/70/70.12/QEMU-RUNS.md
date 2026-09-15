# 70.12 QEMU 目标实弹记录（2026-09-13）

## 每次运行实际启动的镜像（**权威值 = 各窗 qemu.argv / 运行日志的 `before_sha256`**）

| 窗 | 证据文件 | 启动镜像 sha256（前 16） | harness 记录 |
|---|---|---|---|
| A `probe-window` | `probe-window.qemu-serial.log` / `.qemu.argv` | `3ea55ee28f3053b1` | NO_RESULT（marker 未达） |
| B `boot-window` | `boot-window.qemu-serial.log` / `.qemu.argv` / `.qemu-result.json` | `a2fff2279284f377`（result 内 `image_initial_sha256=09d1fdc2…`） | **PASS** |
| D `fix-empty-bundle` | `fix-empty-bundle.qemu-serial.log` / `.qemu.argv` | `c0dd80f02b3022c9` | NO_RESULT（尾部停顿） |
| E `injected-probe` | `injected-probe.qemu-serial.log` / `.qemu.argv` | `dfc0d6b9019ca44c` | NO_RESULT（尾部停顿） |
| F `dev-vouch-fix-window`（70.15 复查，见下） | `dev-vouch-fix-window.qemu-serial.log`（判据段抽样）/ `.qemu.argv` / `.verdict-lines.txt` | `f31c92348e5e5443` | 目标侧 **root-heartbeat PASS / PRODUCT-READY true / PANIC=0**；harness 侧 `exit=124`（关停超时，非启动失败） |

## 窗 F：70.15 复查——`vouch_development` 上开发入口的三层 P0 假阳性（2026-09-13）

F-70-12..14-REV 的 `REG-70.12-DEV-ENTRY-NO-VOUCH` 把描述判据带上开发入口后，
**开发镜像启动即 fail-closed 挂起**。本轮按"运行 → 判负原因 → 修 → 重跑"逐层剥出
**三层**，全部是**判据假阳性**（不是目标侧代码 bug）：

| 层 | 判负行（实弹原文） | 根因 | 修 |
|---|---|---|---|
| 1 | `38 HAL: assembly FAILED board refused: platform-bad-mmio:qemu,fw-cfg-mmio:unaligned-size` | `mmio_is_legal` 要求 `phys`/`size` 双 4096 对齐；QEMU virt 真实 DTB 里 `fw-cfg` = `0x18` 字节、32 个 `virtio_mmio` = `0x200`（`-machine dumpdtb` 实测，33 节点违规） | 删对齐两条，保留零容量/空地址，新增 `window-too-small` |
| 2 | `platform-coherency-without-fact:qemu,fw-cfg-mmio` | 开发入口账本**恒为空**（`empty_ledger`）、描述由平台自带（DTB 标 `dma-coherent`）⇒ "没有硬件事实"是结构性必然 | 判据加 `!ledger.facts.is_empty()` 前置（与同函数 `ledger.validate` 同形） |
| 3 | `platform-bad-mmio:arm,cortex-a57:zero-size` | CPU 节点的 `reg = <0x0>`（`cpus` 是 `#size-cells=0`）是 **CPU id** 不是 MMIO；旧实现把它当设备收录，设备表出现 `phys=0 size=0` 项 | `is_cpu` 节点不进设备清单（CPU 事实仍走 `tree.cpus`） |

**三层修完后的实弹结果**（窗 F，判据行逐字）：

```
[eneros-root-task] 38 HAL: source=fdt cpus=4 enabled=4 mem=1 devices=44
[eneros-root-task] 38 HAL: timer-freq=62500000 console=arm,pl011 entropy=none iommu=false
[eneros-root-task] 70.12 BOARD sku=qemu-virt-aarch64 revision=1.0.0 desc_digest=f0f65a2512f7a8c4 bound=false devices=0
{"schema":"eneros.g2.v1","test":"root-heartbeat","result":"PASS"}
[eneros-root-task] PRODUCT-READY: true (degraded: ; required degraded: )
```

`PANIC`/`alloc.rs`/`halting` 全零；串口 694,248 行（CR 分隔）。
`devices=0` 的解释：`vouch_development` 不要求网络类设备（有意差异，
virtio-mmio 网卡不描述为 ethernet/net 节点）⇒ `required_devices` 为空，
与"判据未执行"无关（判据是否执行由层 1–3 的挂起反证）。

> **harness 侧 `exit=124` 不是启动失败**：xtask 内层 `timeout --signal=KILL 200s`
> 在负载下未能在期限内关停 QEMU，外层 `timeout` 以 124 收尾；窗内判据行与
> `PRODUCT-READY` 均已产生（与 `REG-70.6-GATE-T1-WINDOW-KILL` 同族现象）。
> 本窗证据以**窗内判据行**为准。

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

### 第五轮（窗 L）：pre/post 把边界收到最后一步

插桩 `boot-write inner pre off=9437184 len=4096`（`BootSlotBackend::write` 的
inner 调用**前**）→ **打印**；紧随其后 `BlockBackend::write(&mut self.inner, …)`
进入 `VirtioBlkBackend::write` 首行的 `blk-write enter`（同字符串本窗更早已
打印 30+ 次）→ **不打印**。同窗其它服务仍在打印（TSDB 批提交 / CONTROL-CHAIN
round=37 / agent tick）⇒ console 未死。三处插桩字符串经
`strings -a target/eneros/artifacts/qemu-test/eneros-qemu-test.raw` 各命中 3 次
⇒ 插桩确实编入镜像。宿主 `%CPU=0.0`（10 次采样）⇒ 非自旋。

**结论**：阻塞位于 `inner pre` 之后、驱动方法体首条语句之前，且只在本线程/
本调用；其余服务不受影响。剩余可能：①该线程 console 打印路径阻塞
②调用边界（trait 分派）阻塞。下一步（5 行内）：该行改为"先 `AtomicU32`
自增再打印"，由 root 心跳读计数器即判定。

证据：`trial-instr-{a,b,c,d}.qemu-serial.log` + `*.trial-chain.txt`。

### 第六轮（窗 M）：console 路径排除 + 探针闭环可用

探针改造（`BOOT_WRITE_PRE_PROBES` 原子计数 + svc-ota 侧读取）闭环跑通：

```
[svc-ota] BOOT-TRIAL: begin len=29
[svc-ota] BOOT-TRIAL-PROBE pre_count=0          ← storage 侧探针计数（进入本次 trial 前）
[svc-ota] BOOT-TRIAL: TX-SENT slot_req=0x09 t=66427 body=24
[svc-storage] BOOT-TRIAL: RX req_len=25 (slot 0x09)
[svc-storage] BOOT-TRIAL: write0 lba=18432 len=4096 rec_lba=18432
[svc-storage] boot-write lba=18432 len=4096 allowed=true
[svc-storage] boot-write inner pre off=9437184 len=4096      ← 探针自增之后，打印**成功**
```

**判定（新事实）**：`boot-write inner pre` 行的**原子自增与打印都发生了**
（该行打印即证明自增先执行且打印成功）⇒ **console 打印路径未被阻塞**；
阻塞发生在**该行之后、驱动方法体第一条语句之前**，且只在本线程/本调用
（同窗其它服务持续打印、宿主 %CPU=0）。⇒ 排除"console 锁"这一可能，剩余
唯一嫌疑面 = **`BlockBackend::write(&mut self.inner, …)` 这一调用边界**
（trait 分派目标或其内部的首个内核操作）。

下一步（可直接执行，无需再改判定语义）：给 `VirtioBlkBackend::write` 方法体
**首行**加同样的"原子计数 + 打印"（而不是只打印），或对 `inner` 调用做
"调用前计数 / 调用后计数"两枚探针——两者都只需一次构建 + 一次窗口即可定论。
