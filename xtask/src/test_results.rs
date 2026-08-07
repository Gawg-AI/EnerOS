use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{invalid, DynResult};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestSuite {
    schema: String,
    suite_id: String,
    artifact_sha256: String,
    cases: Vec<TestCase>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestCase {
    id: String,
    required: bool,
    status: TestStatus,
    timed_out: bool,
    duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum TestStatus {
    Pass,
    Fail,
    Skipped,
    Blocked,
    Quarantined,
}

#[derive(Debug, Serialize)]
struct ValidationReport {
    schema: &'static str,
    result: &'static str,
    suite_id: String,
    required_case_count: usize,
    passed_required_case_count: usize,
    violations: Vec<String>,
}

pub(super) fn command_result(path: &Path) -> DynResult<()> {
    let suite: TestSuite = serde_json::from_slice(&fs::read(path)?)?;
    let report = validate_suite(&suite);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.violations.is_empty() {
        Ok(())
    } else {
        Err(invalid("test result contains a non-PASS required outcome"))
    }
}

pub(super) fn command_gate_semantics() -> DynResult<()> {
    let valid = fixture(TestStatus::Pass, false, true);
    if !validate_suite(&valid).violations.is_empty() {
        return Err(invalid("valid PASS fixture was rejected"));
    }
    for (name, suite) in [
        ("skip", fixture(TestStatus::Skipped, false, true)),
        ("timeout", fixture(TestStatus::Pass, true, true)),
        ("failure", fixture(TestStatus::Fail, false, true)),
        ("blocked", fixture(TestStatus::Blocked, false, true)),
        ("quarantine", fixture(TestStatus::Quarantined, false, true)),
        ("empty", fixture(TestStatus::Pass, false, false)),
    ] {
        if validate_suite(&suite).violations.is_empty() {
            return Err(invalid(format!(
                "gate semantics mutation '{name}' was incorrectly accepted"
            )));
        }
    }
    println!(
        "{{\"schema\":\"eneros.gate-semantics-test.v1\",\"result\":\"PASS\",\"mutations\":6}}"
    );
    Ok(())
}

pub(super) fn command_ci_policy(root: &Path) -> DynResult<()> {
    let workflow_path = root.join(".github/workflows/ci.yml");
    let workflow = fs::read_to_string(&workflow_path)?;
    let mut violations = Vec::new();
    for (pattern, code) in [
        ("|| true", "shell-or-true"),
        ("continue-on-error: true", "continue-on-error"),
        ("--features mock", "mock-test-in-formal-ci"),
        ("#[ignore]", "ignored-test"),
    ] {
        if workflow.contains(pattern) {
            violations.push(code.to_owned());
        }
    }
    // qemu `-kernel` 注入检测：seL4 elfloader 官方验证路径（ADR-0023，CI seL4 构建 +
    // QEMU 运行阶段使用 `-kernel target/sel4/`）是白名单；除此之外的 `-kernel`/`--kernel`
    // （例如绕过 UEFI 启动链直接注入自研内核）视为违规。
    // 注意用 `-kernel `（带空格）精确匹配 qemu 参数，避免误伤 `sel4-kernel-init` 标识符。
    for (line_no, line) in workflow.lines().enumerate() {
        if line.contains("-kernel ") && !line.contains("-kernel target/sel4/") {
            violations.push(format!(
                "ci.yml:{line_no}: qemu-kernel-injection (only '-kernel target/sel4/' is allowed)"
            ));
        }
        if line.contains("--kernel") {
            violations.push(format!("ci.yml:{line_no}: qemu-kernel-injection (--kernel)" ));
        }
    }
    for required in [
        "cargo xtask audit all",
        "cargo xtask test unit",
        "cargo xtask test ci-policy",
        "cargo xtask test gate-semantics",
        "cargo metadata --locked --offline",
        "cargo-closure.ps1 -VerifyDirectory",
    ] {
        if !workflow.contains(required) {
            violations.push(format!("missing-required-step:{required}"));
        }
    }
    // 非 grep 门禁：解析 ci.yml 中每个 `cargo xtask <cmd> <args>` 步骤，静态校验
    // 子命令与首参数合法。注入不可执行的 xtask 步骤（未知子命令 / 非法 mode）
    // 时门禁必须 FAIL —— 防假绿回归（09 号文 P0-28：grep 门禁无法发现步骤不可执行）。
    violations.extend(validate_xtask_steps(&workflow));
    let flash = fs::read_to_string(root.join("tools/flash.sh"))?;
    if flash.contains("-kernel") || flash.contains("dd if=") || flash.contains("make build") {
        violations.push("legacy-flash-remains-active".to_owned());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "eneros.ci-policy-report.v1",
            "result": if violations.is_empty() { "PASS" } else { "FAIL" },
            "violations": violations,
        }))?
    );
    if violations.is_empty() {
        Ok(())
    } else {
        Err(invalid("CI fail-closed policy verification failed"))
    }
}

