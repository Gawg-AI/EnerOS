//! 设备能力摘要——登记元数据规范编码的 SM3（SPEC-SERIES-2026-08-27/A · WP-A.3）.
//!
//! 四元组 `(endpoint, adapter_kind, point_schema_version, allowed_ops)`
//! 规范化编码后经 [`eneros_crypto::sm3_hash`] 单源计算：
//!
//! - 新鲜登记在 [`crate::registry::DeviceRegistry::register`] 内部兜底计算，
//!   蓝图 40 时代的全零占位退休；
//! - 回放路径统一走 [`crate::registry::DeviceRegistry::register_with_registration`]，
//!   其中按本四元组**重算**——旧快照/WAL 携带的全零摘要自动换真值自愈，
//!   persist 行格式不变；
//! - 退化全零编码拒绝登记（措辞对齐 `SecretHandle::is_zero_placeholder`
//!   全零守卫先例，fail-closed）。

use alloc::vec::Vec;

use eneros_crypto::sm3_hash;

use crate::registry::{AdapterKind, OperationSet, PointSchemaVersion};

/// 计算设备能力摘要：SM3(endpoint ‖ 0x00 ‖ kind_code ‖ schema_v_le32 ‖ ops_bits).
///
/// 规范化定界无歧义：endpoint 后固定一个 NUL 分隔（endpoint 为 EIPC 服务名/
/// 协议地址，不含 NUL），其余字段均为定长编码。同输入必同输出。
pub fn capability_digest(
    endpoint: &str,
    kind: AdapterKind,
    schema_version: PointSchemaVersion,
    allowed_ops: &OperationSet,
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(endpoint.len() + 7);
    buf.extend_from_slice(endpoint.as_bytes());
    buf.push(0x00);
    buf.push(kind.code());
    buf.extend_from_slice(&schema_version.0.to_le_bytes());
    buf.push(allowed_ops.bits());
    sm3_hash(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::PointSchemaVersion;

    #[test]
    fn digest_deterministic_and_input_sensitive() {
        let ops = OperationSet::read_write_readback();
        let a = capability_digest("svc-a", AdapterKind::Pcs, PointSchemaVersion(1), &ops);
        let b = capability_digest("svc-a", AdapterKind::Pcs, PointSchemaVersion(1), &ops);
        assert_eq!(a, b, "同输入必同输出");
        assert_ne!(a, [0u8; 32], "真值不得为全零占位");

        // 四个输入维度各自参与摘要（变更任一 → 摘要变化）。
        assert_ne!(
            a,
            capability_digest("svc-b", AdapterKind::Pcs, PointSchemaVersion(1), &ops)
        );
        assert_ne!(
            a,
            capability_digest("svc-a", AdapterKind::Bms, PointSchemaVersion(1), &ops)
        );
        assert_ne!(
            a,
            capability_digest("svc-a", AdapterKind::Pcs, PointSchemaVersion(2), &ops)
        );
        let ro = OperationSet::new(OperationSet::READ).expect("合法位标志");
        assert_ne!(
            a,
            capability_digest("svc-a", AdapterKind::Pcs, PointSchemaVersion(1), &ro)
        );
    }
}
