//! svc-federation — 联邦服务（蓝图 61 §12 联邦能力，host 侧）。
//!
//! 角色：
//!
//! - `coord`：区域协调器。mTLS 监听；站点 offer → [`AllocationSolver`]
//!   分层分配（只分站点目标/备用）→ 逐站 propose → commitment 确认 →
//!   Ack → 归档 progress/settlement 证据。事件 append-only journal 落盘，
//!   崩溃后 restore 重建会话（epoch+1，陈旧 leader 消息被窗口拒绝）。
//! - `site`：站点联邦客户端。读本地能力包络（站内 Snapshot/Registry 的
//!   保守聚合）→ 发布签名 offer → proposal 就地包络校验（恶意超范围
//!   目标结构化拒绝，站内继续自治）→ 签 commitment → 上传聚合证据。
//!
//! 能力纪律：本二进制**无** ControlGateway/AuthorizedWriter/设备 writer
//! 能力——联邦只交换能力、承诺与证据；站内安全与授权权威不外移。
//!
//! 用法：
//!   svc-federation coord --listen 127.0.0.1:9443 --ca ca.pem --cert c.pem --key c.key \
//!       --node-id 0 --tenant 1 --journal-dir ./fedj --targets targets.txt \
//!       --voters 3 --auto-failover yes --sm2-key-hex <64hex> --sm2-key-id 1 --site-keys keys.txt
//!   svc-federation site --coord 127.0.0.1:9443 --ca ca.pem --cert s.pem --key s.key \
//!       --node-id 42 --tenant 1 --journal-dir ./fedj --envelope env.txt \
//!       --offer-id <32hex> --price-digest <64hex> --capability-root <64hex> \
//!       --sm2-key-hex <64hex> --sm2-key-id 11 --peer-key-id 1 --peer-key-hex <130hex> --run-once

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{Sm2KeyPair, Sm2PrivateKey, Sm2PublicKey};
use eneros_federation::allocation::{
    AllocationInput, AllocationResult, AllocationSolver, SiteOfferView, TargetProfile,
};
use eneros_federation::fedxport::{recv_frame, send_frame};
use eneros_federation::keys::{KeyRegistry, KeyStatus};
use eneros_federation::leadership::{EpochRegulator, LeadPolicy};
use eneros_federation::negotiation::{
    CoordinatorNegotiator, Dir, JournalRecord, NegotiationJournal, SiteNegotiator, SiteSigner,
    StepOutcome,
};
use eneros_federation::offer::{
    decode_body, BlobRefV1, CapabilityOffer, FedEnvelope, FedKind, ProgressEvidence,
    SettlementEvidence,
};
use eneros_tls::{TlsAcceptor, TlsClientConfig, TlsConnector, TlsServerConfig};

const USAGE: &str = "usage 见模块文档（svc-federation.rs 头部）";
/// 协调器每连接会话的最大迭代（防失控循环）。
const MAX_PEER_ITER: u32 = 4096;

// ---------------------------------------------------------------
// 参数解析（零额外依赖）
// ---------------------------------------------------------------

#[derive(Clone, Debug)]
struct Args {
    mode: String,
    listen: Option<SocketAddr>,
    coord: Option<SocketAddr>,
    ca: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    node_id: u32,
    tenant: u32,
    epoch: u64,
    journal_dir: PathBuf,
    targets: Option<PathBuf>,
    envelope: Option<PathBuf>,
    site_keys: Option<PathBuf>,
    offer_id: [u8; 16],
    price_schema: u8,
    price_digest: [u8; 32],
    capability_root: [u8; 32],
    sm2_key_hex: String,
    sm2_key_id: u32,
    peer_key_id: u32,
    peer_key_hex: String,
    voters: u32,
    auto_failover: bool,
    run_once: bool,
}

fn hex_decode(s: &str) -> Vec<u8> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        eprintln!("bad hex: {s}");
        exit(2);
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16).expect("hi");
        let lo = (b[i + 1] as char).to_digit(16).expect("lo");
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    out
}

fn hex32(s: &str, what: &str) -> [u8; 32] {
    let b = hex_decode(s);
    assert_eq!(b.len(), 32, "{what} 需要 32 字节 hex");
    b.try_into().expect("32")
}