/// xtask 子命令 → 首参数（mode）白名单。与 xtask/src/main.rs 的 real_main 分派一致；
/// 只校验 mode 级合法性，后续参数由命令自身在真实执行时校验。
/// 末尾的 `None` 表示该子命令不接受 mode（无参数或参数由实现自行解析）。
fn xtask_command_modes() -> &'static [(&'static str, Option<&'static [&'static str]>, &'static str)] {
    &[
        ("audit", Some(&["all", "profile", "layout", "toolchain", "trust"]), "audit"),
        ("build", Some(&["all"]), "build <profile-id>|all"),
        ("test", Some(&["unit", "result", "gate-semantics", "ci-policy"]), "test"),
        (
            "image",
            Some(&["raw", "iso", "recovery", "vars", "update", "qemu-prod"]),
            "image",
        ),
        ("fault", Some(&["manifest", "bootctrl", "installer", "artifact"]), "fault"),
        ("fuzz", None, "fuzz <target> <iterations>"),
        ("build-compare", None, "build-compare"),
        ("audit-unsafe-safety", None, "audit-unsafe-safety"),
        ("run", Some(&["qemu-prod", "qemu-g3", "qemu-install"]), "run"),
        (
            "inspect",
            Some(&["raw", "recovery", "installer", "update"]),
            "inspect",
        ),
        ("evidence", Some(&["verify", "gate"]), "evidence"),
        (
            "flash",
            Some(&["list", "verify-inventory", "lab-create", "authorize-file", "write"]),
            "flash",
        ),
        (
            "qa",
            Some(&["boot-reset", "soak", "soak-run", "perf-baseline", "perf-gate", "trace", "proof", "validation"]),
            "qa",
        ),
        ("release", Some(&["sbom", "verify-sbom"]), "release"),
        ("symbolize", None, "symbolize <dump> <elf>"),
    ]
}

/// 校验 ci.yml 中每个 `cargo xtask ...` 步骤的命令可执行性（静态解析，不实际运行）。
/// 支持 `run: cargo xtask ...`（单行）与 `run: |` 多行块内的 `cargo xtask ...` 行。
fn validate_xtask_steps(workflow: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for (line_no, line) in workflow.lines().enumerate() {
        let trimmed = line.trim();
        // 兼容 `run: cargo xtask ...` 与块内 `cargo xtask ...` 两种写法。
        let candidate = trimmed
            .strip_prefix("run:")
            .map(str::trim)
            .unwrap_or(trimmed);
        if !candidate.starts_with("cargo xtask") {
            continue;
        }
        let rest = &candidate["cargo xtask".len()..].trim_start();
        let mut tokens = rest.split_whitespace();
        let Some(command) = tokens.next() else {
            violations.push(format!("ci.yml:{line_no}: cargo xtask without a command"));
            continue;
        };
        let schema = xtask_command_modes();
        let Some((_, modes, usage)) = schema.iter().find(|(name, _, _)| *name == command) else {
            violations.push(format!("ci.yml:{line_no}: unknown xtask command '{command}'"));
            continue;
        };
        let mode = tokens.next();
        match modes {
            Some(allowed) => match mode {
                Some(mode) if allowed.contains(&mode) => {}
                Some(mode) => violations.push(format!(
                    "ci.yml:{line_no}: xtask {command} invalid mode '{mode}' (usage: {usage})"
                )),
                None => violations.push(format!(
                    "ci.yml:{line_no}: xtask {command} requires a mode (usage: {usage})"
                )),
            },
            None => {
                // 无固定 mode 的子命令：校验至少存在一个参数
                // （fuzz/symbolize；build-compare 与 audit-unsafe-safety
                // 为无参数命令，豁免）。
                if mode.is_none()
                    && command != "build-compare"
                    && command != "audit-unsafe-safety"
                {
                    violations.push(format!(
                        "ci.yml:{line_no}: xtask {command} requires arguments (usage: {usage})"
                    ));
                }
            }
        }
    }
    violations
}

