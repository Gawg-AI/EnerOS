//! 蓝图 50（WP-50.3）产线烧录/写入工具（蓝图 16 实施面）：
//! `cargo xtask factory provision` —— 整盘镜像写入 + bootctrl 初始化
//! （Genesis，复用 qemu_harness::reset_bootctrl_copies 同源语义）+ 产线
//! 密钥校验（对接 47.3 显式键注入机制——`--sign-key` / `ENEROS_SIGN_KEY`
//! / `ENEROS_KMS_DIR`，无键 fail-closed 拒绝）+ 点表摘要入制造记录 +
//! 写后自检（全量 readback digest 一致 + GPT/bootctrl 复验）。
//!
//! 产线记录：每台设备序列号→镜像 digest→密钥域→点表 digest 入
//! 制造记录（schema `eneros.manufacturing-record.v1`，Ed25519 站端签章；
//! `factory verify-record` 校验）。工具 host 侧（Windows/Linux 双跑）。

use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use eneros_image_format::parse_gpt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{invalid, DynResult};

const RECORD_SCHEMA: &str = "eneros.manufacturing-record.v1";

/// 环境变量：产线 KMS 目录（47.3 KMS_DIR 对接点；目录内 factory.key 为
/// 站端签章键）。优先级低于显式 --sign-key 与 ENEROS_SIGN_KEY。
pub const ENV_KMS_DIR: &str = "ENEROS_KMS_DIR";

/// 制造记录（schema 校验面 + 签章覆盖字段）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingRecord {
    pub schema: String,
    /// 设备序列号（产线段分配，如 ENP-2026-000137）。
    pub serial: String,
    /// 整盘镜像 SHA-256（provision 写入并 readback 验证后的值）。
    pub image_sha256: String,
    /// 点表文件 SHA-256（点表事实源摘要）。
    pub points_sha256: String,
    /// 密钥域（站端签章公钥 SHA-256 前 16 hex——可追溯不泄密）。
    pub key_domain: String,
    /// bootctrl Genesis 状态（sequence 归零、双副本一致）。
    pub bootctrl_genesis: bool,
    /// 写后自检结果（full readback == source digest）。
    pub readback_verified: bool,
    pub station_id: String,
    pub provisioned_at_unix: u64,
    pub result: String,
    /// Ed25519(上述全部字段的 canonical JSON)，站端私钥签。
    pub signature_hex: String,
}

