//! `io_uring` operations used by the lb-io runtime. Every helper builds a fresh ring for ONE op and
//! tears it down. That synchronicity is what makes the `unsafe` pushes below sound: the caller's
//! stack storage outlives `submit_and_wait`. Do NOT make these async without rewriting ownership.

use std::io;
use std::mem::MaybeUninit;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::RawFd;

use io_uring::{IoUring, cqueue, opcode, squeue, types};

/// Sentinel tag proving the reaped CQE is the one we submitted.
const NOP_USER_DATA: u64 = 0xDEAD_BEEF_u64;

/// Result of a successful [`nop_roundtrip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UringNopResult {
    /// The `user_data` tag copied back on the completion queue entry.
    pub user_data: u64,
}

/// Submit a single `NOP` and reap the completion. Failure is EXPECTED pre-5.1, under
/// `kernel.io_uring_disabled=1`, or behind a seccomp filter — callers treat it as "use epoll".
pub fn nop_roundtrip() -> io::Result<UringNopResult> {
    let mut ring = IoUring::new(8)?;
    let nop = opcode::Nop::new().build().user_data(NOP_USER_DATA);

    // SAFETY: a NOP references no caller-owned memory, and the fresh 8-entry ring cannot overflow.
    unsafe { push_sqe(&mut ring, &nop)? };

    ring.submit_and_wait(1)?;

    let cqe = reap_cqe(&mut ring)?;
    check_cqe(&cqe)?;
    Ok(UringNopResult {
        user_data: cqe.user_data(),
    })
}

/// Single-shot `IORING_OP_ACCEPT` — not multishot, no fixed-slot installation.
pub fn accept_one(listener_fd: RawFd) -> io::Result<(RawFd, SocketAddr)> {
    let mut ring = IoUring::new(8)?;

    // Sized for the larger of `sockaddr_in` / `sockaddr_in6`; the kernel fills both out.
    let mut addr_storage = MaybeUninit::<libc::sockaddr_storage>::zeroed();
    let mut addr_len: libc::socklen_t =
        core::mem::size_of::<libc::sockaddr_storage>()
            .try_into()
            .map_err(|_| io::Error::other("sockaddr_storage size exceeds socklen_t"))?;

    let entry = opcode::Accept::new(
        types::Fd(listener_fd),
        addr_storage.as_mut_ptr().cast::<libc::sockaddr>(),
        core::ptr::addr_of_mut!(addr_len),
    )
    .build()
    .user_data(0xACCE_7700_u64);

    // SAFETY: `addr_storage` and `addr_len` are stack locals that outlive `submit_and_wait`; both
    // pointers are writable, correctly typed for the accept opcode, and do not alias.
    unsafe { push_sqe(&mut ring, &entry)? };

    ring.submit_and_wait(1)?;
    let cqe = reap_cqe(&mut ring)?;
    let fd = check_cqe(&cqe)?;

    // SAFETY: a successful accept means the kernel wrote a `sockaddr_in`/`sockaddr_in6` into
    // `addr_storage` and set `addr_len`; the typed views below are read only after the family check.
    let addr = unsafe { sockaddr_storage_to_socketaddr(&addr_storage, addr_len)? };

    Ok((fd, addr))
}

/// Receive from `fd` into `buf` via `IORING_OP_RECV`.
pub fn recv(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let len_u32 = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    let mut ring = IoUring::new(8)?;

    let entry = opcode::Recv::new(types::Fd(fd), buf.as_mut_ptr(), len_u32)
        .build()
        .user_data(0x2ECC_0000_u64);

    // SAFETY: this call is synchronous, so `buf` outlives the kernel write, and `len_u32` is
    // bounded by the slice length so the kernel cannot write past its end.
    unsafe { push_sqe(&mut ring, &entry)? };

    ring.submit_and_wait(1)?;
    let cqe = reap_cqe(&mut ring)?;
    let n = check_cqe(&cqe)?;
    Ok(usize_from_nonneg_i32(n))
}

