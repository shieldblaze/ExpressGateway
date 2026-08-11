//! Config hot-reload diff: partition every change into swappable (applied live via the
//! per-listener `ArcSwap`) and restart-required (structural or establishment-input).
//!
//! HONESTY CONTRACT: every restart-required change must be DETECTED and reported so the reload
//! routine can warn per field. A change that is silently dropped, or reported as a clean success,
//! makes the reload lie about what is running.
//!
//! Listener identity is the BIND ADDRESS, so a changed `address` reads as a remove plus an add.

use crate::LbConfig;

/// A swappable change; each variant names the field and listener for the reload log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwappableChange {
    /// Per-listener L7 fields applied live by rebuilding the proxy. This set must EXACTLY match
    /// what `rebuild_l7_proxies` consumes, or the diff reports as applied something that is not:
    /// `backends`, `http`, `h2_security`, `websocket`, `alt_svc`, `grpc`.
    ListenerL7 {
        /// Bind address of the affected listener.
        address: String,
        /// The specific changed L7 fields (for the operator log).
        fields: Vec<&'static str>,
    },
    /// `[runtime].max_keepalive_requests` changed; applied by rebuilding EVERY L7 proxy.
    RuntimeMaxKeepaliveRequests,
}

impl SwappableChange {
    /// Human-readable one-line description for the operator log.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::ListenerL7 { address, fields } => {
                format!(
                    "listener {address}: L7 config changed ({}) — applied live",
                    fields.join(", ")
                )
            }
            Self::RuntimeMaxKeepaliveRequests => {
                "runtime.max_keepalive_requests changed — applied live (all L7 listeners)"
                    .to_owned()
            }
        }
    }

    /// Stable field label for the `{field}`-keyed reload metric.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::ListenerL7 { .. } => "listener.l7",
            Self::RuntimeMaxKeepaliveRequests => "max_keepalive_requests",
        }
    }

    /// Bind address this change targets, if it is a per-listener change.
    #[must_use]
    pub fn address(&self) -> Option<&str> {
        match self {
            Self::ListenerL7 { address, .. } => Some(address),
            Self::RuntimeMaxKeepaliveRequests => None,
        }
    }
}

/// A restart-required change, carrying enough context for the "not applied" warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartRequiredChange {
    /// Listener added — binds a new port.
    ListenerAdded {
        /// Bind address of the added listener.
        address: String,
    },
    /// Listener removed — unbinds a port.
    ListenerRemoved {
        /// Bind address of the removed listener.
        address: String,
    },
    /// Protocol changed — the whole datapath differs.
    ListenerProtocol {
        /// Bind address of the affected listener.
        address: String,
        /// Old protocol.
        old: String,
        /// New protocol.
        new: String,
    },
    /// A listener field baked at spawn changed (TLS paths, ALPN, QUIC transport params, drain
    /// budgets, the H3 extended-connect toggle).
    ListenerField {
        /// Bind address of the affected listener.
        address: String,
        /// The specific non-swappable field that changed.
        field: &'static str,
    },
    /// A `[runtime]` establishment input (XDP attach) changed.
    RuntimeField {
        /// The specific non-swappable runtime field that changed.
        field: &'static str,
    },
    /// The `[observability]` (admin/metrics) bind changed.
    ObservabilityBind,
    /// The `[admin]` block (auth / bind policy) changed.
    AdminBlock,
    /// `[passthrough]` changed; Mode A is a flow-keyed datapath baked at spawn.
    PassthroughBlock,
}

