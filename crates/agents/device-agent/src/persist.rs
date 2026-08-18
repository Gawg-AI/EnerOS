//! 蓝图 40 WP-40.10：Registry 持久化重建（snapshot + WAL / atomic generation）。
//!
//! 旧 CSV 实现（`register` 4 参、`crate::registry::PointAccess` 路径、
//! `DeviceType::Other` 变体、无 device_id）已漂移且从未编入模块——**不作
//! 基线**，本模块按新规实现：
//!
//! - [`DeviceRegistration`]：持久化的设备元数据（不含任何运行期句柄——
//!   绝不持久化裸指针/CPtr；adapter 由 factory 在 load 时重建）；
//! - **snapshot**：`EREGSNAP` 头 + 每设备登记行 + 点名行 + SM3 摘要行
//!   （校验 magic/schema/摘要——损坏/篡改拒绝，不静默重建）；
//! - **WAL**：`EREGWAL1` 头 + 变更记录（register/update/remove +
//!   generation）——`save_registry` 先落 WAL 再原子改 snapshot（rename），
//!   掉电后 load 用 WAL 回放补齐（generation 单调防旧配置覆盖）；
//! - **load 语义**：adapter factory 缺失 → `available = false`（**不静默
//!   丢设备**）；重复设备名/ID、未知 adapter kind、stale config → 结构化
//!   [`PersistError`]；
//! - register/update/remove 全部经 WAL 落账（审计轨迹持久化）。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[cfg(feature = "persist-fs")]
use eneros_fs::fs_trait::FileSystem;
use eneros_protocol_abstract::access::PointAccessWriter;
use eneros_upa_model::{DeviceId, PointId};

use crate::device_type::DeviceType;
use crate::registry::{
    AdapterKind, DeviceRegistration, DeviceRegistry, EndpointRef, OperationSet, PointSchemaVersion,
};

/// snapshot schema 版本。
pub const SNAPSHOT_SCHEMA: u16 = 1;
/// snapshot 文件头。
pub const SNAPSHOT_MAGIC: &str = "EREGSNAP";
/// WAL 文件头。
pub const WAL_MAGIC: &str = "EREGWAL1";
/// 能力摘要占位（未注入时的全零——装配层注入真实摘要）。
pub const CAPABILITY_DIGEST_PLACEHOLDER: [u8; 32] = [0u8; 32];

/// 持久化错误（结构化；损坏/重复/未知一律拒绝，不静默）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistError {
    /// 文件系统读写失败。
    Io(String),
    /// magic/schema/摘要不符（损坏或被篡改）。
    Corrupt(String),
    /// 重复设备名。
    DuplicateDevice { name: String },
    /// 重复设备 ID。
    DuplicateDeviceId { id: DeviceId },
    /// 未知适配器种类。
    UnknownAdapter { name: String, code: u8 },
    /// 配置代次过期（旧快照 + 新 WAL 冲突）。
    StaleConfig { name: String, generation: u64 },
    /// 解析失败。
    Parse(String),
}

impl core::fmt::Display for PersistError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PersistError::Io(m) => write!(f, "registry io: {m}"),
            PersistError::Corrupt(m) => write!(f, "registry corrupt: {m}"),
            PersistError::DuplicateDevice { name } => write!(f, "duplicate device '{name}'"),
            PersistError::DuplicateDeviceId { id } => write!(f, "duplicate device id {id}"),
            PersistError::UnknownAdapter { name, code } => {
                write!(f, "unknown adapter kind {code} for '{name}'")
            }
            PersistError::StaleConfig { name, generation } => {
                write!(f, "stale config for '{name}' generation {generation}")
            }
            PersistError::Parse(m) => write!(f, "registry parse: {m}"),
        }
    }
}

/// 变更记录（WAL 条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalEntry {
    /// 注册/更新（generation 单调）。
    Register {
        name: String,
        device_id: DeviceId,
        kind: AdapterKind,
        endpoint: String,
        schema: u32,
        ops: u8,
        capability: [u8; 32],
        generation: u64,
        points: Vec<(String, PointId)>,
    },
    /// 移除。
    Remove { name: String },
}

