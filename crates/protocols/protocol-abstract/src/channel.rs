//! 安全通道接缝（蓝图 39 WP-39.9）。
//!
//! # 不变量（fail-closed）
//!
//! - 明文通道**只能**实现为 [`InsecureStream`]（`negotiated_security() ==
//!   Plain`）；不得用"名字含 Tls 但无 TLS 语义"的类型冒充安全通道。
//! - release 默认 [`ChannelPolicy::default_release`]：`allow_plain = false`、
//!   最小安全等级 TLS1.2、控制写禁止 0-RTT——明文/弱策略在 release
//!   配置下直接 `PolicyDenied`。
//! - 证书过期/吊销/未知时间/算法不允许/身份不匹配 → 握手失败（由
//!   provider 实现；本层保证错误不转为成功）。

use alloc::boxed::Box;

use crate::transport::{TransportError, TransportHandle};

/// 通道安全等级（协商结果/策略要求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChannelSecurity {
    /// 明文（仅显式允许；release 默认禁止）。
    Plain,
    /// TLS 1.2。
    Tls12,
    /// TLS 1.3。
    Tls13,
    /// IEC 62351 安全（SM4-GCM + SM3-HMAC）。
    Iec62351,
}

/// 对端端点（固定缓冲，no_std）。
#[derive(Debug, Clone, Copy)]
pub struct Endpoint<'a> {
    /// 主机名/IP（字节）。
    pub host: &'a [u8],
    /// 端口。
    pub port: u16,
}

/// 对端身份引用（握手后校验）。
#[derive(Debug, Clone, Copy)]
pub struct IdentityRef<'a> {
    /// 主体名（subject/CN）。
    pub subject: &'a [u8],
    /// 角色（usage/EKU 语义）。
    pub role: &'a [u8],
}

/// 通道策略（release 默认禁止明文）。
#[derive(Debug, Clone, Copy)]
pub struct ChannelPolicy {
    /// 最低允许的安全等级。
    pub min_security: ChannelSecurity,
    /// 是否允许明文（release 默认 false）。
    pub allow_plain: bool,
    /// 允许会话恢复。
    pub allow_resumption: bool,
    /// 允许 0-RTT（控制写默认禁止）。
    pub allow_zero_rtt: bool,
    /// 要求固定对端身份（pinning）。
    pub require_pinned_identity: bool,
}

impl ChannelPolicy {
    /// release 默认：TLS1.2 起步、禁止明文、禁止 0-RTT、要求固定身份。
    pub const fn default_release() -> Self {
        Self {
            min_security: ChannelSecurity::Tls12,
            allow_plain: false,
            allow_resumption: true,
            allow_zero_rtt: false,
            require_pinned_identity: true,
        }
    }

    /// 开发/L0 测试默认：允许明文（仅测试与 lab 使用；生产必须
    /// [`Self::default_release`]）。
    pub const fn default_dev() -> Self {
        Self {
            min_security: ChannelSecurity::Plain,
            allow_plain: true,
            allow_resumption: false,
            allow_zero_rtt: false,
            require_pinned_identity: false,
        }
    }

    /// 该策略是否允许给定安全等级。
    pub fn allows(&self, security: ChannelSecurity) -> bool {
        match security {
            ChannelSecurity::Plain => self.allow_plain,
            other => other >= self.min_security,
        }
    }
}

/// 通道错误（fail-closed：任何失败都不得转成功）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// 该安全等级/提供方不可用。
    Unsupported,
    /// 策略拒绝（明文被禁/安全等级不足）。
    PolicyDenied,
    /// 握手失败。
    HandshakeFailed,
    /// 对端身份不匹配/未固定。
    PeerIdentityMismatch,
    /// 证书已吊销。
    Revoked,
    /// 证书过期/时间未知。
    Expired,
    /// RNG 不可用（握手必须真随机）。
    NoRng,
    /// 底层 I/O 失败。
    Io,
    /// 超时。
    Timeout,
    /// 通道已关闭。
    Closed,
}

/// 安全通道（已握手/已建立的加密流）。
pub trait SecureChannel {
    /// 发送明文数据（内部加密）。
    fn send(&mut self, data: &[u8]) -> Result<(), ChannelError>;
    /// 接收解密数据（`buf` 不足时截断并返回实际长度——调用方按长度
    /// 判断完整性；协议帧边界由上层处理）。
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ChannelError>;
    /// 关闭通道（flush + close_notify）。
    fn close(&mut self) -> Result<(), ChannelError>;
    /// 实际协商的安全等级。
    fn negotiated_security(&self) -> ChannelSecurity;
    /// 对端身份（未验证/明文为 None）。
    fn peer_identity(&self) -> Option<IdentityRef<'_>>;
}

