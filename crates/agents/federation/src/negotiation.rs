//! # Phase 7 承诺协商会话（蓝图 61 §12 §2.3 协议流程）
//!
//! [`SiteNegotiator`] / [`CoordinatorNegotiator`] 是实现 §2.3 六步流程的
//! 确定性状态机：offer 发布/修订 → 分配提议 → 接受或结构化拒绝 →
//! 双方签 AllocationCommitment → 回执 → 聚合进度/结算证据。
//!
//! 硬规则：
//!
//! - 无跨站原子设备事务：每站自治，通过承诺/偏差/重规划收敛（§2.3 末行；
//!   站内执行永远由站内 Compiler/Saga 决定，本会话只记录承诺义务）。
//! - 接受提议前必须过 [`AllocationSolver::check_envelope`]（站内最新
//!   快照生成的包络是最终权威；云发恶意超范围目标 → 结构化拒绝，站内
//!   继续自治——`tests/phase7_security.rs::malicious_cloud_target_rejected`）。
//! - 防重放：epoch 单调（重启递增）+ per-peer 窗口 seq；旧 epoch 消息
//!   （陈旧 leader）一律拒收；重放 seq 一律拒收（可观测计数）。
//! - 全部事件 append-only journal；协调器崩溃后 `restore` 重建会话，
//!   已签承诺不重复签发（`tests/phase7_security.rs::coordinator_restart`）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::Sm2KeyPair;

use crate::allocation::{AllocationSolver, EnvelopeCheck};
use crate::keys::{KeyError, KeyRegistry};
use crate::offer::{
    sign_envelope, AllocationCommitment, AllocationProposal, CapabilityOffer, CommitmentAck,
    EnvelopeHdr, FedBody, FedEnvelope, FedKind, OfferReject, OfferRevocation, ProgressEvidence,
    RejectReason, SettlementEvidence, CLOCK_SKEW_TOLERANCE_MS,
};

/// 会话可容纳的最大站点数（C3 100 站资格上限的 10 倍余量，防内存耗尽）。
pub const MAX_SESSIONS: usize = 1_024;

/// 本节点签名器（密钥永不离开站点/协调器；关键路径禁 Debug）。
pub struct SiteSigner {
    pub key_id: u32,
    kp: Sm2KeyPair,
    rng: CsRng,
}

impl SiteSigner {
    pub fn new(key_id: u32, kp: Sm2KeyPair, rng: CsRng) -> Self {
        Self { key_id, kp, rng }
    }

    pub fn public_key(&self) -> &Sm2PublicKeyAlias {
        &self.kp.public_key
    }

    /// 签一封消息（信封头由调用方提供）。
    pub fn sign(&mut self, body: &FedBody, hdr: &EnvelopeHdr) -> FedEnvelope {
        sign_envelope(body, hdr, &self.kp, &mut self.rng).expect("sign")
    }
}

/// 公钥别名（避免调用方直接依赖 crypto 内部路径）。
pub type Sm2PublicKeyAlias = eneros_crypto::sm2::Sm2PublicKey;

/// 消息方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

/// 证据种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    Progress,
    Settlement,
}

/// Journal 记录（append-only，一次写入不可变）。
#[derive(Clone, Debug)]
pub struct JournalRecord {
    pub at_ns: u64,
    pub dir: Dir,
    pub peer: u32,
    pub kind: FedKind,
    pub epoch: u64,
    pub sequence: u64,
    pub body: FedBody,
    /// 入向：验签/窗口判定结果；出向：本地决策说明。
    pub verdict: &'static str,
}

/// 会话日志（append-only；restore 时按序重放）。
#[derive(Clone, Debug, Default)]
pub struct NegotiationJournal {
    pub records: Vec<JournalRecord>,
}

