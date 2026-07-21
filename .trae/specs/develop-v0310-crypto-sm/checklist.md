# Checklist — v0.31.0 国密算法库

> 验证清单：所有检查项必须通过才能标记版本完成。国标测试向量（KAT）是硬性验收标准。

## 一、目录结构校验（§2.4.1）

- [x] **C1 新 crate 位置**：`crates/security/crypto/` 在新增子系统 `crates/security/` 下，未放根目录
- [x] **C2 workspace members**：根 `Cargo.toml` 的 members 添加 `"crates/security/crypto"`
- [x] **C3 跨 crate path 引用**：`crates/security/crypto/Cargo.toml` 无外部依赖（纯 Rust 实现）
- [x] **C4 文档分类**：新文档在 `docs/security/` 下（首次创建此目录），未平面化放 `docs/` 根
- [x] **C5 无根目录 crate**：仓库根目录下无新增 Rust crate 文件夹

## 二、crypto crate 基础设施校验

- [x] **C6 crate 创建**：`crates/security/crypto/Cargo.toml` + `src/lib.rs` 创建
- [x] **C7 Cargo.toml**：name="eneros-crypto", version="0.31.0", 无外部依赖
- [x] **C8 lib.rs no_std**：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + VERSION="0.31.0"
- [x] **C9 error.rs**：CryptoError 13 变体（实际实现命名更规范：InvalidLength/InvalidInput/InvalidKeyLength/InvalidNonceLength/ModInverseFailed/PointNotOnCurve/InvalidPointEncoding/ScalarOutOfRange/SignatureInvalid/TagMismatch/InvalidPadding/RngFailed/InternalError；与 checklist 描述的变体名不同但功能等价覆盖相同场景）
- [x] **C10 error.rs is_security_critical()**：正确识别 7 个安全关键错误（SignatureInvalid/TagMismatch/InvalidPadding/PointNotOnCurve/InvalidPointEncoding/ScalarOutOfRange/ModInverseFailed；比 checklist 描述的 4 个更全面）
- [x] **C11 constant_time.rs ct_eq**：恒定时间比较（XOR 累积，长度不等提前返回）
- [x] **C12 constant_time.rs ct_zeroize**：用 `core::ptr::write_volatile` + `compiler_fence` 防优化

## 三、U256 大整数运算校验

- [x] **C13 U256 结构体**：`limbs: [u64; 4]` 小端序 + Clone/Copy/Debug
- [x] **C14 U256 常量**：ZERO / ONE
- [x] **C15 U256 from_be_bytes / to_be_bytes**：32 字节大端序转换
- [x] **C16 U256 from_hex**：解析 64 字符十六进制字符串
- [x] **C17 U256 cmp / is_zero**：比较与零判断
- [x] **C18 U256 bit / bit_len**：位操作
- [x] **C19 U256 add_mod / sub_mod**：模加模减
- [x] **C20 U256 mul_mod**：模乘（256×256→512 中间结果）
- [x] **C21 U256 inv_mod**：扩展欧几里得模逆
- [x] **C22 U256 测试**：加减乘模逆 + hex 解析 + bit 操作（20+ tests）— 64 tests PASS

## 四、SM3 密码杂凑校验

- [x] **C23 SM3 IV 常量**：`[0x7380166F, 0x4914B2B9, ...]`
- [x] **C24 SM3 T/FF/GG/P0/P1 函数**：按 GB/T 32905-2016
- [x] **C25 SM3 message_expand**：W[0..68] + W'[0..64]
- [x] **C26 SM3 compress**：64 轮压缩函数 CF
- [x] **C27 Sm3Hasher 结构体**：state/buffer/buffer_len/total_len
- [x] **C28 Sm3Hasher::new / update / finalize**：流式接口（含填充逻辑）
- [x] **C29 Sm3::hash**：便捷函数
- [x] **C30 Sm3Hasher::Default**：实现 Default trait
- [x] **C31 SM3 KAT 1**：输入 "abc" → 66c7f0f4 62eeedd9 d1f2d46b dc10e4e2 4167c487 5cf2f7a2 297da02b 8f4ba8e0
- [x] **C32 SM3 KAT 2**：64 字节消息 → debe9ff9 2275b8a1 38604889 c18e5a4d 6fdb70e5 387e5765 293dcba3 9c0c5732
- [x] **C33 SM3 流式测试**：分块 update 与整块 hash 结果一致
- [x] **C34 SM3 单元测试**：边界情况 + 空输入 + 长输入（15+ tests）

