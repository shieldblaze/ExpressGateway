//! Linkage probe binary for the `scripts/never_decrypted_proof.sh` LINKAGE gate: `cargo bloat`
//! needs a binary target and cannot inspect an rlib, so this is the smallest consumer of
//! `lb_quic`'s Mode A surface. Under `--no-default-features --features quic-passthrough-only` the
//! compiled binary must contain NO `quic-terminate` module tree — that is what the gate asserts.

use std::net::SocketAddr;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use lb_quic::{PassthroughListener, PassthroughParams};

fn main() {
    // `PassthroughListener::spawn` is deliberately NOT called (it would bind a UDP port and
    // generate a retry secret); the linker only needs the symbol reference.
    let bind: SocketAddr = "127.0.0.1:0".parse().expect("bind parse");
    let backend: SocketAddr = "127.0.0.1:1".parse().expect("backend parse");
    let secret = PathBuf::from("/dev/null");
    let params = PassthroughParams::new(bind, vec![backend], secret);

    // Taking the function pointer pulls in `spawn`'s body; `black_box` defeats DCE without a call.
    let f = PassthroughListener::spawn;
    std::hint::black_box(f);
    // Reference the cancellation-token type so the spawn argument surface is linked too.
    let _tok = CancellationToken::new();

    println!("lb-quic passthrough linkage probe: params={params:?}");
}
