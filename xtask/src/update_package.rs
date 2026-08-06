use std::fs::{self, File};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use eneros_boot_protocol::{
    ManifestSlot, ReleaseManifest, UpdateManifest, UpdatePayloadKind, UPDATE_MANIFEST_SIGNED_SIZE,
    UPDATE_MANIFEST_SIZE,
};
use eneros_image_format::parse_gpt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{invalid, load_profile, read_json, DynResult, ImageLayout, ProfileClass};

const DELTA_MAGIC: [u8; 8] = *b"ENRDLT01";
const DELTA_VERSION: u16 = 1;
const DELTA_HEADER_SIZE: usize = 40;
const DELTA_RECORD_HEADER_SIZE: usize = 16;
const DELTA_BLOCK_SIZE: usize = 4096;

#[derive(Debug)]
struct BuildOptions {
    profile: String,
    kind: UpdatePayloadKind,
    base: PathBuf,
    target: PathBuf,
    signing_key: PathBuf,
    output: PathBuf,
}

#[derive(Debug)]
struct InspectOptions {
    package: PathBuf,
    profile: String,
    base: PathBuf,
    target: PathBuf,
    public_key: PathBuf,
    report: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct RawIdentity {
    release_id: [u8; 16],
    security_version: u64,
    source_date_epoch: u64,
    length: u64,
    digest: [u8; 32],
}

#[derive(Debug, Serialize)]
struct UpdateReport {
    schema: &'static str,
    result: &'static str,
    payload_kind: &'static str,
    package_path: String,
    package_size_bytes: u64,
    package_sha256: String,
    manifest_sha256: String,
    platform_id: String,
    key_id: String,
    update_id: String,
    base_release_id: String,
    target_release_id: String,
    base_security_version: u64,
    target_security_version: u64,
    payload_size_bytes: u64,
    payload_sha256: String,
    base_image_sha256: String,
    target_image_sha256: String,
    signature: &'static str,
    base_binding: &'static str,
    target_binding: &'static str,
}

pub(super) fn command_image_update(root: &Path, args: &[String]) -> DynResult<()> {
    build_update(root, parse_build_options(args)?)
}

pub(super) fn command_inspect_update(root: &Path, args: &[String]) -> DynResult<()> {
    inspect_update(root, parse_inspect_options(args)?)
}

fn parse_build_options(args: &[String]) -> DynResult<BuildOptions> {
    let profile = args
        .first()
        .ok_or_else(|| invalid("image update requires an explicit profile"))?
        .clone();
    let mut kind = None;
    let mut base = None;
    let mut target = None;
    let mut signing_key = None;
    let mut output = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--kind" => {
                kind = Some(match value.as_str() {
                    "full" => UpdatePayloadKind::Full,
                    "delta" => UpdatePayloadKind::Delta,
                    other => return Err(invalid(format!("unsupported update kind '{other}'"))),
                })
            }
            "--base" => base = Some(PathBuf::from(value)),
            "--target" => target = Some(PathBuf::from(value)),
            "--signing-key" => signing_key = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown image update option '{flag}'"))),
        }
        index += 2;
    }
    let kind = kind.ok_or_else(|| invalid("--kind full|delta is mandatory"))?;
    Ok(BuildOptions {
        profile,
        kind,
        base: base.ok_or_else(|| invalid("--base is mandatory"))?,
        target: target.ok_or_else(|| invalid("--target is mandatory"))?,
        signing_key: signing_key.ok_or_else(|| invalid("--signing-key is mandatory"))?,
        output: output.ok_or_else(|| invalid("--output is mandatory"))?,
    })
}

