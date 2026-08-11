//! H3 → {H1, H2, H3} request/response bridge: relays a `quiche::h3`-decoded
//! request to the chosen upstream and streams the response back under the R8
//! bounded-channel backpressure gate.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::BoxBody;
use hyper::Request;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

/// Total request-body cap ⇒ H3 `413`. NOT the memory bound — that is the
/// bounded in-flight body channel (`H3_BODY_CHANNEL_DEPTH` × ≤8 KiB chunks).
// TODO(s3): wire into listener/actor config.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Total H1-response byte cap ⇒ clean H3 `502`. A DoS threshold, NOT a memory
/// bound: the response is streamed (see [`H3_RESP_CHANNEL_DEPTH`]).
// TODO(s3): make configurable.
pub const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Largest single `ReqBodyEvent::Chunk`. With `H3_BODY_CHANNEL_DEPTH` this
/// bounds in-flight bytes INDEPENDENT of total body size; larger DATA is split.
pub const H3_BODY_CHUNK_MAX: usize = 8 * 1024;

/// RFC 9114 §8.1 `H3_REQUEST_CANCELLED` — put on the **request** leg when the
/// H3→H3 connector aborts without FIN. Deliberately NOT the response leg's
/// [`crate::conn_actor::H3_INTERNAL_ERROR`]: here the proxy is the *client*, so
/// a client cancel really cancels; there it is the *server*, where this code
/// would misattribute a gateway failure. Not a consistency bug — do not "fix".
const H3_REQUEST_CANCELLED: u64 = 0x010c;

/// Cap on accumulated partial frame-header bytes: two QUIC varints of ≤ 8 bytes
/// each (RFC 9000 §16), so a well-formed header is ≤ 16 — larger ⇒ Reset.
pub const MAX_FRAME_HEADER_BYTES: usize = 16;

/// Cap on a body-phase trailing HEADERS QPACK block. Unlike DATA the block MUST
/// be whole-buffered to decode, so it needs an explicit ceiling.
pub const MAX_TRAILER_BLOCK_BYTES: usize = 64 * 1024;

/// Request-body event forwarded over the per-stream bounded body channel from
/// `conn_actor::poll_h3` to the egress task.
#[derive(Debug, Clone)]
pub enum ReqBodyEvent {
    /// A bounded request-body chunk.
    Chunk(Bytes),
    /// End of request body; `trailers` is the RFC 9114 §4.1 trailing section.
    End {
        /// Request trailers (post-DATA HEADERS frame); empty if none.
        trailers: Vec<(String, String)>,
    },
    /// Reset / abort before a clean end — the egress task MUST abort the
    /// upstream and fail the request.
    Reset,
}

/// Depth of the per-stream bounded RESPONSE channel back into the actor.
/// Retained memory = this × [`H3_RESP_CHUNK_MAX`], response-size independent.
pub const H3_RESP_CHANNEL_DEPTH: usize = 8;

/// Largest response-body slice per `RespEvent::Body`; larger reads are split so
/// in-flight memory stays bounded regardless of body size.
pub const H3_RESP_CHUNK_MAX: usize = 8 * 1024;

/// H3 frame-header ceiling (two varints, RFC 9000 §16), re-exported under a
/// response-side name so the memory gauge OVER-estimates, never under.
pub const H3_FRAME_HDR_MAX: usize = MAX_FRAME_HEADER_BYTES;

/// F-S7-6: the H3→H3 connector's NO-FORWARD-PROGRESS **idle** deadline — NOT a
/// wall-clock cap. Its fixed-5 s wall-clock predecessor truncated a valid,
/// actively-progressing 8 MiB response at ~4.37 MiB. Reset ONLY on bidirectional
/// application-data progress; NEVER by keepalive, timers, zero-byte reads or
/// backpressure parks (R-S76-5).
pub const H3_RESP_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// One unit of the bounded response pipe from an upstream reader task back to
/// the actor. DECODED, not wire bytes — the actor's `quiche::h3` owns framing.
/// Ordering: one [`Head`](Self::Head), then [`Body`](Self::Body) chunks, an
/// OPTIONAL [`Trailers`](Self::Trailers), then [`End`](Self::End); on ANY abort
/// a single [`Reset`](Self::Reset) and NEVER `End`.
#[derive(Debug, Clone)]
pub enum RespEvent {
    /// The response head. Emitted exactly once, before any `Body`.
    Head {
        /// Parsed response status code.
        status: u16,
        /// Decoded non-pseudo response headers (hop-by-hop stripped).
        headers: Vec<(String, String)>,
    },
    /// A decoded response-body chunk (≤ [`H3_RESP_CHUNK_MAX`], producer-split).
    Body(Bytes),
    /// The RFC 9114 §4.1 trailing field section, hop-by-hop stripped. Emitted
    /// only when non-empty, after the last `Body` and before `End`.
    Trailers(Vec<(String, String)>),
    /// All response events delivered — the actor FINs the client stream.
    End,
    /// Abort: the actor RESET_STREAMs the client (never FIN).
    Reset,
}

/// A DECODED upstream-H3 response event for an HTTP/1.1 or HTTP/2 *front*, so
/// an L7 front never re-decodes H3 frames it did not produce (wrong layer;
/// would re-introduce buffering in `lb-l7`). Same ordering contract as
/// [`RespEvent`], including a single `Reset` and NEVER `End` on any abort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H3RespEvent {
    /// The response head; pseudo-headers filtered. Emitted once, before `Body`.
    Head {
        /// Parsed `:status` pseudo-header.
        status: u16,
        /// Decoded non-pseudo headers (`content-length` kept as a regular one).
        headers: Vec<(String, String)>,
    },
    /// A decoded response-body chunk (≤ [`H3_RESP_CHUNK_MAX`]).
    Body(Bytes),
    /// The RFC 9114 §4.1 trailing field section, pseudo-headers filtered.
    Trailers(Vec<(String, String)>),
    /// Clean stream end — NEVER emitted on a partial / aborted response.
    End,
    /// Abort — the caller drops / RESETs its client and never finalizes.
    Reset,
}

/// Why a response producer aborted. EVERY variant emits [`RespEvent::Reset`]
/// and returns `Err`, so the actor RESET_STREAMs and NEVER FINs — a partial
/// body is never presentable as complete. The caller MUST mark the pooled
/// upstream NON-reusable on every variant (C2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespAbort {
    /// Upstream socket reset / read error mid-response.
    UpstreamReset,
    /// Socket EOF before the declared `Content-Length` was satisfied.
    PrematureEof,
    /// `Transfer-Encoding: chunked` decode error, or EOF before the terminator.
    ChunkedDecode,
    /// Total response exceeded the cap ([`MAX_RESPONSE_BODY_BYTES`]).
    OverCap,
    /// HEADERS parse failure, or head over the head cap before `CRLF CRLF`.
    BadHead,
    /// The response channel was closed by the actor (client cancelled).
    ClientGone,
}

/// Test gauge: peak per-stream request-body memory retained, counting the
/// buffers UPSTREAM of the chunk split — so it FAILS if the decoder buffers a
/// whole DATA frame, which [`MAX_INFLIGHT_BODY_BYTES`] cannot detect.
#[cfg(any(test, feature = "test-gauges"))]
pub static MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Max-update for [`MAX_RETAINED_BODY_BYTES`].
#[cfg(any(test, feature = "test-gauges"))]
pub fn record_retained(n: usize) {
    use std::sync::atomic::Ordering;
    let mut cur = MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    while n > cur {
        match MAX_RETAINED_BODY_BYTES.compare_exchange_weak(
            cur,
            n,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => cur = observed,
        }
    }
}

/// Test gauge: peak per-stream RESPONSE memory retained (an UPPER bound). A
/// whole-response buffering implementation would make this grow with response
/// size; the bounded channel keeps it response-size independent.
#[cfg(any(test, feature = "test-gauges"))]
pub static MAX_RETAINED_RESP_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Max-update for [`MAX_RETAINED_RESP_BYTES`].
#[cfg(any(test, feature = "test-gauges"))]
pub fn record_resp_retained(n: usize) {
    use std::sync::atomic::Ordering;
    let mut cur = MAX_RETAINED_RESP_BYTES.load(Ordering::Relaxed);
    while n > cur {
        match MAX_RETAINED_RESP_BYTES.compare_exchange_weak(
            cur,
            n,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => cur = observed,
        }
    }
}

/// Parsed H3 request headers.
#[derive(Debug, Clone)]
pub struct H3Request {
    /// `:method` pseudo-header.
    pub method: String,
    /// `:path` pseudo-header.
    pub path: String,
    /// `:authority` pseudo-header.
    pub authority: String,
    /// Non-pseudo headers. Not emitted on the H1 leg (which only sets `Host` +
    /// `Content-Length`), hence the `dead_code` allow.
    #[allow(dead_code)]
    pub extra: Vec<(String, String)>,
    /// RFC 9114 §4.1 trailing field section; empty when the request has none.
    pub trailers: Vec<(String, String)>,
}

impl Default for H3Request {
    /// Mirrors [`H3Request::from_headers`]'s missing-pseudo defaults so a
    /// defaulted value is wire-coherent rather than carrying empty pseudos.
    fn default() -> Self {
        Self {
            method: "GET".to_string(),
            path: "/".to_string(),
            authority: String::new(),
            extra: Vec::new(),
            trailers: Vec::new(),
        }
    }
}

impl H3Request {
    /// Extract pseudo-headers from a QPACK-decoded field list. Missing ones are
    /// DEFAULTED — deliberately NOT validation; see
    /// [`validate_request_pseudo_headers`], which runs first.
    #[must_use]
    pub fn from_headers(headers: Vec<(String, String)>) -> Self {
        let mut method = None;
        let mut path = None;
        let mut authority = None;
        let mut extra = Vec::new();
        for (name, value) in headers {
            match name.as_str() {
                ":method" => method = Some(value),
                ":path" => path = Some(value),
                ":authority" => authority = Some(value),
                ":scheme" => {
                    // Known-required but not actionable here.
                }
                _ => extra.push((name, value)),
            }
        }
        Self {
            method: method.unwrap_or_else(|| "GET".to_string()),
            path: path.unwrap_or_else(|| "/".to_string()),
            authority: authority.unwrap_or_default(),
            extra,
            // RFC 9114 §4.1: request trailers arrive in a SECOND HEADERS
            // frame after DATA, so they are never present at head-decode time.
            trailers: Vec::new(),
        }
    }
}

/// RFC 9114 §4.3 / §4.3.1 request pseudo-header validation (h3spec #12–15).
/// Returns `Err(reason)` on the FIRST violation; the caller resets the stream
/// with `H3_MESSAGE_ERROR` — a **stream** error (§4.1.3), so the connection
/// survives. Runs BEFORE [`H3Request::from_headers`] (which silently defaults
/// missing pseudo-headers) and before any upstream is dialled, so a malformed
/// request never reaches a backend. A missing `:authority`/`Host` on http/https
/// is STRICT (owner ruling). `ws_enabled` gates `:protocol`: when ON the request
/// is an RFC 8441/9220 Extended CONNECT and MUST carry `:scheme` + `:path` +
/// `:authority` — the OPPOSITE of a classic CONNECT; when OFF it is rejected as
/// unregistered (#14). quiche does not validate these, so this is the sole
/// authority.
///
/// # Errors
/// Returns a static reason string naming the RFC clause violated.
pub fn validate_request_pseudo_headers(
    headers: &[(String, String)],
    ws_enabled: bool,
) -> Result<(), &'static str> {
    let mut method: Option<&str> = None;
    let mut scheme: Option<&str> = None;
    let mut seen_path = false;
    let mut seen_authority = false;
    let mut seen_host = false;
    let mut seen_protocol = false;
    let mut seen_regular = false;

    for (name, value) in headers {
        if name.starts_with(':') {
            if seen_regular {
                return Err("h3 pseudo-header after regular field (RFC 9114 §4.3)");
            }
            match name.as_str() {
                ":method" => {
                    if method.is_some() {
                        return Err("h3 duplicate :method pseudo-header (RFC 9114 §4.3.1)");
                    }
                    method = Some(value);
                }
                ":scheme" => {
                    if scheme.is_some() {
                        return Err("h3 duplicate :scheme pseudo-header (RFC 9114 §4.3.1)");
                    }
                    scheme = Some(value);
                }
                ":path" => {
                    if seen_path {
                        return Err("h3 duplicate :path pseudo-header (RFC 9114 §4.3.1)");
                    }
                    seen_path = true;
                }
                ":authority" => {
                    if seen_authority {
                        return Err("h3 duplicate :authority pseudo-header (RFC 9114 §4.3.1)");
                    }
                    seen_authority = true;
                }
                // Registered ONLY under `ws_enabled`; otherwise it falls
                // through to the prohibited arm below (R3 byte-identical).
                ":protocol" if ws_enabled => {
                    if seen_protocol {
                        return Err("h3 duplicate :protocol pseudo-header (RFC 9114 §4.3.1)");
                    }
                    seen_protocol = true;
                }
                _ => {
                    return Err("h3 prohibited/unknown request pseudo-header (RFC 9114 §4.3)");
                }
            }
        } else {
            seen_regular = true;
            if name.eq_ignore_ascii_case("host") {
                seen_host = true;
            }
        }
    }

    // #13 — classic CONNECT omits :scheme/:path (§4.4); Extended CONNECT
    // INVERTS that (RFC 8441 §4).
    match method {
        None => Err("h3 missing mandatory :method pseudo-header (RFC 9114 §4.3.1)"),
        Some("CONNECT") if seen_protocol => {
            if scheme.is_none() {
                Err("h3 websocket extended CONNECT missing :scheme (RFC 8441 §4)")
            } else if !seen_path {
                Err("h3 websocket extended CONNECT missing :path (RFC 8441 §4)")
            } else if !seen_authority {
                Err("h3 websocket extended CONNECT missing :authority (RFC 8441 §4)")
            } else {
                Ok(())
            }
        }
        Some("CONNECT") => {
            if scheme.is_some() || seen_path {
                Err("h3 CONNECT request must omit :scheme/:path (RFC 9114 §4.4)")
            } else if !seen_authority {
                Err("h3 CONNECT request missing :authority (RFC 9114 §4.4)")
            } else {
                Ok(())
            }
        }
        Some(_) => {
            // Only reachable under `ws_enabled`; otherwise already rejected.
            if seen_protocol {
                return Err("h3 :protocol pseudo-header requires :method=CONNECT (RFC 8441 §4)");
            }
            let Some(scheme) = scheme else {
                return Err("h3 missing mandatory :scheme pseudo-header (RFC 9114 §4.3.1)");
            };
            if !seen_path {
                return Err("h3 missing mandatory :path pseudo-header (RFC 9114 §4.3.1)");
            }
            let mandatory_authority =
                scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http");
            if mandatory_authority && !seen_authority && !seen_host {
                return Err("h3 http/https request missing :authority or Host (RFC 9114 §4.3.1)");
            }
            Ok(())
        }
    }
}

