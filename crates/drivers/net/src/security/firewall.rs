//! Firewall rule engine with connection tracking (v0.30.0).
//!
//! Provides a simple first-match-wins rule list backed by an IPv4 CIDR source
//! filter, plus a default policy that consults the [`ConnectionTracker`] from
//! [`super::rate_limit`] when no explicit rule matches. Rules are evaluated in
//! insertion order; the first matching rule's [`FirewallAction`] is returned.
//!
//! # Rule matching
//!
//! Only the source IP CIDR is evaluated in this version. `dst_port` and
//! `protocol` fields are carried on [`FirewallRule`] for forward compatibility
//! but are not consulted by [`Firewall::match_rule`]. A rule with `src_ip ==
//! None` matches every source.

use alloc::vec::Vec;

use super::rate_limit::ConnectionTracker;
use crate::tcpip::addr::{Ipv4Addr, Ipv4Cidr};

/// IP protocol type alias (re-exports smoltcp type).
pub type IpProtocol = smoltcp::wire::IpProtocol;

/// Firewall action for a matched packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallAction {
    /// Permit the connection.
    Allow,
    /// Silently discard the connection.
    Drop,
    /// Reject the connection (actively refuse).
    Reject,
}

/// Default firewall policy when no rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallPolicy {
    /// Permit by default, subject to connection-tracker limits.
    AllowAll,
    /// Deny by default.
    DropAll,
}

/// A single firewall rule.
///
/// `src_ip` of `None` matches any source address. `dst_port` and `protocol`
/// are stored for future use but are not evaluated by [`Firewall::match_rule`]
/// in this version.
#[derive(Debug, Clone)]
pub struct FirewallRule {
    /// Action taken when this rule matches.
    pub action: FirewallAction,
    /// Source CIDR to match, or `None` to match any source.
    pub src_ip: Option<Ipv4Cidr>,
    /// Destination port to match (not evaluated in this version).
    pub dst_port: Option<u16>,
    /// IP protocol to match (not evaluated in this version).
    pub protocol: Option<IpProtocol>,
}

/// Firewall rule engine with connection tracking.
///
/// Holds an ordered list of [`FirewallRule`]s, a [`FirewallPolicy`] used when
/// no rule matches, and a [`ConnectionTracker`] consulted only on the
/// `AllowAll` default path to enforce per-IP/total caps.
pub struct Firewall {
    rules: Vec<FirewallRule>,
    default_policy: FirewallPolicy,
    conn_tracker: ConnectionTracker,
}

impl Firewall {
    /// Create a new firewall with the given default policy and connection
    /// tracker. The rule list starts empty.
    pub fn new(default: FirewallPolicy, conn_tracker: ConnectionTracker) -> Self {
        Self {
            rules: Vec::new(),
            default_policy: default,
            conn_tracker,
        }
    }

    /// Append a rule to the end of the rule list. Rules are evaluated in
    /// insertion order; earlier rules take priority.
    pub fn add_rule(&mut self, rule: FirewallRule) {
        self.rules.push(rule);
    }

    /// Remove the rule at `index`. Out-of-bounds indices are silently ignored.
    /// Removal preserves the relative order of the remaining rules, which is
    /// important because rule priority is positional.
    pub fn remove_rule(&mut self, index: usize) {
        if index < self.rules.len() {
            self.rules.remove(index);
        }
    }

    /// Evaluate a new connection from `src` at timestamp `now` (ms).
    ///
    /// Rules are scanned in order; the first matching rule's action is
    /// returned and the connection tracker is **not** consulted. If no rule
    /// matches, the default policy applies:
    /// - [`FirewallPolicy::AllowAll`] → [`ConnectionTracker::try_connect`] is
    ///   called; returns [`FirewallAction::Allow`] on success or
    ///   [`FirewallAction::Drop`] when a cap is exceeded.
    /// - [`FirewallPolicy::DropAll`] → returns [`FirewallAction::Drop`] without
    ///   touching the tracker.
    pub fn check_connection(&mut self, src: Ipv4Addr, now: u64) -> FirewallAction {
        for rule in &self.rules {
            if Self::match_rule(rule, src) {
                return rule.action;
            }
        }
        match self.default_policy {
            FirewallPolicy::AllowAll => {
                if self.conn_tracker.try_connect(src, now) {
                    FirewallAction::Allow
                } else {
                    FirewallAction::Drop
                }
            }
            FirewallPolicy::DropAll => FirewallAction::Drop,
        }
    }

