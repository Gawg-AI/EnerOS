//! IEC 104 主站错误类型.
//!
//! D5：复用 `eneros-iec104-slave` 的 [`Iec104Error`](eneros_iec104_slave::Iec104Error)，
//! 主站层额外定义 [`MasterError`] 覆盖连接/状态/传输层错误。

pub use eneros_iec104_slave::Iec104Error;

/// IEC 104 主站错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterError {
    /// 未连接（目标设备不存在于连接表）
    NotConnected,
    /// 连接失败（传输层 connect 返回错误）
    ConnectFailed,
    /// 发送失败
    SendFailed,
    /// 接收失败
    RecvFailed,
    /// 状态错误（操作在当前状态下不合法）
    StateError,
    /// 超时
    Timeout,
    /// IEC 104 协议层错误
    Iec104(Iec104Error),
}

impl From<Iec104Error> for MasterError {
    fn from(e: Iec104Error) -> Self {
        Self::Iec104(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_iec104_error() {
        let err = Iec104Error::Decode;
        let master_err: MasterError = err.into();
        assert_eq!(master_err, MasterError::Iec104(Iec104Error::Decode));
    }

    #[test]
    fn test_eq() {
        assert_eq!(MasterError::NotConnected, MasterError::NotConnected);
        assert_ne!(MasterError::NotConnected, MasterError::Timeout);
    }

    #[test]
    fn test_clone() {
        let e = MasterError::StateError;
        let cloned = e.clone();
        assert_eq!(e, cloned);
    }
}