fn find_header_sep(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_status_line(line: &str) -> Result<u16, String> {
    let mut parts = line.splitn(3, ' ');
    let _ver = parts.next().ok_or("no HTTP version")?;
    let code = parts.next().ok_or("no status code")?;
    code.parse::<u16>()
        .map_err(|e| format!("status parse {code:?}: {e}"))
}

/// Response-direction hop-by-hop field names a proxy MUST NOT forward. A
/// DELIBERATE duplicate of `lb_l7::h2_to_h1::RESPONSE_HOP_BY_HOP` — `lb-quic`
/// sits BELOW `lb-l7` and cannot depend on it (reverse layering); keep the two
/// in sync. Stripping is REQUIRED by RFC 9114 §4.2, not tidiness.
const RESPONSE_HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
];

/// `true` iff `name_lower` (an ALREADY-lowercased name) is response hop-by-hop.
fn is_response_hop_by_hop(name_lower: &str) -> bool {
    RESPONSE_HOP_BY_HOP.contains(&name_lower)
}

/// Response-body framing decided from the parsed upstream H1 response headers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RespFraming {
    /// `Content-Length: n` — stream exactly `n` body bytes.
    ContentLength(usize),
    /// `Transfer-Encoding: chunked` — incremental de-chunk.
    Chunked,
    /// No CL, no TE — body runs until socket EOF.
    Eof,
}

/// Incremental HTTP/1.1 chunked-transfer decoder for RESPONSES. EVERY malformed
/// input ⇒ [`RespAbort::ChunkedDecode`] — never a truncated or forwarded body
/// presented as complete (C3).
#[derive(Debug)]
struct ChunkDecoder {
    /// Bytes not yet consumed (a partial size line or chunk body straddling
    /// reads). Payload is drained immediately — never whole-chunk buffered.
    buf: Vec<u8>,
    /// `Some(remaining)` inside a chunk body; `None` awaiting the next size line.
    in_chunk: Option<usize>,
    /// Zero-size chunk seen — no more payload. The RFC 9112 §7.1.2 trailer
    /// section and final CRLF may still be pending; see [`Self::complete`].
    done: bool,
    /// Zero-size chunk + trailer section + terminating CRLF ALL consumed. The
    /// producer loop exits on THIS, not `done`, which would drop the trailers.
    complete: bool,
    /// Decoded RFC 9112 §7.1.2 trailer fields; taken once via
    /// [`Self::take_trailers`] for the post-DATA H3 trailing-HEADERS frame.
    trailers: Vec<(String, String)>,
    /// Trailer-section bytes read so far, hard-bounded by
    /// [`MAX_TRAILER_SECTION`] so a hostile trailer block cannot grow memory.
    trailer_buf: Vec<u8>,
}

/// Max bytes a chunk-size line may occupy before it is rejected as
/// malformed/hostile framing (smuggling guard, C3).
const MAX_CHUNK_SIZE_LINE: usize = 256;

/// Max bytes the RFC 9112 §7.1.2 chunked trailer section may occupy before it
/// is rejected ⇒ [`RespAbort::ChunkedDecode`] (hostile-trailer ceiling).
const MAX_TRAILER_SECTION: usize = 64 * 1024;

impl ChunkDecoder {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            in_chunk: None,
            done: false,
            complete: false,
            trailers: Vec::new(),
            trailer_buf: Vec::new(),
        }
    }

    /// Take the decoded trailer fields. Only meaningful once
    /// [`Self::complete`] is set.
    fn take_trailers(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.trailers)
    }

    /// Feed `input`, appending decoded payload to `out`. ANY malformed framing
    /// (including a malformed trailer section) ⇒ `Err(ChunkedDecode)`.
    fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), RespAbort> {
        self.buf.extend_from_slice(input);
        loop {
            if self.complete {
                return Ok(());
            }
            if self.done {
                // Only the trailer section is left. PC-2: consumes from
                // `self.buf`, so one coalesced with the zero-size line parses.
                return self.parse_trailer_section();
            }
            match self.in_chunk {
                Some(0) => {
                    // Expect the trailing CRLF after a chunk body.
                    let Some(lead) = self.buf.get(..2) else {
                        return Ok(());
                    };
                    if lead != b"\r\n" {
                        return Err(RespAbort::ChunkedDecode);
                    }
                    self.buf.drain(..2);
                    self.in_chunk = None;
                }
                Some(remaining) => {
                    if self.buf.is_empty() {
                        return Ok(());
                    }
                    let take = remaining.min(self.buf.len());
                    let Some(slice) = self.buf.get(..take) else {
                        return Err(RespAbort::ChunkedDecode);
                    };
                    out.extend_from_slice(slice);
                    self.buf.drain(..take);
                    self.in_chunk = Some(remaining - take);
                }
                None => {
                    let Some(nl) = self.buf.windows(2).position(|w| w == b"\r\n") else {
                        if self.buf.len() > MAX_CHUNK_SIZE_LINE {
                            return Err(RespAbort::ChunkedDecode);
                        }
                        return Ok(());
                    };
                    if nl > MAX_CHUNK_SIZE_LINE {
                        return Err(RespAbort::ChunkedDecode);
                    }
                    let Some(line) = self.buf.get(..nl) else {
                        return Err(RespAbort::ChunkedDecode);
                    };
                    // Chunk extension: the size is the hex before the first ';'.
                    let hex_end = line.iter().position(|&b| b == b';').unwrap_or(line.len());
                    let hex = std::str::from_utf8(line.get(..hex_end).unwrap_or(line))
                        .map_err(|_| RespAbort::ChunkedDecode)?
                        .trim();
                    if hex.is_empty() {
                        return Err(RespAbort::ChunkedDecode);
                    }
                    let size =
                        usize::from_str_radix(hex, 16).map_err(|_| RespAbort::ChunkedDecode)?;
                    self.buf.drain(..nl + 2);
                    if size == 0 {
                        // Zero-size terminator: do NOT return — loop so a
                        // trailer section coalesced into THIS read is consumed.
                        self.done = true;
                        continue;
                    }
                    self.in_chunk = Some(size);
                }
            }
        }
    }

    /// Parse the RFC 9112 §7.1.2 trailer section after the zero-size chunk. A
    /// section split across reads or coalesced with the zero-size line parses
    /// identically (PC-2). ANY malformed input — no `:`, a `:`-prefixed name
    /// (RFC 9114 §4.3), an empty name, or oversize — ⇒ `ChunkedDecode`.
    fn parse_trailer_section(&mut self) -> Result<(), RespAbort> {
        loop {
            if !self.buf.is_empty() {
                if self.trailer_buf.len() + self.buf.len() > MAX_TRAILER_SECTION {
                    return Err(RespAbort::ChunkedDecode);
                }
                self.trailer_buf.append(&mut self.buf);
            }
            let Some(nl) = self.trailer_buf.windows(2).position(|w| w == b"\r\n") else {
                // Bound the partial accumulation — an unterminated section
                // is hostile.
                if self.trailer_buf.len() > MAX_TRAILER_SECTION {
                    return Err(RespAbort::ChunkedDecode);
                }
                return Ok(());
            };
            if nl == 0 {
                self.trailer_buf.drain(..2);
                self.complete = true;
                return Ok(());
            }
            let line = self.trailer_buf.get(..nl).ok_or(RespAbort::ChunkedDecode)?;
            let line = std::str::from_utf8(line).map_err(|_| RespAbort::ChunkedDecode)?;
            // A trailer line MUST be `name: value`; no `:` is the C3 case.
            let (name, value) = line.split_once(':').ok_or(RespAbort::ChunkedDecode)?;
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(RespAbort::ChunkedDecode);
            }
            // RFC 9114 §4.3: a trailer section MUST NOT carry pseudo-headers.
            if name.starts_with(':') {
                return Err(RespAbort::ChunkedDecode);
            }
            self.trailers.push((name, value.trim().to_owned()));
            self.trailer_buf.drain(..nl + 2);
        }
    }
}