impl NegotiationJournal {
    pub fn append(&mut self, r: JournalRecord) {
        self.records.push(r);
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// 会话处理结果（驱动方据此发 outbox / 记录）。
#[derive(Clone, Debug, PartialEq)]
pub enum StepOutcome {
    /// 无副作用处理完成。
    Accepted,
    /// offer 注册/修订成功。
    OfferRegistered { offer_id: [u8; 16], revision: u64 },
    /// offer 与当前不兼容（重复/过期 revision）。
    Superseded { current_revision: u64 },
    /// offer 已撤销。
    OfferRevoked { offer_id: [u8; 16] },
    /// 站点接受提议并已产出承诺（outbox 含 Commitment）。
    ProposalAccepted { commitment_id: [u8; 16] },
    /// 站点结构化拒绝（outbox 含 Reject）。
    ProposalRejected {
        reason: RejectReason,
        detail: Vec<u8>,
    },
    /// 协调器确认承诺并已产出 Ack。
    CommitmentConfirmed { commitment_id: [u8; 16] },
    /// 站点收到 Ack。
    Acknowledged { commitment_id: [u8; 16] },
    /// 对端拒绝。
    RejectedByPeer {
        reason: RejectReason,
        detail: Vec<u8>,
    },
    /// 证据已归档。
    EvidenceRecorded { kind: EvidenceKind },
    /// 会话到期（offer/commitment 过期）。
    Expired,
    /// 重放（同 epoch seq 回退）。
    ReplayRejected { sequence: u64 },
    /// 陈旧 leader（旧 epoch）。
    StaleLeaderRejected { epoch: u64 },
    /// 验签/密钥失败。
    SignatureRejected,
    /// 租户不符。
    TenantMismatch,
    /// 无法处理（状态不允许该消息）。
    NotInState,
}

/// 承诺状态（站点侧）。
#[derive(Clone, Debug)]
pub struct SiteCommitment {
    pub commitment: AllocationCommitment,
    pub acked: bool,
    pub last_progress_seq: u64,
}

/// 站点协商会话。
pub struct SiteNegotiator {
    pub site_id: u32,
    pub tenant_id: u32,
    /// 本地纪元（重启递增；由调用方从持久化状态恢复后 +1 传入）。
    pub epoch: u64,
    keys: KeyRegistry,
    peer_epoch: Option<u64>,
    last_seq: u64,
    offer: Option<CapabilityOffer>,
    commitment: Option<SiteCommitment>,
    journal: NegotiationJournal,
    next_seq: u64,
    // 可观测计数。
    pub replay_rejected: u64,
    pub stale_leader_rejected: u64,
    pub signature_rejected: u64,
    pub tenant_rejected: u64,
    pub expired_count: u64,
}

impl SiteNegotiator {
    pub fn new(site_id: u32, tenant_id: u32, epoch: u64, seq_start: u64) -> Self {
        Self {
            site_id,
            tenant_id,
            epoch,
            keys: KeyRegistry::new(),
            peer_epoch: None,
            last_seq: 0,
            offer: None,
            commitment: None,
            journal: NegotiationJournal::default(),
            next_seq: seq_start,
            replay_rejected: 0,
            stale_leader_rejected: 0,
            signature_rejected: 0,
            tenant_rejected: 0,
            expired_count: 0,
        }
    }

    /// 登记对端（协调器）公钥；返回注册表分配的 key_id（测试便捷路径）。
    pub fn register_peer_key(
        &mut self,
        pk: &Sm2PublicKeyAlias,
        at_ns: u64,
    ) -> Result<u32, KeyError> {
        self.keys.register(pk, at_ns)
    }

    /// 重启恢复密钥注册表（密钥持久化，注册表原样恢复）。
    pub fn restore_keys(&mut self, keys: KeyRegistry) {
        self.keys = keys;
    }

    /// 撤销对端（协调器）key：后续一切以该 key 签名的消息被拒。
    pub fn revoke_peer_key(&mut self, key_id: u32, at_ns: u64) -> Result<(), KeyError> {
        self.keys.revoke(key_id, at_ns)
    }

    pub fn keys(&self) -> &KeyRegistry {
        &self.keys
    }

    /// 以证书派生 key_id 登记对端（协调器）公钥（全局 id 空间）。
    pub fn register_peer_key_with_id(
        &mut self,
        key_id: u32,
        pk: &Sm2PublicKeyAlias,
        at_ns: u64,
    ) -> Result<(), KeyError> {
        self.keys.register_with_id(key_id, pk, at_ns)
    }

    pub fn current_offer(&self) -> Option<&CapabilityOffer> {
        self.offer.as_ref()
    }

    pub fn commitment(&self) -> Option<&SiteCommitment> {
        self.commitment.as_ref()
    }

    pub fn journal(&self) -> &NegotiationJournal {
        &self.journal
    }

    fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// 发布/修订 offer（revision 严格递增；同 offer_id append-only）。
    pub fn publish_offer(
        &mut self,
        offer: &CapabilityOffer,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> Result<(StepOutcome, FedEnvelope), NegotiationError> {
        if let Some(cur) = &self.offer {
            if offer.revision != cur.revision + 1 {
                return Err(NegotiationError::RevisionNotSequential);
            }
            if offer.offer_id != cur.offer_id {
                return Err(NegotiationError::OfferIdChanged);
            }
        } else if offer.revision != 1 {
            return Err(NegotiationError::RevisionNotSequential);
        }
        if offer.valid_until_ns <= now_ns {
            return Err(NegotiationError::OfferAlreadyExpired);
        }
        let hdr = EnvelopeHdr {
            epoch: self.epoch,
            sequence: self.next_seq(),
            tenant_id: self.tenant_id,
            site_id: self.site_id,
            created_ns: now_ns,
            deadline_ns: now_ns + 3_600_000_000_000, // 1h 响应窗口
            signer_key_id: signer.key_id,
        };
        let env = signer.sign(&FedBody::Offer(offer.clone()), &hdr);
        self.offer = Some(offer.clone());
        // 能力变化 → 既有承诺义务维持（本地账本），但旧 pending 语义不再
        // 适用：站内重新封装由下一次 proposal 触发。
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::Out,
            peer: 0,
            kind: FedKind::Offer,
            epoch: self.epoch,
            sequence: hdr.sequence,
            body: FedBody::Offer(offer.clone()),
            verdict: "published",
        });
        Ok((
            StepOutcome::OfferRegistered {
                offer_id: offer.offer_id,
                revision: offer.revision,
            },
            env,
        ))
    }

    /// 撤销 offer（能力变化时；commitment 义务不受影响）。
    pub fn revoke_offer(
        &mut self,
        reason: RejectReason,
        detail: Vec<u8>,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> Result<(StepOutcome, FedEnvelope), NegotiationError> {
        let offer_id = match &self.offer {
            Some(o) => o.offer_id,
            None => return Err(NegotiationError::NoCurrentOffer),
        };
        let hdr = EnvelopeHdr {
            epoch: self.epoch,
            sequence: self.next_seq(),
            tenant_id: self.tenant_id,
            site_id: self.site_id,
            created_ns: now_ns,
            deadline_ns: now_ns + 3_600_000_000_000,
            signer_key_id: signer.key_id,
        };
        let body = FedBody::Revoke(OfferRevocation {
            offer_id,
            reason,
            detail: detail.clone(),
        });
        let env = signer.sign(&body, &hdr);
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::Out,
            peer: 0,
            kind: FedKind::Revoke,
            epoch: self.epoch,
            sequence: hdr.sequence,
            body,
            verdict: "revoked",
        });
        self.offer = None;
        Ok((StepOutcome::OfferRevoked { offer_id }, env))
    }

