# SPEC-C QEMU 窗口证据汇总（2026-08-27）

## 窗口②（首成功窗）：target/evidence-spec-c-20260827/

| 判据 | 结果 | 留证行 |
|------|------|--------|
| 授予表单源直证 | ✅ | `EIPC-GRANT-TABLE edges=32 base_slot=3` |
| 新边① mint | ✅ | `EIPC-GRANT: svc-telemetry -> svc-storage slot=3 badge=13` |
| 新边② mint | ✅ | `EIPC-GRANT: svc-mgmt -> svc-telemetry slot=4 badge=25` |
| D-TSDB-2 方案 A 打穿 | ✅ | `[svc-telemetry] TSDB-DISK-OPEN region=blocks[8192,24576] LFS vol=256KiB` |
| mgmt 取数通道绑定 | ✅ | `[svc-mgmt] history EIPC bound (svc-telemetry)` |
| 就绪 | ✅ | `PRODUCT-READY: true (degraded: svc-mqtt; required degraded: )` —— mqtt 出站链为 DE-52-05 登记项，非必需服务 |
| PANIC | 0 条 | qemu-serial.log |

**C-SMOKE-VERDICT: PASS**（VERIFY.txt 六项全 1）。

## 负例帧（history-query point=99）＝BLOCKED-BY-DEF-52-05

客户端已发 CONNECT/CONNACK（broker.jsonl conn=46424），但 guest→broker 的
出站 TCP 本窗口 ReadFailed（svc-mqtt Degraded last_err=ReadFailed）——这正是
蓝图 52 终验登记的 **DEF-52-05「mqtt 出站即断」P1**，非本卷引入。w10 曾实证
同一 skeleton 下出站可活（重试铁律），属环境时序面。

复现指令（届时 owner/下一窗口执行）：
```bash
bash evidence/spec-2026-08-27-C/run_window.sh target/evidence-spec-c-win3 \
  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw 420
bash evidence/spec-2026-08-27-C/verify.sh    target/evidence-spec-c-win3
```
期望转绿判据：`hist_neg_frame=1`（transcript 收到 `Expected-FAIL +
no-such-point:99` 错误帧）。
若有数正例：先起 `evidence/47/modbus_slave47.py` 采集馈入使
`TSDB-DISK-COMMIT batch=` 行出现，再对存在点位发 history-query 比对逐字段。

## 本窗口新钉的环境事实（下次省 30 分钟）

1. **改了 agent-sdk 授予表 ⇒ 必须手动 `bash third_party/sel4-build-rust.sh`
   重链接再打包**：guest 的 root-task 来自 top-rust 内核重链接产物，cargo
   target 里的 ELF 不是 guest 实际跑的（本次窗口①两新边静默缺席的根因，
   重链接后 edges=32 直证闭环）。脚本无执行位须 `bash <script>` 显式调用。
2. littlefs2-sys 目标侧首次消费需 bindgen 交叉头：
   `BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/aarch64-none-elf --target=aarch64-none-elf"`；
   该 sysroot 缺失报错形态为 `/usr/include/stdint.h … libc-header-start.h`。