fn parse_inspect_options(args: &[String]) -> DynResult<InspectOptions> {
    let package = args
        .first()
        .ok_or_else(|| invalid("inspect update requires a package path"))?;
    let mut profile = None;
    let mut base = None;
    let mut target = None;
    let mut public_key = None;
    let mut report = None;
    let mut index = 1usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--profile" => profile = Some(value.clone()),
            "--base" => base = Some(PathBuf::from(value)),
            "--target" => target = Some(PathBuf::from(value)),
            "--release-public-key" => public_key = Some(PathBuf::from(value)),
            "--report" => report = Some(PathBuf::from(value)),
            _ => return Err(invalid(format!("unknown inspect update option '{flag}'"))),
        }
        index += 2;
    }
    Ok(InspectOptions {
        package: PathBuf::from(package),
        profile: profile.ok_or_else(|| invalid("--profile is mandatory"))?,
        base: base.ok_or_else(|| invalid("--base is mandatory"))?,
        target: target.ok_or_else(|| invalid("--target is mandatory"))?,
        public_key: public_key.ok_or_else(|| invalid("--release-public-key is mandatory"))?,
        report,
    })
}

fn build_update(root: &Path, options: BuildOptions) -> DynResult<()> {
    let profile = load_profile(root, &options.profile)?;
    if profile.class != ProfileClass::Release
        || !profile.signing.required
        || profile.signing.allow_test_keys
    {
        return Err(invalid(
            "update packages require a non-test release profile",
        ));
    }
    let layout = load_layout(root, &profile)?;
    let base_path = target_input(root, &options.base, "base image")?;
    let target_path = target_input(root, &options.target, "target image")?;
    let output = target_output(root, &options.output, "update package")?;
    let key_bytes = fs::read(&options.signing_key)?;
    let secret: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("release signing seed must be exactly 32 bytes"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let verifying_key = signing_key.verifying_key();
    let public_key = verifying_key.to_bytes();
    let key_id = first_16(Sha256::digest(public_key));
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));

    let base_bytes = fs::read(&base_path)?;
    let target_bytes = fs::read(&target_path)?;
    let base = verified_raw_identity(
        &base_bytes,
        layout.sector_size,
        &verifying_key,
        platform_id,
        key_id,
    )?;
    let target = verified_raw_identity(
        &target_bytes,
        layout.sector_size,
        &verifying_key,
        platform_id,
        key_id,
    )?;
    if target.security_version <= base.security_version {
        return Err(invalid(format!(
            "update target security version {} is not newer than base {}",
            target.security_version, base.security_version
        )));
    }
    if target.source_date_epoch < base.source_date_epoch {
        return Err(invalid("update target epoch predates the base release"));
    }
    let payload = match options.kind {
        UpdatePayloadKind::Full => target_bytes.clone(),
        UpdatePayloadKind::Delta => build_delta(&base_bytes, &target_bytes)?,
    };
    let payload_digest: [u8; 32] = Sha256::digest(&payload).into();
    let update_id = derive_update_id(options.kind, &base, &target, payload.len(), payload_digest);
    let unsigned_manifest = UpdateManifest::new(
        options.kind,
        platform_id,
        key_id,
        base.release_id,
        target.release_id,
        base.security_version,
        target.security_version,
        target.source_date_epoch,
        u64::try_from(payload.len())?,
        base.length,
        target.length,
        base.digest,
        target.digest,
        payload_digest,
        update_id,
    );
    let unsigned = unsigned_manifest
        .encode_unsigned()
        .map_err(|error| invalid(format!("update manifest encode: {error:?}")))?;
    let manifest = unsigned_manifest.with_signature(signing_key.sign(&unsigned).to_bytes());
    let encoded = manifest
        .encode_signed()
        .map_err(|error| invalid(format!("signed update manifest encode: {error:?}")))?;
    let mut package = Vec::with_capacity(encoded.len() + payload.len());
    package.extend_from_slice(&encoded);
    package.extend_from_slice(&payload);
    write_synced(&output, &package)?;
    let report = verify_package(
        root,
        &output,
        &package,
        &base_bytes,
        &target_bytes,
        &layout,
        &verifying_key,
        platform_id,
        key_id,
    )?;
    fs::write(
        output.with_extension("enupd.report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn inspect_update(root: &Path, options: InspectOptions) -> DynResult<()> {
    let profile = load_profile(root, &options.profile)?;
    if profile.class != ProfileClass::Release {
        return Err(invalid("update inspection requires a release profile"));
    }
    let layout = load_layout(root, &profile)?;
    let package_path = target_input(root, &options.package, "update package")?;
    let base_path = target_input(root, &options.base, "base image")?;
    let target_path = target_input(root, &options.target, "target image")?;
    let package = fs::read(&package_path)?;
    let base = fs::read(&base_path)?;
    let target = fs::read(&target_path)?;
    let public_key_bytes = fs::read(&options.public_key)?;
    let public_key: [u8; 32] = public_key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("release public key must be exactly 32 bytes"))?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)?;
    let key_id = first_16(Sha256::digest(public_key));
    let platform_id = first_16(Sha256::digest(profile.platform.as_bytes()));
    let report = verify_package(
        root,
        &package_path,
        &package,
        &base,
        &target,
        &layout,
        &verifying_key,
        platform_id,
        key_id,
    )?;
    let report_path = match options.report {
        Some(path) => target_output(root, &path, "update inspection report")?,
        None => package_path.with_extension("enupd.inspect.json"),
    };
    fs::write(report_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_package(
    root: &Path,
    package_path: &Path,
    package: &[u8],
    base: &[u8],
    target: &[u8],
    layout: &ImageLayout,
    verifying_key: &VerifyingKey,
    platform_id: [u8; 16],
    key_id: [u8; 16],
) -> DynResult<UpdateReport> {
    let encoded_manifest = package
        .get(..UPDATE_MANIFEST_SIZE)
        .ok_or_else(|| invalid("update package manifest is truncated"))?;
    let manifest = UpdateManifest::decode(encoded_manifest)
        .map_err(|error| invalid(format!("update manifest decode: {error:?}")))?;
    if manifest.platform_id != platform_id || manifest.key_id != key_id {
        return Err(invalid(
            "update package platform or release key binding differs",
        ));
    }
    verifying_key
        .verify(
            &encoded_manifest[..UPDATE_MANIFEST_SIGNED_SIZE],
            &Signature::from_bytes(&manifest.signature),
        )
        .map_err(|_| invalid("update package manifest signature is invalid"))?;
    let payload = &package[UPDATE_MANIFEST_SIZE..];
    if u64::try_from(payload.len())? != manifest.payload_length
        || Sha256::digest(payload).as_slice() != manifest.payload_digest
    {
        return Err(invalid("update package payload length or digest differs"));
    }
    let base_identity =
        verified_raw_identity(base, layout.sector_size, verifying_key, platform_id, key_id)?;
    let target_identity = verified_raw_identity(
        target,
        layout.sector_size,
        verifying_key,
        platform_id,
        key_id,
    )?;
    if base_identity.release_id != manifest.base_release_id
        || base_identity.security_version != manifest.base_security_version
        || base_identity.length != manifest.base_image_length
        || base_identity.digest != manifest.base_image_digest
    {
        return Err(invalid(
            "update package does not bind the supplied base image",
        ));
    }
    if target_identity.release_id != manifest.target_release_id
        || target_identity.security_version != manifest.target_security_version
        || target_identity.length != manifest.target_image_length
        || target_identity.digest != manifest.target_image_digest
    {
        return Err(invalid(
            "update package does not bind the supplied target image",
        ));
    }
    if manifest.payload_kind == UpdatePayloadKind::Full && payload != target {
        return Err(invalid(
            "full update payload is not byte-identical to target image",
        ));
    }
    if manifest.payload_kind == UpdatePayloadKind::Delta {
        let reconstructed = apply_delta(base, payload)?;
        if reconstructed != target || build_delta(base, target)? != payload {
            return Err(invalid(
                "delta payload does not canonically reconstruct the target image",
            ));
        }
    }
    Ok(UpdateReport {
        schema: "eneros.update-inspection.v1",
        result: "PASS",
        payload_kind: kind_name(manifest.payload_kind),
        package_path: relative(root, package_path)?,
        package_size_bytes: u64::try_from(package.len())?,
        package_sha256: hex(&Sha256::digest(package)),
        manifest_sha256: hex(&Sha256::digest(encoded_manifest)),
        platform_id: hex(&manifest.platform_id),
        key_id: hex(&manifest.key_id),
        update_id: hex(&manifest.update_id),
        base_release_id: hex(&manifest.base_release_id),
        target_release_id: hex(&manifest.target_release_id),
        base_security_version: manifest.base_security_version,
        target_security_version: manifest.target_security_version,
        payload_size_bytes: manifest.payload_length,
        payload_sha256: hex(&manifest.payload_digest),
        base_image_sha256: hex(&manifest.base_image_digest),
        target_image_sha256: hex(&manifest.target_image_digest),
        signature: "PASS",
        base_binding: "PASS",
        target_binding: "PASS",
    })
}

fn verified_raw_identity(
    image: &[u8],
    sector_size: u32,
    verifying_key: &VerifyingKey,
    platform_id: [u8; 16],
    key_id: [u8; 16],
) -> DynResult<RawIdentity> {
    let gpt = parse_gpt(image, sector_size)
        .map_err(|error| invalid(format!("update input GPT is invalid: {error:?}")))?;
    let expected_slots = [
        ("BOOT_A", ManifestSlot::A),
        ("BOOT_B", ManifestSlot::B),
        ("RECOVERY", ManifestSlot::Recovery),
    ];
    let mut release_id = None;
    let mut security_version = None;
    let mut source_date_epoch = None;
    for (partition, slot) in expected_slots {
        let encoded = crate::image_factory::extract_gpt_manifest_bytes(image, &gpt, partition)?;
        let manifest = ReleaseManifest::decode(&encoded)
            .map_err(|error| invalid(format!("{partition} manifest decode: {error:?}")))?;
        if manifest.header.platform_id != platform_id
            || manifest.header.key_id != key_id
            || manifest
                .components
                .iter()
                .any(|component| component.slot != slot)
        {
            return Err(invalid(format!(
                "{partition} manifest platform, key, or slot binding differs"
            )));
        }
        let signed = ReleaseManifest::signed_region(&encoded)
            .map_err(|error| invalid(format!("{partition} signed region: {error:?}")))?;
        verifying_key
            .verify(signed, &Signature::from_bytes(&manifest.signature))
            .map_err(|_| invalid(format!("{partition} manifest signature is invalid")))?;
        if release_id.is_some_and(|value| value != manifest.header.release_id)
            || security_version.is_some_and(|value| value != manifest.header.security_version)
            || source_date_epoch.is_some_and(|value| value != manifest.header.source_date_epoch)
        {
            return Err(invalid("raw image release manifests disagree"));
        }
        release_id = Some(manifest.header.release_id);
        security_version = Some(manifest.header.security_version);
        source_date_epoch = Some(manifest.header.source_date_epoch);
    }
    Ok(RawIdentity {
        release_id: release_id.ok_or_else(|| invalid("raw image has no release identity"))?,
        security_version: security_version
            .ok_or_else(|| invalid("raw image has no security version"))?,
        source_date_epoch: source_date_epoch
            .ok_or_else(|| invalid("raw image has no release epoch"))?,
        length: u64::try_from(image.len())?,
        digest: Sha256::digest(image).into(),
    })
}

fn derive_update_id(
    kind: UpdatePayloadKind,
    base: &RawIdentity,
    target: &RawIdentity,
    payload_length: usize,
    payload_digest: [u8; 32],
) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update(b"EnerOS/update-id/v1");
    hash.update([kind as u8]);
    hash.update(base.release_id);
    hash.update(target.release_id);
    hash.update(base.security_version.to_le_bytes());
    hash.update(target.security_version.to_le_bytes());
    hash.update((payload_length as u64).to_le_bytes());
    hash.update(payload_digest);
    first_16(hash.finalize())
}