fn hex16(s: &str, what: &str) -> [u8; 16] {
    let b = hex_decode(s);
    assert_eq!(b.len(), 16, "{what} 需要 16 字节 hex");
    b.try_into().expect("16")
}

fn parse_args() -> Args {
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        mode: String::new(),
        listen: None,
        coord: None,
        ca: PathBuf::new(),
        cert: PathBuf::new(),
        key: PathBuf::new(),
        node_id: 0,
        tenant: 1,
        epoch: 1,
        journal_dir: PathBuf::from("fed-journal"),
        targets: None,
        envelope: None,
        site_keys: None,
        offer_id: [0x11; 16],
        price_schema: 1,
        price_digest: [0u8; 32],
        capability_root: [0u8; 32],
        sm2_key_hex: String::new(),
        sm2_key_id: 0,
        peer_key_id: 0,
        peer_key_hex: String::new(),
        voters: 0,
        auto_failover: false,
        run_once: false,
    };
    let mut i = 0;
    while i < v.len() {
        let k = v[i].clone();
        let mut next = || {
            i += 1;
            v.get(i).cloned().unwrap_or_else(|| {
                eprintln!("missing value for {k}");
                exit(2);
            })
        };
        match k.as_str() {
            "coord" | "site" | "dump-pubkey" | "revoke" => a.mode = k,
            "--listen" => a.listen = Some(next().parse().expect("listen addr")),
            "--coord" => a.coord = Some(next().parse().expect("coord addr")),
            "--ca" => a.ca = PathBuf::from(next()),
            "--cert" => a.cert = PathBuf::from(next()),
            "--key" => a.key = PathBuf::from(next()),
            "--node-id" => a.node_id = next().parse().expect("node-id u32"),
            "--tenant" => a.tenant = next().parse().expect("tenant u32"),
            "--epoch" => a.epoch = next().parse().expect("epoch u64"),
            "--journal-dir" => a.journal_dir = PathBuf::from(next()),
            "--targets" => a.targets = Some(PathBuf::from(next())),
            "--envelope" => a.envelope = Some(PathBuf::from(next())),
            "--site-keys" => a.site_keys = Some(PathBuf::from(next())),
            "--offer-id" => a.offer_id = hex16(&next(), "offer-id"),
            "--price-schema" => a.price_schema = next().parse().expect("price-schema u8"),
            "--price-digest" => a.price_digest = hex32(&next(), "price-digest"),
            "--capability-root" => a.capability_root = hex32(&next(), "capability-root"),
            "--sm2-key-hex" => a.sm2_key_hex = next(),
            "--sm2-key-id" => a.sm2_key_id = next().parse().expect("sm2-key-id u32"),
            "--peer-key-id" => a.peer_key_id = next().parse().expect("peer-key-id u32"),
            "--peer-key-hex" => a.peer_key_hex = next(),
            "--voters" => a.voters = next().parse().expect("voters u32"),
            "--auto-failover" => a.auto_failover = next() == "yes",
            "--run-once" => a.run_once = true,
            other => {
                eprintln!("unknown arg: {other}\n{USAGE}");
                exit(2);
            }
        }
        i += 1;
    }
    if a.mode.is_empty() {
        eprintln!("必须指定 coord | site\n{USAGE}");
        exit(2);
    }
    a
}

// ---------------------------------------------------------------
// append-only journal（body 直存，restore 直解）
// ---------------------------------------------------------------

const J_MAGIC: [u8; 4] = *b"FJ01";
const J_VER: u8 = 1;

/// key 生命周期事件行格式：`R kid owner at` 注册 / `T old new at` 轮换 /
/// `V kid at` 撤销 / `D kid at` 退休。restore 时按序重放。
const KEY_EVENT_PREFIX: &str = "FJE1";

fn append_key_event(path: &Path, line: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{KEY_EVENT_PREFIX} {line}");
    }
}

/// 重放 key 事件（协调器启动时调用；撤销/轮换在重启后保持）。
fn apply_key_events(coord: &mut CoordinatorNegotiator, path: &Path) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let now = now_ns();
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with(KEY_EVENT_PREFIX) {
            continue;
        }
        let parts: Vec<&str> = t.split_whitespace().collect();
        if parts.get(1).copied() == Some("V") && parts.len() == 3 {
            let kid: u32 = parts[2].parse().expect("key id");
            // 重启后重放撤销：注册表里该 key 若存在则立即撤销（不复活）。
            if coord.keys().status(kid) == Some(KeyStatus::Active) {
                let _ = coord.revoke_site_key(kid, now);
                eprintln!("[coord] key {kid} 撤销重放生效");
            }
        }
    }
}

