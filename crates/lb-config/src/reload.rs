//! S37-C: config hot-reload diff/partition.
//!
//! [`LbConfig::diff`] compares a currently-applied config against a
//! freshly-loaded (already validated) one and partitions every change into
//! two buckets:
//!
//! * **swappable** — applied live via the per-listener `ArcSwap` snapshot
//!   (backends/weights, timeouts, limits, …) without rebinding ports or
//!   resetting in-flight connections.
//! * **restart-required** — structural / establishment-input changes
//!   (listener add/remove, bind address, protocol, XDP, QUIC transport
//!   params, …) that the running process CANNOT apply via a config swap.
//!
//! The HONESTY contract (S37-C non-negotiable): the diff DETECTS and
//! reports EVERY restart-required change so the reload routine can log a
//! clear per-field "requires restart, not applied" warning + bump a
//! metric. A restart-required change is NEVER silently dropped or
//! mistaken for a clean success.
//!
//! Listener identity is the **bind address** (`ListenerConfig.address`):
//! a listener present in old-but-not-new (or vice versa), or whose
//! `address` changed, is a structural (restart-required) change.

use crate::LbConfig;

/// A single swappable change detected by [`LbConfig::diff`]. Each variant
/// names the field + the listener it belongs to so the reload log is
/// precise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwappableChange {
    /// One or more of a listener's per-listener L7 fields changed and ALL
    /// of them are applied live by rebuilding the listener's `H1Proxy` /
    /// `H2Proxy` from the new config (the rebuild reads the WHOLE new
    /// listener config, so this set must EXACTLY match what the rebuild
    /// applies — the honesty invariant). `fields` lists the specific
    /// changed L7 fields for the operator log. Identified by bind address.
    ///
    /// In scope: `backends` (addresses/weights/protocols), `http`
    /// (header/body/total/head timeouts), `h2_security`, `websocket`
    /// (the H1/H2 knobs; the H3 `h3_extended_connect` sub-field only
    /// affects the QUIC datapath and is irrelevant on an L7 listener),
    /// `alt_svc`, `grpc`.
    ListenerL7 {
        /// Bind address of the affected listener.
        address: String,
        /// The specific changed L7 fields (for the operator log).
        fields: Vec<&'static str>,
    },
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
        }
    }

    /// Stable field label for the `{field}`-keyed reload metric.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::ListenerL7 { .. } => "listener.l7",
        }
    }

    /// Bind address this change targets.
    #[must_use]
    pub fn address(&self) -> &str {
        match self {
            Self::ListenerL7 { address, .. } => address,
        }
    }
}

/// A single restart-required change detected by [`LbConfig::diff`]. Each
/// variant carries enough context to name the field + scope in the
/// operator-facing "requires restart, not applied" warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartRequiredChange {
    /// A listener was added (present in the new config, absent in the
    /// old). Adding a listener binds a new port — not a config swap.
    ListenerAdded {
        /// Bind address of the added listener.
        address: String,
    },
    /// A listener was removed (present in old, absent in new). Removing a
    /// listener unbinds a port — not a config swap.
    ListenerRemoved {
        /// Bind address of the removed listener.
        address: String,
    },
    /// A listener's protocol changed — the entire datapath would change.
    ListenerProtocol {
        /// Bind address of the affected listener.
        address: String,
        /// Old protocol.
        old: String,
        /// New protocol.
        new: String,
    },
    /// A listener's non-swappable settings changed (TLS cert/key path,
    /// ALPN, QUIC transport params, drain budgets, the websocket H3
    /// extended-connect toggle, …). These are establishment-input /
    /// structural fields baked at spawn. Carried as one bucket per
    /// listener with a field tag so the warning is still per-field.
    ListenerField {
        /// Bind address of the affected listener.
        address: String,
        /// The specific non-swappable field that changed.
        field: &'static str,
    },
    /// A process-wide `[runtime]` field that is an establishment input
    /// (XDP attach) changed.
    RuntimeField {
        /// The specific non-swappable runtime field that changed.
        field: &'static str,
    },
    /// The `[observability]` (admin/metrics) bind changed.
    ObservabilityBind,
    /// The `[admin]` block (auth / bind policy) changed.
    AdminBlock,
    /// The `[passthrough]` (Mode A) block changed. Mode A is a
    /// flow-keyed datapath baked at spawn; treated restart-required for
    /// v1 (documented).
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