/// **Incremental, bounded, backpressured** H3 RESPONSE egress for an H1
/// upstream: read only to the head terminator (64 KiB cap ⇒
/// [`RespAbort::BadHead`]), emit [`RespEvent::Head`] **before any body byte**,
/// then stream the body as ≤ [`H3_RESP_CHUNK_MAX`] chunks as they arrive.
/// `tx.send(..).await` is the backpressure point: a stalled H3 client fills the
/// channel, this parks, and the upstream socket is not read.
///
/// EVERY failure emits [`RespEvent::Reset`] and returns `Err` — never a
/// truncated body as complete. The caller MUST mark the pooled upstream
/// NON-reusable on any `Err` (C2).
///
/// # Errors
///
/// Returns [`RespAbort`] naming the upstream / framing / cap / cancel cause.
pub async fn stream_h1_response(
    stream: &mut TcpStream,
    tx: &tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    /// Send a `RespEvent`, mapping a closed channel to `ClientGone` so the
    /// producer stops reading the upstream.
    macro_rules! send {
        ($tx:expr, $ev:expr) => {
            $tx.send($ev).await.map_err(|_| RespAbort::ClientGone)?
        };
    }

    const HEAD_CAP: usize = 64 * 1024;
    let mut head = Vec::with_capacity(1024);
    let mut rbuf = [0u8; 8 * 1024];
    let sep = loop {
        if let Some(p) = find_header_sep(&head) {
            break p;
        }
        if head.len() > HEAD_CAP {
            let _ = tx.send(RespEvent::Reset).await;
            return Err(RespAbort::BadHead);
        }
        let n = match stream.read(&mut rbuf).await {
            Ok(n) => n,
            Err(_) => {
                let _ = tx.send(RespEvent::Reset).await;
                return Err(RespAbort::UpstreamReset);
            }
        };
        if n == 0 {
            let _ = tx.send(RespEvent::Reset).await;
            return Err(RespAbort::BadHead);
        }
        head.extend_from_slice(rbuf.get(..n).unwrap_or(&rbuf));
    };
    // Bytes already read past the terminator are the first body bytes.
    let mut body_prefix = head.split_off(sep + 4);
    head.truncate(sep);

    let head_str = std::str::from_utf8(&head).map_err(|_| RespAbort::BadHead);
    let head_str = match head_str {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(RespEvent::Reset).await;
            return Err(e);
        }
    };
    let mut lines = head_str.split("\r\n");
    let status = match lines
        .next()
        .ok_or(RespAbort::BadHead)
        .and_then(|l| parse_status_line(l).map_err(|_| RespAbort::BadHead))
    {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(RespEvent::Reset).await;
            return Err(e);
        }
    };
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    // CF-H3-HEAD: collect the FULL non-hop-by-hop set. `content-length` is
    // re-added below from the ONE `framing` source; `transfer-encoding` is
    // hop-by-hop (de-chunked here, the H3 leg is FIN-delimited).
    let mut fwd_headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        if k == "content-length" {
            match v.trim().parse::<usize>() {
                Ok(n) => content_length = Some(n),
                Err(_) => {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::BadHead);
                }
            }
        } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if !is_response_hop_by_hop(&k) {
            fwd_headers.push((k, v.trim().to_string()));
        }
    }
    // Transfer-Encoding beats Content-Length (RFC 9112 §6.1); BOTH is smuggling.
    let framing = if chunked {
        if content_length.is_some() {
            let _ = tx.send(RespEvent::Reset).await;
            return Err(RespAbort::BadHead);
        }
        RespFraming::Chunked
    } else if let Some(n) = content_length {
        RespFraming::ContentLength(n)
    } else {
        RespFraming::Eof
    };

    // Re-add `content-length` ONLY for ContentLength framing; chunked/EOF are
    // FIN-delimited on the H3 leg (CF-H3-HEAD).
    if let RespFraming::ContentLength(n) = &framing {
        fwd_headers.push(("content-length".to_string(), n.to_string()));
    }
    // `cap`/`total` counts bytes — a DoS threshold, not a memory bound.
    let mut total: usize = 0;
    total = total.saturating_add(fwd_headers.iter().map(|(n, v)| n.len() + v.len()).sum());
    if total > cap {
        let _ = tx.send(RespEvent::Reset).await;
        return Err(RespAbort::OverCap);
    }
    send!(
        tx,
        RespEvent::Head {
            status,
            headers: fwd_headers.clone(),
        }
    );

    // Emit one ≤H3_RESP_CHUNK_MAX chunk; `cap`/`total` counts PAYLOAD bytes.
    macro_rules! emit_data {
        ($payload:expr) => {{
            for slice in $payload.chunks(H3_RESP_CHUNK_MAX) {
                total = total.saturating_add(slice.len());
                if total > cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                send!(tx, RespEvent::Body(Bytes::copy_from_slice(slice)));
            }
        }};
    }

    match framing {
        RespFraming::ContentLength(n) => {
            let mut remaining = n;
            if !body_prefix.is_empty() {
                if body_prefix.len() > remaining {
                    // More bytes than declared = framing violation.
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::ChunkedDecode);
                }
                remaining -= body_prefix.len();
                let p = std::mem::take(&mut body_prefix);
                emit_data!(&p);
            }
            while remaining > 0 {
                let want = remaining.min(rbuf.len());
                let dst = rbuf.get_mut(..want).ok_or(RespAbort::UpstreamReset)?;
                let nr = match stream.read(dst).await {
                    Ok(n) => n,
                    Err(_) => {
                        let _ = tx.send(RespEvent::Reset).await;
                        return Err(RespAbort::UpstreamReset);
                    }
                };
                if nr == 0 {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::PrematureEof);
                }
                remaining -= nr;
                let slice = rbuf.get(..nr).unwrap_or(&rbuf);
                emit_data!(slice);
            }
        }
        RespFraming::Chunked => {
            let mut dec = ChunkDecoder::new();
            let mut decoded = Vec::new();
            if !body_prefix.is_empty() {
                let p = std::mem::take(&mut body_prefix);
                if let Err(e) = dec.feed(&p, &mut decoded) {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(e);
                }
                if !decoded.is_empty() {
                    let d = std::mem::take(&mut decoded);
                    emit_data!(&d);
                }
            }
            // C4: loop until `complete` (terminator + trailer section + final
            // CRLF), NOT merely `done` — that would drop the trailer section.
            // EOF before `complete` ⇒ ChunkedDecode, never a truncated body.
            while !dec.complete {
                let nr = match stream.read(&mut rbuf).await {
                    Ok(n) => n,
                    Err(_) => {
                        let _ = tx.send(RespEvent::Reset).await;
                        return Err(RespAbort::UpstreamReset);
                    }
                };
                if nr == 0 {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::ChunkedDecode);
                }
                if let Err(e) = dec.feed(rbuf.get(..nr).unwrap_or(&rbuf), &mut decoded) {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(e);
                }
                if !decoded.is_empty() {
                    let d = std::mem::take(&mut decoded);
                    emit_data!(&d);
                }
            }
            // C4: the RFC 9112 §7.1.2 trailer section becomes an RFC 9114 §4.1
            // trailing section — after the last DATA, before `End`, never on abort.
            let trailers = dec.take_trailers();
            if !trailers.is_empty() {
                total = total.saturating_add(trailers.iter().map(|(n, v)| n.len() + v.len()).sum());
                if total > cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                send!(tx, RespEvent::Trailers(trailers));
            }
        }
        RespFraming::Eof => {
            if !body_prefix.is_empty() {
                let p = std::mem::take(&mut body_prefix);
                emit_data!(&p);
            }
            loop {
                let nr = match stream.read(&mut rbuf).await {
                    Ok(n) => n,
                    Err(_) => {
                        let _ = tx.send(RespEvent::Reset).await;
                        return Err(RespAbort::UpstreamReset);
                    }
                };
                if nr == 0 {
                    break; // EOF-delimited: clean end.
                }
                let slice = rbuf.get(..nr).unwrap_or(&rbuf);
                emit_data!(slice);
            }
        }
    }

    send!(tx, RespEvent::End);
    Ok(())
}

/// H3 RESPONSE egress for a hyper H2 upstream — the H2 cousin of
/// [`stream_h1_response`], IDENTICAL `RespEvent` / `RespAbort` contract. Pulls
/// the body ONE frame at a time via [`http_body_util::BodyExt::frame`]; the
/// bounded `tx.send(..).await` is the backpressure point (a stalled client stops
/// the actor draining, hyper stops issuing `WINDOW_UPDATE`s, the H2 send window
/// closes), so retained memory is body-size INDEPENDENT — never `.collect()`.
/// RFC 9110 §6.5 trailers become one post-DATA `Trailers` before `End`. Any
/// failure ⇒ [`RespEvent::Reset`] + `Err`, never a clean FIN.
///
/// # Errors
///
/// Returns `Err(RespAbort)` describing why the relay aborted.
pub async fn stream_h2_response(
    resp: hyper::Response<hyper::body::Incoming>,
    tx: &tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    macro_rules! send {
        ($tx:expr, $ev:expr) => {
            $tx.send($ev).await.map_err(|_| RespAbort::ClientGone)?
        };
    }

    let (parts, mut body) = resp.into_parts();

    // Forward `content-length` only when the upstream declared a valid one, so
    // the H3 client gets the same framing decision.
    let declared_len: Option<usize> = parts
        .headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<usize>().ok());
    // CF-H3-HEAD: forward the FULL non-hop-by-hop set; `content-length` is
    // re-added from the single `declared_len` source. `iter()` yields repeated
    // names (set-cookie) individually; a non-UTF-8 value is skipped.
    let mut fwd_headers: Vec<(String, String)> = Vec::with_capacity(parts.headers.len());
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n == "content-length" || is_response_hop_by_hop(n) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            fwd_headers.push((n.to_string(), v.to_string()));
        }
    }
    if let Some(n) = declared_len {
        fwd_headers.push(("content-length".to_string(), n.to_string()));
    }
    // `cap`/`total` counts decoded header bytes (DoS threshold, not memory).
    let mut total: usize = fwd_headers.iter().map(|(n, v)| n.len() + v.len()).sum();
    if total > cap {
        let _ = tx.send(RespEvent::Reset).await;
        return Err(RespAbort::OverCap);
    }
    send!(
        tx,
        RespEvent::Head {
            status: parts.status.as_u16(),
            headers: fwd_headers.clone(),
        }
    );

    // Emit one ≤H3_RESP_CHUNK_MAX chunk; `cap`/`total` counts PAYLOAD bytes.
    macro_rules! emit_data {
        ($payload:expr) => {{
            for slice in $payload.chunks(H3_RESP_CHUNK_MAX) {
                total = total.saturating_add(slice.len());
                if total > cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                send!(tx, RespEvent::Body(Bytes::copy_from_slice(slice)));
            }
        }};
    }

    while let Some(frame_res) = body.frame().await {
        let frame = match frame_res {
            Ok(f) => f,
            Err(_) => {
                // Upstream body error mid-response ⇒ Reset, never a clean FIN.
                let _ = tx.send(RespEvent::Reset).await;
                return Err(RespAbort::UpstreamReset);
            }
        };
        if let Some(data) = frame.data_ref() {
            let bytes: &[u8] = data;
            if !bytes.is_empty() {
                emit_data!(bytes);
            }
        } else if let Some(tmap) = frame.trailers_ref() {
            // RFC 9110 §6.5 trailers → one post-DATA trailing HEADERS frame.
            let trailers: Vec<(String, String)> = tmap
                .iter()
                .filter(|(n, _)| !n.as_str().starts_with(':'))
                .filter_map(|(n, v)| {
                    v.to_str()
                        .ok()
                        .map(|vs| (n.as_str().to_owned(), vs.to_owned()))
                })
                .collect();
            if !trailers.is_empty() {
                total = total.saturating_add(trailers.iter().map(|(n, v)| n.len() + v.len()).sum());
                if total > cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                send!(tx, RespEvent::Trailers(trailers));
            }
        }
        // Any other frame kind is ignored — never forwarded raw.
    }

    send!(tx, RespEvent::End);
    Ok(())
}

/// Build ONLY the HTTP/1.1 request head so the body can be streamed after it.
/// `framing` picks the entity-body header: `None` ⇒ `Content-Length: 0`,
/// `ContentLength(n)` ⇒ that length, `Chunked` ⇒ `Transfer-Encoding: chunked`.
fn build_h1_head(req: &H3Request, framing: &H1BodyFraming) -> Vec<u8> {
    let mut s = String::with_capacity(128);
    s.push_str(&req.method);
    s.push(' ');
    s.push_str(&req.path);
    s.push_str(" HTTP/1.1\r\n");
    if !req.authority.is_empty() {
        s.push_str("Host: ");
        s.push_str(&req.authority);
        s.push_str("\r\n");
    }
    match framing {
        H1BodyFraming::None => s.push_str("Content-Length: 0\r\n"),
        H1BodyFraming::ContentLength(n) => {
            s.push_str("Content-Length: ");
            s.push_str(&n.to_string());
            s.push_str("\r\n");
        }
        H1BodyFraming::Chunked => s.push_str("Transfer-Encoding: chunked\r\n"),
    }
    s.push_str("Connection: close\r\n");
    s.push_str("\r\n");
    s.into_bytes()
}

/// HTTP/1.1 request entity-body framing choice.
#[derive(Debug, Clone, PartialEq, Eq)]
enum H1BodyFraming {
    /// No body — `Content-Length: 0`.
    None,
    /// Client supplied `content-length`; forward raw bytes unframed.
    ContentLength(u64),
    /// No client `content-length`; HTTP/1.1 chunked transfer-coding.
    Chunked,
}

/// Test gauge: peak in-flight request-body bytes buffered by the streaming
/// egress (the single peeked chunk) — proves the whole body is not buffered.
#[cfg(any(test, feature = "test-gauges"))]
pub static MAX_INFLIGHT_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(test, feature = "test-gauges"))]
fn record_inflight(n: usize) {
    use std::sync::atomic::Ordering;
    let mut cur = MAX_INFLIGHT_BODY_BYTES.load(Ordering::Relaxed);
    while n > cur {
        match MAX_INFLIGHT_BODY_BYTES.compare_exchange_weak(
            cur,
            n,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => cur = observed,
        }
    }
}

#[cfg(not(any(test, feature = "test-gauges")))]
#[inline]
fn record_inflight(_n: usize) {}

/// Write one request-body chunk with the chosen framing. Empty data is a no-op
/// — a zero-length DATA frame must NOT emit a spurious chunk terminator.
async fn write_body_chunk(stream: &mut TcpStream, data: &[u8], chunked: bool) -> Result<(), ()> {
    if data.is_empty() {
        return Ok(());
    }
    if chunked {
        let hdr = format!("{:x}\r\n", data.len());
        stream.write_all(hdr.as_bytes()).await.map_err(|_| ())?;
        stream.write_all(data).await.map_err(|_| ())?;
        stream.write_all(b"\r\n").await.map_err(|_| ())?;
    } else {
        stream.write_all(data).await.map_err(|_| ())?;
    }
    Ok(())
}

/// Terminal outcome of the request-write half. Every non-`Complete` outcome
/// means the request was NOT completed on the wire, so the caller MUST mark the
/// pooled connection non-reusable (request-smuggling guard).
enum ReqWriteOutcome {
    /// Head + body fully written and flushed; the caller streams the response
    /// on the same stream.
    Complete,
    /// Graceful abort from the body channel: `Reset` ⇒ 413, channel closed
    /// before `End` ⇒ 502.
    Aborted(u16, &'static [u8]),
}

/// The request-write half: peek the first body event to choose framing, write
/// the H1 head, then forward request DATA incrementally (one event held at a
/// time ⇒ request-side backpressure). On any abort it returns BEFORE the chunked
/// terminator / full `Content-Length`, so the upstream never sees a completable
/// request.
///
/// **C2:** this only borrows `stream` and NEVER calls `set_reusable` — the
/// CALLER marks the `PooledTcp` non-reusable on `Err(())`, on
/// [`ReqWriteOutcome::Aborted`], and on any [`RespAbort`] from
/// [`stream_h1_response`].
///
/// # Errors
///
/// `Err(())` on any upstream write/flush I/O failure — the caller's 502 path.
#[allow(clippy::too_many_lines)]
async fn write_h1_request(
    req: &H3Request,
    stream: &mut TcpStream,
    body_rx: &mut tokio::sync::mpsc::Receiver<ReqBodyEvent>,
) -> Result<ReqWriteOutcome, ()> {
    // Peek the FIRST body event: at most ONE chunk is buffered, never the body.
    let first = body_rx.recv().await;
    let (framing, mut pending_first): (H1BodyFraming, Option<Bytes>) = match &first {
        Some(ReqBodyEvent::End { .. }) | None => (H1BodyFraming::None, None),
        Some(ReqBodyEvent::Reset) => {
            // Reset before any data ⇒ the oversized/abort signal ⇒ 413.
            return Ok(ReqWriteOutcome::Aborted(413, b"payload too large"));
        }
        Some(ReqBodyEvent::Chunk(b)) => {
            let cl = req.extra.iter().find_map(|(n, v)| {
                if n.eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<u64>().ok()
                } else {
                    None
                }
            });
            match cl {
                Some(n) => (H1BodyFraming::ContentLength(n), Some(b.clone())),
                None => (H1BodyFraming::Chunked, Some(b.clone())),
            }
        }
    };
    record_inflight(pending_first.as_ref().map_or(0, Bytes::len));