fn journal_append(path: &Path, r: &JournalRecord) -> std::io::Result<()> {
    let body = r
        .body
        .encode()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "body encode"))?;
    let mut buf = Vec::with_capacity(64 + body.len());
    buf.extend_from_slice(&J_MAGIC);
    buf.push(J_VER);
    buf.extend_from_slice(&r.at_ns.to_be_bytes());
    buf.push(match r.dir {
        Dir::In => 1u8,
        Dir::Out => 2u8,
    });
    buf.extend_from_slice(&r.peer.to_be_bytes());
    buf.push(r.kind as u8);
    buf.extend_from_slice(&r.epoch.to_be_bytes());
    buf.extend_from_slice(&r.sequence.to_be_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    buf.extend_from_slice(&(r.verdict.len() as u16).to_be_bytes());
    buf.extend_from_slice(r.verdict.as_bytes());
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(&buf)?;
    f.flush()
}

/// 读回全部记录；损坏尾段截断并告警（崩溃恢复语义）。
fn journal_load(path: &Path) -> (NegotiationJournal, u64) {
    let mut j = NegotiationJournal::default();
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(_) => return (j, 0),
    };
    let mut pos = 0usize;
    let mut max_seq = 0u64;
    while pos + 5 <= data.len() {
        if data[pos..pos + 4] != J_MAGIC || data[pos + 4] != J_VER {
            eprintln!("[journal] {path:?}: 尾部损坏于 {pos}，截断");
            break;
        }
        pos += 5;
        let rd = |p: &mut usize, n: usize| -> Option<&[u8]> {
            if data.len() - *p < n {
                None
            } else {
                let s = &data[*p..*p + n];
                *p += n;
                Some(s)
            }
        };
        let (Some(at), Some(dir), Some(peer), Some(kind), Some(ep), Some(seq), Some(lenb)) = (
            rd(&mut pos, 8).map(|b| u64::from_be_bytes(b.try_into().expect("u64"))),
            rd(&mut pos, 1).and_then(|b| match b[0] {
                1 => Some(Dir::In),
                2 => Some(Dir::Out),
                _ => None,
            }),
            rd(&mut pos, 4).map(|b| u32::from_be_bytes(b.try_into().expect("u32"))),
            rd(&mut pos, 1),
            rd(&mut pos, 8).map(|b| u64::from_be_bytes(b.try_into().expect("u64"))),
            rd(&mut pos, 8).map(|b| u64::from_be_bytes(b.try_into().expect("u64"))),
            rd(&mut pos, 4).map(|b| u32::from_be_bytes(b.try_into().expect("u32")) as usize),
        ) else {
            eprintln!("[journal] {path:?}: 头截断");
            break;
        };
        let Some(body) = rd(&mut pos, lenb) else {
            eprintln!("[journal] {path:?}: body 截断");
            break;
        };
        let Some(vl) = rd(&mut pos, 2).map(|b| u16::from_be_bytes(b.try_into().expect("u16")))
        else {
            eprintln!("[journal] {path:?}: verdict 头截断");
            break;
        };
        let Some(verdict) = rd(&mut pos, vl as usize) else {
            eprintln!("[journal] {path:?}: verdict 截断");
            break;
        };
        let kind = match FedKind::from_u8(kind[0]) {
            Ok(k) => k,
            Err(_) => {
                eprintln!("[journal] {path:?}: kind 非法");
                break;
            }
        };
        let decoded = match decode_body(kind, body) {
            Ok(b) => b,
            Err(_) => {
                eprintln!("[journal] {path:?}: body 解码失败");
                break;
            }
        };
        max_seq = max_seq.max(seq);
        j.append(JournalRecord {
            at_ns: at,
            dir,
            peer,
            kind,
            epoch: ep,
            sequence: seq,
            body: decoded,
            verdict: Box::leak(
                String::from_utf8_lossy(verdict)
                    .into_owned()
                    .into_boxed_str(),
            ),
        });
    }
    (j, max_seq)
}

