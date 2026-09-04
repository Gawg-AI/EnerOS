//! Agent process handle (AGT-003).
//!
//! MASTER §G6 (L2064): "Agent supervisor、真实 ELF spawn、kill/restart/quarantine."
//! MASTER §1.2 (L67): "AgentOS | spawn 后对象即释放，SDK IPC 只计数 |
//! 接口原型"
//! MASTER §1.3 P0 风险 #6 (L89): "Agent spawn 不是进程创建。AgentEntry 在局部
//! Box 中启动后即被释放；registry 留下的只是描述符/状态。"
//!
//! `ProcessHandle` replaces the prohibited `Box<dyn AgentEntry>` pattern.
//! Instead of creating an in-process trait object whose methods are called
//! directly, the supervisor creates a `ProcessHandle` that represents a
//! **real isolated execution context** with:
//!
//! - Its own address space (PT_LOAD segments from a validated ELF)
//! - An entry point (where execution begins)
//! - A resource budget (memory, CPU)
//! - A capability set (what the Agent is allowed to do)
//! - A lifecycle state (Created → Running → Stopped)
//!
//! Communication with the Agent process is through the IPC wire codec
//! (AGT-006/007), NOT through trait object method calls.
//!
//! # Design
//!
//! `ProcessHandle` is a **data** type, not a trait object. It contains the
//! information needed to create and manage a real EL0 process. The actual
//! process creation (mapping pages, setting up EL0, scheduling) is performed
//! by the kernel/supervisor through a defined interface.
//!
//! # no_std
//!
//! `#![cfg_attr(not(test), no_std)]` inherited from lib.rs.

use alloc::string::String;
use alloc::vec::Vec;

use crate::elf::LoadedSegment;
use crate::error::AgentError;
use crate::{AgentId, AgentType};

/// Process state (mirrors the lifecycle but is process-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// ELF loaded and validated, process not yet started.
    Loaded,
    /// Process is running in its own address space.
    Running,
    /// Process stopped (graceful shutdown).
    Stopped,
    /// Process crashed or was killed.
    Terminated,
    /// Process quarantined (suspicious behavior, under investigation).
    Quarantined,
}

/// Resource budget for an Agent process.
#[derive(Debug, Clone, Copy)]
pub struct ResourceBudget {
    /// Maximum memory in bytes.
    pub max_memory_bytes: u64,
    /// CPU priority (0=lowest, 255=highest).
    pub priority: u8,
    /// Maximum CPU time per tick in microseconds.
    pub max_cpu_us_per_tick: u64,
    /// Maximum IPC messages per second.
    pub max_ipc_per_sec: u32,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024, // 64 MB
            priority: 128,
            max_cpu_us_per_tick: 10_000, // 10 ms
            max_ipc_per_sec: 1000,
        }
    }
}

/// A handle to a loaded Agent process.
///
/// This is NOT a `Box<dyn AgentEntry>`. It is a data structure that
/// represents a real isolated process with its own address space.
/// The supervisor creates this handle after loading and validating
/// the Agent's ELF binary, and uses it to manage the process lifecycle.
#[derive(Debug, Clone)]
pub struct ProcessHandle {
    /// Agent ID (assigned by the supervisor).
    pub agent_id: AgentId,
    /// Agent type (from the package manifest).
    pub agent_type: AgentType,
    /// Agent name (from the package manifest).
    pub name: String,
    /// ELF entry point virtual address.
    pub entry_point: u64,
    /// Loaded PT_LOAD segments (validated for W^X).
    pub segments: Vec<LoadedSegment>,
    /// Resource budget.
    pub budget: ResourceBudget,
    /// Current process state.
    pub state: ProcessState,
    /// Total virtual memory size.
    pub total_vmem: u64,
}

impl ProcessHandle {
    /// Create a new ProcessHandle from a loaded ELF.
    pub fn new(
        agent_id: AgentId,
        agent_type: AgentType,
        name: &str,
        entry_point: u64,
        segments: &[LoadedSegment],
        budget: ResourceBudget,
    ) -> Self {
        let total_vmem = segments.iter().map(|s| s.memsz).sum();
        Self {
            agent_id,
            agent_type,
            name: String::from(name),
            entry_point,
            segments: segments.to_vec(),
            budget,
            state: ProcessState::Loaded,
            total_vmem,
        }
    }

    /// Transition to Running.
    pub fn start(&mut self) -> Result<(), String> {
        match self.state {
            ProcessState::Loaded => {
                self.state = ProcessState::Running;
                Ok(())
            }
            _ => Err(String::from("process not in Loaded state")),
        }
    }

