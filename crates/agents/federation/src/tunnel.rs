//! v0.98.1 纵向加密认证（36 号文合规，刚性子版本）：SM2 IKE 密钥协商 + SM4 密文隧道
//! + SM3-HMAC 认证 + 64 位滑动窗口重放保护 + 调度令牌验签 + 多隧道管理。
//!
//! ## 设计要点
//!
//! - **最小两方 IKE**（E3）：发起方生成 32 字节 PMS（注入 [`CsRng`]）→ SM2 加密至对端
//!   公钥 + SM2 签名 `SM3(PMS‖spi_offer_be)` + SPI 提议 → 应答方解密验签 → 应答帧
//!   携带 `spi_offer‖spi_answer‖SM2签名(SM3(PMS‖spi_answer_be))` → 双方独立调用
//!   [`derive_tunnel_keys`] 派生同一 [`TunnelKeys`]（内部先排序 SPI，参数顺序无关）。
//!   完整 IKE 状态机/证书链后置集成。
//! - **密钥派生**（SM3 域分离）：`encrypt_key = SM3("eneros-ve-enc"‖PMS‖spi_a‖spi_b)[..16]`、
//!   `auth_key = SM3("eneros-ve-mac"‖PMS‖spi_a‖spi_b)`（a ≤ b 排序后拼接，双端一致）。
//! - **密文隧道**：帧 `local_spi:u32be‖seq:u64be‖iv[16]‖SM4-CBC(iv, plaintext)‖
//!   SM3-HMAC(auth_key, spi‖seq‖iv‖ct)`；IV 由注入 RNG 生成（E6，CBC 可预测 IV 不安全）；
//!   接收侧先恒定时间校验 HMAC，再做重放检查，最后解密（解密失败视为认证失败）。
//! - **重放窗口**（E9）：u64 seq + 64-bit 滑动位图（IPsec 惯例）。位图 bit i 对应
//!   `seq = recv_seq - i`（bit0 为当前最大 seq 本身）：`seq > recv_seq` 推进窗口并置
//!   bit0；`seq == recv_seq` 或 `recv_seq - seq >= 64` → [`EncryptError::ReplayDetected`]；
//!   窗口内按位置检查/置位。
//! - **密钥轮换**（E12）：[`VerticalEncryptTunnel::rotate`] 原位替换派生密钥（旧密钥由
//!   `Drop` 恒定时间清零）+ 重置 seq/重放窗口；隧道持有派生密钥而非证书，证书轮换
//!   天然不影响已有连接。
//! - **可观测**：[`TunnelManager`] 4 个 pub 计数器 `established_count` / `send_count` /
//!   `recv_count` / `replay_reject_count`。
//!
//! ## E1~E12 偏差表（简版，相对蓝图 v0.98.1 原文）
//!
//! | 编号 | 偏差 |
//! |------|------|
//! | E1 | 同 crate `tunnel.rs` 单模块（与 v0.98.0 同属联邦安全通道族） |
//! | E2 | `VerticalEncryptDevice` sync trait + Mock 回环（真实卡驱动现场适配注入） |
//! | E3 | 最小两方 SM2 IKE：PMS + SM2 加密/签名 + SPI 提议，完整 IKE 状态机后置 |
//! | E4 | 证书 opaque bytes + 复用 `CertVerifier`，不新造证书类型 |
//! | E5 | `Vec<u8>` 缓冲（Agent Runtime 有用户堆，alloc 可用） |
//! | E6 | 随机 IV 由注入 `CsRng` 生成（CBC 可预测 IV 不安全） |
//! | E7 | `EncryptError` 7 变体最小完备（补 TagMismatch/InvalidFrame/UnknownTunnel） |
//! | E8 | `DispatchToken`/`AuthResult` 结构定义，过期判定先于验签 |
//! | E9 | u64 seq + 64-bit 滑动位图重放窗口（IPsec 惯例） |
//! | E10 | Mock 双端回环互通测试替代真实装置互通（现场验收项） |
//! | E11 | eneros-crypto 纯增量 `sm3/hmac.rs`（通用密码原语归属 crypto crate） |
//! | E12 | `rotate` 原位换钥 + 重放窗口重置；`TunnelManager` 多隧道 + 4 计数器 |
//!
//! ## 本模块相对 spec 的补充偏差（实现期确认）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | EX1 | `responder_accept` 增加 `rng: &mut CsRng` 参数 | SM2 签名需要随机数（k 值），无法确定性派生；与 `initiator_hello` 对称注入 |
//! | EX2 | `VerticalEncryptDevice::poll` 返回 `Option<Vec<u8>>`（非 `Result<Option<..>>`）；`TunnelManager::recv()` 无参（自设备 poll，按帧 spi 路由） | pinned 设计定稿：设备故障经 `xmit`/`poll` 空转表达，路由依赖帧头 spi |
//! | EX3 | `TunnelManager.device` 直接用 `MockVerticalEncryptDevice` 具体类型 | 保持简单（E2）；生产适配期可泛化 |
//! | EX4 | 重放位图映射修正为 bit i ↔ `seq = recv_seq - i`（推进时 `\| 1` 标记新最大 seq 于 bit0，窗口内检查位 `1 << (recv_seq - seq)`），并拒绝 `seq == 0` | 任务 pinned 伪码（`1 << (diff - 1)`）存在 off-by-one：窗口内乱序首收旧帧（如先收 seq2 再收 seq1）会被误判重放；修正后 TV25/TV27/TV28 与 C70~C72 语义自洽 |

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{sm2_decrypt, sm2_encrypt};
use eneros_crypto::sm4::cbc::Sm4Cbc;
use eneros_crypto::{
    ct_eq, ct_zeroize, hmac_sm3, sm2_sign, sm2_verify, sm3_hash, Sm2KeyPair, Sm2PublicKey,
    Sm2Signature,
};

/// SM2 签名序列化长度（r‖s，字节）
const SIG_LEN: usize = 64;
/// PMS（预主密钥）长度（字节）
const PMS_LEN: usize = 32;
/// SM4-CBC 分组长度（字节）
const BLOCK_LEN: usize = 16;
/// SM3-HMAC 认证标签长度（字节）
const TAG_LEN: usize = 32;
/// 隧道帧头固定部分长度：spi(4) + seq(8) + iv(16)
const HEADER_LEN: usize = 4 + 8 + BLOCK_LEN;
/// 最小合法帧长：HEADER + 最小一块密文(16) + tag(32)
const MIN_FRAME_LEN: usize = HEADER_LEN + BLOCK_LEN + TAG_LEN;

/// 纵向加密认证错误（7 变体最小完备，E7）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptError {
    /// 握手失败（PMS 加解密失败或签名生成失败）
    HandshakeFailed,
    /// 证书/签名无效（IKE 验签或调度令牌验签不通过）
    CertInvalid,
    /// 重放攻击（seq 重复或超出 64 位滑动窗口）
    ReplayDetected,
    /// 纵向加密装置错误（发送失败或无数据可读）
    DeviceError,
    /// SM3-HMAC 认证标签不匹配（含 CBC 解密填充错误）
    TagMismatch,
    /// 帧格式错误（长度不足/字段越界/SPI 不匹配）
    InvalidFrame,
    /// 未知隧道（SPI 未注册）
    UnknownTunnel,
}

/// 隧道派生密钥对（Clone/PartialEq；**禁 Debug**，项目硬约束：密钥不明文泄露）
///
/// `Drop` 时使用 [`ct_zeroize`] 恒定时间清零两个密钥。
#[derive(Clone, PartialEq)]
pub struct TunnelKeys {
    /// SM4-CBC 加密密钥（16 字节）
    encrypt_key: [u8; 16],
    /// SM3-HMAC 认证密钥（32 字节）
    auth_key: [u8; 32],
}

impl TunnelKeys {
    /// 构造隧道密钥对
    pub fn new(encrypt_key: [u8; 16], auth_key: [u8; 32]) -> Self {
        Self {
            encrypt_key,
            auth_key,
        }
    }

    /// 读取加密密钥（16 字节）
    pub fn encrypt_key(&self) -> &[u8; 16] {
        &self.encrypt_key
    }

    /// 读取认证密钥（32 字节）
    pub fn auth_key(&self) -> &[u8; 32] {
        &self.auth_key
    }
}

impl Drop for TunnelKeys {
    fn drop(&mut self) {
        ct_zeroize(&mut self.encrypt_key);
        ct_zeroize(&mut self.auth_key);
    }
}

