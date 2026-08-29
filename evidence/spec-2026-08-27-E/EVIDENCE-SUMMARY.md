# SPEC-SERIES-2026-08-27/E · 证据汇总（2026-08-27）

## 一、host 侧（全绿，7777 测试 / 392 套件，cargo 退出码 0）

| WP | 判据 | 锚点 |
|----|------|------|
| E.1 | C1：围栏/别名/碎裂/垃圾/超大/不扩权 9 测试 | `crates/ai/intent-contract/src/peer.rs`（45 绿）|
| E.2 | C1：append→重启读回逐序一致；中段篡改 verify_log 定位 fail-stop；截尾 prefix 收敛；容量丢新计数；锚不符拒续 | `crates/security/audit/src/durable_log.rs`（6 绿）|
| E.3 | C1：正/反噪声序列半宽单调+catalog 钳；C2：闸关零写零消费 + durable 写失败整步回滚；C4：learn-reset 回出厂并持久；跨重启快照逐字段恢复；wire KAT | `crates/agents/control-services/src/learn.rs`（7 绿）+ `ai_worker.rs` e3/e4 测试 |
| E.4 | SOC 死区带单源（MID±FACTORY_HALF_WIDTH），出厂阈值派生唯一 | `ai_worker::soc_band` |

## 二、QEMU 实弹（qemu-test 镜像 0715514a…，serial=window1 → target/evidence-e/win1-t2）

**已达成（目标侧接线实证，serial 可 grep）**：

| 判据行 | 含义 |
|--------|------|
| `[svc-control-dispatch] AUDIT-CHAIN-DURABLE fresh base_block=24576 slots=64` | E.2 审计区段（CAQ-2 下方划区）真持久装配上电，容量策略显式 |
| `[svc-ai-worker] LEARN-STATE restored=1 current=0.100000 min=0.060000 max=0.140000 samples=0 rejected=0` | E.3 学习器装配（factory=0.1，catalog 界 [0.06,0.14]） |
| `[svc-ai-worker] LEARN-THRESHOLDS charge=0.300000 discharge=0.500000` | E.4 单源派生阈值（=MID±FACTORY_HALF_WIDTH）上电 |
| `[svc-ai-worker] AI-PEER-CFG loaded … learn=true` | E.3 学习闸（ai-peer 新键）经 bundle 全链生效 |
| `PRODUCT-READY: true` | 服务面整体无回归 |

**BLOCKED（预存缺陷，非本卷引入；登记待 owner/后续卷）**：

| 判据 | 阻断点 | 实证行（serial） |
|------|--------|------------------|
| E.3-C3 学习闭环全链（LEARN-APPLY 序列/边界对照帧） | 可信遥测无源：多分块 TEL_INGEST chunked 传输 target 侧 Transport 失败（单分块 svc-config 正常——ai-peer 配置取数成功），合成注入路径在 elfloader 构建链从未启用（45.5 feature 门控 + `sel4-build-rust.sh` 不带 feature） | `[svc-modbus-tcp] INGEST eipc=Transport` |
| E.2-C2 重启续链 / E.2-C3 篡改负例 | 同上——审计链需要控制事件，事件需要可信遥测（12 态链 step 2）；空链重启=回 Fresh，无续链对照 | `AUDIT-CHAIN appended=…` 无 |
| E.1-C2 围栏对端编排 | qemu-test 装配 mqtt 无 NIC（root 不予协议面移交），DEF-52-05 同域 | `[svc-mqtt] handover missing` |
| （辅助）编排节奏 | net 轮询负载下根环 tick ~24s 且近乎冻结；本卷已将链周期 100→5 tick（自愈重试，round=1 已能发出） | `CONTROL-CHAIN: round=1 telemetry-ingest=0` |

## 三、复现指令

```bash
# host 全量
cargo test --workspace
# 镜像（root-task 特性参数化后）
ENEROS_SIGN_KEY=product/security/keys-local/dev-signing.key \
ENEROS_KMS_DIR=configs/kms-dev SOURCE_DATE_EPOCH=1755523200 \
BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/aarch64-none-elf" \
  cargo xtask image qemu-prod   # 或 image raw qemu-test --signing-key …
# 窗口（馈入件先起：broker+振荡从站+对端+scada）
bash evidence/spec-2026-08-27-E/window_prod.sh target/evidence-e/<dir> 720
WINDOW_MEDIA=target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  bash evidence/spec-2026-08-27-E/window_prod.sh target/evidence-e/<dir> 720
# 核验
bash evidence/spec-2026-08-27-E/verify.sh target/evidence-e/<dir>
```

## 四、本窗口新增环境事实（下次省 1 小时）

1. `sel4-build-rust.sh` root-task feature 已参数化：`ROOT_TASK_FEATURES="sel4-eipc,qemu-test-synth"` 可启用合成遥测——但 synth 注入的 chunked 传输本身 Transport 失败（45.5 从未上过靶），启用无益，修通前勿用。
2. bundle 为**单文件跨 profile**（`product/config-bundle.bin`）：`config compile --profile qemu-prod` 产物供 qemu-test 镜像嵌入；qemu-test profile 缺 protocol-dlt645 声明（CF-01 预存）。
3. 根环编排分支周期已改：链 5-tick / EXECUTE 15 / OBS 25（service.rs 三常量）。
4. svc-modbus-tcp 插件 start 的 `open()` 即建连——**从站馈入必须先于 QEMU 启动**，否则插件落壳（45/47 同款教训）。
5. 假对端三件套在 `evidence/spec-2026-08-27-E/`：`peer_brain_e.py`（e1-cycle：fence→drift→strings）、`modbus_slave_e.py`（SOC 475↔525 振荡）、`window_prod.sh`。