## 五、SM4 分组密码校验

- [x] **C35 SM4 SBOX**：256 字节固定表
- [x] **C36 SM4 FK / CK 常量**：系统参数 + 固定参数
- [x] **C37 SM4 tau / l_transform / t_transform**：非线性/线性/合成变换
- [x] **C38 SM4 l_prime / t_prime**：密钥扩展用变换
- [x] **C39 SM4 key_expand**：32 轮密钥扩展
- [x] **C40 SM4 encrypt_block / decrypt_block**：32 轮 Feistel + 反序 R
- [x] **C41 Sm4 结构体**：round_keys: [u32; 32]
- [x] **C42 Sm4::new / encrypt_block / decrypt_block**：接口实现
- [x] **C43 SM4 KAT**：key=0123...3210, pt=0123...3210 → ct=681edf34d206965e86b3e94f536e4246
- [x] **C44 SM4 加解密一致性**：同密钥同明文加解密还原
- [x] **C45 SM4 单元测试**：边界 + 多轮加解密（10+ tests）

## 六、SM4-CBC 模式校验

- [x] **C46 Sm4Cbc 结构体**：cipher: Sm4 + iv: [u8; 16]
- [x] **C47 Sm4Cbc::new**：构造函数
- [x] **C48 Sm4Cbc::encrypt**：PKCS#7 填充 + CBC 链接
- [x] **C49 Sm4Cbc::decrypt**：去填充 + 验证 PKCS#7
- [x] **C50 SM4-CBC 加解密一致性**：任意长度明文加解密还原
- [x] **C51 SM4-CBC PKCS#7 边界**：0 字节 / 16 字节 / 17 字节填充测试
- [x] **C52 SM4-CBC 单元测试**：10+ tests

## 七、SM4-GCM 认证加密校验

- [x] **C53 Sm4Gcm 结构体**：cipher: Sm4 + nonce: [u8; 12]
- [x] **C54 GHASH 函数**：GF(2^128) 多项式乘法
- [x] **C55 Sm4Gcm::encrypt**：CTR 模式加密 + GHASH 认证，返回 (ciphertext, tag)
- [x] **C56 Sm4Gcm::decrypt**：恒定时间 tag 比较（用 ct_eq）
- [x] **C57 SM4-GCM 加解密一致性**：含 AAD 加解密还原
- [x] **C58 SM4-GCM tag 篡改失败**：tag 不匹配返回 AuthTagMismatch
- [x] **C59 SM4-GCM AAD 测试**：空 AAD / 非空 AAD
- [x] **C60 SM4-GCM 单元测试**：15+ tests

## 八、CSRNG 安全随机数生成器校验

- [x] **C61 CsRng 结构体**：state: [u8; 32] + counter: u64
- [x] **C62 CsRng::new**：固定种子（标注禁止用于生产）
- [x] **C63 CsRng::from_seed**：从种子构造
- [x] **C64 CsRng::fill_bytes**：基于 SM3 DRBG 填充
- [x] **C65 CsRng::next_u32 / next_u64**：便捷方法
- [x] **C66 CsRng::reseed**：重新播种
- [x] **C67 CsRng::Default**：实现 Default trait
- [x] **C68 CSRNG 不重复测试**：1000 个 32 字节随机数无重复
- [x] **C69 CSRNG 确定性测试**：同种子同输出
- [x] **C70 CSRNG 单元测试**：10+ tests

## 九、SM2 椭圆曲线运算校验

- [x] **C71 Sm2Curve 常量**：P/A/B/N/GX/GY 按 GB/T 32918.1-2017
- [x] **C72 EcPoint 结构体**：x/y: U256 + is_infinity: bool + Clone/Debug
- [x] **C73 EcPoint::generator**：返回基点 G
- [x] **C74 EcPoint::is_on_curve**：验证 y^2 ≡ x^3 + ax + b mod p
- [x] **C75 EcPoint::add**：点加（处理无穷远点/相同点/相反点）
- [x] **C76 EcPoint::double**：点倍（λ = (3x^2+a)/(2y) mod p）
- [x] **C77 EcPoint::scalar_mult**：Montgomery ladder（恒定时间）
- [x] **C78 EcPoint::scalar_base_mult**：k * G
- [x] **C79 EcPoint::to_bytes_uncompressed**：04 ‖ x ‖ y
- [x] **C80 EcPoint::from_bytes**：解析 04/02/03 前缀
- [x] **C81 基点在曲线上**：G.is_on_curve() == true
- [x] **C82 SM2 曲线运算测试**：点加/倍/标量乘法 + KAT（20+ tests）

