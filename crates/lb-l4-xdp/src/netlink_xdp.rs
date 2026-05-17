//! ROUND8-L4-12: real RTM_GETLINK XDP prog-id query.
//!
//! Closes the EBUSY-on-redeploy hazard (finding ROUND8-L4-12, primary
//! production failure mode A): on a redeploy onto an interface that
//! still has our previous XDP program attached, a plain attach fails
//! `EBUSY`. To replace atomically (or to *verify* a detach actually
//! removed the program) the loader must know the kernel-visible
//! `prog_id` currently bound to the interface's `IFLA_XDP` attribute.
//!
//! aya 0.13.1 exposes no public `bpf_xdp_query` wrapper (same API
//! blocker family as ROUND8-L4-05). Rather than ship a `prog_id: None`
//! stub, we issue the query ourselves over a raw `AF_NETLINK` /
//! `NETLINK_ROUTE` socket — exactly what `ip link show` /
//! `bpftool net` do under the hood. The only dependency is `libc`,
//! which is already a workspace dependency (no new dep).
//!
//! Wire format (kernel UAPI, stable since Linux 4.13 when `IFLA_XDP`
//! landed):
//!
//! ```text
//! RTM_GETLINK request:
//!   nlmsghdr { len, type=RTM_GETLINK, flags=REQUEST, seq, pid=0 }
//!   ifinfomsg { ifi_family=AF_UNSPEC, ifi_index=<target> }
//!
//! RTM_NEWLINK reply:
//!   nlmsghdr { type=RTM_NEWLINK, ... }
//!   ifinfomsg { ifi_index, ... }
//!   rtattr*  ...
//!     rtattr IFLA_XDP (43) — NESTED container
//!       rtattr IFLA_XDP_PROG_ID (4) -> u32   (only present when a
//!                                             prog is attached)
//!       rtattr IFLA_XDP_ATTACHED (2) -> u8   (XDP mode)
//! ```
//!
//! [`parse_getlink_response`] / [`parse_ifinfo_payload`] are pure,
//! allocation-free, panic-free (no slice indexing — every read goes
//! through `.get()`), so the byte-parse proof in
//! `tests/round8_netlink_xdp_query.rs` exercises them against a
//! real-shaped blob with no `CAP_NET_ADMIN`. [`query_xdp_prog_id`] is
//! the live production caller.

#![cfg(target_os = "linux")]
#![allow(unsafe_code)]

use std::io;
use std::mem::size_of;

// ---- kernel UAPI constants (stable; libc only exposes a subset) ----

/// `IFLA_XDP` outer attribute type (kernel `if_link.h`).
const IFLA_XDP: u16 = 43;
/// Nested `IFLA_XDP_PROG_ID` — u32 kernel `bpf_prog_info.id`.
const IFLA_XDP_PROG_ID: u16 = 4;
/// Nested `IFLA_XDP_ATTACHED` — u8 attach mode (XDP_ATTACHED_*).
const IFLA_XDP_ATTACHED: u16 = 2;

const NLMSG_ALIGNTO: usize = 4;
const RTA_ALIGNTO: usize = 4;
const NLMSGHDR_LEN: usize = 16;
const IFINFOMSG_LEN: usize = 16;
const RTATTR_HDR_LEN: usize = 4;

/// `XDP_ATTACHED_NONE` (kernel `if_link.h`): `IFLA_XDP_ATTACHED == 0`
/// means no program attached.
pub const XDP_ATTACHED_NONE: u8 = 0;

#[inline]
const fn align(len: usize, to: usize) -> usize {
    (len + to - 1) & !(to - 1)
}

/// Slice-safe little helpers — every multibyte read goes through
/// `.get()` so a hostile / truncated kernel buffer can never panic
/// (crate denies `clippy::indexing_slicing`).
#[inline]
fn read_u16(buf: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    let s = buf.get(at..end)?;
    Some(u16::from_ne_bytes([*s.first()?, *s.get(1)?]))
}

#[inline]
fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let s = buf.get(at..end)?;
    Some(u32::from_ne_bytes([
        *s.first()?,
        *s.get(1)?,
        *s.get(2)?,
        *s.get(3)?,
    ]))
}

#[inline]
fn read_i32(buf: &[u8], at: usize) -> Option<i32> {
    read_u32(buf, at).map(|v| v as i32)
}

/// One decoded XDP attachment fact for an interface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XdpLinkInfo {
    /// Kernel `bpf_prog_info.id` of the attached program, if any.
    /// `None` means the `IFLA_XDP` nested block had no
    /// `IFLA_XDP_PROG_ID` (i.e. nothing attached).
    pub prog_id: Option<u32>,
    /// Raw `IFLA_XDP_ATTACHED` mode byte (`XDP_ATTACHED_*`), if the
    /// kernel reported it.
    pub attached_mode: Option<u8>,
}