    let head = build_h1_head(req, &framing);
    let chunked = framing == H1BodyFraming::Chunked;

    if let Err(e) = stream.write_all(&head).await {
        tracing::warn!(error = %e, "H3→H1 stream head write failed");
        return Err(());
    }

    if let Some(b) = pending_first.take() {
        if write_body_chunk(stream, &b, chunked).await.is_err() {
            tracing::warn!(error = %"first chunk", "H3→H1 stream body write failed");
            return Err(());
        }
    }

    // One event held at a time ⇒ backpressure: a slow upstream stalls this
    // loop, the channel fills, and poll_h3 stops extending QUIC flow control.
    let mut clean_end = matches!(first, Some(ReqBodyEvent::End { .. }) | None);
    while let Some(ev) = body_rx.recv().await {
        match ev {
            ReqBodyEvent::Chunk(b) => {
                record_inflight(b.len());
                if write_body_chunk(stream, &b, chunked).await.is_err() {
                    tracing::warn!(error = %"chunk", "H3→H1 stream body write failed");
                    return Err(());
                }
            }
            ReqBodyEvent::End { trailers: _ } => {
                // Request trailers are INTENTIONALLY DROPPED on the H3→H1 leg
                // (still parsed upstream, so a malformed block is rejected).
                // Forwarding them needs chunked PLUS a `Trailer:` announcement
                // (RFC 9110 §6.5), and smuggling peer-controlled fields into the
                // H1 head is a request-smuggling vector. The body is already
                // correctly framed, so this is an RFC-acceptable downgrade.
                clean_end = true;
                break;
            }
            ReqBodyEvent::Reset => {
                // Mid-body Reset — an oversized cap breach OR a client cancel.
                // Return IMMEDIATELY, BEFORE the chunked terminator / the full
                // Content-Length, so the backend never sees a completable
                // request; the CALLER marks the conn non-reusable. The 413 is
                // incidental; the load-bearing invariant is the abort.
                tracing::warn!(
                    "SESSION 2 / P1-B: H3→H1 stream body Reset (oversized or \
                     client cancel); aborting upstream without completing the request"
                );
                return Ok(ReqWriteOutcome::Aborted(413, b"payload too large"));
            }
        }
    }
    if !clean_end {
        // Producer dropped mid-body — never present a truncated request.
        tracing::warn!("H3→H1 stream channel closed before End; aborting upstream");
        return Ok(ReqWriteOutcome::Aborted(502, b"bad gateway"));
    }
    if chunked {
        if let Err(e) = stream.write_all(b"0\r\n\r\n").await {
            tracing::warn!(error = %e, "H3→H1 stream chunked terminator failed");
            return Err(());
        }
    }

    if let Err(e) = stream.flush().await {
        tracing::warn!(error = %e, "H3→H1 stream flush failed");
        return Err(());
    }
    Ok(ReqWriteOutcome::Complete)
}

/// **Incremental, bounded, backpressured** H3→H1 relay with incremental
/// response egress — the actor's H1 producer task body. Owns the `PooledTcp`;
/// the request half is [`write_h1_request`], the response is streamed by
/// [`stream_h1_response`] into the bounded `resp_tx` (the R8 memory bound).
///
/// **C2:** EVERY outcome marks the pooled connection NON-reusable before it
/// drops — including the clean path, since the request carries
/// `Connection: close` — so a partial request can never poison a pooled conn.
///
/// Returns `Ok(())` once the response was piped or after an inline `413`/`502`;
/// `Err(RespAbort)` means the actor already saw the matching `Reset`.
pub async fn h3_to_h1_stream_resp(
    req: &H3Request,
    backend: SocketAddr,
    pool: &TcpPool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    resp_tx: tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    /// Emit a complete inline H3 response for the request-write abort paths.
    /// Best-effort: a closed channel just means nobody is listening.
    async fn inline(tx: &tokio::sync::mpsc::Sender<RespEvent>, status: u16, body: &[u8]) {
        let _ = tx
            .send(RespEvent::Head {
                status,
                headers: Vec::new(),
            })
            .await;
        if !body.is_empty() {
            let _ = tx.send(RespEvent::Body(Bytes::copy_from_slice(body))).await;
        }
        let _ = tx.send(RespEvent::End).await;
    }

    let mut pooled = match pool.acquire_async(backend).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "H3→H1 resp stream backend acquire failed");
            inline(&resp_tx, 502, b"bad gateway").await;
            // No upstream connection acquired — nothing to poison.
            return Ok(());
        }
    };

    let outcome: Result<(), RespAbort> = {
        let Some(stream) = pooled.stream_mut() else {
            inline(&resp_tx, 502, b"bad gateway").await;
            pooled.set_reusable(false);
            return Ok(());
        };

        match write_h1_request(req, stream, &mut body_rx).await {
            Ok(ReqWriteOutcome::Complete) => {
                // Any RespAbort ⇒ the upstream was consumed unfaithfully ⇒ C2.
                stream_h1_response(stream, &resp_tx, cap).await
            }
            Ok(ReqWriteOutcome::Aborted(status, body)) => {
                inline(&resp_tx, status, body).await;
                // Request never completed on the wire — smuggling guard.
                pooled.set_reusable(false);
                return Ok(());
            }
            Err(()) => {
                inline(&resp_tx, 502, b"bad gateway").await;
                pooled.set_reusable(false);
                return Ok(());
            }
        }
    };

    // C2: every remaining outcome — `Err` AND the clean path — marks the
    // pooled connection non-reusable before it drops.
    pooled.set_reusable(false);
    outcome
}

/// Error carried by the streaming H2 request body so a mid-body abort is
/// expressible (`hyper::Error` has no public constructor).
#[derive(Debug)]
struct H3ReqAbort;

impl std::fmt::Display for H3ReqAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("H3→H2 request body aborted (client RESET / producer dropped mid-body)")
    }
}
impl std::error::Error for H3ReqAbort {}

/// The streaming H2 request body: one hyper DATA `Frame` per
/// `ReqBodyEvent::Chunk`, completing on `End`/closed and **erroring**
/// ([`H3ReqAbort`]) on a mid-body `Reset` so hyper RST_STREAMs the upstream — a
/// truncated request is never presented as complete (BINDING case 7).
/// `poll_frame` polls `body_rx` directly, so backpressure is end-to-end.
struct H3ReqStreamBody {
    body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    first: Option<Bytes>,
    done: bool,
}

impl hyper::body::Body for H3ReqStreamBody {
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        use std::task::Poll;
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        if let Some(b) = this.first.take() {
            return Poll::Ready(Some(Ok(hyper::body::Frame::data(b))));
        }
        match this.body_rx.poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(ReqBodyEvent::Chunk(b))) => {
                Poll::Ready(Some(Ok(hyper::body::Frame::data(b))))
            }
            Poll::Ready(Some(ReqBodyEvent::End { trailers: _ })) => {
                // Request trailers are NOT forwarded on the H2 leg (parity
                // with H3→H1); this is a clean, fully-framed end-of-stream.
                this.done = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(ReqBodyEvent::Reset)) | Poll::Ready(None) => {
                // Mid-body RESET / producer dropped before End: error so hyper
                // RST_STREAMs — a truncated request is NEVER presented as
                // complete. H2 multiplexing ⇒ a per-stream RST is not poison.
                this.done = true;
                Poll::Ready(Some(Err(Box::new(H3ReqAbort))))
            }
        }
    }
}

/// Build the upstream H2 request with a **streaming, bounded-incremental** body
/// fed from the inbound H3 request DATA channel. Framing mirrors
/// [`write_h1_request`], peeking the FIRST `ReqBodyEvent`: `End`/closed ⇒ a
/// legitimately **bodyless** request (NOT a dropped body); `Reset` ⇒ pre-dial
/// `Err(413)` so the caller dials NOTHING; `Chunk` ⇒ an [`H3ReqStreamBody`]
/// that errors on a mid-body `Reset`. Request trailers are DROPPED on this leg
/// (parity with H3→H1 — a lossless downgrade, not silent body loss).
fn h2_request_body_from_rx(
    req: &H3Request,
    addr: std::net::SocketAddr,
    body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    first: Option<ReqBodyEvent>,
) -> Result<Request<lb_io::http2_pool::H2ReqBody>, u16> {
    let scheme = "http"; // upstream is plaintext H2 in v1
    let authority = if req.authority.is_empty() {
        addr.to_string()
    } else {
        req.authority.clone()
    };
    let uri = format!("{scheme}://{authority}{}", req.path);
    let mut builder = Request::builder().method(req.method.as_str()).uri(uri);
    for (n, v) in &req.extra {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }

    let body: lb_io::http2_pool::H2ReqBody = match first {
        // Bodyless: legitimately empty (Content-Length: 0), NOT a dropped body.
        Some(ReqBodyEvent::End { .. }) | None => Full::<Bytes>::new(Bytes::new())
            .map_err(|never| match never {})
            .boxed(),
        // Reset before any data ⇒ pre-dial abort (413); nothing is dialled.
        Some(ReqBodyEvent::Reset) => return Err(413),
        // `H3ReqStreamBody` pulls `body_rx` one frame at a time (end-to-end
        // backpressure) and errors on a mid-body Reset so hyper RST_STREAMs.
        Some(ReqBodyEvent::Chunk(b0)) => BoxBody::new(H3ReqStreamBody {
            body_rx,
            first: Some(b0),
            done: false,
        }),
    };

    builder.body(body).map_err(|_| 502u16)
}

/// The streaming H3→H2 orchestrator — the H2 cousin of
/// [`h3_to_h1_stream_resp`], same channel contract. Peeks the first body event
/// (a pre-data `Reset` ⇒ inline 413 with NOTHING dialled), does the header
/// roundtrip, then relays the response via [`stream_h2_response`]. Returns
/// `Ok(())` after a piped or inline response; `Err(RespAbort)` ⇒ the actor
/// RESET_STREAMs.
pub async fn h3_to_h2_stream_resp(
    req: &H3Request,
    addr: std::net::SocketAddr,
    pool: &Http2Pool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    resp_tx: tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    /// Emit a complete inline H3 response (Head + Body, then `End`).
    /// Best-effort: a closed channel just means nobody is listening.
    async fn inline(tx: &tokio::sync::mpsc::Sender<RespEvent>, status: u16, body: &[u8]) {
        let _ = tx
            .send(RespEvent::Head {
                status,
                headers: Vec::new(),
            })
            .await;
        if !body.is_empty() {
            let _ = tx.send(RespEvent::Body(Bytes::copy_from_slice(body))).await;
        }
        let _ = tx.send(RespEvent::End).await;
    }

    let first = body_rx.recv().await;

    let request = match h2_request_body_from_rx(req, addr, body_rx, first) {
        Ok(r) => r,
        Err(413) => {
            inline(&resp_tx, 413, b"payload too large").await;
            return Ok(());
        }
        Err(_) => {
            inline(&resp_tx, 502, b"bad gateway").await;
            return Ok(());
        }
    };

    let resp = match pool.send_request(addr, request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, %addr, "H3→H2 stream send_request failed");
            inline(&resp_tx, 502, b"bad gateway").await;
            return Ok(());
        }
    };

    stream_h2_response(resp, &resp_tx, cap).await
}

/// The per-front RESPONSE SINK [`stream_request_to_h3_upstream`] relays
/// through: its transport driver is front-agnostic and shared, and only the
/// response *emission* differs per front. [`Wire`](Self::Wire) serves an H3
/// front; [`Decoded`](Self::Decoded) serves an H1/H2 front whose L7 layer runs
/// its own head transform without re-decoding H3. `total`/`cap` is a DoS abort
/// threshold, NOT a memory mechanism.
pub enum H3RespOut {
    /// H3 front: emit decoded [`RespEvent`]s that the actor re-encodes.
    Wire {
        /// Response-event channel back to the actor.
        tx: tokio::sync::mpsc::Sender<RespEvent>,
        /// Cumulative relayed bytes (cap accounting).
        total: usize,
        /// DoS abort threshold (NOT a memory bound).
        cap: usize,
    },
    /// H1/H2 front: forward the decoded [`H3RespEvent`].
    Decoded {
        /// Decoded-response-event channel to the L7 front producer.
        tx: tokio::sync::mpsc::Sender<H3RespEvent>,
        /// Cumulative decoded payload bytes relayed (cap accounting).
        total: usize,
        /// DoS abort threshold (NOT a memory bound).
        cap: usize,
    },
}