    /// Test whether `rule` matches source address `src`.
    ///
    /// Returns `true` when the rule's `src_ip` is `None` (match-all) or when
    /// the CIDR contains `src`. `dst_port` and `protocol` are not evaluated.
    pub fn match_rule(rule: &FirewallRule, src: Ipv4Addr) -> bool {
        match &rule.src_ip {
            Some(cidr) => cidr.contains_addr(&src),
            None => true,
        }
    }

    /// Return a slice over the current rule list.
    pub fn rules(&self) -> &[FirewallRule] {
        &self.rules
    }

    /// Return the configured default policy.
    pub fn default_policy(&self) -> FirewallPolicy {
        self.default_policy
    }

    /// Borrow the underlying connection tracker.
    pub fn conn_tracker(&self) -> &ConnectionTracker {
        &self.conn_tracker
    }

    /// Mutably borrow the underlying connection tracker.
    pub fn conn_tracker_mut(&mut self) -> &mut ConnectionTracker {
        &mut self.conn_tracker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcpip::addr::{ipv4_addr, ipv4_cidr};

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        ipv4_addr(a, b, c, d)
    }

    fn cidr(a: u8, b: u8, c: u8, d: u8, prefix: u8) -> Ipv4Cidr {
        ipv4_cidr(ipv4_addr(a, b, c, d), prefix)
    }

    fn allow_rule_src(src: Ipv4Cidr) -> FirewallRule {
        FirewallRule {
            action: FirewallAction::Allow,
            src_ip: Some(src),
            dst_port: None,
            protocol: None,
        }
    }

    fn drop_rule_src(src: Ipv4Cidr) -> FirewallRule {
        FirewallRule {
            action: FirewallAction::Drop,
            src_ip: Some(src),
            dst_port: None,
            protocol: None,
        }
    }

    fn reject_rule_src(src: Ipv4Cidr) -> FirewallRule {
        FirewallRule {
            action: FirewallAction::Reject,
            src_ip: Some(src),
            dst_port: None,
            protocol: None,
        }
    }

    fn tracker() -> ConnectionTracker {
        ConnectionTracker::new(5, 100)
    }

    #[test]
    fn test_new_initial_state() {
        let fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        assert!(fw.rules().is_empty());
        assert_eq!(fw.default_policy(), FirewallPolicy::AllowAll);
        assert_eq!(fw.conn_tracker().total(), 0);
        assert_eq!(fw.conn_tracker().max_per_ip(), 5);
    }

    #[test]
    fn test_add_rule_appends() {
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        fw.add_rule(allow_rule_src(cidr(192, 168, 1, 0, 24)));
        assert_eq!(fw.rules().len(), 1);
        fw.add_rule(drop_rule_src(cidr(10, 0, 0, 0, 8)));
        assert_eq!(fw.rules().len(), 2);
        assert_eq!(fw.rules()[0].action, FirewallAction::Allow);
        assert_eq!(fw.rules()[1].action, FirewallAction::Drop);
    }

    #[test]
    fn test_check_connection_matches_allow_rule() {
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        fw.add_rule(allow_rule_src(cidr(192, 168, 1, 0, 24)));
        assert_eq!(
            fw.check_connection(ip(192, 168, 1, 55), 0),
            FirewallAction::Allow
        );
        // Matching an Allow rule must not touch the connection tracker.
        assert_eq!(fw.conn_tracker().total(), 0);
    }