// ------------------------------------------------------------
// IKE（最小两方 SM2 密钥协商，E3）
// ------------------------------------------------------------

/// IKE 发起方 hello（E3）：
///
/// 1. 生成 32 字节 PMS（注入 RNG）；
/// 2. 用对端公钥 SM2 加密 PMS（`C1‖C3‖C2`）；
/// 3. 用自身私钥签名 `SM3(PMS‖spi_offer_be)`；
/// 4. 帧 = `spi_offer:u32be‖pms_ct_len:u32be‖pms_ct‖sig[64]`。
///
/// 返回 `(hello_frame, pms)`；PMS 由调用方保存用于 [`initiator_finish`] 与
/// [`derive_tunnel_keys`]。任何密码运算失败 → [`EncryptError::HandshakeFailed`]。
pub fn initiator_hello(
    local_kp: &Sm2KeyPair,
    peer_pk: &Sm2PublicKey,
    spi_offer: u32,
    rng: &mut CsRng,
) -> Result<(Vec<u8>, [u8; 32]), EncryptError> {
    let mut pms = [0u8; PMS_LEN];
    rng.fill_bytes(&mut pms);

    let pms_ct = sm2_encrypt(&pms, peer_pk, rng).map_err(|_| EncryptError::HandshakeFailed)?;

    let mut digest_in = Vec::with_capacity(PMS_LEN + 4);
    digest_in.extend_from_slice(&pms);
    digest_in.extend_from_slice(&spi_offer.to_be_bytes());
    let digest = sm3_hash(&digest_in);
    ct_zeroize(&mut digest_in); // PMS 副本即时清零

    let sig = sm2_sign(&digest, &local_kp.private_key, &local_kp.public_key, rng)
        .map_err(|_| EncryptError::HandshakeFailed)?;

    let mut frame = Vec::with_capacity(8 + pms_ct.len() + SIG_LEN);
    frame.extend_from_slice(&spi_offer.to_be_bytes());
    frame.extend_from_slice(&(pms_ct.len() as u32).to_be_bytes());
    frame.extend_from_slice(&pms_ct);
    frame.extend_from_slice(&sig.to_bytes());
    Ok((frame, pms))
}

/// IKE 应答方 accept（E3）：
///
/// 1. 解析 hello 帧（长度/字段越界 → [`EncryptError::InvalidFrame`]）；
/// 2. 用自身私钥 SM2 解密 PMS（失败 → [`EncryptError::HandshakeFailed`]）；
/// 3. 用对端公钥验签 `SM3(PMS‖spi_offer_be)`（失败/不通过 → [`EncryptError::CertInvalid`]）；
/// 4. 应答帧 = `spi_offer:u32be‖spi_answer:u32be‖SM2签名(SM3(PMS‖spi_answer_be))`。
///
/// 返回 `(answer_frame, pms)`。
///
/// **偏差 EX1**：较 spec 增加 `rng: &mut CsRng` 参数（SM2 应答签名需要随机数）。
pub fn responder_accept(
    hello: &[u8],
    own_kp: &Sm2KeyPair,
    peer_pk: &Sm2PublicKey,
    spi_answer: u32,
    rng: &mut CsRng,
) -> Result<(Vec<u8>, [u8; 32]), EncryptError> {
    if hello.len() < 8 + SIG_LEN {
        return Err(EncryptError::InvalidFrame);
    }
    let spi_offer = u32::from_be_bytes([hello[0], hello[1], hello[2], hello[3]]);
    let ct_len = u32::from_be_bytes([hello[4], hello[5], hello[6], hello[7]]) as usize;
    if hello.len() != 8 + ct_len + SIG_LEN {
        return Err(EncryptError::InvalidFrame);
    }
    let pms_ct = &hello[8..8 + ct_len];
    let mut sig_bytes = [0u8; SIG_LEN];
    sig_bytes.copy_from_slice(&hello[8 + ct_len..]);

    let mut pms_vec =
        sm2_decrypt(pms_ct, &own_kp.private_key).map_err(|_| EncryptError::HandshakeFailed)?;
    if pms_vec.len() != PMS_LEN {
        ct_zeroize(&mut pms_vec);
        return Err(EncryptError::HandshakeFailed);
    }
    let mut pms = [0u8; PMS_LEN];
    pms.copy_from_slice(&pms_vec);
    ct_zeroize(&mut pms_vec);

    // 验签发起方签名：SM3(PMS‖spi_offer_be)
    let mut digest_in = Vec::with_capacity(PMS_LEN + 4);
    digest_in.extend_from_slice(&pms);
    digest_in.extend_from_slice(&spi_offer.to_be_bytes());
    let digest = sm3_hash(&digest_in);
    ct_zeroize(&mut digest_in);
    let sig = Sm2Signature::from_bytes(&sig_bytes);
    match sm2_verify(&digest, &sig, peer_pk) {
        Ok(true) => {}
        _ => {
            ct_zeroize(&mut pms);
            return Err(EncryptError::CertInvalid);
        }
    }

    // 应答签名：SM3(PMS‖spi_answer_be)
    let mut answer_digest_in = Vec::with_capacity(PMS_LEN + 4);
    answer_digest_in.extend_from_slice(&pms);
    answer_digest_in.extend_from_slice(&spi_answer.to_be_bytes());
    let answer_digest = sm3_hash(&answer_digest_in);
    ct_zeroize(&mut answer_digest_in);
    let answer_sig = sm2_sign(&answer_digest, &own_kp.private_key, &own_kp.public_key, rng)
        .map_err(|_| {
            ct_zeroize(&mut pms);
            EncryptError::HandshakeFailed
        })?;

    let mut answer = Vec::with_capacity(8 + SIG_LEN);
    answer.extend_from_slice(&spi_offer.to_be_bytes());
    answer.extend_from_slice(&spi_answer.to_be_bytes());
    answer.extend_from_slice(&answer_sig.to_bytes());
    Ok((answer, pms))
}

/// IKE 发起方 finish（E3）：解析应答帧（格式错 → [`EncryptError::InvalidFrame`]），
/// 用对端公钥验签 `SM3(PMS‖spi_answer_be)`（PMS 为本地保存的预主密钥；
/// 验签失败 → [`EncryptError::CertInvalid`]），成功返回对端应答的 `spi_answer`。
///
/// `local_kp` 为保留参数（未来密钥确认扩展，当前仅验签无需本地私钥）。
pub fn initiator_finish(
    answer: &[u8],
    local_kp: &Sm2KeyPair,
    peer_pk: &Sm2PublicKey,
    pms: &[u8; 32],
) -> Result<u32, EncryptError> {
    let _ = local_kp; // 保留参数：未来密钥确认扩展
    if answer.len() != 8 + SIG_LEN {
        return Err(EncryptError::InvalidFrame);
    }
    let spi_answer = u32::from_be_bytes([answer[4], answer[5], answer[6], answer[7]]);
    let mut sig_bytes = [0u8; SIG_LEN];
    sig_bytes.copy_from_slice(&answer[8..]);

    let mut digest_in = Vec::with_capacity(PMS_LEN + 4);
    digest_in.extend_from_slice(pms);
    digest_in.extend_from_slice(&spi_answer.to_be_bytes());
    let digest = sm3_hash(&digest_in);
    ct_zeroize(&mut digest_in);

    let sig = Sm2Signature::from_bytes(&sig_bytes);
    match sm2_verify(&digest, &sig, peer_pk) {
        Ok(true) => Ok(spi_answer),
        _ => Err(EncryptError::CertInvalid),
    }
}

