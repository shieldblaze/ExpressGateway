//! S15 A2-7 — linkage probe binary for the
//! `scripts/never_decrypted_proof.sh` LINKAGE gate.
//!
//! `cargo bloat` requires a binary / dylib / cdylib target — it
//! cannot directly inspect an rlib. This example is the smallest
//! consumer of `lb_quic`'s public Mode A surface (the symbols the
//! gate is asserting are termination-free):
//!
//!   - `PassthroughListener::spawn` (the recv-loop entry point;
//!     transitively pulls FlowEntry / handle_inbound / Retry
//!     writer / Maglev pick / reverse pump).
//!   - `PassthroughParams::new` (the public constructor).
//!
//! Under `--no-default-features --features quic-passthrough-only`,
//! the compiled binary contains **no** `quic-terminate` module
//! tree (router.rs / conn_actor.rs / h3_bridge.rs / listener.rs)
//! — those are all `#[cfg(feature = "quic-terminate")]` in
//! `lb-quic/src/lib.rs`. The owner ruling §9.5 PRIMARY-1 LINKAGE
//! proof binds at the lb-quic boundary; this probe is the
//! cargo-bloat-shaped lever.
//!
//! The probe never runs — `main` constructs the params, prints a
//! sentinel, and exits. Tokio is needed for the type signature
//! (`PassthroughListener::spawn` is async) and is brought in via
//! the workspace pin.

use std::net::SocketAddr;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use lb_quic::{PassthroughListener, PassthroughParams};

fn main() {
    // Construct the params + show the params shape compiles. We
    // do NOT call `PassthroughListener::spawn` (it would bind a
    // UDP port and generate a retry secret on first run) — the
    // linker only needs the symbol reference for bloat to attribute
    // the transitive code to lb-quic.
    let bind: SocketAddr = "127.0.0.1:0".parse().expect("bind parse");
    let backend: SocketAddr = "127.0.0.1:1".parse().expect("backend parse");
    let secret = PathBuf::from("/dev/null");
    let params = PassthroughParams::new(bind, vec![backend], secret);

    // Take the function pointer so the linker pulls in
    // `PassthroughListener::spawn`'s function body and its
    // transitive closure (handle_inbound → handle_initial →
    // build_retry_packet, reverse_pump → parse_public_header, etc.).
    // `std::hint::black_box` defeats dead-code-elimination on the
    // function-pointer take without requiring a runtime call.
    let f = PassthroughListener::spawn;
    std::hint::black_box(f);
    // Reference the cancellation-token type from tokio_util via the
    // re-export path so the linker also pulls in the spawn argument
    // surface (CancellationToken::new is cheap and always-linked).
    let _tok = CancellationToken::new();

    println!("lb-quic passthrough linkage probe: params={params:?}");
}
