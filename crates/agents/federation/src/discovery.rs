//! 发现协议：证书验证、在线广播、加入处理、心跳保活、超时剔除。
//!
//! 同步 trait + `Box<dyn>` 依赖注入；计数器追踪 join/reject/broadcast/stale。

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::membership::*;

/// 联邦发现错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FedError {
    /// 证书验证失败
    InvalidCert,
    /// 节点 id 重复（含与自身 id 冲突）
    DuplicateNode,
    /// 未知节点（心跳目标不在注册表中）
    UnknownNode,
    /// 广播失败
    BroadcastFailed,
}

/// 证书验证器（同步，无 Send+Sync 约束）
pub trait CertVerifier {
    /// 验证证书字节；通过返回 `Ok(())`，否则返回 `Err(FedError)`
    fn verify(&mut self, cert: &[u8]) -> Result<(), FedError>;
}

/// 在线广播总线（同步，无 Send+Sync 约束）
pub trait PresenceBus {
    /// 向联邦广播成员在线信息
    fn broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError>;
}

/// Mock 证书验证器：按 `accept` 开关放行/拒绝
#[derive(Debug, Clone)]
pub struct MockCertVerifier {
    /// true 放行，false 拒绝
    pub accept: bool,
    /// verify 调用次数
    pub verify_count: u64,
}

impl MockCertVerifier {
    /// 创建 Mock 验证器，verify_count 初始为 0
    pub fn new(accept: bool) -> Self {
        Self {
            accept,
            verify_count: 0,
        }
    }
}

impl CertVerifier for MockCertVerifier {
    fn verify(&mut self, _cert: &[u8]) -> Result<(), FedError> {
        self.verify_count += 1;
        if self.accept {
            Ok(())
        } else {
            Err(FedError::InvalidCert)
        }
    }
}

/// Mock 广播总线：前 `fail_times` 次广播失败，之后成功并记录
#[derive(Debug, Clone)]
pub struct MockPresenceBus {
    /// 已成功广播的成员记录
    pub broadcasts: Vec<MemberInfo>,
    /// 剩余应失败次数
    pub fail_times: u32,
}

impl MockPresenceBus {
    /// 创建 Mock 总线，`fail_times` 为前几次广播应失败次数，broadcasts 初始为空
    pub fn new(fail_times: u32) -> Self {
        Self {
            broadcasts: Vec::new(),
            fail_times,
        }
    }
}

impl PresenceBus for MockPresenceBus {
    fn broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(FedError::BroadcastFailed);
        }
        self.broadcasts.push(member.clone());
        Ok(())
    }
}

/// 联邦发现协调器
pub struct FederationDiscovery {
    /// 成员注册表
    pub registry: MemberRegistry,
    /// 证书验证器
    pub verifier: Box<dyn CertVerifier>,
    /// 广播总线
    pub bus: Box<dyn PresenceBus>,
    /// 心跳间隔（ms）；超时阈值为 `heartbeat_interval_ms * 3`
    pub heartbeat_interval_ms: u64,
    /// 成功加入计数
    pub join_count: u64,
    /// 拒绝计数（证书拒绝 + 重复拒绝 + 广播失败）
    pub reject_count: u64,
    /// 成功广播计数
    pub broadcast_count: u64,
    /// 累计剔除计数
    pub stale_count: u64,
}

impl FederationDiscovery {
    /// 创建发现协调器：registry 为空，4 个计数器全零
    pub fn new(
        self_id: u64,
        verifier: Box<dyn CertVerifier>,
        bus: Box<dyn PresenceBus>,
        heartbeat_interval_ms: u64,
    ) -> Self {
        Self {
            registry: MemberRegistry::new(self_id),
            verifier,
            bus,
            heartbeat_interval_ms,
            join_count: 0,
            reject_count: 0,
            broadcast_count: 0,
            stale_count: 0,
        }
    }