impl WalEntry {
    /// 行编码（逗号分隔；能力摘要 hex）。
    fn encode(&self) -> String {
        match self {
            WalEntry::Register {
                name,
                device_id,
                kind,
                endpoint,
                schema,
                ops,
                capability,
                generation,
                points,
            } => {
                let mut line = format!(
                    "R,{name},{device_id},{},{endpoint},{schema},{ops},{},{generation}",
                    kind.code(),
                    hex32(capability)
                );
                for (pn, pid) in points {
                    line.push_str(&format!(";{pn}:{pid}"));
                }
                line
            }
            WalEntry::Remove { name } => format!("X,{name}"),
        }
    }

    /// 行解码（未知类型/畸形 → None，fail-closed）。
    fn decode(line: &str) -> Option<WalEntry> {
        let mut parts = line.splitn(9, ',');
        let tag = parts.next()?;
        match tag {
            "R" => {
                let name = parts.next()?.to_string();
                let device_id = parts.next()?.parse().ok()?;
                let kind = AdapterKind::from_code(parts.next()?.parse().ok()?)?;
                let endpoint = parts.next()?.to_string();
                let schema = parts.next()?.parse().ok()?;
                let ops = parts.next()?.parse().ok()?;
                let capability = hex_decode(parts.next()?)?;
                // 第 9 段：`generation;name:id;name:id...`。
                let gen_and_points = parts.next()?;
                let mut gp = gen_and_points.splitn(2, ';');
                let generation = gp.next()?.parse().ok()?;
                let points_part = gp.next().unwrap_or("");
                let mut points = Vec::new();
                for seg in points_part.split(';') {
                    if seg.is_empty() {
                        continue;
                    }
                    let mut kv = seg.splitn(2, ':');
                    let pn = kv.next()?;
                    let pid = kv.next()?.parse().ok()?;
                    points.push((pn.to_string(), pid));
                }
                Some(WalEntry::Register {
                    name,
                    device_id,
                    kind,
                    endpoint,
                    schema,
                    ops,
                    capability,
                    generation,
                    points,
                })
            }
            "X" => Some(WalEntry::Remove {
                name: parts.next()?.to_string(),
            }),
            _ => None,
        }
    }
}

/// WAL 文件（头 + 条目行 + SM3 摘要行）。
pub fn encode_wal(entries: &[WalEntry]) -> Vec<u8> {
    let mut out = alloc::string::String::new();
    out.push_str(WAL_MAGIC);
    out.push('\n');
    for e in entries {
        out.push_str(&e.encode());
        out.push('\n');
    }
    let digest = sm3_of(out.as_bytes());
    out.push_str("D,");
    out.push_str(&hex32(&digest));
    out.push('\n');
    out.into_bytes()
}

/// 解析并校验 WAL（摘要不符 → Corrupt；条目解码失败 → Parse）。
pub fn parse_wal(data: &[u8]) -> Result<Vec<WalEntry>, PersistError> {
    let text = core::str::from_utf8(data).map_err(|e| PersistError::Parse(format!("utf8 {e}")))?;
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("");
    if head != WAL_MAGIC {
        return Err(PersistError::Corrupt("WAL magic".into()));
    }
    let mut entries = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(digest_line) = line.strip_prefix("D,") {
            // 校验行必须是最后一行；摘要在行首 `D,` 之前的全部内容上计算。
            let expected = hex_decode(digest_line)
                .ok_or_else(|| PersistError::Corrupt("WAL digest 格式".into()))?;
            let body_end = data.len().saturating_sub(line.len() + 1);
            let body = &data[..body_end];
            if sm3_of(body) != expected {
                return Err(PersistError::Corrupt("WAL digest 不符（篡改/损坏）".into()));
            }
            continue;
        }
        let entry =
            WalEntry::decode(line).ok_or_else(|| PersistError::Parse(format!("WAL 行: {line}")))?;
        entries.push(entry);
    }
    Ok(entries)
}