fn build_delta(base: &[u8], target: &[u8]) -> DynResult<Vec<u8>> {
    let block_count = target.len().div_ceil(DELTA_BLOCK_SIZE);
    let mut changed = Vec::new();
    for block in 0..block_count {
        let start = block
            .checked_mul(DELTA_BLOCK_SIZE)
            .ok_or_else(|| invalid("delta block offset overflow"))?;
        let end = target.len().min(
            start
                .checked_add(DELTA_BLOCK_SIZE)
                .ok_or_else(|| invalid("delta block end overflow"))?,
        );
        let target_block = &target[start..end];
        let base_block = base.get(start..end).unwrap_or(&[]);
        if base_block != target_block {
            changed.push((start, target_block));
        }
    }
    let mut delta = Vec::new();
    delta.extend_from_slice(&DELTA_MAGIC);
    delta.extend_from_slice(&DELTA_VERSION.to_le_bytes());
    delta.extend_from_slice(&(DELTA_HEADER_SIZE as u16).to_le_bytes());
    delta.extend_from_slice(&(DELTA_BLOCK_SIZE as u32).to_le_bytes());
    delta.extend_from_slice(&u64::try_from(base.len())?.to_le_bytes());
    delta.extend_from_slice(&u64::try_from(target.len())?.to_le_bytes());
    delta.extend_from_slice(&u32::try_from(changed.len())?.to_le_bytes());
    delta.extend_from_slice(&0u32.to_le_bytes());
    for (offset, bytes) in changed {
        delta.extend_from_slice(&u64::try_from(offset)?.to_le_bytes());
        delta.extend_from_slice(&u32::try_from(bytes.len())?.to_le_bytes());
        delta.extend_from_slice(&0u32.to_le_bytes());
        delta.extend_from_slice(bytes);
    }
    Ok(delta)
}