    /// 收到任何消息的统一入口：先验签、再纪元/重放窗口、再分派。
    pub fn on_message(
        &mut self,
        env: &FedEnvelope,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> (StepOutcome, Vec<FedEnvelope>) {
        // 1) 租户隔离。
        if env.tenant_id != self.tenant_id {
            self.tenant_rejected += 1;
            return (StepOutcome::TenantMismatch, Vec::new());
        }
        // 2) 验签（注册表状态 + 签名）。
        if let Err(_e) = self.keys.verify_envelope(env, now_ns) {
            self.signature_rejected += 1;
            return (StepOutcome::SignatureRejected, Vec::new());
        }
        // 3) 纪元/重放窗口 + 时钟容忍。
        match self.window_check(env, now_ns) {
            WindowVerdict::Ok => {}
            WindowVerdict::Replay(s) => {
                self.replay_rejected += 1;
                return (StepOutcome::ReplayRejected { sequence: s }, Vec::new());
            }
            WindowVerdict::StaleLeader(ep) => {
                self.stale_leader_rejected += 1;
                return (StepOutcome::StaleLeaderRejected { epoch: ep }, Vec::new());
            }
            WindowVerdict::ClockSkew => {
                self.signature_rejected += 1;
                return (StepOutcome::SignatureRejected, Vec::new());
            }
        }
        // 4) 分派（已验签内容）。
        self.on_message_verified(env, now_ns, signer)
    }

    /// 纪元/重放窗口：对端纪元单调；窗口内 seq 严格递增；时钟容忍。
    fn window_check(&mut self, env: &FedEnvelope, now_ns: u64) -> WindowVerdict {
        if now_ns > env.deadline_ns
            || now_ns.saturating_add(CLOCK_SKEW_TOLERANCE_MS * 1_000_000) < env.created_ns
        {
            return WindowVerdict::ClockSkew;
        }
        match self.peer_epoch {
            None => {
                self.peer_epoch = Some(env.epoch);
                self.last_seq = env.sequence;
                WindowVerdict::Ok
            }
            Some(ep) => {
                if env.epoch < ep {
                    return WindowVerdict::StaleLeader(env.epoch);
                }
                if env.epoch == ep {
                    if env.sequence <= self.last_seq {
                        return WindowVerdict::Replay(env.sequence);
                    }
                    self.last_seq = env.sequence;
                } else {
                    // 纪元前跳（协调器故障转移/重启）——接受并重置窗口。
                    self.peer_epoch = Some(env.epoch);
                    self.last_seq = env.sequence;
                }
                WindowVerdict::Ok
            }
        }
    }

    fn on_message_verified(
        &mut self,
        env: &FedEnvelope,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> (StepOutcome, Vec<FedEnvelope>) {
        let body = match crate::offer::decode_body(env.kind, &env.body) {
            Ok(b) => b,
            Err(_) => {
                self.signature_rejected += 1;
                return (StepOutcome::SignatureRejected, Vec::new());
            }
        };
        let mut out = Vec::new();
        let outcome = match body {
            FedBody::Proposal(ref p) => {
                if self.offer.is_none() {
                    StepOutcome::NotInState
                } else if p.site_id != self.site_id {
                    StepOutcome::RejectedByPeer {
                        reason: RejectReason::IdentityMismatch,
                        detail: b"proposal target site mismatch".to_vec(),
                    }
                } else {
                    let offer = self.offer.clone().expect("checked");
                    // 站内接受闸：最新快照包络（站内重解/认证是 svc 在签名
                    // 前完成的步骤——本模块保持纯包络裁决）。
                    match AllocationSolver::check_envelope(
                        &offer,
                        p.interval_start_ns,
                        p.interval_ns,
                        &p.points_kw,
                        p.tolerance_profile_id,
                    ) {
                        EnvelopeCheck::Ok => {
                            let commitment_id = Self::deterministic_id(p.proposal_id, b"CMT");
                            let commitment = AllocationCommitment {
                                commitment_id,
                                offer_id: p.offer_id,
                                coordinator_id: 0,
                                site_id: self.site_id,
                                tolerance_profile_id: p.tolerance_profile_id,
                                interval_start_ns: p.interval_start_ns,
                                interval_ns: p.interval_ns,
                                points_kw: p.points_kw.clone(),
                                target_curve_ref: p.target_curve_ref,
                                accepted_at_ns: now_ns,
                                expires_at_ns: now_ns + 7 * 24 * 3_600_000_000_000,
                                revision: 1,
                            };
                            let hdr = EnvelopeHdr {
                                epoch: self.epoch,
                                sequence: self.next_seq(),
                                tenant_id: self.tenant_id,
                                site_id: self.site_id,
                                created_ns: now_ns,
                                deadline_ns: now_ns + 3_600_000_000_000,
                                signer_key_id: signer.key_id,
                            };
                            let env = signer.sign(&FedBody::Commitment(commitment.clone()), &hdr);
                            self.commitment = Some(SiteCommitment {
                                commitment,
                                acked: false,
                                last_progress_seq: 0,
                            });
                            out.push(env);
                            StepOutcome::ProposalAccepted { commitment_id }
                        }
                        _ => {
                            let reason = if self.offer_is_expired(now_ns) {
                                RejectReason::Expired
                            } else {
                                RejectReason::OutOfEnvelope
                            };
                            let detail = match AllocationSolver::check_envelope(
                                &offer,
                                p.interval_start_ns,
                                p.interval_ns,
                                &p.points_kw,
                                p.tolerance_profile_id,
                            ) {
                                EnvelopeCheck::PowerOutOfRange => {
                                    b"target outside power envelope".to_vec()
                                }
                                EnvelopeCheck::RampOutOfRange => {
                                    b"target outside ramp envelope".to_vec()
                                }
                                EnvelopeCheck::EnergyOutOfRange => {
                                    b"target outside energy envelope".to_vec()
                                }
                                EnvelopeCheck::GridMismatch => {
                                    b"grid/unit mismatch with local offer".to_vec()
                                }
                                EnvelopeCheck::Ok => b"".to_vec(),
                            };
                            let reject = OfferReject {
                                offer_id: p.offer_id,
                                reason,
                                detail: detail.clone(),
                            };
                            let hdr = EnvelopeHdr {
                                epoch: self.epoch,
                                sequence: self.next_seq(),
                                tenant_id: self.tenant_id,
                                site_id: self.site_id,
                                created_ns: now_ns,
                                deadline_ns: now_ns + 3_600_000_000_000,
                                signer_key_id: signer.key_id,
                            };
                            let env = signer.sign(&FedBody::Reject(reject.clone()), &hdr);
                            out.push(env);
                            StepOutcome::ProposalRejected { reason, detail }
                        }
                    }
                }
            }
            FedBody::Ack(ref a) => match &mut self.commitment {
                Some(sc) if sc.commitment.commitment_id == a.commitment_id => {
                    sc.acked = true;
                    StepOutcome::Acknowledged {
                        commitment_id: a.commitment_id,
                    }
                }
                _ => StepOutcome::NotInState,
            },
            FedBody::Reject(ref r) => {
                let _ = r;
                StepOutcome::RejectedByPeer {
                    reason: r.reason,
                    detail: r.detail.clone(),
                }
            }
            FedBody::Offer(_)
            | FedBody::Commitment(_)
            | FedBody::Progress(_)
            | FedBody::Settlement(_)
            | FedBody::Revoke(_) => StepOutcome::NotInState,
        };
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::In,
            peer: 0,
            kind: env.kind,
            epoch: env.epoch,
            sequence: env.sequence,
            body,
            verdict: outcome_verdict(&outcome),
        });
        (outcome, out)
    }

    fn offer_is_expired(&self, now_ns: u64) -> bool {
        match &self.offer {
            Some(o) => now_ns > o.valid_until_ns,
            None => false,
        }
    }

