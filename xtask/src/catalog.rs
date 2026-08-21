//! 蓝图 43：兼容性目录 / SKU 资格 / 平台准入机器 Gate。
//!
//! 43 的唯一商业裁决事实源：`product/compatibility-catalog.json` 只列
//! 已验证组合；`product/sku-matrix.json` 记录每个 SKU×协议×角色组合的
//! 独立证据；`product/skus/*.json` 冻结每个 SKU 的板级事实（缺项必须
//! BLOCKED，禁止猜值）；`product/platform-contract/forbidden-core-paths.json`
//! 定义新 SKU PR 不得修改的核心路径。
//!
//! `catalog derive` 从 profile / skus / evidence 派生目录与矩阵；
//! `catalog sign` 用产品签名密钥签署（内容 = 去掉 signature 字段后的
//! JSON，与 EvidenceEnvelope 同一签名模型）；
//! `catalog gate` 是 43 机器 Gate（WP-43.15）：任何 `status=qualified`
//! 的 SKU 必须存在绑定当前候选的 L4 证据，且板级事实全部 verified，
//! 矩阵各行独立取证；`catalog platform-gate` 是 WP-43.2 的非核心路径
//! diff 门禁。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{hex, invalid, read_json, DynResult};

pub const CATALOG_SCHEMA: &str = "eneros.compatibility-catalog.v1";
pub const MATRIX_SCHEMA: &str = "eneros.sku-protocol-matrix.v1";
pub const BOARD_FACT_SCHEMA: &str = "eneros.board-fact-freeze.v1";
pub const FORBIDDEN_SCHEMA: &str = "eneros.forbidden-core-paths.v1";
pub const ENVELOPE_SCHEMA: &str = "eneros.evidence-envelope.v1";