    /// Transition to Stopped (graceful).
    pub fn stop(&mut self) -> Result<(), String> {
        match self.state {
            ProcessState::Running => {
                self.state = ProcessState::Stopped;
                Ok(())
            }
            _ => Err(String::from("process not running")),
        }
    }

    /// Transition to Terminated (killed).
    pub fn kill(&mut self) {
        self.state = ProcessState::Terminated;
    }

    /// Transition to Quarantined.
    pub fn quarantine(&mut self) {
        self.state = ProcessState::Quarantined;
    }

    /// True if the process is alive (Running or Loaded).
    pub fn is_alive(&self) -> bool {
        matches!(self.state, ProcessState::Running | ProcessState::Loaded)
    }

    /// True if the process memory exceeds its budget.
    pub fn exceeds_memory_budget(&self) -> bool {
        self.total_vmem > self.budget.max_memory_bytes
    }

    /// Check if the process has an executable segment.
    pub fn has_executable_segment(&self) -> bool {
        self.segments.iter().any(|s| s.flags.execute)
    }
}

/// Agent package manifest (pre-load metadata).
///
/// Describes a signed Agent package before the ELF is loaded.
#[derive(Debug, Clone)]
pub struct AgentPackageManifest {
    /// Agent name.
    pub name: String,
    /// Agent version (semantic).
    pub version: String,
    /// Agent type.
    pub agent_type: AgentType,
    /// Resource budget.
    pub budget: ResourceBudget,
    /// SHA-256 hash of the ELF binary (32 bytes).
    pub elf_hash: [u8; 32],
    /// Signature over (name + version + elf_hash) (64 bytes SM2).
    pub signature: [u8; 64],
    /// Signer public key (SM2, 64 bytes uncompressed).
    pub signer_pubkey: [u8; 64],
}

/// A complete Agent package: manifest + ELF binary.
#[derive(Debug, Clone)]
pub struct AgentPackage {
    pub manifest: AgentPackageManifest,
    pub elf_bytes: Vec<u8>,
}

impl AgentPackage {
    /// Validate that the ELF hash in the manifest matches the actual ELF bytes.
    pub fn verify_hash(&self) -> bool {
        let computed = sm3_hash(&self.elf_bytes);
        computed == self.manifest.elf_hash
    }

    /// SEC-002 + 蓝图 66 WP-66.1.2（体检 C3）：**锚定验签**——完整验证
    /// （fail-closed）+ 签名公钥必须与注入信任锚逐字节相等。
    ///
    /// 旧 `verify()`（包内公钥验包内签名 = 自证）已删除：外部唯一入口是
    /// 锚定版；锚 = 装配期静态注入（`AgentSpawner::new`），禁止 None 兜底
    /// 回退自证。
    ///
    /// 验证序（与旧 verify() 逐语义等价 + 锚定准入）：
    /// 1) 哈希校验；2) 未签名（signature 与 signer_pubkey 全零）一律拒绝
    ///    （供应链要求签名 ELF PIE）；3) SM2 验签（载荷
    ///    name‖version‖elf_hash）；4) 签名公钥 == anchor（逐字节；锚定
    ///    准入，SIG13）。
    pub fn verify_against(
        &self,
        anchor: &eneros_crypto::sm2::Sm2PublicKey,
    ) -> Result<(), AgentError> {
        use eneros_crypto::sm2::{sm2_verify, Sm2PublicKey, Sm2Signature};

        // 1) 哈希校验
        if !self.verify_hash() {
            return Err(AgentError::CodeLoadFailed(String::from(
                "agent ELF hash mismatch",
            )));
        }
        // 2) 未签名拒绝（fail-closed）
        let unsigned = self.manifest.signature.iter().all(|&b| b == 0)
            || self.manifest.signer_pubkey.iter().all(|&b| b == 0);
        if unsigned {
            return Err(AgentError::CodeLoadFailed(String::from(
                "agent package is unsigned; signed ELF PIE required (SEC-002)",
            )));
        }
        // 3) SM2 验签（载荷 = name‖version‖elf_hash，与 signer 侧一致）
        let mut msg: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(
            self.manifest.name.len() + self.manifest.version.len() + 32,
        );
        msg.extend_from_slice(self.manifest.name.as_bytes());
        msg.extend_from_slice(self.manifest.version.as_bytes());
        msg.extend_from_slice(&self.manifest.elf_hash);

        let mut pk_bytes = [0u8; 65];
        pk_bytes[0] = 0x04;
        pk_bytes[1..65].copy_from_slice(&self.manifest.signer_pubkey);
        let pk = Sm2PublicKey::from_bytes(&pk_bytes).map_err(|e| {
            AgentError::CodeLoadFailed(alloc::format!("invalid signer pubkey: {e:?}"))
        })?;
        let sig = Sm2Signature::from_bytes(&self.manifest.signature);
        let valid = sm2_verify(&msg, &sig, &pk)
            .map_err(|e| AgentError::CodeLoadFailed(alloc::format!("SM2 verify failed: {e:?}")))?;
        if !valid {
            return Err(AgentError::CodeLoadFailed(String::from(
                "agent package signature invalid",
            )));
        }
        // 4) 锚定准入（SIG13）：签名公钥必须与信任锚逐字节相等
        let anchor_xy = &anchor.to_bytes_uncompressed()[1..65];
        if self.manifest.signer_pubkey[..] != *anchor_xy {
            return Err(AgentError::CodeLoadFailed(String::from(
                "agent pubkey not pinned (trust anchor mismatch; SEC-002)",
            )));
        }
        Ok(())
    }
}

