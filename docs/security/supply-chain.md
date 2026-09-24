# EnerOS 供应链与依赖审计

> 版本：v0.257.0（RC-3a，D3 安全评审准备）
> 依据：蓝图/14-阶段D-商用验收闸门.md D3「SBOM/依赖审计绿」；
> 数据：Cargo.lock（296 个锁定依赖）、deny.toml、CI cargo-deny 门禁。

## 1. 依赖锁定状态

- **Cargo.lock**：全 workspace 依赖锁定（296 个包），`cargo build --locked`
  强制一致解析；CI `cargo fetch --locked` + `cargo metadata --locked --offline`
  验证离线可解析。
- **toolchain pin**：`rust-toolchain.toml` 锁定 nightly-2026-04-04 +
  components（rust-src/clippy/rustfmt）+ targets（aarch64-unknown-none/
  aarch64-unknown-uefi），蓝图 §5.2 依赖锁定。
- **seL4 pin**：14.0.0 基线（docs/adr/ADR-0023）。third_party/sel4-bootstrap.sh
  对三源 tarball 做 sha256 fail-closed 校验：seL4 tag 14.0.0 逐字节钉死；
  seL4_tools/util_libs 钉 commit（与 2026-07 构建缓存解压树逐字节一致），
  moving branch（refs/heads/master）已退役。补丁面由
  third_party/sel4-converge-patches.sh 收敛器保证：受管文件
  （kernel boot.c/vspace.c、elfloader sys_boot.c/linker.lds）收敛到期望
  sha256 终态并逐文件校验，任何起始状态（fresh/部分应用/感染/漂移）
  都收敛到同一字节（2026-09-23 审计复核施工，DEF-52-01/-02 家族关闭）。
  边界如实声明：工具链 tarball（gcc/cmake/ninja）为版本化 release URL、
  哈希暂未固化；解压树完整性由受管文件收敛校验覆盖，非全树审计。
- **QEMU 钉版分面（WP-74.2.6，蓝图 74 卷二）**：seL4 构建面 qemu =
  xpack-qemu-arm 9.2.4-1，bootstrap 第 7 项钉版（tarball sha256
  fail-closed 校验，哈希取自官方 .sha 边车并与缓存解压树逐字节比对），
  与两构建脚本 `-DQEMU_BINARY`、sel4-powercut.sh 默认值单点一致；
  CI runner / 开发机**系统 qemu** 为另一面（xtask qemu_harness 生产证据
  线所用），版本随 runner/主机，不进 bootstrap 钉版——以 PATH 管道
  改造为前提的全面切换超最小面，登记为边界例外。内核 GICv2 驱动
  （seL4 14.0.0 qemu-arm-virt 平台唯一内置）要求启动命令 gic-version=2，
  与构建面 qemu 版本选择无关。

## 2. 许可证/漏洞门禁（CI fail-closed）

`deny.toml` + `cargo-deny check --manifest-path Cargo.toml`：
- 许可证策略（deny.toml 声明的允许集）；
- 漏洞 advisory 检查（RUSTSEC 数据库）；
- 重复依赖检查。

CI 输出为 fail-closed：任何违规 → 构建失败。

## 3. 自研 vs 第三方依赖面

| 面 | 自研（受控） | 第三方（审计） |
|---|---|---|
| 密码学 | SM2/SM3/SM4（product_allowed=false，待独立评审） | ed25519-dalek（密码评审覆盖） |
| 内核 | seL4（形式化验证） | — |
| AI 求解 | HiGHS（v1.7.2 pinned commit） | — |
| 协议 | Modbus/IEC104/IEC61850 自研 | — |
| LLM | llama.cpp（pinned source） | — |
| 解析/序列化 | toml/serde_json/heapless 等 | cargo-deny 审计 |

## 4. 评审证据

- `Cargo.lock`（sha256 可验证）；
- CI cargo-deny 日志（许可证/漏洞/重复三查）；
- `third_party/` 锁定源码清单；
- SBOM 生成：`xtask release sbom`（v0.249 确认 xtask/sbom.rs 实现）。

## 5. 残留事项

- 自研国密库 `product_allowed=false`：待 D3 密码学独立评审通过后解除
  （09 号文 §5.3 首项，外部评审依赖）；
- 部分 AI crate（rl/online/dialog/marl）为研究线隔离（A11），不入
  发布闭包——不构成供应链风险。
