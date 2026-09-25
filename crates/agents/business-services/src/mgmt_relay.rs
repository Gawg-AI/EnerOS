//! WP-75.2.8（R16-I）：svc-mgmt TargetExecutor 判据下沉库。
//!
//! 相A：ConfigSet S/C/A 中继响应归类（bin 手抄收编；2026-09-24
//! 「先判 ERR：否则拒绝被记成完成副作用」已修 bug 的第一道锁）。
//! 相B：重启在途对账终态判定（超时/在途/终态三态）。
//! 相C：支持包快照分节格式化（golden 锁词形）。
//!
//! 层位（对照 dispatch/telemetry/ota 先例）：**纯判据下沉，EIPC 拨号
//! 留 bin**——root_call/persist CAS/客户端超时等跨进程面不进本模块；
//! 响应字节 → ActionResult/终态 的归类判据 host 可测。

use alloc::format;
use alloc::string::String;

use crate::mgmt_ops::OpStatus;
use crate::mgmt_rpc::{ActionResult, RpcStatus};

// ===== 相A：ConfigSet S/C/A 中继响应归类（svc-mgmt.rs 手抄保形） =====

/// S 臂（stage-part）响应归类。判据保形（svc-mgmt bin 原文）：
/// **先判 ERR**——svc-config 的拒绝以 `ERR:` 文本承载（长度 ≥1 且首
/// 字节 'E'=69），"非空即成功"会把每次拒绝都记成
/// `config-stage-part:seq/total accepted` 并作为**已完成**副作用写进
/// 幂等账本（同一 request_id 无法重试）。EIPC Err 臂留 bin
/// （`config-stage-eipc:{e:?}`——错误类型属拨号面）。
pub fn classify_stage_resp(resp: &[u8], total: u8) -> ActionResult {
    if resp.starts_with(b"ERR:") {
        return ActionResult {
            status: RpcStatus::Failed,
            detail: format!(
                "config-stage-rejected:{}",
                core::str::from_utf8(resp).unwrap_or("?")
            ),
            payload: None,
        };
    }
    if !resp.is_empty() {
        return ActionResult {
            status: RpcStatus::Completed,
            detail: format!("config-stage-part:{}/{} accepted", resp[0], total),
            payload: None,
        };
    }
    ActionResult {
        status: RpcStatus::Failed,
        detail: String::from("config-stage-rejected:empty-response"),
        payload: None,
    }
}

/// C 臂（stage-commit）响应归类：32B digest = Completed（detail =
/// 前 8 字节 hex + 省略号；payload 携带 digest 全量）；其余 = Failed
/// （`config-commit-rejected:{utf8}`）。
pub fn classify_commit_resp(resp: &[u8]) -> ActionResult {
    if resp.len() == 32 {
        let hex: String = resp[..8].iter().map(|b| format!("{b:02x}")).collect();
        ActionResult {
            status: RpcStatus::Completed,
            detail: format!("config-staged:digest={hex}…"),
            payload: Some(resp.to_vec()),
        }
    } else {
        ActionResult {
            status: RpcStatus::Failed,
            detail: format!(
                "config-commit-rejected:{}",
                core::str::from_utf8(resp).unwrap_or("?")
            ),
            payload: None,
        }
    }
}

/// A 臂（activate）响应归类：8B generation LE = Completed
/// （`config-activated:gen={g}`，payload 携带全量）；其余 = Failed。
pub fn classify_activate_resp(resp: &[u8]) -> ActionResult {
    if resp.len() == 8 {
        let gen = u64::from_le_bytes(resp[..8].try_into().unwrap());
        ActionResult {
            status: RpcStatus::Completed,
            detail: format!("config-activated:gen={gen}"),
            payload: Some(resp.to_vec()),
        }
    } else {
        ActionResult {
            status: RpcStatus::Failed,
            detail: format!(
                "config-activate-rejected:{}",
                core::str::from_utf8(resp).unwrap_or("?")
            ),
            payload: None,
        }
    }
}

// ===== 相B：重启在途对账终态判定 =====

/// 重启在途对账 detail 解析：`restart <target> base_gen=<g>` →
/// `(target, base_gen)`；非此形 = `None`（不构成对账对象）。
pub fn parse_restart_detail(detail: &str) -> Option<(String, u64)> {
    let rest = detail.strip_prefix("restart ")?;
    let (target, base_text) = rest.split_once(" base_gen=")?;
    let base_gen: u64 = base_text.parse().ok()?;
    Some((String::from(target), base_gen))
}

/// 终态判定三态（svc-mgmt `observe_operation` 判据保形）：
/// ① `gen > base_gen` → Completed（`restart-complete:…`）；
/// ② `now_ms - created_ms > timeout_ms` → Failed（`restart-timeout:…`
/// 附 root serial 核对提示）；
/// ③ 其余 = `None`（仍在途，继续观察）。
/// 观测事实组（`root_mgmt::parse_instance_resp` 的 `(gen, _lc, restarts,
/// ready)` 三字段直传——避免 8 参拆散同源元组）。
pub struct RestartFacts {
    pub gen: u64,
    pub restarts: u64,
    pub ready: bool,
}