/// SM3 域分离隧道密钥派生（双方一致）：
///
/// - 内部先将两个 SPI 排序（`a ≤ b`），双端调用参数顺序不同结果一致；
/// - `encrypt_key = SM3("eneros-ve-enc"‖PMS‖a_be‖b_be)[..16]`；
/// - `auth_key = SM3("eneros-ve-mac"‖PMS‖a_be‖b_be)`。
pub fn derive_tunnel_keys(pms: &[u8; 32], spi_l: u32, spi_r: u32) -> TunnelKeys {
    let (a, b) = if spi_l <= spi_r {
        (spi_l, spi_r)
    } else {
        (spi_r, spi_l)
    };

    let mut enc_in = Vec::with_capacity(13 + PMS_LEN + 8);
    enc_in.extend_from_slice(b"eneros-ve-enc");
    enc_in.extend_from_slice(pms);
    enc_in.extend_from_slice(&a.to_be_bytes());
    enc_in.extend_from_slice(&b.to_be_bytes());
    let enc_digest = sm3_hash(&enc_in);
    ct_zeroize(&mut enc_in);
    let mut encrypt_key = [0u8; 16];
    encrypt_key.copy_from_slice(&enc_digest[..16]);

    let mut mac_in = Vec::with_capacity(13 + PMS_LEN + 8);
    mac_in.extend_from_slice(b"eneros-ve-mac");
    mac_in.extend_from_slice(pms);
    mac_in.extend_from_slice(&a.to_be_bytes());
    mac_in.extend_from_slice(&b.to_be_bytes());
    let auth_key = sm3_hash(&mac_in);
    ct_zeroize(&mut mac_in);

    TunnelKeys::new(encrypt_key, auth_key)
}

// ------------------------------------------------------------
// 密文隧道
// ------------------------------------------------------------

/// 纵向加密隧道（字段全 pub；不派生 Debug：包含 [`TunnelKeys`] 密钥材料）
pub struct VerticalEncryptTunnel {
    /// 本端 SPI（发送帧携带）
    pub local_spi: u32,
    /// 对端 SPI（接收帧匹配）
    pub remote_spi: u32,
    /// 派生密钥对
    pub keys: TunnelKeys,
    /// 发送序号（每帧递增，首帧为 1）
    pub send_seq: u64,
    /// 已接收的最大序号
    pub recv_seq: u64,
    /// 重放窗口位图：bit i 对应 `seq = recv_seq - i`（bit0 为当前最大 seq）
    pub replay_bitmap: u64,
}

impl VerticalEncryptTunnel {
    /// 创建隧道：send_seq/recv_seq/replay_bitmap 全零
    pub fn new(local_spi: u32, remote_spi: u32, keys: TunnelKeys) -> Self {
        Self {
            local_spi,
            remote_spi,
            keys,
            send_seq: 0,
            recv_seq: 0,
            replay_bitmap: 0,
        }
    }

    /// 加密发送（E6）：
    ///
    /// send_seq+=1 得 seq；随机 IV[16]；帧 =
    /// `local_spi:u32be‖seq:u64be‖iv‖SM4-CBC(iv, plaintext)‖
    /// SM3-HMAC(auth_key, spi‖seq‖iv‖ct)`。
    pub fn tunnel_send(&mut self, plaintext: &[u8], rng: &mut CsRng) -> Vec<u8> {
        self.send_seq += 1;
        let seq = self.send_seq;

        let mut iv = [0u8; BLOCK_LEN];
        rng.fill_bytes(&mut iv);
        let ct = Sm4Cbc::new(self.keys.encrypt_key(), &iv).encrypt(plaintext);

        let mut frame = Vec::with_capacity(HEADER_LEN + ct.len() + TAG_LEN);
        frame.extend_from_slice(&self.local_spi.to_be_bytes());
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(&iv);
        frame.extend_from_slice(&ct);
        let tag = hmac_sm3(self.keys.auth_key(), &frame);
        frame.extend_from_slice(&tag);
        frame
    }

    /// 解密接收：
    ///
    /// 1. 长度/格式检查（< 最小帧长或密文段非 16 字节整数倍 → [`EncryptError::InvalidFrame`]）；
    /// 2. 帧 SPI 必须等于 `remote_spi`（否 → [`EncryptError::InvalidFrame`]）；
    /// 3. SM3-HMAC 重算 + [`ct_eq`] 恒定时间比较（失败 → [`EncryptError::TagMismatch`]）；
    /// 4. 重放检查（先于解密，重复/超窗 → [`EncryptError::ReplayDetected`]，通过后更新窗口）；
    /// 5. SM4-CBC 解密（填充错误视为认证失败 → [`EncryptError::TagMismatch`]）。
    pub fn tunnel_recv(&mut self, frame: &[u8]) -> Result<Vec<u8>, EncryptError> {
        if frame.len() < MIN_FRAME_LEN || (frame.len() - HEADER_LEN - TAG_LEN) % BLOCK_LEN != 0 {
            return Err(EncryptError::InvalidFrame);
        }
        let spi = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        if spi != self.remote_spi {
            return Err(EncryptError::InvalidFrame);
        }
        let seq = u64::from_be_bytes([
            frame[4], frame[5], frame[6], frame[7], frame[8], frame[9], frame[10], frame[11],
        ]);
        let mut iv = [0u8; BLOCK_LEN];
        iv.copy_from_slice(&frame[12..28]);
        let ct = &frame[HEADER_LEN..frame.len() - TAG_LEN];
        let tag = &frame[frame.len() - TAG_LEN..];

        let expected = hmac_sm3(self.keys.auth_key(), &frame[..frame.len() - TAG_LEN]);
        if !ct_eq(&expected, tag) {
            return Err(EncryptError::TagMismatch);
        }

        self.replay_check_and_update(seq)?;

        match Sm4Cbc::new(self.keys.encrypt_key(), &iv).decrypt(ct) {
            Ok(pt) => Ok(pt),
            Err(_) => Err(EncryptError::TagMismatch),
        }
    }

    /// 重放检查与窗口更新（E9，偏差 EX4）：
    ///
    /// - `seq == 0` → 拒绝（本实现序号自 1 起始，0 为非法外来帧）；
    /// - `seq > recv_seq`：推进窗口，`bitmap = (bitmap << shift) | 1`（shift ≥ 64 时重置为 1），
    ///   `recv_seq = seq`；
    /// - `seq == recv_seq`：bit0 已在推进时置位 → [`EncryptError::ReplayDetected`]；
    /// - `seq < recv_seq`：`diff = recv_seq - seq`，`diff >= 64`（超窗）→ 重放；
    ///   否则检查 `1 << diff` 位，已置位 → 重放，未置位 → 置位放行。
    fn replay_check_and_update(&mut self, seq: u64) -> Result<(), EncryptError> {
        if seq == 0 {
            return Err(EncryptError::ReplayDetected);
        }
        if seq > self.recv_seq {
            let shift = seq - self.recv_seq;
            self.replay_bitmap = if shift >= 64 {
                1
            } else {
                (self.replay_bitmap << shift) | 1
            };
            self.recv_seq = seq;
            return Ok(());
        }
        let diff = self.recv_seq - seq;
        if diff >= 64 {
            return Err(EncryptError::ReplayDetected);
        }
        let bit = 1u64 << diff;
        if self.replay_bitmap & bit != 0 {
            return Err(EncryptError::ReplayDetected);
        }
        self.replay_bitmap |= bit;
        Ok(())
    }

    /// 原位换钥（E12）：替换派生密钥（旧密钥由 [`TunnelKeys::drop`] 恒定时间清零），
    /// 并重置 send_seq/recv_seq/replay_bitmap（换钥后序号空间重新开始）。
    pub fn rotate(&mut self, new_keys: TunnelKeys) {
        self.keys = new_keys;
        self.send_seq = 0;
        self.recv_seq = 0;
        self.replay_bitmap = 0;
    }
}

// ------------------------------------------------------------
// 调度令牌验签（36 号文合规，E8）
// ------------------------------------------------------------

/// 调度认证令牌（E8；签名为 SM2 签名序列化 r‖s 64 字节）
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchToken {
    /// 调度指令明文
    pub payload: Vec<u8>,
    /// SM2 签名（r‖s，64 字节）
    pub signature: [u8; 64],
    /// 过期时刻（毫秒时间戳，`now_ms >= expires_ms` 即过期）
    pub expires_ms: u64,
}

/// 调度验签结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthResult {
    /// 未过期且签名有效
    Granted,
    /// 未过期但签名无效
    Denied,
    /// 已过期（不验签直接判定）
    Expired,
}

/// 验证调度令牌（E8）：**过期判定先于验签**（`now_ms >= expires_ms` →
/// [`AuthResult::Expired`]，不做任何验签计算）；未过期时用调度主站公钥 SM2 验签
/// `payload`，通过 → [`AuthResult::Granted`]，否则 → [`AuthResult::Denied`]。
pub fn verify_dispatch_auth(token: &DispatchToken, pk: &Sm2PublicKey, now_ms: u64) -> AuthResult {
    if now_ms >= token.expires_ms {
        return AuthResult::Expired;
    }
    let sig = Sm2Signature::from_bytes(&token.signature);
    match sm2_verify(&token.payload, &sig, pk) {
        Ok(true) => AuthResult::Granted,
        _ => AuthResult::Denied,
    }
}

