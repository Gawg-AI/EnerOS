use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{invalid, load_profile, DynResult, ProfileClass};

#[derive(Debug, Deserialize)]
struct BinaryBoms {
    schema: String,
    source_commit: String,
    cargo_graph_sha256: String,
    binaries: Vec<BinaryBom>,
}

#[derive(Debug, Deserialize)]
struct BinaryBom {
    selector: String,
}

#[derive(Debug, Clone)]
struct BomPackage {
    id: String,
    name: String,
    version: String,
    license: String,
    source: String,
    manifest_path: String,
    workspace_member: bool,
    resolved_features: Vec<String>,
}

#[derive(Debug)]
struct BuildOptions {
    profile: String,
    artifact: PathBuf,
    cyclonedx: PathBuf,
    spdx: PathBuf,
}

#[derive(Debug)]
struct VerifyOptions {
    profile: String,
    artifact: PathBuf,
    cyclonedx: PathBuf,
    spdx: PathBuf,
    fault: Option<String>,
}

#[derive(Debug, Serialize)]
struct SbomReport {
    schema: &'static str,
    result: &'static str,
    profile: String,
    source_commit: String,
    source_date_epoch: u64,
    artifact_path: String,
    artifact_size_bytes: u64,
    artifact_sha256: String,
    cargo_graph_sha256: String,
    profile_audit_sha256: String,
    forbidden_policy_sha256: String,
    binary_selectors: Vec<String>,
    package_count: usize,
    cyclonedx_path: String,
    cyclonedx_sha256: String,
    spdx_path: String,
    spdx_sha256: String,
}

struct ExactProfileClosure {
    selectors: Vec<String>,
    packages: BTreeMap<String, BomPackage>,
    profile_audit_sha256: String,
    forbidden_policy_sha256: String,
}

pub(super) fn command_release(root: &Path, args: &[String]) -> DynResult<()> {
    let Some((mode, tail)) = args.split_first() else {
        return Err(invalid("release requires sbom or verify-sbom"));
    };
    match mode.as_str() {
        "sbom" => build_sbom(root, parse_build_options(tail)?),
        "verify-sbom" => verify_sbom_command(root, parse_verify_options(tail)?),
        other => Err(invalid(format!("unknown release mode '{other}'"))),
    }
}

fn parse_build_options(args: &[String]) -> DynResult<BuildOptions> {
    if args.len() != 8 {
        return Err(invalid(
            "release sbom requires --profile <id> --artifact <file> --cyclonedx <json> --spdx <json>",
        ));
    }
    Ok(BuildOptions {
        profile: value_after(args, "--profile")?.to_owned(),
        artifact: PathBuf::from(value_after(args, "--artifact")?),
        cyclonedx: PathBuf::from(value_after(args, "--cyclonedx")?),
        spdx: PathBuf::from(value_after(args, "--spdx")?),
    })
}

fn parse_verify_options(args: &[String]) -> DynResult<VerifyOptions> {
    let fault = optional_value_after(args, "--fault")?.map(ToOwned::to_owned);
    let expected = if fault.is_some() { 10 } else { 8 };
    if args.len() != expected {
        return Err(invalid(
            "release verify-sbom requires --profile <id> --artifact <file> --cyclonedx <json> --spdx <json> [--fault package-omission|artifact-digest|missing-license]",
        ));
    }
    Ok(VerifyOptions {
        profile: value_after(args, "--profile")?.to_owned(),
        artifact: PathBuf::from(value_after(args, "--artifact")?),
        cyclonedx: PathBuf::from(value_after(args, "--cyclonedx")?),
        spdx: PathBuf::from(value_after(args, "--spdx")?),
        fault,
    })
}