// ---------------------------------------------------------------
// coord
// ---------------------------------------------------------------

fn run_coord(a: &Args) -> ! {
    let policy = LeadPolicy {
        voters: a.voters,
        auto_failover: a.auto_failover,
        hardware_witness: false,
    };
    if let Err(e) = policy.validate() {
        eprintln!("[coord] 领导权策略非法: {e:?}——两节点只允许主备+人工 failover");
        exit(2);
    }
    fs::create_dir_all(&a.journal_dir).expect("journal dir");
    let jpath = a.journal_dir.join("coord.journal");
    let (journal, max_seq) = journal_load(&jpath);
    let epoch = EpochRegulator::sanitized(journal_max_epoch(&journal).max(1)).current();
    let mut coord = CoordinatorNegotiator::new(a.node_id, a.tenant, epoch, max_seq.max(1));
    if !journal.is_empty() {
        coord.restore(&journal, read_site_key_registry(&a.site_keys));
        eprintln!(
            "[coord] 崩溃恢复：{} 条记录重放，epoch {epoch}",
            journal.len()
        );
    } else {
        for (kid, owner, pk) in read_site_keys(&a.site_keys) {
            coord
                .register_site_key(kid, owner, &pk, now_ns())
                .expect("register site key");
        }
    }
    // P1-1：撤销/轮换状态持久化——key 生命周期事件与协商 journal 分开
    // 落盘（keys.journal，append-only），重启 restore 时重放（撤销不复活）。
    apply_key_events(&mut coord, &a.journal_dir.join("keys.journal"));
    let targets = read_targets(a.targets.as_ref().expect("--targets"));
    if targets.is_empty() || targets.len() > 288 {
        eprintln!("[coord] targets 为空或超 288 区间");
        exit(2);
    }
    let mut signer = make_signer(a);
    let ca = fs::read(&a.ca).expect("ca file");
    let cert = fs::read(&a.cert).expect("cert file");
    let key = fs::read(&a.key).expect("key file");
    let server_cfg = TlsServerConfig::new_mtls(&cert, &key, &ca).expect("server mtls");
    let listener = TcpListener::bind(a.listen.expect("--listen")).expect("bind");
    let acceptor = TlsAcceptor::new(listener, server_cfg);
    eprintln!(
        "[coord] epoch {epoch} listening {} (voters={}, auto={})",
        acceptor_local(a, &acceptor),
        a.voters,
        a.auto_failover
    );

    // 顺序处理每个站点连接（每连接 = 一个站点的完整会话）。
    loop {
        let Ok(mut stream) = acceptor.accept() else {
            eprintln!("[coord] accept 失败（持续重试）");
            continue;
        };
        let _ = stream
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(30)));
        eprintln!("[coord] 站点连接建立");
        run_coord_peer(
            &mut coord,
            &mut signer,
            &mut stream,
            &targets,
            &jpath,
            epoch,
        );
    }
}

fn acceptor_local(_a: &Args, acc: &TlsAcceptor) -> String {
    let _ = acc;
    "tls".to_string()
}

/// P1-2 修复：本站纪元只从**本站出向**记录的 epoch 恢复——入向记录里
/// 的 epoch 是 peer 自报值，若用它做重启基线，恶意站点可把协调器纪元
/// 推到 u64::MAX-1 使重启后 `+1` 溢出/回绕（投毒）。
fn journal_max_epoch(j: &NegotiationJournal) -> u64 {
    j.records
        .iter()
        .filter(|r| r.dir == Dir::Out)
        .map(|r| r.epoch)
        .max()
        .unwrap_or(1)
}