/// 安全通道工厂（协议 transport 获取安全流的唯一入口）。
pub trait SecureChannelFactory {
    /// 连接并建立安全通道。
    ///
    /// `identity` 为本端身份（None = 匿名，取决于策略）；`deadline_ms`
    /// 为总超时（含握手）。策略不允许的等级 → [`ChannelError::PolicyDenied`]。
    fn connect(
        &mut self,
        endpoint: &Endpoint<'_>,
        identity: Option<&IdentityRef<'_>>,
        policy: &ChannelPolicy,
        deadline_ms: u64,
    ) -> Result<Box<dyn SecureChannel>, ChannelError>;
}

/// 装配模式（profile 分级；47 WP-47.5 policy/实现一致机器校验输入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyMode {
    /// qemu-test：允许显式明文（`InsecureStream`，永不冒充）。
    QemuTest,
    /// release-tls：声明 SecureProfile 的协议必须已有注册工厂，否则
    /// 装配拒绝（fail-closed）。
    ReleaseTls,
}

/// 47（WP-47.5）：安全通道工厂注册表（装配期按协议 id 注册第一实现；
/// `assembly_allows` 的机器校验输入）。
#[derive(Default)]
pub struct SecureChannelRegistry {
    factories: alloc::collections::BTreeMap<&'static str, Box<dyn SecureChannelFactory>>,
}

impl SecureChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    /// 注册协议 id → 工厂（同一 id 重复注册 → 后者覆盖，装配期单次）。
    pub fn register(&mut self, id: &'static str, factory: Box<dyn SecureChannelFactory>) {
        self.factories.insert(id, factory);
    }
    /// 是否已注册（机器校验）。
    pub fn has(&self, id: &str) -> bool {
        self.factories.contains_key(id)
    }
    /// 经已注册工厂建立安全通道（未注册 → `Unsupported`，fail-closed）。
    pub fn connect(
        &mut self,
        id: &str,
        endpoint: &Endpoint<'_>,
        identity: Option<&IdentityRef<'_>>,
        policy: &ChannelPolicy,
        deadline_ms: u64,
    ) -> Result<Box<dyn SecureChannel>, ChannelError> {
        let factory = self
            .factories
            .get_mut(id)
            .ok_or(ChannelError::Unsupported)?;
        factory.connect(endpoint, identity, policy, deadline_ms)
    }
}

/// 47（WP-47.5）：policy 与实现一致机器校验——**不一致 = 装配失败**。
///
/// - `ReleaseTls`：声明 SecureProfile（Tls12/Tls13/Iec62351）的协议必须
///   已有注册工厂且策略允许安全等级；无安全声明协议（GOOSE/SV——62351
///   现状）按宣称边界允许（明文 + 网络隔离补偿，见
///   `docs/security/channel-claims.md`）；
/// - `QemuTest`：显式明文允许（`InsecureStream`），SecureProfile 协议
///   有工厂时优先安全通道。
pub fn assembly_allows(
    mode: AssemblyMode,
    profile: crate::plugin::SecurityProfile,
    has_factory: bool,
    policy: &ChannelPolicy,
) -> bool {
    match mode {
        AssemblyMode::QemuTest => policy.allows(ChannelSecurity::Plain) || has_factory,
        AssemblyMode::ReleaseTls => match profile {
            crate::plugin::SecurityProfile::None => true,
            _ => has_factory && policy.allows(ChannelSecurity::Tls12),
        },
    }
}

/// 明文直通流（**不冒充安全通道**）。
///
/// 只实现 [`SecureChannel`] 以便统一组合，但：
/// - `negotiated_security()` 恒为 [`ChannelSecurity::Plain`]；
/// - 调用方必须显式经 `ChannelPolicy::allows(Plain)` 放行，release 默认
///   拒绝（蓝图 39 WP-39.9：`PlainTcp` 只可实现 `InsecureStream`）。
pub struct InsecureStream {
    inner: Box<dyn TransportHandle>,
}

impl InsecureStream {
    /// 包一层已建立的传输句柄（明文）。
    pub fn new(inner: Box<dyn TransportHandle>) -> Self {
        Self { inner }
    }
}

impl SecureChannel for InsecureStream {
    fn send(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        self.inner.send(data).map_err(map_transport)
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ChannelError> {
        self.inner.recv(buf).map_err(map_transport)
    }
    fn close(&mut self) -> Result<(), ChannelError> {
        self.inner.close().map_err(map_transport)
    }
    fn negotiated_security(&self) -> ChannelSecurity {
        ChannelSecurity::Plain
    }
    fn peer_identity(&self) -> Option<IdentityRef<'_>> {
        None
    }
}

