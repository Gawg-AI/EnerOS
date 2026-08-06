use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{invalid, read_json, DynResult};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedTools {
    schema: String,
    rust_channel: String,
    host_tools: Vec<HostTool>,
    wsl_distribution: String,
    wsl_tools: Vec<WslTool>,
    wsl_materials: Vec<WslMaterial>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostTool {
    name: String,
    version_contains: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WslTool {
    name: String,
    path: String,
    version_contains: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WslMaterial {
    name: String,
    path: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct ToolchainReport {
    schema: &'static str,
    result: &'static str,
    rust_channel: String,
    tools: Vec<ToolEvidence>,
    materials: Vec<MaterialEvidence>,
    violations: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ToolEvidence {
    name: String,
    environment: &'static str,
    path: String,
    version: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct MaterialEvidence {
    name: String,
    environment: &'static str,
    path: String,
    size_bytes: u64,
    sha256: String,
}

pub(super) fn command_audit_toolchain(root: &Path, args: &[String]) -> DynResult<()> {
    let fault = match args {
        [] => None,
        [flag, tool] if flag == "--fault-inject-tool" => Some(tool.as_str()),
        _ => {
            return Err(invalid(
                "audit toolchain accepts only [--fault-inject-tool <name>]",
            ));
        }
    };
    let manifest: PinnedTools = read_json(&root.join("product/toolchains/pinned-tools.json"))?;
    if manifest.schema != "eneros.pinned-tools.v1"
        || manifest.rust_channel != "nightly-2026-04-04"
        || manifest.host_tools.is_empty()
        || manifest.wsl_tools.is_empty()
        || manifest.wsl_materials.is_empty()
    {
        return Err(invalid("pinned tool manifest header is invalid"));
    }
    let mut tools = Vec::new();
    let mut violations = Vec::new();
    for expected in manifest.host_tools {
        let which = Command::new("rustup")
            .args([
                "which",
                &expected.name,
                "--toolchain",
                &manifest.rust_channel,
            ])
            .output()?;
        if !which.status.success() {
            violations.push(format!("missing-host-tool:{}", expected.name));
            continue;
        }
        let path = String::from_utf8(which.stdout)?.trim().to_owned();
        let version_output = Command::new(&path).arg("--version").output()?;
        if !version_output.status.success() {
            violations.push(format!("host-version-failed:{}", expected.name));
            continue;
        }
        let version = combined_output(&version_output)?;
        let digest = hex(&Sha256::digest(fs::read(&path)?));
        let expected_hash = if fault == Some(expected.name.as_str()) {
            "00"
        } else {
            expected.sha256.as_str()
        };
        if !version.contains(&expected.version_contains) {
            violations.push(format!("host-version-mismatch:{}", expected.name));
        }
        if digest != expected_hash {
            violations.push(format!("host-digest-mismatch:{}", expected.name));
        }
        tools.push(ToolEvidence {
            name: expected.name,
            environment: "host",
            path,
            version,
            sha256: digest,
        });
    }
    for expected in manifest.wsl_tools {
        let version_args = if expected.name == "xorriso" {
            vec!["-version"]
        } else {
            vec!["--version"]
        };
        let version_output = Command::new("wsl.exe")
            .args(["-d", &manifest.wsl_distribution, "--", &expected.path])
            .args(version_args)
            .output()?;
        if !version_output.status.success() {
            violations.push(format!("wsl-version-failed:{}", expected.name));
            continue;
        }
        let version = combined_output(&version_output)?;
        let hash_output = Command::new("wsl.exe")
            .args([
                "-d",
                &manifest.wsl_distribution,
                "--",
                "sha256sum",
                &expected.path,
            ])
            .output()?;
        if !hash_output.status.success() {
            violations.push(format!("wsl-digest-failed:{}", expected.name));
            continue;
        }
        let digest = String::from_utf8_lossy(&hash_output.stdout)
            .split_ascii_whitespace()
            .next()
            .ok_or_else(|| invalid("sha256sum returned no digest"))?
            .to_owned();
        let expected_hash = if fault == Some(expected.name.as_str()) {
            "00"
        } else {
            expected.sha256.as_str()
        };
        if !version.contains(&expected.version_contains) {
            violations.push(format!("wsl-version-mismatch:{}", expected.name));
        }
        if digest != expected_hash {
            violations.push(format!("wsl-digest-mismatch:{}", expected.name));
        }
        tools.push(ToolEvidence {
            name: expected.name,
            environment: "wsl",
            path: expected.path,
            version,
            sha256: digest,
        });
    }
    let mut materials = Vec::new();
    for expected in manifest.wsl_materials {
        let (digest, size_bytes) = if cfg!(windows) {
            let output = Command::new("wsl.exe")
                .args([
                    "-d",
                    &manifest.wsl_distribution,
                    "--",
                    "sha256sum",
                    &expected.path,
                ])
                .output()?;
            if !output.status.success() {
                violations.push(format!("wsl-material-digest-failed:{}", expected.name));
                continue;
            }
            let digest = String::from_utf8_lossy(&output.stdout)
                .split_ascii_whitespace()
                .next()
                .ok_or_else(|| invalid("sha256sum returned no material digest"))?
                .to_owned();
            let size = Command::new("wsl.exe")
                .args([
                    "-d",
                    &manifest.wsl_distribution,
                    "--",
                    "stat",
                    "-Lc",
                    "%s",
                    &expected.path,
                ])
                .output()?;
            if !size.status.success() {
                violations.push(format!("wsl-material-size-failed:{}", expected.name));
                continue;
            }
            let size_bytes = String::from_utf8(size.stdout)?
                .trim()
                .parse::<u64>()
                .map_err(|_| invalid("material size is not an unsigned integer"))?;
            (digest, size_bytes)
        } else {
            let bytes = fs::read(&expected.path)?;
            (hex(&Sha256::digest(&bytes)), u64::try_from(bytes.len())?)
        };
        let expected_hash = if fault == Some(expected.name.as_str()) {
            "00"
        } else {
            expected.sha256.as_str()
        };
        if digest != expected_hash {
            violations.push(format!("wsl-material-digest-mismatch:{}", expected.name));
        }
        materials.push(MaterialEvidence {
            name: expected.name,
            environment: if cfg!(windows) { "wsl" } else { "host" },
            path: expected.path,
            size_bytes,
            sha256: digest,
        });
    }
    if let Some(tool) = fault {
        let known = tools.iter().any(|evidence| evidence.name == tool)
            || materials.iter().any(|evidence| evidence.name == tool);
        if !known {
            return Err(invalid(format!("unknown fault-injected tool '{tool}'")));
        }
    }
    let report = ToolchainReport {
        schema: "eneros.toolchain-audit.v1",
        result: if violations.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        rust_channel: manifest.rust_channel,
        tools,
        materials,
        violations,
    };
    let directory = root.join("target/eneros/evidence/REL-001");
    fs::create_dir_all(&directory)?;
    let path = if fault.is_some() {
        directory.join("toolchain-fault-report.json")
    } else {
        directory.join("toolchain-report.json")
    };
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.violations.is_empty() {
        Ok(())
    } else {
        Err(invalid("pinned toolchain verification failed"))
    }
}

fn combined_output(output: &std::process::Output) -> DynResult<String> {
    let mut bytes = output.stdout.clone();
    bytes.extend_from_slice(&output.stderr);
    Ok(String::from_utf8_lossy(&bytes)
        .replace('\0', "")
        .trim()
        .to_owned())
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