impl RestartRequiredChange {
    /// Human-readable one-line description for the operator log.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::ListenerAdded { address } => {
                format!("listener {address}: added — requires restart, not applied")
            }
            Self::ListenerRemoved { address } => {
                format!("listener {address}: removed — requires restart, not applied")
            }
            Self::ListenerProtocol { address, old, new } => format!(
                "listener {address}: protocol {old:?} -> {new:?} — requires restart, not applied"
            ),
            Self::ListenerField { address, field } => {
                format!("listener {address}: {field} changed — requires restart, not applied")
            }
            Self::RuntimeField { field } => {
                format!("runtime.{field} changed — requires restart, not applied")
            }
            Self::ObservabilityBind => {
                "observability bind changed — requires restart, not applied".to_owned()
            }
            Self::AdminBlock => "admin block changed — requires restart, not applied".to_owned(),
            Self::PassthroughBlock => {
                "passthrough block changed — requires restart, not applied".to_owned()
            }
        }
    }

    /// Stable field label for the `{field}`-keyed reload metric.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::ListenerAdded { .. } => "listener.added",
            Self::ListenerRemoved { .. } => "listener.removed",
            Self::ListenerProtocol { .. } => "listener.protocol",
            Self::ListenerField { field, .. } | Self::RuntimeField { field } => field,
            Self::ObservabilityBind => "observability.bind",
            Self::AdminBlock => "admin",
            Self::PassthroughBlock => "passthrough",
        }
    }
}

/// The full partition of every change between two configs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadPlan {
    /// Changes that can be applied live via the per-listener `ArcSwap`.
    pub swappable: Vec<SwappableChange>,
    /// Changes the process cannot apply; logged and counted, never dropped.
    pub restart_required: Vec<RestartRequiredChange>,
}

impl ReloadPlan {
    /// True only when NOTHING changed. "Only restart-required changes" is not a no-op — it still
    /// has to produce warnings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.swappable.is_empty() && self.restart_required.is_empty()
    }

    /// `true` when there is at least one live-applicable change.
    #[must_use]
    pub fn has_swappable(&self) -> bool {
        !self.swappable.is_empty()
    }
}

impl LbConfig {
    /// Partition every change between the applied config and `new`.
    ///
    /// EXHAUSTIVE: every differing field lands in exactly one bucket, so `restart_required` is
    /// the complete set a caller must warn about.
    #[must_use]
    pub fn diff(&self, new: &Self) -> ReloadPlan {
        let mut plan = ReloadPlan::default();

        for old_l in &self.listeners {
            match new.listeners.iter().find(|n| n.address == old_l.address) {
                None => plan
                    .restart_required
                    .push(RestartRequiredChange::ListenerRemoved {
                        address: old_l.address.clone(),
                    }),
                Some(new_l) => diff_listener(old_l, new_l, &mut plan),
            }
        }
        for new_l in &new.listeners {
            if !self.listeners.iter().any(|o| o.address == new_l.address) {
                plan.restart_required
                    .push(RestartRequiredChange::ListenerAdded {
                        address: new_l.address.clone(),
                    });
            }
        }

        diff_runtime(self.runtime.as_ref(), new.runtime.as_ref(), &mut plan);
        if self.observability != new.observability {
            plan.restart_required
                .push(RestartRequiredChange::ObservabilityBind);
        }
        if self.admin != new.admin {
            plan.restart_required
                .push(RestartRequiredChange::AdminBlock);
        }
        if self.security != new.security {
            // `strict_te` is swappable in the taxonomy but the swap is NOT WIRED, so it is
            // reported restart-required rather than becoming a silent no-op.
            plan.restart_required
                .push(RestartRequiredChange::RuntimeField {
                    field: "security.strict_te",
                });
        }
        if self.passthrough != new.passthrough {
            plan.restart_required
                .push(RestartRequiredChange::PassthroughBlock);
        }

        plan
    }
}