    /// 处理加入请求：证书验证 → 重复检查 → 注册 → 广播
    pub fn handle_join(&mut self, req: JoinRequest, now_ms: u64) -> Result<MemberInfo, FedError> {
        if self.verifier.verify(&req.cert).is_err() {
            self.reject_count += 1;
            return Err(FedError::InvalidCert);
        }
        if req.node_id == self.registry.self_id || self.registry.members.contains_key(&req.node_id)
        {
            self.reject_count += 1;
            return Err(FedError::DuplicateNode);
        }
        let member = MemberInfo {
            node_id: req.node_id,
            addr: req.addr,
            role: req.role,
            capabilities: req.capabilities,
            last_seen: now_ms,
            cert: CertRef::from_bytes(&req.cert),
        };
        self.registry.add(member.clone());
        match self.bus.broadcast(&member) {
            Ok(()) => {
                self.broadcast_count += 1;
                self.join_count += 1;
                Ok(member)
            }
            Err(e) => {
                // 成员已注册保留，可由 broadcast_presence 重试
                self.reject_count += 1;
                Err(e)
            }
        }
    }

    /// 广播指定成员在线信息；成功计入 broadcast_count
    pub fn broadcast_presence(&mut self, member: &MemberInfo) -> Result<(), FedError> {
        self.bus.broadcast(member)?;
        self.broadcast_count += 1;
        Ok(())
    }

    /// 心跳：成员存在刷新 last_seen，未知节点返回 Err(UnknownNode)
    pub fn heartbeat(&mut self, node_id: u64, now_ms: u64) -> Result<(), FedError> {
        if self.registry.heartbeat(node_id, now_ms) {
            Ok(())
        } else {
            Err(FedError::UnknownNode)
        }
    }