impl H3RespOut {
    /// Emit a complete inline response (head + body, then `End`). Best-effort:
    /// a closed channel just means nobody is listening.
    async fn inline(&mut self, status: u16, body: &[u8]) {
        match self {
            Self::Wire { tx, .. } => {
                let _ = tx
                    .send(RespEvent::Head {
                        status,
                        headers: Vec::new(),
                    })
                    .await;
                if !body.is_empty() {
                    let _ = tx.send(RespEvent::Body(Bytes::copy_from_slice(body))).await;
                }
                let _ = tx.send(RespEvent::End).await;
            }
            Self::Decoded { tx, .. } => {
                let _ = tx
                    .send(H3RespEvent::Head {
                        status,
                        headers: Vec::new(),
                    })
                    .await;
                if !body.is_empty() {
                    let _ = tx
                        .send(H3RespEvent::Body(Bytes::copy_from_slice(body)))
                        .await;
                }
                let _ = tx.send(H3RespEvent::End).await;
            }
        }
    }

    /// Relay the response HEAD; both arms parse `:status` out and forward the
    /// full non-pseudo set with `content-length` kept as a regular header.
    async fn on_head(&mut self, fields: &[(String, String)]) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, total, cap } => {
                // CF-H3-HEAD: the hop-by-hop strip is REQUIRED, not tidiness —
                // RFC 9114 §4.2 forbids connection-specific fields in an H3
                // field section, so an upstream's `connection` must not relay.
                let mut status: u16 = 502;
                let mut headers: Vec<(String, String)> = Vec::with_capacity(fields.len());
                for (n, v) in fields {
                    if n == ":status" {
                        if let Ok(s) = v.parse::<u16>() {
                            status = s;
                        }
                    } else if !n.starts_with(':') && !is_response_hop_by_hop(n) {
                        headers.push((n.clone(), v.clone()));
                    }
                }
                *total = total.saturating_add(headers.iter().map(|(n, v)| n.len() + v.len()).sum());
                if *total > *cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                tx.send(RespEvent::Head { status, headers })
                    .await
                    .map_err(|_| RespAbort::ClientGone)
            }
            Self::Decoded { tx, .. } => {
                let mut status: u16 = 502;
                let mut headers: Vec<(String, String)> = Vec::with_capacity(fields.len());
                for (n, v) in fields {
                    if n == ":status" {
                        if let Ok(s) = v.parse::<u16>() {
                            status = s;
                        }
                    } else if !n.starts_with(':') {
                        headers.push((n.clone(), v.clone()));
                    }
                }
                tx.send(H3RespEvent::Head { status, headers })
                    .await
                    .map_err(|_| RespAbort::ClientGone)
            }
        }
    }

    /// Relay one response-body slice (≤ [`H3_RESP_CHUNK_MAX`]).
    async fn on_data(&mut self, slice: &[u8]) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, total, cap } => {
                *total = total.saturating_add(slice.len());
                if *total > *cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                tx.send(RespEvent::Body(Bytes::copy_from_slice(slice)))
                    .await
                    .map_err(|_| RespAbort::ClientGone)
            }
            Self::Decoded { tx, total, cap } => {
                *total = total.saturating_add(slice.len());
                if *total > *cap {
                    let _ = tx.send(H3RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                tx.send(H3RespEvent::Body(Bytes::copy_from_slice(slice)))
                    .await
                    .map_err(|_| RespAbort::ClientGone)
            }
        }
    }

    /// Relay the (non-empty) trailing field section.
    async fn on_trailers(&mut self, trailers: Vec<(String, String)>) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, total, cap } => {
                *total =
                    total.saturating_add(trailers.iter().map(|(n, v)| n.len() + v.len()).sum());
                if *total > *cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                tx.send(RespEvent::Trailers(trailers))
                    .await
                    .map_err(|_| RespAbort::ClientGone)
            }
            Self::Decoded { tx, .. } => tx
                .send(H3RespEvent::Trailers(trailers))
                .await
                .map_err(|_| RespAbort::ClientGone),
        }
    }

    /// Terminal clean end — the actor / L7 front FINs the client.
    async fn on_end(&mut self) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, .. } => tx
                .send(RespEvent::End)
                .await
                .map_err(|_| RespAbort::ClientGone),
            Self::Decoded { tx, .. } => tx
                .send(H3RespEvent::End)
                .await
                .map_err(|_| RespAbort::ClientGone),
        }
    }

    /// Best-effort abort signal — the client is RESET and never FIN'd. A
    /// closed channel is ignored.
    async fn on_reset(&mut self) {
        match self {
            Self::Wire { tx, .. } => {
                let _ = tx.send(RespEvent::Reset).await;
            }
            Self::Decoded { tx, .. } => {
                let _ = tx.send(H3RespEvent::Reset).await;
            }
        }
    }
}

/// The H3→H3 cell's streaming response producer: a thin front for
/// [`stream_request_to_h3_upstream`] with an [`H3RespOut::Wire`] sink and
/// `forward_req_trailers = false` (the H3→H3 request-trailer drop).
#[allow(clippy::large_futures)]
pub async fn h3_to_h3_stream_resp(
    req: &H3Request,
    addr: SocketAddr,
    sni: &str,
    pool: &QuicUpstreamPool,
    body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    resp_tx: tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    let authority = if req.authority.is_empty() {
        sni.to_string()
    } else {
        req.authority.clone()
    };
    let headers: Vec<(String, String)> = vec![
        (":method".to_string(), req.method.clone()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), authority),
        (":path".to_string(), req.path.clone()),
    ];
    let sink = H3RespOut::Wire {
        tx: resp_tx,
        total: 0,
        cap,
    };
    stream_request_to_h3_upstream(headers, false, addr, sni, pool, body_rx, sink).await
}

