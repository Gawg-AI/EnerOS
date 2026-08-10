# WP-29.3 安全 P0（SE-01~05）修复与证据

执行日期：2026-08-11。蓝图坐标：29 §5（WP-29.3）。

## A. 波及面自证

| 条目 | 文件 | 修复 |
|---|---|---|
| SE-01 | attestation/src/lib.rs（TpmError）+ Cargo.toml + .github/workflows/ci.yml | `KeyUnavailable` 变体 + From 映射归并 QuoteFailed；CI 矩阵增 tpm2 check+test |
| SE-02 | secure-boot/src/manifest.rs + secure_boot.rs + tests/secure_boot_test.rs + 蓝图/25 台账 | verify_and_parse 无状态直验 + min_timestamp 参数化 + 全仓消费点替换 + parse() deprecated |
| SE-03 | attestation/src/real.rs + Cargo.toml | Tpm2Backend::new(tpm, ak_handle) 注入；DER→r‖s（crypto-gmssl der_to_rs，tpm2 feature 挂 optional dep） |
| SE-04 | attestation/src/real.rs | encode 扩为 RATT‖ver‖nonce‖pcr_select‖pcr_values‖quote_time‖signature；decode_result 按回码映射 reason |
| SE-05 | attestation/src/lib.rs + Cargo.toml | MockAttestTransport 整体 cfg(any(test, feature="mock"))；mock feature 补声明 |

## B. 依赖方向检查

attestation 新增 optional 依赖 eneros-crypto-gmssl（仅 tpm2 feature 激活，
std 面 vetted binding，ADR-022）；不引入反向依赖；secure-boot 依赖方向不变。

## C. 消费方清点

- TpmError：全 match 面仅 lib.rs From 映射 + real.rs（grep 核实），同步完成。
- Tpm2Backend：零外部消费（仅 real.rs 内部）——new 签名变更零破坏。
- verify_and_parse：原零成功消费（Bug）；现 4 处（secure_boot.rs 测试）+
  6 处（tests/secure_boot_test.rs）全部替换为验签路径。
- MockAttestTransport：仅 attestation 内部测试消费——门控后默认构建符号消失。

## D. 二次影响对照（S 条目）

- S1：P0 不隔夜——SE-01/SE-02 本批关闭。
- S2：SE-05 门控后全项目 mock 门控达标（生产构建 doc 无 MockAttestTransport
  符号页，grep 核实）。
- S8：SE-04 证据链自包含（远端可离线重算 digest 验签）；SE-02 信任链下级
  公钥绑定恢复。
- 不变量：验签失败即 Err（无新绕过）；attestation 任一环节 Err → 证明失败。

## E. 测试基线（改动后）

- `cargo check -p eneros-attestation --features tpm2`：通过（SE-01 判据，
  修复前 E0599）。
- `cargo test -p eneros-attestation --features tpm2`：33 passed 全绿。
- `cargo test -p eneros-secure-boot`：42 unit + 8 integration 全绿
  （新增正例 test_verify_and_parse_accepts_signed_image + 张冠李戴负例
  test_verify_and_parse_rejects_mismatched_signer + 防降级
  test_verify_and_parse_rejects_stale_timestamp）。
- `cargo test -p eneros-tpm`：全绿（SE-07 基线）。
- SE-04 环回判据：encode→解码 Quote 材料→重算 digest→sm2_verify 通过
  （test_encode_decode_requote_verify_roundtrip）。
- SE-03 判据：DER 72B 注入→r‖s 转换→验签通过（test_der_signature_to_rs_verify）。

## 台账

蓝图/25 头部新增台账修正：B4-02（v0.235.0 secure-boot 绑定销项）标
"实际失效，由 29 SE-02 修复重开"。
