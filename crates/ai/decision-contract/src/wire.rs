//! wire 辅助（小端读写 + magic 常量；decoder 先校后分配的共享原语）。

/// `CandidatePlan` V1 magic。
pub const MAGIC_CAND: [u8; 4] = *b"CAND";
/// `CertifiedPlan` V1 magic。
pub const MAGIC_CERT: [u8; 4] = *b"CERT";
/// `CommandIntent` V1 magic（定义于 intent.rs；此处登记常量对照）。
pub use crate::intent::INTENT_MAGIC;

/// 约束数上界（防御性；.36 RuleBundle 编译面同参）。
pub const MAX_CONSTRAINTS: usize = 64;

pub(crate) fn put_u16(out: &mut alloc::vec::Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32(out: &mut alloc::vec::Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u64(out: &mut alloc::vec::Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn get_u16(buf: &[u8], pos: usize) -> Result<[u8; 2], crate::error::ContractError> {
    if pos + 2 > buf.len() {
        return Err(crate::error::ContractError::Malformed("truncated"));
    }
    Ok(buf[pos..pos + 2].try_into().unwrap())
}

pub(crate) fn get_u64(buf: &[u8], pos: usize) -> Result<[u8; 8], crate::error::ContractError> {
    if pos + 8 > buf.len() {
        return Err(crate::error::ContractError::Malformed("truncated"));
    }
    Ok(buf[pos..pos + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_helpers_bounds() {
        let b = [1u8, 2, 3];
        assert!(get_u16(&b, 2).is_err());
        assert!(get_u64(&b, 0).is_err());
        assert_eq!(get_u16(&b, 0).unwrap(), [1, 2]);
    }
}