    /// 剔除超时成员（阈值 = heartbeat_interval_ms * 3），返回被剔 id 升序
    pub fn sweep_stale(&mut self, now_ms: u64) -> Vec<u64> {
        let removed = self
            .registry
            .remove_stale(self.heartbeat_interval_ms * 3, now_ms);
        self.stale_count += removed.len() as u64;
        removed
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use core::cell::RefCell;
    use core::net::{IpAddr, Ipv4Addr};
    use std::vec;

    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    fn req(id: u64) -> JoinRequest {
        JoinRequest {
            node_id: id,
            addr: ip(id as u8),
            role: NodeRole::EdgeBox,
            cert: vec![id as u8, 1, 2],
            capabilities: vec![10, 20],
        }
    }

    fn fd_with(bus: MockPresenceBus) -> FederationDiscovery {
        FederationDiscovery::new(
            100,
            Box::new(MockCertVerifier::new(true)),
            Box::new(bus),
            3000,
        )
    }

    /// 测试专用记录总线：借 Rc<RefCell> 让测试侧可回读广播内容
    struct RecordingBus(Rc<RefCell<Vec<MemberInfo>>>);

    impl PresenceBus for RecordingBus {
        fn broadcast(&mut self, member: &MemberInfo) -> Result<(), FedError> {
            self.0.borrow_mut().push(member.clone());
            Ok(())
        }
    }

    // T13: FedError 四变体互不等；Copy/Eq 语义
    #[test]
    fn t13_fed_error_variants() {
        let errs = [
            FedError::InvalidCert,
            FedError::DuplicateNode,
            FedError::UnknownNode,
            FedError::BroadcastFailed,
        ];
        for (i, a) in errs.iter().enumerate() {
            for (j, b) in errs.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
        let e = FedError::InvalidCert;
        let e2 = e; // Copy
        assert_eq!(e, e2);
    }

    // T14: MockCertVerifier accept=true → verify Ok，verify_count==1
    #[test]
    fn t14_mock_verifier_accept() {
        let mut v = MockCertVerifier::new(true);
        assert_eq!(v.verify_count, 0);
        assert_eq!(v.verify(b"cert"), Ok(()));
        assert_eq!(v.verify_count, 1);
    }

    // T15: MockCertVerifier accept=false → Err(InvalidCert)，verify_count 累加
    #[test]
    fn t15_mock_verifier_reject() {
        let mut v = MockCertVerifier::new(false);
        assert_eq!(v.verify(b"a"), Err(FedError::InvalidCert));
        assert_eq!(v.verify(b"b"), Err(FedError::InvalidCert));
        assert_eq!(v.verify_count, 2);
    }

    // T16: MockPresenceBus fail_times=0 → broadcast Ok，broadcasts 含 member
    #[test]
    fn t16_mock_bus_success() {
        let mut bus = MockPresenceBus::new(0);
        let m = fd_with(MockPresenceBus::new(0))
            .handle_join(req(1), 1000)
            .unwrap();
        assert_eq!(bus.broadcast(&m), Ok(()));
        assert_eq!(bus.broadcasts.len(), 1);
        assert_eq!(bus.broadcasts[0], m);
    }

    // T17: MockPresenceBus fail_times=2 → 前 2 次 Err，第 3 次 Ok 入 broadcasts
    #[test]
    fn t17_mock_bus_fail_then_recover() {
        let mut bus = MockPresenceBus::new(2);
        let m = fd_with(MockPresenceBus::new(0))
            .handle_join(req(1), 1000)
            .unwrap();
        assert_eq!(bus.broadcast(&m), Err(FedError::BroadcastFailed));
        assert_eq!(bus.broadcast(&m), Err(FedError::BroadcastFailed));
        assert_eq!(bus.broadcast(&m), Ok(()));
        assert_eq!(bus.broadcasts.len(), 1);
        assert_eq!(bus.broadcasts[0], m);
    }

    // T18: Mock 可作 Box<dyn CertVerifier>/Box<dyn PresenceBus> 注入（多态）
    #[test]
    fn t18_mock_trait_objects() {
        let mut v: Box<dyn CertVerifier> = Box::new(MockCertVerifier::new(true));
        let mut b: Box<dyn PresenceBus> = Box::new(MockPresenceBus::new(0));
        assert_eq!(v.verify(b"x"), Ok(()));
        let m = MemberInfo {
            node_id: 1,
            addr: ip(1),
            role: NodeRole::EdgeBox,
            capabilities: vec![],
            last_seen: 0,
            cert: CertRef::default(),
        };
        assert_eq!(b.broadcast(&m), Ok(()));
    }

    // T19: FederationDiscovery::new 初始状态
    #[test]
    fn t19_discovery_new_initial_state() {
        let fd = fd_with(MockPresenceBus::new(0));
        assert!(fd.registry.members.is_empty());
        assert_eq!(fd.registry.self_id, 100);
        assert_eq!(fd.heartbeat_interval_ms, 3000);
        assert_eq!(fd.join_count, 0);
        assert_eq!(fd.reject_count, 0);
        assert_eq!(fd.broadcast_count, 0);
        assert_eq!(fd.stale_count, 0);
    }

    // T20: handle_join 成功 → Ok(member)：字段与请求一致
    #[test]
    fn t20_handle_join_success_fields() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        let r = JoinRequest {
            node_id: 7,
            addr: ip(7),
            role: NodeRole::EdgeCoordinator,
            cert: vec![9, 9],
            capabilities: vec![1, 2, 3],
        };
        let m = fd.handle_join(r, 5000).unwrap();
        assert_eq!(m.node_id, 7);
        assert_eq!(m.addr, ip(7));
        assert_eq!(m.role, NodeRole::EdgeCoordinator);
        assert_eq!(m.capabilities, vec![1, 2, 3]);
        assert_eq!(m.last_seen, 5000);
    }

    // T21: handle_join 成功 → cert 指纹正确；registry 含成员；计数器
    #[test]
    fn t21_handle_join_cert_and_counters() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        let r = req(7);
        let expected_fp = CertRef::from_bytes(&r.cert).fingerprint;
        let m = fd.handle_join(r, 1000).unwrap();
        assert_eq!(m.cert.fingerprint, expected_fp);
        assert!(fd.registry.members.contains_key(&7));
        assert_eq!(
            fd.registry.members.get(&7).unwrap().cert.fingerprint,
            expected_fp
        );
        assert_eq!(fd.join_count, 1);
        assert_eq!(fd.broadcast_count, 1);
    }