/// The result of [`LbConfig::diff`]: the full partition of every change
/// between two configs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadPlan {
    /// Changes that can be applied live via the per-listener `ArcSwap`.
    pub swappable: Vec<SwappableChange>,
    /// Changes that the running process cannot apply (logged + counted,
    /// never silently dropped).
    pub restart_required: Vec<RestartRequiredChange>,
}

impl ReloadPlan {
    /// `true` when the two configs are byte-for-byte equivalent (no
    /// change at all). Distinct from "only restart-required changes",
    /// which is NOT a no-op (it still produces honest warnings).
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
    /// Partition the change-set between `self` (currently applied) and
    /// `new` (freshly loaded + validated) into swappable vs
    /// restart-required (see module docs).
    ///
    /// Listener identity is the bind address. The diff is exhaustive:
    /// every field that differs lands in exactly one bucket, so a caller
    /// can rely on `restart_required` to be the COMPLETE set of changes
    /// it must warn about (HONESTY contract).
    #[must_use]
    pub fn diff(&self, new: &Self) -> ReloadPlan {
        let mut plan = ReloadPlan::default();

        // ── listeners (matched by bind address) ──
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

        // ── process-wide blocks ──
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
            // `[security].strict_te` feeds the shared HooksBundle — a
            // SWAPPABLE knob in the taxonomy. Increment 1 wires only the
            // backend-set swap; until the strict_te swap lands, surface
            // it honestly as a restart-required field so it is NEVER a
            // silent no-op. (Reclassified to swappable in a later
            // increment.)
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

    // Protocol change = whole datapath change (restart-required).
    if old.protocol != new.protocol {
        plan.restart_required
            .push(RestartRequiredChange::ListenerProtocol {
                address: addr.clone(),
                old: old.protocol.clone(),
                new: new.protocol.clone(),
            });
        // A protocol change subsumes every other field diff on this
        // listener — don't double-report.
        return;
    }

    // SWAPPABLE: the per-listener L7 fields the proxy rebuild applies
    // from the new config. These MUST exactly match what
    // `rebuild_l7_proxies` consumes from the new `ListenerConfig` (the
    // honesty invariant — the diff's "applied" set == the actually-
    // applied behaviour). The rebuild calls `build_h1_proxy` /
    // `build_h2_proxy` with `new.{backends, http, h2_security, websocket,
    // alt_svc, grpc}`, so a change to ANY of these is applied live.
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

    // RESTART-REQUIRED per-listener fields — establishment inputs or
    // values the proxy rebuild does NOT touch, so reporting them
    // restart-required is TRUTHFUL:
    //   * `tls`  — cert/key PATH + ALPN: the TLS bundle is owned by the
    //     listener mode + the SIGUSR1 cert-reload path, not rebuilt here.
    //   * `quic` — transport params + retry secret: QUIC datapath, baked
    //     into the config_factory at spawn (not an L7 proxy field).
    //   * `drain_timeout_ms` / `drain_jitter_ms` — read at SHUTDOWN from
    //     the boot config, never from the proxy, so a rebuild does not
    //     apply them.
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

/// Diff the `[runtime]` block. XDP attach fields are establishment
/// inputs (restart-required); the swappable runtime knobs
/// (`max_keepalive_requests`, limits, …) are surfaced honestly as
/// restart-required until their swap is wired in a later increment.
fn diff_runtime(
    old: Option<&crate::RuntimeConfig>,
    new: Option<&crate::RuntimeConfig>,
    plan: &mut ReloadPlan,
) {
    match (old, new) {
        (None, None) => {}
        (Some(o), Some(n)) if o == n => {}
        (None, Some(_)) | (Some(_), None) => {
            // The whole block appeared or disappeared — every field it
            // carries is affected; report it once as a runtime change.
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
            // Establishment inputs (PERMANENTLY restart-required).
            rt_field!(xdp_enabled, "xdp_enabled");
            rt_field!(xdp_interface, "xdp_interface");
            rt_field!(xdp_mode, "xdp_mode");
            rt_field!(
                xdp_new_flow_cap_per_sec_per_cpu,
                "xdp_new_flow_cap_per_sec_per_cpu"
            );
            rt_field!(tls, "runtime.tls");
            // Swappable-in-taxonomy knobs — surfaced honestly until
            // wired (later increments reclassify these to swappable).
            rt_field!(drain_timeout_ms, "drain_timeout_ms");
            rt_field!(readiness_settle_ms, "readiness_settle_ms");
            rt_field!(drain_jitter_ms, "drain_jitter_ms");
            rt_field!(handshake_timeout_ms, "handshake_timeout_ms");
            rt_field!(max_inflight_connections, "max_inflight_connections");
            rt_field!(connect_timeout_ms, "connect_timeout_ms");
            rt_field!(per_ip_connection_cap, "per_ip_connection_cap");
            rt_field!(watchdog, "watchdog");
            rt_field!(header_underscore_policy, "header_underscore_policy");
            rt_field!(max_keepalive_requests, "max_keepalive_requests");
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
        // Identity is the bind address, so changing it reads as one
        // listener removed + one added (both restart-required) — never
        // mistaken for a swappable change.
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
        // protocol change subsumes the backend change — exactly one
        // restart-required entry, no swappable.
        assert!(plan.swappable.is_empty());
        assert_eq!(plan.restart_required.len(), 1);
        assert_eq!(plan.restart_required[0].field(), "listener.protocol");
    }

    #[test]
    fn mixed_change_partitions_both_buckets() {
        // Listener A: backend swap (swappable). Listener B: added
        // (restart-required). Exhaustive partition.
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
        // The rebuild applies new.http, so an http-timeout change MUST be
        // classified swappable (the honesty invariant — the diff's
        // "applied" set must match what the rebuild actually applies).
        let mut a_l = listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]);
        a_l.http = Some(http(5_000));
        let mut b_l = listener("0.0.0.0:8080", vec![backend("10.0.0.1:80", 1)]);
        b_l.http = Some(http(9_000));
        let plan = cfg(vec![a_l]).diff(&cfg(vec![b_l]));
        assert_eq!(plan.swappable.len(), 1, "http change must be swappable");
        assert!(plan.restart_required.is_empty());
        let fields = l7_fields(&plan.swappable[0]);
        assert!(fields.contains(&"http"), "fields must name http: {fields:?}");
        assert!(!fields.contains(&"backends"), "backends did not change");
    }

    /// Extract the changed-field list from a `ListenerL7` swappable change
    /// (test helper — avoids `panic!` which the crate's lint set denies).
    fn l7_fields(change: &SwappableChange) -> Vec<&'static str> {
        match change {
            SwappableChange::ListenerL7 { fields, .. } => fields.clone(),
        }
    }

    #[test]
    fn combined_backend_and_http_change_is_one_swappable_with_both_fields() {
        // HONESTY: a reload changing backends AND an http timeout together
        // must report BOTH as swappable (the rebuild applies both) — never
        // "backend swapped, timeout silently applied while claimed
        // restart-required". One ListenerL7 entry listing both fields.
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
    fn tls_and_drain_changes_are_restart_required_not_swappable() {
        // tls (cert/key path + ALPN) and drain budgets are NOT applied by
        // the proxy rebuild, so reporting them restart-required is
        // truthful — they must NOT leak into the swappable bucket.
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