fn run_coord_peer(
    coord: &mut CoordinatorNegotiator,
    signer: &mut SiteSigner,
    stream: &mut (impl Read + Write),
    targets: &[f64],
    jpath: &Path,
    epoch: u64,
) {
    let _ = epoch;
    let mut iter = 0u32;
    loop {
        iter += 1;
        if iter > MAX_PEER_ITER {
            eprintln!("[coord] 会话迭代超限，断开");
            return;
        }
        let env = match recv_frame(stream) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[coord] recv: {e}");
                return;
            }
        };
        let now = now_ns();
        let (out, outbox) = coord.on_message(&env, now, signer);
        if let Some(r) = in_record(&env, now, &out) {
            journal_append(jpath, &r).ok();
        }
        match out {
            StepOutcome::OfferRegistered { .. } => {
                eprintln!("[coord] offer 来自 site {}", env.site_id);
                // offer 到达 → 以全部已注册 offer 求解 → 只向本连接站点 propose。
                solve_and_propose(coord, signer, stream, targets, jpath, now, env.site_id);
            }
            StepOutcome::CommitmentConfirmed { commitment_id } => {
                eprintln!("[coord] commitment {commitment_id:02x?} 确认");
            }
            StepOutcome::EvidenceRecorded { .. } => {
                eprintln!("[coord] 证据归档 site {}", env.site_id);
            }
            StepOutcome::ReplayRejected { .. }
            | StepOutcome::StaleLeaderRejected { .. }
            | StepOutcome::SignatureRejected
            | StepOutcome::TenantMismatch
            | StepOutcome::NotInState => {
                eprintln!("[coord] 拒绝 site {}: {out:?}", env.site_id);
                continue;
            }
            _ => {}
        }
        for e in outbox {
            if let Some(r) = out_record(&e, now) {
                journal_append(jpath, &r).ok();
            }
            if let Err(err) = send_frame(stream, &e) {
                eprintln!("[coord] send: {err}");
            }
        }
    }
}

/// 全站求解并按站点提案（本连接只发本站点；其余站点等自己连接时提案）。
fn solve_and_propose(
    coord: &mut CoordinatorNegotiator,
    signer: &mut SiteSigner,
    stream: &mut (impl Read + Write),
    targets: &[f64],
    jpath: &Path,
    now: u64,
    only_site: u32,
) {
    let mut views: Vec<SiteOfferView> = Vec::new();
    let mut rank = 0u32;
    for (site_id, s) in coord.sessions() {
        if let Some(o) = s.offer() {
            views.push(SiteOfferView {
                site_id: *site_id,
                offer_id: o.offer_id,
                revision: o.revision,
                p_min_kw: o.p_min_kw,
                p_max_kw: o.p_max_kw,
                energy_min_kwh: o.energy_min_kwh,
                energy_max_kwh: o.energy_max_kwh,
                ramp_kw_s: o.ramp_kw_s,
                reserve_up_kw: o.reserve_up_kw,
                reserve_down_kw: o.reserve_down_kw,
                price_rank: rank,
            });
            rank += 1;
        }
    }
    if views.is_empty() {
        return;
    }
    let input = AllocationInput {
        interval_start_ns: 0,
        interval_ns: 3_600_000_000_000,
        sites: views,
        target: TargetProfile {
            p_target_kw: targets.to_vec(),
            reserve_up_kw: 0.0,
            reserve_down_kw: 0.0,
        },
    };
    match AllocationSolver::solve(&input) {
        Ok(AllocationResult::Feasible { targets, residual }) => {
            if residual.unmet_kw.iter().any(|v| *v > 1e-6) {
                eprintln!("[coord] 残差未覆盖（如实上报，不 default-fill）");
            }
            for t in targets {
                if t.site_id != only_site {
                    continue;
                }
                let o = coord
                    .session(t.site_id)
                    .and_then(|s| s.offer())
                    .cloned()
                    .expect("offer");
                let proposal = match AllocationSolver::to_proposal(
                    &t,
                    &o,
                    proposal_id_of(t.site_id),
                    1,
                    0,
                    3_600_000_000_000,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[coord] proposal 编码失败 {e:?}");
                        return;
                    }
                };
                match coord.propose(t.site_id, &proposal, now, signer) {
                    Ok((_, env)) => {
                        if let Some(r) = out_record(&env, now) {
                            journal_append(jpath, &r).ok();
                        }
                        if let Err(err) = send_frame(stream, &env) {
                            eprintln!("[coord] propose send: {err}");
                        }
                    }
                    Err(e) => eprintln!("[coord] propose 失败 {e:?}"),
                }
            }
        }
        Ok(AllocationResult::Infeasible(env)) => {
            eprintln!("[coord] 不可行（不 default-fill）: {}", env.reason);
        }
        Err(e) => eprintln!("[coord] 求解错误 {e:?}"),
    }
}