    /// 提交执行后的聚合进度（只上聚合，不上原始点位）。
    pub fn submit_progress(
        &mut self,
        evidence: &ProgressEvidence,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> Result<(StepOutcome, FedEnvelope), NegotiationError> {
        let sc = self
            .commitment
            .as_mut()
            .ok_or(NegotiationError::NotCommitted)?;
        if sc.commitment.commitment_id != evidence.commitment_id {
            return Err(NegotiationError::CommitMismatch);
        }
        if evidence.seq != sc.last_progress_seq + 1 {
            return Err(NegotiationError::ProgressSeqGap);
        }
        sc.last_progress_seq = evidence.seq;
        let hdr = EnvelopeHdr {
            epoch: self.epoch,
            sequence: self.next_seq(),
            tenant_id: self.tenant_id,
            site_id: self.site_id,
            created_ns: now_ns,
            deadline_ns: now_ns + 3_600_000_000_000,
            signer_key_id: signer.key_id,
        };
        let env = signer.sign(&FedBody::Progress(evidence.clone()), &hdr);
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::Out,
            peer: 0,
            kind: FedKind::Progress,
            epoch: self.epoch,
            sequence: hdr.sequence,
            body: FedBody::Progress(evidence.clone()),
            verdict: "progress",
        });
        Ok((
            StepOutcome::EvidenceRecorded {
                kind: EvidenceKind::Progress,
            },
            env,
        ))
    }

    /// 提交结算证据。
    pub fn submit_settlement(
        &mut self,
        evidence: &SettlementEvidence,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> Result<(StepOutcome, FedEnvelope), NegotiationError> {
        let sc = self
            .commitment
            .as_mut()
            .ok_or(NegotiationError::NotCommitted)?;
        if sc.commitment.commitment_id != evidence.commitment_id {
            return Err(NegotiationError::CommitMismatch);
        }
        let hdr = EnvelopeHdr {
            epoch: self.epoch,
            sequence: self.next_seq(),
            tenant_id: self.tenant_id,
            site_id: self.site_id,
            created_ns: now_ns,
            deadline_ns: now_ns + 3_600_000_000_000,
            signer_key_id: signer.key_id,
        };
        let env = signer.sign(&FedBody::Settlement(evidence.clone()), &hdr);
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::Out,
            peer: 0,
            kind: FedKind::Settlement,
            epoch: self.epoch,
            sequence: hdr.sequence,
            body: FedBody::Settlement(evidence.clone()),
            verdict: "settlement",
        });
        Ok((
            StepOutcome::EvidenceRecorded {
                kind: EvidenceKind::Settlement,
            },
            env,
        ))
    }

    /// 崩溃恢复：重放 journal + 恢复密钥注册表（密钥持久化）。
    /// 纪元推进由调用方负责（svc 在持久化纪元上 +1 后传入 [`Self::new`]）。
    pub fn restore(&mut self, journal: &NegotiationJournal, keys: KeyRegistry) {
        self.journal = journal.clone();
        self.keys = keys;
        for r in &journal.records {
            match &r.body {
                FedBody::Offer(o) if r.dir == Dir::Out => {
                    self.offer = Some(o.clone());
                }
                FedBody::Commitment(c) if r.dir == Dir::Out && self.commitment.is_none() => {
                    self.commitment = Some(SiteCommitment {
                        commitment: c.clone(),
                        acked: false,
                        last_progress_seq: 0,
                    });
                }
                FedBody::Revoke(_) if r.dir == Dir::Out => {
                    self.offer = None;
                }
                _ => {}
            }
            // 窗口重建（只计入向消息的对端纪元/seq）。
            if r.dir == Dir::In {
                match self.peer_epoch {
                    None => {
                        self.peer_epoch = Some(r.epoch);
                        self.last_seq = r.sequence;
                    }
                    Some(ep) => {
                        if r.epoch > ep || (r.epoch == ep && r.sequence > self.last_seq) {
                            self.peer_epoch = Some(r.epoch);
                            self.last_seq = r.sequence;
                        }
                    }
                }
            }
            if self.next_seq <= r.sequence && r.dir == Dir::Out {
                self.next_seq = r.sequence;
            }
        }
    }

    /// 到期检查：offer 过期 / commitment 过期。
    pub fn check_expiry(&mut self, now_ns: u64) -> StepOutcome {
        if let Some(o) = &self.offer {
            if now_ns > o.valid_until_ns {
                self.expired_count += 1;
                self.offer = None;
                return StepOutcome::Expired;
            }
        }
        if let Some(sc) = &self.commitment {
            if now_ns > sc.commitment.expires_at_ns {
                self.expired_count += 1;
                return StepOutcome::Expired;
            }
        }
        StepOutcome::Accepted
    }

    /// 确定性 id 派生（commitment_id 由 proposal_id 派生，防伪造引用）。
    fn deterministic_id(seed: [u8; 16], tag: &[u8]) -> [u8; 16] {
        let mut id = [0u8; 16];
        for (i, b) in seed.iter().enumerate() {
            id[i] = b ^ tag[i % tag.len()];
        }
        id
    }
}

/// 会话驱动错误（本地调用侧，非 wire 错误）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegotiationError {
    RevisionNotSequential,
    OfferIdChanged,
    OfferAlreadyExpired,
    NoCurrentOffer,
    NotCommitted,
    CommitMismatch,
    ProgressSeqGap,
    SessionsFull,
}

enum WindowVerdict {
    Ok,
    Replay(u64),
    StaleLeader(u64),
    ClockSkew,
}

fn outcome_verdict(o: &StepOutcome) -> &'static str {
    match o {
        StepOutcome::Accepted => "accepted",
        StepOutcome::OfferRegistered { .. } => "offer-registered",
        StepOutcome::Superseded { .. } => "superseded",
        StepOutcome::OfferRevoked { .. } => "offer-revoked",
        StepOutcome::ProposalAccepted { .. } => "accepted",
        StepOutcome::ProposalRejected { .. } => "rejected",
        StepOutcome::CommitmentConfirmed { .. } => "commitment-confirmed",
        StepOutcome::Acknowledged { .. } => "acknowledged",
        StepOutcome::RejectedByPeer { .. } => "peer-rejected",
        StepOutcome::EvidenceRecorded { .. } => "evidence",
        StepOutcome::Expired => "expired",
        StepOutcome::ReplayRejected { .. } => "replay-rejected",
        StepOutcome::StaleLeaderRejected { .. } => "stale-leader-rejected",
        StepOutcome::SignatureRejected => "signature-rejected",
        StepOutcome::TenantMismatch => "tenant-mismatch",
        StepOutcome::NotInState => "not-in-state",
    }
}

/// 协调器侧单站点会话。
pub struct CoordinatorSession {
    pub site_id: u32,
    offer: Option<CapabilityOffer>,
    pending: Option<AllocationProposal>,
    commitment: Option<AllocationCommitment>,
    pub evidence_progress: Vec<ProgressEvidence>,
    pub evidence_settlement: Vec<SettlementEvidence>,
    peer_epoch: Option<u64>,
    last_seq: u64,
    pub replay_rejected: u64,
    pub stale_leader_rejected: u64,
}

impl CoordinatorSession {
    fn new(site_id: u32) -> Self {
        Self {
            site_id,
            offer: None,
            pending: None,
            commitment: None,
            evidence_progress: Vec::new(),
            evidence_settlement: Vec::new(),
            peer_epoch: None,
            last_seq: 0,
            replay_rejected: 0,
            stale_leader_rejected: 0,
        }
    }

    pub fn offer(&self) -> Option<&CapabilityOffer> {
        self.offer.as_ref()
    }
    pub fn commitment(&self) -> Option<&AllocationCommitment> {
        self.commitment.as_ref()
    }
    pub fn pending_proposal(&self) -> Option<&AllocationProposal> {
        self.pending.as_ref()
    }
}

/// 协调器协商会话（每个站点一个 [`CoordinatorSession`]）。
pub struct CoordinatorNegotiator {
    pub coordinator_id: u32,
    pub tenant_id: u32,
    pub epoch: u64,
    keys: KeyRegistry,
    sessions: BTreeMap<u32, CoordinatorSession>,
    journal: NegotiationJournal,
    next_seq: u64,
    pub replay_rejected: u64,
    pub stale_leader_rejected: u64,
    pub signature_rejected: u64,
    pub tenant_rejected: u64,
}