/// SM3 哈希（SEC-001：真实 eneros-crypto SM3）。
///
/// Agent 包完整性校验必须使用国密 SM3（GB/T 32905-2016）。
/// 此前 FNV-1a 假哈希（A1）已删除——假哈希使完整性校验形同虚设。
pub(crate) fn sm3_hash(data: &[u8]) -> [u8; 32] {
    eneros_crypto::sm3_hash(data)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    use crate::elf::SegmentFlags;

    fn dummy_segments() -> Vec<LoadedSegment> {
        vec![
            LoadedSegment {
                vaddr: 0x1000,
                memsz: 0x1000,
                filesz: 0x1000,
                offset: 0,
                flags: SegmentFlags {
                    read: true,
                    write: false,
                    execute: true,
                },
                align: 0x1000,
            },
            LoadedSegment {
                vaddr: 0x2000,
                memsz: 0x2000,
                filesz: 0x1000,
                offset: 0x1000,
                flags: SegmentFlags {
                    read: true,
                    write: true,
                    execute: false,
                },
                align: 0x1000,
            },
        ]
    }

    #[test]
    fn test_process_handle_creation() {
        let handle = ProcessHandle::new(
            AgentId::generate(),
            AgentType::Energy,
            "test-agent",
            0x1000,
            &dummy_segments(),
            ResourceBudget::default(),
        );
        assert_eq!(handle.state, ProcessState::Loaded);
        assert!(handle.is_alive());
        assert!(handle.has_executable_segment());
        assert_eq!(handle.total_vmem, 0x3000); // 0x1000 + 0x2000
    }

    #[test]
    fn test_process_lifecycle() {
        let mut handle = ProcessHandle::new(
            AgentId::generate(),
            AgentType::Device,
            "dev",
            0x1000,
            &dummy_segments(),
            ResourceBudget::default(),
        );

        // Loaded → Running → Stopped.
        assert!(handle.start().is_ok());
        assert_eq!(handle.state, ProcessState::Running);
        assert!(handle.is_alive());

        assert!(handle.stop().is_ok());
        assert_eq!(handle.state, ProcessState::Stopped);
        assert!(!handle.is_alive());

        // Cannot start from Stopped.
        assert!(handle.start().is_err());
    }

    #[test]
    fn test_process_kill() {
        let mut handle = ProcessHandle::new(
            AgentId::generate(),
            AgentType::Grid,
            "grid",
            0x1000,
            &dummy_segments(),
            ResourceBudget::default(),
        );
        handle.start().unwrap();
        handle.kill();
        assert_eq!(handle.state, ProcessState::Terminated);
        assert!(!handle.is_alive());
    }

    #[test]
    fn test_process_quarantine() {
        let mut handle = ProcessHandle::new(
            AgentId::generate(),
            AgentType::Market,
            "market",
            0x1000,
            &dummy_segments(),
            ResourceBudget::default(),
        );
        handle.start().unwrap();
        handle.quarantine();
        assert_eq!(handle.state, ProcessState::Quarantined);
        assert!(!handle.is_alive());
    }

    #[test]
    fn test_memory_budget_exceeded() {
        let small_budget = ResourceBudget {
            max_memory_bytes: 0x100, // 256 bytes — way too small
            priority: 128,
            max_cpu_us_per_tick: 10_000,
            max_ipc_per_sec: 1000,
        };
        let handle = ProcessHandle::new(
            AgentId::generate(),
            AgentType::Energy,
            "big",
            0x1000,
            &dummy_segments(),
            small_budget,
        );
        assert!(handle.exceeds_memory_budget());
    }

    #[test]
    fn test_package_hash_verification() {
        let elf_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let hash = sm3_hash(&elf_data);

        let manifest = AgentPackageManifest {
            name: String::from("test"),
            version: String::from("1.0.0"),
            agent_type: AgentType::Energy,
            budget: ResourceBudget::default(),
            elf_hash: hash,
            signature: [0u8; 64],
            signer_pubkey: [0u8; 64],
        };
        let pkg = AgentPackage {
            manifest,
            elf_bytes: elf_data.clone(),
        };
        assert!(pkg.verify_hash());

        // Tamper with ELF.
        let mut tampered = pkg.clone();
        tampered.elf_bytes[0] = 0x00;
        assert!(!tampered.verify_hash());
    }

    /// SEC-001 回归：真实 SM3 必须满足 GB/T 32905-2016 已知测试向量。
    ///
    /// SM3("abc") = 66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0
    /// FNV-1a 假哈希（A1 旧实现）对该向量必然失败——此测试是
    /// "旧代码失败/新代码通过"的捕获证据。
    #[test]
    fn test_sm3_kat_vector_abc() {
        let digest = sm3_hash(b"abc");
        let expected: [u8; 32] = [
            0x66, 0xc7, 0xf0, 0xf4, 0x62, 0xee, 0xed, 0xd9, //
            0xd1, 0xf2, 0xd4, 0x6b, 0xdc, 0x10, 0xe4, 0xe2, //
            0x41, 0x67, 0xc4, 0x87, 0x5c, 0xf2, 0xf7, 0xa2, //
            0x29, 0x7d, 0xa0, 0x2b, 0x8f, 0x4b, 0xa8, 0xe0, //
        ];
        assert_eq!(digest, expected, "SM3('abc') KAT 向量必须匹配");
    }

    /// SEC-001 回归：SM3 空串 KAT 向量（1ab21d8355cfa17f8e61194831e81a8f22bec8c728fefb747ed035eb5082aa2b）。
    #[test]
    fn test_sm3_kat_vector_empty() {
        let digest = sm3_hash(b"");
        let expected: [u8; 32] = [
            0x1a, 0xb2, 0x1d, 0x83, 0x55, 0xcf, 0xa1, 0x7f, //
            0x8e, 0x61, 0x19, 0x48, 0x31, 0xe8, 0x1a, 0x8f, //
            0x22, 0xbe, 0xc8, 0xc7, 0x28, 0xfe, 0xfb, 0x74, //
            0x7e, 0xd0, 0x35, 0xeb, 0x50, 0x82, 0xaa, 0x2b, //
        ];
        assert_eq!(digest, expected, "SM3('') KAT 向量必须匹配");
    }

    /// SEC-002 回归：签名 ELF PIE 验签全链路。
    ///
    /// - 正确签名 → `verify()` Ok
    /// - 篡改 ELF（哈希错）→ 拒绝
    /// - 篡改签名（错签）→ SM2 验签拒绝
    /// - 未签名（全零）→ 拒绝（fail-closed）
    #[test]
    fn test_sec002_sm2_signature_verification_chain() {
        use eneros_crypto::sm2::{sm2_sign, Sm2KeyPair, Sm2PrivateKey};
        use eneros_crypto::CsRng;

        let elf_data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03];
        let hash = sm3_hash(&elf_data);
        let mut rng = CsRng::new();

        // 生成密钥对并签名载荷 name‖version‖elf_hash
        let keypair = Sm2KeyPair::generate(&mut rng).expect("keygen");
        let msg = [
            b"energy-agent".as_slice(),
            b"1.0.0".as_slice(),
            hash.as_slice(),
        ]
        .concat();
        let sig =
            sm2_sign(&msg, &keypair.private_key, &keypair.public_key, &mut rng).expect("sign");
        let sig_bytes = sig.to_bytes();
        let pk_bytes: [u8; 64] = {
            let uncompressed = keypair.public_key.to_bytes_uncompressed();
            let mut out = [0u8; 64];
            out.copy_from_slice(&uncompressed[1..65]);
            out
        };

        let manifest = AgentPackageManifest {
            name: String::from("energy-agent"),
            version: String::from("1.0.0"),
            agent_type: AgentType::Energy,
            budget: ResourceBudget::default(),
            elf_hash: hash,
            signature: sig_bytes,
            signer_pubkey: pk_bytes,
        };
        let pkg = AgentPackage {
            manifest,
            elf_bytes: elf_data.clone(),
        };

        // 1) 正确签名 + 锚=签名钥 → 通过（蓝图 66：外部唯一入口锚定版）
        pkg.verify_against(&keypair.public_key)
            .expect("signed package must verify against its own key as anchor");

        // 2) 篡改 ELF → 哈希校验拒绝
        let mut tampered = pkg.clone();
        tampered.elf_bytes[0] ^= 0xFF;
        assert!(
            tampered.verify_against(&keypair.public_key).is_err(),
            "tampered ELF must be rejected"
        );

        // 3) 篡改签名（错误 r 分量）→ SM2 验签拒绝
        let mut bad_sig = pkg.clone();
        bad_sig.manifest.signature[0] ^= 0x01;
        assert!(
            bad_sig.verify_against(&keypair.public_key).is_err(),
            "wrong signature must be rejected"
        );

        // 4) 未签名（全零）→ fail-closed 拒绝
        let mut unsigned = pkg.clone();
        unsigned.manifest.signature = [0u8; 64];
        assert!(
            unsigned.verify_against(&keypair.public_key).is_err(),
            "unsigned package must be rejected (fail-closed)"
        );

        // 5) 换公钥（错签方）→ 验签拒绝
        let mut wrong_pk = pkg.clone();
        wrong_pk.manifest.signer_pubkey[0] ^= 0x01;
        assert!(
            wrong_pk.verify_against(&keypair.public_key).is_err(),
            "wrong signer pubkey must be rejected"
        );

        // 6) SIG13（蓝图 66）：锚定准入——锚 ≠ 签名钥时即使签名本身有效也拒绝
        //（坑：CsRng::new() 为固定种子，两次 generate 得同一钥对——负例锚
        //  必须用不同固定私钥 d=2 派生，勿走随机源）
        let mut d2 = [0u8; 32];
        d2[31] = 2;
        let other_keypair =
            Sm2KeyPair::from_private_key(&Sm2PrivateKey::from_bytes(&d2).expect("fixed sk"))
                .expect("keypair");
        let err = pkg
            .verify_against(&other_keypair.public_key)
            .expect_err("non-pinned signer must be rejected (SIG13)");
        assert!(
            format!("{err}").contains("agent pubkey not pinned"),
            "SIG13 错误词形须含 agent pubkey not pinned，得：{err}"
        );
    }

    #[test]
    fn test_elf_load_then_process_handle() {
        // Build a minimal valid ELF (reusing the test helper pattern from elf.rs).
        use crate::elf::*;
        let mut buf = vec![0u8; ELF64_EHDR_SIZE + ELF64_PHDR_SIZE + 8];
        buf[0..4].copy_from_slice(&ELFMAG);
        buf[4] = ELFCLASS64;
        buf[5] = ELFDATA2LSB;
        buf[6] = 1;
        buf[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        buf[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        buf[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[32..40].copy_from_slice(&(ELF64_EHDR_SIZE as u64).to_le_bytes());
        buf[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        buf[56..58].copy_from_slice(&1u16.to_le_bytes());
        let ph = ELF64_EHDR_SIZE;
        buf[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        buf[ph + 4..ph + 8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        buf[ph + 8..ph + 16]
            .copy_from_slice(&((ELF64_EHDR_SIZE + ELF64_PHDR_SIZE) as u64).to_le_bytes());
        buf[ph + 16..ph + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        buf[ph + 32..ph + 40].copy_from_slice(&8u64.to_le_bytes());
        buf[ph + 40..ph + 48].copy_from_slice(&8u64.to_le_bytes());
        buf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());

        // Parse ELF.
        let elf = LoadedElf::parse(&buf).expect("ELF should parse");

        // Create ProcessHandle from parsed ELF.
        let handle = ProcessHandle::new(
            AgentId::generate(),
            AgentType::Energy,
            "test-agent",
            elf.entry(),
            elf.segments(),
            ResourceBudget::default(),
        );

        assert_eq!(handle.entry_point, 0x1000);
        assert_eq!(handle.segments.len(), 1);
        assert!(handle.has_executable_segment());
        assert_eq!(handle.state, ProcessState::Loaded);
    }
}