fn build_sbom(root: &Path, options: BuildOptions) -> DynResult<()> {
    let source_date_epoch = source_date_epoch()?;
    let profile = load_profile(root, &options.profile)?;
    if !matches!(
        profile.class,
        ProfileClass::Release | ProfileClass::Recovery
    ) {
        return Err(invalid(
            "SBOM generation requires a release or recovery profile",
        ));
    }
    let artifact_path = target_input(root, &options.artifact, "SBOM artifact")?;
    let cyclonedx_path = target_output(root, &options.cyclonedx, "CycloneDX SBOM")?;
    let spdx_path = target_output(root, &options.spdx, "SPDX SBOM")?;
    let artifact = fs::read(&artifact_path)?;
    let artifact_digest = hex(&Sha256::digest(&artifact));
    let boms: BinaryBoms =
        serde_json::from_slice(&fs::read(root.join("product/closure/binary-boms.json"))?)?;
    let exact = profile_packages(root, &profile, &boms)?;
    let selectors = exact.selectors;
    let packages = exact.packages;
    validate_packages(&packages)?;
    let artifact_name = artifact_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("SBOM artifact filename is not UTF-8"))?;
    let cyclonedx = build_cyclonedx(
        &profile.id,
        artifact_name,
        artifact.len(),
        &artifact_digest,
        source_date_epoch,
        &boms,
        &exact.profile_audit_sha256,
        &exact.forbidden_policy_sha256,
        &selectors,
        &packages,
    );
    let spdx = build_spdx(
        &profile.id,
        artifact_name,
        artifact.len(),
        &artifact_digest,
        source_date_epoch,
        &boms,
        &exact.profile_audit_sha256,
        &exact.forbidden_policy_sha256,
        &packages,
    )?;
    verify_documents(
        &cyclonedx,
        &spdx,
        &profile.id,
        &artifact_digest,
        &exact.profile_audit_sha256,
        &exact.forbidden_policy_sha256,
        &packages,
    )?;
    fs::write(&cyclonedx_path, serde_json::to_vec_pretty(&cyclonedx)?)?;
    fs::write(&spdx_path, serde_json::to_vec_pretty(&spdx)?)?;
    let cyclonedx_bytes = fs::read(&cyclonedx_path)?;
    let spdx_bytes = fs::read(&spdx_path)?;
    let report = SbomReport {
        schema: "eneros.sbom-report.v1",
        result: "PASS",
        profile: profile.id,
        source_commit: boms.source_commit,
        source_date_epoch,
        artifact_path: relative(root, &artifact_path)?,
        artifact_size_bytes: u64::try_from(artifact.len())?,
        artifact_sha256: artifact_digest,
        cargo_graph_sha256: boms.cargo_graph_sha256,
        profile_audit_sha256: exact.profile_audit_sha256,
        forbidden_policy_sha256: exact.forbidden_policy_sha256,
        binary_selectors: selectors,
        package_count: packages.len(),
        cyclonedx_path: relative(root, &cyclonedx_path)?,
        cyclonedx_sha256: hex(&Sha256::digest(cyclonedx_bytes)),
        spdx_path: relative(root, &spdx_path)?,
        spdx_sha256: hex(&Sha256::digest(spdx_bytes)),
    };
    let report_path = cyclonedx_path.with_extension("cdx.report.json");
    fs::write(report_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn verify_sbom_command(root: &Path, options: VerifyOptions) -> DynResult<()> {
    let profile = load_profile(root, &options.profile)?;
    let artifact_path = target_input(root, &options.artifact, "SBOM artifact")?;
    let cyclonedx_path = target_input(root, &options.cyclonedx, "CycloneDX SBOM")?;
    let spdx_path = target_input(root, &options.spdx, "SPDX SBOM")?;
    let artifact_digest = hex(&Sha256::digest(fs::read(artifact_path)?));
    let boms: BinaryBoms =
        serde_json::from_slice(&fs::read(root.join("product/closure/binary-boms.json"))?)?;
    let exact = profile_packages(root, &profile, &boms)?;
    let packages = exact.packages;
    let mut cyclonedx: Value = serde_json::from_slice(&fs::read(cyclonedx_path)?)?;
    let spdx: Value = serde_json::from_slice(&fs::read(spdx_path)?)?;
    match options.fault.as_deref() {
        None => {}
        Some("package-omission") => {
            cyclonedx
                .get_mut("components")
                .and_then(Value::as_array_mut)
                .and_then(|components| components.pop())
                .ok_or_else(|| invalid("CycloneDX has no component to remove"))?;
        }
        Some("artifact-digest") => {
            let digest = cyclonedx
                .pointer("/metadata/component/hashes/0/content")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("CycloneDX artifact digest is absent"))?
                .to_owned();
            let replacement = if digest.starts_with('0') { "1" } else { "0" };
            cyclonedx["metadata"]["component"]["hashes"][0]["content"] =
                Value::String(format!("{replacement}{}", &digest[1..]));
        }
        Some("missing-license") => {
            let component = cyclonedx
                .get_mut("components")
                .and_then(Value::as_array_mut)
                .and_then(|components| components.first_mut())
                .ok_or_else(|| invalid("CycloneDX has no component"))?;
            component
                .as_object_mut()
                .ok_or_else(|| invalid("CycloneDX component is not an object"))?
                .remove("licenses");
        }
        Some(other) => return Err(invalid(format!("unknown SBOM fault '{other}'"))),
    }
    verify_documents(
        &cyclonedx,
        &spdx,
        &profile.id,
        &artifact_digest,
        &exact.profile_audit_sha256,
        &exact.forbidden_policy_sha256,
        &packages,
    )?;
    println!(
        "SBOM verify result=PASS profile={} packages={}",
        profile.id,
        packages.len()
    );
    Ok(())
}