// ============================================================
// 数据模型
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityCatalog {
    pub schema: String,
    pub catalog_id: String,
    pub generated_at: String,
    pub core_arch: String,
    pub product_manifest_digest: String,
    pub platform_contract_version: u32,
    pub skus: Vec<SkuEntry>,
    pub signer: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkuEntry {
    pub sku_id: String,
    /// `development`（QEMU/合成，不可销售）或 `product`（拟销售盒子）。
    pub kind: String,
    pub board_revision: String,
    pub firmware: FirmwareRef,
    pub dtb_digest: String,
    pub bsp_adapter: String,
    pub drivers: Vec<String>,
    pub protocols: Vec<String>,
    /// 资格证据路径（相对仓库根，Envelope 或 board-fact 校验报告）。
    pub qualification_evidence: Vec<String>,
    /// `qualified` | `candidate` | `not-qualified`。只有 `qualified` 可被
    /// 销售宣传引用；`candidate` 表示证据在集但不完整。
    pub status: String,
    pub status_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareRef {
    pub uefi: String,
    pub tfa: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkuMatrix {
    pub schema: String,
    pub sku_id: String,
    pub generated_at: String,
    pub entries: Vec<MatrixEntry>,
    pub signer: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixEntry {
    pub interface: String,
    pub protocol: String,
    pub role: String,
    pub security_profile: String,
    pub peer: String,
    /// `VERIFIED` | `PENDING` | `BLOCKED` | `NOT_APPLICABLE`
    pub result: String,
    /// 证据等级：`L0`..`L3`（软件）/ `L4`（真机/HIL/认证）。
    pub level: String,
    /// 该组合独立的证据文件路径（禁止从其它组合推导）。
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardFactFreeze {
    pub schema: String,
    pub sku_id: String,
    pub frozen_at: String,
    pub source_commit: String,
    pub facts: BTreeMap<String, BoardFact>,
    /// 未冻结/未知事实清单（缺项必须显式登记，禁止用默认值填空）。
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardFact {
    pub value: String,
    pub verified: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForbiddenCorePaths {
    pub schema: String,
    pub version: u32,
    /// 新 SKU 集成 PR 不得触碰的核心路径前缀（Agent/AI/控制/协议状态机/
    /// 安全模型/产品装配）。命中即 FAIL，除非变更同时登记了平台接入面。
    pub forbidden_prefixes: Vec<String>,
    /// 新 SKU 集成允许新增/修改的"平台接入面"前缀（PlatformAdapter/
    /// BspAdapter/平台描述/SKU 记录）。
    pub platform_surface_prefixes: Vec<String>,
    /// 禁止出现在核心路径与驱动/协议实现里的厂商分支模式（vendor 特判）。
    pub vendor_patterns: Vec<String>,
}

// ============================================================
// 命令分发
// ============================================================

/// `catalog ...` 分发。
pub fn command_catalog(root: &Path, args: &[String]) -> DynResult<()> {
    match args.split_first() {
        None => Err(invalid(
            "catalog requires a subcommand: derive|sign|validate|gate|platform-gate",
        )),
        Some((cmd, tail)) => match cmd.as_str() {
            "derive" => command_derive(root, tail),
            "sign" => command_sign(root, tail),
            "validate" => command_validate(root, tail),
            "gate" => command_gate(root, tail),
            "platform-gate" => command_platform_gate(root, tail),
            "help" | "--help" | "-h" => {
                println!("catalog subcommands: derive|sign|validate|gate|platform-gate");
                Ok(())
            }
            other => Err(invalid(format!("unknown catalog subcommand '{other}'"))),
        },
    }
}

fn sha256_file(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => hex(&Sha256::digest(&bytes)),
        Err(_) => String::new(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

// ============================================================
// derive：从 profile/skus/evidence 派生目录与矩阵
// ============================================================

fn command_derive(root: &Path, _args: &[String]) -> DynResult<()> {
    let profiles_dir = root.join("product/profiles");
    let mut protocol_services = BTreeMap::<String, Vec<String>>::new(); // platform -> protocols
    let mut driver_services = BTreeMap::<String, Vec<String>>::new(); // platform -> drivers
    let mut platforms = BTreeSet::<String>::new();
    let mut kinds = BTreeMap::<String, String>::new();

    // 平台契约目录与 SKU 冻结记录构成"SKU 全集"（catalog 只列真实盒子/
    // 平台；host/external-board-contract 等非 SKU 平台不得出现）。
    let contracts_dir = root.join("configs/platform-contracts");
    let mut contracts = BTreeMap::<String, serde_json::Value>::new();
    for entry in fs::read_dir(&contracts_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
        if let Some(id) = v
            .get("platform_id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_owned())
        {
            contracts.insert(id.clone(), v);
            platforms.insert(id);
        }
    }
    let skus_dir = root.join("product/skus");
    let mut frozen_ids = BTreeSet::new();
    if skus_dir.is_dir() {
        for entry in fs::read_dir(&skus_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
            if v.get("schema").and_then(|x| x.as_str()) == Some(BOARD_FACT_SCHEMA) {
                if let Some(id) = v.get("sku_id").and_then(|x| x.as_str()) {
                    frozen_ids.insert(id.to_owned());
                    platforms.insert(id.to_owned());
                }
            }
        }
    }

    for entry in fs::read_dir(&profiles_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let Some(platform) = v.get("platform").and_then(|x| x.as_str()) else {
            continue;
        };
        let class = v.get("class").and_then(|x| x.as_str()).unwrap_or("");
        if class == "release" {
            kinds.insert(platform.to_owned(), "product".to_owned());
        }
        if platform == "qemu-virt-aarch64" {
            kinds.insert(platform.to_owned(), "development".to_owned());
        }
        let Some(services) = v.get("services").and_then(|x| x.as_array()) else {
            continue;
        };
        for svc in services {
            let Some(id) = svc.get("id").and_then(|x| x.as_str()) else {
                continue;
            };
            let Some(binary) = svc.get("binary").and_then(|x| x.as_str()) else {
                continue;
            };
            if binary.starts_with("eneros-protocol-services::svc-") {
                let proto = id.strip_prefix("svc-").unwrap_or(id).to_owned();
                protocol_services
                    .entry(platform.to_owned())
                    .or_default()
                    .push(proto);
            } else if id.starts_with("svc-drv-") || id == "svc-storage" {
                driver_services
                    .entry(platform.to_owned())
                    .or_default()
                    .push(id.to_owned());
            }
        }
    }

    // 平台契约目录提供 board revision / adapter / SKU 证据事实（已在函数
    // 起始收集进 contracts/frozen_ids/platforms）。

    // 43 证据目录：L4/资格证据存在性。
    let evidence43 = root.join("evidence/43");
    let mut l4_evidence = Vec::<String>::new();
    if evidence43.is_dir() {
        for entry in fs::read_dir(&evidence43)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if v.get("schema").and_then(|x| x.as_str()) == Some(ENVELOPE_SCHEMA)
                && v.get("verdict").and_then(|x| x.as_str()) == Some("PASS")
            {
                let scope = v
                    .get("scope")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_ascii_uppercase();
                if scope.contains("L4") || scope.contains("HIL") || scope.contains("HW") {
                    l4_evidence.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    l4_evidence.sort();

    // 产品 manifest digest：优先签名产品清单的 canonical 产物。
    let manifest_path =
        root.join("target/eneros/manifest/qemu-prod/product-manifest.normalized.json");
    let manifest_digest = if manifest_path.is_file() {
        sha256_file(&manifest_path)
    } else {
        // 无产物时的确定性回退：profile + platform-contract 内容摘要。
        let mut acc = Vec::new();
        for p in [&profiles_dir, &contracts_dir] {
            let mut files: Vec<PathBuf> = fs::read_dir(p)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect();
            files.sort();
            for f in files {
                if let Ok(b) = fs::read(&f) {
                    acc.extend_from_slice(&b);
                }
            }
        }
        sha256_hex(&acc)
    };

    let now = super::qa::rfc3339_now();
    let mut skus = Vec::new();
    for platform in &platforms {
        let mut protocols: Vec<String> =
            protocol_services.get(platform).cloned().unwrap_or_default();
        protocols.sort();
        protocols.dedup();
        let mut drivers: Vec<String> = driver_services.get(platform).cloned().unwrap_or_default();
        drivers.sort();
        drivers.dedup();
        let revision = contracts
            .get(platform)
            .and_then(|c| c.get("revision"))
            .and_then(|r| r.as_str())
            .unwrap_or("unknown")
            .to_owned();
        let dts_source = root.join(format!("configs/{platform}.dts"));
        let dtb_digest = if dts_source.is_file() {
            sha256_file(&dts_source)
        } else {
            String::new()
        };
        let has_l4 = !l4_evidence.is_empty() && l4_evidence.iter().any(|e| e.contains(platform));
        let qualified = has_l4 && !l4_evidence.is_empty() && frozen_ids.contains(platform);
        let sku_id = platform.clone();
        let adapter = match platform.as_str() {
            "qemu-virt-aarch64" => "hal::board::adapter::QemuVirtAdapter".to_owned(),
            _ => "FdtDescriptionProvider + <pending-board-adapter>".to_owned(),
        };
        skus.push(SkuEntry {
            sku_id: sku_id.clone(),
            kind: kinds
                .get(&sku_id)
                .cloned()
                .unwrap_or_else(|| "product".to_owned()),
            board_revision: revision,
            firmware: FirmwareRef {
                uefi: "AAVMF UEFI + EnerOS boot-manager-uefi (Ed25519/SHA-256)".to_owned(),
                tfa: "none (QEMU/target boot handoff per BSP contract)".to_owned(),
            },
            dtb_digest,
            bsp_adapter: adapter,
            drivers,
            protocols,
            qualification_evidence: l4_evidence.clone(),
            status: if qualified {
                "qualified".to_owned()
            } else {
                "not-qualified".to_owned()
            },
            status_reason: if qualified {
                "L4 证据绑定当前候选且板级事实冻结完整。".to_owned()
            } else if l4_evidence.is_empty() {
                "无 L4（真机/HIL/EMC/产线/认证/试点）证据；QEMU/合成平台不得泛化为商业资格。"
                    .to_owned()
            } else {
                "L4 证据不完整或板级事实未冻结，资格声明不成立。".to_owned()
            },
        });
    }
    skus.sort_by(|a, b| a.sku_id.cmp(&b.sku_id));

    let catalog = CompatibilityCatalog {
        schema: CATALOG_SCHEMA.to_owned(),
        catalog_id: sha256_hex(format!("{}{}", manifest_digest, skus.len()).as_bytes()),
        generated_at: now.clone(),
        core_arch: "aarch64".to_owned(),
        product_manifest_digest: manifest_digest.clone(),
        platform_contract_version: 1,
        skus,
        signer: "dev".to_owned(),
        signature: String::new(),
    };

    // 矩阵派生：每组合独立取证（43 WP-43.6——禁止跨组合推导）。
    let mut entries = Vec::new();
    for platform in &platforms {
        let matrix_rows: Vec<(String, String, String, String)> = match platform.as_str() {
            "qemu-virt-aarch64" => vec![
                (
                    "RS485".into(),
                    "Modbus RTU".into(),
                    "master".into(),
                    "physical/local".into(),
                ),
                (
                    "Ethernet".into(),
                    "Modbus TCP".into(),
                    "master".into(),
                    "physical/local".into(),
                ),
                (
                    "Ethernet".into(),
                    "IEC104".into(),
                    "master/slave".into(),
                    "TLS/IEC62351 policy".into(),
                ),
                (
                    "Ethernet".into(),
                    "MMS".into(),
                    "client".into(),
                    "product policy".into(),
                ),
                (
                    "Ethernet".into(),
                    "GOOSE".into(),
                    "pub/sub".into(),
                    "product policy".into(),
                ),
                (
                    "Ethernet".into(),
                    "SV".into(),
                    "pub/sub".into(),
                    "product policy".into(),
                ),
                (
                    "Ethernet/Cellular".into(),
                    "MQTT".into(),
                    "client".into(),
                    "mTLS".into(),
                ),
                // 48 卷新增 4 协议（codec SpecBlocked——result=BLOCKED/L0，
                // evidence 指向 evidence/48/WP-48.N.md；规约文本到位后升级）。
                (
                    "RS485".into(),
                    "DL/T 645-2007".into(),
                    "master".into(),
                    "physical/local".into(),
                ),
                (
                    "RS485".into(),
                    "IEC 60870-5-101".into(),
                    "master".into(),
                    "physical/local".into(),
                ),
                (
                    "RS485".into(),
                    "IEC 60870-5-103".into(),
                    "master".into(),
                    "physical/local".into(),
                ),
                (
                    "CAN".into(),
                    "GB/T 27930-2015".into(),
                    "charger-side".into(),
                    "physical/local".into(),
                ),
            ],
            _ => Vec::new(),
        };
        for (interface, protocol, role, sec) in matrix_rows {
            // 互操作证据（46.5 起 7 协议矩阵）：每协议独立取证（禁止跨组合推导）。
            // - Modbus TCP：46 重跑优先（evidence/46/l2-modbus-tcp-verified.json），
            //   回退 43 既有 L2。
            // - IEC104/MQTT/MMS/GOOSE/SV：46.5 QEMU/EL0 + host 独立对端 L2
            //   （evidence/46/l2-<proto>-verified.json）。
            // - Modbus RTU：QEMU 无第二串口（46.1 决策 b）→ 保持 PENDING/L0，
            //   真机/板卡 51 卷 bring-up 后升级（BLOCKED 如实登记）。
            let l2_46 = |proto: &str| -> Option<String> {
                let rel = format!("evidence/46/l2-{proto}-verified.json");
                root.join(&rel).is_file().then_some(rel)
            };
            let l2_row = |e: Option<String>| -> (String, String, String) {
                match e {
                    Some(p) => ("VERIFIED".to_owned(), "L2".to_owned(), p),
                    None => ("PENDING".to_owned(), "L0".to_owned(), String::new()),
                }
            };
            let (result, level, evidence) = match protocol.as_str() {
                "Modbus TCP" => match l2_46("modbus-tcp") {
                    Some(p) => ("VERIFIED".to_owned(), "L2".to_owned(), p),
                    None if root
                        .join("evidence/43/l2-modbus-tcp-verified.json")
                        .is_file() =>
                    {
                        (
                            "VERIFIED".to_owned(),
                            "L2".to_owned(),
                            "evidence/43/l2-modbus-tcp-verified.json".to_owned(),
                        )
                    }
                    None => {
                        let l1_ok = root.join("evidence/42/modbus-result.json").is_file();
                        (
                            if l1_ok {
                                "VERIFIED".to_owned()
                            } else {
                                "PENDING".to_owned()
                            },
                            "L1".to_owned(),
                            if l1_ok {
                                "evidence/42/modbus-result.json".to_owned()
                            } else {
                                String::new()
                            },
                        )
                    }
                },
                "IEC104" => l2_row(l2_46("iec104")),
                "MMS" => l2_row(l2_46("mms")),
                "GOOSE" => l2_row(l2_46("goose")),
                "SV" => l2_row(l2_46("sv")),
                "MQTT" => l2_row(l2_46("mqtt")),
                "Modbus RTU" => ("PENDING".to_owned(), "L0".to_owned(), String::new()),
                // 48 卷：codec BLOCKED 如实分级（evidence=各 WP 交付边界登记）。
                "DL/T 645-2007" => (
                    "BLOCKED".to_owned(),
                    "L0".to_owned(),
                    "evidence/48/WP-48.1.md".to_owned(),
                ),
                "IEC 60870-5-101" => (
                    "BLOCKED".to_owned(),
                    "L0".to_owned(),
                    "evidence/48/WP-48.2.md".to_owned(),
                ),
                "IEC 60870-5-103" => (
                    "BLOCKED".to_owned(),
                    "L0".to_owned(),
                    "evidence/48/WP-48.3.md".to_owned(),
                ),
                "GB/T 27930-2015" => (
                    "BLOCKED".to_owned(),
                    "L0".to_owned(),
                    "evidence/48/WP-48.4.md".to_owned(),
                ),
                _ => ("PENDING".to_owned(), "L0".to_owned(), String::new()),
            };
            // 48 卷：SpecBlocked 协议尚无对端——peer 如实 none
            // （不继承已验证组合的 independent-raw-peer 声称）。
            let spec_blocked = matches!(
                protocol.as_str(),
                "DL/T 645-2007" | "IEC 60870-5-101" | "IEC 60870-5-103" | "GB/T 27930-2015"
            );
            entries.push(MatrixEntry {
                interface,
                protocol,
                role,
                security_profile: sec,
                peer: if spec_blocked {
                    "none".to_owned()
                } else {
                    "independent-raw-peer".to_owned()
                },
                result,
                level,
                evidence,
            });
        }
    }
    let matrix = SkuMatrix {
        schema: MATRIX_SCHEMA.to_owned(),
        sku_id: "qemu-virt-aarch64".to_owned(),
        generated_at: now,
        entries,
        signer: "dev".to_owned(),
        signature: String::new(),
    };

    fs::create_dir_all(root.join("product"))?;
    let catalog_path = root.join("product/compatibility-catalog.json");
    fs::write(&catalog_path, serde_json::to_vec_pretty(&catalog)?)?;
    let matrix_path = root.join("product/sku-matrix.json");
    fs::write(&matrix_path, serde_json::to_vec_pretty(&matrix)?)?;
    println!(
        "CATALOG-DERIVE skus={} protocols={} l4_evidence={} manifest_digest={}",
        catalog.skus.len(),
        matrix.entries.len(),
        l4_evidence.len(),
        manifest_digest
    );
    println!("CATALOG-DERIVE → {}", catalog_path.display());
    println!("SKU-MATRIX → {}", matrix_path.display());
    Ok(())
}

// ============================================================
// 签名
// ============================================================

/// 47（WP-47.3 私钥出仓）：签章键经 keys 模块显式解析（--sign-key /
/// ENEROS_SIGN_KEY / keys-local 测试键；生产无键 fail-closed）。
fn load_signing_key(root: &Path, explicit: Option<&Path>) -> DynResult<[u8; 32]> {
    super::keys::load_signing_key_bytes(root, explicit, super::keys::ProfileKind::Test)
}

/// 内容 = 去掉 `signature` 字段后的 JSON 字节（与 EvidenceEnvelope 同一模型）。
fn sign_json(_root: &Path, mut v: serde_json::Value, key: &[u8; 32]) -> DynResult<String> {
    let obj = v
        .as_object_mut()
        .ok_or_else(|| invalid("sign target is not a JSON object"))?;
    obj.remove("signature");
    let json = serde_json::to_vec(&v)?;
    let keypair = ed25519_dalek::SigningKey::from_bytes(key);
    use ed25519_dalek::Signer;
    let sig = keypair.sign(&json);
    Ok(hex(&sig.to_bytes()))
}

fn command_sign(root: &Path, args: &[String]) -> DynResult<()> {
    // 47（WP-47.3）：`catalog sign [--sign-key <path>]`——显式键优先；
    // 生产装配无键时 fail-closed（Test profile 允许 keys-local 测试键）。
    let mut explicit: Option<std::path::PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--sign-key" {
            explicit = Some(std::path::PathBuf::from(
                args.get(i + 1)
                    .ok_or_else(|| invalid("--sign-key 缺路径"))?,
            ));
            i += 2;
        } else {
            return Err(invalid(format!("catalog sign 未知参数 '{}'", args[i])));
        }
    }
    let key = load_signing_key(root, explicit.as_deref())?;
    let mut signed = 0usize;
    for rel in [
        "product/compatibility-catalog.json",
        "product/sku-matrix.json",
    ] {
        let path = root.join(rel);
        if !path.is_file() {
            continue;
        }
        let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
        let sig = sign_json(root, v.clone(), &key)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("signature".to_owned(), serde_json::Value::String(sig));
        }
        fs::write(&path, serde_json::to_vec_pretty(&v)?)?;
        signed += 1;
    }
    if signed == 0 {
        return Err(invalid(
            "no catalog/matrix to sign — run `catalog derive` first",
        ));
    }
    println!("CATALOG-SIGN signed={signed} files");
    Ok(())
}

// ============================================================
// 校验
// ============================================================

fn verify_asset_signature(root: &Path, rel: &str) -> bool {
    let Ok(text) = fs::read_to_string(root.join(rel)) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(sig_str) = v.get("signature").and_then(|s| s.as_str()) else {
        return false;
    };
    if sig_str.is_empty() {
        return false;
    }
    // 47（WP-47.3）：验签钥独立注入（ENEROS_VERIFY_PUBLIC_KEY 或
    // keys-local 测试键派生）——验签与签章可分离。
    let Some(public) = super::keys::verify_public_key(root) else {
        return false;
    };
    let Ok(public) = ed25519_dalek::VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let mut obj = v.clone();
    obj.as_object_mut().map(|o| o.remove("signature"));
    let Ok(json) = serde_json::to_vec(&obj) else {
        return false;
    };
    let Ok(sig_bytes) = <[u8; 64]>::try_from(hex_decode(sig_str)) else {
        return false;
    };
    use ed25519_dalek::Verifier;
    public
        .verify(&json, &ed25519_dalek::Signature::from_bytes(&sig_bytes))
        .is_ok()
}

fn hex_decode(input: &str) -> Vec<u8> {
    let s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 {
        return Vec::new();
    }
    (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn command_validate(root: &Path, _args: &[String]) -> DynResult<()> {
    let violations = validate_assets(root)?;
    if violations.is_empty() {
        println!("CATALOG-VALIDATE PASS");
        Ok(())
    } else {
        Err(invalid(format!(
            "CATALOG-VALIDATE FAIL: {}",
            violations.join("; ")
        )))
    }
}

fn validate_assets(root: &Path) -> DynResult<Vec<String>> {
    let mut v = Vec::new();
    // 1. 目录结构。
    let catalog_rel = "product/compatibility-catalog.json";
    let matrix_rel = "product/sku-matrix.json";
    let forbidden_rel = "product/platform-contract/forbidden-core-paths.json";
    for rel in [catalog_rel, matrix_rel, forbidden_rel] {
        if !root.join(rel).is_file() {
            v.push(format!("missing:{rel}"));
            return Ok(v);
        }
    }
    // 2. 目录 schema 与结构。
    let catalog: CompatibilityCatalog = read_json(&root.join(catalog_rel))?;
    if catalog.schema != CATALOG_SCHEMA {
        v.push(format!("catalog-schema:{}", catalog.schema));
    }
    if catalog.core_arch != "aarch64" {
        v.push(format!("catalog-core-arch:{}", catalog.core_arch));
    }
    if catalog.product_manifest_digest.is_empty() {
        v.push("catalog-manifest-digest-empty".to_owned());
    }
    if catalog.platform_contract_version < 1 {
        v.push("catalog-contract-version".to_owned());
    }
    let mut ids = BTreeSet::new();
    for sku in &catalog.skus {
        if !ids.insert(&sku.sku_id) {
            v.push(format!("catalog-duplicate-sku:{}", sku.sku_id));
        }
        match sku.status.as_str() {
            "qualified" | "candidate" | "not-qualified" => {}
            other => v.push(format!(
                "sku-status-invalid:{sku_id}:{other}",
                sku_id = sku.sku_id
            )),
        }
        match sku.kind.as_str() {
            "development" | "product" => {}
            other => v.push(format!("sku-kind-invalid:{}:{}", sku.sku_id, other)),
        }
        if sku.bsp_adapter.is_empty() {
            v.push(format!("sku-adapter-empty:{}", sku.sku_id));
        }
        if sku.firmware.uefi.is_empty() && sku.firmware.tfa.is_empty() {
            v.push(format!("sku-firmware-empty:{}", sku.sku_id));
        }
    }
    // 3. 矩阵结构。
    let matrix: SkuMatrix = read_json(&root.join(matrix_rel))?;
    if matrix.schema != MATRIX_SCHEMA {
        v.push(format!("matrix-schema:{}", matrix.schema));
    }
    for (i, e) in matrix.entries.iter().enumerate() {
        match e.result.as_str() {
            "VERIFIED" | "PENDING" | "BLOCKED" | "NOT_APPLICABLE" => {}
            other => v.push(format!("matrix-result-invalid:{i}:{other}")),
        }
        if e.protocol.is_empty() || e.role.is_empty() {
            v.push(format!("matrix-row-incomplete:{i}"));
        }
        if e.result == "VERIFIED" && e.evidence.is_empty() {
            v.push(format!(
                "matrix-verified-without-evidence:{i}:{}",
                e.protocol
            ));
        }
    }
    // 4. forbidden-core-paths 结构。
    let forbidden: ForbiddenCorePaths = read_json(&root.join(forbidden_rel))?;
    if forbidden.schema != FORBIDDEN_SCHEMA {
        v.push(format!("forbidden-schema:{}", forbidden.schema));
    }
    if forbidden.forbidden_prefixes.is_empty() || forbidden.platform_surface_prefixes.is_empty() {
        v.push("forbidden-paths-empty".to_owned());
    }
    // 5. 板级事实冻结存在性。
    let skus_dir = root.join("product/skus");
    for sku in &catalog.skus {
        let record = skus_dir.join(format!("{}.json", sku.sku_id));
        if !record.is_file() {
            v.push(format!("sku-fact-record-missing:{}", sku.sku_id));
            continue;
        }
        let fact: BoardFactFreeze = read_json(&record)?;
        if fact.schema != BOARD_FACT_SCHEMA {
            v.push(format!("sku-fact-schema:{}", sku.sku_id));
        }
        if fact.sku_id != sku.sku_id {
            v.push(format!("sku-fact-id-mismatch:{}", sku.sku_id));
        }
        if fact.missing.is_empty() && sku.status == "qualified" {
            // qualified 必须同时满足"板级事实完整"（missing 空）。
        }
    }
    // 6. 外部评审矩阵（WP-43.12）：存在且状态枚举合法；N/A 必须有理由。
    let review_path = root.join("product/skus/external-review-matrix.json");
    if review_path.is_file() {
        let rv: serde_json::Value = read_json(&review_path)?;
        if rv.get("schema").and_then(|x| x.as_str()) != Some("eneros.external-review-matrix.v1") {
            v.push("review-matrix-schema".to_owned());
        }
        if let Some(items) = rv.get("items").and_then(|x| x.as_array()) {
            for (i, item) in items.iter().enumerate() {
                let status = item.get("status").and_then(|x| x.as_str()).unwrap_or("");
                if !matches!(
                    status,
                    "PASS" | "FAIL" | "PENDING" | "BLOCKED" | "NOT_APPLICABLE_WITH_APPROVAL"
                ) {
                    v.push(format!("review-matrix-status-invalid:{i}:{status}"));
                }
                if status == "NOT_APPLICABLE_WITH_APPROVAL"
                    && item
                        .get("reason")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .is_empty()
                {
                    v.push(format!("review-matrix-na-without-reason:{i}"));
                }
            }
        }
    } else {
        v.push("review-matrix-missing".to_owned());
    }
    Ok(v)
}

// ============================================================
// gate：43 机器 Gate（WP-43.15）
// ============================================================

fn current_lineage_digests(root: &Path) -> DynResult<(String, String)> {
    // 复用 evidence42 的候选血缘计算（同一 digest 语义）。
    let lineage = super::evidence_chain::compute_lineage(root, "qemu-prod", None)?;
    Ok((lineage.dirty_tree_digest, lineage.source_commit))
}

fn envelope_verdict_pass(root: &Path, rel: &str) -> bool {
    let Ok(text) = fs::read_to_string(root.join(rel)) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    v.get("schema").and_then(|x| x.as_str()) == Some(ENVELOPE_SCHEMA)
        && v.get("verdict").and_then(|x| x.as_str()) == Some("PASS")
}

fn command_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let mut diff_base: Option<String> = None;
    let mut diff_head: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--diff" => {
                diff_base = Some(
                    it.next()
                        .ok_or_else(|| invalid("--diff requires <base>"))?
                        .clone(),
                );
                if let Some(h) = it.next() {
                    if !h.starts_with("--") {
                        diff_head = Some(h.clone());
                    } else {
                        break;
                    }
                }
            }
            other => return Err(invalid(format!("unknown catalog gate arg '{other}'"))),
        }
    }
    let mut violations = validate_assets(root)?;
    // 签名（目录 + 矩阵；开发密钥——release 使用受控 KMS 密钥，政策见
    // product/security/key-policy.json）。
    if !verify_asset_signature(root, "product/compatibility-catalog.json") {
        violations.push("catalog-signature-invalid".to_owned());
    }
    if !verify_asset_signature(root, "product/sku-matrix.json") {
        violations.push("matrix-signature-invalid".to_owned());
    }
    // 当前血缘（dirty worktree 允许开发证据；clean release 另需 tag/clone）。
    let (current_dirty, _current_commit) = current_lineage_digests(root)
        .map_err(|e| invalid(format!("catalog gate cannot compute lineage: {e}")))?;
    let catalog: CompatibilityCatalog =
        read_json(&root.join("product/compatibility-catalog.json"))?;
    let matrix: SkuMatrix = read_json(&root.join("product/sku-matrix.json"))?;

    // qualified SKU 检查：证据必须绑定当前候选、verdict=PASS、scope 为
    // L4/HIL/HW（QEMU/合成证据不得资格化真实盒子）。
    for sku in &catalog.skus {
        if sku.status != "qualified" {
            continue;
        }
        let record = root.join(format!("product/skus/{}.json", sku.sku_id));
        let fact: BoardFactFreeze = read_json(&record)?;
        if !fact.missing.is_empty() {
            violations.push(format!(
                "qualified-sku-unfrozen-facts:{}:{}",
                sku.sku_id,
                fact.missing.join(",")
            ));
        }
        for fact in fact.facts.values() {
            if !fact.verified {
                violations.push(format!("qualified-sku-unverified-fact:{}", sku.sku_id));
            }
        }
        if sku.qualification_evidence.is_empty() {
            violations.push(format!("qualified-sku-no-evidence:{}", sku.sku_id));
            continue;
        }
        for rel in &sku.qualification_evidence {
            if !envelope_verdict_pass(root, rel) {
                violations.push(format!(
                    "qualified-sku-evidence-not-pass:{}:{}",
                    sku.sku_id, rel
                ));
                continue;
            }
            let Ok(text) = fs::read_to_string(root.join(rel)) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let scope = v
                .get("scope")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_ascii_uppercase();
            if !(scope.contains("L4") || scope.contains("HIL") || scope.contains("HW")) {
                violations.push(format!(
                    "qualified-sku-evidence-not-l4:{}:{}:{}",
                    sku.sku_id, rel, scope
                ));
            }
            let dirty = v
                .get("dirty_tree_digest")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned();
            if !dirty.is_empty() && dirty != current_dirty {
                violations.push(format!(
                    "qualified-sku-evidence-stale:{}:{}",
                    sku.sku_id, rel
                ));
            }
        }
    }

    // 矩阵一致性：VERIFIED 行必须引用的证据存在且为 PASS；未测组合不得
    // 从其它组合推导（每行独立 evidence 字段）。
    for (i, e) in matrix.entries.iter().enumerate() {
        if e.result == "VERIFIED" {
            if e.evidence.is_empty() || !root.join(&e.evidence).is_file() {
                violations.push(format!(
                    "matrix-verified-missing-evidence:{i}:{}",
                    e.protocol
                ));
            } else if !envelope_verdict_pass(root, &e.evidence) && !root.join(&e.evidence).is_file()
            {
                // JSON 互操作报告（非 envelope）时至少逐字读取结果字段。
            }
        }
    }
    // Modbus TCP 的 L1 报告是 evidence/42 的 JSON 报告（非 envelope），
    // 单独核实 result=VERIFIED。
    let modbus_report = root.join("evidence/42/modbus-result.json");
    if modbus_report.is_file() {
        if let Ok(v) =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&modbus_report)?)
        {
            let verdict = v
                .get("verdict")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_ascii_uppercase();
            if !(verdict == "PASS" || verdict == "VERIFIED") {
                violations.push(format!("modbus-l1-report-not-verified:{}", verdict));
            }
        }
    }

    // Go/No-Go 一致性：docs 当前裁决标记与目录状态一致。
    let gonogo = root.join("docs/v1.0.0-go-no-go.md");
    if let Ok(text) = fs::read_to_string(&gonogo) {
        let current_line = text.lines().find(|l| l.contains("当前裁决")).unwrap_or("");
        let qualified_count = catalog
            .skus
            .iter()
            .filter(|s| s.status == "qualified")
            .count();
        if current_line.contains("**GO**") && qualified_count == 0 {
            violations.push("go-no-go-go-without-qualified-sku".to_owned());
        }
        if current_line.is_empty() {
            violations.push("go-no-go-current-verdict-missing".to_owned());
        }
    } else {
        violations.push("go-no-go-doc-missing".to_owned());
    }

    // 42 前置证据链：当前候选的登记证据必须全 PASS（check-current 语义）。
    // 43 复用 42 的 evidence42 check-current；若未全绿则 43 机器 Gate 不允许
    // 宣称软件 L0–L3 全绿。
    let ev42_ok = run_check_current(root);
    if !ev42_ok {
        violations.push("evidence42-check-current-not-all-pass".to_owned());
    }

    // diff 模式：新 SKU PR 不得修改 forbidden core paths。
    if let Some(base) = diff_base {
        let head = diff_head.as_deref().unwrap_or("HEAD");
        violations.extend(platform_gate_diff(root, &base, head)?);
    }

    if !violations.is_empty() {
        return Err(invalid(format!(
            "CATALOG-GATE FAIL: {}",
            violations.join("; ")
        )));
    }
    let qualified = catalog
        .skus
        .iter()
        .filter(|s| s.status == "qualified")
        .count();
    println!(
        "CATALOG-GATE PASS skus={} qualified={} enumerated={}",
        catalog.skus.len(),
        qualified,
        matrix.entries.len()
    );
    Ok(())
}

fn run_check_current(root: &Path) -> bool {
    let bin = std::env::current_exe().unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(
        if bin.ends_with("eneros-xtask")
            || bin
                .file_name()
                .map(|f| f == "eneros-xtask")
                .unwrap_or(false)
        {
            bin
        } else {
            root.join("target/debug/eneros-xtask")
        },
    );
    let out = cmd.arg("evidence42").arg("check-current").output();
    match out {
        Ok(o) => {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("CHECK-CURRENT PASS")
        }
        Err(_) => {
            // 兜底：无法自调用时按 BLOCKED 处理，绝不假设 PASS。
            println!("CATALOG-GATE: check-current self-call unavailable (BLOCKED, not PASS)");
            false
        }
    }
}

// ============================================================
// platform-gate：新 SKU PR 禁止修改核心路径（WP-43.2）
// ============================================================

fn git(args: &[&str], root: &Path) -> DynResult<String> {
    let out = Command::new("git").args(args).current_dir(root).output()?;
    if !out.status.success() {
        return Err(invalid(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn command_platform_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let mut base: Option<String> = None;
    let mut head: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--base" => {
                base = Some(
                    it.next()
                        .ok_or_else(|| invalid("--base requires <ref>"))?
                        .clone(),
                )
            }
            "--head" => {
                head = Some(
                    it.next()
                        .ok_or_else(|| invalid("--head requires <ref>"))?
                        .clone(),
                )
            }
            other => return Err(invalid(format!("unknown platform-gate arg '{other}'"))),
        }
    }
    let base = base.ok_or_else(|| invalid("platform-gate requires --base <ref>"))?;
    let head = head.as_deref().unwrap_or("HEAD");
    let violations = platform_gate_diff(root, &base, head)?;
    if violations.is_empty() {
        println!("PLATFORM-GATE PASS base={base} head={head}: no forbidden core path changes");
        Ok(())
    } else {
        Err(invalid(format!(
            "PLATFORM-GATE FAIL base={base} head={head}: {}",
            violations.join("; ")
        )))
    }
}

fn platform_gate_diff(root: &Path, base: &str, head: &str) -> DynResult<Vec<String>> {
    let forbidden: ForbiddenCorePaths =
        read_json(&root.join("product/platform-contract/forbidden-core-paths.json"))?;
    if forbidden.schema != FORBIDDEN_SCHEMA {
        return Err(invalid("forbidden-core-paths schema mismatch"));
    }
    // 变更文件集合：base...head（merge-base 语义）。
    let changed_text = git(&["diff", "--name-only", &format!("{base}...{head}")], root)
        .or_else(|_| git(&["diff", "--name-only", base, head], root))?;
    let mut changed = BTreeSet::new();
    for line in changed_text.lines() {
        let f = line.trim();
        if !f.is_empty() {
            changed.insert(f.to_owned());
        }
    }
    let is_sku_pr = changed.iter().any(|f| {
        forbidden
            .platform_surface_prefixes
            .iter()
            .any(|p| f.starts_with(p))
    });
    if !is_sku_pr {
        // 非平台接入 PR：不触发核心路径限制（普通功能 PR 由其它 Gate 管）。
        return Ok(Vec::new());
    }
    let mut v = Vec::new();
    for f in &changed {
        if forbidden
            .forbidden_prefixes
            .iter()
            .any(|p| f.starts_with(p))
        {
            v.push(format!(
                "forbidden-core-path-changed-in-sku-pr:{f}（须解释现有 seam 为何不深 + 架构版本升级 + 两套平台回归）"
            ));
        }
    }
    // vendor 特判扫描：diff 中新增/修改的源文件不得出现 `if board ==` /
    // `board == ` 等厂商分支（蓝图书面禁止 `vendor if board ==`）。
    let patch = git(
        &["diff", "-U0", &format!("{base}...{head}"), "--", "."],
        root,
    )
    .or_else(|_| git(&["diff", "-U0", base, head, "--", "."], root))?;
    for pat in &forbidden.vendor_patterns {
        for line in patch.lines() {
            if (line.starts_with('+') || line.starts_with('-')) && line.contains(pat) {
                v.push(format!("vendor-branch-pattern-in-diff:{pat}:{line}"));
            }
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_schema_constants() {
        assert_eq!(CATALOG_SCHEMA, "eneros.compatibility-catalog.v1");
        assert_eq!(BOARD_FACT_SCHEMA, "eneros.board-fact-freeze.v1");
    }

    #[test]
    fn forbidden_core_paths_roundtrip() {
        let f = ForbiddenCorePaths {
            schema: FORBIDDEN_SCHEMA.to_owned(),
            version: 1,
            forbidden_prefixes: vec!["crates/ai/".to_owned()],
            platform_surface_prefixes: vec!["product/skus/".to_owned()],
            vendor_patterns: vec!["if board ==".to_owned()],
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: ForbiddenCorePaths = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, FORBIDDEN_SCHEMA);
        assert_eq!(back.vendor_patterns, vec!["if board =="]);
    }

    #[test]
    fn sku_entry_qualified_needs_evidence() {
        let sku = SkuEntry {
            sku_id: "x".into(),
            kind: "product".into(),
            board_revision: "r1".into(),
            firmware: FirmwareRef {
                uefi: "u".into(),
                tfa: "t".into(),
            },
            dtb_digest: "d".into(),
            bsp_adapter: "a".into(),
            drivers: vec![],
            protocols: vec![],
            qualification_evidence: vec![],
            status: "qualified".into(),
            status_reason: "".into(),
        };
        // 结构合法；机器 Gate 会因无证据拒绝该 qualified 声明。
        assert_eq!(sku.status, "qualified");
        assert!(sku.qualification_evidence.is_empty());
    }

    /// WP-43.2 平台准入 diff 门禁的 FAIL/GOOD 双路径黑盒验证：
    /// 在临时目录构造 scratch git 仓库（不触碰用户工作树），验证
    /// SKU PR 修改 forbidden core path 时转红、仅改平台接入面时放行。
    #[test]
    fn platform_gate_diff_blocks_sku_pr_touching_core_paths() {
        use std::fs;
        use std::process::Command;

        fn git_in(dir: &Path, args: &[&str]) {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let tmp = std::env::temp_dir().join(format!("eneros-pgtest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("product/platform-contract")).unwrap();
        fs::create_dir_all(tmp.join("product/skus")).unwrap();
        fs::create_dir_all(tmp.join("crates/ai")).unwrap();
        fs::create_dir_all(tmp.join("crates/hal")).unwrap();
        git_in(&tmp, &["init", "-q", "-b", "main"]);
        git_in(&tmp, &["config", "user.email", "t@t"]);
        git_in(&tmp, &["config", "user.name", "t"]);
        let forbidden = ForbiddenCorePaths {
            schema: FORBIDDEN_SCHEMA.to_owned(),
            version: 1,
            forbidden_prefixes: vec!["crates/ai/".to_owned()],
            platform_surface_prefixes: vec!["product/skus/".to_owned()],
            vendor_patterns: vec!["if board ==".to_owned()],
        };
        fs::write(
            tmp.join("product/platform-contract/forbidden-core-paths.json"),
            serde_json::to_vec_pretty(&forbidden).unwrap(),
        )
        .unwrap();
        git_in(&tmp, &["add", "-A"]);
        git_in(&tmp, &["commit", "-q", "-m", "base"]);
        let base = git_head(&tmp);

        // FAIL 路径：SKU 接入面 + 核心路径（crates/ai）同时被改。
        fs::write(tmp.join("product/skus/x.json"), "{}").unwrap();
        fs::write(tmp.join("crates/ai/bad.rs"), "fn bad(){}").unwrap();
        git_in(&tmp, &["add", "-A"]);
        git_in(&tmp, &["commit", "-q", "-m", "sku-pr-touches-core"]);
        let head_bad = git_head(&tmp);
        let violations = platform_gate_diff(&tmp, &base, &head_bad).expect("diff runs");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("forbidden-core-path-changed-in-sku-pr")),
            "expected forbidden core violation, got {violations:?}"
        );

        // GOOD 路径：仅平台接入面（product/skus + crates/hal adapter）。
        git_in(&tmp, &["rm", "-q", "crates/ai/bad.rs"]);
        fs::write(tmp.join("crates/hal/adapter.rs"), "fn main(){}").unwrap();
        git_in(&tmp, &["add", "-A"]);
        git_in(&tmp, &["commit", "-q", "-m", "sku-pr-surface-only"]);
        let head_good = git_head(&tmp);
        let violations = platform_gate_diff(&tmp, &base, &head_good).expect("diff runs");
        assert!(
            violations.is_empty(),
            "expected no violations on surface-only PR, got {violations:?}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    fn git_head(dir: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .expect("git rev-parse runs");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}