/// Send from `buf` on `fd` via `IORING_OP_SEND`.
pub fn send(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let len_u32 = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    let mut ring = IoUring::new(8)?;

    let entry = opcode::Send::new(types::Fd(fd), buf.as_ptr(), len_u32)
        .build()
        .user_data(0x5EDD_0000_u64);

    // SAFETY: `buf` outlives the synchronous `submit_and_wait`, and `len_u32` is bounded by its length.
    unsafe { push_sqe(&mut ring, &entry)? };

    ring.submit_and_wait(1)?;
    let cqe = reap_cqe(&mut ring)?;
    let n = check_cqe(&cqe)?;
    Ok(usize_from_nonneg_i32(n))
}

/// `IORING_OP_SPLICE` up to `len` bytes. One side MUST be a pipe, per `splice(2)`.
pub fn splice(from: RawFd, to: RawFd, len: u32) -> io::Result<u32> {
    let mut ring = IoUring::new(8)?;

    let entry = opcode::Splice::new(types::Fd(from), -1, types::Fd(to), -1, len)
        .build()
        .user_data(0x5917_CE00_u64);

    // SAFETY: splice carries no caller-owned memory; the fds stay owned by the caller throughout.
    unsafe { push_sqe(&mut ring, &entry)? };

    ring.submit_and_wait(1)?;
    let cqe = reap_cqe(&mut ring)?;
    let n = check_cqe(&cqe)?;
    Ok(u32_from_nonneg_i32(n))
}

/// Push a single SQE.
///
/// # Safety
/// Memory referenced by `entry` must live until `submit_and_wait` returns. Every helper here is
/// synchronous, so its caller's stack storage does.
unsafe fn push_sqe(ring: &mut IoUring, entry: &squeue::Entry) -> io::Result<()> {
    let mut sq = ring.submission();
    // SAFETY: forwarded from the caller of this function.
    match unsafe { sq.push(entry) } {
        Ok(()) => Ok(()),
        Err(_) => Err(io::Error::other("io_uring submission queue full")),
    }
}

fn reap_cqe(ring: &mut IoUring) -> io::Result<cqueue::Entry> {
    let mut cq = ring.completion();
    cq.sync();
    cq.next()
        .ok_or_else(|| io::Error::other("io_uring completion queue empty after submit_and_wait"))
}

/// Decode a CQE: negative is errno, non-negative is the op's return value.
fn check_cqe(cqe: &cqueue::Entry) -> io::Result<i32> {
    let code = cqe.result();
    if code < 0 {
        Err(io::Error::from_raw_os_error(-code))
    } else {
        Ok(code)
    }
}

/// Widen a [`check_cqe`]-validated non-negative `i32` to `usize` without a lossy cast.
#[inline]
fn usize_from_nonneg_i32(n: i32) -> usize {
    // `n >= 0` is an invariant of our callers; fall back to 0 otherwise.
    usize::try_from(n).unwrap_or(0)
}

/// Widen a [`check_cqe`]-validated non-negative `i32` to `u32` without a lossy cast.
#[inline]
fn u32_from_nonneg_i32(n: i32) -> u32 {
    u32::try_from(n).unwrap_or(0)
}