fn read_site_keys(p: &Option<PathBuf>) -> Vec<(u32, u32, Sm2PublicKey)> {
    let Some(p) = p else { return Vec::new() };
    let text = fs::read_to_string(p).expect("site-keys file");
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // 行格式：`key_id=<u32> owner=<site_id> pub=<130hex>`
        let mut kid: Option<u32> = None;
        let mut owner: Option<u32> = None;
        let mut pubhex: Option<&str> = None;
        for tok in t.split_whitespace() {
            if let Some(v) = tok.strip_prefix("key_id=") {
                kid = Some(v.parse().expect("key id"));
            } else if let Some(v) = tok.strip_prefix("owner=") {
                owner = Some(v.parse().expect("owner site id"));
            } else if let Some(v) = tok.strip_prefix("pub=") {
                pubhex = Some(v);
            }
        }
        let (kid, owner, pubhex) = (
            kid.expect("key_id="),
            owner.expect("owner="),
            pubhex.expect("pub="),
        );
        let bytes = hex_decode(pubhex);
        let pk = Sm2PublicKey::from_bytes(&bytes).expect("sm2 pubkey");
        out.push((kid, owner, pk));
    }
    out
}

fn read_site_key_registry(p: &Option<PathBuf>) -> KeyRegistry {
    let mut reg = KeyRegistry::new();
    let now = now_ns();
    for (kid, owner, pk) in read_site_keys(p) {
        reg.register_with_owner(kid, owner, &pk, now).expect("reg");
    }
    reg
}

fn read_targets(p: &Path) -> Vec<f64> {
    let text = fs::read_to_string(p).expect("targets file");
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let v: f64 = t.parse().expect("target kw");
        if !v.is_finite() || v < 0.0 {
            eprintln!("target 非法: {t}");
            exit(2);
        }
        out.push(v);
    }
    out
}

// ---------------------------------------------------------------
// site
// ---------------------------------------------------------------

fn run_site(a: &Args) -> ! {
    fs::create_dir_all(&a.journal_dir).expect("journal dir");
    let jpath = a.journal_dir.join("site.journal");
    let (journal, max_seq) = journal_load(&jpath);
    let epoch = EpochRegulator::sanitized(journal_max_epoch(&journal).max(1)).current();
    let mut site = SiteNegotiator::new(a.node_id, a.tenant, epoch, max_seq.max(1).max(100));
    let mut key_reg = KeyRegistry::new();
    let pkb = hex_decode(&a.peer_key_hex);
    if pkb.is_empty() {
        eprintln!("[site] --peer-key-hex 未提供（协调器公钥登记为组装前置）");
        exit(2);
    }
    let pk = Sm2PublicKey::from_bytes(&pkb).expect("peer sm2 pubkey");
    key_reg
        .register_with_id(a.peer_key_id, &pk, now_ns())
        .expect("peer key reg");
    let restored = !journal.is_empty();
    if restored {
        site.restore(&journal, key_reg);
        eprintln!("[site] 崩溃恢复 {} 条记录，epoch {epoch}", journal.len());
    } else {
        site.restore_keys(key_reg);
    }
    let offer = read_envelope(
        a.envelope.as_ref().expect("--envelope"),
        a.offer_id,
        a.price_schema,
        a.price_digest,
        a.capability_root,
    );
    let mut signer = make_signer(a);

    let ca = fs::read(&a.ca).expect("ca file");
    let cert = fs::read(&a.cert).expect("cert file");
    let key = fs::read(&a.key).expect("key file");
    let client_cfg = TlsClientConfig::new_mtls(&ca, &cert, &key).expect("client mtls");
    let mut stream = TlsConnector::new(client_cfg, "coord-0".try_into().expect("server name"))
        .connect(a.coord.expect("--coord"))
        .expect("connect coord");

    let now = now_ns();
    if !restored {
        // 首启：发布 offer。重启恢复的站点已发布过（journal 有记录），
        // 协调器侧状态仍在——直接进入等待提案，避免 revision 冲突。
        let (out, env) = site
            .publish_offer(&offer, now, &mut signer)
            .expect("publish offer");
        eprintln!("[site] {out:?}");
        if let Some(r) = out_record(&env, now) {
            journal_append(&jpath, &r).ok();
        }
        if let Err(e) = send_frame(&mut stream, &env) {
            eprintln!("[site] send offer: {e}");
            exit(1);
        }
    } else {
        eprintln!("[site] 恢复会话（已有 offer，直接进入协商）");
    }

    let mut iter = 0u32;
    loop {
        iter += 1;
        if iter > MAX_PEER_ITER {
            eprintln!("[site] 迭代超限退出");
            exit(1);
        }
        let env = match recv_frame(&mut stream) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[site] recv: {e}");
                exit(1);
            }
        };
        let now = now_ns();
        let (out, outbox) = site.on_message(&env, now, &mut signer);
        if let Some(r) = in_record(&env, now, &out) {
            journal_append(&jpath, &r).ok();
        }
        if matches!(
            out,
            StepOutcome::ReplayRejected { .. }
                | StepOutcome::StaleLeaderRejected { .. }
                | StepOutcome::SignatureRejected
                | StepOutcome::TenantMismatch
        ) {
            eprintln!("[site] 拒绝: {out:?}");
            continue;
        }
        for e in &outbox {
            if let Some(r) = out_record(e, now) {
                journal_append(&jpath, &r).ok();
            }
            if let Err(err) = send_frame(&mut stream, e) {
                eprintln!("[site] send: {err}");
            }
        }
        match out {
            StepOutcome::ProposalAccepted { commitment_id } => {
                eprintln!("[site] commitment {commitment_id:02x?} 已签（站内执行由站内 Compiler/Saga 承担）");
                submit_demo_evidence(&mut site, &mut signer, &mut stream, &jpath, commitment_id);
            }
            StepOutcome::ProposalRejected { reason, detail } => {
                eprintln!(
                    "[site] 结构化拒绝 {reason:?}: {}（站内继续自治）",
                    String::from_utf8_lossy(&detail)
                );
                if a.run_once {
                    exit(2);
                }
            }
            StepOutcome::Acknowledged { .. } => {
                eprintln!("[site] acked（协商闭环完成）");
                if a.run_once {
                    eprintln!("[site] run-once 完成");
                    exit(0);
                }
            }
            _ => {}
        }
    }
}