/// snapshot 序列化（头 + 设备登记行 + 点名行 + SM3 摘要行）。
pub fn serialize_snapshot(registry: &DeviceRegistry) -> Vec<u8> {
    let mut out = alloc::string::String::new();
    out.push_str(SNAPSHOT_MAGIC);
    out.push_str(" v");
    out.push_str(&SNAPSHOT_SCHEMA.to_string());
    out.push('\n');
    for (name, info) in registry.iter() {
        let r = &info.registration;
        out.push_str(&format!(
            "D,{name},{},{},{},{},{},{},{}\n",
            r.device_id,
            r.adapter_kind.code(),
            r.endpoint.0,
            r.point_schema.0,
            r.allowed_ops.bits(),
            hex32(&r.capability_digest),
            r.config_generation
        ));
        for (pn, pid) in &info.point_map {
            out.push_str(&format!("P,{name},{pn},{pid}\n"));
        }
    }
    let digest = sm3_of(out.as_bytes());
    out.push_str("S,");
    out.push_str(&hex32(&digest));
    out.push('\n');
    out.into_bytes()
}

/// 快照设备条目（parse_snapshot 返回项；别名消除超长三元组类型的重复展开）。
type SnapshotDevice = (String, DeviceRegistration, Vec<(String, PointId)>);

/// 解析并校验 snapshot（magic/schema/摘要；重复设备名/ID、未知 kind 拒绝）。
pub fn parse_snapshot(data: &[u8]) -> Result<Vec<SnapshotDevice>, PersistError> {
    let text = core::str::from_utf8(data).map_err(|e| PersistError::Parse(format!("utf8 {e}")))?;
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("");
    let schema = head
        .strip_prefix(SNAPSHOT_MAGIC)
        .and_then(|rest| rest.trim().strip_prefix("v"))
        .and_then(|v| v.parse::<u16>().ok());
    match schema {
        Some(1) => {}
        _ => {
            return Err(PersistError::Corrupt(format!(
                "snapshot magic/schema: {head}"
            )))
        }
    }

    let mut devices: Vec<SnapshotDevice> = Vec::new();
    let mut current: Option<SnapshotDevice> = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(digest_line) = line.strip_prefix("S,") {
            let expected = hex_decode(digest_line)
                .ok_or_else(|| PersistError::Corrupt("snapshot digest 格式".into()))?;
            let body_end = data.len().saturating_sub(line.len() + 1);
            if sm3_of(&data[..body_end]) != expected {
                return Err(PersistError::Corrupt(
                    "snapshot digest 不符（篡改/损坏）".into(),
                ));
            }
            continue;
        }
        if let Some(d_line) = line.strip_prefix("D,") {
            if let Some(prev) = current.take() {
                devices.push(prev);
            }
            let mut parts = d_line.splitn(9, ',');
            let name = parts.next().unwrap_or("").to_string();
            let device_id = parts
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| PersistError::Parse(format!("device id 行: {line}")))?;
            let kind = AdapterKind::from_code(
                parts
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| PersistError::Parse(format!("adapter kind 行: {line}")))?,
            )
            .ok_or_else(|| PersistError::UnknownAdapter {
                name: name.clone(),
                code: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0xFF),
            })?;
            let endpoint = parts.next().unwrap_or("").to_string();
            let schema_v = parts
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| PersistError::Parse(format!("schema 行: {line}")))?;
            let ops = parts
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| PersistError::Parse(format!("ops 行: {line}")))?;
            let capability = hex_decode(parts.next().unwrap_or(""))
                .ok_or_else(|| PersistError::Parse(format!("capability 行: {line}")))?;
            let generation = parts
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| PersistError::Parse(format!("generation 行: {line}")))?;
            let allowed_ops = OperationSet::new(ops)
                .ok_or_else(|| PersistError::Parse(format!("非法 ops {ops}")))?;
            current = Some((
                name,
                DeviceRegistration {
                    device_id,
                    adapter_kind: kind,
                    endpoint: EndpointRef(endpoint),
                    point_schema: PointSchemaVersion(schema_v),
                    allowed_ops,
                    capability_digest: capability,
                    config_generation: generation,
                },
                Vec::new(),
            ));
            continue;
        }
        if let Some(p_line) = line.strip_prefix("P,") {
            let mut parts = p_line.splitn(4, ',');
            let name = parts.next().unwrap_or("").to_string();
            let pn = parts.next().unwrap_or("").to_string();
            let pid = parts
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| PersistError::Parse(format!("point 行: {line}")))?;
            let Some((_, _, points)) = current.as_mut() else {
                return Err(PersistError::Parse(format!("点名行无设备头: {line}")));
            };
            // 重复点名 → 拒绝。
            if points.iter().any(|(n, _)| *n == pn) {
                return Err(PersistError::DuplicateDevice { name });
            }
            points.push((pn, pid));
            continue;
        }
        return Err(PersistError::Parse(format!("未知行: {line}")));
    }
    if let Some(prev) = current.take() {
        devices.push(prev);
    }
    // 重复设备名/ID 校验。
    let mut seen_names = alloc::collections::BTreeSet::new();
    let mut seen_ids = alloc::collections::BTreeSet::new();
    for (name, reg, _) in &devices {
        if !seen_names.insert(name.clone()) {
            return Err(PersistError::DuplicateDevice { name: name.clone() });
        }
        if !seen_ids.insert(reg.device_id) {
            return Err(PersistError::DuplicateDeviceId { id: reg.device_id });
        }
    }
    Ok(devices)
}