/// Bounded streaming H3-upstream connector, shared by all three `→H3` cells
/// (H3→H3 via [`h3_to_h3_stream_resp`]; H1→H3 / H2→H3 via the `lb-l7` bridge,
/// which own building `headers`). Wraps the pooled, established upstream
/// `quiche::Connection` as a `quiche::h3` CLIENT and drives its
/// send/recv/timeout loop, relaying through the per-front [`H3RespOut`] sink.
///
/// The response body is NEVER whole-buffered — it is read into a FIXED scratch
/// and streamed in ≤ [`H3_RESP_CHUNK_MAX`] slices, so retained memory is
/// response-size INDEPENDENT and `cap` is only a DoS abort threshold. The
/// sink's `send(..).await` is the backpressure gate: a stalled client parks
/// this fn, `stream_recv` stops, and quiche withholds `MAX_STREAM_DATA`.
///
/// A leading `Reset` ⇒ inline `413` with NOTHING dialled. A mid-body `Reset`,
/// or a producer dropped before `End`, ⇒ NO FIN plus
/// `stream_shutdown(Write, H3_REQUEST_CANCELLED)`, so the upstream never sees a
/// truncated-as-complete request (BINDING case-7). The pooled conn is marked
/// non-reusable on EVERY exit.
///
/// # Errors
///
/// Returns `Err(RespAbort)`: a partial / premature-FIN / decode-error / reset
/// response is NEVER given a clean end — only a best-effort sink `Reset`, so the
/// caller RESETs the client and never FINs (response-splitting guard).
#[allow(clippy::too_many_lines, clippy::large_futures)]
pub async fn stream_request_to_h3_upstream(
    headers: Vec<(String, String)>,
    forward_req_trailers: bool,
    addr: SocketAddr,
    sni: &str,
    pool: &QuicUpstreamPool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    mut sink: H3RespOut,
) -> Result<(), RespAbort> {
    // F-S7-6 idle deadline: reset ONLY on bidirectional app-data progress.
    let mut idle_deadline = tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
    macro_rules! send_progress {
        ($call:expr) => {{
            $call?;
            idle_deadline = tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
        }};
    }

    let mut req_streaming: bool = false;
    let mut first_chunk: Option<Bytes> = None;
    let mut bodyless_trailers: Vec<(String, String)> = Vec::new();
    match body_rx.recv().await {
        None => {}
        Some(ReqBodyEvent::End { trailers }) if forward_req_trailers => {
            bodyless_trailers = trailers;
        }
        Some(ReqBodyEvent::End { .. }) => {}
        Some(ReqBodyEvent::Reset) => {
            sink.inline(413, b"payload too large").await;
            return Ok(());
        }
        Some(ReqBodyEvent::Chunk(b0)) => {
            req_streaming = true;
            first_chunk = Some(b0);
        }
    }

    let mut pooled = match pool.acquire(addr, sni).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, %addr, "H3 upstream stream pool acquire failed");
            sink.inline(502, b"bad gateway").await;
            return Ok(());
        }
    };
    let Some(upstream) = pooled.get_mut() else {
        tracing::warn!("H3 upstream stream pool returned empty handle");
        sink.inline(502, b"bad gateway").await;
        return Ok(());
    };

    let socket_clone = Arc::clone(upstream.socket());
    let local = upstream.local();
    let peer = upstream.peer();
    let qconn: &mut quiche::Connection = match upstream.connection_mut() {
        Some(c) => c,
        None => {
            pooled.set_reusable(false);
            sink.inline(502, b"bad gateway").await;
            return Ok(());
        }
    };

    // `with_transport` sends SETTINGS and opens the client control + QPACK
    // uni streams. The conn is used once (non-reusable on every exit).
    let mut h3 = match crate::h3_config::build_client_h3_config()
        .and_then(|cfg| quiche::h3::Connection::with_transport(qconn, &cfg))
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "INC-4: client h3 init (config/with_transport) failed");
            pooled.set_reusable(false);
            sink.inline(502, b"bad gateway").await;
            return Ok(());
        }
    };

    // FIN here ONLY for a bodyless request with no trailers to forward.
    let headers_fin = !req_streaming && bodyless_trailers.is_empty();
    let h3_headers: Vec<quiche::h3::Header> = headers
        .iter()
        .map(|(n, v)| quiche::h3::Header::new(n.as_bytes(), v.as_bytes()))
        .collect();
    let stream_id = match h3.send_request(qconn, &h3_headers, headers_fin) {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "INC-4: h3 send_request (upstream HEADERS)");
            pooled.set_reusable(false);
            sink.inline(502, b"bad gateway").await;
            return Ok(());
        }
    };
    drop(h3_headers);

    // Bodyless WITH forwarded trailers (L7 fronts): a trailing field section +
    // FIN, no DATA (RFC 9114 §4.1). On error abort WITHOUT FIN (case-7).
    let ship_bodyless_trailers = !req_streaming && !bodyless_trailers.is_empty();
    if ship_bodyless_trailers {
        let tr: Vec<quiche::h3::Header> = bodyless_trailers
            .iter()
            .map(|(n, v)| quiche::h3::Header::new(n.as_bytes(), v.as_bytes()))
            .collect();
        if let Err(e) = h3.send_additional_headers(qconn, stream_id, &tr, true, true) {
            tracing::warn!(error = %e, "INC-4: h3 send_additional_headers (req trailers)");
            let _ = qconn.stream_shutdown(stream_id, quiche::Shutdown::Write, H3_REQUEST_CANCELLED);
            pooled.set_reusable(false);
            sink.on_reset().await;
            return Err(RespAbort::UpstreamReset);
        }
    }

    // --- request-DATA send state. `send_body` frames the DATA, so `InHand`
    // holds RAW bytes; the memory bound is the depth-8 `body_rx`. ---
    enum ReqSend {
        /// Raw chunk bytes; `sent` already written via send_body (partial retry).
        InHand { bytes: Bytes, sent: usize },
        /// Previous chunk fully sent; pull the next ReqBodyEvent at the park.
        AwaitNext,
        /// Clean end-of-request: a FIN (send_body(.., true)) has been written.
        Ended,
    }
    let mut req_send = if req_streaming {
        match first_chunk.take() {
            Some(b0) => ReqSend::InHand { bytes: b0, sent: 0 },
            None => ReqSend::AwaitNext,
        }
    } else {
        ReqSend::Ended
    };

    let mut scratch = [0u8; H3_RESP_CHUNK_MAX];
    let mut out_buf = vec![0u8; 65_535];
    let mut in_buf = vec![0u8; 65_535];
    let mut sent_head = false;
    let mut response_complete = false;
    let mut outcome: Result<(), RespAbort> = Ok(());

    // Content-length truncation guard (defense-in-depth, owner-ruled): quiche
    // does NOT enforce RFC 9114 §7.1 DATA-frame completeness at FIN, so a
    // backend declaring N bytes and cleanly FINing after M<N arrives as
    // `Event::Finished` with no error (CF-QUICHE-FRAME-COMPLETENESS). Residual
    // gap: a no-content-length mid-frame FIN. Skipped for bodiless responses.
    let req_is_head = headers
        .iter()
        .any(|(n, v)| n == ":method" && v.eq_ignore_ascii_case("HEAD"));
    let mut declared_cl: Option<u64> = None;
    let mut resp_status: Option<u16> = None;
    let mut body_relayed: u64 = 0;

    // Drain the response body into the bounded sink, ≤`H3_RESP_CHUNK_MAX` per
    // slice; the scratch is fixed and the body is NEVER whole-buffered.
    // `sink.on_data().await` is the R8 backpressure point. A mid-body
    // `recv_body` error (F-MD-4) maps to an upstream reset, never a clean EOF.
    // Used on a `Data` event AND unconditionally after the poll loop (PASS-3):
    // quiche does not re-arm `Data` after a 0-length DATA frame while the stream
    // stays readable, so `poll` can advance the NEXT frame without a fresh
    // event. The outer label is passed in — macro-hygienic labels cannot break
    // a label defined at the call site.
    macro_rules! drain_resp_body {
        ($evloop:lifetime, $progressed:ident, $relayed:ident) => {{
            loop {
                match h3.recv_body(qconn, stream_id, &mut scratch) {
                    Ok(0) => break,
                    Ok(n) => {
                        let slice = scratch.get(..n).unwrap_or(&[]);
                        match sink.on_data(slice).await {
                            Ok(()) => {
                                $progressed = true;
                                $relayed = $relayed.saturating_add(n as u64);
                                idle_deadline =
                                    tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
                            }
                            Err(a) => {
                                outcome = Err(a);
                                break $evloop;
                            }
                        }
                    }
                    Err(quiche::h3::Error::Done) => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "INC-4: h3 recv_body (genuine reset)");
                        outcome = Err(RespAbort::UpstreamReset);
                        break $evloop;
                    }
                }
            }
        }};
    }

    'evloop: while tokio::time::Instant::now() < idle_deadline {
        // Set by `drain_resp_body!`; drives the re-poll-instead-of-park below.
        let mut progressed = false;
        // (a) request-DATA egress. `send_body` is flow-control-gated: Done ⇒
        // keep the chunk in hand and do NOT pull `body_rx`, pausing the upload.
        if let ReqSend::InHand { bytes, sent } = &mut req_send {
            let rest = bytes.get(*sent..).unwrap_or(&[]);
            match h3.send_body(qconn, stream_id, rest, false) {
                Ok(n) => {
                    *sent = sent.saturating_add(n);
                    if n > 0 {
                        idle_deadline = tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
                    }
                    if *sent >= bytes.len() {
                        req_send = ReqSend::AwaitNext;
                    }
                }
                Err(quiche::h3::Error::Done) => { /* window closed — retain, no pull */ }
                Err(e) => {
                    tracing::warn!(error = %e, "INC-4: h3 send_body (request DATA)");
                    let _ = qconn.stream_shutdown(
                        stream_id,
                        quiche::Shutdown::Write,
                        H3_REQUEST_CANCELLED,
                    );
                    outcome = Err(RespAbort::UpstreamReset);
                    break 'evloop;
                }
            }
        }

        while let Ok((n, info)) = qconn.send(&mut out_buf) {
            let bytes = out_buf.get(..n).unwrap_or(&[]);
            if socket_clone.send_to(bytes, info.to).await.is_err() {
                break;
            }
        }

        'poll: loop {
            match h3.poll(qconn) {
                Ok((sid, quiche::h3::Event::Headers { list, .. })) if sid == stream_id => {
                    let fields: Vec<(String, String)> = list
                        .iter()
                        .map(|h| {
                            use quiche::h3::NameValue;
                            (
                                String::from_utf8_lossy(h.name()).into_owned(),
                                String::from_utf8_lossy(h.value()).into_owned(),
                            )
                        })
                        .collect();
                    if !sent_head {
                        // Capture `:status` + `content-length` for the
                        // truncation guard before the sink consumes `fields`.
                        for (n, v) in &fields {
                            if n == ":status" {
                                resp_status = v.parse::<u16>().ok();
                            } else if n.eq_ignore_ascii_case("content-length") {
                                declared_cl = v.trim().parse::<u64>().ok();
                            }
                        }
                        send_progress!(sink.on_head(&fields).await);
                        sent_head = true;
                    } else {
                        // RFC 9114 §4.3: a pseudo-header in a trailing section
                        // is malformed ⇒ Reset, never forwarded.
                        if fields.iter().any(|(n, _)| n.starts_with(':')) {
                            sink.on_reset().await;
                            outcome = Err(RespAbort::BadHead);
                            break 'evloop;
                        }
                        if !fields.is_empty() {
                            // Explicit match→break (not `?`) so the post-loop
                            // `on_reset` still runs; `on_head` can use `?`
                            // because the sink already Reset on its error paths.
                            match sink.on_trailers(fields).await {
                                Ok(()) => {
                                    idle_deadline =
                                        tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
                                }
                                Err(a) => {
                                    outcome = Err(a);
                                    break 'evloop;
                                }
                            }
                        }
                    }
                }
                Ok((sid, quiche::h3::Event::Data)) if sid == stream_id => {
                    drain_resp_body!('evloop, progressed, body_relayed);
                }
                Ok((sid, quiche::h3::Event::Finished)) if sid == stream_id => {
                    // F-MD-4 MIRROR. quiche delivers `Finished` (NOT `Reset`)
                    // for a response stream the BACKEND RESET *after* its last
                    // DATA frame: its first `finished_streams` pop lacks the
                    // reset re-check the second pop performs. A clean end here
                    // would response-split the downstream client. Probe as
                    // quiche does — a zero-length `stream_recv` returns
                    // `StreamReset` — and map to `on_reset`, never `on_end`.
                    let was_reset = matches!(
                        qconn.stream_recv(stream_id, &mut []),
                        Err(quiche::Error::StreamReset(_))
                    );
                    // Truncation guard: a clean FIN with fewer body bytes than
                    // the declared `content-length` ⇒ RESET downstream, never a
                    // clean End. Skipped for bodiless (HEAD / 1xx / 204 / 304).
                    let bodiless_status = req_is_head
                        || matches!(
                            resp_status,
                            Some(s) if (100..200).contains(&s) || s == 204 || s == 304
                        );
                    let cl_truncated =
                        declared_cl.is_some_and(|cl| !bodiless_status && body_relayed < cl);
                    if was_reset {
                        tracing::debug!(
                            stream_id,
                            "INC-4 F-MD-4: Finished on a RESET response stream; \
                             Reset downstream (not a clean End)"
                        );
                        outcome = Err(RespAbort::UpstreamReset);
                    } else if !sent_head {
                        outcome = Err(RespAbort::PrematureEof);
                    } else if cl_truncated {
                        tracing::warn!(
                            stream_id,
                            declared_cl = ?declared_cl,
                            body_relayed,
                            "INC-4: content-length under-run at clean FIN (truncated \
                             upstream response); Reset downstream (not a clean End)"
                        );
                        outcome = Err(RespAbort::PrematureEof);
                    } else {
                        response_complete = true;
                    }
                    break 'evloop;
                }
                Ok((sid, quiche::h3::Event::Reset(code))) if sid == stream_id => {
                    tracing::debug!(
                        stream_id,
                        code,
                        "INC-4 F-MD-4: upstream reset response stream"
                    );
                    outcome = Err(RespAbort::UpstreamReset);
                    break 'evloop;
                }
                // Other streams / GoAway / PriorityUpdate — nothing to do.
                Ok(_) => {}
                Err(quiche::h3::Error::Done) => break 'poll,
                Err(e) => {
                    // quiche enforces the control / QPACK / frame-sequence
                    // rules itself and has already closed the conn — never a
                    // forwarded or false-complete response.
                    tracing::warn!(error = %e, "INC-4: h3 poll (upstream protocol error)");
                    outcome = Err(RespAbort::UpstreamReset);
                    break 'evloop;
                }
            }
        }

        // PASS-3 (edge-trigger safety): `poll` may have advanced the next DATA
        // frame WITHOUT emitting a `Data` event (the 0-length re-arm gap above);
        // relay it every tick so it is never stranded.
        if sent_head && !response_complete {
            drain_resp_body!('evloop, progressed, body_relayed);
        }

        if response_complete {
            break 'evloop;
        }

        // The next events are ALREADY queued in quiche and need no socket I/O,
        // so re-poll rather than park — parking would wait out the full quiche
        // timeout. Bounded: `progressed` needs ≥1 relayed byte, so no spin.
        if progressed {
            continue 'evloop;
        }

        // (d) the SINGLE park point: socket | next body event | quiche timeout.
        let timeout = qconn
            .timeout()
            .unwrap_or(std::time::Duration::from_millis(50));
        let want_next = matches!(req_send, ReqSend::AwaitNext);
        tokio::select! {
            biased;
            r = tokio::time::timeout(timeout, socket_clone.recv_from(&mut in_buf)) => {
                match r {
                    Ok(Ok((n, from))) => {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: local };
                        match qconn.recv(slice, info) {
                            Ok(_) | Err(quiche::Error::Done) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "INC-4: upstream recv");
                                outcome = Err(RespAbort::UpstreamReset);
                                break 'evloop;
                            }
                        }
                    }
                    Ok(Err(_)) | Err(_) => {
                        qconn.on_timeout();
                    }
                }
            }
            ev = body_rx.recv(), if want_next => {
                match j2_req_event_action(ev, forward_req_trailers) {
                    J2ReqAction::SendData(bytes) => {
                        req_send = ReqSend::InHand { bytes, sent: 0 };
                    }
                    J2ReqAction::FinWithTrailers(trailers) => {
                        let tr: Vec<quiche::h3::Header> = trailers
                            .iter()
                            .map(|(n, v)| quiche::h3::Header::new(n.as_bytes(), v.as_bytes()))
                            .collect();
                        match h3.send_additional_headers(qconn, stream_id, &tr, true, true) {
                            Ok(()) => {
                                idle_deadline =
                                    tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
                                req_send = ReqSend::Ended;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "INC-4: h3 send_additional_headers (req trailers, streaming)");
                                let _ = qconn.stream_shutdown(
                                    stream_id,
                                    quiche::Shutdown::Write,
                                    H3_REQUEST_CANCELLED,
                                );
                                outcome = Err(RespAbort::UpstreamReset);
                                break 'evloop;
                            }
                        }
                    }
                    J2ReqAction::FinNoTrailers => {
                        match h3.send_body(qconn, stream_id, &[], true) {
                            Ok(_) | Err(quiche::h3::Error::Done) => {
                                idle_deadline =
                                    tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "INC-4: h3 send_body FIN (request)");
                                outcome = Err(RespAbort::UpstreamReset);
                                break 'evloop;
                            }
                        }
                        req_send = ReqSend::Ended;
                    }
                    J2ReqAction::AbortNoFin => {
                        // Mid-body RESET / producer dropped before End ⇒ the
                        // upstream must NEVER see a completable request (case-7).
                        let _ = qconn.stream_shutdown(
                            stream_id,
                            quiche::Shutdown::Write,
                            H3_REQUEST_CANCELLED,
                        );
                        outcome = Err(RespAbort::UpstreamReset);
                        break 'evloop;
                    }
                }
            }
        }
        let _ = peer;
    }

    // One request per pooled upstream conn — non-reusable on EVERY exit.
    pooled.set_reusable(false);

    if response_complete {
        sink.on_end().await?;
        return Ok(());
    }
    if outcome.is_ok() {
        // Idle deadline fell through — premature EOF; NEVER End a partial.
        sink.on_reset().await;
        return Err(RespAbort::PrematureEof);
    }
    sink.on_reset().await;
    outcome
}

/// The request-send action taken for the next `ReqBodyEvent`, factored out
/// so `s7_j2_request_send_decision` exercises the REAL decision.
#[derive(Debug, PartialEq, Eq)]
enum J2ReqAction {
    /// `Chunk` ⇒ forward the RAW chunk bytes as one bounded request-body chunk
    /// (`quiche::h3::send_body` frames the DATA, so there is no encode step).
    SendData(Bytes),
    /// `End` ⇒ clean end-of-request: FIN the upstream request stream, request
    /// trailers DROPPED (H3→H3 leg, and the no-trailer case generally).
    FinNoTrailers,
    /// `End { trailers }` + `forward_req_trailers` ⇒ post-DATA HEADERS then FIN
    /// (RFC 9114 §4.1). L7 fronts only; never produced for H3→H3.
    FinWithTrailers(Vec<(String, String)>),
    /// `Reset` / channel-closed-before-`End` ⇒ mid-body abort: NO FIN,
    /// `stream_shutdown(Write, H3_REQUEST_CANCELLED)` (case-7 parity).
    AbortNoFin,
}