fn submit_demo_evidence(
    site: &mut SiteNegotiator,
    signer: &mut SiteSigner,
    stream: &mut (impl Read + Write),
    jpath: &Path,
    commitment_id: [u8; 16],
) {
    let now = now_ns();
    let prog = ProgressEvidence {
        commitment_id,
        site_id: site.site_id,
        interval_start_ns: 0,
        interval_end_ns: 3_600_000_000_000,
        delivered_kwh: 1.0,
        deviation_kw: 0.1,
        evidence_ref: BlobRefV1::new(3, b"progress-evidence-v1"),
        seq: 1,
    };
    if let Ok((_, env)) = site.submit_progress(&prog, now, signer) {
        if let Some(r) = out_record(&env, now) {
            journal_append(jpath, &r).ok();
        }
        let _ = send_frame(stream, &env);
    }
    let set = SettlementEvidence {
        commitment_id,
        site_id: site.site_id,
        settle_start_ns: 0,
        settle_end_ns: 3_600_000_000_000,
        measured_milli_kwh: 1_000,
        quality_bp: 9_900,
        note: b"settled".to_vec(),
    };
    if let Ok((_, env)) = site.submit_settlement(&set, now, signer) {
        if let Some(r) = out_record(&env, now) {
            journal_append(jpath, &r).ok();
        }
        let _ = send_frame(stream, &env);
    }
}

// ---------------------------------------------------------------
// 公共
// ---------------------------------------------------------------

/// 站点级确定性 proposal_id（站点 id → 16 字节）。
fn proposal_id_of(site_id: u32) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[..4].copy_from_slice(&site_id.to_be_bytes());
    id
}

fn make_signer(a: &Args) -> SiteSigner {
    let kb = hex_decode(&a.sm2_key_hex);
    assert_eq!(kb.len(), 32, "--sm2-key-hex 需要 32 字节");
    let arr: [u8; 32] = kb.try_into().expect("32");
    let sk = Sm2PrivateKey::from_bytes(&arr).expect("sm2 private key");
    let kp = Sm2KeyPair::from_private_key(&sk).expect("sm2 keypair");
    SiteSigner::new(a.sm2_key_id, kp, CsRng::new())
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64
}

