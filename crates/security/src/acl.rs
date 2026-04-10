//! Network Access Control List (NACL) with binary radix trie matching.
//!
//! Supports both IPv4 and IPv6 CIDR rules with O(prefix_length) lookup.
//! Copy-on-write updates via `arc_swap::ArcSwap` allow lock-free reads.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use tracing::{debug, trace};

/// ACL operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclMode {
    /// Default deny: only explicitly allowed IPs pass.
    Allowlist,
    /// Default allow: only explicitly denied IPs are blocked.
    Denylist,
}

/// Action for a matched rule or the default decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclAction {
    Allow,
    Deny,
}

// ── Radix Trie ──────────────────────────────────────────────────────────────

/// A node in the binary radix trie.
#[derive(Debug, Clone)]
struct TrieNode {
    children: [Option<Box<TrieNode>>; 2],
    /// If `Some`, this node marks the end of a prefix rule.
    action: Option<AclAction>,
    /// Per-rule hit counter (only meaningful when `action.is_some()`).
    hits: u64,
}

impl TrieNode {
    fn new() -> Self {
        Self {
            children: [None, None],
            action: None,
            hits: 0,
        }
    }
}

/// Binary radix trie storing CIDR rules.
///
/// Each bit of the IP address selects the next child (0 or 1). A prefix of
/// length *n* traverses exactly *n* nodes.
#[derive(Debug, Clone)]
pub struct RadixTrie {
    root: TrieNode,
}

impl Default for RadixTrie {
    fn default() -> Self {
        Self::new()
    }
}

impl RadixTrie {
    pub fn new() -> Self {
        Self {
            root: TrieNode::new(),
        }
    }

    /// Insert a CIDR prefix with the given action.
    pub fn insert(&mut self, bits: &[u8], prefix_len: u8, action: AclAction) {
        let mut node = &mut self.root;
        for i in 0..prefix_len as usize {
            let byte_idx = i / 8;
            let bit_idx = 7 - (i % 8);
            let bit = ((bits[byte_idx] >> bit_idx) & 1) as usize;
            node = node.children[bit].get_or_insert_with(|| Box::new(TrieNode::new()));
        }
        node.action = Some(action);
        node.hits = 0;
    }

    /// Remove a CIDR prefix. Returns `true` if the rule existed.
    pub fn remove(&mut self, bits: &[u8], prefix_len: u8) -> bool {
        Self::remove_recursive(&mut self.root, bits, prefix_len, 0)
    }

    fn remove_recursive(node: &mut TrieNode, bits: &[u8], prefix_len: u8, depth: usize) -> bool {
        if depth == prefix_len as usize {
            if node.action.is_some() {
                node.action = None;
                node.hits = 0;
                return true;
            }
            return false;
        }
        let byte_idx = depth / 8;
        let bit_idx = 7 - (depth % 8);
        let bit = ((bits[byte_idx] >> bit_idx) & 1) as usize;
        if let Some(ref mut child) = node.children[bit] {
            Self::remove_recursive(child, bits, prefix_len, depth + 1)
        } else {
            false
        }
    }

    /// Lookup the longest matching prefix for the given IP bits.
    /// Returns the action and increments the hit counter of the matched rule.
    pub fn lookup(&mut self, bits: &[u8], total_bits: usize) -> Option<AclAction> {
        // Walk the trie, remembering the deepest terminal node's depth.
        let mut best_action: Option<AclAction> = None;
        let mut best_depth: Option<usize> = None;

        // Check root
        if self.root.action.is_some() {
            best_action = self.root.action;
            best_depth = Some(0);
        }

        let mut node = &self.root;
        for i in 0..total_bits {
            let byte_idx = i / 8;
            let bit_idx = 7 - (i % 8);
            let bit = ((bits[byte_idx] >> bit_idx) & 1) as usize;
            match &node.children[bit] {
                Some(child) => {
                    node = child;
                    if node.action.is_some() {
                        best_action = node.action;
                        best_depth = Some(i + 1);
                    }
                }
                None => break,
            }
        }

        // Increment hit counter on the best-matched node.
        if let Some(depth) = best_depth {
            let node_mut = Self::walk_to_depth(&mut self.root, bits, depth);
            node_mut.hits += 1;
        }

        best_action
    }