impl CoordinatorNegotiator {
    pub fn new(coordinator_id: u32, tenant_id: u32, epoch: u64, seq_start: u64) -> Self {
        Self {
            coordinator_id,
            tenant_id,
            epoch,
            keys: KeyRegistry::new(),
            sessions: BTreeMap::new(),
            journal: NegotiationJournal::default(),
            next_seq: seq_start,
            replay_rejected: 0,
            stale_leader_rejected: 0,
            signature_rejected: 0,
            tenant_rejected: 0,
        }
    }

    /// 登记站点公钥（key_id 来自站点证书，全局唯一）。
    pub fn register_site_key(
        &mut self,
        key_id: u32,
        pk: &Sm2PublicKeyAlias,
        at_ns: u64,
    ) -> Result<(), KeyError> {
        self.keys.register_with_id(key_id, pk, at_ns)
    }

    pub fn keys(&self) -> &KeyRegistry {
        &self.keys
    }

    /// 撤销站点 key（证书撤销路径）：该站点后续消息被拒（不影响其他站点）。
    pub fn revoke_site_key(&mut self, key_id: u32, at_ns: u64) -> Result<(), KeyError> {
        self.keys.revoke(key_id, at_ns)
    }

    pub fn session(&self, site_id: u32) -> Option<&CoordinatorSession> {
        self.sessions.get(&site_id)
    }

    pub fn sessions(&self) -> &BTreeMap<u32, CoordinatorSession> {
        &self.sessions
    }

    pub fn journal(&self) -> &NegotiationJournal {
        &self.journal
    }

    fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// 收到站点消息（租户 → 验签 → 窗口 → 分派）。
    pub fn on_message(
        &mut self,
        env: &FedEnvelope,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> (StepOutcome, Vec<FedEnvelope>) {
        if env.tenant_id != self.tenant_id {
            self.tenant_rejected += 1;
            return (StepOutcome::TenantMismatch, Vec::new());
        }
        if let Err(_e) = self.keys.verify_envelope(env, now_ns) {
            self.signature_rejected += 1;
            return (StepOutcome::SignatureRejected, Vec::new());
        }
        // 会话上限（防未知站点 DoS 撑爆内存）。
        if !self.sessions.contains_key(&env.site_id) && self.sessions.len() >= MAX_SESSIONS {
            return (StepOutcome::NotInState, Vec::new());
        }
        let session = self
            .sessions
            .entry(env.site_id)
            .or_insert_with(|| CoordinatorSession::new(env.site_id));
        // 窗口（按站点独立：纪元前跳接受并重置）。
        match session.peer_epoch {
            None => {
                session.peer_epoch = Some(env.epoch);
                session.last_seq = env.sequence;
            }
            Some(ep) => {
                if env.epoch < ep {
                    session.stale_leader_rejected += 1;
                    self.stale_leader_rejected += 1;
                    return (
                        StepOutcome::StaleLeaderRejected { epoch: env.epoch },
                        Vec::new(),
                    );
                }
                if env.epoch == ep {
                    if env.sequence <= session.last_seq {
                        session.replay_rejected += 1;
                        self.replay_rejected += 1;
                        return (
                            StepOutcome::ReplayRejected {
                                sequence: env.sequence,
                            },
                            Vec::new(),
                        );
                    }
                    session.last_seq = env.sequence;
                } else {
                    session.peer_epoch = Some(env.epoch);
                    session.last_seq = env.sequence;
                }
            }
        }
        let body = match crate::offer::decode_body(env.kind, &env.body) {
            Ok(b) => b,
            Err(_) => {
                self.signature_rejected += 1;
                return (StepOutcome::SignatureRejected, Vec::new());
            }
        };
        let mut out = Vec::new();
        let outcome = match &body {
            FedBody::Offer(o) => {
                if o.valid_until_ns <= now_ns {
                    StepOutcome::RejectedByPeer {
                        reason: RejectReason::Expired,
                        detail: b"offer already expired".to_vec(),
                    }
                } else {
                    let cur = session.offer.clone();
                    match &cur {
                        Some(c) if c.revision >= o.revision => StepOutcome::Superseded {
                            current_revision: c.revision,
                        },
                        _ => {
                            if o.revision != cur.as_ref().map(|c| c.revision + 1).unwrap_or(1) {
                                StepOutcome::RejectedByPeer {
                                    reason: RejectReason::Superseded,
                                    detail: b"revision gap".to_vec(),
                                }
                            } else {
                                session.offer = Some(o.clone());
                                session.pending = None; // 新能力 → 旧提议作废
                                StepOutcome::OfferRegistered {
                                    offer_id: o.offer_id,
                                    revision: o.revision,
                                }
                            }
                        }
                    }
                }
            }
            FedBody::Revoke(rv) => {
                if let Some(o) = &session.offer {
                    if o.offer_id == rv.offer_id {
                        session.offer = None;
                        session.pending = None;
                        StepOutcome::OfferRevoked {
                            offer_id: rv.offer_id,
                        }
                    } else {
                        StepOutcome::NotInState
                    }
                } else {
                    StepOutcome::NotInState
                }
            }
            FedBody::Commitment(c) => {
                let matched = match (&session.offer, &session.pending) {
                    (Some(o), Some(p)) => {
                        c.offer_id == o.offer_id
                            && c.tolerance_profile_id == p.tolerance_profile_id
                            && c.target_curve_ref == p.target_curve_ref
                            && c.expires_at_ns > c.accepted_at_ns
                            // 跨节点时钟偏斜容忍：未来 ±skew 内视为有效。
                            && c.accepted_at_ns
                                <= now_ns.saturating_add(CLOCK_SKEW_TOLERANCE_MS * 1_000_000)
                    }
                    _ => false,
                };
                if matched && session.commitment.is_none() {
                    session.commitment = Some(c.clone());
                    let ack = CommitmentAck {
                        commitment_id: c.commitment_id,
                        acked_at_ns: now_ns,
                    };
                    let hdr = EnvelopeHdr {
                        epoch: self.epoch,
                        sequence: self.next_seq(),
                        tenant_id: self.tenant_id,
                        site_id: self.coordinator_id,
                        created_ns: now_ns,
                        deadline_ns: now_ns + 3_600_000_000_000,
                        signer_key_id: signer.key_id,
                    };
                    out.push(signer.sign(&FedBody::Ack(ack), &hdr));
                    StepOutcome::CommitmentConfirmed {
                        commitment_id: c.commitment_id,
                    }
                } else {
                    StepOutcome::NotInState
                }
            }
            FedBody::Reject(r) => StepOutcome::RejectedByPeer {
                reason: r.reason,
                detail: r.detail.clone(),
            },
            FedBody::Progress(p) => {
                if is_active_commitment(session, p.commitment_id) {
                    session.evidence_progress.push(p.clone());
                    StepOutcome::EvidenceRecorded {
                        kind: EvidenceKind::Progress,
                    }
                } else {
                    StepOutcome::NotInState
                }
            }
            FedBody::Settlement(s) => {
                if is_active_commitment(session, s.commitment_id) {
                    session.evidence_settlement.push(s.clone());
                    StepOutcome::EvidenceRecorded {
                        kind: EvidenceKind::Settlement,
                    }
                } else {
                    StepOutcome::NotInState
                }
            }
            FedBody::Proposal(_) | FedBody::Ack(_) => StepOutcome::NotInState,
        };
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::In,
            peer: env.site_id,
            kind: env.kind,
            epoch: env.epoch,
            sequence: env.sequence,
            body,
            verdict: outcome_verdict(&outcome),
        });
        (outcome, out)
    }

    /// 对某站点发起分配提议（调用方已用 AllocationSolver 求得目标）。
    pub fn propose(
        &mut self,
        site_id: u32,
        proposal: &AllocationProposal,
        now_ns: u64,
        signer: &mut SiteSigner,
    ) -> Result<(StepOutcome, FedEnvelope), NegotiationError> {
        if !self.sessions.contains_key(&site_id) {
            if self.sessions.len() >= MAX_SESSIONS {
                return Err(NegotiationError::SessionsFull);
            }
            self.sessions
                .insert(site_id, CoordinatorSession::new(site_id));
        }
        let session = self.sessions.get_mut(&site_id).expect("inserted");
        if session.offer.is_none() {
            return Err(NegotiationError::NoCurrentOffer);
        }
        session.pending = Some(proposal.clone());
        let hdr = EnvelopeHdr {
            epoch: self.epoch,
            sequence: self.next_seq(),
            tenant_id: self.tenant_id,
            site_id: self.coordinator_id,
            created_ns: now_ns,
            deadline_ns: now_ns + 3_600_000_000_000,
            signer_key_id: signer.key_id,
        };
        let env = signer.sign(&FedBody::Proposal(proposal.clone()), &hdr);
        self.journal.append(JournalRecord {
            at_ns: now_ns,
            dir: Dir::Out,
            peer: site_id,
            kind: FedKind::Proposal,
            epoch: self.epoch,
            sequence: hdr.sequence,
            body: FedBody::Proposal(proposal.clone()),
            verdict: "proposed",
        });
        Ok((StepOutcome::Accepted, env))
    }

    /// 崩溃恢复：重放 journal（入库时已验签，restore 只重建状态）+ 恢复
    /// 密钥注册表。纪元推进由调用方负责（svc 在持久化纪元上 +1 后传入
    /// [`Self::new`]）；随后本站出的消息按新纪元签名，旧纪元消息被对端拒绝。
    pub fn restore(&mut self, journal: &NegotiationJournal, keys: KeyRegistry) {
        self.journal = journal.clone();
        self.keys = keys;
        for r in &journal.records {
            // 状态只从入向记录重建（出向记录只用于 pending 提议恢复）。
            if r.dir == Dir::Out {
                if let FedBody::Proposal(p) = &r.body {
                    let session = self
                        .sessions
                        .entry(r.peer)
                        .or_insert_with(|| CoordinatorSession::new(r.peer));
                    if session.offer.is_some() {
                        session.pending = Some(p.clone());
                    }
                }
                continue;
            }
            let site_id = r.peer;
            let session = self
                .sessions
                .entry(site_id)
                .or_insert_with(|| CoordinatorSession::new(site_id));
            match &r.body {
                FedBody::Offer(o) => {
                    let cur = session.offer.clone();
                    let next = cur.as_ref().map(|c| c.revision + 1).unwrap_or(1);
                    if o.revision == next {
                        session.offer = Some(o.clone());
                        session.pending = None;
                    }
                }
                FedBody::Revoke(rv) => {
                    if let Some(o) = &session.offer {
                        if o.offer_id == rv.offer_id {
                            session.offer = None;
                            session.pending = None;
                        }
                    }
                }
                FedBody::Commitment(c) if session.commitment.is_none() => {
                    session.commitment = Some(c.clone());
                }
                FedBody::Progress(p) => session.evidence_progress.push(p.clone()),
                FedBody::Settlement(s) => session.evidence_settlement.push(s.clone()),
                _ => {}
            }
            // 窗口重建（对端最后 seq/epoch，只计入向）。
            match session.peer_epoch {
                None => {
                    session.peer_epoch = Some(r.epoch);
                    session.last_seq = r.sequence;
                }
                Some(ep) => {
                    if r.epoch > ep || (r.epoch == ep && r.sequence > session.last_seq) {
                        session.peer_epoch = Some(r.epoch);
                        session.last_seq = r.sequence;
                    }
                }
            }
        }
    }
}