fn validate_suite(suite: &TestSuite) -> ValidationReport {    let mut violations = Vec::new();
    if suite.schema != "eneros.test-result.v1" {
        violations.push("schema".to_owned());
    }
    if suite.suite_id.is_empty()
        || suite.artifact_sha256.len() != 64
        || !suite
            .artifact_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        violations.push("identity".to_owned());
    }
    if suite.cases.is_empty() {
        violations.push("empty-suite".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut required = 0usize;
    let mut passed_required = 0usize;
    for case in &suite.cases {
        if case.id.is_empty() || !ids.insert(case.id.as_str()) {
            violations.push(format!("duplicate-or-empty-case:{}", case.id));
        }
        if case.timed_out {
            violations.push(format!("timeout:{}", case.id));
        }
        if case.required {
            required += 1;
            if case.status == TestStatus::Pass && !case.timed_out {
                passed_required += 1;
            } else {
                violations.push(format!("required-not-pass:{}", case.id));
            }
        }
        if case.duration_ms == 0 && case.status == TestStatus::Pass {
            violations.push(format!("zero-duration-pass:{}", case.id));
        }
    }
    if required == 0 {
        violations.push("no-required-cases".to_owned());
    }
    ValidationReport {
        schema: "eneros.test-result-validation.v1",
        result: if violations.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        suite_id: suite.suite_id.clone(),
        required_case_count: required,
        passed_required_case_count: passed_required,
        violations,
    }
}

fn fixture(status: TestStatus, timed_out: bool, include_case: bool) -> TestSuite {
    TestSuite {
        schema: "eneros.test-result.v1".to_owned(),
        suite_id: "gate-semantics".to_owned(),
        artifact_sha256: "1".repeat(64),
        cases: if include_case {
            vec![TestCase {
                id: "REQ-QA-001".to_owned(),
                required: true,
                status,
                timed_out,
                duration_ms: 1,
            }]
        } else {
            Vec::new()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_required_pass_is_accepted() {
        assert!(validate_suite(&fixture(TestStatus::Pass, false, true))
            .violations
            .is_empty());
        assert!(!validate_suite(&fixture(TestStatus::Skipped, false, true))
            .violations
            .is_empty());
        assert!(!validate_suite(&fixture(TestStatus::Pass, true, true))
            .violations
            .is_empty());
        assert!(!validate_suite(&fixture(TestStatus::Pass, false, false))
            .violations
            .is_empty());
    }

    #[test]
    fn xtask_step_validation_accepts_legal_commands() {
        let workflow = "run: cargo xtask evidence verify\n\
                        run: cargo xtask image raw qemu-prod --signing-key x\n\
                        run: cargo xtask fuzz boot_protocol 10000\n\
                        run: cargo xtask build-compare\n\
                        run: cargo xtask test gate-semantics\n";
        assert!(validate_xtask_steps(workflow).is_empty());
    }

    #[test]
    fn xtask_step_validation_rejects_unknown_command() {
        let workflow = "run: cargo xtask evidence verify\n\
                        run: cargo xtask bogus-cmd arg\n";
        let violations = validate_xtask_steps(workflow);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("unknown xtask command 'bogus-cmd'")),
            "violations: {violations:?}"
        );
    }

    #[test]
    fn xtask_step_validation_rejects_invalid_mode_and_missing_mode() {
        let invalid_mode = validate_xtask_steps("run: cargo xtask evidence bogus\n");
        assert!(
            invalid_mode
                .iter()
                .any(|v| v.contains("invalid mode 'bogus'")),
            "violations: {invalid_mode:?}"
        );
        let missing_mode = validate_xtask_steps("run: cargo xtask image\n");
        assert!(
            missing_mode
                .iter()
                .any(|v| v.contains("requires a mode")),
            "violations: {missing_mode:?}"
        );
    }

    #[test]
    fn xtask_step_validation_accepts_run_prefix_and_block_form() {
        // 兼容 `run: cargo xtask ...`（单行）与多行块内的裸 `cargo xtask ...`。
        let workflow = "run: cargo xtask test ci-policy\n\
                        run: |\n  cargo xtask audit all\n  cargo xtask build all\n";
        assert!(validate_xtask_steps(workflow).is_empty(), "{workflow}");
    }

    #[test]
    fn kernel_injection_allows_only_sel4_white_list() {
        // `-kernel target/sel4/`（seL4 elfloader 官方验证，ADR-0023）放行；
        // 其他 `-kernel` / `--kernel`（绕过 UEFI 启动链注入自研内核）拒绝。
        let allowed = "run: qemu-system-aarch64 -kernel target/sel4/eneros-root-image-arm-qemu-arm-virt\n";
        assert!(!allowed.contains("-kernel ") || allowed.contains("-kernel target/sel4/"));
        let blocked = "run: qemu-system-aarch64 -kernel foo.elf\n";
        assert!(blocked.contains("-kernel ") && !blocked.contains("-kernel target/sel4/"));
        let long_flag = "run: qemu --kernel foo\n";
        assert!(long_flag.contains("--kernel"));
    }
}