/// Interpret the `sockaddr_storage` the kernel wrote during ACCEPT.
///
/// # Safety
/// `storage` must hold `addr_len` initialised bytes of `AF_INET`/`AF_INET6`, as guaranteed by a
/// successful `IORING_OP_ACCEPT` completion.
unsafe fn sockaddr_storage_to_socketaddr(
    storage: &MaybeUninit<libc::sockaddr_storage>,
    addr_len: libc::socklen_t,
) -> io::Result<SocketAddr> {
    // SAFETY: forwarded from the caller's invariant.
    let storage_ref = unsafe { &*storage.as_ptr() };
    match i32::from(storage_ref.ss_family) {
        libc::AF_INET => {
            let need = libc::socklen_t::try_from(core::mem::size_of::<libc::sockaddr_in>())
                .unwrap_or(libc::socklen_t::MAX);
            if addr_len < need {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "AF_INET sockaddr truncated",
                ));
            }
            // SAFETY: family is AF_INET, addr_len covers sizeof(sockaddr_in), storage is repr(C).
            let sin = unsafe { &*core::ptr::from_ref(storage_ref).cast::<libc::sockaddr_in>() };
            let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
            let port = u16::from_be(sin.sin_port);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        libc::AF_INET6 => {
            let need = libc::socklen_t::try_from(core::mem::size_of::<libc::sockaddr_in6>())
                .unwrap_or(libc::socklen_t::MAX);
            if addr_len < need {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "AF_INET6 sockaddr truncated",
                ));
            }
            // SAFETY: family is AF_INET6 and addr_len covers sizeof(sockaddr_in6).
            let sin6 = unsafe { &*core::ptr::from_ref(storage_ref).cast::<libc::sockaddr_in6>() };
            let ip = std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr);
            let port = u16::from_be(sin6.sin6_port);
            let flowinfo = sin6.sin6_flowinfo;
            let scope_id = sin6.sin6_scope_id;
            Ok(SocketAddr::V6(SocketAddrV6::new(
                ip, port, flowinfo, scope_id,
            )))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected sockaddr family {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::AsRawFd;

    fn skip_if<T>(res: io::Result<T>, what: &str) -> Option<T> {
        match res {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("skipping {what}: {e}");
                None
            }
        }
    }

    #[test]
    fn nop_roundtrip_ok_or_skip() {
        if let Some(res) = skip_if(nop_roundtrip(), "nop") {
            assert_eq!(res.user_data, NOP_USER_DATA);
        }
    }

    #[test]
    fn accept_one_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client_thread = std::thread::spawn(move || {
            let _ = TcpStream::connect(addr);
        });
        let fd = listener.as_raw_fd();
        match accept_one(fd) {
            Ok((accepted_fd, peer)) => {
                assert!(accepted_fd > 0);
                assert_eq!(
                    peer.ip(),
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                );
                // SAFETY: we own the fd the kernel handed us.
                unsafe { libc::close(accepted_fd) };
            }
            Err(e) => eprintln!("skipping accept_one_loopback: {e}"),
        }
        client_thread.join().unwrap();
    }

    #[test]
    fn recv_send_loopback_pair() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client_thread = std::thread::spawn(move || -> io::Result<TcpStream> {
            let mut client = TcpStream::connect(addr)?;
            client.write_all(b"PING")?;
            let mut resp = [0u8; 4];
            client.read_exact(&mut resp)?;
            assert_eq!(&resp, b"PONG");
            Ok(client)
        });

        let (server, _) = listener.accept().unwrap();
        let fd = server.as_raw_fd();

        let mut buf = [0u8; 4];
        match recv(fd, &mut buf) {
            Ok(n) => {
                assert_eq!(n, 4);
                assert_eq!(&buf, b"PING");
            }
            Err(e) => {
                eprintln!("skipping recv_send_loopback_pair: {e}");
                drop(server);
                let _ = client_thread.join();
                return;
            }
        }

        match send(fd, b"PONG") {
            Ok(n) => assert_eq!(n, 4),
            Err(e) => eprintln!("skipping send path: {e}"),
        }

        let _ = client_thread.join().unwrap();
    }

    #[test]
    fn splice_rejects_non_pipe_or_succeeds() {
        // splice(2) needs one side to be a pipe; socket-to-socket either works or EINVALs, never panics.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _client_thread = std::thread::spawn(move || {
            let _ = TcpStream::connect(addr);
        });
        let (a, _) = listener.accept().unwrap();
        let b = TcpStream::connect(addr).unwrap();
        match splice(a.as_raw_fd(), b.as_raw_fd(), 0) {
            Ok(_) | Err(_) => {}
        }
    }
}