## 十、SM2 密钥对校验

- [x] **C83 Sm2PrivateKey**：`pub [u8; 32]` + as_bytes() + zeroize()
- [x] **C84 Sm2PublicKey**：x/y: [u8; 32]
- [x] **C85 Sm2KeyPair 结构体**：private_key + public_key
- [x] **C86 Sm2KeyPair::generate**：生成密钥对（用 CSRNG）
- [x] **C87 Sm2KeyPair::from_private_key**：从私钥派生（公钥 = sk * G）
- [x] **C88 Sm2KeyPair::public_key_bytes**：04 ‖ x ‖ y（65 字节）
- [x] **C89 SM2 密钥对派生测试**：from_private_key 公钥 == sk*G

## 十一、SM2 数字签名校验

- [x] **C90 Sm2Signature**：r/s: [u8; 32]
- [x] **C91 Sm2Signer 结构体**：user_id: Vec<u8>
- [x] **C92 Sm2Signer::new**：默认 user_id = "1234567812345678"
- [x] **C93 Sm2Signer::with_user_id**：自定义 user_id
- [x] **C94 Z 值计算**：SM3(ENTL ‖ ID ‖ a ‖ b ‖ Gx ‖ Gy ‖ Px ‖ Py)
- [x] **C95 Sm2Signer::sign**：完整签名流程（e=SM3(Z‖M)，循环生成 k）
- [x] **C96 Sm2Signer::verify**：验签（恒定时间比较）
- [x] **C97 sm2_sign / sm2_verify 便捷函数**：使用默认 Sm2Signer
- [x] **C98 SM2 签名+验签端到端**：自生成密钥对签名验签通过
- [x] **C99 SM2 篡改消息验签失败**：签名后篡改消息返回 false
- [x] **C100 SM2 篡改签名验签失败**：篡改 r/s 返回 false
- [x] **C101 SM2 签名 KAT**：GB/T 32918.5-2017 向量
- [x] **C102 SM2 签名单元测试**：15+ tests

## 十二、SM2 公钥加密校验

- [x] **C103 sm2_encrypt**：生成 C1 ‖ C3 ‖ C2（国标顺序）
- [x] **C104 sm2_decrypt**：解析 + 验证 C3 + 解密 C2
- [x] **C105 KDF 密钥派生函数**：基于 SM3
- [x] **C106 SM2 加解密一致性**：公钥加密 + 私钥解密还原
- [x] **C107 SM2 篡改密文解密失败**：C3 不匹配返回 DecryptionFailed
- [x] **C108 SM2 空明文测试**：加密空明文解密还原
- [x] **C109 SM2 加密 KAT**：GB/T 32918.5-2017 向量
- [x] **C110 SM2 加密单元测试**：10+ tests

## 十三、lib.rs 模块完善校验

- [x] **C111 lib.rs 模块声明**：所有 7 个子模块（error/constant_time/bigint/sm3/sm4/sm2/rng）
- [x] **C112 lib.rs re-exports**：pub use 导出所有公共类型
- [x] **C113 lib.rs 文档注释**：架构图 + 使用示例 + 偏差声明 + 国标引用

## 十四、国标 KAT 集成测试校验

- [x] **C114 tests/sm3_kat.rs**：SM3 国标测试向量文件创建
- [x] **C115 SM3 KAT 1 通过**："abc" → 66c7f0f4...
- [x] **C116 SM3 KAT 2 通过**：64 字节消息 → debe9ff9...
- [x] **C117 tests/sm4_kat.rs**：SM4 国标测试向量文件创建
- [x] **C118 SM4-ECB KAT 通过**：key=0123...3210 → ct=681edf...4246
- [x] **C119 SM4-CBC KAT 通过**（如有国标向量）
- [x] **C120 SM4-GCM 测试通过**：NIST GCM 测试向量改编
- [x] **C121 tests/sm2_kat.rs**：SM2 国标测试向量文件创建
- [x] **C122 SM2 签名 KAT 通过**：GB/T 32918.5-2017
- [x] **C123 SM2 加密 KAT 通过**：GB/T 32918.5-2017