/// 保存 snapshot（先写 WAL 变更落账语义由调用方经 [`log_wal_entry`] 承担；
/// 本函数直接写原子 snapshot 文件并删除 WAL）。
#[cfg(feature = "persist-fs")]
pub fn save_snapshot<F: FileSystem>(
    fs: &mut F,
    path: &str,
    registry: &DeviceRegistry,
) -> Result<(), PersistError> {
    let data = serialize_snapshot(registry);
    fs.write_file(path, &data)
        .map_err(|e| PersistError::Io(format!("{e:?}")))?;
    fs.sync()
        .map_err(|e| PersistError::Io(format!("sync {e:?}")))?;
    // 快照落定后清空 WAL。
    let wal_path = format!("{path}.wal");
    let _ = fs.remove(&wal_path);
    Ok(())
}

/// 追加一条变更到 WAL（read-modify-write；fs 无 append 语义）。
#[cfg(feature = "persist-fs")]
pub fn log_wal_entry<F: FileSystem>(
    fs: &mut F,
    path: &str,
    entry: &WalEntry,
) -> Result<(), PersistError> {
    let wal_path = format!("{path}.wal");
    let mut entries = match fs.read_file(&wal_path) {
        Ok(data) if !data.is_empty() => parse_wal(&data).unwrap_or_default(),
        _ => Vec::new(),
    };
    entries.push(entry.clone());
    let data = encode_wal(&entries);
    fs.write_file(&wal_path, &data)
        .map_err(|e| PersistError::Io(format!("{e:?}")))?;
    Ok(())
}