    #[test]
    fn test_check_connection_matches_drop_rule() {
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        fw.add_rule(drop_rule_src(cidr(10, 0, 0, 0, 8)));
        assert_eq!(
            fw.check_connection(ip(10, 1, 2, 3), 0),
            FirewallAction::Drop
        );
        // Matching a Drop rule must not touch the connection tracker.
        assert_eq!(fw.conn_tracker().total(), 0);
    }

    #[test]
    fn test_check_connection_matches_reject_rule() {
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        fw.add_rule(reject_rule_src(cidr(172, 16, 0, 0, 12)));
        assert_eq!(
            fw.check_connection(ip(172, 16, 5, 5), 0),
            FirewallAction::Reject
        );
    }

    #[test]
    fn test_check_connection_no_match_allow_all() {
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        fw.add_rule(drop_rule_src(cidr(10, 0, 0, 0, 8)));
        // 192.168.x.x does not match the 10/8 drop rule → falls through to
        // AllowAll, which calls try_connect and admits the connection.
        assert_eq!(
            fw.check_connection(ip(192, 168, 1, 1), 0),
            FirewallAction::Allow
        );
        assert_eq!(fw.conn_tracker().total(), 1);
        assert_eq!(fw.conn_tracker().count_for(ip(192, 168, 1, 1)), 1);
    }

    #[test]
    fn test_check_connection_no_match_drop_all() {
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        fw.add_rule(allow_rule_src(cidr(10, 0, 0, 0, 8)));
        // 192.168.x.x does not match the 10/8 allow rule → falls through to
        // DropAll.
        assert_eq!(
            fw.check_connection(ip(192, 168, 1, 1), 0),
            FirewallAction::Drop
        );
        // DropAll must not touch the connection tracker.
        assert_eq!(fw.conn_tracker().total(), 0);
    }