fn map_transport(e: TransportError) -> ChannelError {
    match e {
        TransportError::Unsupported => ChannelError::Unsupported,
        TransportError::ConnectFailed => ChannelError::Io,
        TransportError::Io => ChannelError::Io,
        TransportError::Timeout => ChannelError::Timeout,
        TransportError::Closed => ChannelError::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EchoTransport;

    #[test]
    fn release_policy_denies_plain() {
        let policy = ChannelPolicy::default_release();
        assert!(!policy.allows(ChannelSecurity::Plain));
        assert!(policy.allows(ChannelSecurity::Tls12));
        assert!(policy.allows(ChannelSecurity::Tls13));
        assert!(!policy.allow_zero_rtt);
    }

    #[test]
    fn dev_policy_allows_plain() {
        let policy = ChannelPolicy::default_dev();
        assert!(policy.allows(ChannelSecurity::Plain));
    }

    #[test]
    fn insecure_stream_reports_plain_and_passes_io() {
        let mut stream = InsecureStream::new(Box::new(EchoTransport::default()));
        assert_eq!(stream.negotiated_security(), ChannelSecurity::Plain);
        assert!(stream.peer_identity().is_none());
        stream.send(b"abc").expect("send");
        let mut buf = [0u8; 16];
        let n = stream.recv(&mut buf).expect("recv");
        assert_eq!(&buf[..n], b"abc");
    }

    // ============ 47（WP-47.5）工厂注册表 + policy/实现一致机器校验 ============

    /// 测试工厂（记录 connect 调用）。
    struct FakeFactory {
        pub calls: usize,
    }
    impl SecureChannelFactory for FakeFactory {
        fn connect(
            &mut self,
            _endpoint: &Endpoint<'_>,
            _identity: Option<&IdentityRef<'_>>,
            _policy: &ChannelPolicy,
            _deadline_ms: u64,
        ) -> Result<Box<dyn SecureChannel>, ChannelError> {
            self.calls += 1;
            Ok(Box::new(InsecureStream::new(Box::new(
                EchoTransport::default(),
            ))))
        }
    }

    #[test]
    fn registry_connect_routes_to_registered_factory() {
        let mut reg = SecureChannelRegistry::new();
        assert!(!reg.has("mqtt"), "未注册 → has=false（机器校验输入）");
        let f = Box::new(FakeFactory { calls: 0 });
        reg.register("mqtt", f);
        assert!(reg.has("mqtt"));
        let ep = Endpoint {
            host: b"10.0.2.2",
            port: 8883,
        };
        let ch = reg
            .connect("mqtt", &ep, None, &ChannelPolicy::default_dev(), 5000)
            .expect("connect");
        assert_eq!(ch.negotiated_security(), ChannelSecurity::Plain);
        // 未注册协议 → Unsupported（fail-closed）。
        assert!(matches!(
            reg.connect("iec104", &ep, None, &ChannelPolicy::default_dev(), 5000),
            Err(ChannelError::Unsupported)
        ));
    }

    #[test]
    fn assembly_allows_machine_check_matrix() {
        use crate::plugin::SecurityProfile;
        let release = ChannelPolicy::default_release();
        let dev = ChannelPolicy::default_dev();
        // ReleaseTls：SecureProfile 无工厂 → 拒绝装配（fail-closed）。
        assert!(!assembly_allows(
            AssemblyMode::ReleaseTls,
            SecurityProfile::Tls12,
            false,
            &release
        ));
        // ReleaseTls：SecureProfile + 工厂 → 允许。
        assert!(assembly_allows(
            AssemblyMode::ReleaseTls,
            SecurityProfile::Tls12,
            true,
            &release
        ));
        // ReleaseTls：无安全声明协议（GOOSE/SV——62351 现状）→ 宣称边界允许。
        assert!(assembly_allows(
            AssemblyMode::ReleaseTls,
            SecurityProfile::None,
            false,
            &release
        ));
        // QemuTest：显式明文允许。
        assert!(assembly_allows(
            AssemblyMode::QemuTest,
            SecurityProfile::Tls12,
            false,
            &dev
        ));
        // QemuTest + release 策略（禁明文）且无工厂 → 拒绝。
        assert!(!assembly_allows(
            AssemblyMode::QemuTest,
            SecurityProfile::Tls12,
            false,
            &release
        ));
    }
}