fn profile_packages(
    root: &Path,
    profile: &crate::ProductProfile,
    boms: &BinaryBoms,
) -> DynResult<ExactProfileClosure> {
    if boms.schema != "eneros.delivery.binary-boms.v1" {
        return Err(invalid("unsupported binary BOM schema"));
    }
    let audit = crate::audit_profile(root, &profile.id, None)?;
    if audit.result != "PASS" {
        return Err(invalid(format!(
            "profile '{}' cannot produce an SBOM because its exact audit failed: {}",
            profile.id,
            audit.violations.join("; ")
        )));
    }
    let profile_audit_sha256 = hex(&Sha256::digest(serde_json::to_vec(&audit)?));
    let known_selectors: BTreeSet<&str> = boms
        .binaries
        .iter()
        .map(|binary| binary.selector.as_str())
        .collect();
    let mut selectors = audit
        .selected_binaries
        .iter()
        .map(|binary| binary.selector.clone())
        .collect::<Vec<_>>();
    for selector in &selectors {
        if !known_selectors.contains(selector.as_str()) {
            return Err(invalid(format!(
                "profile binary '{selector}' has no generated BOM metadata"
            )));
        }
    }

    let mut packages = BTreeMap::new();
    for exact in &audit.closure_packages {
        packages.insert(
            exact.id.clone(),
            BomPackage {
                id: exact.id.clone(),
                name: exact.name.clone(),
                version: exact.version.clone(),
                license: exact.license.clone(),
                source: exact.source.clone(),
                manifest_path: exact.manifest_path.clone(),
                workspace_member: exact.workspace_member,
                resolved_features: exact.resolved_features.clone(),
            },
        );
    }
    if packages.len() != audit.closure_package_count {
        return Err(invalid("exact profile closure package count differs"));
    }
    selectors.sort();
    selectors.dedup();
    Ok(ExactProfileClosure {
        selectors,
        packages,
        profile_audit_sha256,
        forbidden_policy_sha256: audit.forbidden_policy_sha256,
    })
}

