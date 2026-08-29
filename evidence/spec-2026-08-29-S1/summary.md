# S1 卷执行摘要（SPEC-SERIES-2026-08-29/S1 · 2026-08-29）

```
卷        : 协议服务装配阶梯收编（WP-S1.1 + WP-S1.2）
基线      : 0.314.22 @ 687d1af → 收口 0.314.23 @ <bump 后 HEAD>
全仓      : 7817 → 7834 passed / 0 failed（Δ+17 = 本卷新增测试，零回归）
提交      : 12 个代码提交 + 1 个版本 bump（每步可独立 revert）
出口      : WP-S1.1 C1–C13 与 WP-S1.2 C1–C8 全部实跑留证；C8 PARTIAL（见下）
```

## 提交清单（执行序 = 规格迁移序）

| # | 提交 | 内容 |
|---|------|------|
| 1 | d701ee5 | agent-sdk config_gateway 内核（config_get/ConfigStep/response_body，6 测试） |
| 2 | 0344398 | protocol_ladder 内核 + service_cfg 管道单源重写（9 测试；C5 变异实证） |
| 3 | 86d4c6c | svc-goose 迁移（133→97） |
| 4 | ed4c59f | svc-sv 迁移（133→97） |
| 5 | 0a477c7 | svc-mms 迁移（207→160；drive_mms_read 留 bin） |
| 6 | af8cfd5 | svc-iec104 迁移（150→103；config_dump/300 轮显式化） |
| 7 | b0103ea | svc-modbus-tcp 迁移（420→387；DegradedStyle 引入；10 测试） |
| 8 | 52faf8a | svc-mqtt 迁移（624→589；SECURITY-CHECK/三端点留 bin） |
| 9 | 76f370e | svc-modbus-rtu 槽位迁移 + C6 冻结测试（assemble 词表不动） |
| 10 | 7101d90 | WP-S1.2 调用点收口（cfg_bindings×2/dispatch/ai_handler） |
| 11 | 9c73007 | C1/C3 合规收尾（mqtt capability_shell + 文档词形） |
| 12 | e15aadb | C10 实测抓缺修复（degraded 字段 ×4 + PointRegistry 导入） |
| 13 | 97e7ae8 | bump 0.314.23 + Cargo.lock 同步（R7） |

## 判据总表

| 判据 | 结果 | 证据 |
|------|------|------|
| WP-S1.1 C1 阶梯归零 | ✅ | WP-S1.1.md（grep exit=1） |
| C2 槽位字面量清零 | ✅ | 同上（17→0） |
| C3 eipc_resolve 单点 | ✅ | 同上（内核 1 / mqtt 3） |
| C4 六臂 host 可达 | ✅ | 同上（10 tests / 0.00s） |
| C5 词形变异红测 | ✅ | 同上（恰 1 红） |
| C6 槽位冻结红测 | ✅ | 同上（插边即红） |
| C7 crate 无回归 | ✅ | 同上（25 ≥ 15） |
| **C8 行数分档** | **⚠ PARTIAL** | 同上（预算未计 bin 样板地板 ~50 行；结构目标达成，-408 行） |
| C9 零新增 validate_config | ✅（附注） | 同上（唯一命中 = 测试假件必填 trait 方法） |
| C10 target 编译 | ✅ | 同上（EXIT=0；**实跑抓出 2 类 host 盲区缺陷**） |
| C11 clippy/fmt | ✅ | 同上 |
| C12 全仓 ≥7817 | ✅ | 同上（7834 / 0 failed） |
| C13 QEMU 判定行 diff | ✅ | 同上（双镜像双跑；归一化 diff 空） |
| WP-S1.2 C1–C8 | ✅ ×8 | WP-S1.2.md |

## 微偏差登记（M 系列——全部串口行为面，均已在代码注释就地登记）

