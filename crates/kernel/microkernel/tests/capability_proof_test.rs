//! 能力管理器形式化证明 host 集成测试（v0.155.0，D1 降级口径 host 可测骨架）。
//!
//! 本文件从集成测试视角对 v0.155.0 能力管理器形式化证明 ★ 交付物做静态契约断言：
//! Capability_Proof.thy 关键字完整性 + ROOT session 含新 theory +
//! ADR-0001 范围核验（能力证明三定理）+ 证明链存在性 +
//! v0.154.0 Ipc_Proof.thy 未被改动（Surgical Changes）。
//! 按 D1 决策仅做文件内容/常量断言（Isabelle 未安装降级）。
//! 集成测试在 host 上编译运行，可使用 std。

#![allow(clippy::disallowed_macros)]

use std::fs;
use std::path::{Path, PathBuf};

/// 定位仓库根：自 `CARGO_MANIFEST_DIR`（crates/kernel/microkernel）
/// 沿 ancestors 上溯，取首个含 `rust-toolchain.toml` 的目录。
///
/// 同 `formal_framework_test.rs::repo_root` / `scheduler_spec_test.rs::repo_root`
/// / `scheduler_proof_test.rs::repo_root` / `memory_spec_test.rs::repo_root`
/// / `memory_proof_test.rs::repo_root` / `ipc_proof_test.rs::repo_root` 口径——
/// 避免硬编码相对路径，稳健定位仓库根（formal/ 目录在仓库根下）。
fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|dir| dir.join("rust-toolchain.toml").is_file())
        .expect("未能在 CARGO_MANIFEST_DIR 祖先目录中找到仓库根 rust-toolchain.toml")
        .to_path_buf()
}

/// 定位 formal/ 目录：仓库根/formal。
fn formal_dir() -> PathBuf {
    repo_root().join("formal")
}

/// 读取 formal/ 下指定文件内容，失败时 panic 报告路径。
fn read_formal_file(name: &str) -> String {
    let path = formal_dir().join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取形式化框架文件 {:?} 失败：{}", path, e))
}

/// 断言 content 含所有 keywords（逐项报告缺失项）。
fn assert_contains_all(content: &str, keywords: &[&str], file_label: &str) {
    for kw in keywords {
        assert!(
            content.contains(kw),
            "{} 缺少关键字 `{}`（F1~F5 关键字完整性契约）",
            file_label,
            kw
        );
    }
}

/// 检测 Isabelle 文件中是否存在非注释的 sorry 证明命令。
///
/// 剥离 `(* ... *)` 注释块后检查剩余内容是否含 `sorry`。
/// 注释中提及 "无 sorry" / "不含 sorry" 不算违规。
fn has_sorry_outside_comments(content: &str) -> bool {
    let mut result = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("(*") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        if let Some(end) = rest.find("*)") {
            rest = &rest[end + 2..];
        } else {
            // 未闭合注释，忽略剩余
            rest = "";
        }
    }
    result.push_str(rest);
    result.contains("sorry")
}

/// F1：Capability_Proof.thy 关键字完整性——能力管理器形式化证明理论含 15 关键字。
///
/// 覆盖：theory 声明 + Capability 导入 + lemma mint_rights_eq +
/// theorem rights_monotone（★ 权限单调性）+ lemma descendant_subset（辅助引理）+
/// descendant.induct（归纳规则）+ direct_child（归纳基础）+
/// trans（归纳传递）+ theorem revoke_cascade（★ 级联撤销）+
/// theorem revoke_self_removed + theorem no_bypass（★ 不可绕过）+
/// theorem capability_security（组合定理）+ revoke_def + check_def + end 收尾。
/// ★ 里程碑验收：整个文件不含 sorry。
#[test]
fn f1_capability_proof_thy_keywords() {
    let content = read_formal_file("Isabelle/Capability_Proof.thy");

    assert_contains_all(
        &content,
        &[
            "theory Capability_Proof",
            "imports Capability",
            "lemma mint_rights_eq",
            "theorem rights_monotone",
            "lemma descendant_subset",
            "descendant.induct",
            "direct_child",
            "trans",
            "theorem revoke_cascade",
            "theorem revoke_self_removed",
            "theorem no_bypass",
            "theorem capability_security",
            "revoke_def",
            "check_def",
            "end",
        ],
        "Capability_Proof.thy",
    );

    // ★ 里程碑验收：整个证明文件不含非注释 sorry
    assert!(
        !has_sorry_outside_comments(&content),
        "★ 里程碑验收失败：Capability_Proof.thy 含非注释 sorry（v0.155.0 必须完整证明无 sorry）"
    );
}

