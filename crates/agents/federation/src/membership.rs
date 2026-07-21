//! 成员管理：节点角色、证书引用、成员信息与注册表。
//!
//! 全部 no_std + alloc 兼容；集合用 `BTreeMap`（遍历按 node_id 升序），
//! 时间由调用方以 `now_ms` 注入。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::net::IpAddr;

/// FNV-1a 64 offset basis
const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
/// FNV-1a 64 prime
const FNV_PRIME: u64 = 1099511628211;

/// 联邦节点角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeRole {
    /// 边缘盒子（默认角色）
    #[default]
    EdgeBox,
    /// 边缘协调器
    EdgeCoordinator,
    /// 云端协调器
    CloudCoordinator,
}

/// 证书引用：仅作确定性标识，无密码学语义
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CertRef {
    /// 证书内容的 FNV-1a 64 指纹
    pub fingerprint: u64,
}

impl CertRef {
    /// 对证书字节做 FNV-1a 64 确定性折叠，生成指纹
    pub fn from_bytes(cert: &[u8]) -> CertRef {
        let mut f: u64 = FNV_OFFSET_BASIS;
        for &b in cert {
            f ^= b as u64;
            f = f.wrapping_mul(FNV_PRIME);
        }
        CertRef { fingerprint: f }
    }
}

/// 联邦成员信息
#[derive(Debug, Clone, PartialEq)]
pub struct MemberInfo {
    /// 节点标识
    pub node_id: u64,
    /// 节点网络地址
    pub addr: IpAddr,
    /// 节点角色
    pub role: NodeRole,
    /// 能力标签集合
    pub capabilities: Vec<u64>,
    /// 最后一次心跳/见到的时间（ms）
    pub last_seen: u64,
    /// 证书引用
    pub cert: CertRef,
}

/// 加入联邦请求
#[derive(Debug, Clone, PartialEq)]
pub struct JoinRequest {
    /// 节点标识
    pub node_id: u64,
    /// 节点网络地址
    pub addr: IpAddr,
    /// 节点角色
    pub role: NodeRole,
    /// 证书原始字节
    pub cert: Vec<u8>,
    /// 能力标签集合
    pub capabilities: Vec<u64>,
}

/// 成员注册表
#[derive(Debug, Clone)]
pub struct MemberRegistry {
    /// 成员表（key 为 node_id，BTreeMap 遍历天然升序）
    pub members: BTreeMap<u64, MemberInfo>,
    /// 本节点 id
    pub self_id: u64,
}

impl MemberRegistry {
    /// 创建空注册表
    pub fn new(self_id: u64) -> Self {
        Self {
            members: BTreeMap::new(),
            self_id,
        }
    }

    /// 添加成员；同 id 覆盖
    pub fn add(&mut self, m: MemberInfo) {
        self.members.insert(m.node_id, m);
    }

    /// 移除成员；存在并移除返回 true
    pub fn remove(&mut self, node_id: u64) -> bool {
        self.members.remove(&node_id).is_some()
    }

    /// 心跳：成员存在则刷新 last_seen 为 now_ms 并返回 true，否则返回 false
    pub fn heartbeat(&mut self, node_id: u64, now_ms: u64) -> bool {
        match self.members.get_mut(&node_id) {
            Some(m) => {
                m.last_seen = now_ms;
                true
            }
            None => false,
        }
    }

    /// 剔除超时成员：`now_ms - last_seen > timeout_ms`（严格大于，边界存活）。
    /// 返回被剔除的 node_id 升序列表。
    pub fn remove_stale(&mut self, timeout_ms: u64, now_ms: u64) -> Vec<u64> {
        let stale: Vec<u64> = self
            .members
            .iter()
            .filter(|(_, m)| now_ms.saturating_sub(m.last_seen) > timeout_ms)
            .map(|(&id, _)| id)
            .collect();
        for &id in &stale {
            self.members.remove(&id);
        }
        stale
    }

