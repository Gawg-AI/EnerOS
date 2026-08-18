# RECON-36 — 蓝图 36 侦察报告（事实源与真实性门禁止血）

> schema: `eneros.recon-36.v1`
> 审查基线：`HEAD=473b1e26681be33acc654f32e88251d8c19ad3cf` + 全部未提交/未跟踪开发代码
> 编制：2026-08-12（蓝图 36 执行）
> 前置：`蓝图/35-00`（16 P0/30 P1/5 P2）与 `蓝图/35-01`（事实总账）已接受为 NO-GO 基线

## 1. `product/profiles/**` 全部字段与 consumer

| 文件 | schema | consumer |
|---|---|---|
| `qemu-prod.json` | v2（本次升级：+kernel/services/configs/策略引用） | `xtask load_profile`（main.rs）、`audit_profile`、`image_factory`（构建/打包）、`product_manifest::normalize_profile`（36 新增） |
| `board-prod.json` | v1（兼容解析） | 同上 |
| `factory.json` / `recovery.json` / `qemu-dev.json` / `qemu-prod-sv2.json` / `host-unit.json` | v1 | 同上（v1 兼容路径） |
| `schema.json` | v2（本次升级：role 枚举扩展为 8 角色 + services/configs/kernel 字段） | 外部工具与一致性文档；xtask 以 `deny_unknown_fields` 严格解析做等价校验 |

字段清单（v2）：schema/id/class/platform/target/image_layout/artifacts/kernel/binaries/services/configs/security_policy_ref/evidence_policy_ref/signing/security。

## 2. xtask 中读取 profile/BOM/bundle/evidence 的全部 caller

- `load_profile`（main.rs:1100）：所有 audit/image/release 命令入口
- `audit_profile`（main.rs:631）：profile 审计
- `image_factory::command_image`：读 profile → 构建 → `build_init_bundle(root, sde, profile)`（36 改为 manifest 派生）
- `build_init_bundle`：**36 已替换硬编码 11 服务列表** → `normalize_profile` + `product_deriver::init_bundle`（WP-36.4）
- `release sbom`（sbom.rs）：读 profile + closure/binary-boms.json
- `artifact_inspector`：反解析 raw/recovery/installer/update
- `evidence::command_evidence`：读 `product/evidence/g0-g1-requirements.json`
- `qa::command_qa`（g7-audit/interop-report/kat-report/perf-gate/compliance-report）
- 36 新增：`manifest normalize/derive/registry/verify-bundle`、`policy kernel/adr`、`closure gate`、`gate docs`

## 3. root-task 最终 bundle consumer

`crates/runtime/root-task/src/bundle.rs`（ENRINIT2 解析）+ `spawn.rs`（服务 spawn 计划）。
bundle 条目 (kind, flags, name, ELF)：kind=优先级（1=root/2=required/3=optional），
36 的 manifest 派生与 image_factory compose 保持一致；反解析 Gate 由
`product_deriver::verify_media_bundle` 在 compose 后立即执行。

## 4. `cargo metadata` 正式 target resolved graph

- 13 个 manifest 服务（root-task + 4 demo + 6 drv/storage + boot-manager + installer）
- target 分组：`aarch64-unknown-none`（11 服务）、`aarch64-unknown-uefi`（boot-manager/installer）
- 36 改用 `cargo tree -e normal,build --prefix depth` 做精确闭包（metadata 的
  node.features 受 feature unification 污染：dev 边/其它 target 的启用全局可见，
  且 dep edge 不携带 features 字段——cargo 1.96 行为实测确认）

## 5. mock/fake/stub/simulated/null/test-support 标记来源

- `[dependencies]` 段启用 mock：**修复前** 2 处（modbus-rtu→rs485[mock]、pentest）；
  **修复后 0 处**（全部移至 `[dev-dependencies]`，WP-36.6）
- `[dev-dependencies]` 启用 mock：fs/framework/tsdb/recovery/ota-ab/durable-log/
  modbus-tcp/iec104-master/vpp-scenario 等（合法：不进 release 闭包）
- `product_allowed=false`：`eneros-crypto`（SM2/SM3/SM4，无商业批准）
- 36 处置：root-task/services/audit 的链式哈希 SM3→SHA-256（闭包移除未授权 crypto）

## 6. 会生成 PASS 的命令与 exit-code 分支（36 止血前后）

| 命令/脚本 | 止血前 | 止血后 |
|---|---|---|
| `qa g7-audit` | VERIFIED（test 存在即过）exit 0 | 只能 PENDING/G7-REVIEW，默认 exit 2；`--allow-non-pass` exit 0（non_release） |
| `qa interop-report` | 缺 skip 文件推 VERIFIED | 需 result JSON（verdict=PASS+peer_identity+oracle）；非 D2-PASS 写 non_release=true |
| `tests/interop/*.sh` | placeholder echo PASS exit 0 | 写 PENDING 证据 + exit 1 |
| `tests/soak/soak72.sh` | 空 executor/空 artifact hash PASS | 空 executor/无硬件 = PENDING exit 1；artifact_sha256 空 = FAIL exit 1 |
| `evidence verify` | 存在性检查不要求 PASSED | 保留；release 改用 `evidence gate`（verdict=PASS 才 0） |
| `qa perf-gate` | SLO=0/自基线放行 | SLO=0 或 baseline==candidate → FAIL |
| CI QEMU `\|\| test -s log` | timeout 容忍 | 已删除（NO_RESULT/FAIL） |
| `g1-artifacts.sh` toolchain `\|\| true` | 静默吞失败 | 显式捕获 rc（BLOCKED_EVIDENCE 记录） |

## 7. Go/No-Go / task card / evidence report 状态来源

- `docs/v1.0.0-go-no-go.md`：首屏 35 系列 NO-GO 横幅已存在；历史 PASS 表保留为失效历史
- task card：`docs/delivery/g0-g1/task-cards/*.yaml`（evidence verify/gate 读取 status）
- 36 新增 `gate docs`：go-no-go 必须含当前 NO-GO 裁决、不得引用 stale evidence（evidence/index.json 14 条 stale 全部列出）

## 8. 触及文件的未提交 diff 与保护策略

- 全部改动保持未提交（禁止自动 commit）；无 reset/clean/checkout
- 已备份被修改的 Cargo.toml（root-task/modbus-rtu）于 `.reasonix/tasks/*.bak`
- dirty worktree 证据以 `dirty_tree_digest` 标注，不冒充 clean release

## 9. 环境 BLOCKED_EVIDENCE

- QEMU 全链路（`gkernel-qemu-boot.log`）：本机无 SEL4_SRC 重建链完整证据 → BLOCKED
- Windows 跨平台验证：本机 Linux → BLOCKED（42 portability）
- 真机/HIL/interop 外部 peer：无 → PENDING（36 止血语义）
