use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{invalid, read_json, DynResult};

const EXPECTED_REQUIREMENTS: &[&str] = &[
    "REQ-G0-BASE-001",
    "REQ-G0-BASE-002",
    "REQ-G0-BASE-003",
    "REQ-G0-BASE-004",
    "REQ-G0-BASE-005",
    "REQ-G0-BASE-006",
    "REQ-G0-BASE-007",
    "REQ-G0-BASE-008",
    "REQ-G0-BASE-009",
    "REQ-G0-BASE-010",
    "REQ-G0-BASE-011",
    "REQ-G0-BASE-012",
    "REQ-G0-SEC-001",
    "REQ-G0-SEC-002",
    "REQ-G0-SEC-003",
    "REQ-G0-REL-001",
    "REQ-G0-REL-002",
    "REQ-G0-REL-004",
    "REQ-G0-QA-001",
    "REQ-G0-QA-002",
    "REQ-G1-IMG-001",
    "REQ-G1-IMG-002",
    "REQ-G1-IMG-003",
    "REQ-G1-IMG-004",
    "REQ-G1-IMG-005",
    "REQ-G1-IMG-006",
    "REQ-G1-IMG-007",
    "REQ-G1-IMG-008",
    "REQ-G1-IMG-009",
    "REQ-G1-IMG-010",
    "REQ-G1-IMG-011",
    "REQ-G1-BOOT-001",
    "REQ-G1-BOOT-002",
    "REQ-G1-BOOT-003",
    "REQ-G1-BOOT-004",
    "REQ-G1-BOOT-005",
    "REQ-G1-BOOT-006",
    "REQ-G1-BOOT-007",
    "REQ-G1-BOOT-008",
    "REQ-G1-BOOT-009",
    "REQ-G1-BOOT-010",
    "REQ-G1-BOOT-011",
    "REQ-G1-BOOT-012",
    "REQ-G1-BOOT-013",
    "REQ-G1-BOOT-014",
    "REQ-G1-SEC-002",
    "REQ-G1-SEC-005",
    "REQ-G1-SEC-006",
    "REQ-G1-SEC-007",
    "REQ-G1-SEC-008",
    "REQ-G1-REL-002",
    "REQ-G1-REL-003",
    "REQ-G1-REL-007",
    "REQ-G1-QA-003",
    "REQ-G1-QA-004",
    "REQ-G1-QA-005",
    // 21-A G-KERNEL 闸门（K3.2）：端到端 QEMU 启动日志证据。
    "REQ-GK-KERNEL-001",
    // 21-B G-SW1 闸门（A3）：AI 决策链端到端证据（runner 真后端装配 trace）。
    "REQ-GS1-AI-001",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema: String,
    gates: Vec<String>,
    requirements: Vec<Requirement>,
    artifacts: Vec<ArtifactBinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Requirement {
    requirement_id: String,
    task_id: String,
    artifact_role: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactBinding {
    role: String,
    path: String,
}

#[derive(Debug, Serialize)]
struct GateEvidence {
    schema: &'static str,
    integrity_result: &'static str,
    gate_status: &'static str,
    source_commit: String,
    tree_state: &'static str,
    dirty_entry_count: usize,
    requirement_count: usize,
    completed_count: usize,
    open_count: usize,
    blocked_count: usize,
    requirements: Vec<RequirementEvidence>,
    artifacts: Vec<ArtifactEvidence>,
    violations: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RequirementEvidence {
    requirement_id: String,
    task_id: String,
    status: String,
    task_card: String,
    artifact_role: String,
}

#[derive(Debug, Serialize)]
struct ArtifactEvidence {
    role: String,
    path: String,
    present: bool,
    size_bytes: Option<u64>,
    sha256: Option<String>,
}

pub(super) fn command_evidence(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, fault) = match args {
        [mode] if mode == "verify" || mode == "gate" => (mode.as_str(), None),
        [mode, flag, fault] if (mode == "verify" || mode == "gate") && flag == "--fault" => {
            (mode.as_str(), Some(fault.as_str()))
        }
        _ => {
            return Err(invalid(
                "evidence requires verify|gate [--fault missing-requirement|duplicate-requirement|missing-artifact]",
            ));
        }
    };
    let report = verify(root, fault)?;
    let directory = root.join("target/eneros/evidence/g0-g1");
    fs::create_dir_all(&directory)?;
    let output = if fault.is_some() {
        directory.join("gate-coverage-fault.json")
    } else {
        directory.join("gate-coverage.json")
    };
    fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.violations.is_empty() {
        return Err(invalid(
            "requirement/evidence integrity verification failed",
        ));
    }
    if mode == "gate" && report.gate_status != "PASSED" {
        return Err(invalid("G0/G1 Gate remains OPEN or BLOCKED"));
    }
    Ok(())
}

fn verify(root: &Path, fault: Option<&str>) -> DynResult<GateEvidence> {
    let mut registry: Registry = read_json(&root.join("product/evidence/g0-g1-requirements.json"))?;
    match fault {
        None => {}
        Some("missing-requirement") => {
            registry.requirements.pop();
        }
        Some("duplicate-requirement") => {
            let duplicate = registry
                .requirements
                .first()
                .cloned()
                .ok_or_else(|| invalid("registry has no requirement to duplicate"))?;
            registry.requirements.push(duplicate);
        }
        Some("missing-artifact") => {
            let artifact = registry
                .artifacts
                .first_mut()
                .ok_or_else(|| invalid("registry has no artifact to mutate"))?;
            artifact.path = "target/eneros/faults/evidence/missing.bin".to_owned();
        }
        Some(other) => return Err(invalid(format!("unknown evidence fault '{other}'"))),
    }
    let mut violations = Vec::new();
    if registry.schema != "eneros.requirement-registry.v1"
        || registry.gates != ["G0", "G1", "GK", "GS1"]
    {
        violations.push("registry-header".to_owned());
    }
    let expected: BTreeSet<&str> = EXPECTED_REQUIREMENTS.iter().copied().collect();
    let mut observed = BTreeSet::new();
    let mut task_statuses: BTreeMap<String, String> = BTreeMap::new();
    let mut requirements = Vec::new();
    let mut completed = 0usize;
    let mut open = 0usize;
    let mut blocked = 0usize;
    for requirement in &registry.requirements {
        if !observed.insert(requirement.requirement_id.as_str()) {
            violations.push(format!(
                "duplicate-requirement:{}",
                requirement.requirement_id
            ));
        }
        if !requirement.requirement_id.ends_with(&requirement.task_id) {
            violations.push(format!(
                "requirement-task-mismatch:{}",
                requirement.requirement_id
            ));
        }
        let task_path = root
            .join("docs/delivery/g0-g1/task-cards")
            .join(format!("{}.yaml", requirement.task_id));
        let status = match task_statuses.get(&requirement.task_id) {
            Some(status) => status.clone(),
            None => {
                let status = read_task_status(&task_path)?;
                task_statuses.insert(requirement.task_id.clone(), status.clone());
                status
            }
        };
        match status.as_str() {
            "completed" => completed += 1,
            "candidate" => blocked += 1,
            "in-progress" => open += 1,
            other => violations.push(format!(
                "unknown-task-status:{}:{other}",
                requirement.task_id
            )),
        }
        requirements.push(RequirementEvidence {
            requirement_id: requirement.requirement_id.clone(),
            task_id: requirement.task_id.clone(),
            status,
            task_card: relative(root, &task_path)?,
            artifact_role: requirement.artifact_role.clone(),
        });
    }
    for missing in expected.difference(&observed) {
        violations.push(format!("missing-requirement:{missing}"));
    }
    for extra in observed.difference(&expected) {
        violations.push(format!("unexpected-requirement:{extra}"));
    }
    let used_roles: BTreeSet<&str> = registry
        .requirements
        .iter()
        .map(|requirement| requirement.artifact_role.as_str())
        .collect();
    let mut artifact_roles = BTreeSet::new();
    let mut artifacts = Vec::new();
    for binding in &registry.artifacts {
        if !artifact_roles.insert(binding.role.as_str()) {
            violations.push(format!("duplicate-artifact-role:{}", binding.role));
        }
        let relative_path = Path::new(&binding.path);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            violations.push(format!("unsafe-artifact-path:{}", binding.role));
            continue;
        }
        let path = root.join(relative_path);
        let required_now = registry.requirements.iter().any(|requirement| {
            requirement.artifact_role == binding.role
                && task_statuses
                    .get(&requirement.task_id)
                    .is_some_and(|status| status == "completed")
        });
        let present = path.is_file();
        if required_now && !present {
            violations.push(format!("missing-bound-artifact:{}", binding.role));
        }
        let (size_bytes, sha256) = if present {
            let bytes = fs::read(&path)?;
            (
                Some(u64::try_from(bytes.len())?),
                Some(hex(&Sha256::digest(bytes))),
            )
        } else {
            (None, None)
        };
        artifacts.push(ArtifactEvidence {
            role: binding.role.clone(),
            path: binding.path.clone(),
            present,
            size_bytes,
            sha256,
        });
    }
    for role in used_roles.difference(&artifact_roles) {
        violations.push(format!("missing-artifact-role:{role}"));
    }
    let status_output = Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain=v1", "-z"])
        .output()?;
    if !status_output.status.success() {
        return Err(invalid("git status failed during evidence collection"));
    }
    let dirty_entry_count = status_output
        .stdout
        .iter()
        .filter(|byte| **byte == 0)
        .count();
    let commit_output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !commit_output.status.success() {
        return Err(invalid("git rev-parse failed during evidence collection"));
    }
    let source_commit = String::from_utf8(commit_output.stdout)?.trim().to_owned();
    let all_completed = open == 0 && blocked == 0 && violations.is_empty();
    Ok(GateEvidence {
        schema: "eneros.gate-evidence.v1",
        integrity_result: if violations.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        gate_status: if all_completed {
            "PASSED"
        } else if blocked != 0 {
            "BLOCKED"
        } else {
            "OPEN"
        },
        source_commit,
        tree_state: if dirty_entry_count == 0 {
            "clean"
        } else {
            "dirty"
        },
        dirty_entry_count,
        requirement_count: requirements.len(),
        completed_count: completed,
        open_count: open,
        blocked_count: blocked,
        requirements,
        artifacts,
        violations,
    })
}

fn read_task_status(path: &Path) -> DynResult<String> {
    if !path.is_file() {
        return Err(invalid(format!("task card is missing: {}", path.display())));
    }
    let text = fs::read_to_string(path)?;
    let matches = text
        .lines()
        .filter_map(|line| line.strip_prefix("status: "))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(invalid(format!(
            "task card has no unique status: {}",
            path.display()
        )));
    }
    Ok(matches[0].trim_matches('"').to_owned())
}

fn relative(root: &Path, path: &Path) -> DynResult<String> {
    Ok(path
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_requirement_ids_are_unique() {
        let unique: BTreeSet<&str> = EXPECTED_REQUIREMENTS.iter().copied().collect();
        assert_eq!(unique.len(), EXPECTED_REQUIREMENTS.len());
        assert!(EXPECTED_REQUIREMENTS
            .iter()
            .all(|id| id.starts_with("REQ-G0-")
                || id.starts_with("REQ-G1-")
                || id.starts_with("REQ-GK-")
                || id.starts_with("REQ-GS1-")));
    }
}