## 十五、构建校验（§2.4.2）

- [x] **C124 cargo metadata**：`cargo metadata --format-version 1 > /dev/null` 成功
- [x] **C125 cargo build eneros-crypto**：`cargo build -p eneros-crypto` 编译成功
- [x] **C126 cargo test eneros-crypto**：`cargo test -p eneros-crypto` 通过（目标 150+ tests，含 KAT）
- [x] **C127 aarch64 交叉编译**：`cargo build -p eneros-crypto --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] **C128 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C129 cargo clippy**：`cargo clippy -p eneros-crypto --all-targets -- -D warnings` 无 warning
- [x] **C130 cargo deny check**：licenses/bans/sources 通过；advisories 允许因 GitHub 网络不可达跳过
- [x] **C131 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部 PASS
- [x] **C132 eneros-ci**：`cargo run -p eneros-ci` Overall: PASS（audit 允许跳过）

## 十六、文档与规范校验

- [x] **C133 sm-crypto-design.md**：v0.31.0 设计文档已创建，含国标引用 + 算法说明 + 性能基准 + 内存预算 + OOM 策略 + 偏差声明
- [x] **C134 docs/security/README.md**：首次创建 docs/security/ 目录索引（如需要）
- [x] **C135 文档位置**：文档在 `docs/security/` 下，未放 `docs/` 根
- [x] **C136 无垃圾文件**：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪

## 十七、版本标识校验

- [x] **C137 根 Cargo.toml**：workspace.package.version = "0.31.0"
- [x] **C138 eneros-crypto Cargo.toml**：version = "0.31.0"
- [x] **C139 lib.rs VERSION**：`pub const VERSION: &str = "0.31.0"`
- [x] **C140 Makefile**：VERSION := 0.31.0 + 含 crypto-build/crypto-test 目标
- [x] **C141 ci.yml**：Version: v0.31.0 + eneros-crypto 在构建步骤中
- [x] **C142 gate.rs**：注释含 v0.31.0 说明 + eneros-crypto 配置

## 十八、设计原则合规

- [x] **C143 Karpathy Think Before Coding**：国标合规策略明确（GB/T 引用 + KAT 硬性验收）
- [x] **C144 Karpathy Simplicity First**：U256 固定大小 + 纯软件实现 + 仅 ECB/CBC/GCM + 默认 user_id
- [x] **C145 Karpathy Surgical Changes**：v0.27.0~v0.30.2 源文件未修改；仅新增 crate + 修改根 Cargo.toml/Makefile/ci.yml/gate.rs
- [x] **C146 Karpathy Goal-Driven Execution**：国标 KAT 是验收标准 + 端到端签名/加解密测试
- [x] **C147 ADR 合规**：未引入自研重复组件，纯 Rust 实现遵循蓝图 §5.1 选型决策
- [x] **C148 偏差声明**：spec.md 明确记录 CSRNG 熵源、性能基准、NIST 测试、SM4 模式范围

## 十九、内存预算声明（§5.6）

- [x] **C149 内存预算声明**：在设计文档中声明内存占用（总计 ≤ 2 KB，不含堆）
- [x] **C150 OOM 策略**：密码学库不触发 OOM；调用方需确保堆 ≥ 4 KB 用于 SM2 加密输出

## 二十、后续版本解锁

- [x] **C151 解锁 v0.32.0**：PKI 证书基础（SM2/SM3/SM4 已就绪）
- [x] **C152 解锁 v0.39.0**：能力 Token（SM2 签名已就绪）
- [x] **C153 解锁 v0.78.0**：消息签名（SM2 签名已就绪）
- [x] **C154 解锁 v0.113.0**：Secure Boot（SM2/SM3/SM4 已就绪）
- [x] **C155 解锁 Phase 2 v0.115.0**：mTLS 通信安全（SM2/SM3/SM4 + CSRNG 已就绪）
- [x] **C156 解锁 v0.169.0**：Agent DID（SM2 密钥对已就绪）