    // T22: handle_join 成功 → bus 收到内容一致的 member
    #[test]
    fn t22_handle_join_bus_received_member() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut fd = FederationDiscovery::new(
            100,
            Box::new(MockCertVerifier::new(true)),
            Box::new(RecordingBus(log.clone())),
            3000,
        );
        let m = fd.handle_join(req(7), 1000).unwrap();
        let recorded = log.borrow();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0], m);
    }

    // T23: verifier reject → Err(InvalidCert)、reject_count==1、registry 空、bus 无广播
    #[test]
    fn t23_handle_join_cert_rejected() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut fd = FederationDiscovery::new(
            100,
            Box::new(MockCertVerifier::new(false)),
            Box::new(RecordingBus(log.clone())),
            3000,
        );
        assert_eq!(fd.handle_join(req(7), 1000), Err(FedError::InvalidCert));
        assert_eq!(fd.reject_count, 1);
        assert!(fd.registry.members.is_empty());
        assert!(log.borrow().is_empty());
        assert_eq!(fd.join_count, 0);
        assert_eq!(fd.broadcast_count, 0);
    }

    // T24: 拒绝后同一请求改 accept=true 再次 handle_join → 成功（无残留状态）
    #[test]
    fn t24_reject_then_retry_success() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.verifier = Box::new(MockCertVerifier::new(false));
        assert_eq!(fd.handle_join(req(7), 1000), Err(FedError::InvalidCert));
        fd.verifier = Box::new(MockCertVerifier::new(true));
        let m = fd.handle_join(req(7), 2000).unwrap();
        assert_eq!(m.last_seen, 2000);
        assert_eq!(fd.join_count, 1);
        assert_eq!(fd.reject_count, 1);
        assert!(fd.registry.members.contains_key(&7));
    }

    // T25: 同 node_id 再次 handle_join → Err(DuplicateNode)；原成员不被覆盖
    #[test]
    fn t25_duplicate_node_rejected() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(7), 1000).unwrap();
        let mut r2 = req(7);
        r2.role = NodeRole::CloudCoordinator;
        assert_eq!(fd.handle_join(r2, 9999), Err(FedError::DuplicateNode));
        assert_eq!(fd.reject_count, 1);
        let orig = fd.registry.members.get(&7).unwrap();
        assert_eq!(orig.last_seen, 1000); // 原值保持
        assert_eq!(orig.role, NodeRole::EdgeBox);
    }

    // T26: node_id == self_id → Err(DuplicateNode)
    #[test]
    fn t26_self_id_duplicate_rejected() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        assert_eq!(fd.handle_join(req(100), 1000), Err(FedError::DuplicateNode));
        assert_eq!(fd.reject_count, 1);
        assert!(fd.registry.members.is_empty());
    }

    // T27: 多次拒绝后 reject_count 累计正确
    #[test]
    fn t27_reject_count_accumulates() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap();
        assert_eq!(fd.handle_join(req(1), 2000), Err(FedError::DuplicateNode));
        assert_eq!(fd.handle_join(req(100), 2000), Err(FedError::DuplicateNode));
        fd.verifier = Box::new(MockCertVerifier::new(false));
        assert_eq!(fd.handle_join(req(2), 2000), Err(FedError::InvalidCert));
        assert_eq!(fd.reject_count, 3);
        assert_eq!(fd.join_count, 1);
    }

    // T28: bus fail_times=1 → 首次 handle_join Err(BroadcastFailed)，成员已注册保留
    #[test]
    fn t28_broadcast_failure_keeps_member() {
        let mut fd = fd_with(MockPresenceBus::new(1));
        assert_eq!(fd.handle_join(req(1), 1000), Err(FedError::BroadcastFailed));
        assert_eq!(fd.reject_count, 1);
        assert!(fd.registry.members.contains_key(&1)); // 已注册保留
        assert_eq!(fd.join_count, 0); // join_count 不加
        assert_eq!(fd.broadcast_count, 0);
    }

    // T29: 广播失败后 broadcast_presence(&member) 重试成功 → broadcast_count+=1
    #[test]
    fn t29_broadcast_retry_success() {
        let mut fd = fd_with(MockPresenceBus::new(1));
        assert_eq!(fd.handle_join(req(1), 1000), Err(FedError::BroadcastFailed));
        let m = fd.registry.members.get(&1).unwrap().clone();
        assert_eq!(fd.broadcast_presence(&m), Ok(()));
        assert_eq!(fd.broadcast_count, 1);
    }

    // T30: heartbeat(存在成员, 新时间) → Ok，last_seen 刷新
    #[test]
    fn t30_heartbeat_existing_member() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap();
        assert_eq!(fd.heartbeat(1, 5000), Ok(()));
        assert_eq!(fd.registry.members.get(&1).unwrap().last_seen, 5000);
    }

    // T31: heartbeat(未知节点) → Err(UnknownNode)
    #[test]
    fn t31_heartbeat_unknown_node() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        assert_eq!(fd.heartbeat(99, 5000), Err(FedError::UnknownNode));
    }

    // T32: sweep_stale 边界保留：last_seen=1000，timeout=9000，sweep(10_000) → 保留
    #[test]
    fn t32_sweep_stale_boundary_kept() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap();
        let removed = fd.sweep_stale(10_000); // 10000-1000=9000，不 > 9000
        assert!(removed.is_empty());
        assert_eq!(fd.stale_count, 0);
        assert!(fd.registry.members.contains_key(&1));
    }

    // T33: sweep_stale 剔除：sweep(10_001) → 返回 [node_id]、stale_count==1
    #[test]
    fn t33_sweep_stale_eviction() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap();
        let removed = fd.sweep_stale(10_001); // 9001 > 9000
        assert_eq!(removed, vec![1]);
        assert_eq!(fd.stale_count, 1);
        assert!(fd.registry.members.is_empty());
    }

    // T34: sweep_stale 后 heartbeat(被剔 id) → Err(UnknownNode)
    #[test]
    fn t34_heartbeat_after_eviction() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap();
        fd.sweep_stale(10_001);
        assert_eq!(fd.heartbeat(1, 11_000), Err(FedError::UnknownNode));
    }

    // T35: sweep_stale 多成员混合（1 超时 1 存活）→ 仅剔超时者，返回升序；stale_count 累计
    #[test]
    fn t35_sweep_stale_mixed_members() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(5), 1000).unwrap();
        fd.handle_join(req(2), 1000).unwrap();
        fd.handle_join(req(7), 9000).unwrap();
        let removed = fd.sweep_stale(10_500); // 5、2 超时（9500>9000），7 存活（1500）
        assert_eq!(removed, vec![2, 5]); // 升序
        assert_eq!(fd.stale_count, 2);
        assert!(fd.registry.members.contains_key(&7));
        // 再次剔除，stale_count 累计
        let removed2 = fd.sweep_stale(20_000); // 7: 20000-9000=11000>9000
        assert_eq!(removed2, vec![7]);
        assert_eq!(fd.stale_count, 3);
    }

    // T36: 全链路：join(A,1000) → heartbeat(A,2000) → join(B,3000) → sweep(12_000)
    #[test]
    fn t36_full_flow_join_heartbeat_sweep() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap(); // A
        fd.heartbeat(1, 2000).unwrap();
        fd.handle_join(req(2), 3000).unwrap(); // B
        let removed = fd.sweep_stale(12_000);
        // A: 12000-2000=10000>9000 剔除；B: 12000-3000=9000 不成立保留
        assert_eq!(removed, vec![1]);
        assert_eq!(fd.join_count, 2);
        assert_eq!(fd.stale_count, 1);
        assert!(fd.registry.members.contains_key(&2));
    }

    // T37: 全链路：join → 断网（bus 故障）→ broadcast_presence 恢复 → heartbeat 正常
    #[test]
    fn t37_full_flow_network_recovery() {
        let mut fd = fd_with(MockPresenceBus::new(1));
        assert_eq!(fd.handle_join(req(1), 1000), Err(FedError::BroadcastFailed));
        let m = fd.registry.members.get(&1).unwrap().clone();
        assert_eq!(fd.broadcast_presence(&m), Ok(())); // 恢复
        assert_eq!(fd.heartbeat(1, 2000), Ok(()));
        assert_eq!(fd.registry.members.get(&1).unwrap().last_seen, 2000);
    }

    // T38: 全链路：3 节点顺序加入，list 升序；中间节点超时被剔，剩余仍升序
    #[test]
    fn t38_full_flow_sorted_list_after_eviction() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(30), 1000).unwrap();
        fd.handle_join(req(10), 1000).unwrap();
        fd.handle_join(req(20), 1000).unwrap();
        let ids: Vec<u64> = fd.registry.list().iter().map(|m| m.node_id).collect();
        assert_eq!(ids, vec![10, 20, 30]);

        fd.heartbeat(10, 5000).unwrap();
        fd.heartbeat(30, 5000).unwrap();
        let removed = fd.sweep_stale(10_500); // 20: 9500>9000 剔除
        assert_eq!(removed, vec![20]);
        let ids: Vec<u64> = fd.registry.list().iter().map(|m| m.node_id).collect();
        assert_eq!(ids, vec![10, 30]);
    }

    // T39: 多角色混布：三种角色节点加入，role 字段各自回显
    #[test]
    fn t39_multi_role_members() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        let mut r1 = req(1);
        r1.role = NodeRole::EdgeBox;
        let mut r2 = req(2);
        r2.role = NodeRole::EdgeCoordinator;
        let mut r3 = req(3);
        r3.role = NodeRole::CloudCoordinator;
        let m1 = fd.handle_join(r1, 1000).unwrap();
        let m2 = fd.handle_join(r2, 1000).unwrap();
        let m3 = fd.handle_join(r3, 1000).unwrap();
        assert_eq!(m1.role, NodeRole::EdgeBox);
        assert_eq!(m2.role, NodeRole::EdgeCoordinator);
        assert_eq!(m3.role, NodeRole::CloudCoordinator);
        assert_eq!(
            fd.registry.members.get(&2).unwrap().role,
            NodeRole::EdgeCoordinator
        );
    }

    // T40: 计数器综合断言：混合各类操作后 4 计数器精确等于预期值
    #[test]
    fn t40_counters_comprehensive() {
        let mut fd = fd_with(MockPresenceBus::new(0));
        fd.handle_join(req(1), 1000).unwrap(); // join=1, bcast=1
        fd.handle_join(req(2), 1000).unwrap(); // join=2, bcast=2

        fd.verifier = Box::new(MockCertVerifier::new(false));
        assert_eq!(fd.handle_join(req(3), 1500), Err(FedError::InvalidCert)); // reject=1

        fd.verifier = Box::new(MockCertVerifier::new(true));
        assert_eq!(fd.handle_join(req(1), 1600), Err(FedError::DuplicateNode)); // reject=2
        fd.handle_join(req(3), 2000).unwrap(); // join=3, bcast=3

        assert_eq!(fd.heartbeat(1, 5000), Ok(()));
        assert_eq!(fd.heartbeat(99, 5000), Err(FedError::UnknownNode)); // 无计数

        let m1 = fd.registry.members.get(&1).unwrap().clone();
        assert_eq!(fd.broadcast_presence(&m1), Ok(())); // bcast=4

        let removed = fd.sweep_stale(20_000); // 1/2/3 全部超时剔除
        assert_eq!(removed, vec![1, 2, 3]);

        assert_eq!(fd.join_count, 3);
        assert_eq!(fd.reject_count, 2);
        assert_eq!(fd.broadcast_count, 4);
        assert_eq!(fd.stale_count, 3);
    }
}