/// Diff two listeners that share a bind address.
fn diff_listener(old: &crate::ListenerConfig, new: &crate::ListenerConfig, plan: &mut ReloadPlan) {
    let addr = &old.address;

    if old.protocol != new.protocol {
        plan.restart_required
            .push(RestartRequiredChange::ListenerProtocol {
                address: addr.clone(),
                old: old.protocol.clone(),
                new: new.protocol.clone(),
            });
        // A protocol change subsumes every other field diff here.
        return;
    }

    // This set MUST exactly match what `rebuild_l7_proxies` consumes — the rebuild passes
    // `new.{backends, http, h2_security, websocket, alt_svc, grpc}`. Diverge and the diff reports
    // as applied something that is not.
    let mut l7_fields: Vec<&'static str> = Vec::new();
    macro_rules! swappable_l7 {
        ($field:ident, $tag:literal) => {
            if old.$field != new.$field {
                l7_fields.push($tag);
            }
        };
    }
    swappable_l7!(backends, "backends");
    swappable_l7!(http, "http");
    swappable_l7!(h2_security, "h2_security");
    swappable_l7!(websocket, "websocket");
    swappable_l7!(alt_svc, "alt_svc");
    swappable_l7!(grpc, "grpc");
    if !l7_fields.is_empty() {
        plan.swappable.push(SwappableChange::ListenerL7 {
            address: addr.clone(),
            fields: l7_fields,
        });
    }

    // The proxy rebuild does NOT touch these, so restart-required is truthful: `tls` belongs to
    // the listener mode and SIGUSR1 cert-reload path, `quic` is baked into the config_factory at
    // spawn, and the drain budgets are read at SHUTDOWN from the boot config.
    macro_rules! restart_field {
        ($field:ident, $tag:literal) => {
            if old.$field != new.$field {
                plan.restart_required
                    .push(RestartRequiredChange::ListenerField {
                        address: addr.clone(),
                        field: $tag,
                    });
            }
        };
    }
    restart_field!(tls, "tls");
    restart_field!(quic, "quic");
    restart_field!(drain_timeout_ms, "drain_timeout_ms");
    restart_field!(drain_jitter_ms, "drain_jitter_ms");
}