/// Parse a `RTM_NEWLINK` reply body (everything after — and excluding
/// — the leading 16-byte `nlmsghdr`) and pull out the `IFLA_XDP`
/// nested block.
///
/// `payload` starts at the `ifinfomsg`. Returns
/// `XdpLinkInfo::default()` (prog_id `None`) when no XDP program is
/// attached — the success signal `detach_verifying` needs. Pure,
/// allocation-free, panic-free.
#[must_use]
pub fn parse_ifinfo_payload(payload: &[u8]) -> XdpLinkInfo {
    let mut out = XdpLinkInfo::default();
    // ifinfomsg is 16 bytes; attributes follow, NLMSG-aligned.
    let attr_start = align(IFINFOMSG_LEN, NLMSG_ALIGNTO);
    let Some(attrs) = payload.get(attr_start..) else {
        return out;
    };
    for (atype, adata) in RtattrIter::new(attrs) {
        if atype != IFLA_XDP {
            continue;
        }
        // Nested container: walk its inner rtattrs.
        for (inner_type, inner_data) in RtattrIter::new(adata) {
            match inner_type {
                IFLA_XDP_PROG_ID => {
                    if let Some(id) = read_u32(inner_data, 0) {
                        out.prog_id = Some(id);
                    }
                }
                IFLA_XDP_ATTACHED => {
                    if let Some(b) = inner_data.first() {
                        out.attached_mode = Some(*b);
                    }
                }
                _ => {}
            }
        }
    }
    // The kernel never hands out prog_id 0; treat 0 as "none" so
    // attach_replacing does not think a foreign prog 0 owns the iface.
    if out.prog_id == Some(0) {
        out.prog_id = None;
    }
    out
}

/// Parse a full netlink datagram (one or more `nlmsghdr`-prefixed
/// messages — e.g. the raw `recv()` buffer) and return the XDP link
/// info from the first `RTM_NEWLINK`. `NLMSG_ERROR` with a non-zero
/// errno becomes `Err`; `NLMSG_DONE` / unrelated types are skipped.
///
/// # Errors
///
/// - [`io::ErrorKind::InvalidData`] if a message is truncated /
///   internally inconsistent.
/// - An OS error if the kernel returned `NLMSG_ERROR` with errno.
pub fn parse_getlink_response(buf: &[u8]) -> io::Result<XdpLinkInfo> {
    const NLMSG_ERROR: u16 = 0x2;
    const NLMSG_DONE: u16 = 0x3;
    const RTM_NEWLINK: u16 = 16;

    let mut off = 0usize;
    while off
        .checked_add(NLMSGHDR_LEN)
        .is_some_and(|e| e <= buf.len())
    {
        let nlmsg_len = read_u32(buf, off)
            .map(|v| v as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short nlmsghdr len"))?;
        let type_off = off.checked_add(4).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "nlmsghdr offset overflow")
        })?;
        let nlmsg_type = read_u16(buf, type_off)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short nlmsghdr type"))?;

        let msg_end = off.checked_add(nlmsg_len);
        if nlmsg_len < NLMSGHDR_LEN || msg_end.is_none_or(|e| e > buf.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "truncated netlink message: len={nlmsg_len} off={off} buf={}",
                    buf.len()
                ),
            ));
        }
        let body_start = off
            .checked_add(NLMSGHDR_LEN)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "body offset overflow"))?;
        // msg_end is Some here (checked above).
        let body_end = match msg_end {
            Some(e) => e,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "message end overflow",
                ));
            }
        };
        let body = buf
            .get(body_start..body_end)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "body slice OOB"))?;

        match nlmsg_type {
            NLMSG_DONE => break,
            NLMSG_ERROR => {
                // First 4 bytes of the body are a signed errno (0 = ACK).
                if let Some(errno) = read_i32(body, 0) {
                    if errno != 0 {
                        return Err(io::Error::from_raw_os_error(
                            errno.checked_neg().unwrap_or(errno),
                        ));
                    }
                }
            }
            RTM_NEWLINK => return Ok(parse_ifinfo_payload(body)),
            _ => {}
        }

        let step = align(nlmsg_len, NLMSG_ALIGNTO);
        match off.checked_add(step) {
            Some(n) if n > off => off = n,
            // Zero / overflowing step would loop forever — stop.
            _ => break,
        }
    }
    Ok(XdpLinkInfo::default())
}

/// Minimal `rtattr` TLV iterator. Each attribute is
/// `rta_len: u16, rta_type: u16` then `rta_len - 4` payload bytes,
/// padded up to `RTA_ALIGNTO`. Malformed entries terminate iteration
/// (never panic, never infinite-loop).
struct RtattrIter<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> RtattrIter<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
}