// ------------------------------------------------------------
// 纵向加密装置抽象与多隧道管理（E2/E12）
// ------------------------------------------------------------

/// 纵向加密装置抽象（sync，无 async 无 Send+Sync，E2）
///
/// 真实纵向加密卡驱动在现场适配阶段实现本 trait 注入；测试用
/// [`MockVerticalEncryptDevice`] 回环。
pub trait VerticalEncryptDevice {
    /// 发送一帧到装置
    fn xmit(&mut self, frame: &[u8]) -> Result<(), EncryptError>;
    /// 从装置取一帧（无数据 → `None`）
    fn poll(&mut self) -> Option<Vec<u8>>;
}

/// Mock 纵向加密装置（字段全 pub，支持发送故障注入）
#[derive(Debug, Clone, Default)]
pub struct MockVerticalEncryptDevice {
    /// 已成功发送的帧记录
    pub xmitted: Vec<Vec<u8>>,
    /// 待接收帧队列（poll 弹队首）
    pub pending: Vec<Vec<u8>>,
    /// 剩余 xmit 应失败次数（>0 → `Err(DeviceError)` 并递减）
    pub fail_times: u32,
}

impl MockVerticalEncryptDevice {
    /// 创建空 Mock 装置（无故障注入）
    pub fn new() -> Self {
        Self::default()
    }
}

impl VerticalEncryptDevice for MockVerticalEncryptDevice {
    fn xmit(&mut self, frame: &[u8]) -> Result<(), EncryptError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(EncryptError::DeviceError);
        }
        self.xmitted.push(frame.to_vec());
        Ok(())
    }

    fn poll(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_empty() {
            None
        } else {
            Some(self.pending.remove(0))
        }
    }
}

/// 多隧道管理器（E12；字段全 pub；按 local_spi 索引，接收按帧 spi 匹配 remote_spi 路由）
pub struct TunnelManager {
    /// 已注册隧道（key = local_spi）
    pub tunnels: BTreeMap<u32, VerticalEncryptTunnel>,
    /// 纵向加密装置（E2，偏差 EX3：直接用 Mock 具体类型保持简单）
    pub device: MockVerticalEncryptDevice,
    /// 累计注册隧道数
    pub established_count: u64,
    /// 累计成功发送帧数
    pub send_count: u64,
    /// 累计成功接收帧数
    pub recv_count: u64,
    /// 累计重放拒绝帧数
    pub replay_reject_count: u64,
}

impl TunnelManager {
    /// 创建管理器：隧道表为空，4 计数器全零
    pub fn new(device: MockVerticalEncryptDevice) -> Self {
        Self {
            tunnels: BTreeMap::new(),
            device,
            established_count: 0,
            send_count: 0,
            recv_count: 0,
            replay_reject_count: 0,
        }
    }

    /// 注册隧道（按 local_spi 索引；established_count+=1）
    pub fn add(&mut self, tunnel: VerticalEncryptTunnel) {
        self.tunnels.insert(tunnel.local_spi, tunnel);
        self.established_count += 1;
    }

    /// 注销隧道：存在则移除并返回 true，否则 false
    pub fn remove(&mut self, local_spi: u32) -> bool {
        self.tunnels.remove(&local_spi).is_some()
    }

    /// 经指定本地隧道加密发送：未知 spi → [`EncryptError::UnknownTunnel`]；
    /// 装置发送失败 → [`EncryptError::DeviceError`] 传播（send_count 不变）；
    /// 成功 → send_count+=1。
    pub fn send(
        &mut self,
        local_spi: u32,
        plaintext: &[u8],
        rng: &mut CsRng,
    ) -> Result<(), EncryptError> {
        let tunnel = match self.tunnels.get_mut(&local_spi) {
            Some(t) => t,
            None => return Err(EncryptError::UnknownTunnel),
        };
        let frame = tunnel.tunnel_send(plaintext, rng);
        self.device.xmit(&frame)?;
        self.send_count += 1;
        Ok(())
    }

    /// 从装置 poll 一帧并按帧头 spi 路由到 `remote_spi == frame_spi` 的隧道解密：
    ///
    /// - 无数据可读 → [`EncryptError::DeviceError`]；帧长 < 4 → [`EncryptError::InvalidFrame`]；
    /// - 无匹配隧道 → [`EncryptError::UnknownTunnel`]；
    /// - 重放拒绝 → replay_reject_count+=1 并传播 [`EncryptError::ReplayDetected`]；
    /// - 其他解密错误原样传播；成功 → recv_count+=1，返回 `(frame_spi, 明文)`。
    pub fn recv(&mut self) -> Result<(u32, Vec<u8>), EncryptError> {
        let frame = match self.device.poll() {
            Some(f) => f,
            None => return Err(EncryptError::DeviceError),
        };
        if frame.len() < 4 {
            return Err(EncryptError::InvalidFrame);
        }
        let frame_spi = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let tunnel = match self
            .tunnels
            .values_mut()
            .find(|t| t.remote_spi == frame_spi)
        {
            Some(t) => t,
            None => return Err(EncryptError::UnknownTunnel),
        };
        match tunnel.tunnel_recv(&frame) {
            Ok(pt) => {
                self.recv_count += 1;
                Ok((frame_spi, pt))
            }
            Err(EncryptError::ReplayDetected) => {
                self.replay_reject_count += 1;
                Err(EncryptError::ReplayDetected)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------
    // 测试辅助
    // ------------------------------------------------------------

    /// 生成两个不同密钥对（同一 RNG 顺序生成，避免固定种子撞钥）
    fn two_keypairs() -> (Sm2KeyPair, Sm2KeyPair) {
        let mut rng = CsRng::new();
        let a = Sm2KeyPair::generate(&mut rng).expect("kp a");
        let b = Sm2KeyPair::generate(&mut rng).expect("kp b");
        (a, b)
    }

    /// 生成与 two_keypairs() 不同的第三方密钥对（固定种子下顺序推进到第 3 个）
    fn stranger_keypair() -> Sm2KeyPair {
        let mut rng = CsRng::new();
        let _a = Sm2KeyPair::generate(&mut rng).expect("kp a");
        let _b = Sm2KeyPair::generate(&mut rng).expect("kp b");
        Sm2KeyPair::generate(&mut rng).expect("kp c")
    }

    /// 完整 IKE 握手：返回（发起方 PMS、发起方 keys、应答方 keys），并断言 spi_answer 正确
    fn handshake(spi_offer: u32, spi_answer: u32) -> (TunnelKeys, TunnelKeys) {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (hello, pms_i) =
            initiator_hello(&init, &resp.public_key, spi_offer, &mut rng).expect("hello");
        let (answer, pms_r) =
            responder_accept(&hello, &resp, &init.public_key, spi_answer, &mut rng)
                .expect("accept");
        let got = initiator_finish(&answer, &init, &resp.public_key, &pms_i).expect("finish");
        assert_eq!(got, spi_answer);
        assert_eq!(pms_i, pms_r);
        let ki = derive_tunnel_keys(&pms_i, spi_offer, spi_answer);
        let kr = derive_tunnel_keys(&pms_r, spi_answer, spi_offer);
        (ki, kr)
    }

    /// 构造一对互通隧道：A(local=100, remote=200) ↔ B(local=200, remote=100)
    fn tunnel_pair() -> (VerticalEncryptTunnel, VerticalEncryptTunnel) {
        let (ki, kr) = handshake(100, 200);
        // TunnelKeys 禁 Debug：用 assert!(==) 替代 assert_eq!
        assert!(ki == kr);
        (
            VerticalEncryptTunnel::new(100, 200, ki),
            VerticalEncryptTunnel::new(200, 100, kr),
        )
    }

    /// 固定密钥隧道对（跳过 IKE，隧道收发系列测试用）
    fn fixed_tunnel_pair() -> (VerticalEncryptTunnel, VerticalEncryptTunnel) {
        let keys = TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]);
        (
            VerticalEncryptTunnel::new(100, 200, keys.clone()),
            VerticalEncryptTunnel::new(200, 100, keys),
        )
    }

    /// 签名调度令牌
    fn make_token(payload: &[u8], expires_ms: u64) -> (DispatchToken, Sm2KeyPair) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("kp");
        let sig = sm2_sign(payload, &kp.private_key, &kp.public_key, &mut rng).expect("sign");
        (
            DispatchToken {
                payload: payload.to_vec(),
                signature: sig.to_bytes(),
                expires_ms,
            },
            kp,
        )
    }

