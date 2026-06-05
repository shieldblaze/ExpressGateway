# S33 Phase 2 — breaking-API group: exact adaptation plan (lead pre-scope)

All 4 verified read-only against registry source (rand 0.10.1, socket2 0.6.3 downloaded) +
docs.rs (rcgen 0.14.8). Behavior-preserving, documented renames only (R5). One attributable
commit per crate. Per-increment: targeted `cargo test -p <affected>` for attribution; full
`--workspace --all-features --no-fail-fast` ×3 binding gate at Phase-2 END (verifier).

Order (lowest-risk first): socket2 → toml → rand → rcgen.

## 1. socket2 0.5 → 0.6.3  (near-no-op, verify)
- Spec: root `Cargo.toml` `socket2 = { version = "0.5", features = ["all"] }` → `version = "0.6"`.
- Code: NONE expected. Our surface = `SockRef`, `set_reuse_address`, `set_reuse_port`,
  `set_send_buffer_size` (lb-io/src/sockopts.rs, lb/src/main.rs:5013) — all present unchanged in
  0.6.3 (`socket.rs:1125/1152`, `sys/unix.rs:2202`, `sockref.rs:61`). The 0.6 v4/v6 method
  renames are for multicast/TTL methods we don't use.
- Confirm: `cargo check -p lb-io -p lb`. If any break → read + adapt documented; if >mechanical → drop.

## 2. toml 0.8 → 1.1.2  (near-no-op, verify)
- Spec: root `Cargo.toml` `toml = "0.8"` → `toml = "1"`.
- Code: NONE expected. Surface = `toml::from_str` (lb-config/src/lib.rs:1222,1874),
  `toml::de::Error` (lib.rs:25 `#[from]`), `.parse::<toml::Value>()` (lb-controlplane:234) — stable
  across 1.x. NOTE toml 1.0 may need a `serde` feature kept (check the spec keeps default feats).
- Confirm: `cargo check -p lb-config -p lb-controlplane`. If `Value`/`de::Error` shape changed →
  adapt documented; if >mechanical → drop.

## 3. rand 0.8 → 0.10.1  (mechanical renames)
- Spec: root `Cargo.toml` `rand = "0.8"` → `rand = "0.10"` (keep default features → `thread_rng`
  feature on → `rand::rng()` available; `Rng`/`SeedableRng`/`StdRng`/`ThreadRng`/`seed_from_u64`
  all retained, re-exported from rand_core).
- Code renames (verified vs rand-0.10.1 source — `gen_range` REMOVED, `thread_rng` removed):
  - `rand::thread_rng()` → `rand::rng()`
  - `.gen_range(` → `.random_range(`
  Sites:
  - crates/lb-balancer/src/weighted_random.rs:32 (`gen_range`), :50 (`thread_rng`)
  - crates/lb-balancer/src/p2c.rs:34,35 (`gen_range`), :62 (`thread_rng`)
  - crates/lb-balancer/src/random.rs:25 (`gen_range`), :33 (`thread_rng`)
  - crates/lb-io/src/pool.rs:799,802,811 (`gen_range`, test)
  - crates/lb/src/main.rs:2795 (`thread_rng`), :2797 (`gen_range`), :3277 (`thread_rng().gen_range`)
  - LEAVE `ring::rand::*` (quic_pool.rs) — that is RING, not the rand crate.
- Note: `random_range` supports RangeInclusive + signed (main.rs:2797 `-jitter..=jitter`) — fine.
- Confirm: `cargo test -p lb-balancer -p lb-io` (LB selectors use seeded StdRng → deterministic).

## 4. rcgen 0.13 → 0.14.8  (one field rename × 9)
- Specs (6 Cargo.toml, all `rcgen = { version = "0.13", features = ["pem"] }` → `"0.14"`):
  - Cargo.toml:188 (root dev-dep), crates/lb-soak/Cargo.toml:45, crates/lb-quic/Cargo.toml:75,
    crates/lb-security/Cargo.toml:49, crates/lb/Cargo.toml:76, crates/lb-l7/Cargo.toml:82.
  - Keep `features = ["pem"]` + default features (0.14 defaults include `crypto`+`aws_lc_rs` →
    `KeyPair::generate()` + `KeyPair: SigningKey` available).
- Code: `CertifiedKey` field `key_pair` → renamed **`signing_key`** in 0.14 (struct now generic
  `CertifiedKey<S: SigningKey>`, fields `cert` + `signing_key`). Rename `generated.key_pair` →
  `generated.signing_key` at these 9 sites (ALL are `generate_simple_self_signed(...)` results):
  - crates/lb-security/src/ticket.rs:743, 797, 841
  - crates/lb-security/src/handshake.rs:133
  - crates/lb-l7/tests/sni_authority_421.rs:46
  - crates/lb-quic/tests/round8_h3_authority_enforced.rs:60
  - crates/lb-security/tests/timeout_accept.rs:50
  - crates/lb-quic/tests/h3_graceful_close.rs:69
  - crates/lb-security/tests/tls_versions.rs:34
- UNCHANGED (verified): `params.self_signed(&key_pair)` (KeyPair: SigningKey + blanket `&S` impl),
  `KeyPair::generate()`, `KeyPair::serialize_pem/der`, `Certificate::pem/der`,
  `CertificateParams::new`, `is_ca`, `extended_key_usages`. The 30 `let key_pair =
  KeyPair::generate()` locals stay (self_signed(&key_pair) still compiles).
- Confirm: `cargo test -p lb-security -p lb-quic -p lb-l7`.

## Drop rule (R6): any crate whose adaptation proves >mechanical (real behavior change) → pin its
spec back, keep the rest, document. socket2/toml are the dual-version-or-conflict watch (check the
held surface didn't move, like prometheus). rand/rcgen are pure renames — low risk.