| id | 偏差 | 影响 | 补偿 |
|----|------|------|------|
| M-1 | svc-modbus-tcp telemetry 绑定观测行从「点表行后、传输阶段前」前移至阶梯调用前（descriptor 后） | 一条观测行位置；C13 判定行集与 WP-S1.2 C8 计数集不受影响 | 无需 |
| M-2 | PointsPolicy 增 `Eager` 变体（modbus-tcp 点表行先于传输的现行行序保真） | 纯类型面扩充 | F-3 四轴漏计取回时机轴，执行中补 |
| M-3 | 阶梯路径 points 取数 `Unavailable`（含 get_entry 失败）统一呈现 `config EIPC NOT granted` 行；原 get_entry 失败打 `ConfigGet(points) EIPC fail: {e:?}` | 仅「已授予但请求超时」降级路径换词形 | M-3w 同族 |
| M-3w | RTU wrapper 的 resolve/建端失败为静默空表（原 NOT-granted 行） | 结构上不可达路径（边由 C6 冻结锁定） | 无需 |
| M-4 | 插件构造次数 2→1（原 descriptor 打印与 start 各构造一次；纯构造无副作用） | 无可观察差异 | 无需 |
| M-5 | goose/sv/mqtt `EmptyPointRegistry` → 空 `VecPointRegistry`（= 规格已登记的 DQ-9） | 插件经 `&mut dyn PointRegistry` 观察等价（resolve→None/len→0/collect_ids→空）；C4 臂 6 host 实证 | 无需 |
| M-6 | `DegradedStyle` 字段（Standard ×5 / modbus-tcp HandoverAlias）：try_new→_start 两段打印的降级词形逐字保真 | 纯类型面扩充；C13 实证两族词形不变 | 规格 §2「variance 全走数据参数」原则的延伸 |

## 规格判据口径的执行注记

- **C3 计数**：grep 计数与「调用点」对齐——文档改引 `eipc_grants` 模块名。
- **C9 过滤**：规格命令的 `grep -v "mod tests"` 只滤 mod 声明行；唯一
  命中为测试假件必填 trait 方法（F-24），生产路径零调用不变。
- **C4 红测对象**：config_gateway 无串口词形（R2），变异对象 = 分类映射
  （等价于词形面的实现/断言分离验证）。
- **C13 顺序**：多服务并发打印的行**编号**有运行间噪声，按「逐词形、内容
  多重集」diff；同进程内相对序逐进程核对一致。

## §5 待裁决清单增补（owner 追认）

| id | 问题 | 当前倾向 |
|----|------|---------|
| DQ-10 | M-1~M-6 微偏差（见上表）是否追认 | 全部行为等价或不可达路径；建议追认 |
| DQ-11 | C8 分档口径：维持 wc -l 绝对值 vs 改「非样板行数」（bin 样板地板 ~50 行不可省：cfg 门控/ALLOC/panic/main/七参签名） | 建议改口径后重评（结构归零已达成） |
| DQ-12 | F-15 漏计 ai_handler 第 5 处管道（规格事实表修正） | 已按 6 处收编；规格 v5 订正 |
| DQ-13 | svc-mgmt 的 get_entry 管道（逐请求 epoch）不在五处收编范围——其 decode 面自有实现，是否后续小卷收口 | 独立小卷 |

## 环境事实（QEMU 窗口可复现配方，本卷实证）

- `cargo +nightly` 在本机坏（Missing manifest）——**默认 toolchain 即
  nightly-2026-04-04，裸 cargo 等价**；交叉 GCC 需
  `PATH=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$PATH`。
- 镜像：`SOURCE_DATE_EPOCH=1785465257 cargo xtask image raw qemu-prod
  --signing-key product/security/keys-local/dev-signing.key --uefi-key
  product/security/keys-local/uefi-test.key --uefi-cert
  product/security/keys-local/uefi-test.pem`（uefi 密钥在 keys-local/ 下，
  非仓库根）。
- 干净树（worktree）构建需补 gitignored 输入：third_party/{llama.cpp,
  sel4-build,highs-1.7.2}、product/{config-bundle.bin,config-bundle-pubkey.hex,
  security/keys-local}、configs/kms-dev/control-chain.key（symlink 即可）；
  vars 模板若缺，从既有 artifacts 拷 AAVMF_VARS.fd。
- `cargo xtask run qemu-prod … --expect PASS` 的 exit=124（200s 无 ACPI
  截断）为已知基线，串口 transcript 即证据（qemu-serial.log）。
- QEMU 需 `PATH=$HOME/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH`。