/// Classify the next request-body event into its send action. `None` (producer
/// dropped before a clean `End`) is treated identically to a mid-body `Reset` —
/// never a truncated-as-complete request. `forward_req_trailers = false`
/// (H3→H3) always maps `End { trailers }` to `FinNoTrailers`.
fn j2_req_event_action(ev: Option<ReqBodyEvent>, forward_req_trailers: bool) -> J2ReqAction {
    match ev {
        Some(ReqBodyEvent::Chunk(b)) => J2ReqAction::SendData(b),
        Some(ReqBodyEvent::End { trailers }) => {
            if forward_req_trailers && !trailers.is_empty() {
                J2ReqAction::FinWithTrailers(trailers)
            } else {
                J2ReqAction::FinNoTrailers
            }
        }
        Some(ReqBodyEvent::Reset) | None => J2ReqAction::AbortNoFin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn pseudo_valid_request_accepted_negative_control() {
        // Negative control: a well-formed request MUST pass.
        let ok = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":authority", "example.com"),
            ("user-agent", "h3spec"),
        ]);
        assert!(validate_request_pseudo_headers(&ok, false).is_ok());
        let min = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":authority", "h"),
        ]);
        assert!(validate_request_pseudo_headers(&min, false).is_ok());
    }

    #[test]
    fn pseudo_13_absent_authority_rejected_for_http_scheme() {
        // #13 (strict) — http/https with neither :authority nor Host.
        let neither = h(&[(":method", "GET"), (":scheme", "https"), (":path", "/")]);
        assert!(
            validate_request_pseudo_headers(&neither, false).is_err(),
            "https request with no :authority and no Host must be rejected"
        );
        let with_host = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            ("host", "example.com"),
        ]);
        assert!(
            validate_request_pseudo_headers(&with_host, false).is_ok(),
            "Host is a valid alternative to :authority (§4.3.1)"
        );
    }

    #[test]
    fn pseudo_12_duplicate_rejected() {
        let dup_method = h(&[
            (":method", "GET"),
            (":method", "POST"),
            (":scheme", "https"),
            (":path", "/"),
        ]);
        assert!(validate_request_pseudo_headers(&dup_method, false).is_err());
        let dup_path = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":path", "/x"),
        ]);
        assert!(validate_request_pseudo_headers(&dup_path, false).is_err());
    }

    #[test]
    fn pseudo_13_missing_mandatory_rejected() {
        let no_method = h(&[(":scheme", "https"), (":path", "/")]);
        assert!(validate_request_pseudo_headers(&no_method, false).is_err());
        let no_path = h(&[(":method", "GET"), (":scheme", "https")]);
        assert!(validate_request_pseudo_headers(&no_path, false).is_err());
        let no_scheme = h(&[(":method", "GET"), (":path", "/")]);
        assert!(validate_request_pseudo_headers(&no_scheme, false).is_err());
    }

    #[test]
    fn pseudo_14_prohibited_or_unknown_rejected() {
        let status_in_req = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":status", "200"),
        ]);
        assert!(validate_request_pseudo_headers(&status_in_req, false).is_err());
        let unknown = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":madeup", "x"),
        ]);
        assert!(validate_request_pseudo_headers(&unknown, false).is_err());
    }

    #[test]
    fn pseudo_15_after_regular_field_rejected() {
        let after = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            ("user-agent", "h3spec"),
            (":path", "/"),
        ]);
        assert!(validate_request_pseudo_headers(&after, false).is_err());
    }

    #[test]
    fn pseudo_connect_request_rules() {
        // RFC 9114 §4.4: CONNECT omits :scheme/:path and needs :authority.
        let ok = h(&[(":method", "CONNECT"), (":authority", "example.com:443")]);
        assert!(validate_request_pseudo_headers(&ok, false).is_ok());
        let bad_has_path = h(&[
            (":method", "CONNECT"),
            (":authority", "example.com:443"),
            (":path", "/"),
        ]);
        assert!(validate_request_pseudo_headers(&bad_has_path, false).is_err());
        let bad_no_authority = h(&[(":method", "CONNECT")]);
        assert!(validate_request_pseudo_headers(&bad_no_authority, false).is_err());
    }

    #[test]
    fn pseudo_ws_extended_connect_accepted_when_enabled() {
        let ext = h(&[
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":path", "/chat"),
            (":authority", "example.com:443"),
        ]);
        assert!(
            validate_request_pseudo_headers(&ext, true).is_ok(),
            "extended CONNECT with :scheme+:path+:authority must be accepted under ws_enabled"
        );
    }

    /// R3 control: the SAME Extended CONNECT is rejected via the unchanged #14
    /// path when WS is off.
    #[test]
    fn pseudo_ws_extended_connect_rejected_when_disabled() {
        let ext = h(&[
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":path", "/chat"),
            (":authority", "example.com:443"),
        ]);
        let err = validate_request_pseudo_headers(&ext, false)
            .expect_err("extended CONNECT must be rejected when ws_enabled=false (R3)");
        assert!(
            err.contains("prohibited/unknown request pseudo-header"),
            "ws-off reject must be the unchanged #14 path, got: {err}"
        );
    }

    #[test]
    fn pseudo_ws_extended_connect_requires_scheme_and_path() {
        let no_scheme = h(&[
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":path", "/chat"),
            (":authority", "example.com:443"),
        ]);
        assert!(
            validate_request_pseudo_headers(&no_scheme, true)
                .is_err_and(|e| e.contains("missing :scheme")),
            "extended CONNECT without :scheme must be rejected (RFC 8441 §4)"
        );
        let no_path = h(&[
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":authority", "example.com:443"),
        ]);
        assert!(
            validate_request_pseudo_headers(&no_path, true)
                .is_err_and(|e| e.contains("missing :path")),
            "extended CONNECT without :path must be rejected (RFC 8441 §4)"
        );
        let no_authority = h(&[
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":path", "/chat"),
        ]);
        assert!(
            validate_request_pseudo_headers(&no_authority, true)
                .is_err_and(|e| e.contains("missing :authority")),
            "extended CONNECT without :authority must be rejected (RFC 8441 §4)"
        );
    }

    #[test]
    fn pseudo_ws_protocol_requires_connect_method() {
        let proto_on_get = h(&[
            (":method", "GET"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":path", "/chat"),
            (":authority", "example.com:443"),
        ]);
        assert!(
            validate_request_pseudo_headers(&proto_on_get, true)
                .is_err_and(|e| e.contains("requires :method=CONNECT")),
            ":protocol on a non-CONNECT method must be rejected (RFC 8441 §4)"
        );
    }

    /// The WS gate must NOT relax the classic-CONNECT envelope.
    #[test]
    fn pseudo_classic_connect_unchanged_under_ws_enabled() {
        let ok = h(&[(":method", "CONNECT"), (":authority", "example.com:443")]);
        assert!(validate_request_pseudo_headers(&ok, true).is_ok());
        let bad_has_path = h(&[
            (":method", "CONNECT"),
            (":authority", "example.com:443"),
            (":path", "/"),
        ]);
        assert!(
            validate_request_pseudo_headers(&bad_has_path, true)
                .is_err_and(|e| e.contains("must omit :scheme/:path")),
            "a classic CONNECT (no :protocol) with :path must still be rejected under ws_enabled"
        );
    }

    #[test]
    fn pseudo_ws_duplicate_protocol_rejected() {
        let dup = h(&[
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":path", "/chat"),
            (":authority", "example.com:443"),
        ]);
        assert!(
            validate_request_pseudo_headers(&dup, true)
                .is_err_and(|e| e.contains("duplicate :protocol")),
            "a duplicated :protocol must be rejected (RFC 9114 §4.3.1)"
        );
    }

    #[test]
    fn pseudo_normal_request_unchanged_under_ws_enabled() {
        let ok = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":authority", "example.com"),
        ]);
        assert!(validate_request_pseudo_headers(&ok, true).is_ok());
        assert!(validate_request_pseudo_headers(&ok, false).is_ok());
    }

    /// `H3ReqStreamBody` frame + abort contract: a mid-body `Reset` and a
    /// premature close MUST error so hyper RST_STREAMs (BINDING case 7).
    #[tokio::test]
    async fn s6_i2_h3_req_stream_body_frame_and_abort_contract() {
        use http_body_util::BodyExt as _;

        let (tx, rx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(8);
        let mut body = H3ReqStreamBody {
            body_rx: rx,
            first: Some(Bytes::from_static(b"AAAA")),
            done: false,
        };
        tx.send(ReqBodyEvent::Chunk(Bytes::from_static(b"BBBB")))
            .await
            .unwrap();
        tx.send(ReqBodyEvent::End {
            trailers: Vec::new(),
        })
        .await
        .unwrap();
        drop(tx);
        let f1 = body.frame().await.unwrap().unwrap();
        assert_eq!(f1.into_data().unwrap().as_ref(), b"AAAA");
        let f2 = body.frame().await.unwrap().unwrap();
        assert_eq!(f2.into_data().unwrap().as_ref(), b"BBBB");
        assert!(body.frame().await.is_none(), "End ⇒ clean EOS");
        assert!(body.frame().await.is_none(), "done latches");

        // (b) mid-body Reset ⇒ Err (hyper RST_STREAMs; BINDING case 7).
        let (tx, rx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(8);
        let mut body = H3ReqStreamBody {
            body_rx: rx,
            first: Some(Bytes::from_static(b"X")),
            done: false,
        };
        tx.send(ReqBodyEvent::Reset).await.unwrap();
        let _ = body.frame().await.unwrap().unwrap(); // first chunk
        let err = body.frame().await.unwrap();
        assert!(err.is_err(), "mid-body Reset MUST surface as a body error");
        assert!(
            body.frame().await.is_none(),
            "post-error poll latches to None"
        );

        // (c) channel closed before End ⇒ Err, never a truncated request.
        let (tx, rx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(8);
        let mut body = H3ReqStreamBody {
            body_rx: rx,
            first: Some(Bytes::from_static(b"Y")),
            done: false,
        };
        drop(tx);
        let _ = body.frame().await.unwrap().unwrap(); // first chunk
        assert!(
            body.frame().await.unwrap().is_err(),
            "premature close MUST error (truncation guard)"
        );

        // (d) a leading `Reset` ⇒ pre-dial 413, nothing dialled.
        let (_tx, rx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(1);
        let req = H3Request {
            method: "POST".to_string(),
            path: "/p".to_string(),
            authority: "h.test".to_string(),
            extra: Vec::new(),
            trailers: Vec::new(),
        };
        let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let r = h2_request_body_from_rx(&req, addr, rx, Some(ReqBodyEvent::Reset));
        assert_eq!(r.err(), Some(413), "pre-data Reset ⇒ 413, no dial");

        let (_tx, rx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(1);
        let r = h2_request_body_from_rx(
            &req,
            addr,
            rx,
            Some(ReqBodyEvent::End {
                trailers: Vec::new(),
            }),
        );
        assert!(r.is_ok(), "bodyless request must build");
    }

    /// `H3ReqAbort`'s `Display`/`Error` impls — the request-smuggling path.
    #[test]
    fn g5_h3reqabort_display_and_error_impl() {
        let e = H3ReqAbort;
        let s = e.to_string();
        assert!(
            s.contains("request body aborted"),
            "Display must describe the abort cause, got: {s}"
        );
        let dyn_err: &dyn std::error::Error = &e;
        assert!(dyn_err.source().is_none());
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(H3ReqAbort);
        assert!(boxed.to_string().contains("client RESET"));
    }

    /// `h2_request_body_from_rx` head-construction arms not otherwise reached.
    #[tokio::test]
    async fn g5_h2_request_body_from_rx_head_construction_arms() {
        // (a) empty authority ⇒ addr fallback; a `:`-pseudo header is SKIPPED
        // while a regular header is copied. Bodyless, so nothing is dialled.
        let req = H3Request {
            method: "GET".to_string(),
            path: "/x".to_string(),
            authority: String::new(), // ← empty ⇒ addr fallback
            extra: vec![
                (":scheme".to_string(), "https".to_string()), // pseudo ⇒ skip
                ("x-keep".to_string(), "1".to_string()),      // regular ⇒ copy
            ],
            trailers: Vec::new(),
        };
        let addr: std::net::SocketAddr = "127.0.0.1:65000".parse().unwrap();
        let (_tx, rx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(1);
        let built = h2_request_body_from_rx(
            &req,
            addr,
            rx,
            Some(ReqBodyEvent::End {
                trailers: Vec::new(),
            }),
        )
        .expect("empty-authority bodyless request must build");
        assert_eq!(
            built.uri().authority().map(ToString::to_string),
            Some("127.0.0.1:65000".to_string()),
            "empty :authority must fall back to addr"
        );
        assert_eq!(
            built.headers().get("x-keep").map(|v| v.to_str().unwrap()),
            Some("1"),
            "regular header must be copied"
        );
        assert!(
            built.headers().get(":scheme").is_none(),
            "pseudo-header must be skipped (not copied as a real header)"
        );

        let req2 = H3Request {
            method: "GET".to_string(),
            path: "/y".to_string(),
            authority: "explicit.host:443".to_string(),
            extra: Vec::new(),
            trailers: Vec::new(),
        };
        let (_tx2, rx2) = tokio::sync::mpsc::channel::<ReqBodyEvent>(1);
        let built2 =
            h2_request_body_from_rx(&req2, addr, rx2, None).expect("bodyless (None) must build");
        assert_eq!(
            built2.uri().authority().map(ToString::to_string),
            Some("explicit.host:443".to_string())
        );
    }

    /// `h3_to_h2_stream_resp`'s pre-dial inline arms: a pre-data `Reset` ⇒ 413
    /// and a builder failure ⇒ 502, both returning before the pool is dialled.
    #[tokio::test]
    async fn g5_h3_to_h2_stream_resp_inline_413_and_502_no_dial() {
        let pool = lb_io::http2_pool::Http2Pool::new(
            lb_io::http2_pool::Http2PoolConfig::default(),
            lb_io::pool::TcpPool::new(
                lb_io::pool::PoolConfig::default(),
                lb_io::sockopts::BackendSockOpts::default(),
                lb_io::Runtime::new(),
            ),
        );
        let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();

        let req = H3Request {
            method: "POST".to_string(),
            path: "/p".to_string(),
            authority: "h.test".to_string(),
            extra: Vec::new(),
            trailers: Vec::new(),
        };
        let (btx, brx) = tokio::sync::mpsc::channel::<ReqBodyEvent>(2);
        btx.send(ReqBodyEvent::Reset).await.unwrap();
        let (rtx, mut rrx) = tokio::sync::mpsc::channel::<RespEvent>(8);
        let r = h3_to_h2_stream_resp(&req, addr, &pool, brx, rtx, MAX_RESPONSE_BODY_BYTES).await;
        assert!(r.is_ok(), "pre-data Reset path returns Ok(())");
        let mut saw_end = false;
        let mut head_status: Option<u16> = None;
        while let Ok(ev) = rrx.try_recv() {
            match ev {
                RespEvent::Head { status, .. } => head_status = Some(status),
                RespEvent::Body(_) | RespEvent::Trailers(_) => {}
                RespEvent::End => saw_end = true,
                RespEvent::Reset => panic!("413 path must not Reset"),
            }
        }
        assert!(saw_end, "inline path must emit End");
        assert_eq!(head_status, Some(413), "pre-data Reset ⇒ inline 413");

        // --- 502 arm: an invalid method byte makes the builder fail ⇒ 502 ---
        let bad = H3Request {
            method: "BAD METHOD WITH SPACES".to_string(),
            path: "/p".to_string(),
            authority: "h.test".to_string(),
            extra: Vec::new(),
            trailers: Vec::new(),
        };
        let (btx2, brx2) = tokio::sync::mpsc::channel::<ReqBodyEvent>(2);
        btx2.send(ReqBodyEvent::End {
            trailers: Vec::new(),
        })
        .await
        .unwrap();
        let (rtx2, mut rrx2) = tokio::sync::mpsc::channel::<RespEvent>(8);
        let r2 = h3_to_h2_stream_resp(&bad, addr, &pool, brx2, rtx2, MAX_RESPONSE_BODY_BYTES).await;
        assert!(r2.is_ok(), "builder-failure path returns Ok(())");
        let mut head_status2: Option<u16> = None;
        let mut saw_end2 = false;
        while let Ok(ev) = rrx2.try_recv() {
            match ev {
                RespEvent::Head { status, .. } => head_status2 = Some(status),
                RespEvent::Body(_) | RespEvent::Trailers(_) => {}
                RespEvent::End => saw_end2 = true,
                RespEvent::Reset => {}
            }
        }
        assert!(saw_end2, "inline 502 must emit End");
        assert_eq!(head_status2, Some(502), "builder failure ⇒ inline 502");
        assert_eq!(pool.peer_count(), 0, "no upstream dial on inline arms");
    }

    /// Happy-path chunked decode across a split feed.
    #[test]
    fn chunk_decoder_decodes_split_chunks() {
        let mut dec = ChunkDecoder::new();
        let mut out = Vec::new();
        dec.feed(b"4\r\nWik", &mut out).unwrap();
        dec.feed(b"i\r\n5\r\npedia\r\n", &mut out).unwrap();
        assert!(!dec.done);
        dec.feed(b"0\r\n", &mut out).unwrap();
        assert!(dec.done);
        assert_eq!(out, b"Wikipedia");
    }

    /// C3: every malformed chunked framing ⇒ `ChunkedDecode`, never a truncated
    /// or forwarded body presented as complete.
    #[test]
    fn chunk_decoder_rejects_malformed_framing_c3() {
        // (a) non-hex chunk size.
        let mut d = ChunkDecoder::new();
        assert_eq!(
            d.feed(b"zz\r\nabc\r\n", &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );
        // (b) empty chunk-size token.
        let mut d = ChunkDecoder::new();
        assert_eq!(
            d.feed(b"\r\nabc\r\n", &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );
        // (c) wrong byte where the post-body CRLF must be.
        let mut d = ChunkDecoder::new();
        let mut o = Vec::new();
        assert_eq!(d.feed(b"3\r\nabcXX", &mut o), Err(RespAbort::ChunkedDecode));
        // (d) chunk-size line longer than the smuggling-guard cap.
        let mut d = ChunkDecoder::new();
        let huge = format!("{}\r\n", "1".repeat(MAX_CHUNK_SIZE_LINE + 8));
        assert_eq!(
            d.feed(huge.as_bytes(), &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );
    }

    /// A chunk extension is tolerated; mis-parsing the size past `;` would
    /// frame the body wrong (smuggling-relevant).
    #[test]
    fn chunk_decoder_tolerates_chunk_extension() {
        let mut dec = ChunkDecoder::new();
        let mut out = Vec::new();
        dec.feed(b"4;name=value\r\nbody\r\n0\r\n", &mut out)
            .unwrap();
        assert!(dec.done);
        assert_eq!(out, b"body");
    }

    /// C4: `done` (zero-size chunk) is distinct from `complete` (trailer section
    /// + CRLF consumed). PC-2: coalesced and split sections must parse alike.
    #[test]
    fn chunk_decoder_parses_trailer_section_c4() {
        let mut d = ChunkDecoder::new();
        let mut o = Vec::new();
        d.feed(
            b"3\r\nabc\r\n0\r\nx-checksum: deadbeef\r\nx-two: v2\r\n\r\n",
            &mut o,
        )
        .unwrap();
        assert!(d.done && d.complete, "trailer section consumed");
        assert_eq!(o, b"abc");
        assert_eq!(
            d.take_trailers(),
            vec![
                ("x-checksum".to_string(), "deadbeef".to_string()),
                ("x-two".to_string(), "v2".to_string()),
            ]
        );

        let mut d = ChunkDecoder::new();
        let mut o = Vec::new();
        d.feed(b"3\r\nabc\r\n0\r\n", &mut o).unwrap();
        assert!(d.done && !d.complete, "awaiting trailer section");
        d.feed(b"x-checksum: dead", &mut o).unwrap();
        assert!(!d.complete);
        d.feed(b"beef\r\n", &mut o).unwrap();
        d.feed(b"\r\n", &mut o).unwrap();
        assert!(d.complete);
        assert_eq!(o, b"abc");
        assert_eq!(
            d.take_trailers(),
            vec![("x-checksum".to_string(), "deadbeef".to_string())]
        );

        let mut d = ChunkDecoder::new();
        let mut o = Vec::new();
        d.feed(b"3\r\nabc\r\n0\r\n\r\n", &mut o).unwrap();
        assert!(d.complete);
        assert_eq!(o, b"abc");
        assert!(d.take_trailers().is_empty());

        // (d) C3/C4 parity — junk (a no-colon line) after the terminator.
        let mut d = ChunkDecoder::new();
        assert_eq!(
            d.feed(b"3\r\nabc\r\n0\r\nthis-is-junk\r\n\r\n", &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );

        // (e) a `:`-prefixed pseudo-header in the trailer section (§4.3).
        let mut d = ChunkDecoder::new();
        assert_eq!(
            d.feed(b"0\r\n:status: 200\r\n\r\n", &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );

        // (f) an oversized trailer section (MAX_TRAILER_SECTION).
        let mut d = ChunkDecoder::new();
        let mut huge = Vec::from(&b"0\r\n"[..]);
        huge.extend_from_slice(b"x-big: ");
        huge.extend(std::iter::repeat_n(b'A', MAX_TRAILER_SECTION + 16));
        assert_eq!(
            d.feed(&huge, &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );
    }

    /// Pure unit proof of the request send-half decision table, driving the
    /// REAL `j2_req_event_action` the park arm calls.
    #[test]
    fn s7_j2_request_send_decision() {
        // (a) Chunk ⇒ SendData carrying the RAW bytes verbatim. (a)-(d) use
        // forward_req_trailers=false: the H3→H3 drop semantics.
        let payload = vec![0x5Au8; H3_BODY_CHUNK_MAX]; // non-trivial, max-size
        let act = j2_req_event_action(
            Some(ReqBodyEvent::Chunk(Bytes::from(payload.clone()))),
            false,
        );
        match act {
            J2ReqAction::SendData(bytes) => {
                assert_eq!(
                    bytes.as_ref(),
                    &payload[..],
                    "Chunk forwards the RAW chunk bytes verbatim (send_body frames)"
                );
            }
            other => panic!("Chunk ⇒ SendData, got {other:?}"),
        }
        // An empty chunk is still SendData — a zero-length DATA frame is valid.
        assert!(matches!(
            j2_req_event_action(Some(ReqBodyEvent::Chunk(Bytes::new())), false),
            J2ReqAction::SendData(_)
        ));

        // (b) End ⇒ FIN; the action carries NO trailer payload.
        assert_eq!(
            j2_req_event_action(
                Some(ReqBodyEvent::End {
                    trailers: vec![("x-trailer".into(), "v".into())],
                }),
                false,
            ),
            J2ReqAction::FinNoTrailers,
            "End ⇒ FIN; trailers are not forwarded on the H3→H3 leg"
        );

        // (c) mid-body Reset ⇒ abort WITHOUT FIN (BINDING case-7).
        assert_eq!(
            j2_req_event_action(Some(ReqBodyEvent::Reset), false),
            J2ReqAction::AbortNoFin,
            "mid-body Reset MUST abort the upstream request with NO FIN"
        );

        // (d) closed channel before End ⇒ the identical abort, no FIN.
        assert_eq!(
            j2_req_event_action(None, false),
            J2ReqAction::AbortNoFin,
            "premature channel close MUST abort with NO FIN (truncation guard)"
        );

        // (e) forward_req_trailers=true (L7 fronts): non-empty ⇒ FinWithTrailers,
        // empty ⇒ bare FIN. The abort guards are UNAFFECTED by the flag.
        assert_eq!(
            j2_req_event_action(
                Some(ReqBodyEvent::End {
                    trailers: vec![("x-trailer".into(), "v".into())],
                }),
                true,
            ),
            J2ReqAction::FinWithTrailers(vec![("x-trailer".into(), "v".into())]),
            "forward=true + non-empty End{{trailers}} ⇒ FinWithTrailers"
        );
        assert_eq!(
            j2_req_event_action(Some(ReqBodyEvent::End { trailers: vec![] }), true),
            J2ReqAction::FinNoTrailers,
            "forward=true + EMPTY End ⇒ bare FIN (no empty trailers frame)"
        );
        assert_eq!(
            j2_req_event_action(Some(ReqBodyEvent::Reset), true),
            J2ReqAction::AbortNoFin,
            "forward=true does NOT weaken the mid-body truncation guard"
        );
        assert_eq!(
            j2_req_event_action(None, true),
            J2ReqAction::AbortNoFin,
            "forward=true does NOT weaken the premature-close truncation guard"
        );
    }

    /// The `Decoded` sink MUST surface an upstream trailing field section as
    /// `H3RespEvent::Trailers` — a connector trailer-DROP would otherwise slip.
    #[tokio::test]
    async fn s12_decoded_sink_on_trailers_emits_h3respevent_trailers() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<H3RespEvent>(4);
        let mut sink = H3RespOut::Decoded {
            tx,
            total: 0,
            cap: MAX_RESPONSE_BODY_BYTES,
        };
        let trailers = vec![
            ("grpc-status".to_string(), "0".to_string()),
            ("x-trailer".to_string(), "v1".to_string()),
        ];
        let r = sink.on_trailers(trailers.clone()).await;
        assert!(r.is_ok(), "on_trailers with a live channel returns Ok");
        match rx.try_recv() {
            Ok(H3RespEvent::Trailers(got)) => assert_eq!(
                got, trailers,
                "the Decoded sink must surface the upstream response trailers \
                 verbatim as H3RespEvent::Trailers"
            ),
            other => panic!("expected H3RespEvent::Trailers, got {other:?}"),
        }
    }

    /// The `Decoded` sink's `on_head` MUST forward the FULL non-pseudo set
    /// (pseudo-headers filtered, `content-length` kept as a regular header).
    #[tokio::test]
    async fn s12_decoded_sink_on_head_forwards_full_nonpseudo_set() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<H3RespEvent>(4);
        let mut sink = H3RespOut::Decoded {
            tx,
            total: 0,
            cap: MAX_RESPONSE_BODY_BYTES,
        };
        let fields = vec![
            (":status".to_string(), "200".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("content-length".to_string(), "12".to_string()),
            ("x-eg-resp".to_string(), "round-trip".to_string()),
        ];
        let r = sink.on_head(&fields).await;
        assert!(r.is_ok(), "on_head with a live channel returns Ok");
        match rx.try_recv() {
            Ok(H3RespEvent::Head { status, headers }) => {
                assert_eq!(status, 200, ":status parsed out of the field list");
                assert_eq!(
                    headers,
                    vec![
                        ("content-type".to_string(), "application/json".to_string()),
                        ("content-length".to_string(), "12".to_string()),
                        ("x-eg-resp".to_string(), "round-trip".to_string()),
                    ],
                    "the Decoded sink forwards the full non-pseudo set in order \
                     (pseudo-headers filtered, content-length retained as a \
                     regular header)"
                );
            }
            other => panic!("expected H3RespEvent::Head, got {other:?}"),
        }
    }
}