    /// 按 node_id 升序返回全部成员克隆
    pub fn list(&self) -> Vec<MemberInfo> {
        self.members.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use core::net::Ipv4Addr;
    use std::vec;

    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    fn member(node_id: u64, last_seen: u64) -> MemberInfo {
        MemberInfo {
            node_id,
            addr: ip(node_id as u8),
            role: NodeRole::EdgeBox,
            capabilities: vec![10, 20],
            last_seen,
            cert: CertRef::default(),
        }
    }

    // T1: NodeRole Default == EdgeBox；三变体互不等；Copy/Eq 语义
    #[test]
    fn t1_node_role_default_and_eq() {
        let a = NodeRole::default();
        assert_eq!(a, NodeRole::EdgeBox);
        assert_ne!(NodeRole::EdgeBox, NodeRole::EdgeCoordinator);
        assert_ne!(NodeRole::EdgeBox, NodeRole::CloudCoordinator);
        assert_ne!(NodeRole::EdgeCoordinator, NodeRole::CloudCoordinator);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    // T2: CertRef Default fingerprint==0；Copy/Eq 语义
    #[test]
    fn t2_cert_ref_default_and_copy() {
        let c = CertRef::default();
        assert_eq!(c.fingerprint, 0);
        let d = c; // Copy
        assert_eq!(c, d);
        assert_ne!(c, CertRef { fingerprint: 1 });
    }

    // T3: MemberInfo 构造字段回显；Clone 独立性；PartialEq
    #[test]
    fn t3_member_info_fields_and_clone() {
        let m = MemberInfo {
            node_id: 7,
            addr: ip(7),
            role: NodeRole::EdgeCoordinator,
            capabilities: vec![1, 2],
            last_seen: 1234,
            cert: CertRef { fingerprint: 99 },
        };
        assert_eq!(m.node_id, 7);
        assert_eq!(m.addr, ip(7));
        assert_eq!(m.role, NodeRole::EdgeCoordinator);
        assert_eq!(m.capabilities, vec![1, 2]);
        assert_eq!(m.last_seen, 1234);
        assert_eq!(m.cert.fingerprint, 99);

        let mut c = m.clone();
        c.capabilities.push(3);
        assert_eq!(m.capabilities, vec![1, 2]); // 原值不受影响
        assert_eq!(c.capabilities, vec![1, 2, 3]);
        assert_eq!(m, m.clone()); // PartialEq
        assert_ne!(m, c);
    }

    // T4: JoinRequest 构造字段回显；Clone 独立性
    #[test]
    fn t4_join_request_fields_and_clone() {
        let r = JoinRequest {
            node_id: 3,
            addr: ip(3),
            role: NodeRole::CloudCoordinator,
            cert: vec![0xAA, 0xBB],
            capabilities: vec![5],
        };
        assert_eq!(r.node_id, 3);
        assert_eq!(r.addr, ip(3));
        assert_eq!(r.role, NodeRole::CloudCoordinator);
        assert_eq!(r.cert, vec![0xAA, 0xBB]);
        assert_eq!(r.capabilities, vec![5]);

        let mut c = r.clone();
        c.cert.push(0xCC);
        assert_eq!(r.cert.len(), 2);
        assert_eq!(c.cert.len(), 3);
        assert_eq!(r, r.clone());
    }

    // T5: CertRef::from_bytes 确定性；不同字节序列大概率不同指纹
    #[test]
    fn t5_cert_ref_from_bytes_deterministic() {
        let a = CertRef::from_bytes(&[1, 2, 3]);
        let b = CertRef::from_bytes(&[1, 2, 3]);
        assert_eq!(a, b);
        let c = CertRef::from_bytes(&[3, 2, 1]);
        assert_ne!(a.fingerprint, c.fingerprint);
    }

    // T6: from_bytes 空切片 == offset basis；单字节折叠符合 FNV-1a 手工计算
    #[test]
    fn t6_cert_ref_from_bytes_fnv_values() {
        assert_eq!(CertRef::from_bytes(&[]).fingerprint, FNV_OFFSET_BASIS);
        let expected = (FNV_OFFSET_BASIS ^ 0xABu64).wrapping_mul(FNV_PRIME);
        assert_eq!(CertRef::from_bytes(&[0xAB]).fingerprint, expected);
    }

    // T7: registry new 空；add 后 list 含成员；add 同 id 覆盖
    #[test]
    fn t7_registry_new_add_overwrite() {
        let mut reg = MemberRegistry::new(100);
        assert_eq!(reg.self_id, 100);
        assert!(reg.members.is_empty());
        assert!(reg.list().is_empty());

        reg.add(member(1, 1000));
        assert_eq!(reg.list().len(), 1);
        assert_eq!(reg.list()[0].node_id, 1);

        let mut newer = member(1, 2000);
        newer.role = NodeRole::EdgeCoordinator;
        reg.add(newer);
        assert_eq!(reg.list().len(), 1); // 覆盖而非新增
        assert_eq!(reg.members.get(&1).unwrap().last_seen, 2000);
        assert_eq!(reg.members.get(&1).unwrap().role, NodeRole::EdgeCoordinator);
    }

    // T8: add id 2、1 后 list() 按 node_id 升序
    #[test]
    fn t8_registry_list_sorted() {
        let mut reg = MemberRegistry::new(100);
        reg.add(member(2, 1000));
        reg.add(member(1, 1000));
        let ids: Vec<u64> = reg.list().iter().map(|m| m.node_id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    // T9: heartbeat(存在的 id, 5000) → true 且 last_seen==5000
    #[test]
    fn t9_heartbeat_existing() {
        let mut reg = MemberRegistry::new(100);
        reg.add(member(1, 1000));
        assert!(reg.heartbeat(1, 5000));
        assert_eq!(reg.members.get(&1).unwrap().last_seen, 5000);
    }

    // T10: heartbeat(99, _) → false
    #[test]
    fn t10_heartbeat_unknown() {
        let mut reg = MemberRegistry::new(100);
        assert!(!reg.heartbeat(99, 5000));
    }

    // T11: remove_stale 边界：last_seen=1000，remove_stale(9000, 10_000) → 保留
    #[test]
    fn t11_remove_stale_boundary_kept() {
        let mut reg = MemberRegistry::new(100);
        reg.add(member(1, 1000));
        let removed = reg.remove_stale(9000, 10_000); // 9000 > 9000 不成立
        assert!(removed.is_empty());
        assert!(reg.members.contains_key(&1));
    }

    // T12: remove_stale 剔除：remove_stale(9000, 10_001) → 剔除；多成员混合剔除返回升序
    #[test]
    fn t12_remove_stale_eviction_sorted() {
        let mut reg = MemberRegistry::new(100);
        reg.add(member(1, 1000));
        let removed = reg.remove_stale(9000, 10_001); // 9001 > 9000
        assert_eq!(removed, vec![1]);
        assert!(!reg.members.contains_key(&1));

        // 多成员混合：id 5、2 超时，id 7 存活
        reg.add(member(5, 1000));
        reg.add(member(7, 9000));
        reg.add(member(2, 1000));
        let removed = reg.remove_stale(9000, 10_500);
        assert_eq!(removed, vec![2, 5]); // 升序
        assert!(reg.members.contains_key(&7));
        assert_eq!(reg.members.len(), 1);
    }
}