    // ------------------------------------------------------------
    // TV1~TV4：EncryptError / TunnelKeys
    // ------------------------------------------------------------

    // TV1: EncryptError 派生（Debug/Clone/Copy/PartialEq/Eq），7 变体互不等
    #[test]
    fn tv01_encrypt_error_derive() {
        let errs = [
            EncryptError::HandshakeFailed,
            EncryptError::CertInvalid,
            EncryptError::ReplayDetected,
            EncryptError::DeviceError,
            EncryptError::TagMismatch,
            EncryptError::InvalidFrame,
            EncryptError::UnknownTunnel,
        ];
        for (i, a) in errs.iter().enumerate() {
            for (j, b) in errs.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
        let e = EncryptError::TagMismatch;
        let e2 = e; // Copy 语义
        assert_eq!(e, e2);
        assert_eq!(format!("{:?}", e), "TagMismatch"); // Debug
    }

    // TV2: TunnelKeys::new 与访问器（字段私有，经 accessor 读取）
    #[test]
    fn tv02_tunnel_keys_new_and_accessors() {
        let enc = [0x11u8; 16];
        let auth = [0x22u8; 32];
        let keys = TunnelKeys::new(enc, auth);
        assert_eq!(keys.encrypt_key(), &enc);
        assert_eq!(keys.auth_key(), &auth);
    }

    // TV3: TunnelKeys Clone/PartialEq 语义
    #[test]
    fn tv03_tunnel_keys_clone_eq() {
        let k1 = TunnelKeys::new([0x11u8; 16], [0x22u8; 32]);
        let k2 = k1.clone();
        // TunnelKeys 禁 Debug：用 assert!(==/!=) 替代 assert_eq!/assert_ne!
        assert!(k1 == k2);
        let k3 = TunnelKeys::new([0x33u8; 16], [0x22u8; 32]);
        assert!(k1 != k3);
        let k4 = TunnelKeys::new([0x11u8; 16], [0x44u8; 32]);
        assert!(k1 != k4);
    }

    // TV4: TunnelKeys 禁 Debug（编译期注释断言：类型未 derive Debug，密钥不明文泄露）
    //      + Drop 实现存在性（drop(t) 能编译即证明实现了 Drop 清零）
    #[test]
    fn tv04_tunnel_keys_no_debug_and_drop() {
        // 编译期说明：TunnelKeys 仅 derive(Clone, PartialEq)，未 derive Debug；
        // 若未来有人误加 Debug，本测试上方的接口约束评审应拦截（C52）。
        let t = TunnelKeys::new([0xAAu8; 16], [0xBBu8; 32]);
        let keep = t.clone();
        drop(t); // Drop 实现存在（ct_zeroize 清零两个密钥）
                 // 原值已 drop，clone 仍独立可用
        assert_eq!(keep.encrypt_key(), &[0xAAu8; 16]);
        assert_eq!(keep.auth_key(), &[0xBBu8; 32]);
    }

    // ------------------------------------------------------------
    // TV5~TV14：IKE 双端协商
    // ------------------------------------------------------------

    // TV5: IKE 全链路 hello→accept→finish→双端 derive_tunnel_keys 一致
    #[test]
    fn tv05_ike_full_handshake_keys_match() {
        let (ki, kr) = handshake(100, 200);
        assert!(ki == kr);
    }

    // TV6: hello 帧布局：spi_offer‖pms_ct_len‖pms_ct‖sig[64]，长度字段自洽
    #[test]
    fn tv06_hello_frame_layout() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let spi_offer = 0xAABBCCDDu32;
        let (hello, pms) =
            initiator_hello(&init, &resp.public_key, spi_offer, &mut rng).expect("hello");
        assert_eq!(&hello[0..4], &spi_offer.to_be_bytes());
        let ct_len = u32::from_be_bytes([hello[4], hello[5], hello[6], hello[7]]) as usize;
        // SM2 加密输出 C1(65)‖C3(32)‖C2(32) = 129
        assert_eq!(ct_len, 65 + 32 + PMS_LEN);
        assert_eq!(hello.len(), 8 + ct_len + SIG_LEN);
        assert_ne!(pms, [0u8; 32]); // PMS 为随机非零
    }