pub(super) fn command_factory(root: &Path, args: &[String]) -> DynResult<()> {
    let Some((mode, tail)) = args.split_first() else {
        return Err(invalid("factory requires 'provision' or 'verify-record'"));
    };
    match mode.as_str() {
        "provision" => command_provision(root, tail),
        "verify-record" => command_verify_record(root, tail),
        other => Err(invalid(format!("unknown factory mode '{other}'"))),
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn command_provision(root: &Path, args: &[String]) -> DynResult<()> {
    let image_path = arg_value(args, "--image")
        .ok_or_else(|| invalid("factory provision requires --image <path>"))?;
    let target_path = arg_value(args, "--target")
        .ok_or_else(|| invalid("factory provision requires --target <raw disk/file>"))?;
    let serial = arg_value(args, "--serial")
        .ok_or_else(|| invalid("factory provision requires --serial <SN>"))?;
    let points_rel = arg_value(args, "--points").unwrap_or_else(|| "configs/points.toml".into());
    let station_id = arg_value(args, "--station").unwrap_or_else(|| "station-default".into());
    let record_out = arg_value(args, "--record")
        .ok_or_else(|| invalid("factory provision requires --record <json out>"))?;

    // ---- 1. 产线密钥解析（47.3 机制对接）：--sign-key > ENEROS_SIGN_KEY >
    //         ENEROS_KMS_DIR/factory.key；**无键 → fail-closed 拒绝**（DoD 负例）。
    let key_bytes = resolve_factory_key(root, arg_value(args, "--sign-key").map(PathBuf::from))?;
    let signing = SigningKey::from_bytes(&key_bytes);
    let key_domain = hex(&Sha256::digest(signing.verifying_key().as_bytes()))[..16].to_owned();

    // ---- 2. 读镜像 + 结构校验（GPT 可解析 + BOOTCTRL 存在且可解码）。
    let image_path = root.join(&image_path);
    let mut image =
        fs::read(&image_path).map_err(|e| invalid(format!("image read failed: {e}")))?;
    let gpt = parse_gpt(&image, 512)
        .map_err(|error| invalid(format!("image GPT parse failed: {error:?}")))?;
    if gpt.find_by_name("BOOTCTRL").is_none() {
        return Err(invalid("image has no BOOTCTRL partition"));
    }

    // ---- 3. bootctrl 初始化（Genesis）：seq 归零 + 双副本一致
    //         （与 qemu_harness::reset_bootctrl_copies 同源语义——独立实现，
    //         不引 harness 私有函数；字节语义由 boot_control 编解码保证）。
    genesis_bootctrl(&mut image)?;
    let image_sha256 = hex(&Sha256::digest(&image));

    // ---- 4. 点表摘要（点表事实源）。
    let points_path = root.join(&points_rel);
    let points_bytes =
        fs::read(&points_path).map_err(|e| invalid(format!("points read failed: {e}")))?;
    let points_sha256 = hex(&Sha256::digest(&points_bytes));

    // ---- 5. 整盘写入（空盘 → 镜像字节）。
    let target_path = root.join(&target_path);
    fs::write(&target_path, &image).map_err(|e| invalid(format!("target write failed: {e}")))?;

    // ---- 6. 写后自检：全量 readback digest 一致 + GPT 复验 + bootctrl 复验。
    let readback =
        fs::read(&target_path).map_err(|e| invalid(format!("target readback failed: {e}")))?;
    let readback_sha256 = hex(&Sha256::digest(&readback));
    let readback_verified = readback_sha256 == image_sha256;
    if !readback_verified {
        return Err(invalid(format!(
            "readback mismatch: wrote {image_sha256} got {readback_sha256}"
        )));
    }
    let reread_gpt = parse_gpt(&readback, 512)
        .map_err(|error| invalid(format!("readback GPT parse failed: {error:?}")))?;
    if reread_gpt != gpt {
        return Err(invalid("readback GPT differs from source"));
    }
    let bootctrl_genesis = bootctrl_is_genesis(&readback)?;

    // ---- 7. 制造记录（签章）。
    let record = ManufacturingRecord {
        schema: RECORD_SCHEMA.to_owned(),
        serial,
        image_sha256,
        points_sha256,
        key_domain,
        bootctrl_genesis,
        readback_verified: true,
        station_id,
        provisioned_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        result: "PASS".to_owned(),
        signature_hex: String::new(),
    };
    let mut record = record;
    record.signature_hex = sign_record(&record, &signing);
    let record_json = serde_json::to_vec_pretty(&record)?;
    let out = target_output(root, Path::new(&record_out), "manufacturing record")?;
    fs::write(out, record_json)?;

    println!(
        "FACTORY-PROVISION serial={} image={} points={} key_domain={} genesis={} readback=verified result=PASS",
        record.serial,
        &record.image_sha256[..16],
        &record.points_sha256[..16],
        record.key_domain,
        record.bootctrl_genesis
    );
    Ok(())
}

/// `factory verify-record <json> --image <path>`：制造记录 schema 校验 +
/// 签章验证 + 与在库镜像 digest 对账（抽检项）。
fn command_verify_record(root: &Path, args: &[String]) -> DynResult<()> {
    let record_path = arg_value(args, "--record")
        .ok_or_else(|| invalid("factory verify-record requires --record <json>"))?;
    let record_bytes = fs::read(root.join(&record_path))
        .map_err(|e| invalid(format!("record read failed: {e}")))?;
    let record: ManufacturingRecord = serde_json::from_slice(&record_bytes)
        .map_err(|e| invalid(format!("record schema violation: {e}")))?;
    if record.schema != RECORD_SCHEMA {
        return Err(invalid(format!("unexpected schema {}", record.schema)));
    }
    if record.result != "PASS" {
        return Err(invalid(format!("record result={}", record.result)));
    }
    // 签章验证（站端公钥由 key_domain 无法反推——用显式 --sign-key 或测试键）。
    let key_bytes = resolve_factory_key(root, arg_value(args, "--sign-key").map(PathBuf::from))?;
    let signing = SigningKey::from_bytes(&key_bytes);
    let verifying: VerifyingKey = signing.verifying_key();
    let unsigned = unsigned_json(&record)?;
    let sig_bytes = crate::evidence_chain::hex_decode(record.signature_hex.trim());
    let sig =
        Signature::from_slice(&sig_bytes).map_err(|_| invalid("record signature malformed"))?;
    verifying
        .verify(&unsigned, &sig)
        .map_err(|_| invalid("record signature INVALID"))?;
    // 与在库镜像对账（可选提供）。
    if let Some(image_rel) = arg_value(args, "--image") {
        let image = fs::read(root.join(image_rel))
            .map_err(|e| invalid(format!("image read failed: {e}")))?;
        let image_sha = hex(&Sha256::digest(&image));
        if image_sha != record.image_sha256 {
            return Err(invalid(format!(
                "image digest mismatch: record {} actual {}",
                &record.image_sha256[..16],
                &image_sha[..16]
            )));
        }
    }
    println!(
        "FACTORY-RECORD-VERIFY serial={} schema=OK signature=VALID result=PASS",
        record.serial
    );
    Ok(())
}

/// 产线密钥解析（47.3 显式注入纪律）：显式路径 > `ENEROS_SIGN_KEY` >
/// `ENEROS_KMS_DIR/factory.key`。**三者皆无 → Err（fail-closed 拒绝 provision）**。
fn resolve_factory_key(root: &Path, explicit: Option<PathBuf>) -> DynResult<[u8; 32]> {
    let path = if let Some(p) = explicit {
        p
    } else if let Ok(p) = std::env::var(crate::keys::ENV_SIGN_KEY) {
        PathBuf::from(p)
    } else if let Ok(dir) = std::env::var(ENV_KMS_DIR) {
        PathBuf::from(dir).join("factory.key")
    } else {
        return Err(invalid(format!(
            "产线签章键未提供：--sign-key <path> 或 {} 或 {ENV_KMS_DIR}/factory.key —— 缺密钥拒绝（fail-closed）",
            crate::keys::ENV_SIGN_KEY,
        )));
    };
    let abs = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let bytes = fs::read(&abs)
        .map_err(|e| invalid(format!("signing key read failed ({}): {e}", abs.display())))?;
    let len = bytes.len();
    bytes.try_into().map_err(|_| {
        invalid(if len < 32 {
            "signing key too short (need 32B)"
        } else {
            "signing key invalid length"
        })
    })
}

/// bootctrl Genesis：双副本一致解码 → sequence 归零 → 重编码双写。
fn genesis_bootctrl(image: &mut [u8]) -> DynResult<()> {
    use eneros_boot_protocol::boot_control::{
        BootControl, BOOT_CONTROL_COPY_OFFSETS, BOOT_CONTROL_RECORD_SIZE,
    };
    let gpt = parse_gpt(image, 512)
        .map_err(|error| invalid(format!("BOOTCTRL GPT parse failed: {error:?}")))?;
    let partition = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("image has no BOOTCTRL partition"))?;
    let start = usize::try_from(
        partition
            .first_lba
            .checked_mul(512)
            .ok_or_else(|| invalid("BOOTCTRL offset overflow"))?,
    )?;
    let read = |img: &[u8], off: u64| -> DynResult<[u8; BOOT_CONTROL_RECORD_SIZE]> {
        let s = start
            .checked_add(off as usize)
            .ok_or_else(|| invalid("copy overflow"))?;
        Ok(img
            .get(s..s + BOOT_CONTROL_RECORD_SIZE)
            .ok_or_else(|| invalid("BOOTCTRL copy truncated"))?
            .try_into()
            .expect("record size"))
    };
    let first = read(image, BOOT_CONTROL_COPY_OFFSETS[0])?;
    let second = read(image, BOOT_CONTROL_COPY_OFFSETS[1])?;
    if first != second {
        return Err(invalid(
            "genesis requires equal canonical BOOTCTRL source copies",
        ));
    }
    let mut control = BootControl::decode(&first)
        .map_err(|error| invalid(format!("BOOTCTRL decode failed: {error:?}")))?;
    control.sequence = 0;
    let record = control
        .encode()
        .map_err(|error| invalid(format!("BOOTCTRL encode failed: {error:?}")))?;
    for off in BOOT_CONTROL_COPY_OFFSETS {
        let s = start.checked_add(off as usize).expect("in-range");
        image[s..s + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(&record);
    }
    Ok(())
}

/// 读回后校验 bootctrl 处于 Genesis 态（seq=0 且双副本一致）。
fn bootctrl_is_genesis(image: &[u8]) -> DynResult<bool> {
    use eneros_boot_protocol::boot_control::{
        BootControl, BOOT_CONTROL_COPY_OFFSETS, BOOT_CONTROL_RECORD_SIZE,
    };
    let gpt = parse_gpt(image, 512)
        .map_err(|error| invalid(format!("BOOTCTRL verify GPT parse failed: {error:?}")))?;
    let partition = gpt
        .find_by_name("BOOTCTRL")
        .ok_or_else(|| invalid("image has no BOOTCTRL partition"))?;
    let start = usize::try_from(
        partition
            .first_lba
            .checked_mul(512)
            .ok_or_else(|| invalid("BOOTCTRL offset overflow"))?,
    )?;
    let copy = |off: u64| -> DynResult<&[u8]> {
        let s = start.checked_add(off as usize).expect("in-range");
        image
            .get(s..s + BOOT_CONTROL_RECORD_SIZE)
            .ok_or_else(|| invalid("BOOTCTRL copy truncated"))
    };
    let a = copy(BOOT_CONTROL_COPY_OFFSETS[0])?;
    let b = copy(BOOT_CONTROL_COPY_OFFSETS[1])?;
    if a != b {
        return Ok(false);
    }
    let control = BootControl::decode(a)
        .map_err(|error| invalid(format!("BOOTCTRL verify decode failed: {error:?}")))?;
    Ok(control.sequence == 0)
}

fn unsigned_json(record: &ManufacturingRecord) -> DynResult<Vec<u8>> {
    let mut r = record.clone();
    r.signature_hex = String::new();
    Ok(serde_json::to_vec(&r)?)
}

fn sign_record(record: &ManufacturingRecord, key: &SigningKey) -> String {
    let unsigned = unsigned_json(record).unwrap_or_default();
    hex(&key.sign(&unsigned).to_bytes())
}

fn target_output(root: &Path, out: &Path, what: &'static str) -> DynResult<PathBuf> {
    let _ = what;
    if out.is_absolute() {
        Ok(out.to_path_buf())
    } else {
        Ok(root.join(out))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_seed_rejects_overlong_input() {
        let root = std::env::temp_dir().join(format!("eneros-factory-key-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("seed.bin");
        fs::write(&path, [5u8; 64]).unwrap();
        assert!(resolve_factory_key(&root, Some(path.clone())).is_err());
        fs::write(&path, [5u8; 32]).unwrap();
        assert_eq!(resolve_factory_key(&root, Some(path)).unwrap(), [5u8; 32]);
        fs::remove_dir_all(root).unwrap();
    }

    /// 50：制造记录 schema 往返 + 签章验证 + 篡改检测。
    #[test]
    fn manufacturing_record_sign_and_verify() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let record = ManufacturingRecord {
            schema: RECORD_SCHEMA.to_owned(),
            serial: "ENP-2026-000001".to_owned(),
            image_sha256: "ab".repeat(32),
            points_sha256: "cd".repeat(32),
            key_domain: "0123456789abcdef".to_owned(),
            bootctrl_genesis: true,
            readback_verified: true,
            station_id: "station-test".to_owned(),
            provisioned_at_unix: 1_755_523_200,
            result: "PASS".to_owned(),
            signature_hex: String::new(),
        };
        let mut signed = record.clone();
        signed.signature_hex = sign_record(&signed, &key);
        // 签名可验证。
        let unsigned = unsigned_json(&signed).unwrap();
        let sig = Signature::from_slice(&crate::evidence_chain::hex_decode(
            signed.signature_hex.trim(),
        ))
        .unwrap();
        assert!(key.verifying_key().verify(&unsigned, &sig).is_ok());
        // 字段篡改 → 验签失败。
        let mut tampered = signed.clone();
        tampered.serial = "ENP-2026-000002".to_owned();
        let t_unsigned = unsigned_json(&tampered).unwrap();
        assert!(key.verifying_key().verify(&t_unsigned, &sig).is_err());
    }
}