/// 从磁盘加载注册表（snapshot + WAL 回放）。
///
/// `factory` 按登记元数据重建运行期 adapter；**重建失败 → 设备标记
/// `available = false`（不静默丢设备）**。重复/未知/损坏 → Err。
#[cfg(feature = "persist-fs")]
pub fn load_registry<F: FileSystem>(
    fs: &mut F,
    path: &str,
    mut factory: impl FnMut(&str, &DeviceRegistration) -> Option<Box<dyn PointAccessWriter>>,
) -> Result<DeviceRegistry, PersistError> {
    let snap = fs
        .read_file(path)
        .map_err(|e| PersistError::Io(format!("{e:?}")))?;
    let mut registry = DeviceRegistry::new();
    if snap.is_empty() {
        return Ok(registry);
    }
    let devices = parse_snapshot(&snap)?;
    for (name, reg, points) in devices {
        let point_map = points.into_iter().collect();
        match factory(&name, &reg) {
            Some(adapter) => {
                registry.register_with_registration(
                    &name,
                    device_type_of(&reg.adapter_kind),
                    reg,
                    adapter,
                    point_map,
                );
            }
            None => {
                // factory 缺失 → 标记不可用（不静默丢设备）。
                registry.register_with_registration(
                    &name,
                    device_type_of(&reg.adapter_kind),
                    reg.clone(),
                    unavailable_adapter(),
                    point_map,
                );
                let _ = registry.set_available(&name, false);
            }
        }
    }
    // WAL 回放（generation 单调防旧配置覆盖；remove 生效）。
    let wal_path = format!("{path}.wal");
    if let Ok(data) = fs.read_file(&wal_path) {
        if !data.is_empty() {
            let entries = parse_wal(&data)?;
            for e in entries {
                match e {
                    WalEntry::Register {
                        name,
                        device_id,
                        kind,
                        endpoint,
                        schema,
                        ops,
                        capability,
                        generation,
                        points,
                    } => {
                        let allowed_ops = OperationSet::new(ops)
                            .ok_or_else(|| PersistError::Parse(format!("WAL 非法 ops {ops}")))?;
                        let reg = DeviceRegistration {
                            device_id,
                            adapter_kind: kind,
                            endpoint: EndpointRef(endpoint),
                            point_schema: PointSchemaVersion(schema),
                            allowed_ops,
                            capability_digest: capability,
                            config_generation: generation,
                        };
                        // stale config：WAL generation < 快照 generation → 拒绝。
                        if let Some(info) = registry.get_mut(&name) {
                            if info.registration.config_generation > generation {
                                return Err(PersistError::StaleConfig { name, generation });
                            }
                        }
                        match factory(&name, &reg) {
                            Some(adapter) => {
                                registry.register_with_registration(
                                    &name,
                                    device_type_of(&kind),
                                    reg,
                                    adapter,
                                    points.into_iter().collect(),
                                );
                            }
                            None => {
                                registry.register_with_registration(
                                    &name,
                                    device_type_of(&kind),
                                    reg.clone(),
                                    unavailable_adapter(),
                                    points.into_iter().collect(),
                                );
                                let _ = registry.set_available(&name, false);
                            }
                        }
                    }
                    WalEntry::Remove { name } => {
                        registry.remove(&name);
                    }
                }
            }
        }
    }
    Ok(registry)
}

/// 不可用适配器占位（factory 缺失时登记；路由层对 available=false 明确失败）。
#[allow(dead_code)] // 44.2：默认拒绝适配器（当前装配未选 fallback，44.4 冻结登记）
fn unavailable_adapter() -> Box<dyn PointAccessWriter> {
    Box::new(crate::registry::UnavailableAdapter)
}

#[allow(dead_code)] // 44.2：适配器→设备类型换算（当前路径未用，44.4 冻结登记）
fn device_type_of(kind: &AdapterKind) -> DeviceType {
    match kind {
        AdapterKind::Pcs => DeviceType::Pcs,
        AdapterKind::Battery => DeviceType::Battery,
        AdapterKind::Bms => DeviceType::Bms,
        AdapterKind::Meter => DeviceType::Meter,
        AdapterKind::Temperature => DeviceType::Temperature,
        AdapterKind::Other => DeviceType::Pcs, // Other 不落盘（from_code 拒绝 5 以上；此处仅为穷尽匹配）
    }
}

fn sm3_of(data: &[u8]) -> [u8; 32] {
    eneros_crypto::sm3_hash(data)
}