fn apply_delta(base: &[u8], delta: &[u8]) -> DynResult<Vec<u8>> {
    if delta.len() < DELTA_HEADER_SIZE || delta[..8] != DELTA_MAGIC {
        return Err(invalid("delta header is truncated or has wrong magic"));
    }
    if read_u16(delta, 8)? != DELTA_VERSION
        || usize::from(read_u16(delta, 10)?) != DELTA_HEADER_SIZE
        || usize::try_from(read_u32(delta, 12)?)? != DELTA_BLOCK_SIZE
        || read_u32(delta, 36)? != 0
    {
        return Err(invalid(
            "delta header version, geometry, or reserved field differs",
        ));
    }
    let base_length = usize::try_from(read_u64(delta, 16)?)?;
    let target_length = usize::try_from(read_u64(delta, 24)?)?;
    let record_count = usize::try_from(read_u32(delta, 32)?)?;
    if base_length != base.len() || target_length == 0 {
        return Err(invalid("delta base or target length differs"));
    }
    let mut output = base.to_vec();
    output.resize(target_length, 0);
    let mut cursor = DELTA_HEADER_SIZE;
    let mut previous_offset = None;
    for _ in 0..record_count {
        let header_end = cursor
            .checked_add(DELTA_RECORD_HEADER_SIZE)
            .ok_or_else(|| invalid("delta record header overflow"))?;
        if header_end > delta.len() {
            return Err(invalid("delta record header is truncated"));
        }
        let offset = usize::try_from(read_u64(delta, cursor)?)?;
        let length = usize::try_from(read_u32(delta, cursor + 8)?)?;
        if read_u32(delta, cursor + 12)? != 0
            || offset % DELTA_BLOCK_SIZE != 0
            || length == 0
            || length > DELTA_BLOCK_SIZE
            || previous_offset.is_some_and(|previous| offset <= previous)
        {
            return Err(invalid("delta record is noncanonical"));
        }
        let expected_length = DELTA_BLOCK_SIZE.min(
            target_length
                .checked_sub(offset)
                .ok_or_else(|| invalid("delta record starts after target"))?,
        );
        if length != expected_length {
            return Err(invalid(
                "delta record length is not canonical for its offset",
            ));
        }
        cursor = header_end;
        let data_end = cursor
            .checked_add(length)
            .ok_or_else(|| invalid("delta record data overflow"))?;
        let data = delta
            .get(cursor..data_end)
            .ok_or_else(|| invalid("delta record data is truncated"))?;
        let output_end = offset
            .checked_add(length)
            .ok_or_else(|| invalid("delta target range overflow"))?;
        output
            .get_mut(offset..output_end)
            .ok_or_else(|| invalid("delta target range is outside target"))?
            .copy_from_slice(data);
        cursor = data_end;
        previous_offset = Some(offset);
    }
    if cursor != delta.len() {
        return Err(invalid(
            "delta has trailing bytes or incorrect record count",
        ));
    }
    Ok(output)
}