/// Diff `[runtime]`. XDP attach is permanently restart-required; the taxonomy-swappable knobs
/// are reported restart-required until their swap is actually wired.
fn diff_runtime(
    old: Option<&crate::RuntimeConfig>,
    new: Option<&crate::RuntimeConfig>,
    plan: &mut ReloadPlan,
) {
    // Compare EFFECTIVE values, not `Option`s, so the change is still detected when the
    // `[runtime]` block itself appears or disappears.
    let eff_keepalive =
        |c: Option<&crate::RuntimeConfig>| c.map_or(100, |r| r.max_keepalive_requests);
    if eff_keepalive(old) != eff_keepalive(new) {
        plan.swappable
            .push(SwappableChange::RuntimeMaxKeepaliveRequests);
    }

    match (old, new) {
        (None, None) => {}
        (Some(o), Some(n)) if o == n => {}
        (None, Some(_)) | (Some(_), None) => {
            // The whole block appeared or disappeared; report once.
            plan.restart_required
                .push(RestartRequiredChange::RuntimeField { field: "runtime" });
        }
        (Some(o), Some(n)) => {
            macro_rules! rt_field {
                ($field:ident, $tag:literal) => {
                    if o.$field != n.$field {
                        plan.restart_required
                            .push(RestartRequiredChange::RuntimeField { field: $tag });
                    }
                };
            }
            rt_field!(xdp_enabled, "xdp_enabled");
            rt_field!(xdp_interface, "xdp_interface");
            rt_field!(xdp_mode, "xdp_mode");
            rt_field!(
                xdp_new_flow_cap_per_sec_per_cpu,
                "xdp_new_flow_cap_per_sec_per_cpu"
            );
            rt_field!(tls, "runtime.tls");
            // Swappable in the taxonomy, but the swap is not wired yet.
            rt_field!(drain_timeout_ms, "drain_timeout_ms");
            rt_field!(readiness_settle_ms, "readiness_settle_ms");
            rt_field!(drain_jitter_ms, "drain_jitter_ms");
            rt_field!(handshake_timeout_ms, "handshake_timeout_ms");
            rt_field!(max_inflight_connections, "max_inflight_connections");
            rt_field!(connect_timeout_ms, "connect_timeout_ms");
            rt_field!(per_ip_connection_cap, "per_ip_connection_cap");
            rt_field!(watchdog, "watchdog");
            rt_field!(header_underscore_policy, "header_underscore_policy");
            // `max_keepalive_requests` is handled swappably above, deliberately not here.
            rt_field!(
                max_requests_per_h3_connection,
                "max_requests_per_h3_connection"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SwappableChange;
    use crate::{BackendConfig, LbConfig, ListenerConfig};

    fn backend(addr: &str, weight: u32) -> BackendConfig {
        BackendConfig {
            address: addr.to_owned(),
            protocol: "h1".to_owned(),
            weight,
            tls_ca_path: None,
            tls_verify_hostname: None,
            tls_verify_peer: true,
        }
    }

    fn listener(addr: &str, backends: Vec<BackendConfig>) -> ListenerConfig {
        ListenerConfig {
            address: addr.to_owned(),
            protocol: "h1".to_owned(),
            tls: None,
            quic: None,
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends,
        }
    }

    fn cfg(listeners: Vec<ListenerConfig>) -> LbConfig {
        LbConfig {
            listeners,
            ..LbConfig::default()
        }
    }

    #[test]
    fn identical_configs_are_empty_plan() {
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let plan = a.diff(&a.clone());
        assert!(plan.is_empty());
        assert!(!plan.has_swappable());
    }

    #[test]
    fn backend_set_change_is_swappable() {
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let b = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.2:80", 1)],
        )]);
        let plan = a.diff(&b);
        assert_eq!(plan.swappable.len(), 1);
        assert!(plan.restart_required.is_empty());
        assert_eq!(plan.swappable[0].field(), "listener.l7");
        assert!(plan.has_swappable());
    }

    #[test]
    fn backend_weight_change_is_swappable() {
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let b = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 5)],
        )]);
        let plan = a.diff(&b);
        assert_eq!(plan.swappable.len(), 1);
        assert!(plan.restart_required.is_empty());
    }

    #[test]
    fn listener_add_is_restart_required() {
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let b = cfg(vec![
            listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]),
            listener("0.0.0.0:8443", vec![backend("10.0.0.2:80", 1)]),
        ]);
        let plan = a.diff(&b);
        assert!(plan.swappable.is_empty());
        assert_eq!(plan.restart_required.len(), 1);
        assert_eq!(plan.restart_required[0].field(), "listener.added");
    }

    #[test]
    fn listener_remove_is_restart_required() {
        let a = cfg(vec![
            listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]),
            listener("0.0.0.0:8443", vec![backend("10.0.0.2:80", 1)]),
        ]);
        let b = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let plan = a.diff(&b);
        assert!(plan.swappable.is_empty());
        assert_eq!(plan.restart_required.len(), 1);
        assert_eq!(plan.restart_required[0].field(), "listener.removed");
    }

    #[test]
    fn bind_address_change_is_add_plus_remove() {
        // A changed bind address must read as remove + add, never as a swappable change.
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let b = cfg(vec![listener(
            "0.0.0.0:9090",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let plan = a.diff(&b);
        assert!(plan.swappable.is_empty());
        assert_eq!(plan.restart_required.len(), 2);
        let fields: Vec<_> = plan.restart_required.iter().map(|c| c.field()).collect();
        assert!(fields.contains(&"listener.added"));
        assert!(fields.contains(&"listener.removed"));
    }

    #[test]
    fn protocol_change_is_restart_required_and_subsumes_backends() {
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let mut bl = listener("0.0.0.0:8080", vec![backend("10.0.0.2:80", 9)]);
        bl.protocol = "h1s".to_owned();
        let b = cfg(vec![bl]);
        let plan = a.diff(&b);
        // The protocol change subsumes the backend change.
        assert!(plan.swappable.is_empty());
        assert_eq!(plan.restart_required.len(), 1);
        assert_eq!(plan.restart_required[0].field(), "listener.protocol");
    }

    #[test]
    fn mixed_change_partitions_both_buckets() {
        // Exhaustive partition across two listeners.
        let a = cfg(vec![listener(
            "0.0.0.0:8080",
            vec![backend("10.0.0.1:80", 1)],
        )]);
        let b = cfg(vec![
            listener("0.0.0.0:8080", vec![backend("10.0.0.9:80", 1)]),
            listener("0.0.0.0:8443", vec![backend("10.0.0.2:80", 1)]),
        ]);
        let plan = a.diff(&b);
        assert_eq!(plan.swappable.len(), 1);
        assert_eq!(plan.restart_required.len(), 1);
    }

    fn http(header_ms: u64) -> crate::HttpTimeoutsConfig {
        crate::HttpTimeoutsConfig {
            header_timeout_ms: header_ms,
            body_timeout_ms: 30_000,
            total_timeout_ms: 60_000,
            head_timeout_ms: 30_000,
        }
    }

    #[test]
    fn http_timeout_change_is_swappable_l7() {
        // The rebuild applies `new.http`, so this MUST classify swappable.
        let mut a_l = listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]);
        a_l.http = Some(http(5_000));
        let mut b_l = listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]);
        b_l.http = Some(http(9_000));
        let plan = cfg(vec![a_l]).diff(&cfg(vec![b_l]));
        assert_eq!(plan.swappable.len(), 1, "http change must be swappable");
        assert!(plan.restart_required.is_empty());
        let fields = l7_fields(&plan.swappable[0]);
        assert!(
            fields.contains(&"http"),
            "fields must name http: {fields:?}"
        );
        assert!(!fields.contains(&"backends"), "backends did not change");
    }

    /// Changed-field list from a `ListenerL7`; returns rather than panicking, which is denied.
    fn l7_fields(change: &SwappableChange) -> Vec<&'static str> {
        match change {
            SwappableChange::ListenerL7 { fields, .. } => fields.clone(),
            SwappableChange::RuntimeMaxKeepaliveRequests => Vec::new(),
        }
    }

    #[test]
    fn combined_backend_and_http_change_is_one_swappable_with_both_fields() {
        // Both fields must appear on ONE entry — the rebuild applies both, so claiming either is
        // restart-required would be a lie.
        let mut a_l = listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]);
        a_l.http = Some(http(5_000));
        let mut b_l = listener("0.0.0.0:8080", vec![backend("10.0.0.2:80", 1)]);
        b_l.http = Some(http(9_000));
        let plan = cfg(vec![a_l]).diff(&cfg(vec![b_l]));
        assert_eq!(plan.swappable.len(), 1);
        assert!(plan.restart_required.is_empty());
        let fields = l7_fields(&plan.swappable[0]);
        assert!(fields.contains(&"backends"));
        assert!(fields.contains(&"http"));
    }

    #[test]
    fn max_keepalive_requests_change_is_swappable() {
        // Built via `parse_config` so `[runtime]` gets its serde defaults — `RuntimeConfig` has
        // no `Default` derive.
        let base = "[[listeners]]\naddress = \"0.0.0.0:8080\"\nprotocol = \"h1\"\n\
                    [[listeners.backends]]\naddress = \"10.0.0.1:80\"\nweight = 1\n";
        let a =
            crate::parse_config(&format!("{base}[runtime]\nmax_keepalive_requests = 2\n")).unwrap();
        let b =
            crate::parse_config(&format!("{base}[runtime]\nmax_keepalive_requests = 6\n")).unwrap();
        let plan = a.diff(&b);
        assert!(
            plan.swappable
                .iter()
                .any(|c| c.field() == "max_keepalive_requests"),
            "max_keepalive_requests change must be swappable: {:?}",
            plan.swappable
        );
        assert!(
            !plan
                .restart_required
                .iter()
                .any(|c| c.field() == "max_keepalive_requests"),
            "max_keepalive_requests must NOT be reported restart-required"
        );
    }

    #[test]
    fn tls_and_drain_changes_are_restart_required_not_swappable() {
        // The rebuild does not apply tls or the drain budgets, so they must not leak into the
        // swappable bucket.
        let mut a_l = listener("0.0.0.0:8443", vec![backend("10.0.0.1:80", 1)]);
        a_l.protocol = "h1s".to_owned();
        a_l.tls = Some(crate::TlsConfig {
            cert_path: "/a.crt".to_owned(),
            key_path: "/a.key".to_owned(),
            ticket_rotation_interval_seconds: 86_400,
            ticket_rotation_overlap_seconds: 3_600,
        });
        a_l.drain_timeout_ms = Some(5_000);
        let mut b_l = a_l.clone();
        b_l.tls = Some(crate::TlsConfig {
            cert_path: "/b.crt".to_owned(),
            key_path: "/b.key".to_owned(),
            ticket_rotation_interval_seconds: 86_400,
            ticket_rotation_overlap_seconds: 3_600,
        });
        b_l.drain_timeout_ms = Some(9_000);
        let plan = cfg(vec![a_l]).diff(&cfg(vec![b_l]));
        assert!(plan.swappable.is_empty(), "tls/drain must not be swappable");
        let fields: Vec<_> = plan.restart_required.iter().map(|c| c.field()).collect();
        assert!(fields.contains(&"tls"));
        assert!(fields.contains(&"drain_timeout_ms"));
    }
}