fn validate_packages(packages: &BTreeMap<String, BomPackage>) -> DynResult<()> {
    if packages.is_empty() {
        return Err(invalid("profile SBOM closure is empty"));
    }
    for package in packages.values() {
        if package.name.trim().is_empty()
            || package.version.trim().is_empty()
            || package.license.trim().is_empty()
            || package.license.eq_ignore_ascii_case("NOASSERTION")
        {
            return Err(invalid(format!(
                "package '{}' has incomplete identity or license",
                package.id
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_cyclonedx(
    profile: &str,
    artifact_name: &str,
    artifact_size: usize,
    artifact_digest: &str,
    source_date_epoch: u64,
    boms: &BinaryBoms,
    profile_audit_sha256: &str,
    forbidden_policy_sha256: &str,
    selectors: &[String],
    packages: &BTreeMap<String, BomPackage>,
) -> Value {
    let components: Vec<Value> = packages
        .values()
        .map(|package| {
            json!({
                "type": "library",
                "bom-ref": purl(package),
                "name": package.name,
                "version": package.version,
                "purl": purl(package),
                "licenses": [{"expression": package.license}],
                "properties": [
                    {"name": "eneros:cargo-package-id", "value": package.id},
                    {"name": "eneros:source", "value": package.source},
                    {"name": "eneros:manifest-path", "value": package.manifest_path},
                    {"name": "eneros:workspace-member", "value": package.workspace_member.to_string()},
                    {"name": "eneros:resolved-features", "value": package.resolved_features.join(",")}
                ]
            })
        })
        .collect();
    let root_ref = format!("pkg:generic/eneros/{profile}@0.192.0?type=raw-image");
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": format!("urn:uuid:{}", uuid_from_digest(artifact_digest)),
        "version": 1,
        "metadata": {
            "timestamp": rfc3339(source_date_epoch),
            "tools": {"components": [{"type":"application","name":"eneros-xtask","version":"0.192.0"}]},
            "component": {
                "type": "operating-system",
                "bom-ref": root_ref,
                "name": artifact_name,
                "version": "0.192.0",
                "hashes": [{"alg":"SHA-256","content":artifact_digest}],
                "properties": [
                    {"name":"eneros:profile","value":profile},
                    {"name":"eneros:artifact-size-bytes","value":artifact_size.to_string()},
                    {"name":"eneros:source-commit","value":boms.source_commit},
                    {"name":"eneros:cargo-graph-sha256","value":boms.cargo_graph_sha256},
                    {"name":"eneros:profile-audit-sha256","value":profile_audit_sha256},
                    {"name":"eneros:forbidden-policy-sha256","value":forbidden_policy_sha256},
                    {"name":"eneros:binary-selectors","value":selectors.join(",")},
                    {"name":"eneros:model-artifacts","value":"not-integrated-in-g1"}
                ]
            }
        },
        "components": components,
        "dependencies": [{"ref": root_ref, "dependsOn": packages.values().map(purl).collect::<Vec<_>>()}]
    })
}

#[allow(clippy::too_many_arguments)]
fn build_spdx(
    profile: &str,
    artifact_name: &str,
    artifact_size: usize,
    artifact_digest: &str,
    source_date_epoch: u64,
    boms: &BinaryBoms,
    profile_audit_sha256: &str,
    forbidden_policy_sha256: &str,
    packages: &BTreeMap<String, BomPackage>,
) -> DynResult<Value> {
    let root_id = "SPDXRef-Package-EnerOS";
    let file_id = "SPDXRef-File-Artifact";
    let mut spdx_packages = vec![json!({
        "name": format!("EnerOS-{profile}"),
        "SPDXID": root_id,
        "versionInfo": "0.192.0",
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": false,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "MIT OR Apache-2.0",
        "copyrightText": "NOASSERTION",
        "externalRefs": [{
            "referenceCategory":"PACKAGE-MANAGER",
            "referenceType":"purl",
            "referenceLocator":format!("pkg:generic/eneros/{profile}@0.192.0")
        }]
    })];
    let mut relationships = vec![
        json!({"spdxElementId":"SPDXRef-DOCUMENT","relationshipType":"DESCRIBES","relatedSpdxElement":root_id}),
        json!({"spdxElementId":root_id,"relationshipType":"CONTAINS","relatedSpdxElement":file_id}),
    ];
    for (index, package) in packages.values().enumerate() {
        let spdx_id = format!("SPDXRef-Package-{}-{index}", sanitize(&package.name));
        spdx_packages.push(json!({
            "name": package.name,
            "SPDXID": spdx_id,
            "versionInfo": package.version,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": false,
            "licenseConcluded": package.license,
            "licenseDeclared": package.license,
            "copyrightText": "NOASSERTION",
            "externalRefs": [{
                "referenceCategory":"PACKAGE-MANAGER",
                "referenceType":"purl",
                "referenceLocator":purl(package)
            }],
            "comment": format!("cargo-id={}; source={}; manifest={}; features={}", package.id, package.source, package.manifest_path, package.resolved_features.join(","))
        }));
        relationships.push(json!({
            "spdxElementId": root_id,
            "relationshipType": "DEPENDS_ON",
            "relatedSpdxElement": spdx_id
        }));
    }
    Ok(json!({
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": format!("EnerOS-{profile}-{artifact_name}"),
        "documentNamespace": format!("https://eneros.invalid/spdx/{}/{}", profile, artifact_digest),
        "creationInfo": {
            "created": rfc3339(source_date_epoch),
            "creators": ["Tool: eneros-xtask-0.192.0"],
            "comment": format!("source_commit={}; cargo_graph_sha256={}; profile_audit_sha256={profile_audit_sha256}; forbidden_policy_sha256={forbidden_policy_sha256}; artifact_size={artifact_size}", boms.source_commit, boms.cargo_graph_sha256)
        },
        "packages": spdx_packages,
        "files": [{
            "fileName": artifact_name,
            "SPDXID": file_id,
            "checksums": [{"algorithm":"SHA256","checksumValue":artifact_digest}],
            "licenseConcluded": "NOASSERTION",
            "copyrightText": "NOASSERTION"
        }],
        "relationships": relationships,
        "annotations": [{
            "annotationDate": rfc3339(source_date_epoch),
            "annotationType": "OTHER",
            "annotator": "Tool: eneros-xtask-0.192.0",
            "comment": "G1 contains no integrated model artifacts; model inventory is intentionally empty."
        }]
    }))
}

fn verify_documents(
    cyclonedx: &Value,
    spdx: &Value,
    profile: &str,
    artifact_digest: &str,
    profile_audit_sha256: &str,
    forbidden_policy_sha256: &str,
    packages: &BTreeMap<String, BomPackage>,
) -> DynResult<()> {
    validate_packages(packages)?;
    if cyclonedx.get("bomFormat").and_then(Value::as_str) != Some("CycloneDX")
        || cyclonedx.get("specVersion").and_then(Value::as_str) != Some("1.6")
        || cyclonedx
            .pointer("/metadata/component/hashes/0/content")
            .and_then(Value::as_str)
            != Some(artifact_digest)
        || component_property(cyclonedx, "eneros:profile") != Some(profile)
        || component_property(cyclonedx, "eneros:profile-audit-sha256")
            != Some(profile_audit_sha256)
        || component_property(cyclonedx, "eneros:forbidden-policy-sha256")
            != Some(forbidden_policy_sha256)
    {
        return Err(invalid("CycloneDX metadata or artifact binding differs"));
    }
    let components = cyclonedx
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("CycloneDX components are missing"))?;
    let mut observed = BTreeSet::new();
    for component in components {
        let purl = component
            .get("purl")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("CycloneDX component purl is missing"))?;
        let licenses = component
            .get("licenses")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(format!("CycloneDX component '{purl}' has no license")))?;
        if licenses.is_empty() || !observed.insert(purl.to_owned()) {
            return Err(invalid("CycloneDX license or uniqueness check failed"));
        }
    }
    let expected: BTreeSet<String> = packages.values().map(purl).collect();
    if observed != expected {
        return Err(invalid(
            "CycloneDX package set differs from profile closure",
        ));
    }
    let spdx_creation_comment = spdx
        .pointer("/creationInfo/comment")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("SPDX creation binding is missing"))?;
    if spdx.get("spdxVersion").and_then(Value::as_str) != Some("SPDX-2.3")
        || spdx
            .pointer("/files/0/checksums/0/checksumValue")
            .and_then(Value::as_str)
            != Some(artifact_digest)
        || !spdx_creation_comment.contains(&format!("profile_audit_sha256={profile_audit_sha256}"))
        || !spdx_creation_comment.contains(&format!(
            "forbidden_policy_sha256={forbidden_policy_sha256}"
        ))
    {
        return Err(invalid("SPDX metadata or artifact binding differs"));
    }
    let spdx_packages = spdx
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("SPDX packages are missing"))?;
    let mut spdx_purls = BTreeSet::new();
    for package in spdx_packages.iter().skip(1) {
        let declared = package
            .get("licenseDeclared")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("SPDX package license is missing"))?;
        if declared.is_empty() || declared == "NOASSERTION" {
            return Err(invalid("SPDX dependency has unknown license"));
        }
        let purl = package
            .pointer("/externalRefs/0/referenceLocator")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("SPDX package purl is missing"))?;
        if !spdx_purls.insert(purl.to_owned()) {
            return Err(invalid("SPDX package purl is duplicated"));
        }
    }
    if spdx_purls != expected {
        return Err(invalid("SPDX package set differs from profile closure"));
    }
    Ok(())
}

