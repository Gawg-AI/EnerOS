//! 设备注册表持久化（v0.227.0，B3-04）。
//!
//! 设备注册表此前仅内存（重启丢失）。本模块提供 fs 持久化：
//!
//! - [`save_registry`] — 将设备名/类型/点名映射序列化落盘；
//! - [`load_registry`] — 从磁盘恢复注册信息（adapter 由上层按类型重建）。
//!
//! 行格式：`<device_name>,<type_code>,<point_count>`，随后每点名一行
//! `<device_name>.<point_name>,<point_id>`。类型码：0=Pcs 1=Battery
//! 2=Bms 3=Meter 4=Other。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use eneros_fs::fs_trait::FileSystem;

use crate::device_type::DeviceType;
use crate::registry::{DeviceInfo, DeviceRegistry};

/// 序列化注册表为文本。
pub fn serialize_registry(registry: &DeviceRegistry) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, info) in registry.iter() {
        let type_code = type_code(&info.device_type);
        let mut line = String::new();
        line.push_str(name);
        line.push(',');
        line.push_str(&type_code.to_string());
        line.push(',');
        line.push_str(&info.point_map.len().to_string());
        line.push('\n');
        out.extend_from_slice(line.as_bytes());
        for (point_name, point_id) in &info.point_map {
            let mut pl = String::new();
            pl.push_str(name);
            pl.push('.');
            pl.push_str(point_name);
            pl.push(',');
            pl.push_str(&point_id.to_string());
            pl.push('\n');
            out.extend_from_slice(pl.as_bytes());
        }
    }
    out
}

/// 解析注册表文本（返回设备名→(类型, 点名映射)）。
pub fn parse_registry(data: &[u8]) -> Vec<(String, DeviceType, Vec<(String, u32)>)> {
    let text = match core::str::from_utf8(data) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut current: Option<(String, DeviceType, Vec<(String, u32)>)> = None;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, ',');
        let head = parts.next().unwrap_or("");
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        if head.is_empty() {
            continue;
        }
        if a.parse::<u32>().is_ok() && head.contains('.') {
            // 点名行：name.point,id
            if let (Some((_, _, points)), Ok(pid)) = (&mut current, b.parse::<u32>()) {
                if let Some(dot) = head.find('.') {
                    points.push((head[dot + 1..].to_string(), pid));
                }
            }
        } else if let Ok(code) = a.parse::<u32>() {
            // 设备行：name,type_code,count
            if let Some(prev) = current.take() {
                out.push(prev);
            }
            current = Some((head.to_string(), from_type_code(code), Vec::new()));
        }
    }
    if let Some(prev) = current.take() {
        out.push(prev);
    }
    out
}

/// 保存注册表到 fs。
pub fn save_registry<F: FileSystem>(fs: &mut F, path: &str, registry: &DeviceRegistry) -> Result<(), ()> {
    let data = serialize_registry(registry);
    fs.write_file(path, &data).map_err(|_| ())
}

/// 从 fs 加载注册信息。
///
/// `rebuild` 闭包按设备类型重建 PointAccess 适配器（真实协议 seam）。
pub fn load_registry<F: FileSystem>(
    fs: &mut F,
    path: &str,
    mut rebuild: impl FnMut(&str, DeviceType) -> Option<Box<dyn crate::registry::PointAccess>>,
) -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    let data = match fs.read_file(path) {
        Ok(d) => d,
        Err(_) => return registry,
    };
    for (name, dtype, points) in parse_registry(&data) {
        let Some(adapter) = rebuild(&name, dtype) else {
            continue;
        };
        let point_map = points.into_iter().collect();
        registry.register(&name, dtype, adapter, point_map);
    }
    registry
}

/// 设备类型 → 行编码。
fn type_code(t: &DeviceType) -> u32 {
    match t {
        DeviceType::Pcs => 0,
        DeviceType::Battery => 1,
        DeviceType::Bms => 2,
        DeviceType::Meter => 3,
        _ => 4,
    }
}

/// 行编码 → 设备类型。
fn from_type_code(code: u32) -> DeviceType {
    match code {
        0 => DeviceType::Pcs,
        1 => DeviceType::Battery,
        2 => DeviceType::Bms,
        3 => DeviceType::Meter,
        _ => DeviceType::Other,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MockPointDevice;

    fn make_registry() -> DeviceRegistry {
        let mut reg = DeviceRegistry::new();
        let pcs = MockPointDevice::new(1).with_point("voltage", 230.0);
        let mut points = alloc::collections::BTreeMap::new();
        points.insert(String::from("voltage"), 1);
        reg.register("pcs-1", DeviceType::Pcs, Box::new(pcs), points);
        let meter = MockPointDevice::new(2).with_point("kwh", 100.0);
        let mut points2 = alloc::collections::BTreeMap::new();
        points2.insert(String::from("kwh"), 1);
        reg.register("meter-1", DeviceType::Meter, Box::new(meter), points2);
        reg
    }

    #[test]
    fn test_serialize_parse_roundtrip() {
        let reg = make_registry();
        let data = serialize_registry(&reg);
        let parsed = parse_registry(&data);
        assert_eq!(parsed.len(), 2);
        let pcs = parsed.iter().find(|(n, _, _)| n == "pcs-1").expect("pcs");
        assert_eq!(pcs.1, DeviceType::Pcs);
        assert_eq!(pcs.2.len(), 1);
        assert_eq!(pcs.2[0].0, "voltage");
        let meter = parsed.iter().find(|(n, _, _)| n == "meter-1").expect("meter");
        assert_eq!(meter.1, DeviceType::Meter);
    }

    #[test]
    fn test_fs_save_load_roundtrip() {
        let mut fs = eneros_fs::MockFileSystem::new();
        let reg = make_registry();
        save_registry(&mut fs, "/devices.txt", &reg).expect("save");
        let restored = load_registry(&mut fs, "/devices.txt", |name, dtype| {
            // 真实重建：MockPointDevice 模拟协议适配器重建。
            Some(Box::new(MockPointDevice::new(if name == "pcs-1" { 1 } else { 2 })))
        });
        assert_eq!(restored.len(), 2);
        assert!(restored.get_mut("pcs-1").is_some());
        assert!(restored.get_mut("meter-1").is_some());
    }

    #[test]
    fn test_fs_missing_file_empty_registry() {
        let mut fs = eneros_fs::MockFileSystem::new();
        let restored = load_registry(&mut fs, "/nonexistent.txt", |_, _| None);
        assert!(restored.is_empty());
    }

    #[test]
    fn test_type_code_roundtrip() {
        for (t, c) in [
            (DeviceType::Pcs, 0u32),
            (DeviceType::Battery, 1),
            (DeviceType::Bms, 2),
            (DeviceType::Meter, 3),
        ] {
            assert_eq!(type_code(&t), c);
            assert_eq!(from_type_code(c), t);
        }
    }
}