fn out_record(env: &FedEnvelope, now: u64) -> Option<JournalRecord> {
    let body = decode_body(env.kind, &env.body).ok()?;
    Some(JournalRecord {
        at_ns: now,
        dir: Dir::Out,
        peer: 0,
        kind: env.kind,
        epoch: env.epoch,
        sequence: env.sequence,
        body,
        verdict: "out",
    })
}

fn in_record(env: &FedEnvelope, now: u64, out: &StepOutcome) -> Option<JournalRecord> {
    // P0-2 修复：不可信 body（签名合法但语义非法）不得经 expect 崩溃——
    // 记录为 undecodable 并继续服务（重连 DoS 面关闭）。
    let body = match decode_body(env.kind, &env.body) {
        Ok(b) => b,
        Err(_) => {
            return None;
        }
    };
    Some(JournalRecord {
        at_ns: now,
        dir: Dir::In,
        peer: env.site_id,
        kind: env.kind,
        epoch: env.epoch,
        sequence: env.sequence,
        body,
        verdict: step_verdict(out),
    })
}

fn step_verdict(o: &StepOutcome) -> &'static str {
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

fn read_envelope(
    p: &Path,
    offer_id: [u8; 16],
    price_schema: u8,
    price_digest: [u8; 32],
    capability_root: [u8; 32],
) -> CapabilityOffer {
    let text = fs::read_to_string(p).expect("envelope file");
    let mut kv: BTreeMap<String, f64> = BTreeMap::new();
    let mut valid_until_hours = 24.0f64;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let (k, v) = t.split_once('=').expect("key=value");
        if k.trim() == "valid_until_hours" {
            valid_until_hours = v.trim().parse().expect("hours");
        } else {
            kv.insert(k.trim().to_string(), v.trim().parse().expect("kv"));
        }
    }
    let f = |k: &str, d: f64| kv.get(k).copied().unwrap_or(d);
    let offer = CapabilityOffer {
        offer_id,
        interval_start_ns: 0,
        interval_end_ns: 3_600_000_000_000,
        p_min_kw: f("p_min_kw", 0.0),
        p_max_kw: f("p_max_kw", 0.0),
        energy_min_kwh: f("energy_min_kwh", 0.0),
        energy_max_kwh: f("energy_max_kwh", 0.0),
        ramp_kw_s: f("ramp_kw_s", 0.0),
        reserve_up_kw: f("reserve_up_kw", 0.0),
        reserve_down_kw: f("reserve_down_kw", 0.0),
        price_curve_ref: BlobRefV1 {
            schema: price_schema,
            digest: price_digest,
            len: 0,
        },
        capability_root,
        valid_until_ns: now_ns() + (valid_until_hours as u64) * 3_600_000_000_000,
        revision: 1,
    };
    offer.encode().expect("envelope 校验失败");
    offer
}

fn main() {
    let a = parse_args();
    match a.mode.as_str() {
        "coord" => run_coord(&a),
        "site" => run_site(&a),
        "revoke" => {
            // 撤销站点 key（运维动作）：追加事件行到 keys.journal，
            // 重启后由 apply_key_events 重放（撤销持久有效）。
            let path = a.journal_dir.join("keys.journal");
            append_key_event(&path, &format!("V {} {}", a.sm2_key_id, now_ns()));
            eprintln!("[revoke] key {} 撤销已持久化到 {:?}", a.sm2_key_id, path);
        }
        "dump-pubkey" => {
            // 站点公钥登记辅助（模拟 PKI onboarding）：输出
            // `key_id=<id> owner=<node-id> pub=<65B 未压缩 hex>`
            // 供协调器 --site-keys 使用（owner 是身份绑定面）。
            let signer = make_signer(&a);
            let pk = signer.public_key().to_bytes_uncompressed();
            let hex: String = pk.iter().map(|b| format!("{b:02x}")).collect();
            println!("key_id={} owner={} pub={}", a.sm2_key_id, a.node_id, hex);
        }
        _ => {
            eprintln!("{USAGE}");
            exit(2);
        }
    }
}
