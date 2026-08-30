//! 合同错误面（结构化；一切拒绝可观测）。

/// 产物构造/校验/解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// 全零 ID。
    ZeroId,
    /// 必填字段缺失。
    MissingField(&'static str),
    /// 值越界/非法。
    OutOfRange(&'static str),
    /// 时间窗非法（valid_until <= valid_from / 已过期）。
    InvalidWindow,
    /// 摘要不匹配（plan digest / payload sha）。
    DigestMismatch(&'static str),
    /// scope 越权（资产/点/租户不在授权范围）。
    ScopeMismatch(&'static str),
    /// 未知 reason code（fail-closed）。
    UnknownReasonCode(u8),
    /// wire 版本不支持（明确拒绝，不误解字节——[06] §4.4）。
    VersionUnsupported { found: u16, supported: u16 },
    /// wire 长度/布局非法。
    Malformed(&'static str),
    /// 计数乘法越界/超上限（decoder 先校后分配）。
    LimitExceeded(&'static str),
    /// 证书与计划绑定失败（plan_id/plan_sha256 不符）。
    CertificatePlanMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_structured() {
        let e = ContractError::MissingField("plan_id");
        assert!(matches!(e, ContractError::MissingField(_)));
        assert_ne!(ContractError::ZeroId, ContractError::ScopeMismatch("asset"));
    }
}