pub fn classify_restart_observation(
    target: &str,
    base_gen: u64,
    facts: RestartFacts,
    created_ms: u64,
    now_ms: u64,
    timeout_ms: u64,
) -> Option<(OpStatus, String)> {
    let RestartFacts {
        gen,
        restarts,
        ready,
    } = facts;
    if gen > base_gen {
        return Some((
            OpStatus::Completed,
            format!("restart-complete:target={target} gen={gen} restarts={restarts} ready={ready}"),
        ));
    }
    if now_ms.saturating_sub(created_ms) > timeout_ms {
        return Some((
            OpStatus::Failed,
            format!(
                "restart-timeout:target={target} base_gen={base_gen} last_gen={gen}; verify root serial (MGMT-RESTART words)"
            ),
        ));
    }
    None
}

// ===== 相C：快照分节格式化 =====

/// 分节行（svc-mgmt `build_snapshot` section 闭包保形）：
/// `{name}|digest={sm3 前 8 字节 hex}|{body}\n`。
pub fn format_snapshot_section(name: &str, body: &str) -> String {
    let d = eneros_crypto::sm3_hash(body.as_bytes());
    format!(
        "{name}|digest={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}|{body}\n",
        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// ① identity 分节 body。
pub fn format_identity_body() -> String {
    format!("product_version={}", env!("CARGO_PKG_VERSION"))
}

/// ② status 分节 body：本服务计数 + root 维护态投影（None = root 未
/// 绑定 → `missing:root-unbound` 缺项词形）。
pub fn format_status_body(requests_seen: u64, maint: Option<(bool, u64)>) -> String {
    let maint = match maint {
        Some((active, tick)) => format!("maintenance={} entered_tick={tick}", active),
        None => String::from("missing:root-unbound"),
    };
    format!("requests_seen={requests_seen}\n{maint}")
}

/// ③ config 分节 body：`(32B digest, generation)` =
/// `generation={g} digest={前 16B hex} source=svc-config`；不可得 =
/// `missing:config-unreachable` 缺项词形（digest 非 32B 同此臂）。
pub fn format_config_body(digest_info: Option<(alloc::vec::Vec<u8>, u64)>) -> String {
    match digest_info {
        Some((digest, g)) if digest.len() == 32 => {
            let hex = |b: &[u8]| -> String { b.iter().map(|x| format!("{x:02x}")).collect() };
            format!(
                "generation={g} digest={} source=svc-config",
                hex(&digest[..16])
            )
        }
        _ => String::from("missing:config-unreachable"),
    }
}

/// ④ logging 分节 body：logging 配置字节数（运行日志未经支持包分页面
/// ——显式缺项词形）；不可得 = `missing:log-config-unreachable`。
pub fn format_logging_body(raw_len: Option<usize>) -> String {
    match raw_len {
        Some(n) => format!("log-config-bytes={n} runtime-logs=missing:support-bundle-page-only"),
        None => String::from("missing:log-config-unreachable"),
    }
}

/// ⑤ persistence 分节 body：操作账本存储连通性 + 快照时刻。
pub fn format_persistence_body(ops_store_ok: bool, now_ms: u64) -> String {
    format!(
        "ops_store={} at_ms={now_ms}",
        if ops_store_ok {
            "ok"
        } else {
            "missing:storage-unbound"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mgmt_rpc::RpcStatus;

    // ===== 相A：ConfigSet S/C/A 中继响应归类 =====

    /// S 臂：ERR 优先级——拒绝词形（`ERR:` 前缀）必须归 Failed，
    /// 不得落进"非空即成功"的 Completed 臂（2026-09-24 已修 bug 锁：
    /// 拒绝曾被记成 `config-stage-part:69/N accepted` 完成副作用写入
    /// 幂等账本，同一 request_id 无法重试）。
    #[test]
    fn stage_err_response_is_failed_not_completed() {
        let r = classify_stage_resp(b"ERR:bad-part", 3);
        assert_eq!(r.status, RpcStatus::Failed, "拒绝不得归类 Completed");
        assert_eq!(
            r.detail, "config-stage-rejected:ERR:bad-part",
            "拒绝 detail 保形"
        );
        assert!(r.payload.is_none());
    }

    /// S 臂：各态归类——非空分片序号回显 = Completed；空响应 = Failed。
    #[test]
    fn stage_response_classification() {
        let ok = classify_stage_resp(&[2u8], 3);
        assert_eq!(ok.status, RpcStatus::Completed);
        assert_eq!(ok.detail, "config-stage-part:2/3 accepted");
        let empty = classify_stage_resp(b"", 3);
        assert_eq!(empty.status, RpcStatus::Failed);
        assert_eq!(empty.detail, "config-stage-rejected:empty-response");
    }

    /// C 臂：32B digest = Completed（detail 前 8B hex + 省略号，payload
    /// 全量）；非 32B = Failed（config-commit-rejected + 响应文本）。
    #[test]
    fn commit_response_classification() {
        let mut digest = alloc::vec![0xABu8; 32];
        digest[0] = 0x01;
        let r = classify_commit_resp(&digest);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(r.detail, "config-staged:digest=01ababababababab…");
        assert_eq!(r.payload.as_deref(), Some(&digest[..]));
        let bad = classify_commit_resp(b"ERR:mismatch");
        assert_eq!(bad.status, RpcStatus::Failed);
        assert_eq!(bad.detail, "config-commit-rejected:ERR:mismatch");
    }

    /// A 臂：8B generation LE = Completed；其余 = Failed。
    #[test]
    fn activate_response_classification() {
        let mut gen = alloc::vec![0u8; 8];
        gen[..8].copy_from_slice(&7u64.to_le_bytes());
        let r = classify_activate_resp(&gen);
        assert_eq!(r.status, RpcStatus::Completed);
        assert_eq!(r.detail, "config-activated:gen=7");
        assert_eq!(r.payload.as_deref(), Some(&gen[..]));
        let bad = classify_activate_resp(b"ERR:no-bundle");
        assert_eq!(bad.status, RpcStatus::Failed);
        assert_eq!(bad.detail, "config-activate-rejected:ERR:no-bundle");
    }

    // ===== 相B：重启在途对账终态判定 =====

    /// detail 解析：`restart <target> base_gen=<g>`（非此形 = None）。
    #[test]
    fn restart_detail_parsing() {
        let (target, base) =
            parse_restart_detail("restart energy-agent base_gen=41").expect("parse");
        assert_eq!(target, "energy-agent");
        assert_eq!(base, 41);
        assert!(parse_restart_detail("restart-no-space base_gen=1").is_none());
        assert!(parse_restart_detail("restart energy-agent").is_none());
        assert!(parse_restart_detail("restart energy-agent base_gen=x").is_none());
    }

    /// 三态：gen > base = Completed；超时 = Failed（含 root serial 提示）；
    /// 其余在途 = None。
    #[test]
    fn restart_observation_three_states() {
        let facts = |gen, restarts, ready| RestartFacts {
            gen,
            restarts,
            ready,
        };
        let done = classify_restart_observation(
            "energy-agent",
            41,
            facts(42, 3, true),
            1_000,
            2_000,
            120_000,
        );
        let (status, detail) = done.expect("终态");
        assert_eq!(status, OpStatus::Completed);
        assert_eq!(
            detail,
            "restart-complete:target=energy-agent gen=42 restarts=3 ready=true"
        );
        let timeout = classify_restart_observation(
            "energy-agent",
            41,
            facts(41, 0, false),
            1_000,
            200_000,
            120_000,
        );
        let (status, detail) = timeout.expect("终态");
        assert_eq!(status, OpStatus::Failed);
        assert_eq!(
            detail,
            "restart-timeout:target=energy-agent base_gen=41 last_gen=41; verify root serial (MGMT-RESTART words)"
        );
        assert!(
            classify_restart_observation(
                "energy-agent",
                41,
                facts(41, 0, false),
                1_000,
                2_000,
                120_000
            )
            .is_none(),
            "未超时仍在途"
        );
        // 恰在超时边界：不超（> 判据）。
        assert!(classify_restart_observation(
            "energy-agent",
            41,
            facts(41, 0, false),
            1_000,
            121_000,
            120_000
        )
        .is_none());
    }

    // ===== 相C：快照分节格式化 =====

    /// 分节词形 golden：`{name}|digest={sm3 前 8B hex}|{body}\n`。
    #[test]
    fn snapshot_section_golden() {
        let line = format_snapshot_section("persistence", "ops_store=ok at_ms=99");
        assert_eq!(
            line, "persistence|digest=561bef7e4f871178|ops_store=ok at_ms=99\n",
            "分节词形/摘要前缀锁（漂移即红）"
        );
    }

    /// 各分节 body 构造（缺项词形锁）。
    #[test]
    fn snapshot_bodies_golden() {
        assert_eq!(
            format_status_body(7, Some((true, 55))),
            "requests_seen=7\nmaintenance=true entered_tick=55"
        );
        assert_eq!(
            format_status_body(7, None),
            "requests_seen=7\nmissing:root-unbound"
        );
        let mut digest = alloc::vec![0u8; 32];
        digest[0] = 0x12;
        assert_eq!(
            format_config_body(Some((digest, 9))),
            "generation=9 digest=12000000000000000000000000000000 source=svc-config",
            "前 16B hex（32 字符）+ source 词形锁"
        );
        assert_eq!(
            format_config_body(None),
            "missing:config-unreachable",
            "不可得 = 缺项词形"
        );
        assert_eq!(
            format_logging_body(Some(128)),
            "log-config-bytes=128 runtime-logs=missing:support-bundle-page-only"
        );
        assert_eq!(format_logging_body(None), "missing:log-config-unreachable");
        assert_eq!(format_persistence_body(true, 99), "ops_store=ok at_ms=99");
        assert_eq!(
            format_persistence_body(false, 99),
            "ops_store=missing:storage-unbound at_ms=99"
        );
    }
}