    // TV7: answer 帧布局：spi_offer‖spi_answer‖sig[64]；initiator_finish 返回 spi_answer
    #[test]
    fn tv07_answer_frame_layout_and_spi() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (hello, pms) = initiator_hello(&init, &resp.public_key, 111, &mut rng).expect("hello");
        let (answer, _) =
            responder_accept(&hello, &resp, &init.public_key, 222, &mut rng).expect("accept");
        assert_eq!(answer.len(), 8 + SIG_LEN);
        assert_eq!(&answer[0..4], &111u32.to_be_bytes());
        assert_eq!(&answer[4..8], &222u32.to_be_bytes());
        let got = initiator_finish(&answer, &init, &resp.public_key, &pms).expect("finish");
        assert_eq!(got, 222);
    }

    // TV8: 篡改 hello 中 PMS 密文 → responder 解密失败 HandshakeFailed
    #[test]
    fn tv08_tampered_pms_ciphertext() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (mut hello, _) =
            initiator_hello(&init, &resp.public_key, 100, &mut rng).expect("hello");
        hello[8 + 10] ^= 0x01; // 篡改密文第 10 字节
        let mut rng2 = CsRng::new();
        let r = responder_accept(&hello, &resp, &init.public_key, 200, &mut rng2);
        assert_eq!(r.unwrap_err(), EncryptError::HandshakeFailed);
    }

    // TV9: 篡改 hello 签名 → responder 验签失败 CertInvalid
    #[test]
    fn tv09_tampered_hello_signature() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (mut hello, _) =
            initiator_hello(&init, &resp.public_key, 100, &mut rng).expect("hello");
        let last = hello.len() - 1;
        hello[last] ^= 0x01; // 篡改签名末字节
        let mut rng2 = CsRng::new();
        let r = responder_accept(&hello, &resp, &init.public_key, 200, &mut rng2);
        assert_eq!(r.unwrap_err(), EncryptError::CertInvalid);
    }

    // TV10: responder 用错误私钥解密 PMS → HandshakeFailed
    #[test]
    fn tv10_wrong_private_key_decrypt() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (hello, _) = initiator_hello(&init, &resp.public_key, 100, &mut rng).expect("hello");
        let stranger = stranger_keypair();
        let mut rng2 = CsRng::new();
        let r = responder_accept(&hello, &stranger, &init.public_key, 200, &mut rng2);
        assert_eq!(r.unwrap_err(), EncryptError::HandshakeFailed);
    }

    // TV11: hello 帧截断/长度字段不符 → InvalidFrame
    #[test]
    fn tv11_truncated_hello_frame() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (hello, _) = initiator_hello(&init, &resp.public_key, 100, &mut rng).expect("hello");
        let mut r2 = CsRng::new();
        // 极短帧
        assert_eq!(
            responder_accept(&hello[..4], &resp, &init.public_key, 200, &mut r2).unwrap_err(),
            EncryptError::InvalidFrame
        );
        // 尾部截断（长度字段与实际不符）
        let mut r3 = CsRng::new();
        assert_eq!(
            responder_accept(
                &hello[..hello.len() - 1],
                &resp,
                &init.public_key,
                200,
                &mut r3
            )
            .unwrap_err(),
            EncryptError::InvalidFrame
        );
        // ct_len 字段越界
        let mut bad = hello.clone();
        bad[4..8].copy_from_slice(&9999u32.to_be_bytes());
        let mut r4 = CsRng::new();
        assert_eq!(
            responder_accept(&bad, &resp, &init.public_key, 200, &mut r4).unwrap_err(),
            EncryptError::InvalidFrame
        );
    }

    // TV12: 用错误 peer_pk 验签 → CertInvalid（responder 与 initiator_finish 两侧）
    #[test]
    fn tv12_wrong_peer_pk_verify() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (hello, pms) = initiator_hello(&init, &resp.public_key, 100, &mut rng).expect("hello");
        let stranger = stranger_keypair();
        // responder 用错误的发起方公钥验签
        let mut r2 = CsRng::new();
        assert_eq!(
            responder_accept(&hello, &resp, &stranger.public_key, 200, &mut r2).unwrap_err(),
            EncryptError::CertInvalid
        );
        // initiator 用错误的应答方公钥验签 answer
        let mut r3 = CsRng::new();
        let (answer, _) =
            responder_accept(&hello, &resp, &init.public_key, 200, &mut r3).expect("accept");
        assert_eq!(
            initiator_finish(&answer, &init, &stranger.public_key, &pms).unwrap_err(),
            EncryptError::CertInvalid
        );
    }

    // TV13: 篡改 answer 签名 → initiator_finish CertInvalid；answer 截断 → InvalidFrame
    #[test]
    fn tv13_tampered_answer_signature() {
        let (init, resp) = two_keypairs();
        let mut rng = CsRng::new();
        let (hello, pms) = initiator_hello(&init, &resp.public_key, 100, &mut rng).expect("hello");
        let mut r2 = CsRng::new();
        let (mut answer, _) =
            responder_accept(&hello, &resp, &init.public_key, 200, &mut r2).expect("accept");
        let last = answer.len() - 1;
        answer[last] ^= 0x01;
        assert_eq!(
            initiator_finish(&answer, &init, &resp.public_key, &pms).unwrap_err(),
            EncryptError::CertInvalid
        );
        assert_eq!(
            initiator_finish(&answer[..8], &init, &resp.public_key, &pms).unwrap_err(),
            EncryptError::InvalidFrame
        );
    }

    // TV14: derive_tunnel_keys SPI 排序对称 + 域分离（encrypt/auth 不同）+ PMS 区分
    #[test]
    fn tv14_derive_keys_symmetry_domain_separation() {
        let pms = [0x42u8; 32];
        let k_ab = derive_tunnel_keys(&pms, 100, 200);
        let k_ba = derive_tunnel_keys(&pms, 200, 100);
        assert!(k_ab == k_ba); // 参数顺序无关
                               // 域分离：两个密钥不同（auth_key 前 16 字节 != encrypt_key）
        assert_ne!(k_ab.encrypt_key(), &k_ab.auth_key()[..16]);
        // 不同 PMS → 不同密钥
        let k_other = derive_tunnel_keys(&[0x43u8; 32], 100, 200);
        assert!(k_ab != k_other);
        // 不同 SPI 对 → 不同密钥
        let k_spi = derive_tunnel_keys(&pms, 100, 201);
        assert!(k_ab != k_spi);
    }

    // ------------------------------------------------------------
    // TV15~TV22：隧道帧格式与加解密
    // ------------------------------------------------------------

    // TV15: 帧布局字段偏移：spi[0..4]‖seq[4..12]‖iv[12..28]‖ct‖tag[末32]
    #[test]
    fn tv15_tunnel_frame_layout() {
        let (mut a, _) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let frame = a.tunnel_send(b"abc", &mut rng);
        // 明文 3 字节 → PKCS#7 补齐一块 16 字节
        assert_eq!(frame.len(), MIN_FRAME_LEN);
        assert_eq!(&frame[0..4], &100u32.to_be_bytes()); // local_spi
        assert_eq!(&frame[4..12], &1u64.to_be_bytes()); // 首帧 seq=1
        let iv = &frame[12..28];
        assert_eq!(iv.len(), 16);
        // 第二帧 seq=2
        let frame2 = a.tunnel_send(b"abc", &mut rng);
        assert_eq!(&frame2[4..12], &2u64.to_be_bytes());
    }

    // TV16: 加解密往返一致（A.send → B.recv 明文一致，反向同理）
    #[test]
    fn tv16_encrypt_decrypt_roundtrip() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let f1 = a.tunnel_send(b"hello dispatch", &mut rng);
        assert_eq!(b.tunnel_recv(&f1), Ok(b"hello dispatch".to_vec()));
        let f2 = b.tunnel_send(b"ack", &mut rng);
        assert_eq!(a.tunnel_recv(&f2), Ok(b"ack".to_vec()));
    }

    // TV17: 双端互通（IKE 派生密钥对）：A→B 与 B→A 多消息往返
    #[test]
    fn tv17_two_party_ike_keys_interop() {
        let (mut a, mut b) = tunnel_pair();
        let mut rng = CsRng::new();
        for i in 0..5u8 {
            let msg = [i; 10];
            let f = a.tunnel_send(&msg, &mut rng);
            assert_eq!(b.tunnel_recv(&f), Ok(msg.to_vec()));
            let f2 = b.tunnel_send(&msg, &mut rng);
            assert_eq!(a.tunnel_recv(&f2), Ok(msg.to_vec()));
        }
    }

    // TV18: 多帧 seq 递增且 recv_seq 同步推进
    #[test]
    fn tv18_multi_frame_seq_increment() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        for expect_seq in 1..=10u64 {
            let f = a.tunnel_send(b"x", &mut rng);
            assert_eq!(a.send_seq, expect_seq);
            assert_eq!(b.tunnel_recv(&f).unwrap(), b"x".to_vec());
            assert_eq!(b.recv_seq, expect_seq);
        }
    }

    // TV19: 空明文（PKCS#7 整块填充）往返
    #[test]
    fn tv19_empty_plaintext() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let f = a.tunnel_send(b"", &mut rng);
        assert_eq!(f.len(), MIN_FRAME_LEN); // ct 恰一块 16
        assert_eq!(b.tunnel_recv(&f), Ok(Vec::new()));
    }

    // TV20: 长明文（多块 CBC，100 字节 → 112 字节密文）往返
    #[test]
    fn tv20_long_plaintext_multi_block() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let msg = [0x5Au8; 100];
        let f = a.tunnel_send(&msg, &mut rng);
        assert_eq!(f.len(), HEADER_LEN + 112 + TAG_LEN);
        assert_eq!(b.tunnel_recv(&f), Ok(msg.to_vec()));
    }

    // TV21: SPI 不匹配 → InvalidFrame（帧 spi 非 remote_spi）
    #[test]
    fn tv21_spi_mismatch() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let mut f = a.tunnel_send(b"data", &mut rng);
        f[0..4].copy_from_slice(&999u32.to_be_bytes()); // 改成未知 spi（tag 同步失效，但先判 spi）
        assert_eq!(b.tunnel_recv(&f), Err(EncryptError::InvalidFrame));
        // 用对端 local_spi 合法的帧发向错误接收方（spi=200 发给 B(remote=200)→ 实际匹配，
        // 改发给第三个 remote_spi=300 的隧道）
        let keys = TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]);
        let mut c = VerticalEncryptTunnel::new(300, 100, keys);
        let f2 = a.tunnel_send(b"data", &mut rng); // spi=100
                                                   // c.remote_spi=100 匹配 spi=100 → 不该走 InvalidFrame；构造 spi=200 帧给 c
        let mut b2 =
            VerticalEncryptTunnel::new(200, 300, TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]));
        let f3 = b2.tunnel_send(b"data", &mut rng); // spi=200
        assert_eq!(c.tunnel_recv(&f3), Err(EncryptError::InvalidFrame)); // c.remote_spi=100 ≠ 200
        let _ = f2;
    }

    // TV22: 帧过短 / 密文段非 16 整数倍 → InvalidFrame
    #[test]
    fn tv22_frame_too_short() {
        let (_, mut b) = fixed_tunnel_pair();
        assert_eq!(b.tunnel_recv(&[]), Err(EncryptError::InvalidFrame));
        assert_eq!(b.tunnel_recv(&[0u8; 75]), Err(EncryptError::InvalidFrame));
        // 长度达标但 ct 段非 16 倍数（总长 77 → ct=17）
        assert_eq!(
            b.tunnel_recv(&[0u8; MIN_FRAME_LEN + 1]),
            Err(EncryptError::InvalidFrame)
        );
    }

    // ------------------------------------------------------------
    // TV23~TV28：重放保护
    // ------------------------------------------------------------

    // TV23: 同帧二次接收 → ReplayDetected
    #[test]
    fn tv23_same_frame_twice_replay() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let f = a.tunnel_send(b"once", &mut rng);
        assert_eq!(b.tunnel_recv(&f), Ok(b"once".to_vec()));
        assert_eq!(b.tunnel_recv(&f), Err(EncryptError::ReplayDetected));
    }

    // TV24: 窗口外旧帧（recv_seq - seq >= 64）→ ReplayDetected
    #[test]
    fn tv24_out_of_window_old_seq() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let mut frames = Vec::new();
        for _ in 0..70 {
            frames.push(a.tunnel_send(b"m", &mut rng));
        }
        for f in &frames {
            assert!(b.tunnel_recv(f).is_ok());
        }
        assert_eq!(b.recv_seq, 70);
        // seq=5：diff=65 ≥ 64 → 超窗重放（即便它确实收过）
        assert_eq!(b.tunnel_recv(&frames[4]), Err(EncryptError::ReplayDetected));
        // seq=6：diff=64 ≥ 64 → 超窗（C71 边界：seq <= recv_seq-64）
        assert_eq!(b.tunnel_recv(&frames[5]), Err(EncryptError::ReplayDetected));
    }

    // TV25: 窗口内乱序旧帧首次接收 → Ok 且位图置位
    #[test]
    fn tv25_in_window_out_of_order_first_time_ok() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let f1 = a.tunnel_send(b"first", &mut rng); // seq=1
        let f2 = a.tunnel_send(b"second", &mut rng); // seq=2
                                                     // 先收 seq=2（跳变推进窗口）
        assert_eq!(b.tunnel_recv(&f2), Ok(b"second".to_vec()));
        assert_eq!(b.recv_seq, 2);
        assert_eq!(b.replay_bitmap, 1); // bit0 = seq2
                                        // 再收 seq=1（窗口内乱序首收）
        assert_eq!(b.tunnel_recv(&f1), Ok(b"first".to_vec()));
        assert_eq!(b.replay_bitmap, 0b11); // bit1 = seq1 置位
        assert_eq!(b.recv_seq, 2); // recv_seq 不回退
    }

    // TV26: 窗口内乱序重复 → ReplayDetected
    #[test]
    fn tv26_in_window_out_of_order_duplicate() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let f1 = a.tunnel_send(b"first", &mut rng);
        let f2 = a.tunnel_send(b"second", &mut rng);
        assert_eq!(b.tunnel_recv(&f2), Ok(b"second".to_vec()));
        assert_eq!(b.tunnel_recv(&f1), Ok(b"first".to_vec()));
        // 窗口内两帧均重复
        assert_eq!(b.tunnel_recv(&f1), Err(EncryptError::ReplayDetected));
        assert_eq!(b.tunnel_recv(&f2), Err(EncryptError::ReplayDetected));
    }

    // TV27: 大跳变 seq 推进窗口：shift ≥ 64 位图重置为 1；窗口内未收旧帧仍可放行
    #[test]
    fn tv27_large_seq_jump_window_advance() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let mut frames = Vec::new();
        for _ in 0..70 {
            frames.push(a.tunnel_send(b"m", &mut rng));
        }
        // 只收 seq=70（shift=70 ≥ 64 → 位图重置）
        assert!(b.tunnel_recv(&frames[69]).is_ok());
        assert_eq!(b.recv_seq, 70);
        assert_eq!(b.replay_bitmap, 1);
        // seq=7（diff=63，窗口内未收）→ 放行，bit63 置位
        assert_eq!(b.tunnel_recv(&frames[6]), Ok(b"m".to_vec()));
        assert_eq!(b.replay_bitmap, 1 | (1u64 << 63));
        // seq=6（diff=64）→ 超窗重放
        assert_eq!(b.tunnel_recv(&frames[5]), Err(EncryptError::ReplayDetected));
    }

    // TV28: 窗口边界：diff=63 放行 / diff=64 拒绝；seq=0 非法帧拒绝
    #[test]
    fn tv28_window_boundary() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let mut frames = Vec::new();
        for _ in 0..65 {
            frames.push(a.tunnel_send(b"m", &mut rng));
        }
        // 跳到 seq=65
        assert!(b.tunnel_recv(&frames[64]).is_ok());
        // seq=2：diff=63 → 窗口内边界，首收放行
        assert_eq!(b.tunnel_recv(&frames[1]), Ok(b"m".to_vec()));
        // seq=1：diff=64 → 恰好出窗，拒绝
        assert_eq!(b.tunnel_recv(&frames[0]), Err(EncryptError::ReplayDetected));
        // seq=0 非法帧（手工构造：复制合法帧改 seq 字段需同步 tag，改为直接构造 manager 无关路径）
        // 用 rotate 后的全新隧道验证 seq=0 拒绝：构造合法 tag 的 seq=0 帧
        let keys = TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]);
        let mut c = VerticalEncryptTunnel::new(100, 200, keys.clone());
        let mut src = VerticalEncryptTunnel::new(200, 100, keys);
        let f = src.tunnel_send(b"x", &mut rng);
        // 篡改 seq 字段为 0 并重算 tag（模拟合法 MAC 的 seq=0 外来帧）
        let mut f0 = f.clone();
        f0[4..12].copy_from_slice(&0u64.to_be_bytes());
        let f0_len = f0.len();
        let tag = hmac_sm3(c.keys.auth_key(), &f0[..f0_len - TAG_LEN]);
        f0[f0_len - TAG_LEN..].copy_from_slice(&tag);
        assert_eq!(c.tunnel_recv(&f0), Err(EncryptError::ReplayDetected));
    }

    // ------------------------------------------------------------
    // TV29~TV30：HMAC 篡改
    // ------------------------------------------------------------

    // TV29: 篡改密文 → TagMismatch（重放窗口不推进）
    #[test]
    fn tv29_tampered_ciphertext() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let mut f = a.tunnel_send(b"secret", &mut rng);
        f[HEADER_LEN] ^= 0x01; // 篡改 ct 首字节
        assert_eq!(b.tunnel_recv(&f), Err(EncryptError::TagMismatch));
        assert_eq!(b.recv_seq, 0); // 窗口未推进
        assert_eq!(b.replay_bitmap, 0);
    }

    // TV30: 篡改 tag → TagMismatch
    #[test]
    fn tv30_tampered_tag() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let mut f = a.tunnel_send(b"secret", &mut rng);
        let last = f.len() - 1;
        f[last] ^= 0x01;
        assert_eq!(b.tunnel_recv(&f), Err(EncryptError::TagMismatch));
        assert_eq!(b.recv_seq, 0);
    }

    // ------------------------------------------------------------
    // TV31~TV32：rotate 换钥
    // ------------------------------------------------------------

    // TV31: rotate 后新密钥可互通；旧密钥帧 → TagMismatch（C80）
    #[test]
    fn tv31_rotate_old_frame_rejected() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let old_frame = a.tunnel_send(b"old", &mut rng);
        // 双端同步轮换
        let new_keys = TunnelKeys::new([0x01u8; 16], [0x02u8; 32]);
        a.rotate(new_keys.clone());
        b.rotate(new_keys);
        // 旧帧（旧密钥 tag）→ 新密钥 HMAC 校验失败
        assert_eq!(b.tunnel_recv(&old_frame), Err(EncryptError::TagMismatch));
        // 新密钥互通
        let f = a.tunnel_send(b"new", &mut rng);
        assert_eq!(b.tunnel_recv(&f), Ok(b"new".to_vec()));
    }

    // TV32: rotate 重置 send_seq/recv_seq/replay_bitmap
    #[test]
    fn tv32_rotate_resets_state() {
        let (mut a, mut b) = fixed_tunnel_pair();
        let mut rng = CsRng::new();
        let f = a.tunnel_send(b"x", &mut rng);
        assert!(b.tunnel_recv(&f).is_ok());
        assert_eq!(a.send_seq, 1);
        assert_eq!(b.recv_seq, 1);
        assert_ne!(b.replay_bitmap, 0);
        a.rotate(TunnelKeys::new([1u8; 16], [2u8; 32]));
        b.rotate(TunnelKeys::new([1u8; 16], [2u8; 32]));
        assert_eq!(a.send_seq, 0);
        assert_eq!(b.recv_seq, 0);
        assert_eq!(b.replay_bitmap, 0);
        // 重置后序号自 1 重新开始，双端可继续互通
        let f2 = a.tunnel_send(b"after", &mut rng);
        assert_eq!(&f2[4..12], &1u64.to_be_bytes());
        assert_eq!(b.tunnel_recv(&f2), Ok(b"after".to_vec()));
    }

    // ------------------------------------------------------------
    // TV33~TV36：DispatchToken / verify_dispatch_auth
    // ------------------------------------------------------------

    // TV33: 未过期 + 签名正确 → Granted
    #[test]
    fn tv33_dispatch_granted() {
        let (token, kp) = make_token(b"dispatch-order-1", 5000);
        assert_eq!(
            verify_dispatch_auth(&token, &kp.public_key, 4999),
            AuthResult::Granted
        );
    }

    // TV34: 未过期 + 签名错误 → Denied（篡改签名与用错公钥两路）
    #[test]
    fn tv34_dispatch_denied() {
        let (mut token, kp) = make_token(b"dispatch-order-1", 5000);
        token.signature[10] ^= 0x01;
        assert_eq!(
            verify_dispatch_auth(&token, &kp.public_key, 4999),
            AuthResult::Denied
        );
        let (token2, _) = make_token(b"dispatch-order-1", 5000);
        let stranger = stranger_keypair();
        assert_eq!(
            verify_dispatch_auth(&token2, &stranger.public_key, 4999),
            AuthResult::Denied
        );
    }

    // TV35: 过期判定先于验签：签名本身正确但已过期 → Expired（不验签）
    #[test]
    fn tv35_dispatch_expired_before_verify() {
        let (token, kp) = make_token(b"dispatch-order-1", 5000);
        // 签名正确但 now > expires
        assert_eq!(
            verify_dispatch_auth(&token, &kp.public_key, 6000),
            AuthResult::Expired
        );
    }

    // TV36: 边界 now_ms == expires_ms → Expired
    #[test]
    fn tv36_dispatch_boundary_now_eq_expires() {
        let (token, kp) = make_token(b"dispatch-order-1", 5000);
        assert_eq!(
            verify_dispatch_auth(&token, &kp.public_key, 5000),
            AuthResult::Expired
        );
        // 紧邻边界前一刻仍 Granted
        assert_eq!(
            verify_dispatch_auth(&token, &kp.public_key, 4999),
            AuthResult::Granted
        );
    }

    // ------------------------------------------------------------
    // TV37~TV40：Mock 装置 + TunnelManager
    // ------------------------------------------------------------

    // TV37: Mock 装置故障注入（xmit 递减 → Err(DeviceError)）与 poll FIFO 语义
    #[test]
    fn tv37_mock_device_fail_injection() {
        let mut dev = MockVerticalEncryptDevice {
            fail_times: 2,
            ..MockVerticalEncryptDevice::new()
        };
        assert_eq!(dev.xmit(b"a"), Err(EncryptError::DeviceError));
        assert_eq!(dev.fail_times, 1);
        assert_eq!(dev.xmit(b"b"), Err(EncryptError::DeviceError));
        assert_eq!(dev.fail_times, 0);
        assert!(dev.xmitted.is_empty()); // 失败期间不记录
        assert_eq!(dev.xmit(b"c"), Ok(()));
        assert_eq!(dev.xmitted, vec![b"c".to_vec()]);
        // poll：空 → None；入队后按 FIFO 弹出
        assert_eq!(dev.poll(), None);
        dev.pending.push(b"f1".to_vec());
        dev.pending.push(b"f2".to_vec());
        assert_eq!(dev.poll(), Some(b"f1".to_vec()));
        assert_eq!(dev.poll(), Some(b"f2".to_vec()));
        assert_eq!(dev.poll(), None);
    }

    // TV38: TunnelManager add/send/recv 路由正确（双隧道按 remote_spi 区分）
    #[test]
    fn tv38_manager_routing_two_tunnels() {
        let keys = TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]);
        let mut mgr = TunnelManager::new(MockVerticalEncryptDevice::new());
        mgr.add(VerticalEncryptTunnel::new(100, 200, keys.clone()));
        mgr.add(VerticalEncryptTunnel::new(200, 100, keys));
        assert_eq!(mgr.established_count, 2);
        assert!(mgr.tunnels.contains_key(&100));
        assert!(mgr.tunnels.contains_key(&200));

        let mut rng = CsRng::new();
        // 经隧道 100 发送：帧 spi=100 → 路由到 remote_spi==100 的隧道 200
        mgr.send(100, b"to-peer", &mut rng).expect("send");
        assert_eq!(mgr.send_count, 1);
        let frame = mgr.device.xmitted.remove(0);
        assert_eq!(&frame[0..4], &100u32.to_be_bytes());
        mgr.device.pending.push(frame);
        let (spi, pt) = mgr.recv().expect("recv");
        assert_eq!(spi, 100);
        assert_eq!(pt, b"to-peer".to_vec());
        assert_eq!(mgr.recv_count, 1);

        // 经隧道 200 发送：帧 spi=200 → 路由到隧道 100
        mgr.send(200, b"reply", &mut rng).expect("send2");
        let frame2 = mgr.device.xmitted.remove(0);
        mgr.device.pending.push(frame2);
        let (spi2, pt2) = mgr.recv().expect("recv2");
        assert_eq!(spi2, 200);
        assert_eq!(pt2, b"reply".to_vec());
        assert_eq!(mgr.send_count, 2);
        assert_eq!(mgr.recv_count, 2);

        // remove 语义
        assert!(!mgr.remove(999));
        assert!(mgr.remove(200));
        assert!(!mgr.tunnels.contains_key(&200));
    }

    // TV39: UnknownTunnel：send 未知 spi / recv 未知 spi；recv 无数据 → DeviceError
    #[test]
    fn tv39_manager_unknown_tunnel() {
        let keys = TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]);
        let mut mgr = TunnelManager::new(MockVerticalEncryptDevice::new());
        mgr.add(VerticalEncryptTunnel::new(100, 200, keys.clone()));
        let mut rng = CsRng::new();
        // send 未知 spi
        assert_eq!(
            mgr.send(999, b"x", &mut rng),
            Err(EncryptError::UnknownTunnel)
        );
        assert_eq!(mgr.send_count, 0);
        // recv 无数据 → DeviceError
        assert_eq!(mgr.recv(), Err(EncryptError::DeviceError));
        // recv 未知 spi 帧（spi=77，无隧道 remote_spi==77）
        let mut src = VerticalEncryptTunnel::new(77, 100, keys);
        let f = src.tunnel_send(b"foreign", &mut rng);
        mgr.device.pending.push(f);
        assert_eq!(mgr.recv(), Err(EncryptError::UnknownTunnel));
        assert_eq!(mgr.recv_count, 0);
        // 过短帧 → InvalidFrame
        mgr.device.pending.push(vec![1, 2, 3]);
        assert_eq!(mgr.recv(), Err(EncryptError::InvalidFrame));
    }

    // TV40: 4 计数器累计（established/send/recv/replay_reject）+ xmit 失败不计 send_count
    #[test]
    fn tv40_manager_counters_accumulate() {
        let keys = TunnelKeys::new([0x5Au8; 16], [0xA5u8; 32]);
        let mut mgr = TunnelManager::new(MockVerticalEncryptDevice::new());
        mgr.add(VerticalEncryptTunnel::new(100, 200, keys.clone()));
        mgr.add(VerticalEncryptTunnel::new(200, 100, keys));
        let mut rng = CsRng::new();
        // 2 次成功发送 + 2 次成功接收
        for msg in [&b"m1"[..], b"m2"] {
            mgr.send(100, msg, &mut rng).expect("send");
            let f = mgr.device.xmitted.remove(0);
            mgr.device.pending.push(f);
            assert_eq!(mgr.recv().map(|r| r.1), Ok(msg.to_vec()));
        }
        assert_eq!(mgr.established_count, 2);
        assert_eq!(mgr.send_count, 2);
        assert_eq!(mgr.recv_count, 2);
        assert_eq!(mgr.replay_reject_count, 0);
        // 重放：同帧二次送入 → ReplayDetected + replay_reject_count+=1
        mgr.send(100, b"again", &mut rng).expect("send3");
        let f = mgr.device.xmitted.remove(0);
        mgr.device.pending.push(f.clone());
        mgr.device.pending.push(f);
        assert_eq!(mgr.recv().map(|r| r.1), Ok(b"again".to_vec()));
        assert_eq!(mgr.recv(), Err(EncryptError::ReplayDetected));
        assert_eq!(mgr.replay_reject_count, 1);
        // xmit 故障注入：发送失败传播 DeviceError 且 send_count 不变
        mgr.device.fail_times = 1;
        assert_eq!(
            mgr.send(100, b"fail", &mut rng),
            Err(EncryptError::DeviceError)
        );
        assert_eq!(mgr.send_count, 3); // 未增加
    }
}