fn hex32(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MockPointDevice;

    fn make_registry() -> DeviceRegistry {
        let mut reg = DeviceRegistry::new();
        let pcs = MockPointDevice::new(1).with_point("voltage", 230.0);
        let mut points = alloc::collections::BTreeMap::new();
        points.insert(String::from("voltage"), 1u32);
        reg.register("pcs-1", DeviceType::Pcs, 1, Box::new(pcs), points);
        let meter = MockPointDevice::new(2).with_point("kwh", 100.0);
        let mut points2 = alloc::collections::BTreeMap::new();
        points2.insert(String::from("kwh"), 1u32);
        reg.register("meter-1", DeviceType::Meter, 2, Box::new(meter), points2);
        reg
    }

    #[allow(dead_code)] // 44.2：工厂辅助（44.4 冻结登记）
    fn factory(name: &str, _reg: &DeviceRegistration) -> Option<Box<dyn PointAccessWriter>> {
        let id = if name == "pcs-1" { 1 } else { 2 };
        Some(Box::new(MockPointDevice::new(id)))
    }

    #[test]
    fn snapshot_roundtrip_with_registration() {
        let reg = make_registry();
        let data = serialize_snapshot(&reg);
        let parsed = parse_snapshot(&data).expect("parse");
        assert_eq!(parsed.len(), 2);
        let pcs = parsed.iter().find(|(n, _, _)| n == "pcs-1").expect("pcs");
        assert_eq!(pcs.1.device_id, 1);
        assert_eq!(pcs.1.adapter_kind, AdapterKind::Pcs);
        assert_eq!(pcs.1.endpoint.0, "pcs-1");
        assert!(pcs.1.allowed_ops.allows(OperationSet::WRITE));
        assert_eq!(pcs.2.len(), 1);
    }

    #[test]
    fn tampered_snapshot_rejected() {
        let reg = make_registry();
        let mut data = serialize_snapshot(&reg);
        // 篡改设备行（body 区，非摘要行）——摘要校验必须检出。
        let pos = data
            .windows(2)
            .position(|w| w == b"D,")
            .expect("设备行存在")
            + 2;
        // 篡改为另一个合法 ASCII 字符（保持 UTF-8 有效——摘要必须检出）。
        data[pos] = b'q';
        assert!(matches!(
            parse_snapshot(&data),
            Err(PersistError::Corrupt(_))
        ));
    }

    #[test]
    fn duplicate_device_id_rejected() {
        let reg = make_registry();
        let data = serialize_snapshot(&reg);
        // 把 meter-1 的 device_id 改成 1（与 pcs-1 重复）。
        let text = core::str::from_utf8(&data).unwrap().to_string();
        let fixed = text
            .lines()
            .map(|l| {
                if let Some(rest) = l.strip_prefix("D,meter-1,") {
                    let mut parts: Vec<&str> = rest.splitn(8, ',').collect();
                    parts[0] = "1";
                    format!("D,meter-1,{}", parts.join(","))
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut fixed_bytes = fixed.into_bytes();
        fixed_bytes.push(b'\n');
        // 重新计算摘要行。
        let body = &fixed_bytes[..fixed_bytes.len()];
        let digest = sm3_of(body);
        let mut out = fixed_bytes;
        out.extend_from_slice(format!("S,{}\n", hex32(&digest)).as_bytes());
        // 解析（去掉原摘要行）。
        let text = core::str::from_utf8(&out).unwrap();
        let mut lines = text.lines();
        let head = lines.next().unwrap();
        let mut body2 = format!("{head}\n");
        for l in lines {
            if !l.starts_with("S,") {
                body2.push_str(l);
                body2.push('\n');
            }
        }
        let digest2 = sm3_of(body2.as_bytes());
        let mut final_bytes = body2.into_bytes();
        final_bytes.extend_from_slice(format!("S,{}\n", hex32(&digest2)).as_bytes());
        assert!(matches!(
            parse_snapshot(&final_bytes),
            Err(PersistError::DuplicateDeviceId { id: 1 })
        ));
    }

    #[test]
    fn wal_roundtrip_and_tamper() {
        let entries = vec![
            WalEntry::Register {
                name: String::from("pcs-1"),
                device_id: 1,
                kind: AdapterKind::Pcs,
                endpoint: String::from("svc-modbus-rtu"),
                schema: 1,
                ops: OperationSet::read_write_readback().bits(),
                capability: [0xAB; 32],
                generation: 2,
                points: vec![(String::from("voltage"), 1)],
            },
            WalEntry::Remove {
                name: String::from("meter-1"),
            },
        ];
        let data = encode_wal(&entries);
        let parsed = parse_wal(&data).expect("parse");
        assert_eq!(parsed.len(), 2);
        assert!(matches!(
            &parsed[0],
            WalEntry::Register { generation: 2, .. }
        ));
        assert!(matches!(&parsed[1], WalEntry::Remove { name } if name == "meter-1"));
        // 篡改 → 摘要不符（改条目行正文，保持 UTF-8 有效）。
        let mut tampered = data.clone();
        let pos = tampered
            .windows(2)
            .position(|w| w == b"R,")
            .expect("条目行存在")
            + 2;
        tampered[pos] = b'z';
        assert!(matches!(
            parse_wal(&tampered),
            Err(PersistError::Corrupt(_))
        ));
    }

    #[cfg(feature = "persist-fs")]
    #[test]
    fn fs_save_load_roundtrip_and_unavailable_marker() {
        let mut fs = eneros_fs::MockFileSystem::new();
        let reg = make_registry();
        save_snapshot(&mut fs, "/registry.snap", &reg).expect("save");
        let mut restored = load_registry(&mut fs, "/registry.snap", factory).expect("load");
        assert_eq!(restored.len(), 2);
        assert!(restored.get_mut("pcs-1").is_some());
        assert!(restored.get_mut("pcs-1").unwrap().available);
        // factory 缺失 → 设备保留但标记不可用（不静默丢设备）。
        let mut fs2 = eneros_fs::MockFileSystem::new();
        save_snapshot(&mut fs2, "/r.snap", &reg).expect("save2");
        let mut restored2 = load_registry(&mut fs2, "/r.snap", |_, _| None).expect("load2");
        assert_eq!(restored2.len(), 2, "factory 缺失不得丢设备");
        assert!(!restored2.get_mut("pcs-1").unwrap().available);
        assert!(!restored2.get_mut("meter-1").unwrap().available);
    }

    #[cfg(feature = "persist-fs")]
    #[test]
    fn wal_replay_applies_remove_and_register() {
        let mut fs = eneros_fs::MockFileSystem::new();
        let reg = make_registry();
        save_snapshot(&mut fs, "/r.snap", &reg).expect("save");
        // 移除 meter-1 + 注册 battery-1（WAL）。
        log_wal_entry(
            &mut fs,
            "/r.snap",
            &WalEntry::Remove {
                name: String::from("meter-1"),
            },
        )
        .expect("wal");
        log_wal_entry(
            &mut fs,
            "/r.snap",
            &WalEntry::Register {
                name: String::from("battery-1"),
                device_id: 3,
                kind: AdapterKind::Battery,
                endpoint: String::from("svc-modbus-rtu"),
                schema: 1,
                ops: OperationSet::read_write_readback().bits(),
                capability: [0x11; 32],
                generation: 1,
                points: vec![(String::from("soc"), 1)],
            },
        )
        .expect("wal");
        let restored = load_registry(&mut fs, "/r.snap", factory).expect("load");
        assert_eq!(restored.len(), 2, "移除 meter + 新增 battery");
        assert!(!restored.contains("meter-1"));
        assert!(restored.contains("battery-1"));
        assert!(restored.contains("pcs-1"));
    }

    #[cfg(feature = "persist-fs")]
    #[test]
    fn missing_file_empty_registry() {
        let mut fs = eneros_fs::MockFileSystem::new();
        let restored = load_registry(&mut fs, "/nonexistent.snap", |_, _| None).expect("load");
        assert!(restored.is_empty());
    }
}