fn read_u16(bytes: &[u8], offset: usize) -> DynResult<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| invalid("delta u16 is truncated"))?
            .try_into()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> DynResult<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("delta u32 is truncated"))?
            .try_into()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> DynResult<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or_else(|| invalid("delta u64 is truncated"))?
            .try_into()?,
    ))
}

fn load_layout(root: &Path, profile: &crate::ProductProfile) -> DynResult<ImageLayout> {
    let layout_id = profile
        .image_layout
        .as_deref()
        .ok_or_else(|| invalid("update profile has no image_layout"))?;
    read_json(
        &root
            .join("product/image-layouts")
            .join(format!("{layout_id}.json")),
    )
}

fn target_input(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let candidate = candidate.canonicalize()?;
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

fn write_synced(path: &Path, bytes: &[u8]) -> DynResult<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if fs::metadata(path)?.len() != u64::try_from(bytes.len())? {
        return Err(invalid("update package short write"));
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> DynResult<String> {
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    Ok(canonical_path
        .strip_prefix(canonical_root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn first_16(bytes: impl AsRef<[u8]>) -> [u8; 16] {
    let mut output = [0u8; 16];
    output.copy_from_slice(&bytes.as_ref()[..16]);
    output
}

fn kind_name(kind: UpdatePayloadKind) -> &'static str {
    match kind {
        UpdatePayloadKind::Full => "full",
        UpdatePayloadKind::Delta => "delta",
    }
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
    fn full_and_delta_option_contracts_fail_closed() {
        let base = vec![
            "qemu-prod".into(),
            "--kind".into(),
            "full".into(),
            "--base".into(),
            "base.raw".into(),
            "--target".into(),
            "target.raw".into(),
            "--signing-key".into(),
            "key".into(),
            "--output".into(),
            "out.enupd".into(),
        ];
        assert!(parse_build_options(&base).is_ok());

        let mut invalid_delta = base;
        invalid_delta[2] = "delta".into();
        assert!(parse_build_options(&invalid_delta).is_ok());
    }

    #[test]
    fn deterministic_delta_roundtrips_and_mutations_fail_closed() {
        let mut base = vec![0x11; 10_000];
        let mut target = base.clone();
        target[7] = 0x22;
        target[8192] = 0x33;
        let delta = build_delta(&base, &target).unwrap();
        assert_eq!(build_delta(&base, &target).unwrap(), delta);
        assert_eq!(apply_delta(&base, &delta).unwrap(), target);

        base.push(0);
        assert!(apply_delta(&base, &delta).is_err());
        let mut corrupted = delta;
        corrupted[36] = 1;
        assert!(apply_delta(&vec![0x11; 10_000], &corrupted).is_err());
        corrupted[36] = 0;
        corrupted.pop();
        assert!(apply_delta(&vec![0x11; 10_000], &corrupted).is_err());
    }
}