    #[test]
    fn test_check_connection_allow_all_but_conn_limit_exceeded() {
        let tracker = ConnectionTracker::new(1, 1);
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker);
        // First connection is admitted.
        assert_eq!(
            fw.check_connection(ip(10, 0, 0, 1), 0),
            FirewallAction::Allow
        );
        // Second connection exceeds the per-IP cap → Drop.
        assert_eq!(
            fw.check_connection(ip(10, 0, 0, 1), 10),
            FirewallAction::Drop
        );
        // A different IP exceeds the total cap → Drop.
        assert_eq!(
            fw.check_connection(ip(10, 0, 0, 2), 20),
            FirewallAction::Drop
        );
        assert_eq!(fw.conn_tracker().total(), 1);
    }

    #[test]
    fn test_match_rule_cidr_match() {
        let rule = allow_rule_src(cidr(192, 168, 1, 0, 24));
        assert!(Firewall::match_rule(&rule, ip(192, 168, 1, 100)));
        assert!(Firewall::match_rule(&rule, ip(192, 168, 1, 0)));
        assert!(Firewall::match_rule(&rule, ip(192, 168, 1, 255)));
    }

    #[test]
    fn test_match_rule_cidr_no_match() {
        let rule = allow_rule_src(cidr(192, 168, 1, 0, 24));
        assert!(!Firewall::match_rule(&rule, ip(192, 168, 2, 1)));
        assert!(!Firewall::match_rule(&rule, ip(10, 0, 0, 1)));
        assert!(!Firewall::match_rule(&rule, ip(192, 169, 1, 1)));
    }

    #[test]
    fn test_match_rule_src_ip_none_matches_all() {
        let rule = FirewallRule {
            action: FirewallAction::Allow,
            src_ip: None,
            dst_port: None,
            protocol: None,
        };
        assert!(Firewall::match_rule(&rule, ip(10, 0, 0, 1)));
        assert!(Firewall::match_rule(&rule, ip(192, 168, 1, 1)));
        assert!(Firewall::match_rule(&rule, ip(172, 16, 0, 1)));
    }

    #[test]
    fn test_match_rule_cidr_24_boundary() {
        let rule = drop_rule_src(cidr(192, 168, 1, 0, 24));
        // All addresses in 192.168.1.0/24 match.
        assert!(Firewall::match_rule(&rule, ip(192, 168, 1, 0)));
        assert!(Firewall::match_rule(&rule, ip(192, 168, 1, 255)));
        // Adjacent /24 blocks do not match.
        assert!(!Firewall::match_rule(&rule, ip(192, 168, 0, 255)));
        assert!(!Firewall::match_rule(&rule, ip(192, 168, 2, 0)));
    }

    #[test]
    fn test_match_rule_cidr_32_exact_only() {
        let rule = allow_rule_src(cidr(10, 0, 0, 5, 32));
        assert!(Firewall::match_rule(&rule, ip(10, 0, 0, 5)));
        assert!(!Firewall::match_rule(&rule, ip(10, 0, 0, 6)));
        assert!(!Firewall::match_rule(&rule, ip(10, 0, 0, 4)));
    }

    #[test]
    fn test_remove_rule_preserves_order() {
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        fw.add_rule(allow_rule_src(cidr(10, 0, 0, 0, 8)));
        fw.add_rule(drop_rule_src(cidr(192, 168, 0, 0, 16)));
        fw.add_rule(reject_rule_src(cidr(172, 16, 0, 0, 12)));
        assert_eq!(fw.rules().len(), 3);

        fw.remove_rule(0);
        assert_eq!(fw.rules().len(), 2);
        // After removing index 0, the former index 1 shifts to index 0.
        assert_eq!(fw.rules()[0].action, FirewallAction::Drop);
        assert_eq!(fw.rules()[1].action, FirewallAction::Reject);
    }

    #[test]
    fn test_remove_rule_out_of_bounds_noop() {
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        fw.add_rule(allow_rule_src(cidr(10, 0, 0, 0, 8)));
        // Removing an invalid index must not panic.
        fw.remove_rule(5);
        fw.remove_rule(0);
        fw.remove_rule(0);
        assert!(fw.rules().is_empty());
    }

    #[test]
    fn test_multi_rule_first_match_wins() {
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        // Both rules match 10.0.0.5; the first (Allow) must win.
        fw.add_rule(allow_rule_src(cidr(10, 0, 0, 0, 8)));
        fw.add_rule(drop_rule_src(cidr(10, 0, 0, 0, 8)));
        assert_eq!(
            fw.check_connection(ip(10, 0, 0, 5), 0),
            FirewallAction::Allow
        );
    }

    #[test]
    fn test_rules_query_returns_slice() {
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        fw.add_rule(allow_rule_src(cidr(10, 0, 0, 0, 8)));
        let rules: &[FirewallRule] = fw.rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, FirewallAction::Allow);
    }

    #[test]
    fn test_conn_tracker_mut_accessor() {
        let mut fw = Firewall::new(FirewallPolicy::AllowAll, tracker());
        // Mutably access the tracker and drive it directly.
        assert!(fw.conn_tracker_mut().try_connect(ip(10, 0, 0, 1), 0));
        assert_eq!(fw.conn_tracker().count_for(ip(10, 0, 0, 1)), 1);
    }

    #[test]
    fn test_check_connection_default_policy_priority_over_rules() {
        // A match-all Allow rule should take priority over a DropAll default.
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        fw.add_rule(FirewallRule {
            action: FirewallAction::Allow,
            src_ip: None,
            dst_port: None,
            protocol: None,
        });
        assert_eq!(
            fw.check_connection(ip(8, 8, 8, 8), 0),
            FirewallAction::Allow
        );
        // Rule matched → tracker untouched.
        assert_eq!(fw.conn_tracker().total(), 0);
    }

    #[test]
    fn test_check_connection_drop_all_never_tracks() {
        let mut fw = Firewall::new(FirewallPolicy::DropAll, tracker());
        for i in 0..10u8 {
            assert_eq!(
                fw.check_connection(ip(10, 0, 0, i), 0),
                FirewallAction::Drop
            );
        }
        // DropAll never consults the tracker.
        assert_eq!(fw.conn_tracker().total(), 0);
    }
}