    fn walk_to_depth<'a>(root: &'a mut TrieNode, bits: &[u8], depth: usize) -> &'a mut TrieNode {
        let mut node = root;
        for i in 0..depth {
            let byte_idx = i / 8;
            let bit_idx = 7 - (i % 8);
            let bit = ((bits[byte_idx] >> bit_idx) & 1) as usize;
            node = node.children[bit].as_deref_mut().unwrap();
        }
        node
    }

    /// Read-only lookup (does not increment hit counters).
    pub fn lookup_readonly(&self, bits: &[u8], total_bits: usize) -> Option<AclAction> {
        let mut best_action: Option<AclAction> = None;
        let mut node = &self.root;

        if node.action.is_some() {
            best_action = node.action;
        }

        for i in 0..total_bits {
            let byte_idx = i / 8;
            let bit_idx = 7 - (i % 8);
            let bit = ((bits[byte_idx] >> bit_idx) & 1) as usize;
            match &node.children[bit] {
                Some(child) => {
                    node = child;
                    if node.action.is_some() {
                        best_action = node.action;
                    }
                }
                None => break,
            }
        }

        best_action
    }
}

// ── CIDR Parsing ────────────────────────────────────────────────────────────

/// Parse a CIDR string into (bytes, prefix_length, total_bits).
fn parse_cidr(cidr: &str) -> Result<(Vec<u8>, u8, usize)> {
    let parts: Vec<&str> = cidr.split('/').collect();
    let (addr_str, prefix_len) = match parts.len() {
        1 => {
            let addr: IpAddr = parts[0].parse().context("invalid IP address")?;
            let prefix = match addr {
                IpAddr::V4(_) => 32u8,
                IpAddr::V6(_) => 128u8,
            };
            (parts[0], prefix)
        }
        2 => {
            let prefix: u8 = parts[1].parse().context("invalid prefix length")?;
            (parts[0], prefix)
        }
        _ => anyhow::bail!("invalid CIDR format: {}", cidr),
    };

    let addr: IpAddr = addr_str.parse().context("invalid IP address")?;
    match addr {
        IpAddr::V4(v4) => {
            if prefix_len > 32 {
                anyhow::bail!("IPv4 prefix length {} exceeds 32", prefix_len);
            }
            Ok((v4.octets().to_vec(), prefix_len, 32))
        }
        IpAddr::V6(v6) => {
            if prefix_len > 128 {
                anyhow::bail!("IPv6 prefix length {} exceeds 128", prefix_len);
            }
            Ok((v6.octets().to_vec(), prefix_len, 128))
        }
    }
}

/// Convert an IpAddr to (bytes, total_bits).
fn ip_to_bits(ip: &IpAddr) -> (Vec<u8>, usize) {
    match ip {
        IpAddr::V4(v4) => (v4.octets().to_vec(), 32),
        IpAddr::V6(v6) => (v6.octets().to_vec(), 128),
    }
}

// ── NACL ────────────────────────────────────────────────────────────────────

/// Network Access Control List.
///
/// Uses a binary radix trie for O(prefix_length) IP matching with
/// copy-on-write atomic updates for concurrent access.
pub struct Nacl {
    mode: AclMode,
    trie: ArcSwap<RadixTrie>,
    accepted: AtomicU64,
    denied: AtomicU64,
}

impl Nacl {
    /// Create a new NACL with the given mode.
    pub fn new(mode: AclMode) -> Self {
        debug!(?mode, "creating NACL");
        Self {
            mode,
            trie: ArcSwap::new(Arc::new(RadixTrie::new())),
            accepted: AtomicU64::new(0),
            denied: AtomicU64::new(0),
        }
    }

    /// Check whether the given IP is allowed or denied.
    ///
    /// This performs a read-only lookup (hit counters are not incremented
    /// on the shared trie to avoid requiring a mutable snapshot per read).
    pub fn check(&self, ip: &IpAddr) -> AclAction {
        let guard = self.trie.load();
        let (bits, total_bits) = ip_to_bits(ip);
        let rule_action = guard.lookup_readonly(&bits, total_bits);

        let decision = match rule_action {
            Some(action) => action,
            None => match self.mode {
                AclMode::Allowlist => AclAction::Deny,
                AclMode::Denylist => AclAction::Allow,
            },
        };

        match decision {
            AclAction::Allow => {
                self.accepted.fetch_add(1, Ordering::Relaxed);
                trace!(?ip, "ACL allow");
            }
            AclAction::Deny => {
                self.denied.fetch_add(1, Ordering::Relaxed);
                trace!(?ip, "ACL deny");
            }
        }

        decision
    }

