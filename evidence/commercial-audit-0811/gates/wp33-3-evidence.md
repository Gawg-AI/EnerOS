# WP-33.3 OTA 硬化（SE-15/18/19）修复与证据

执行日期：2026-08-11。蓝图坐标：33 §5。

## 0. 侦察报告（开工前落盘）

- **波及面自证**：SE-15 `upgrade.rs`（check_monotonic/compare_versions）；
  SE-18 `rollback.rs`（PartitionTable 序列化）+ `kernel_ota.rs`（5 处
  save_to_device 调用点）+ `tests/ota_integration.rs`；SE-19
  `downloader.rs` + `crates/observability/ops-gate/tests/gr4_remote_ops.rs`
  （2 处 OtaDownloader::new 调用点，observability 在矩阵内）。均在 33 §1
  排斥矩阵 `crates/security/ota-ab/**` + `crates/observability/**` 内。
- **依赖方向**：`eneros_crypto::{hmac_sm3, ct_eq}` 已存在且 ota-ab 已依赖
  eneros-crypto——零新增依赖。
- **消费方清点**：`PartitionTable::{serialize,deserialize}`/`save_to_device`/
  `load_from_device` 生产消费方仅 kernel_ota.rs（5 处，矩阵内）；测试消费方
  rollback.rs / ota_integration.rs（矩阵内）+ `crates/security/recovery/tests/
  recovery_integration.rs`（矩阵外——签名变更必需涟漪，仅机械补参，不扩大
  改动，提交明示）。`OtaDownloader::new` 消费方：downloader.rs 测试 +
  gr4_remote_ops.rs（均矩阵内）。`KernelOta::new` 全仓唯一调用点在其自身
  测试（grep 实证）。
- **二次影响对照**：TABLE_LEN 增 32B（HMAC）——保留块布局变大，块 512B
  满足；`load_from_device` 校验失败回退安全默认语义不变；pst6 篡改字节
  仍命中拒绝路径。SE-15 变更后 `prepare_upgrade` 对不可解析版本返回
  VerifyFailed（当前分区版本恒 "0.0.0" 或合法元数据版本，无合法不可解析
  场景，grep 实证）。
- **测试基线**：开工前 ota-ab 38+5 / recovery 32+6 / ops-gate 2+3 全绿。

## SE-15：check_monotonic 不可解析版本 fail-closed

- **定位**：`upgrade.rs` compare_versions 解析失败返回 Equal（fail-open），
  check_monotonic 据此放行——未知格式版本可绕过防降级门禁。
- **修复**：`check_monotonic` 任一侧不可解析 → `Err(OtaError::VerifyFailed)`
  （parse_version 提为公共辅助，compare_versions 保留原语义供纯比较）。
- **证据**：`test_cry006_check_monotonic_rejects_downgrade` 更新——
  `check_monotonic("dev", "0.123.0")` 与 `check_monotonic("0.123.0","dev")`
  均必 Err(VerifyFailed)（旧判据为放行）。

## SE-18：分区表 SM3-HMAC 认证

- **定位**：`rollback.rs:134-156` 分区表仅 CRC32——CRC 可被无密钥方重算，
  伪造分区表（篡改 active_slot/分区状态后重算 CRC）可通过反序列化。
- **修复**：serialize 尾部追加 SM3-HMAC（覆盖魔数..CRC 全表，最末 32B）；
  deserialize 先常量时间验 HMAC 再验 CRC，任一失败 → None（安全默认）。
  密钥经参数注入：`save_to_device(ota, hmac_key)` /
  `load_from_device(ota, hmac_key, now_ms)` / `serialize(&self, hmac_key)` /
  `deserialize(buf, hmac_key)`；`KernelOta::new` 增 `table_hmac_key` 参数
  （与 root_key 同源派生注入，禁止硬编码——集成层职责，库内不内置密钥）。
- **证据**：新增 `pst8_forged_table_without_hmac_rejected`——攻击者模型
  篡改 active_slot 后重算 CRC（无 HMAC）→ deserialize 必拒；错误密钥必拒；
  正确密钥通过；持久化路径伪造保留块 → load 回退安全默认。
  测试 39+5 全绿（旧 38+5 + pst8）。
- **调试记录（次生缺陷）**：pst8 首版篡改目标选错（upgrade_to_b 后
  active_slot 已是 B，改 B 无实际变更致 HMAC 仍有效）——断言
  "篡改必须产生真实变更" 防回退，测试修正后通过。

## SE-19：downloader 预分配 bound

- **定位**：`downloader.rs:80` `Vec::with_capacity(self.total_size as usize)`
  按网络侧未信任 total_size 一次性预分配——恶意超大声明值可致 OOM。
- **修复**：`OtaDownloader::new(..., max_image_size, fetch) -> Result<Self, ()>`
  构造校验 `total_size <= max_image_size`（分区容量由集成层注入）；
  `download_all` 起步 `with_capacity(64 * 1024)` 随写入增量增长（Vec 倍增）；
  循环内运行时防越界（total_size 为 pub 字段，防调用方改大绕过校验）。
- **证据**：新增 `test_oversize_total_rejected_fail_closed`——超容量构造
  Err；恰等容量通过；pub 字段改大后 download 运行时 Err。

## 回归与门禁

- `cargo test -p eneros-ota-ab -p eneros-recovery -p eneros-ops-gate` 全绿
  （ota-ab 39+5 / recovery 32+6 / ops-gate 2+3）。
- `cargo clippy -p eneros-ota-ab -p eneros-recovery -p eneros-ops-gate
  --all-targets --no-deps --locked -- -D warnings` clean。
- **预存债务登记（非本 WP 引入）**：`crates/drivers/storage/src/driver/
  virtio_blk.rs:363` `mmio.read_reg(0x100 + 0)` 触发 clippy identity_op
  （31 提交 b7b970f 引入，已提交代码；storage 不在 33 排斥矩阵，按纪律
  不越界修复，登记留 34 处理——否则 workspace clippy 门禁失败）。
- 无新增硬编码物理地址/IRQ/板级常量；无真机依赖（HMAC 密钥注入，
  不内置）。