fn component_property<'a>(document: &'a Value, name: &str) -> Option<&'a str> {
    document
        .pointer("/metadata/component/properties")?
        .as_array()?
        .iter()
        .find(|property| property.get("name").and_then(Value::as_str) == Some(name))?
        .get("value")?
        .as_str()
}

fn source_date_epoch() -> DynResult<u64> {
    let value = env::var("SOURCE_DATE_EPOCH")
        .map_err(|_| invalid("SOURCE_DATE_EPOCH is mandatory for SBOM generation"))?;
    let epoch = value
        .parse::<u64>()
        .map_err(|_| invalid("SOURCE_DATE_EPOCH must be an unsigned integer"))?;
    if epoch == 0 {
        return Err(invalid("SOURCE_DATE_EPOCH must be nonzero"));
    }
    Ok(epoch)
}

fn purl(package: &BomPackage) -> String {
    format!("pkg:cargo/{}@{}", package.name, package.version)
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn uuid_from_digest(digest: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

fn rfc3339(epoch: u64) -> String {
    let days = i64::try_from(epoch / 86_400).unwrap_or(i64::MAX);
    let seconds = epoch % 86_400;
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn value_after<'a>(args: &'a [String], flag: &str) -> DynResult<&'a str> {
    let values: Vec<&str> = args
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
        .collect();
    match values.as_slice() {
        [value] => Ok(value),
        [] => Err(invalid(format!("{flag} is mandatory"))),
        _ => Err(invalid(format!("{flag} may only appear once"))),
    }
}

fn optional_value_after<'a>(args: &'a [String], flag: &str) -> DynResult<Option<&'a str>> {
    let values: Vec<&str> = args
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
        .collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(value)),
        _ => Err(invalid(format!("{flag} may only appear once"))),
    }
}