fn is_active_commitment(session: &CoordinatorSession, commitment_id: [u8; 16]) -> bool {
    matches!(&session.commitment, Some(c) if c.commitment_id == commitment_id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use eneros_crypto::sm2::Sm2KeyPair;

    use super::*;
    use crate::allocation::SiteAllocation;
    use crate::offer::{BlobRefV1, CapabilityOffer};

    fn kp() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        (Sm2KeyPair::generate(&mut rng).expect("kp"), rng)
    }

    fn offer_rev(id: u8, revision: u64) -> CapabilityOffer {
        let mut o = offer(id);
        o.revision = revision;
        o
    }

    fn offer(id: u8) -> CapabilityOffer {
        CapabilityOffer {
            offer_id: [id; 16],
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            p_min_kw: 0.0,
            p_max_kw: 100.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: 3_600.0,
            ramp_kw_s: 10.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            price_curve_ref: BlobRefV1::new(1, b"p"),
            capability_root: [0xAA; 32],
            valid_until_ns: u64::MAX,
            revision: 1,
        }
    }

    const NOW: u64 = 1_000_000;
    /// 站点证书 key_id（生产从证书派生；测试手写固定）。
    const SITE_KEY_ID: u32 = 101;
    const COORD_KEY_ID: u32 = 202;

    /// 构造协商对端（站点 9 ↔ 协调器 0）：key_id 全局身份，
    /// 收发双方登记同一 id。
    fn pair() -> (
        SiteNegotiator,
        SiteSigner,
        CoordinatorNegotiator,
        SiteSigner,
    ) {
        let (kp_site, rng_site) = kp();
        let (kp_coord, rng_coord) = kp();
        let mut coord = CoordinatorNegotiator::new(0, 1, 1, 1);
        coord
            .register_site_key(SITE_KEY_ID, &kp_site.public_key, NOW)
            .expect("coord regist site key");
        let mut site = SiteNegotiator::new(9, 1, 1, 100);
        site.register_peer_key_with_id(COORD_KEY_ID, &kp_coord.public_key, NOW)
            .expect("site regist coord key");
        let site_signer = SiteSigner::new(SITE_KEY_ID, kp_site, rng_site);
        let coord_signer = SiteSigner::new(COORD_KEY_ID, kp_coord, rng_coord);
        (site, site_signer, coord, coord_signer)
    }

    #[test]
    fn full_flow_offer_propose_commit_ack() {
        let (mut site, mut site_signer, mut coord, mut coord_signer) = pair();
        // 站点发布 offer。
        let (out, env) = site
            .publish_offer(&offer(7), NOW, &mut site_signer)
            .expect("publish");
        assert!(matches!(
            out,
            StepOutcome::OfferRegistered { revision: 1, .. }
        ));
        // 协调器注册。
        let (out, _) = coord.on_message(&env, NOW + 100, &mut coord_signer);
        assert!(matches!(
            out,
            StepOutcome::OfferRegistered { revision: 1, .. }
        ));
        // 协调器求解 + 提议。
        let alloc = SiteAllocation {
            site_id: 9,
            points_kw: vec![50.0],
            energy_kwh: 50.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            within_envelope: true,
        };
        let proposal =
            AllocationSolver::to_proposal(&alloc, &offer(7), [0x11; 16], 1, 0, 3_600_000_000_000)
                .expect("proposal");
        let (_, env) = coord
            .propose(9, &proposal, NOW + 200, &mut coord_signer)
            .expect("propose");
        // 站点接受 → commitment 出站。
        let (out, outbox) = site.on_message(&env, NOW + 300, &mut site_signer);
        assert!(
            matches!(out, StepOutcome::ProposalAccepted { .. }),
            "{out:?}"
        );
        assert_eq!(outbox.len(), 1);
        // 协调器确认 → Ack 出站。
        let (out, ackbox) = coord.on_message(&outbox[0], NOW + 400, &mut coord_signer);
        assert!(
            matches!(out, StepOutcome::CommitmentConfirmed { .. }),
            "{out:?}"
        );
        assert_eq!(ackbox.len(), 1);
        // 站点收到 Ack。
        let (out, _) = site.on_message(&ackbox[0], NOW + 500, &mut site_signer);
        assert!(matches!(out, StepOutcome::Acknowledged { .. }), "{out:?}");
        // 站点提交进度证据。
        let cid = site.commitment().expect("cmt").commitment.commitment_id;
        let prog = ProgressEvidence {
            commitment_id: cid,
            site_id: 9,
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            delivered_kwh: 50.0,
            deviation_kw: 0.0,
            evidence_ref: BlobRefV1::new(3, b"ev"),
            seq: 1,
        };
        let (out, env) = site
            .submit_progress(&prog, NOW + 600, &mut site_signer)
            .expect("prog");
        assert!(matches!(
            out,
            StepOutcome::EvidenceRecorded {
                kind: EvidenceKind::Progress
            }
        ));
        let (out, _) = coord.on_message(&env, NOW + 700, &mut coord_signer);
        assert!(matches!(
            out,
            StepOutcome::EvidenceRecorded {
                kind: EvidenceKind::Progress
            }
        ));
        assert_eq!(coord.session(9).expect("s9").evidence_progress.len(), 1);
        // 站点提交结算。
        let set = SettlementEvidence {
            commitment_id: cid,
            site_id: 9,
            settle_start_ns: 0,
            settle_end_ns: 3_600_000_000_000,
            measured_milli_kwh: 50_000,
            quality_bp: 10_000,
            note: b"ok".to_vec(),
        };
        let (_, env) = site
            .submit_settlement(&set, NOW + 800, &mut site_signer)
            .expect("set");
        let (out, _) = coord.on_message(&env, NOW + 900, &mut coord_signer);
        assert!(matches!(
            out,
            StepOutcome::EvidenceRecorded {
                kind: EvidenceKind::Settlement
            }
        ));
        assert_eq!(coord.session(9).expect("s9").evidence_settlement.len(), 1);
    }

    #[test]
    fn out_of_envelope_proposal_rejected_structured() {
        let (mut site, mut site_signer, mut coord, mut coord_signer) = pair();
        let (_, env) = site
            .publish_offer(&offer(7), NOW, &mut site_signer)
            .expect("publish");
        let _ = coord.on_message(&env, NOW + 100, &mut coord_signer);
        // 提议 150 kW > p_max 100 → 结构化拒绝。
        let alloc = SiteAllocation {
            site_id: 9,
            points_kw: vec![150.0],
            energy_kwh: 150.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            within_envelope: true,
        };
        let proposal =
            AllocationSolver::to_proposal(&alloc, &offer(7), [0x22; 16], 1, 0, 3_600_000_000_000)
                .expect("proposal");
        let (_, env) = coord
            .propose(9, &proposal, NOW + 200, &mut coord_signer)
            .expect("propose");
        let (out, outbox) = site.on_message(&env, NOW + 300, &mut site_signer);
        match out {
            StepOutcome::ProposalRejected {
                reason: RejectReason::OutOfEnvelope,
                ..
            } => {}
            other => panic!("expected structured rejection, got {other:?}"),
        }
        assert_eq!(outbox.len(), 1);
        // 协调器收到拒绝。
        let (out, _) = coord.on_message(&outbox[0], NOW + 400, &mut coord_signer);
        assert!(matches!(out, StepOutcome::RejectedByPeer { .. }));
        // 站点仍可接收新提议（拒绝路径不污染会话）。
        let alloc2 = SiteAllocation {
            site_id: 9,
            points_kw: vec![30.0],
            energy_kwh: 30.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            within_envelope: true,
        };
        let proposal2 =
            AllocationSolver::to_proposal(&alloc2, &offer(7), [0x33; 16], 1, 0, 3_600_000_000_000)
                .expect("proposal2");
        let (_, env2) = coord
            .propose(9, &proposal2, NOW + 500, &mut coord_signer)
            .expect("propose2");
        let (out, _) = site.on_message(&env2, NOW + 600, &mut site_signer);
        assert!(
            matches!(out, StepOutcome::ProposalAccepted { .. }),
            "{out:?}"
        );
    }

    #[test]
    fn replay_and_stale_leader_rejected() {
        let (mut site, mut site_signer, mut coord, mut coord_signer) = pair();
        let (_, env) = site
            .publish_offer(&offer(7), NOW, &mut site_signer)
            .expect("publish");
        let (_, _) = coord.on_message(&env, NOW + 100, &mut coord_signer);
        // 重放同一 offer（同 seq）→ 协调器拒。
        let (out, _) = coord.on_message(&env, NOW + 200, &mut coord_signer);
        assert!(matches!(out, StepOutcome::ReplayRejected { .. }), "{out:?}");
        assert_eq!(coord.replay_rejected, 1);
        // 站点先收一条合法消息（epoch 1）建立窗口。
        let alloc = SiteAllocation {
            site_id: 9,
            points_kw: vec![10.0],
            energy_kwh: 10.0,
            reserve_up_kw: 1.0,
            reserve_down_kw: 1.0,
            within_envelope: true,
        };
        let proposal =
            AllocationSolver::to_proposal(&alloc, &offer(7), [0x44; 16], 1, 0, 3_600_000_000_000)
                .expect("p");
        let hdr = EnvelopeHdr {
            epoch: 1,
            sequence: 5,
            tenant_id: 1,
            site_id: 0,
            created_ns: NOW,
            deadline_ns: NOW + 3_600_000_000_000,
            signer_key_id: COORD_KEY_ID,
        };
        let env_ok = coord_signer.sign(&FedBody::Proposal(proposal.clone()), &hdr);
        let (out, _) = site.on_message(&env_ok, NOW + 300, &mut site_signer);
        assert!(
            matches!(out, StepOutcome::ProposalAccepted { .. }),
            "{out:?}"
        );
        // 陈旧纪元（epoch 0）→ 站点拒。
        let hdr_stale = EnvelopeHdr {
            epoch: 0,
            sequence: 9,
            tenant_id: 1,
            site_id: 0,
            created_ns: NOW,
            deadline_ns: NOW + 3_600_000_000_000,
            signer_key_id: COORD_KEY_ID,
        };
        let env_stale = coord_signer.sign(&FedBody::Proposal(proposal), &hdr_stale);
        let (out, _) = site.on_message(&env_stale, NOW + 400, &mut site_signer);
        assert!(
            matches!(out, StepOutcome::StaleLeaderRejected { .. }),
            "{out:?}"
        );
        assert_eq!(site.stale_leader_rejected, 1);
    }

    #[test]
    fn revision_and_revoke() {
        let (mut site, mut site_signer, mut coord, mut coord_signer) = pair();
        let (_, env) = site
            .publish_offer(&offer(7), NOW, &mut site_signer)
            .expect("publish");
        let _ = coord.on_message(&env, NOW + 100, &mut coord_signer);
        // 修订 rev2。
        let mut o2 = offer(7);
        o2.revision = 2;
        let (out, env2) = site
            .publish_offer(&o2, NOW + 200, &mut site_signer)
            .expect("rev2");
        assert!(matches!(
            out,
            StepOutcome::OfferRegistered { revision: 2, .. }
        ));
        let (out, _) = coord.on_message(&env2, NOW + 300, &mut coord_signer);
        assert!(matches!(
            out,
            StepOutcome::OfferRegistered { revision: 2, .. }
        ));
        // 同一信封重发 → ReplayRejected（窗口语义，不是 Superseded）。
        let (out, _) = coord.on_message(&env2, NOW + 400, &mut coord_signer);
        assert!(matches!(out, StepOutcome::ReplayRejected { .. }), "{out:?}");
        // 再修订 rev3；随后网络迟到一封"新 seq 但旧 revision=2"的信封
        // （乱序/双写路径）→ Superseded。
        let mut o3 = offer(7);
        o3.revision = 3;
        let (_, env3) = site
            .publish_offer(&o3, NOW + 450, &mut site_signer)
            .expect("rev3");
        let (out, _) = coord.on_message(&env3, NOW + 470, &mut coord_signer);
        assert!(matches!(
            out,
            StepOutcome::OfferRegistered { revision: 3, .. }
        ));
        let env_stale_rev = site_signer.sign(
            &FedBody::Offer(offer_rev(7, 2)),
            &EnvelopeHdr {
                epoch: 1,
                sequence: 500,
                tenant_id: 1,
                site_id: 9,
                created_ns: NOW + 460,
                deadline_ns: NOW + 3_600_000_000_000,
                signer_key_id: SITE_KEY_ID,
            },
        );
        let (out, _) = coord.on_message(&env_stale_rev, NOW + 480, &mut coord_signer);
        assert!(
            matches!(
                out,
                StepOutcome::Superseded {
                    current_revision: 3
                }
            ),
            "{out:?}"
        );
        // 撤销（窗口已被伪造信封推到 500；用站点密钥手构 seq=501 的撤销
        // 信封走协调器撤销路径；站点侧 revoke_offer API 由集成测试覆盖）。
        let revoke_env = site_signer.sign(
            &FedBody::Revoke(OfferRevocation {
                offer_id: [7; 16],
                reason: RejectReason::Other,
                detail: b"maintenance".to_vec(),
            }),
            &EnvelopeHdr {
                epoch: 1,
                sequence: 501,
                tenant_id: 1,
                site_id: 9,
                created_ns: NOW + 500,
                deadline_ns: NOW + 3_600_000_000_000,
                signer_key_id: SITE_KEY_ID,
            },
        );
        let (out, _) = coord.on_message(&revoke_env, NOW + 600, &mut coord_signer);
        assert!(matches!(out, StepOutcome::OfferRevoked { .. }), "{out:?}");
        assert!(coord.session(9).expect("s").offer().is_none());
    }

    #[test]
    fn coordinator_restart_restores_sessions() {
        let (mut site, mut site_signer, mut coord, mut coord_signer) = pair();
        let (_, env) = site
            .publish_offer(&offer(7), NOW, &mut site_signer)
            .expect("publish");
        let _ = coord.on_message(&env, NOW + 100, &mut coord_signer);
        let alloc = SiteAllocation {
            site_id: 9,
            points_kw: vec![40.0],
            energy_kwh: 40.0,
            reserve_up_kw: 5.0,
            reserve_down_kw: 5.0,
            within_envelope: true,
        };
        let proposal =
            AllocationSolver::to_proposal(&alloc, &offer(7), [0x55; 16], 1, 0, 3_600_000_000_000)
                .expect("p");
        let (_, penv) = coord
            .propose(9, &proposal, NOW + 200, &mut coord_signer)
            .expect("propose");
        let (out, cbox) = site.on_message(&penv, NOW + 300, &mut site_signer);
        assert!(matches!(out, StepOutcome::ProposalAccepted { .. }));
        // 协调器崩溃：journal + 注册表持久化后重建，纪元 +1。
        let journal = coord.journal().clone();
        let keys = coord.keys().clone();
        let mut coord2 = CoordinatorNegotiator::new(0, 1, 2, 1_000); // 新纪元
        coord2.restore(&journal, keys);
        assert!(
            coord2.session(9).expect("s").offer().is_some(),
            "offer 恢复"
        );
        assert!(
            coord2.session(9).expect("s").pending_proposal().is_some(),
            "pending 恢复"
        );
        // 迟到的 commitment 在新协调器上正确确认（不重复签发）。
        let (out, ackbox) = coord2.on_message(&cbox[0], NOW + 400, &mut coord_signer);
        assert!(
            matches!(out, StepOutcome::CommitmentConfirmed { .. }),
            "{out:?}"
        );
        assert_eq!(ackbox.len(), 1);
        assert!(coord2.session(9).expect("s").commitment().is_some());
        // 站点收到新纪元 Ack（纪元前跳被接受）。
        let (out, _) = site.on_message(&ackbox[0], NOW + 500, &mut site_signer);
        assert!(matches!(out, StepOutcome::Acknowledged { .. }), "{out:?}");
    }
}