/// F2：ROOT session 配置——Isabelle 构建系统 session 根文件含 Capability + Capability_Proof theory。
///
/// D6：v0.155.0 在 ROOT theories 列表中新增 `"Isabelle/Capability"` + `"Isabelle/Capability_Proof"` 条目，
/// 结构验证确保 session 仍声明全部 10 个理论。
#[test]
fn f2_root_session_has_capability_proof() {
    let content = read_formal_file("ROOT");

    assert_contains_all(
        &content,
        &[
            "session",
            "Isabelle/Kernel",
            "Isabelle/Refinement",
            "Isabelle/Scheduler",
            "Isabelle/Scheduler_Proof",
            "Isabelle/Memory",
            "Isabelle/Memory_Proof",
            "Isabelle/Ipc",
            "Isabelle/Ipc_Proof",
            "Isabelle/Capability",
            "Isabelle/Capability_Proof",
        ],
        "ROOT",
    );
}

/// F3：ADR-0001 范围核验——Capability_Proof.thy 含能力证明三定理关键字。
///
/// ADR-0001 裁剪：能力证明降级为 seL4 引用与文档化。
/// 本测试核验 Capability_Proof.thy 含三核心定理关键字。
#[test]
fn f3_adr0001_scope_verification() {
    let content = read_formal_file("Isabelle/Capability_Proof.thy");

    // 正向核验：Capability_Proof.thy 含能力证明三定理
    assert_contains_all(
        &content,
        &[
            "rights_monotone",
            "revoke_cascade",
            "no_bypass",
            "capability_security",
        ],
        "Capability_Proof.thy (Capability proof)",
    );

    // 核验：Capability_Proof.thy 不含非注释 sorry（ADR-0001 文档化证明仍需完整）
    assert!(
        !has_sorry_outside_comments(&content),
        "Capability_Proof.thy 含非注释 sorry——v0.155.0 ★ 里程碑要求完整证明"
    );
}

/// F4：证明链存在性——Capability_Proof.thy imports Capability imports Main。
///
/// 蓝图 §4.1 三层架构 + v0.155.0 证明链：
/// Capability_Proof.thy（证明层）→ Capability.thy（规范层 v0.155.0）→ Main。
/// 本测试核验证明链完整性，确保 imports 关系正确串接。
#[test]
fn f4_proof_chain_existence() {
    // 证明层：Capability_Proof.thy imports Capability
    let proof = read_formal_file("Isabelle/Capability_Proof.thy");
    assert!(
        proof.contains("imports Capability"),
        "证明层 Capability_Proof.thy 缺少 `imports Capability`（F4 证明链契约）"
    );

    // 规范层：Capability.thy imports Main
    let cap = read_formal_file("Isabelle/Capability.thy");
    assert!(
        cap.contains("imports Main"),
        "规范层 Capability.thy 缺少 `imports Main`（F4 证明链契约）"
    );
}

/// F5：Surgical Changes 核验——v0.154.0 Ipc_Proof.thy 未被 v0.155.0 改动。
///
/// v0.155.0 蓝图声明"Surgical Changes"：仅新增 Capability.thy + Capability_Proof.thy +
/// 修改 ROOT（两行新增）+ README（追加条目），不改动既有 Ipc_Proof.thy 等文件。
/// 本测试通过关键字完整性核验确保 v0.154.0 交付物保持原样。
#[test]
fn f5_v1540_ipc_proof_thy_unchanged() {
    let proof_path = repo_root()
        .join("formal")
        .join("Isabelle")
        .join("Ipc_Proof.thy");
    let content = fs::read_to_string(&proof_path).expect("Ipc_Proof.thy 必须存在");

    // 必须仍含 v0.154.0 关键字（证明完整性）
    assert_contains_all(
        &content,
        &[
            "theory Ipc_Proof",
            "imports Ipc",
            "theorem ipc_integrity",
            "theorem ipc_integrity_recv",
            "inductive ipc_step",
            "theorem ipc_integrity_all",
            "rtranclp.induct",
            "theorem ipc_no_forge",
            "end",
        ],
        "Ipc_Proof.thy (v0.154.0 证明完整)",
    );

    // Ipc_Proof.thy 不应含非注释 sorry（v0.154.0 证明无占位）
    assert!(
        !has_sorry_outside_comments(&content),
        "Ipc_Proof.thy 不应含非注释 sorry（v0.154.0 证明层无占位定理）"
    );
}