    /// Add a CIDR rule. Performs a copy-on-write update of the trie.
    pub fn add_rule(&self, cidr: &str, action: AclAction) -> Result<()> {
        let (bits, prefix_len, _total_bits) = parse_cidr(cidr)?;
        debug!(cidr, ?action, "adding ACL rule");

        let old = self.trie.load();
        let mut new_trie = (**old).clone();
        new_trie.insert(&bits, prefix_len, action);
        self.trie.store(Arc::new(new_trie));
        Ok(())
    }

    /// Remove a CIDR rule. Performs a copy-on-write update of the trie.
    pub fn remove_rule(&self, cidr: &str) -> Result<()> {
        let (bits, prefix_len, _total_bits) = parse_cidr(cidr)?;
        debug!(cidr, "removing ACL rule");

        let old = self.trie.load();
        let mut new_trie = (**old).clone();
        if !new_trie.remove(&bits, prefix_len) {
            anyhow::bail!("rule not found: {}", cidr);
        }
        self.trie.store(Arc::new(new_trie));
        Ok(())
    }

    /// Return (accepted, denied) counters.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.accepted.load(Ordering::Relaxed),
            self.denied.load(Ordering::Relaxed),
        )
    }

    /// Return the current ACL mode.
    pub fn mode(&self) -> AclMode {
        self.mode
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn test_allowlist_default_deny() {
        let nacl = Nacl::new(AclMode::Allowlist);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert_eq!(nacl.check(&ip), AclAction::Deny);
    }

    #[test]
    fn test_denylist_default_allow() {
        let nacl = Nacl::new(AclMode::Denylist);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert_eq!(nacl.check(&ip), AclAction::Allow);
    }

    #[test]
    fn test_allowlist_explicit_allow() {
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("10.0.0.0/8", AclAction::Allow).unwrap();

        let allowed: IpAddr = "10.1.2.3".parse().unwrap();
        let denied: IpAddr = "192.168.1.1".parse().unwrap();

        assert_eq!(nacl.check(&allowed), AclAction::Allow);
        assert_eq!(nacl.check(&denied), AclAction::Deny);
    }

    #[test]
    fn test_denylist_explicit_deny() {
        let nacl = Nacl::new(AclMode::Denylist);
        nacl.add_rule("10.0.0.0/8", AclAction::Deny).unwrap();

        let denied: IpAddr = "10.1.2.3".parse().unwrap();
        let allowed: IpAddr = "192.168.1.1".parse().unwrap();

        assert_eq!(nacl.check(&denied), AclAction::Deny);
        assert_eq!(nacl.check(&allowed), AclAction::Allow);
    }

    #[test]
    fn test_longest_prefix_match() {
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("10.0.0.0/8", AclAction::Deny).unwrap();
        nacl.add_rule("10.1.0.0/16", AclAction::Allow).unwrap();

        let ip1: IpAddr = "10.1.2.3".parse().unwrap();
        let ip2: IpAddr = "10.2.0.1".parse().unwrap();

        // 10.1.2.3 matches /16 (Allow) which is more specific than /8 (Deny)
        assert_eq!(nacl.check(&ip1), AclAction::Allow);
        // 10.2.0.1 only matches /8 (Deny)
        assert_eq!(nacl.check(&ip2), AclAction::Deny);
    }

    #[test]
    fn test_exact_host_ipv4() {
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("192.168.1.100/32", AclAction::Allow).unwrap();

        let exact: IpAddr = "192.168.1.100".parse().unwrap();
        let other: IpAddr = "192.168.1.101".parse().unwrap();

        assert_eq!(nacl.check(&exact), AclAction::Allow);
        assert_eq!(nacl.check(&other), AclAction::Deny);
    }

    #[test]
    fn test_ipv6_cidr() {
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("2001:db8::/32", AclAction::Allow).unwrap();

        let inside: IpAddr = "2001:db8::1".parse().unwrap();
        let outside: IpAddr = "2001:db9::1".parse().unwrap();

        assert_eq!(nacl.check(&inside), AclAction::Allow);
        assert_eq!(nacl.check(&outside), AclAction::Deny);
    }

    #[test]
    fn test_ipv6_exact_host() {
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("::1/128", AclAction::Allow).unwrap();

        let loopback: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let other: IpAddr = "::2".parse().unwrap();

        assert_eq!(nacl.check(&loopback), AclAction::Allow);
        assert_eq!(nacl.check(&other), AclAction::Deny);
    }

    #[test]
    fn test_remove_rule() {
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("10.0.0.0/8", AclAction::Allow).unwrap();

        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert_eq!(nacl.check(&ip), AclAction::Allow);

        nacl.remove_rule("10.0.0.0/8").unwrap();
        assert_eq!(nacl.check(&ip), AclAction::Deny);
    }

    #[test]
    fn test_remove_nonexistent_rule() {
        let nacl = Nacl::new(AclMode::Allowlist);
        let result = nacl.remove_rule("10.0.0.0/8");
        assert!(result.is_err());
    }

    #[test]
    fn test_stats_counting() {
        let nacl = Nacl::new(AclMode::Denylist);
        nacl.add_rule("10.0.0.0/8", AclAction::Deny).unwrap();

        let denied_ip: IpAddr = "10.1.2.3".parse().unwrap();
        let allowed_ip: IpAddr = "192.168.1.1".parse().unwrap();

        nacl.check(&denied_ip);
        nacl.check(&denied_ip);
        nacl.check(&allowed_ip);

        let (accepted, denied) = nacl.stats();
        assert_eq!(accepted, 1);
        assert_eq!(denied, 2);
    }

    #[test]
    fn test_cidr_without_prefix() {
        let nacl = Nacl::new(AclMode::Allowlist);
        // Without /prefix, treat as /32 for IPv4
        nacl.add_rule("192.168.1.1", AclAction::Allow).unwrap();

        let exact: IpAddr = "192.168.1.1".parse().unwrap();
        let other: IpAddr = "192.168.1.2".parse().unwrap();

        assert_eq!(nacl.check(&exact), AclAction::Allow);
        assert_eq!(nacl.check(&other), AclAction::Deny);
    }

    #[test]
    fn test_invalid_cidr() {
        let nacl = Nacl::new(AclMode::Allowlist);
        assert!(nacl.add_rule("not-an-ip/8", AclAction::Allow).is_err());
        assert!(nacl.add_rule("10.0.0.0/33", AclAction::Allow).is_err());
    }

    #[test]
    fn test_radix_trie_direct() {
        let mut trie = RadixTrie::new();

        // Insert 10.0.0.0/8 -> Allow
        let bits = [10u8, 0, 0, 0];
        trie.insert(&bits, 8, AclAction::Allow);

        // Lookup 10.1.2.3
        let lookup_bits = [10u8, 1, 2, 3];
        assert_eq!(
            trie.lookup_readonly(&lookup_bits, 32),
            Some(AclAction::Allow)
        );

        // Lookup 11.0.0.1 -> no match
        let miss_bits = [11u8, 0, 0, 1];
        assert_eq!(trie.lookup_readonly(&miss_bits, 32), None);
    }

    #[test]
    fn test_ipv4_mapped_ipv6_are_separate() {
        // IPv4 and IPv6 are stored in separate bit-spaces (4 vs 16 bytes)
        let nacl = Nacl::new(AclMode::Allowlist);
        nacl.add_rule("10.0.0.0/8", AclAction::Allow).unwrap();

        let v4: IpAddr = "10.1.2.3".parse().unwrap();
        // ::ffff:10.1.2.3 is IPv6 - different bit representation
        let v6: IpAddr = "::ffff:10.1.2.3".parse().unwrap();

        assert_eq!(nacl.check(&v4), AclAction::Allow);
        // The v6 address won't match the v4 rule
        assert_eq!(nacl.check(&v6), AclAction::Deny);
    }
}