impl<'a> Iterator for RtattrIter<'a> {
    type Item = (u16, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let rta_len = read_u16(self.buf, self.pos)? as usize;
        let type_at = self.pos.checked_add(2)?;
        let rta_type = read_u16(self.buf, type_at)?;
        // rta_len includes the 4-byte header; < 4 or running past the
        // buffer is malformed -> stop.
        if rta_len < RTATTR_HDR_LEN {
            return None;
        }
        let data_start = self.pos.checked_add(RTATTR_HDR_LEN)?;
        let data_end = self.pos.checked_add(rta_len)?;
        let data = self.buf.get(data_start..data_end)?;
        let step = align(rta_len, RTA_ALIGNTO);
        let next_pos = self.pos.checked_add(step)?;
        // A non-advancing position would loop forever.
        if next_pos <= self.pos {
            return None;
        }
        self.pos = next_pos;
        Some((rta_type, data))
    }
}

/// Live query: open an `AF_NETLINK`/`NETLINK_ROUTE` socket, send an
/// `RTM_GETLINK` for `iface`, and return the attached XDP `prog_id`
/// (or `None`). A *read* needs no `CAP_NET_ADMIN`.
///
/// # Errors
///
/// - [`io::Error`] for any socket / send / recv failure or if the
///   interface name does not resolve to an ifindex.
pub fn query_xdp_prog_id(iface: &str) -> io::Result<Option<u32>> {
    let ifindex = if_nametoindex(iface)?;
    let buf = rtm_getlink_roundtrip(ifindex)?;
    Ok(parse_getlink_response(&buf)?.prog_id)
}

/// `if_nametoindex(3)` thin wrapper returning an `io::Result`.
fn if_nametoindex(iface: &str) -> io::Result<u32> {
    let c = std::ffi::CString::new(iface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "iface has NUL"))?;
    // SAFETY: `c` is a valid NUL-terminated C string for the call's
    // duration; if_nametoindex reads it and returns 0 on failure.
    let idx = unsafe { libc::if_nametoindex(c.as_ptr()) };
    if idx == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no such interface: {iface}"),
        ));
    }
    Ok(idx)
}

/// RAII fd guard so every early return closes the socket.
struct OwnedFd(libc::c_int);
impl Drop for OwnedFd {
    fn drop(&mut self) {
        // SAFETY: self.0 is an owned, still-open fd.
        unsafe { libc::close(self.0) };
    }
}

/// Send one `RTM_GETLINK` for `ifindex` and return the raw reply.
fn rtm_getlink_roundtrip(ifindex: u32) -> io::Result<Vec<u8>> {
    const RTM_GETLINK: u16 = 18;
    const NLM_F_REQUEST: u16 = 0x01;

    // SAFETY: socket(2) with constant args; fd checked below.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let _guard = OwnedFd(fd);

    // Request: nlmsghdr(16) + ifinfomsg(16). Built field-by-field
    // into a fixed buffer via slice-safe writes.
    let total = NLMSGHDR_LEN + IFINFOMSG_LEN;
    let mut req = vec![0u8; total];
    write_at(&mut req, 0, &(total as u32).to_ne_bytes())?;
    write_at(&mut req, 4, &RTM_GETLINK.to_ne_bytes())?;
    write_at(&mut req, 6, &NLM_F_REQUEST.to_ne_bytes())?;
    write_at(&mut req, 8, &1u32.to_ne_bytes())?; // seq
    write_at(&mut req, 12, &0u32.to_ne_bytes())?; // pid (kernel fills)
    // ifinfomsg: family(1) pad(1) type(2) index(4) flags(4) change(4)
    write_at(&mut req, NLMSGHDR_LEN, &[libc::AF_UNSPEC as u8])?;
    write_at(&mut req, NLMSGHDR_LEN + 4, &ifindex.to_ne_bytes())?;

    let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;

    // SAFETY: req is a valid buffer of req.len() bytes; sa is a
    // zeroed-then-initialised sockaddr_nl of the right size.
    let sent = unsafe {
        libc::sendto(
            fd,
            req.as_ptr().cast(),
            req.len(),
            0,
            std::ptr::addr_of!(sa).cast(),
            size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }

    // 32 KiB is the conventional netlink buffer and more than enough
    // for one link's attributes.
    let mut reply = vec![0u8; 32 * 1024];
    // SAFETY: reply is a valid writable buffer of reply.len() bytes.
    let n = unsafe { libc::recv(fd, reply.as_mut_ptr().cast(), reply.len(), 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let got = usize::try_from(n).unwrap_or(0);
    reply.truncate(got);
    Ok(reply)
}

/// Slice-safe copy of `src` into `buf[at..at+src.len()]`.
fn write_at(buf: &mut [u8], at: usize, src: &[u8]) -> io::Result<()> {
    let end = at
        .checked_add(src.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write overflow"))?;
    let dst = buf
        .get_mut(at..end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write OOB"))?;
    dst.copy_from_slice(src);
    Ok(())
}