fn target_input(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !candidate.starts_with(&target) || !candidate.is_file() {
        return Err(invalid(format!(
            "{label} must be an existing file below target"
        )));
    }
    Ok(candidate)
}

fn target_output(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| invalid(format!("{label} has no parent")))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !parent.starts_with(&target) {
        return Err(invalid(format!("{label} output must stay below target")));
    }
    Ok(parent.join(
        candidate
            .file_name()
            .ok_or_else(|| invalid(format!("{label} has no filename")))?,
    ))
}

fn relative(root: &Path, path: &Path) -> DynResult<String> {
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    Ok(canonical_path
        .strip_prefix(canonical_root)?
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
    fn source_epoch_format_is_stable() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_785_465_257), "2026-07-31T02:34:17Z");
    }

    #[test]
    fn package_identity_helpers_are_deterministic() {
        let package = BomPackage {
            id: "id".to_owned(),
            name: "eneros-example".to_owned(),
            version: "1.2.3".to_owned(),
            license: "MIT".to_owned(),
            source: "workspace-or-path".to_owned(),
            manifest_path: "crates/example/Cargo.toml".to_owned(),
            workspace_member: true,
            resolved_features: vec!["default".to_owned()],
        };
        assert_eq!(purl(&package), "pkg:cargo/eneros-example@1.2.3");
        assert_eq!(sanitize("a/b:c"), "a-b-c");
    }
}
