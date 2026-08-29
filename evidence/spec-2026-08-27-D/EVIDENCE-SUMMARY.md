# SPEC-D 证据摘要（evidence/spec-2026-08-27-D）

日期：2026-08-27 · 基线 HEAD b5d939b + 本卷未提交增量 · workspace 0.314.18

## 窗口 window2（qemu-test 全链冒烟，C3 主判据）

- 媒体：`target/eneros/artifacts/qemu-test/eneros-qemu-test.raw`
  sha256=18d6f16170c062536c4201dc9aa4758fc0a11e8786e636dd6dc6706bcc7cf831
  （`xtask image raw qemu-test --signing-key product/security/keys-local/dev-signing.key …`）
- 驱动：`cargo xtask run qemu-test --image <媒体> --vars-template target/evidence46-l2/template.fd
  --expect PASS --net-mode present --net-user --reset-bootctrl --ready-marker root-heartbeat
  --qmp-deadline-secs 750`
- harness 收尾 RC=124（750s 截断；周期性 HEARTBEAT-KILL/fault 演练流程按 c45 先例
  `[ "$RC" = 124 ] && RC=0` 容忍）——判定以串口实测行为准（R1/R9）。

## 串口实测行（window2/qemu-serial.log）

| 行 | 内容 |
|----|------|
| 3077 | `EIPC-GRANT: svc-modbus-rtu -> svc-config slot=3 badge=14`（46 卷既有边零改动生效） |
| 3208 | `CAPS: svc-modbus-rtu=ep-send,ep-recv` |
| 3211 | `[protocol-service] modbus-rtu plugin=modbus-rtu role=Master interop=CodecReady` |
| 3271 | `[modbus-rtu] TIME-EPOCH=1787837249`（SPEC-B 墙钟链覆盖新 bin） |
| **3273** | **`[svc-modbus-rtu] RTU-UNAVAILABLE reason=no-handle —— capability 壳（EXECUTE ST_UNAVAILABLE）`**（C3 主判据：F-S1 空表下的诚实降级态） |
| 3274/3275 | `[drv-svc-modbus-rtu] ipc loop entered` / `READY-SIGNALED ntfn_slot=2` |
| 3337 | `READINESS: svc-modbus-rtu=Ready` |
| 27324 | `PRODUCT-READY: true (degraded: ; required degraded: )`——本窗零降级 |
| 全卷 | 零 PANIC；`RTU-ACTIVE` 词形 0 次出现（防伪：无柄不得谎报活路） |

## 判定

`bash verify.sh window2` → 七项判据全 1 → **D-C3-HONEST-SHELL-VERDICT: PASS**
（详见 window2/VERIFY.txt 与 judgment-lines.txt）

## host 判据索引

- C1/C2：crates/protocols/modbus-rtu/tests/serial_plugin_chain.rs（5/5 绿）
- D.1 单测：spawn.rs `proto_uart_slot_tests` ×4 + driver_probe 双实例标识测试
- 双键漂移锁：plugin.rs `d_known_keys_literal_lock` ↔ config-bundle protocol.rs `d_rtu_known_keys_literal_lock`
- fuzz 扩面：fuzz/fuzz_targets/modbus_rtu.rs（成帧层半帧垃圾；CI 靶名清单不变）

## 环境备忘（复现）

`image raw qemu-test` 必须：显式 `--signing-key/--uefi-key/--uefi-cert`
（ENEROS_IMAGE_SIGNING_KEY 环境变量对 raw 模式无效）+
`BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/aarch64-none-elf"`
（littlefs2-sys bindgen 走 newlib 头；宿主 gcc include 目录不能解 stdint 链）。
