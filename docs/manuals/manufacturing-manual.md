# EnerOS 制造手册（v1.0 候选版）

> 蓝图 50（WP-50.4）手册四件套之一。产线工具 = `cargo xtask factory`
> （蓝图 16 实施，host 侧，Windows/Linux 双跑）。

## 1. 产线流程（provision）

### 1.1 前置条件

- 产品镜像：`target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw`。
- 站端签章键：`--sign-key <path>` **或** 环境变量 `ENEROS_SIGN_KEY` **或**
  KMS 目录 `ENEROS_KMS_DIR/factory.key`（47.3 显式注入纪律；**三者皆无 →
  工具拒绝执行**，fail-closed）。
- 点表事实源：默认 `configs/points.toml`（`--points` 可指定）。

### 1.2 整盘写入 + bootctrl Genesis + 制造记录

```bash
cargo xtask factory provision \
  --image  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  --target /dev/sdX                        # 真机盘；测试环境为 raw 文件路径
  --serial ENP-2026-000137 \               # 产线段分配序列号
  --station station-01 \
  --sign-key /secure/path/factory.key \
  --record  records/ENP-2026-000137.json
```

工具依次执行并自证：

1. 密钥解析（缺 → `产线签章键未提供…拒绝（fail-closed）` 并以非零退出）；
2. 镜像 GPT 解析 + BOOTCTRL 存在性校验；
3. bootctrl Genesis 初始化（sequence 归零、双副本一致）；
4. 点表摘要（SHA-256）入档；
5. 整盘写入目标介质；
6. 写后自检：全量 readback digest 比对 + GPT 复验 + bootctrl Genesis 复验，
   任一不符即失败退出（不产出记录）；
7. 制造记录落盘（schema `eneros.manufacturing-record.v1`，字段：
   serial / image_sha256 / points_sha256 / key_domain / bootctrl_genesis /
   readback_verified / station_id / provisioned_at_unix / result /
   signature_hex=Ed25519 签章）。成功输出：

```
FACTORY-PROVISION serial=ENP-2026-000137 image=… points=… key_domain=… genesis=true readback=verified result=PASS
```

## 2. 密钥注入 SOP

| 项 | 规则 |
|---|---|
| 键的来源 | 显式 `--sign-key` > `ENEROS_SIGN_KEY` > `ENEROS_KMS_DIR/factory.key` |
| 控制链信封密钥（WP-A.1） | 显式 `--control-chain-key-hex` > `ENEROS_CONTROL_CHAIN_KEY` > `$ENEROS_KMS_DIR/control-chain.key`；三者全缺 ⇒ bundle 编译 fail-closed（开发档模板 configs/kms-dev/） |
| 缺键行为 | **拒绝 provision**（非零退出），绝不降级为无签 |
| 记录内容 | 制造记录仅存 `key_domain`（站端公钥 SHA-256 前 16 hex）——可追溯不泄密；私钥永不出 KMS/工位机 |
| 设备身份密钥 | v1.0 经签名 config bundle 承载信任锚；每设备独立身份密钥注入面为 owner/KMS 后续项（50 卷如实登记） |

## 3. 抽检项（每批次 ≥1 台）

```bash
# a) 制造记录 schema 校验 + 签章验证 + 与在库镜像对账
cargo xtask factory verify-record \
  --record records/ENP-2026-000137.json \
  --sign-key /secure/path/factory.key \
  --image target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw
# 期望输出：FACTORY-RECORD-VERIFY serial=… schema=OK signature=VALID result=PASS

# b) 启动冒烟：provision 出的盘直接引导（QEMU/测试盘）
cargo xtask run qemu-prod --image <provisioned.raw> … --ready-marker product-ready
# 期望串口出现 PRODUCT-READY: true

# c) 篡改检测（负例抽检）：改动记录任一字段后 verify-record 必须报
#    `record signature INVALID`；换镜像对账必须报 `image digest mismatch`。
```

## 4. 制造记录留存

- 每台设备一条 JSON 记录，按 `records/<serial>.json` 归档。
- 字段与设备一一对应；`verify-record` 可随时离线复核（公钥可由站端
  KMS 导出验证，无需私钥）。
