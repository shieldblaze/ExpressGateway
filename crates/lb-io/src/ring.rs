//! Thin `io_uring` probe used by [`super::detect_backend`].
//!
//! Only a single `NOP` roundtrip is implemented; real ACCEPT / RECV / SEND /
//! SPLICE wiring is deferred to Pillar 1b. The goal here is to prove that
//! the kernel supports `io_uring` well enough to be selected as the live
//! backend. If anything fails at build-time or run-time we return an error
//! rather than panicking, and the caller falls back to epoll.

use std::io;

use io_uring::{IoUring, cqueue, opcode, squeue};

/// Sentinel value stamped into the submission queue entry so the probe
/// can confirm the CQE it receives is the one it submitted.
const NOP_USER_DATA: u64 = 0xDEAD_BEEF_u64;

/// Result of a successful [`nop_roundtrip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UringNopResult {
    /// The `user_data` tag copied back on the completion queue entry.
    pub user_data: u64,
}

/// Submit a single `NOP` operation and reap the completion.
///
/// Constructs a fresh `IoUring` with 8 entries, pushes one `NOP` SQE tagged
/// with [`NOP_USER_DATA`], submits and waits for the CQE, then tears the
/// ring down. Returns the observed `user_data`.
///
/// # Errors
/// Any failure during ring construction, SQE submission, or CQE reaping is
/// converted into an [`io::Error`]. Failure is expected on kernels older
/// than 5.1, on systems with `kernel.io_uring_disabled=1`, or under a
/// seccomp filter that rejects `io_uring_setup(2)`.
pub fn nop_roundtrip() -> io::Result<UringNopResult> {
    let mut ring = IoUring::new(8)?;

    let nop = opcode::Nop::new().build().user_data(NOP_USER_DATA);

    // SAFETY: `nop` lives on the stack until after `submit_and_wait`
    // returns. The SQE contains no pointers to caller-owned memory (a NOP
    // takes no inputs), so there are no lifetimes for the kernel to
    // outlive. Pushing into a just-created submission queue with room for
    // 8 entries cannot overflow.
    unsafe {
        push_sqe(&mut ring, &nop)?;
    }

    ring.submit_and_wait(1)?;

    let cqe = reap_cqe(&mut ring)?;
    let code = cqe.result();
    if code < 0 {
        return Err(io::Error::from_raw_os_error(-code));
    }

    Ok(UringNopResult {
        user_data: cqe.user_data(),
    })
}

/// Push a single SQE. Split into a tiny helper so the `unsafe` block has a
/// single, well-scoped responsibility.
///
/// # Safety
/// The caller must ensure that any memory referenced by `entry` lives at
/// least until `submit_and_wait` returns. For `NOP` entries this is
/// trivially satisfied — the opcode references nothing.
unsafe fn push_sqe(ring: &mut IoUring, entry: &squeue::Entry) -> io::Result<()> {
    let mut sq = ring.submission();
    // SAFETY: forwarded from the caller of this function. See the function
    // docs for the full invariant.
    match unsafe { sq.push(entry) } {
        Ok(()) => Ok(()),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::Other,
            "io_uring submission queue full during NOP probe",
        )),
    }
}

fn reap_cqe(ring: &mut IoUring) -> io::Result<cqueue::Entry> {
    let mut cq = ring.completion();
    cq.sync();
    cq.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Other,
            "io_uring completion queue empty after submit_and_wait",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nop_roundtrip_ok_or_skip() {
        match nop_roundtrip() {
            Ok(res) => assert_eq!(res.user_data, NOP_USER_DATA),
            Err(err) => {
                eprintln!("skipping: io_uring unavailable ({err})");
            }
        }
    }
}
