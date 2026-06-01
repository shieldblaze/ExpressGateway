//! Minimal H3 ↔ H1 bridge for Pillar 3b.3c-2.
//!
//! This module takes a single established [`quiche::Connection`] and
//! drives HTTP/3 request/response termination for each readable
//! bidi-stream: decode request HEADERS via lb-h3's
//! [`QpackDecoder::decode`] + [`decode_frame`], forward to a plain
//! HTTP/1.1 backend through [`lb_io::TcpPool`], and write the response
//! HEADERS + DATA back via [`lb_h3::QpackEncoder::encode`] +
//! [`encode_frame`].
//!
//! Scope: only headers that are present in the RFC 9204 QPACK static
//! table (directly or by name reference) — this sidesteps lb-h3's
//! literal-with-literal-name encoding path, which differs from quiche's
//! wire format. The e2e test exercises exactly `:method GET`, `:scheme
//! https`, `:path /`, `:authority <dns_name>` → `:status 200`,
//! `content-length N` — all static-table names or indexed entries.
//!
//! Non-goals for 3b.3c-2: request bodies (POST with payload), trailers,
//! header-value coercion, content-length negotiation beyond echoing
//! what the backend returns. Those land in 3b.3b when the real hyper/
//! tonic servers arrive.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::BoxBody;
use hyper::Request;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

/// SESSION 2 / P1-A: maximum total request-body size the streaming
/// bridge will forward before it returns H3 `413`. This is the
/// *total-size* cap (a request-correctness limit), NOT the
/// memory-safety mechanism — memory safety comes from the bounded
/// in-flight body channel (`H3_BODY_CHANNEL_DEPTH` * <=8 KiB chunks).
// TODO(s3): wire into listener/actor config.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

/// SESSION 2 / P1-C: maximum total H1-RESPONSE size (status line +
/// headers + body) the bridge will buffer from the upstream before it
/// gives up and returns a clean H3 `502`. This is a memory-safety
/// ceiling ONLY: today `read_h1_response` reads the whole upstream
/// response to EOF into one `Vec` (FULLY BUFFERED — see
/// `audit/h3-program/p1c-response-streaming-assessment.md`), so a
/// malicious / mis-configured upstream returning a multi-GB body could
/// OOM the proxy. The cap (64 MiB, mirroring `MAX_REQUEST_BODY_BYTES`)
/// bounds that. It is NOT incremental egress and NOT a backpressure
/// mechanism — genuine incremental response egress is the headline
/// Session 3 item. Every existing test's response body is <= 256 KiB,
/// `<<` this ceiling, so the cap changes no observable behaviour for
/// any conformant response.
// TODO(s3): config + incremental egress (replace this buffer-and-cap
// with a channel back into the actor + progressive `StreamTx`).
pub const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// SESSION 2 / P1-A: largest single `ReqBodyEvent::Chunk` the body
/// phase machine emits. With `H3_BODY_CHANNEL_DEPTH` this bounds the
/// max in-flight bytes (≈ depth * chunk ≈ 64 KiB) INDEPENDENT of the
/// total body size — a DATA frame larger than this is split.
pub const H3_BODY_CHUNK_MAX: usize = 8 * 1024;

/// SESSION 7 / J2 (Q-J2, lead-ruled): the HTTP/3 application error code
/// the H3→H3 connector puts on the **request-leg** stream when it
/// aborts the upstream request without FIN (mid-body client RESET, or
/// the request-body producer dropped before a clean `End`).
///
/// `H3_REQUEST_CANCELLED = 0x010c` (RFC 9114 §8.1: "the request or its
/// response ... is cancelled") is the conformant code HERE because on
/// the request leg the proxy IS the client toward the upstream: the
/// downstream client going away genuinely cancels the request the
/// proxy initiated upstream. This is deliberately the OPPOSITE choice
/// from the *response* leg: [`crate::conn_actor::H3_INTERNAL_ERROR`]
/// (`0x0102`, see `conn_actor.rs:73`) is used when the proxy
/// (acting as *server* toward the downstream client) RESETs the
/// client stream on an aborted response — there, a peer-cancelled
/// (`0x010c`) code would misattribute a gateway-internal failure to
/// the client. The two legs use different codes ON PURPOSE
/// (proxy-as-client vs proxy-as-server); this asymmetry is correct
/// per RFC 9114 §8.1 and must NOT be "fixed" to a false consistency.
const H3_REQUEST_CANCELLED: u64 = 0x010c;

/// SESSION 2 / P1-A FIX: hard cap on the partial frame-header bytes the
/// body-phase parser will accumulate before BOTH the frame-type varint
/// and the length varint decode. Two QUIC varints are at most 8 bytes
/// each (RFC 9000 §16), so a well-formed frame header is ≤ 16 bytes;
/// anything larger is malformed framing → Reset.
pub const MAX_FRAME_HEADER_BYTES: usize = 16;

/// SESSION 2 / P1-A FIX: hard cap on a body-phase trailing HEADERS
/// (RFC 9114 §4.1) QPACK field block. The QPACK decoder needs the whole
/// block buffered to decode, so unlike DATA this MUST be accumulated —
/// but bounded so a hostile/oversized trailer block cannot grow memory
/// without limit. 64 KiB is far above any realistic trailer section.
pub const MAX_TRAILER_BLOCK_BYTES: usize = 64 * 1024;

/// SESSION 2 / P1-A: event forwarded over the per-stream bounded body
/// channel from `conn_actor::poll_h3` to [`h3_to_h1_stream`].
#[derive(Debug, Clone)]
pub enum ReqBodyEvent {
    /// A bounded request-body chunk.
    Chunk(Bytes),
    /// End of request body. `trailers` is the RFC 9114 §4.1 trailing
    /// field section (empty when none).
    End {
        /// Request trailers (post-DATA HEADERS frame); empty if none.
        trailers: Vec<(String, String)>,
    },
    /// The stream was reset / aborted before a clean end — the egress
    /// task must abort the upstream and fail the request.
    Reset,
}

/// SESSION 4 / P1-A: depth of the per-stream bounded RESPONSE channel
/// from the `stream_h1_response` producer task back into the actor.
/// Mirrors [`H3_BODY_CHANNEL_DEPTH`](crate::conn_actor::H3_BODY_CHANNEL_DEPTH).
/// Memory safety = this depth × [`H3_RESP_CHUNK_MAX`] (+ frame header),
/// INDEPENDENT of total response size and of [`MAX_RESPONSE_BODY_BYTES`].
pub const H3_RESP_CHANNEL_DEPTH: usize = 8;

/// SESSION 4 / P1-A: largest response-body slice a single DATA frame in
/// a [`RespEvent::Bytes`] carries. Mirrors [`H3_BODY_CHUNK_MAX`]. A
/// larger upstream read is split into ≤ this many payload bytes per
/// frame so in-flight memory stays bounded regardless of body size.
pub const H3_RESP_CHUNK_MAX: usize = 8 * 1024;

/// SESSION 4 / P1-A: a `RespEvent::Bytes` carries a PRE-ENCODED H3
/// frame — a ≤ [`H3_RESP_CHUNK_MAX`] payload PLUS a small frame header
/// (type + length QUIC varints). A well-formed H3 frame header is ≤ 16
/// bytes (two varints, RFC 9000 §16); this is the same bound as the
/// existing [`MAX_FRAME_HEADER_BYTES`] and is re-exported under a
/// response-side name so the §1.5 memory gauge can OVER-estimate
/// channel occupancy (never under — soundness parity with the
/// request-side gauge).
pub const H3_FRAME_HDR_MAX: usize = MAX_FRAME_HEADER_BYTES;

/// SESSION 7 / F-S7-6: the H3→H3 upstream connector's
/// **NO-FORWARD-PROGRESS idle deadline** — the maximum time
/// [`h3_to_h3_stream_resp`] will wait with ZERO bidirectional
/// application-data progress before aborting the exchange.
///
/// This is explicitly **NOT a wall-clock response cap**. It replaces
/// the original hardcoded `Instant::now() + Duration::from_secs(5)`
/// wall-clock deadline (J1), which truncated a valid, actively-
/// progressing large/slow response at exactly 5 s regardless of
/// progress (a verified defect — an 8 MiB response cut off at
/// ~4.37 MiB). The idle deadline is RESET on every forward-progress
/// event (response stream_recv with n>0 ingress, OR a successful
/// `resp_tx` relay egress, OR a request-DATA `stream_send` with n>0 /
/// the request FIN egress — R-S76-6 bidirectional), so a legitimately
/// slow-but-progressing response OR a large/slow request upload never
/// trips it; only the genuine ABSENCE of all progress for this window
/// fires it. It is NEVER reset by transport keepalive/ACK, the quiche
/// idle timer, zero-byte reads, or backpressure parks (R-S76-5), so a
/// dead-but-connected upstream is still aborted within this bound (no
/// infinite hang) — a deadline-truncated partial is returned as
/// `Err(RespAbort::PrematureEof)` + `Reset`, NEVER `RespEvent::End`
/// (response-splitting guard, post-loop disposition unchanged).
///
/// Sized at 30 s (the same magnitude as
/// [`request_h3_upstream`]'s total budget) but applied as IDLE, not
/// wall-clock. NOTE: `request_h3_upstream`'s own 30 s is a *fixed
/// wall-clock* cap with the SAME latent truncation bug — a separate
/// carry-forward (CF-S7-RHU), an `H1→H3`/`H2→H3` R3 boundary, and is
/// intentionally NOT fixed here.
pub const H3_RESP_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// SESSION 4 / P1-A: one unit of the bounded response pipe from the
/// upstream reader task ([`stream_h1_response`] / [`stream_h2_response`]
/// / the H3→H3 wire connector) back to the actor.
///
/// SESSION 24 / INC-3: this is now a **DECODED** event (not pre-encoded
/// H3 wire bytes). The actor's `quiche::h3::Connection` owns ALL H3
/// framing (HEADERS / DATA / trailing-HEADERS via
/// `send_response`/`send_body`/`send_additional_headers`), so the
/// producers hand back the decoded head / body chunks / trailers and
/// the actor encodes. The producer still owns the hop-by-hop strip,
/// content-length management, and the R8 body chunking (≤
/// [`H3_RESP_CHUNK_MAX`] per `Body`). Ordering contract: exactly one
/// [`Head`](Self::Head) first, then zero or more [`Body`](Self::Body)
/// chunks, then an OPTIONAL [`Trailers`](Self::Trailers), then
/// [`End`](Self::End); on ANY abort a single [`Reset`](Self::Reset) and
/// NEVER `End`.
#[derive(Debug, Clone)]
pub enum RespEvent {
    /// The response head. `status` is the `:status`; `headers` is the
    /// hop-by-hop-stripped, content-length-managed non-pseudo field
    /// list. Emitted exactly once, before any `Body`.
    Head {
        /// Parsed response status code.
        status: u16,
        /// Decoded non-pseudo response headers (hop-by-hop stripped).
        headers: Vec<(String, String)>,
    },
    /// A decoded response-body chunk (≤ [`H3_RESP_CHUNK_MAX`],
    /// producer-split — the R8 bound).
    Body(Bytes),
    /// The RFC 9114 §4.1 trailing field section (post-DATA HEADERS),
    /// hop-by-hop stripped. Emitted only when non-empty, after the last
    /// `Body` and before `End`.
    Trailers(Vec<(String, String)>),
    /// All response events delivered — the actor sets FIN on the client
    /// stream.
    End,
    /// Abort: the actor RESET_STREAMs the client (never FIN). Emitted
    /// for upstream reset / premature EOF / chunked-decode error /
    /// over-cap / bad head / client cancel.
    Reset,
}

/// SESSION 12 / CF-DEDUP-2 — a **DECODED** upstream-H3 response event
/// produced by the shared streaming connector
/// [`stream_request_to_h3_upstream`] for an HTTP/1.1 or HTTP/2 *front*.
///
/// Unlike [`RespEvent`] (which carries PRE-ENCODED H3 wire frames bound
/// straight for an H3 client), this carries the QPACK-/frame-DECODED
/// response so a non-H3 listener (`H1Proxy` / `H2Proxy`) can run its
/// own response head-transform + stream the body to its own wire
/// format — WITHOUT re-decoding H3 frames it never produced (wrong
/// layer; would re-introduce buffering in `lb-l7`). The H3→H3 cell
/// keeps the [`RespEvent`] wire-bytes path (see [`H3RespOut`]).
///
/// Ordering contract (mirrors the wire path's emit order): exactly one
/// [`Head`](Self::Head) FIRST, then zero or more [`Body`](Self::Body)
/// chunks (each ≤ [`H3_RESP_CHUNK_MAX`], the in-flight window bounded
/// by [`H3_RESP_CHANNEL_DEPTH`]), then an OPTIONAL
/// [`Trailers`](Self::Trailers) (post-DATA trailing field section,
/// emitted only when non-empty), then [`End`](Self::End). On ANY abort
/// a single [`Reset`](Self::Reset) is emitted and NEVER `End` — the
/// caller must drop / RESET its client and never finalize a partial
/// (response-splitting / cache-poisoning guard, parity with the wire
/// path's [`RespAbort`] contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H3RespEvent {
    /// The response head. `status` is the parsed `:status`; `headers`
    /// is the FULL decoded non-pseudo response field list
    /// (`content-length` passed through as a regular header).
    /// Pseudo-headers are filtered out. Emitted exactly once, before
    /// any `Body`.
    Head {
        /// Parsed `:status` pseudo-header.
        status: u16,
        /// Decoded non-pseudo response headers (pseudo-headers
        /// filtered; `content-length` retained as a regular header).
        headers: Vec<(String, String)>,
    },
    /// A decoded response-body chunk (≤ [`H3_RESP_CHUNK_MAX`]).
    Body(Bytes),
    /// The RFC 9114 §4.1 trailing field section (post-DATA HEADERS
    /// frame), pseudo-headers filtered. Emitted only when non-empty,
    /// after the last `Body` and before `End`.
    Trailers(Vec<(String, String)>),
    /// Clean stream end — the caller finalizes its client response.
    /// NEVER emitted on a partial / aborted response.
    End,
    /// Abort — the caller drops / RESETs its client and never
    /// finalizes (mirror of [`RespEvent::Reset`]).
    Reset,
}

/// SESSION 4 / P1-A: why [`stream_h1_response`] aborted. EVERY variant
/// maps to a single client-facing outcome — emit [`RespEvent::Reset`]
/// (best-effort) and return `Err(RespAbort)` — so the actor
/// RESET_STREAMs the client and NEVER sets FIN. A partial body is
/// therefore never presentable as a complete response (response-
/// splitting / cache-poisoning guard; parity with the request-side
/// P1-B abort). The caller MUST mark the pooled upstream connection
/// NON-reusable on every variant (approval condition C2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespAbort {
    /// Upstream socket reset / read error mid-response.
    UpstreamReset,
    /// Socket EOF before the declared `Content-Length` was satisfied.
    PrematureEof,
    /// `Transfer-Encoding: chunked` decode error, or EOF before the
    /// chunked terminator.
    ChunkedDecode,
    /// Total response exceeded the cap ([`MAX_RESPONSE_BODY_BYTES`]).
    OverCap,
    /// HEADERS parse failure / head exceeded the head cap before the
    /// `\r\n\r\n` terminator.
    BadHead,
    /// The response channel was closed by the actor (the H3 client
    /// cancelled the stream) — stop reading the upstream.
    ClientGone,
}

/// SESSION 2 / P1-A FIX (test-gauge): the maximum, observed at any
/// instant, of the TOTAL per-stream request-body memory the proxy
/// retains while a body is in flight — i.e. the `StreamRxBuf` internal
/// buffer + every byte still queued in `body_pending` for the stream +
/// the bounded channel occupancy. Unlike [`MAX_INFLIGHT_BODY_BYTES`]
/// (which only ever sees already-split ≤8 KiB chunks in the egress),
/// this captures the buffers UPSTREAM of the split, so it FAILS if the
/// body-phase decoder buffers a whole DATA frame. Recorded in
/// `conn_actor` right after `feed_body` decode and before flush — the
/// point where these buffers are largest.
#[cfg(any(test, feature = "test-gauges"))]
pub static MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// SESSION 2 / P1-A FIX (test-gauge): max-update for
/// [`MAX_RETAINED_BODY_BYTES`].
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

/// SESSION 4 / P1-B (§1.5 non-vacuous memory proof — approval
/// condition C5): the maximum, observed at any instant, of the TOTAL
/// per-stream RESPONSE memory the proxy retains while a response is in
/// flight — i.e. Σ over streams of the `Progressive` `StreamTx` queued
/// bytes + the bounded response-channel occupancy (an UPPER bound;
/// soundness parity with [`MAX_RETAINED_BODY_BYTES`]). Recorded in
/// `conn_actor` right after the §1.4.3 gate refills the `Progressive`
/// `StreamTx`s from the channels and BEFORE `drain_streams_to_conn` —
/// the point where these buffers are largest. A whole-response
/// buffering implementation would make this grow with response size;
/// the bounded channel + the empty-queue gate keep it ≈
/// `H3_RESP_CHANNEL_DEPTH × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)`,
/// independent of total response size and of [`MAX_RESPONSE_BODY_BYTES`].
#[cfg(any(test, feature = "test-gauges"))]
pub static MAX_RETAINED_RESP_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// SESSION 4 / P1-B (test-gauge): max-update for
/// [`MAX_RETAINED_RESP_BYTES`]. Identical CAS-max to
/// [`record_retained`].
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
    /// Non-pseudo headers. Preserved for 3b.3b request-body & header
    /// forwarding; currently not emitted into the H1 request since
    /// the H1 backend path only knows `Host` + `Content-Length`.
    #[allow(dead_code)]
    pub extra: Vec<(String, String)>,
    /// PROTO-2-12: HTTP/3 trailing field section (RFC 9114 §4.1) — a
    /// second HEADERS frame sent after the request DATA frames. Empty
    /// when the request carries no trailers. Populated by the proxy
    /// hot path (`collect_h{1,2}_request_to_h3_fieldlist`) and shipped
    /// on the wire by [`request_h3_upstream`].
    pub trailers: Vec<(String, String)>,
}

impl Default for H3Request {
    /// Mirrors the [`H3Request::from_headers`] missing-pseudo defaults
    /// (method `GET`, path `/`, empty authority) so a defaulted value
    /// is wire-coherent rather than carrying empty pseudo-headers.
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
    /// Extract pseudo-headers from a QPACK-decoded field list. Missing
    /// pseudo-headers are defaulted sensibly for the e2e (method=GET,
    /// path=/, authority=""); this is deliberate — 3b.3c-2 does NOT
    /// do full RFC 9110 validation.
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
            // RFC 9114 §4.1: request trailers arrive in a *second*
            // HEADERS frame after DATA, not in the initial field
            // block parsed here — so this is empty at request-head
            // decode time. The proxy hot path threads inbound H1/H2
            // trailers into `request_h3_upstream` directly.
            trailers: Vec::new(),
        }
    }
}

/// SESSION 22 (h3spec #12–15) — RFC 9114 §4.3 + §4.3.1 request
/// pseudo-header validation. Returns `Err(reason)` on the FIRST
/// violation; the caller (the single H3 ingress site in
/// [`crate::conn_actor::poll_h3`]) resets the request stream with
/// `H3_MESSAGE_ERROR`.
///
/// A malformed request is a **stream** error of type `H3_MESSAGE_ERROR`
/// (RFC 9114 §4.1.3), not a connection error — so the offending stream
/// is reset and the rest of the connection survives. Crucially this runs
/// BEFORE [`H3Request::from_headers`] (which silently defaults missing
/// pseudo-headers) and BEFORE any upstream is dialled, so a malformed
/// request is never forwarded to a backend (request integrity — a
/// smuggling / desync guard; the H3 analogue of the H2-path checks that
/// pass h2spec 146/147).
///
/// Single-sourced: validating here at the sole ingress covers every
/// H3-front cell (H3→H1, H3→H2, H3→H3), which all share this decode path
/// (R12). Mode B (raw QUIC relay) never parses H3 frames, so it is N/A.
///
/// Enforces, per `audit/h3spec/s22-findings.md`:
/// * **#12** no request pseudo-header is duplicated (§4.3.1)
/// * **#13** the mandatory pseudo-headers are present — `:method`,
///   `:scheme`, `:path` for a normal request; `:authority` for CONNECT
///   (§4.3.1 / §4.4); and for a scheme with a mandatory authority
///   component (`http`/`https`) the request MUST carry `:authority` or a
///   `Host` field (§4.3.1). The owner ruled this STRICT (the prior
///   absent-`:authority` SNI-substitution was coverage-only lenience, not
///   a deployment feature — see the findings doc).
/// * **#14** no prohibited or unknown request pseudo-header is present —
///   e.g. the response-only `:status`, or any unregistered `:`-prefixed
///   name (§4.3)
/// * **#15** no pseudo-header appears after a regular field (§4.3)
///
/// # Errors
/// Returns a static reason string naming the RFC clause violated.
pub fn validate_request_pseudo_headers(headers: &[(String, String)]) -> Result<(), &'static str> {
    let mut method: Option<&str> = None;
    let mut scheme: Option<&str> = None;
    let mut seen_path = false;
    let mut seen_authority = false;
    let mut seen_host = false;
    let mut seen_regular = false;

    for (name, value) in headers {
        if name.starts_with(':') {
            // #15 — all pseudo-header fields MUST precede the regular
            // fields (RFC 9114 §4.3).
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
                // #14 — any other `:`-prefixed name (the response-only
                // `:status`, or an unregistered pseudo-header) is
                // prohibited in a request (RFC 9114 §4.3).
                _ => {
                    return Err("h3 prohibited/unknown request pseudo-header (RFC 9114 §4.3)");
                }
            }
        } else {
            seen_regular = true;
            // Track a `Host` field (case-insensitive) as the §4.3.1
            // alternative to `:authority`.
            if name.eq_ignore_ascii_case("host") {
                seen_host = true;
            }
        }
    }

    // #13 — mandatory pseudo-headers. RFC 9114 §4.3.1: a normal request
    // MUST include exactly one each of :method, :scheme and :path. §4.4:
    // a CONNECT request omits :scheme and :path and MUST include
    // :authority. (CONNECT is not otherwise supported by the gateway, but
    // the validation is kept RFC-correct so a future CONNECT path is not
    // pre-broken here.)
    match method {
        None => Err("h3 missing mandatory :method pseudo-header (RFC 9114 §4.3.1)"),
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
            let Some(scheme) = scheme else {
                return Err("h3 missing mandatory :scheme pseudo-header (RFC 9114 §4.3.1)");
            };
            if !seen_path {
                return Err("h3 missing mandatory :path pseudo-header (RFC 9114 §4.3.1)");
            }
            // §4.3.1: for a scheme with a mandatory authority component
            // (http/https) the request MUST carry :authority OR a Host
            // field. SESSION 22 #13 (owner ruling: strict).
            let mandatory_authority =
                scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http");
            if mandatory_authority && !seen_authority && !seen_host {
                return Err("h3 http/https request missing :authority or Host (RFC 9114 §4.3.1)");
            }
            Ok(())
        }
    }
}

/// Build a minimal HTTP/1.1 request line + headers.
///
/// S1-B seam (SESSION 1): `body` threads an OPTIONAL request payload so
/// SESSION 2's request-body forwarding has a stable signature to fill
/// in. **Behaviour-preserving contract:** `body == None` produces the
/// exact bytes the prior bodyless implementation produced
/// (`Content-Length: 0` + `Connection: close`) — the only caller today
/// (`h3_to_h1_roundtrip`) passes `None`, so no on-wire behaviour
/// changes this session. When `Some(bytes)` is passed, the request
/// head carries the correct `Content-Length: <bytes.len()>` and the
/// payload is appended after the header terminator; that path is NOT
/// exercised on the datapath in SESSION 1 (the connection actor does
/// not yet accumulate inbound DATA frames — that is SESSION 2 work in
/// `conn_actor::poll_h3`). See the marker test below (it is
/// `#[ignore]`-d with reason `S2: request-body forwarding`) for the
/// SESSION 2 target.
fn build_h1_request(req: &H3Request, body: Option<&[u8]>) -> Vec<u8> {
    let body_len = body.map_or(0, <[u8]>::len);
    // Build the ASCII head exactly as before, then append the RAW body
    // bytes. SESSION 2 fix: the body is appended via
    // `extend_from_slice` — NO `from_utf8_lossy`/string conversion —
    // so non-UTF-8 payloads (protobuf/images/gzip) survive byte-for-
    // byte. `Content-Length` is unchanged (already `body.len()`).
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
    // `None` keeps the historical `Content-Length: 0` (bodyless
    // GET/HEAD) so the bodyless output is byte-identical to legacy.
    s.push_str("Content-Length: ");
    s.push_str(&body_len.to_string());
    s.push_str("\r\n");
    s.push_str("Connection: close\r\n");
    s.push_str("\r\n");
    let mut out = s.into_bytes();
    out.reserve(body_len);
    if let Some(bytes) = body {
        // Raw, lossless append: the request body forwarded to the H1
        // backend is exactly the inbound H3 DATA-frame bytes.
        out.extend_from_slice(bytes);
    }
    out
}

/// Parsed H1 response captured from the backend.
#[derive(Debug)]
struct H1Response {
    status: u16,
    #[allow(dead_code)]
    headers: HashMap<String, String>,
    body: Bytes,
}

/// Read a plain HTTP/1.1 response line+headers+body from a TCP stream.
/// The mock backend in the e2e test sends a complete `Content-Length`-
/// delimited response and closes; handling `Transfer-Encoding: chunked`
/// and keep-alive is out of scope for 3b.3c-2.
async fn read_h1_response(stream: &mut TcpStream) -> Result<H1Response, String> {
    // Production path: the response-side memory-safety ceiling is the
    // named `MAX_RESPONSE_BODY_BYTES` const. The cap logic lives in
    // `read_h1_response_capped` so it can be unit-tested with a tiny
    // limit (a true 64 MiB transfer is impractical on a 2-CPU/7 GB
    // box); this wrapper is the SOLE production caller and passes the
    // const unchanged → byte-for-byte identical behaviour for every
    // conformant (<<= 256 KiB) response.
    read_h1_response_capped(stream, MAX_RESPONSE_BODY_BYTES).await
}

/// Cap-parameterised core of [`read_h1_response`]. `cap` bounds the
/// total buffered response (status line + headers + body); exceeding it
/// returns `Err` (mapped to a clean H3 `502` by the caller) rather than
/// growing the buffer until the proxy OOMs. The whole response is FULLY
/// BUFFERED here — incremental egress is the headline Session 3 item
/// (`audit/h3-program/p1c-response-streaming-assessment.md`).
async fn read_h1_response_capped(stream: &mut TcpStream, cap: usize) -> Result<H1Response, String> {
    let mut all = Vec::with_capacity(1024);
    let mut buf = [0u8; 4096];
    loop {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("backend read: {e}"))?;
        if n == 0 {
            break;
        }
        all.extend_from_slice(buf.get(..n).unwrap_or(&[]));
        // Bail the instant the accumulated response exceeds the
        // ceiling; the upstream conn is already marked non-reusable by
        // the caller's `fail502!`. Conformant responses are `<<` the
        // 64 MiB production ceiling so this changes nothing observable.
        if all.len() > cap {
            return Err(format!(
                "backend response exceeds {cap} bytes \
                 (P1-C cap; incremental egress is Session 3)"
            ));
        }
        // Optimistic early-exit if we see end of headers followed by
        // Content-Length bytes; for the mock backend the server always
        // closes, so the above read-to-zero drains the socket.
    }
    let sep_pos =
        find_header_sep(&all).ok_or_else(|| "no CRLF CRLF in backend response".to_string())?;
    let head = all.get(..sep_pos).ok_or("head slice")?.to_vec();
    let body_slice = all.get(sep_pos + 4..).unwrap_or(&[]).to_vec();
    let head_str = std::str::from_utf8(&head).map_err(|e| format!("non-utf8 head: {e}"))?;
    let mut lines = head_str.split("\r\n");
    let status_line = lines.next().ok_or("empty status line")?;
    let status = parse_status_line(status_line)?;
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Ok(H1Response {
        status,
        headers,
        body: Bytes::from(body_slice),
    })
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

/// Build the H3 response byte stream from an H1 response.
///
/// Emits one HEADERS frame (`:status`, `content-length`) followed by
/// one DATA frame (body). Returns the concatenated bytes the actor
/// will `stream_send` with FIN.
///
/// Uses only QPACK static-table entries. `:status` for the common
/// codes (200/204/206/302/304/400/403/404/421/425/500/503) is indexed;
/// other values fall back to literal-with-name-ref on the `:status`
/// static entry. `content-length: N` always literal-with-name-ref.
///
/// # Errors
///
/// Surfaces a string-formatted error if QPACK encoding or the H3
/// frame encoder reject the inputs.
pub fn encode_h3_response(status: u16, body: &[u8]) -> Result<Vec<u8>, String> {
    let headers_frame = encode_h3_headers_frame(status, Some(body.len()))?;
    let data_frame = encode_h3_data_frame(body)?;
    let mut out = Vec::with_capacity(headers_frame.len() + data_frame.len());
    out.extend_from_slice(&headers_frame);
    out.extend_from_slice(&data_frame);
    Ok(out)
}

/// SESSION 4 / P1-A: encode just the H3 response HEADERS frame.
///
/// `content_length`:
///   * `Some(n)` — emits `:status` + `content-length: n`. With
///     `Some(body.len())` this is **byte-identical** to the HEADERS
///     frame [`encode_h3_response`] produced before this refactor
///     (the no-regression contract — every existing CL backend +
///     test client depends on it).
///   * `None` — emits `:status` only (length unknown:
///     chunked / EOF-delimited; the client relies on stream FIN).
///
/// QPACK / frame-encoder behaviour is unchanged from the pre-refactor
/// `encode_h3_response`.
///
/// # Errors
///
/// Surfaces a string-formatted error if QPACK encoding or the H3
/// frame encoder reject the inputs.
pub fn encode_h3_headers_frame(
    status: u16,
    content_length: Option<usize>,
) -> Result<Bytes, String> {
    let encoder = QpackEncoder::new();
    let mut headers: Vec<(String, String)> = vec![(":status".to_string(), status.to_string())];
    if let Some(n) = content_length {
        headers.push(("content-length".to_string(), n.to_string()));
    }
    let header_block = encoder
        .encode(&headers)
        .map_err(|e| format!("qpack encode: {e}"))?;
    encode_frame(&H3Frame::Headers { header_block }).map_err(|e| format!("h3 headers frame: {e}"))
}

/// SESSION 12 / CF-H3H3-HEAD: encode the H3 response HEADERS frame
/// carrying the FULL non-pseudo response header set (not just
/// `:status` + `content-length`).
///
/// Emits `:status` FIRST, then every `(name, value)` in `headers`
/// VERBATIM in order. The caller is responsible for having already
/// filtered out pseudo-headers and any hop-by-hop fields it does not
/// want forwarded; this helper re-encodes exactly what it is given
/// (`content-length`, when present, rides through as a regular header).
///
/// This is the full-fidelity sibling of [`encode_h3_headers_frame`]
/// (which intentionally projects to `:status` + `content-length` only
/// and is retained for the inline error responses + the byte-identical
/// `encode_h3_response`). The H3→H3 streaming response head
/// ([`H3RespOut::on_head`] `Wire` arm) uses THIS so it forwards the
/// upstream's full response header set — matching the `Decoded` arm
/// (H1/H2 fronts) and the buffering `request_h3_upstream` (R12
/// convergence: every H3→H3 response head carries content-type /
/// cache-control / set-cookie / custom headers, not just the minimal
/// projection that shipped before).
///
/// QPACK encoding is literal-with-name-ref / literal (no dynamic
/// table), so arbitrary header names round-trip.
///
/// # Errors
///
/// Surfaces a string-formatted error if QPACK encoding or the H3
/// frame encoder reject the inputs.
pub fn encode_h3_headers_frame_full(
    status: u16,
    headers: &[(String, String)],
) -> Result<Bytes, String> {
    let mut fields: Vec<(String, String)> = Vec::with_capacity(headers.len() + 1);
    fields.push((":status".to_string(), status.to_string()));
    fields.extend(headers.iter().cloned());
    let header_block = QpackEncoder::new()
        .encode(&fields)
        .map_err(|e| format!("qpack encode: {e}"))?;
    encode_frame(&H3Frame::Headers { header_block }).map_err(|e| format!("h3 headers frame: {e}"))
}

/// SESSION 12 / CF-H3-HEAD: response-direction hop-by-hop header names
/// that a proxy MUST NOT forward to the downstream peer (the RFC 9110
/// connection-management headers). This MIRRORS
/// `lb_l7::h2_to_h1::RESPONSE_HOP_BY_HOP`. `lb-quic` is BELOW `lb-l7` in
/// the dependency graph and cannot depend on it (reverse layering), so
/// the set is duplicated here, like the other deliberate cross-crate
/// duplications in this file. Keep the two in sync.
///
/// Used by the three H3-FRONT response legs ([`stream_h1_response`],
/// [`stream_h2_response`], [`H3RespOut::on_head`]'s `Wire` arm) which
/// re-encode an upstream response head straight to H3 wire with NO L7
/// front after them, so they must strip hop-by-hop themselves. Stripping
/// here is REQUIRED for conformance: RFC 9114 §4.2 — "An endpoint MUST
/// NOT generate an HTTP/3 field section containing connection-specific
/// header fields." The `Decoded` arm (H1/H2 fronts) forwards the full
/// set because the front applies this same strip at its own layer
/// (`lb_l7::h1_proxy::h3_decoded_resp_head_builder`).
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

/// `true` iff `name_lower` (an ALREADY-lowercased header name) is a
/// response-direction hop-by-hop header (see [`RESPONSE_HOP_BY_HOP`]).
fn is_response_hop_by_hop(name_lower: &str) -> bool {
    RESPONSE_HOP_BY_HOP.contains(&name_lower)
}

/// SESSION 4 / P1-A: encode one H3 response DATA frame carrying
/// `payload`. Byte-identical to the DATA frame [`encode_h3_response`]
/// produced before this refactor.
///
/// # Errors
///
/// Surfaces a string-formatted error if the H3 frame encoder rejects
/// the input.
pub fn encode_h3_data_frame(payload: &[u8]) -> Result<Bytes, String> {
    encode_frame(&H3Frame::Data {
        payload: Bytes::copy_from_slice(payload),
    })
    .map_err(|e| format!("h3 data frame: {e}"))
}

/// SESSION 4 / P1-C (C4): encode the RFC 9114 §4.1 trailing field
/// section as a post-DATA H3 HEADERS frame, carrying the upstream H1
/// response's chunked trailer fields.
///
/// The QPACK + frame encode is intentionally identical to the
/// request-side trailer encode in [`request_h3_upstream`]
/// (`QpackEncoder::encode` → `H3Frame::Headers`). The ~3-line
/// duplication is a deliberate no-regression trade-off: forking a
/// shared helper into the PROTO-2-12-locked `request_h3_upstream` would
/// risk that path's byte-for-byte wire identity for no behavioural
/// gain. TODO(future): dedupe once both paths share a regression lock.
///
/// # Errors
///
/// Surfaces a string-formatted error if QPACK encoding or the H3 frame
/// encoder reject the inputs.
pub fn encode_h3_trailers_frame(trailers: &[(String, String)]) -> Result<Bytes, String> {
    let header_block = QpackEncoder::new()
        .encode(trailers)
        .map_err(|e| format!("qpack trailer encode: {e}"))?;
    encode_frame(&H3Frame::Headers { header_block }).map_err(|e| format!("h3 trailer frame: {e}"))
}

/// SESSION 4 / P1-A: response-body framing decided from the parsed
/// upstream H1 response headers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RespFraming {
    /// `Content-Length: n` — stream exactly `n` body bytes.
    ContentLength(usize),
    /// `Transfer-Encoding: chunked` — incremental de-chunk.
    Chunked,
    /// No CL, no TE — body runs until socket EOF.
    Eof,
}

/// SESSION 4 / P1-A: incremental HTTP/1.1 chunked-transfer decoder for
/// RESPONSES (production previously did not parse chunked responses at
/// all). Feed raw socket bytes; it yields decoded payload and detects
/// the zero-size terminator. Every malformed input ⇒
/// [`RespAbort::ChunkedDecode`] (approval condition C3) — never a
/// truncated/forwarded body presented as complete.
#[derive(Debug)]
struct ChunkDecoder {
    /// Bytes not yet consumed (a partial chunk-size line or a partial
    /// chunk body straddling reads). Bounded: a chunk-size line is
    /// rejected past [`MAX_FRAME_HEADER_BYTES`]-class small limits, and
    /// payload is drained immediately (never whole-chunk buffered).
    buf: Vec<u8>,
    /// `Some(remaining)` while inside a chunk body; `None` while
    /// expecting the next chunk-size line.
    in_chunk: Option<usize>,
    /// The zero-size chunk was seen — no more body payload follows. The
    /// optional RFC 9112 §7.1.2 trailer section + the final CRLF are
    /// still being consumed until [`Self::complete`].
    done: bool,
    /// SESSION 4 / P1-C (C4): the zero-size chunk, the trailer section
    /// (possibly empty), and the terminating CRLF were ALL consumed —
    /// the chunked message is genuinely finished. The
    /// [`stream_h1_response`] chunked loop exits on this (NOT `done`):
    /// stopping at `done` would drop / mis-frame the trailer section.
    complete: bool,
    /// SESSION 4 / P1-C: decoded trailer fields (RFC 9112 §7.1.2
    /// trailer section). Empty when the message had no trailers.
    /// Taken once via [`Self::take_trailers`] for the post-DATA H3
    /// trailing-HEADERS frame.
    trailers: Vec<(String, String)>,
    /// SESSION 4 / P1-C: bytes of the trailer section read so far,
    /// awaiting its terminating blank line. Hard-bounded by
    /// [`MAX_TRAILER_SECTION`] (a hostile/oversized trailer block must
    /// not grow memory without limit — mirrors the request-side
    /// `MAX_TRAILER_BLOCK_BYTES` ceiling rationale).
    trailer_buf: Vec<u8>,
}

/// Max bytes a chunk-size line (`<hex>[;ext]\r\n`) may occupy before we
/// reject it as malformed/hostile framing (smuggling guard, C3).
const MAX_CHUNK_SIZE_LINE: usize = 256;

/// SESSION 4 / P1-C (C4): max bytes the chunked trailer section
/// (RFC 9112 §7.1.2 — the field lines after the `0\r\n` plus the
/// terminating CRLF) may occupy before it is rejected as
/// malformed/hostile framing ⇒ [`RespAbort::ChunkedDecode`]. 64 KiB is
/// far above any realistic trailer section and mirrors the request-side
/// trailer-block ceiling rationale (`h3_bridge.rs` ~:86-87).
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

    /// SESSION 4 / P1-C: take the decoded trailer fields (empty if the
    /// chunked message carried no trailer section). Only meaningful
    /// once [`Self::complete`] is set.
    fn take_trailers(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.trailers)
    }

    /// Feed `input`, appending decoded payload to `out`. Returns
    /// `Err(RespAbort::ChunkedDecode)` on ANY malformed framing
    /// (including a malformed trailer section — C3/C4 parity: a
    /// truncated/forwarded body is NEVER presented as complete).
    fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), RespAbort> {
        self.buf.extend_from_slice(input);
        loop {
            if self.complete {
                // The trailer section + final CRLF were consumed: the
                // message is genuinely finished. Any further bytes are
                // not part of this response.
                return Ok(());
            }
            if self.done {
                // The zero-size chunk was seen; the only thing left is
                // the RFC 9112 §7.1.2 trailer section (possibly empty)
                // terminated by a blank CRLF line. PC-2: this consumes
                // from `self.buf`, so a trailer section coalesced into
                // the SAME read as the `0\r\n` size line is parsed
                // here, not only subsequently-read socket bytes.
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
                    // Awaiting a chunk-size line terminated by CRLF.
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
                    // Strip a chunk extension (`;name=value`); the size
                    // is the hex before the first ';'.
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
                        // Zero-size terminator: no more body payload.
                        // SESSION 4 / P1-C (C4): the optional trailer
                        // section + final CRLF still follow — do NOT
                        // return here; loop back so a trailer section
                        // coalesced into THIS read (after the `0\r\n`
                        // size line, now drained from `self.buf`) is
                        // consumed by `parse_trailer_section` in the
                        // same `feed` call (PC-2 correctness).
                        self.done = true;
                        continue;
                    }
                    self.in_chunk = Some(size);
                }
            }
        }
    }

    /// SESSION 4 / P1-C (C4): parse the RFC 9112 §7.1.2 trailer section
    /// — zero or more `field-line CRLF` then a terminating empty CRLF
    /// line — that follows the zero-size chunk. Called only while
    /// `self.done && !self.complete`.
    ///
    /// Consumes from `self.buf` into the bounded `self.trailer_buf` so a
    /// trailer section split across reads (or coalesced with the
    /// `0\r\n` size line, PC-2) parses identically. On the terminating
    /// blank line sets `self.complete` and decodes the accumulated
    /// fields. ANY malformed input ⇒ [`RespAbort::ChunkedDecode`]
    /// (C3/C4 parity — never a truncated/forwarded body as complete):
    ///   * a field line with no `:` (e.g. junk after the terminator),
    ///   * a `:`-prefixed pseudo-header name (RFC 9114 §4.3, mirrors
    ///     the request-side body-trailer guard),
    ///   * an empty field name,
    ///   * a trailer section exceeding [`MAX_TRAILER_SECTION`].
    fn parse_trailer_section(&mut self) -> Result<(), RespAbort> {
        loop {
            // Move available bytes into the bounded trailer buffer.
            if !self.buf.is_empty() {
                if self.trailer_buf.len() + self.buf.len() > MAX_TRAILER_SECTION {
                    return Err(RespAbort::ChunkedDecode);
                }
                self.trailer_buf.append(&mut self.buf);
            }
            let Some(nl) = self.trailer_buf.windows(2).position(|w| w == b"\r\n") else {
                // No complete line yet. Bound the partial accumulation
                // (a never-terminated trailer section is hostile).
                if self.trailer_buf.len() > MAX_TRAILER_SECTION {
                    return Err(RespAbort::ChunkedDecode);
                }
                return Ok(());
            };
            if nl == 0 {
                // Empty line: end of the trailer section.
                self.trailer_buf.drain(..2);
                self.complete = true;
                return Ok(());
            }
            let line = self.trailer_buf.get(..nl).ok_or(RespAbort::ChunkedDecode)?;
            let line = std::str::from_utf8(line).map_err(|_| RespAbort::ChunkedDecode)?;
            // A trailer field line MUST be `name: value`. No `:` (junk
            // after the zero-size terminator, e.g. the C3 smuggling
            // case) ⇒ malformed framing.
            let (name, value) = line.split_once(':').ok_or(RespAbort::ChunkedDecode)?;
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(RespAbort::ChunkedDecode);
            }
            // RFC 9114 §4.3: a trailer section MUST NOT contain
            // pseudo-header fields. Mirrors the request-side guard
            // (`feed_body` H3 trailer parsing).
            if name.starts_with(':') {
                return Err(RespAbort::ChunkedDecode);
            }
            self.trailers.push((name, value.trim().to_owned()));
            self.trailer_buf.drain(..nl + 2);
        }
    }
}

/// SESSION 4 / P1-A — **incremental, bounded, backpressured** H3
/// RESPONSE egress: read the H1 upstream response and pipe it to the
/// actor as pre-encoded H3 wire bytes over the bounded `tx`, as the
/// bytes arrive.
///
/// 1. Read only until the `\r\n\r\n` head terminator (bounded by a
///    64 KiB head cap → [`RespAbort::BadHead`] if exceeded), parse the
///    status line + headers.
/// 2. Emit the HEADERS [`RespEvent::Bytes`] **immediately** — before
///    any body byte is read. `Content-Length` ⇒
///    [`encode_h3_headers_frame`]`(status, Some(n))` (byte-identical
///    to the legacy HEADERS — the no-regression contract); chunked /
///    EOF ⇒ `Some` length unknown → `None` (`:status` only, client
///    relies on FIN).
/// 3. Stream the body per framing as ≤ [`H3_RESP_CHUNK_MAX`] DATA
///    frames, each emitted the instant its bytes arrive from the
///    socket (NOT after the whole body). `tx.send(..).await` blocking
///    on the bounded channel is the backpressure point: a stalled H3
///    client → full channel → this `await` parks → upstream socket
///    `read()` not called → TCP backpressure to the upstream.
/// 4. On clean completion emit [`RespEvent::End`] and return `Ok(())`.
///    Every failure (upstream reset / premature EOF before CL /
///    chunked-decode error / over-cap / bad head / channel closed by
///    a cancelling client) emits [`RespEvent::Reset`] (best-effort)
///    and returns `Err(RespAbort)` — NEVER a truncated body presented
///    as complete (response-splitting / cache-poisoning guard).
///
/// The caller MUST mark the pooled upstream NON-reusable on any
/// `Err(RespAbort)` (approval condition C2).
///
/// # Errors
///
/// Returns [`RespAbort`] (variant identifies the cause) on any
/// upstream / framing / cap / client-cancel failure.
pub async fn stream_h1_response(
    stream: &mut TcpStream,
    tx: &tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    /// Send a `RespEvent`, mapping a closed channel (cancelling
    /// client) to `ClientGone` so the producer stops reading upstream.
    macro_rules! send {
        ($tx:expr, $ev:expr) => {
            $tx.send($ev).await.map_err(|_| RespAbort::ClientGone)?
        };
    }

    // --- 1. read + parse the head, bounded ---
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
            // EOF before the header terminator: nothing parseable.
            let _ = tx.send(RespEvent::Reset).await;
            return Err(RespAbort::BadHead);
        }
        head.extend_from_slice(rbuf.get(..n).unwrap_or(&rbuf));
    };
    // Bytes already read past the header terminator are the first body
    // bytes — must NOT be lost.
    let mut body_prefix = head.split_off(sep + 4);
    head.truncate(sep);

    let head_str = std::str::from_utf8(&head).map_err(|_| {
        // best-effort Reset; mapped to BadHead.
        RespAbort::BadHead
    });
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
    // CF-H3-HEAD: collect the FULL non-hop-by-hop response header set to
    // re-encode for the H3 client (pre-S12 this parsed only
    // content-length + transfer-encoding and dropped everything else).
    // `content-length` is handled via `framing` (re-added below from the
    // ONE declared-length source), and `transfer-encoding` is
    // hop-by-hop (de-chunked here; the H3 leg is FIN-delimited), so both
    // are excluded from `fwd_headers`.
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
    // Transfer-Encoding takes precedence over Content-Length (RFC 9112
    // §6.1); a message with BOTH is a smuggling vector — reject.
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

    // --- 2. emit HEADERS immediately (before any body byte) ---
    // Forward `:status` + the full non-hop-by-hop set; re-add
    // `content-length` (as a regular header) ONLY for the ContentLength
    // framing so the H3 client gets the same declared length, and never
    // for chunked/EOF (FIN-delimited on the H3 leg — CF-H3-HEAD).
    if let RespFraming::ContentLength(n) = &framing {
        fwd_headers.push(("content-length".to_string(), n.to_string()));
    }
    // SESSION 24 / INC-3: emit the DECODED head (the actor encodes via
    // `quiche::h3::send_response`). `fwd_headers` is already
    // hop-by-hop-stripped + content-length-managed above; we just stop
    // encoding here. The `cap`/`total` accounting now counts the
    // decoded header byte length (a DoS threshold, not a memory bound —
    // mirrors how the Decoded arm counts decoded bytes).
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

    // --- 3. stream the body per framing, as it arrives ---
    // Emit one ≤H3_RESP_CHUNK_MAX DATA chunk from `payload` (the actor
    // encodes the DATA frame via `send_body`). `cap`/`total` counts
    // PAYLOAD bytes.
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
            // Drain the post-head prefix first.
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
                    // EOF before Content-Length satisfied.
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
            // SESSION 4 / P1-C (C4): loop until `complete` (zero-size
            // chunk + trailer section + final CRLF all consumed), NOT
            // merely `done` (zero-size chunk seen) — stopping at `done`
            // would drop / mis-frame the trailer section. EOF before
            // `complete` (terminator OR trailer section truncated) ⇒
            // ChunkedDecode, never a forwarded truncated body.
            while !dec.complete {
                let nr = match stream.read(&mut rbuf).await {
                    Ok(n) => n,
                    Err(_) => {
                        let _ = tx.send(RespEvent::Reset).await;
                        return Err(RespAbort::UpstreamReset);
                    }
                };
                if nr == 0 {
                    // EOF before the chunked terminator / before the
                    // trailer section's terminating CRLF.
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
            // SESSION 4 / P1-C (C4): trailing-HEADERS-after-DATA. The
            // RFC 9112 §7.1.2 chunked trailer section maps to an
            // RFC 9114 §4.1 H3 trailing field section, emitted as ONE
            // final `RespEvent::Trailers` AFTER the last DATA and BEFORE
            // `End` (never before the body; never on an abort — any
            // abort returned above without reaching here). SESSION 24 /
            // INC-3: emitted DECODED; the actor encodes via
            // `send_additional_headers`.
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

    // --- 4. clean completion ---
    send!(tx, RespEvent::End);
    Ok(())
}

/// S6 / H3→H2 R8 (M-B / I1) — stream a hyper H2 upstream
/// [`Response<Incoming>`] back to the H3 client, re-encoded into H3
/// wire frames, **bounded-incrementally** and with end-to-end
/// backpressure. This is the H2 cousin of [`stream_h1_response`] and
/// obeys the IDENTICAL `RespEvent` / `RespAbort` contract:
///
/// 1. Emit the response HEADERS frame (`:status` [+ `content-length`
///    iff the upstream declared one]) as the FIRST [`RespEvent::Bytes`]
///    BEFORE any body byte — byte-identical framing to
///    [`stream_h1_response`]'s step 2.
/// 2. Pull the H2 body **one frame at a time** via
///    [`http_body_util::BodyExt::frame`]; split each DATA frame into
///    `≤ H3_RESP_CHUNK_MAX` slices, encode each as an H3 DATA frame,
///    and `send` it on the bounded `tx`. The `.send().await` is the
///    backpressure point: a stalled H3 client ⇒ the actor stops
///    draining ⇒ this channel fills ⇒ `body.frame().await` is not
///    called again ⇒ hyper stops issuing H2 `WINDOW_UPDATE`s ⇒ the H2
///    upstream's send window closes (stalled client ⇒ paused upstream
///    read). Memory retained = at most the in-hand frame (dropped
///    after splitting) + `H3_RESP_CHANNEL_DEPTH` queued events —
///    body-size INDEPENDENT, never `.collect()`, never a `Vec<u8>`.
/// 3. A trailing H2 trailers frame (RFC 9110 §6.5) is re-encoded as
///    one final post-DATA H3 trailing-HEADERS [`RespEvent::Bytes`]
///    (pseudo-headers filtered) BEFORE `End` — parity with
///    [`stream_h1_response`]'s chunked-trailer C4 behaviour.
/// 4. Clean end ⇒ [`RespEvent::End`] (actor FINs), `Ok(())`.
/// 5. Any hyper body error / premature failure ⇒ best-effort
///    [`RespEvent::Reset`] + `Err(RespAbort::UpstreamReset)` so the
///    actor RESET_STREAMs the client and NEVER FINs — a partial body
///    is never presentable as a complete response (response-splitting
///    guard, identical to [`stream_h1_response`]). Over the `cap` ⇒
///    `Err(RespAbort::OverCap)`. A closed channel (client cancelled)
///    ⇒ `Err(RespAbort::ClientGone)` via the `send!` macro.
///
/// # Errors
///
/// Returns `Err(RespAbort)` describing why the relay aborted; the
/// caller (the H3→H2 orchestrator) propagates it so the actor
/// RESET_STREAMs the client.
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

    // --- 1. emit HEADERS immediately (before any body byte) ---
    // Mirror `stream_h1_response`: forward `content-length` only when
    // the upstream declared a valid one (so the H3 client gets the
    // same `Some(n)` vs `None` framing decision).
    let declared_len: Option<usize> = parts
        .headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<usize>().ok());
    // CF-H3-HEAD: forward the FULL non-hop-by-hop response header set to
    // the H3 client (pre-S12 this emitted only `:status` +
    // content-length). `HeaderMap` carries no pseudo-headers (the H2
    // status rides `parts.status`), so only the hop-by-hop strip is
    // needed; `content-length` is excluded here and re-added from the
    // single `declared_len` source so the H3 framing decision is
    // unchanged. `iter()` yields repeated names (e.g. set-cookie)
    // individually — all forwarded. A non-UTF-8 value is skipped (it
    // could not have been a valid forwarded header anyway).
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
    // SESSION 24 / INC-3: emit the DECODED head (the actor encodes via
    // `send_response`). `cap`/`total` counts decoded header bytes
    // (DoS threshold, not a memory bound).
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

    // --- 2/3. stream body frames as they arrive ---
    // Emit one ≤H3_RESP_CHUNK_MAX DATA chunk per slice (the actor
    // encodes the DATA frame via `send_body`); identical framing/cap
    // discipline to `stream_h1_response`'s `emit_data!`. `cap`/`total`
    // counts PAYLOAD bytes.
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
                // Upstream body error mid-response: best-effort Reset,
                // never a clean FIN (response-splitting guard).
                let _ = tx.send(RespEvent::Reset).await;
                return Err(RespAbort::UpstreamReset);
            }
        };
        if let Some(data) = frame.data_ref() {
            // Re-borrow as a slice; `Bytes` derefs to `[u8]`.
            let bytes: &[u8] = data;
            if !bytes.is_empty() {
                emit_data!(bytes);
            }
        } else if let Some(tmap) = frame.trailers_ref() {
            // RFC 9110 §6.5 trailers → one post-DATA H3 trailing
            // HEADERS frame. Filter pseudo-headers (defensive; H2
            // trailers must not carry them) and skip an empty set.
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
                // SESSION 24 / INC-3: emit DECODED trailers (the actor
                // encodes via `send_additional_headers`).
                total = total.saturating_add(trailers.iter().map(|(n, v)| n.len() + v.len()).sum());
                if total > cap {
                    let _ = tx.send(RespEvent::Reset).await;
                    return Err(RespAbort::OverCap);
                }
                send!(tx, RespEvent::Trailers(trailers));
            }
        }
        // Any other frame kind (none currently in http-body 1.x) is
        // ignored — never forwarded raw.
    }

    // --- 4. clean completion ---
    send!(tx, RespEvent::End);
    Ok(())
}

/// Forward an H3 request to an H1 backend via `TcpPool` and return the
/// H1 response mapped into H3 wire bytes. On any backend failure,
/// returns a 502 response body `b"bad gateway"`.
///
/// CODE-2-09 follow-on: the dial is now an async
/// [`TcpPool::acquire_async`] call instead of
/// `spawn_blocking(pool.acquire)`. The pool's
/// [`lb_io::pool::PoolConfig::connect_timeout`] governs the deadline.
///
/// # Errors
///
/// Surfaces a string-formatted error if the H3 frame encoding of the
/// fallback 502 response itself fails. Backend dial / write / read
/// errors are caught and turned into a 502 response body internally
/// rather than bubbled up.
/// S1-B seam (SESSION 1): `body` is an OPTIONAL request payload.
/// Today every caller (`conn_actor::poll_h3`) passes `None` because the
/// connection actor does not yet accumulate inbound H3 DATA frames —
/// that work is SESSION 2. Passing `None` is byte-for-byte identical to
/// the prior bodyless implementation (verified by the crate-local e2e
/// `h3_h1_bridge_e2e.rs` and unchanged repo-root tests). SESSION 2 will
/// pass `Some(collected_request_body)` here once `poll_h3` threads DATA
/// frames across the stream boundary.
pub async fn h3_to_h1_roundtrip(
    req: &H3Request,
    backend: SocketAddr,
    pool: &TcpPool,
    body: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut pooled = match pool.acquire_async(backend).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "H3→H1 backend acquire failed");
            return encode_h3_response(502, b"bad gateway");
        }
    };
    let h1_request = build_h1_request(req, body);
    let response = {
        let stream = pooled
            .stream_mut()
            .ok_or_else(|| "pool returned empty handle".to_string())?;
        if let Err(e) = stream.write_all(&h1_request).await {
            pooled.set_reusable(false);
            tracing::warn!(error = %e, "H3→H1 backend write failed");
            return encode_h3_response(502, b"bad gateway");
        }
        if let Err(e) = stream.flush().await {
            pooled.set_reusable(false);
            tracing::warn!(error = %e, "H3→H1 backend flush failed");
            return encode_h3_response(502, b"bad gateway");
        }
        match read_h1_response(stream).await {
            Ok(r) => r,
            Err(e) => {
                pooled.set_reusable(false);
                tracing::warn!(error = %e, "H3→H1 backend read failed");
                return encode_h3_response(502, b"bad gateway");
            }
        }
    };
    // Connection: close was sent, socket will be dropped; do not reuse.
    pooled.set_reusable(false);
    encode_h3_response(response.status, &response.body)
}

/// SESSION 2 / P1-A: build ONLY the HTTP/1.1 request head (request
/// line + headers + CRLF CRLF terminator) — no body. Used by
/// [`h3_to_h1_stream`] so the body can be streamed incrementally
/// after the head instead of being concatenated into one buffer.
///
/// `framing` selects the entity-body framing header:
///   * [`H1BodyFraming::None`]    — `Content-Length: 0` (bodyless;
///     BYTE-IDENTICAL to `build_h1_request(req, None)`).
///   * [`H1BodyFraming::ContentLength(n)`] — `Content-Length: n`.
///   * [`H1BodyFraming::Chunked`] — `Transfer-Encoding: chunked`.
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

/// SESSION 2 / P1-A: HTTP/1.1 request entity-body framing choice.
#[derive(Debug, Clone, PartialEq, Eq)]
enum H1BodyFraming {
    /// No body — `Content-Length: 0`.
    None,
    /// Client supplied `content-length`; forward raw bytes unframed.
    ContentLength(u64),
    /// No client `content-length`; HTTP/1.1 chunked transfer-coding.
    Chunked,
}

/// SESSION 2 / P1-A: test-only gauge of the maximum number of
/// in-flight body bytes the streaming egress has buffered at once
/// (the single peeked chunk). Asserted by the backpressure test (T5)
/// to prove the proxy is NOT buffering the whole body.
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

/// SESSION 2 / P1-A: write one request-body chunk to the H1 upstream
/// with the chosen framing. `Err(())` signals an upstream write
/// failure (caller maps to 502). Empty data is a no-op (a zero-length
/// DATA frame must NOT emit a spurious `0\r\n\r\n` chunk-terminator).
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

/// SESSION 4 / P1-A.1 — terminal outcome of the request-write half
/// ([`write_h1_request`]).
///
/// The request body is forwarded incrementally; this reports HOW that
/// phase ended so the caller can pick the client-facing response AND
/// the pooled-connection disposition (C2). Each non-`Complete` outcome
/// means the request was NOT completed on the wire (no chunked
/// terminator / partial `Content-Length`), so the upstream never sees
/// a completable request — the caller MUST mark the pooled connection
/// non-reusable (request-smuggling / cache-poisoning guard).
enum ReqWriteOutcome {
    /// Request head + body fully + correctly written and flushed
    /// (chunked terminator already sent when chunked). The caller
    /// reads/streams the response on the same stream.
    Complete,
    /// A graceful client-facing abort decided by the body channel:
    /// `body_rx` delivered `Reset` (poll_h3's oversized / client-cancel
    /// signal — the pre-extraction `413`) or the channel closed before
    /// `End` (producer dropped mid-body — the pre-extraction `502`).
    /// `(status, body)` is exactly what the pre-extraction
    /// `h3_to_h1_stream` returned for that case.
    Aborted(u16, &'static [u8]),
}

/// SESSION 4 / P1-A.1 — the request-write half, extracted **verbatim**
/// from the pre-extraction `h3_to_h1_stream` body so its observable
/// behaviour is BYTE-IDENTICAL (the two request-side e2e suites,
/// `h3_to_h1_roundtrip`, and all S1–S3 H3 request-streaming e2e stay
/// green — pure extraction, no logic change).
///
/// Peeks the first body event to choose framing, writes the H1 head,
/// then forwards request DATA incrementally over `body_rx` (one event
/// held at a time → request-side backpressure, unchanged from S2). It
/// returns BEFORE the chunked terminator / full `Content-Length` on
/// any abort so the upstream never sees a completable request.
///
/// **Pooled-connection ownership (C2):** the CALLER owns the
/// `PooledTcp`; this helper only borrows `stream` and NEVER calls
/// `set_reusable`. The caller threads ownership as
/// `acquire pooled → let stream = pooled.stream_mut() →
/// write_h1_request(req, stream, &mut rx)` then, on `Err(())`
/// (upstream I/O failure) OR `Ok(ReqWriteOutcome::Aborted(..))`
/// (channel abort) OR — for the streaming P1-B producer — any
/// `Err(RespAbort)` from [`stream_h1_response`], calls
/// `pooled.set_reusable(false)` before the pooled handle drops. The
/// buffered [`h3_to_h1_stream`] caller below preserves the
/// pre-extraction disposition exactly (every terminal path drops the
/// conn non-reusable).
///
/// # Errors
///
/// `Err(())` on any upstream write/flush I/O failure (head, a body
/// chunk, the chunked terminator, or the final flush) — the
/// pre-extraction `502` path. The caller maps it to a client-facing
/// `502` and marks the pooled connection non-reusable.
#[allow(clippy::too_many_lines)]
async fn write_h1_request(
    req: &H3Request,
    stream: &mut TcpStream,
    body_rx: &mut tokio::sync::mpsc::Receiver<ReqBodyEvent>,
) -> Result<ReqWriteOutcome, ()> {
    // Peek the FIRST body event to choose framing. We buffer at most
    // ONE chunk here — bounded, never the whole body.
    let first = body_rx.recv().await;
    let (framing, mut pending_first): (H1BodyFraming, Option<Bytes>) = match &first {
        // Bodyless: byte-identical head to build_h1_request(req, None).
        Some(ReqBodyEvent::End { .. }) | None => (H1BodyFraming::None, None),
        Some(ReqBodyEvent::Reset) => {
            // Reset before any data ⇒ treat as a too-large/abort signal
            // surfaced by poll_h3: respond 413 (poll_h3 only Resets
            // early for the oversized case in this increment).
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

    // Stream the remaining events incrementally. One event held at
    // a time → backpressure: a slow upstream stalls this recv loop,
    // the channel fills, poll_h3 stops extending QUIC flow control.
    // Bodyless (first event == End / channel already closed) is a
    // clean end with no further events.
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
                // SESSION 2 / P1-C — REQUEST TRAILERS ARE INTENTIONALLY
                // DROPPED on the H3→H1/1.1 leg. An H3 request trailing
                // field section (RFC 9114 §4.1: a HEADERS frame after
                // the DATA frames) is fully PARSED upstream by
                // `StreamRxBuf::feed_body` (so a malformed/oversized
                // trailer block is still rejected and the body-phase
                // parser cannot crash/corrupt — that path is
                // regression-locked by the P1-C trailer e2e) and the
                // decoded list is carried here in `End.trailers`, but
                // it is deliberately NOT emitted to the HTTP/1.1
                // upstream. Rationale: forwarding trailers over
                // HTTP/1.1 requires `Transfer-Encoding: chunked` PLUS a
                // request-side `Trailer:` announcement and is only
                // legal for declared, non-forbidden fields (RFC 9110
                // §6.5 / RFC 7230 §4.1.2); silently smuggling
                // peer-controlled trailer fields into the H1 request
                // head/body would be a request-smuggling vector.
                // Dropping them yields a well-formed, complete H1
                // request (the body is already correctly framed by
                // Content-Length or the chunked terminator) and is an
                // RFC-acceptable downgrade: HTTP/1.1 has no obligation
                // to convey upstream-uninterpreted trailers. Genuine
                // H1 trailer egress (chunked + `Trailer:` allow-list)
                // is deferred to a later session. The value is consumed
                // (`trailers: _`) so the stream terminates clean.
                clean_end = true;
                break;
            }
            ReqBodyEvent::Reset => {
                // SESSION 2 / P1-B: mid-body Reset. poll_h3 emits
                // this for BOTH (a) the oversized cap breach and
                // (b) a CLIENT CANCEL (peer QUIC RESET_STREAM /
                // STOP_SENDING before FIN). In every case the body
                // is incomplete and MUST NOT be delivered to the
                // backend as a completed request: we return IMMEDIATELY,
                // BEFORE the `0\r\n\r\n` chunked terminator / before the
                // full Content-Length body is written, so the backend
                // never sees a completable request; the CALLER marks
                // the pooled upstream connection non-reusable (so the
                // partially written request can never be paired with a
                // subsequent one — HTTP-request-smuggling / cache-
                // poisoning guard). The 413 status is the safe
                // client-facing response; a cancelling client has
                // already torn down its stream and will not read it,
                // while the oversized path genuinely wants 413 — the
                // load-bearing invariant here is the upstream abort,
                // not the status code.
                tracing::warn!(
                    "SESSION 2 / P1-B: H3→H1 stream body Reset (oversized or \
                     client cancel); aborting upstream without completing the request"
                );
                return Ok(ReqWriteOutcome::Aborted(413, b"payload too large"));
            }
        }
    }
    if !clean_end {
        // Channel closed before an explicit End/Reset → producer
        // dropped mid-body: abort rather than present a truncated
        // request to the upstream.
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

/// SESSION 2 / P1-A — buffered H3→H1 request-streaming round-trip.
///
/// Composes the extracted request-write half ([`write_h1_request`])
/// with the buffered [`read_h1_response`] + [`encode_h3_response`]
/// tail. Behaviour is BYTE-IDENTICAL to the pre-P1-A.1 monolithic
/// implementation (regression-locked by `h3_h1_stream_body_e2e.rs`,
/// `h3_h1_stream_body_errors_e2e.rs`, and the S1–S3 H3 request
/// streaming e2e). The SESSION 4 streaming producer uses
/// [`write_h1_request`] + [`stream_h1_response`] instead of this
/// buffered tail; this buffered variant is retained UNCHANGED for the
/// request-side suites and is not on the actor's H1 hot path after
/// P1-B.
///
/// `max_body` is the total-size cap surfaced as H3 `413` (the
/// in-flight window is the memory mechanism, separate from this).
///
/// # Errors
///
/// Returns the H3 wire bytes of a `413` when `body_rx` delivers a
/// `Reset` *carrying the too-large signal*; a `502` on any upstream
/// dial/write/read failure or premature channel close. Surfaces a
/// string error only if encoding the fallback response itself fails.
pub async fn h3_to_h1_stream(
    req: &H3Request,
    backend: SocketAddr,
    pool: &TcpPool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    _max_body: usize,
) -> Result<Vec<u8>, String> {
    let mut pooled = match pool.acquire_async(backend).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "H3→H1 stream backend acquire failed");
            return encode_h3_response(502, b"bad gateway");
        }
    };

    // --- write head + stream body incrementally (extracted helper) ---
    let response = {
        let stream = pooled
            .stream_mut()
            .ok_or_else(|| "pool returned empty handle".to_string())?;

        match write_h1_request(req, stream, &mut body_rx).await {
            Ok(ReqWriteOutcome::Complete) => match read_h1_response(stream).await {
                Ok(r) => r,
                Err(e) => {
                    pooled.set_reusable(false);
                    tracing::warn!(error = %e, "H3→H1 stream backend read failed");
                    return encode_h3_response(502, b"bad gateway");
                }
            },
            Ok(ReqWriteOutcome::Aborted(status, body)) => {
                // Request was NOT completed on the wire (returned before
                // the chunked terminator / full Content-Length): drop
                // the pooled conn non-reusable so the partial request
                // can never be paired with a later one — byte-identical
                // to the pre-extraction 413/502 abort disposition.
                pooled.set_reusable(false);
                return encode_h3_response(status, body);
            }
            Err(()) => {
                // Upstream write/flush I/O failure — the pre-extraction
                // `fail502!` path (warn already logged in the helper).
                pooled.set_reusable(false);
                return encode_h3_response(502, b"bad gateway");
            }
        }
    };

    pooled.set_reusable(false);
    encode_h3_response(response.status, &response.body)
}

/// SESSION 4 / P1-B — **incremental, bounded, backpressured** H3→H1
/// with INCREMENTAL RESPONSE egress. The actor's H1 hot-path producer
/// task body (replaces the buffered [`h3_to_h1_stream`] there; the
/// buffered variant is retained only for the request-side e2e suites).
///
/// Owns the [`PooledTcp`] for its whole lifetime. The request-write
/// half is the shared [`write_h1_request`] (byte-identical request
/// behaviour); the response is streamed incrementally via
/// [`stream_h1_response`] into the bounded `resp_tx` channel back to
/// the actor (the §1.4.3 backpressure gate + bounded channel are the
/// memory bound, response-size-independent — R8).
///
/// **C2 (approval condition — pooled-upstream smuggling guard):** on
/// EVERY non-clean outcome — `write_h1_request` `Err(())` (upstream
/// I/O failure) or `Ok(ReqWriteOutcome::Aborted(..))` (channel abort),
/// OR any [`RespAbort`] from `stream_h1_response` (all six variants
/// incl. `ClientGone`) — the `PooledTcp` is marked NON-reusable before
/// it drops, so a partially-written request / partially-consumed
/// upstream response can never poison a pooled connection. The clean
/// path ALSO marks it non-reusable, preserving the pre-P1-B
/// unconditional-on-success disposition (the request carries
/// `Connection: close`; the socket is not re-parked).
///
/// Returns `Ok(())` once the response was fully piped (the actor saw
/// `RespEvent::End` and will FIN), or `Err(RespAbort)` describing why
/// it aborted (the actor already saw the matching `RespEvent::Reset`
/// and will `RESET_STREAM` with `H3_INTERNAL_ERROR`). The request-write
/// abort/error cases are surfaced to the client as a complete inline
/// `413`/`502` (HEADERS+DATA then `End`) and return `Ok(())`.
pub async fn h3_to_h1_stream_resp(
    req: &H3Request,
    backend: SocketAddr,
    pool: &TcpPool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    resp_tx: tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    /// Emit a complete inline H3 response (Head+Body) then `End`, for
    /// the request-write abort/error paths. SESSION 24 / INC-3: emits
    /// DECODED events (the actor encodes via `quiche::h3`). Best-effort:
    /// a closed channel (client already gone) just means nobody is
    /// listening.
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
                // Stream the response incrementally. On ANY RespAbort
                // (upstream reset / premature EOF / chunked-decode /
                // over-cap / bad head / client gone) the upstream was
                // consumed partially/faithlessly ⇒ C2 below.
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

    // C2: every remaining outcome marks the pooled connection
    // non-reusable before it drops — `Err(RespAbort)` (all variants:
    // the upstream response was consumed partially / not faithfully
    // relayed) AND the `Ok(())` clean path (pre-P1-B
    // unconditional-on-success: the request carried `Connection:
    // close`, the socket must not be re-parked).
    pooled.set_reusable(false);
    outcome
}

/// S6 / H3→H2 R8 (M-B / I2) — boxed error type carried by the
/// streaming H2 request body so a mid-body abort is expressible (see
/// [`lb_io::http2_pool::H2ReqBody`]; `hyper::Error` has no public
/// constructor). A distinct unit type keeps the abort cause greppable
/// on the wire-fault path.
#[derive(Debug)]
struct H3ReqAbort;

impl std::fmt::Display for H3ReqAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("H3→H2 request body aborted (client RESET / producer dropped mid-body)")
    }
}
impl std::error::Error for H3ReqAbort {}

/// S6 / H3→H2 R8 (M-B / I2) — the streaming H2 request body. Owns the
/// inbound H3 request DATA `Receiver` and yields exactly one hyper
/// DATA `Frame` per `ReqBodyEvent::Chunk`, completing on `End`/closed
/// and **erroring** ([`H3ReqAbort`]) on a mid-body `Reset` so hyper
/// RST_STREAMs the H2 upstream (a truncated request is never presented
/// as complete — request-smuggling guard, BINDING case 7).
///
/// `tokio::sync::mpsc::Receiver<ReqBodyEvent>` is `Send + Sync` (its
/// payload is `Send + Sync`), so this body satisfies `BoxBody`'s
/// `Send + Sync` bound WITHOUT a pump task or the http-body-util
/// `channel` feature. `poll_frame` polls `body_rx` directly: the
/// backpressure is end-to-end — hyper only polls when the H2 send
/// window is open, so a stalled H2 upstream stops draining `body_rx`,
/// the M-A bounded channel fills, and `poll_h3` stops extending QUIC
/// flow control (memory bound = `H3_BODY_CHANNEL_DEPTH` × chunk,
/// body-size independent). `done` latches so a post-error/EOS poll
/// returns `None`.
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
            // The already-peeked first chunk. (Empty first chunk would
            // have been classified bodyless upstream — never reaches
            // here, but an empty frame is harmless.)
            return Poll::Ready(Some(Ok(hyper::body::Frame::data(b))));
        }
        match this.body_rx.poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(ReqBodyEvent::Chunk(b))) => {
                Poll::Ready(Some(Ok(hyper::body::Frame::data(b))))
            }
            Poll::Ready(Some(ReqBodyEvent::End { trailers: _ })) => {
                // S6 scoped-out: request trailers are NOT forwarded on
                // the H2 leg (parity w/ H3→H1 P1-C). Clean end-of-
                // stream — the body is fully + correctly framed.
                this.done = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(ReqBodyEvent::Reset)) | Poll::Ready(None) => {
                // Mid-body client RESET, or producer dropped before
                // End: error so hyper RST_STREAMs the H2 upstream — a
                // truncated request is NEVER presented as complete
                // (request-smuggling guard, BINDING case 7). H2
                // multiplexing ⇒ a per-stream RST does not poison the
                // connection (lead A2: no extra non-reusable
                // bookkeeping).
                this.done = true;
                Poll::Ready(Some(Err(Box::new(H3ReqAbort))))
            }
        }
    }
}

/// S6 / H3→H2 R8 (M-B / I2) — build the upstream H2 request with a
/// **streaming, bounded-incremental** body fed from the inbound H3
/// request DATA channel (`body_rx`, the M-A pump that H3→H1 already
/// proved). FIXES the dropped-request-body defect: the previous
/// `h3_to_h2_roundtrip` hard-wired `Full::new(Bytes::new())`, silently
/// deleting every request body.
///
/// Framing decision mirrors [`write_h1_request`]: peek the FIRST
/// `ReqBodyEvent` —
///   * `End` / channel-closed first ⇒ a legitimately **bodyless**
///     request (`Full::new(empty)` — content-length 0, NOT a dropped
///     body);
///   * `Reset` first ⇒ pre-dial abort: return `Err(413)` so the caller
///     emits the inline 413 and dials NOTHING (oversized / client
///     cancel before any data — smuggling guard parity with
///     `write_h1_request`'s pre-data `Reset`);
///   * `Chunk(b)` first ⇒ a **streaming** body: an
///     [`http_body_util::channel::Channel`] (capacity 1 — a single
///     in-flight frame; the REAL memory bound is `body_rx`'s
///     `H3_BODY_CHANNEL_DEPTH`, body-size independent) driven by a
///     spawned pump that forwards `Chunk → send_data`, `End → drop
///     sender` (clean EOS), and `Reset` / premature channel close →
///     `sender.abort(H3ReqAbort)`. A body error makes hyper
///     **RST_STREAM** the H2 upstream so a truncated request is NEVER
///     presented as complete (request-smuggling parity; H2
///     multiplexing ⇒ per-stream RST does not poison the connection —
///     no extra non-reusable bookkeeping, per lead A2).
///
/// REQUEST TRAILERS on the H2 leg are intentionally DROPPED (scoped
/// out for S6, parity with the H3→H1 P1-C decision: the body is fully
/// and correctly framed by hyper, so this is a lossless
/// RFC-acceptable downgrade, NOT silent body loss; explicitly
/// reported as a scoped-out item).
///
/// Returns the built `Request`, or `Err(status)` for the pre-dial
/// abort (413 oversized/cancel-before-data, 502 builder failure).
fn h2_request_body_from_rx(
    req: &H3Request,
    addr: std::net::SocketAddr,
    body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    first: Option<ReqBodyEvent>,
) -> Result<Request<lb_io::http2_pool::H2ReqBody>, u16> {
    // Build the request head (URI carries scheme+authority+path so
    // hyper emits the right pseudo-headers) — unchanged from the
    // pre-I2 roundtrip.
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
        // Bodyless: legitimately empty (content-length 0) — NOT a
        // dropped body. Byte-equivalent to the pre-I2 bodyless case.
        Some(ReqBodyEvent::End { .. }) | None => Full::<Bytes>::new(Bytes::new())
            .map_err(|never| match never {})
            .boxed(),
        // Reset before any data ⇒ pre-dial abort (413). Caller emits
        // the inline response and dials nothing.
        Some(ReqBodyEvent::Reset) => return Err(413),
        // Streaming body: the custom `H3ReqStreamBody` pulls `body_rx`
        // directly (one frame at a time → direct end-to-end
        // backpressure; in-flight window = H3_BODY_CHANNEL_DEPTH,
        // body-size independent). It errors on mid-body Reset so hyper
        // RST_STREAMs the upstream.
        Some(ReqBodyEvent::Chunk(b0)) => BoxBody::new(H3ReqStreamBody {
            body_rx,
            first: Some(b0),
            done: false,
        }),
    };

    builder.body(body).map_err(|_| 502u16)
}

/// S6 / H3→H2 R8 (M-B / I2/I3) — the streaming H3→H2 orchestrator.
/// The H2 cousin of [`h3_to_h1_stream_resp`], same `ReqBodyEvent` in /
/// `RespEvent` out channel contract:
///
/// 1. Peek the first request-body event; build the upstream H2 request
///    with a bounded-incremental streaming body
///    ([`h2_request_body_from_rx`]). A pre-data `Reset` ⇒ inline 413
///    (nothing dialled).
/// 2. `pool.send_request` (header roundtrip only — the body streams
///    afterwards). On pool error ⇒ inline 502. The pool already
///    evicts the peer entry on Send/Timeout (lead A2: sufficient
///    connection-level guard; the erroring body handles the
///    per-stream partial-request guard via RST_STREAM).
/// 3. Relay the H2 response back via [`stream_h2_response`]
///    (bounded-incremental, end-to-end backpressure, never a
///    `.collect()` / `Vec<u8>`).
///
/// Returns `Ok(())` once the response was fully piped (`RespEvent::End`
/// sent ⇒ actor FINs) or after an inline error response; or
/// `Err(RespAbort)` from [`stream_h2_response`] (actor RESET_STREAMs).
pub async fn h3_to_h2_stream_resp(
    req: &H3Request,
    addr: std::net::SocketAddr,
    pool: &Http2Pool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    resp_tx: tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort> {
    /// Emit a complete inline H3 response (Head+Body) then `End`.
    /// SESSION 24 / INC-3: emits DECODED events (the actor encodes via
    /// `quiche::h3`). Best-effort: a closed channel (client gone) just
    /// means nobody is listening. Identical helper to
    /// `h3_to_h1_stream_resp`'s.
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

    // Peek the FIRST body event (bounded — one event) to choose
    // framing, exactly as `write_h1_request` does.
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

/// Outcome of a single round-trip to an H3 upstream backend.
///
/// Carries the parsed response status, response field list, and body
/// bytes. Used by [`request_h3_upstream`] so non-H3 listeners
/// (`H1Proxy`, `H2Proxy`) can forward requests to an H3 backend and
/// convert the response back into their own wire format using the
/// `lb-l7` bridge crate.
#[derive(Debug)]
pub struct H3UpstreamResponse {
    /// `:status` pseudo-header value parsed from the response HEADERS
    /// frame.
    pub status: u16,
    /// Response field list. Pseudo-headers (`:status`) are filtered out
    /// — only regular headers remain — so callers can append their own
    /// `Content-Length` etc when bridging.
    pub headers: Vec<(String, String)>,
    /// Response body bytes assembled from all DATA frames received
    /// before stream-FIN.
    pub body: Vec<u8>,
    /// PROTO-2-12: response trailing field section (RFC 9114 §4.1)
    /// parsed from the HEADERS frame the upstream sends *after* its
    /// DATA frames. Pseudo-headers are filtered out. Empty when the
    /// upstream response carries no trailers.
    pub trailers: Vec<(String, String)>,
}

impl Default for H3UpstreamResponse {
    /// The bad-gateway shape: `502`, no headers/body/trailers. Used by
    /// [`request_h3_upstream`]'s error paths.
    fn default() -> Self {
        Self {
            status: 502,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        }
    }
}

/// Forward a pre-built H3 request to an upstream H3 backend via
/// [`QuicUpstreamPool`] and return the parsed response.
///
/// `headers` is the QPACK-encodable field list — callers must
/// pre-populate `:method`, `:scheme`, `:authority`, `:path` plus any
/// regular headers. `body` is forwarded as a single DATA frame; an
/// empty body sends FIN immediately after HEADERS.
///
/// PROTO-2-12: `trailers` is the request trailing field section. When
/// non-empty it is QPACK-encoded into a *second* HEADERS frame emitted
/// after the DATA frame (RFC 9114 §4.1), so wire order is
/// `HEADERS → DATA → HEADERS(trailers) → FIN`. The parsed response
/// likewise surfaces any post-DATA HEADERS frame as
/// [`H3UpstreamResponse::trailers`].
///
/// On any backend failure returns an [`H3UpstreamResponse`] with
/// `status = 502` and an empty body.
#[allow(clippy::too_many_lines, clippy::large_futures)]
pub async fn request_h3_upstream(
    headers: Vec<(String, String)>,
    body: bytes::Bytes,
    trailers: Vec<(String, String)>,
    addr: std::net::SocketAddr,
    sni: &str,
    pool: &QuicUpstreamPool,
) -> H3UpstreamResponse {
    let bad_gateway = H3UpstreamResponse::default;

    let mut pooled = match pool.acquire(addr, sni).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, %addr, "request_h3_upstream pool acquire failed");
            return bad_gateway();
        }
    };
    let Some(upstream) = pooled.get_mut() else {
        tracing::warn!("request_h3_upstream pool returned empty handle");
        return bad_gateway();
    };

    let encoder = QpackEncoder::new();
    let Ok(header_block) = encoder.encode(&headers) else {
        return bad_gateway();
    };
    let Ok(headers_frame) = encode_frame(&H3Frame::Headers { header_block }) else {
        return bad_gateway();
    };
    let body_frame_bytes: bytes::Bytes = if body.is_empty() {
        bytes::Bytes::new()
    } else {
        match encode_frame(&H3Frame::Data {
            payload: body.clone(),
        }) {
            Ok(b) => b,
            Err(_) => return bad_gateway(),
        }
    };
    // PROTO-2-12: RFC 9114 §4.1 trailing field section — a second
    // HEADERS frame after DATA. Encoded only when trailers are
    // present so a no-trailer request keeps the original
    // `HEADERS → DATA → FIN` shape.
    let trailers_frame_bytes: bytes::Bytes = if trailers.is_empty() {
        bytes::Bytes::new()
    } else {
        let Ok(trailer_block) = encoder.encode(&trailers) else {
            return bad_gateway();
        };
        match encode_frame(&H3Frame::Headers {
            header_block: trailer_block,
        }) {
            Ok(b) => b,
            Err(_) => return bad_gateway(),
        }
    };
    let mut request_bytes = Vec::with_capacity(
        headers_frame.len() + body_frame_bytes.len() + trailers_frame_bytes.len(),
    );
    request_bytes.extend_from_slice(&headers_frame);
    request_bytes.extend_from_slice(&body_frame_bytes);
    request_bytes.extend_from_slice(&trailers_frame_bytes);

    let stream_id: u64 = 0;
    let socket_clone = Arc::clone(upstream.socket());
    let local = upstream.local();
    let qconn_mut: &mut quiche::Connection = match upstream.connection_mut() {
        Some(c) => c,
        None => return bad_gateway(),
    };

    let mut sent_pos = 0usize;
    while sent_pos < request_bytes.len() {
        let chunk = request_bytes.get(sent_pos..).unwrap_or(&[]);
        let fin = sent_pos + chunk.len() >= request_bytes.len();
        match qconn_mut.stream_send(stream_id, chunk, fin) {
            Ok(n) => {
                if n == 0 {
                    break;
                }
                sent_pos = sent_pos.saturating_add(n);
            }
            Err(quiche::Error::Done) => break,
            Err(e) => {
                tracing::warn!(error = %e, "request_h3_upstream stream_send");
                pooled.set_reusable(false);
                return bad_gateway();
            }
        }
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut out_buf = vec![0u8; 65_535];
    let mut in_buf = vec![0u8; 65_535];
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_headers: Vec<(String, String)> = Vec::new();
    let mut decoded_trailers: Vec<(String, String)> = Vec::new();
    let mut decoded_body: Vec<u8> = Vec::new();
    let mut body_complete = false;
    let mut expected_len: Option<usize> = None;
    let mut stream_finished = false;

    while tokio::time::Instant::now() < deadline {
        while let Ok((n, info)) = qconn_mut.send(&mut out_buf) {
            let bytes = out_buf.get(..n).unwrap_or(&[]);
            if socket_clone.send_to(bytes, info.to).await.is_err() {
                break;
            }
        }

        let readable: Vec<u64> = qconn_mut.readable().collect();
        for sid in readable {
            if sid != stream_id {
                continue;
            }
            let mut chunk = [0u8; 8192];
            while let Ok((n, fin)) = qconn_mut.stream_recv(sid, &mut chunk) {
                rx_tail.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                if fin {
                    stream_finished = true;
                }
            }
        }

        loop {
            match decode_frame(&rx_tail, 1 << 20) {
                Ok((H3Frame::Headers { header_block }, consumed)) => {
                    rx_tail.drain(..consumed);
                    // PROTO-2-12 / RFC 9114 §4.1: the first HEADERS
                    // frame is the response head; any *subsequent*
                    // HEADERS frame (after DATA) is the trailing field
                    // section. Pseudo-headers are filtered out of the
                    // trailer list per RFC 9114 §4.3.
                    let is_trailers = decoded_status.is_some();
                    if let Ok(hdrs) = QpackDecoder::new().decode(&header_block) {
                        for (n, v) in hdrs {
                            if is_trailers {
                                if !n.starts_with(':') {
                                    decoded_trailers.push((n, v));
                                }
                            } else if n == ":status" {
                                decoded_status = v.parse::<u16>().ok();
                            } else if !n.starts_with(':') {
                                if n == "content-length" {
                                    expected_len = v.parse::<usize>().ok();
                                }
                                decoded_headers.push((n, v));
                            }
                        }
                    }
                }
                Ok((H3Frame::Data { payload }, consumed)) => {
                    rx_tail.drain(..consumed);
                    decoded_body.extend_from_slice(&payload);
                    if let Some(cl) = expected_len {
                        if decoded_body.len() >= cl {
                            body_complete = true;
                        }
                    }
                }
                Ok((_other, consumed)) => {
                    rx_tail.drain(..consumed);
                }
                Err(_) => break,
            }
        }

        if decoded_status.is_some() && (body_complete || stream_finished) {
            break;
        }

        let timeout = qconn_mut
            .timeout()
            .unwrap_or(std::time::Duration::from_millis(50));
        match tokio::time::timeout(timeout, socket_clone.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let info = quiche::RecvInfo { from, to: local };
                match qconn_mut.recv(slice, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(_) => break,
                }
            }
            Ok(Err(_)) | Err(_) => {
                qconn_mut.on_timeout();
            }
        }
    }

    pooled.set_reusable(false);

    H3UpstreamResponse {
        status: decoded_status.unwrap_or(502),
        headers: decoded_headers,
        body: decoded_body,
        trailers: decoded_trailers,
    }
}

/// SESSION 12 / CF-DEDUP-2 — the per-front RESPONSE SINK the shared
/// streaming connector [`stream_request_to_h3_upstream`] relays the
/// decoded upstream-H3 response through. Mechanism A2: the connector's
/// transport driver (quiche send/recv/timeout, the request-DATA pump,
/// the F-MD-4 abort discipline, the F-S7-6 idle deadline, the total-
/// `cap` DoS threshold) is FRONT-AGNOSTIC and shared; only the
/// response *emission* differs per front, and that difference lives
/// here.
///
/// * [`Wire`](Self::Wire) — the **H3 front** (H3→H3): each relay
///   RE-ENCODES to H3 wire frames — the response head via
///   [`encode_h3_headers_frame_full`] (`:status` plus the FULL
///   non-pseudo response header set, the CF-H3H3-HEAD fix; pre-S12 this
///   was the lossy status-and-content-length-only projection), the body
///   via `encode_h3_data_frame`, the trailers via
///   `encode_h3_trailers_frame` — and sends them on a
///   `Sender<RespEvent>`. The DATA and trailer framing and the cap
///   accounting reproduce the promoted H3→H3 cell byte-for-byte; only
///   the head now carries the full header set (the R12-mandated
///   convergence with the `Decoded` arm and `request_h3_upstream`).
/// * [`Decoded`](Self::Decoded) — an **H1/H2 front** (H1→H3 / H2→H3):
///   each relay forwards the DECODED [`H3RespEvent`] (FULL non-pseudo
///   header set in `Head`) on a `Sender<H3RespEvent>`, so the L7 front
///   runs its own head-transform without re-decoding H3.
///
/// Cumulative `total` + `cap` (the DoS abort threshold, NOT a memory
/// mechanism — identical role to [`stream_h2_response`]) live here so
/// the `Wire` arm's cap accounting (`total += frame.len()`) is the
/// EXACT pre-S12 logic; the `Decoded` arm tracks decoded payload
/// length for the same threshold role. The driver owns the F-S7-6
/// idle-deadline reset (it fires after each relay method returns
/// `Ok`), so progress-tracking is unchanged.
pub enum H3RespOut {
    /// H3 front: re-encode to H3 wire frames (byte-identical to the
    /// pre-S12 inline H3→H3 path) onto a [`RespEvent`] channel.
    Wire {
        /// Pre-encoded H3 wire-byte channel back to the actor.
        tx: tokio::sync::mpsc::Sender<RespEvent>,
        /// Cumulative encoded-frame bytes relayed (cap accounting).
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
    /// Emit a complete inline response (head + body, then `End`).
    /// Best-effort: a closed channel (client gone) just means nobody
    /// is listening — same as the pre-S12 `inline` helper.
    ///
    /// `Wire`: emits a decoded `RespEvent::Head { status, .. }` +
    /// `RespEvent::Body(body)` (when non-empty) + `End` (SESSION 24 / INC-3:
    /// the actor now re-encodes via `quiche::h3::send_response`/`send_body`).
    /// `Decoded`: a synthesized `Head { status, headers: [] }` +
    /// `Body(body)` (when non-empty) + `End`.
    async fn inline(&mut self, status: u16, body: &[u8]) {
        match self {
            Self::Wire { tx, .. } => {
                // SESSION 24 / INC-3: emit DECODED Head + Body + End
                // (the actor encodes). No encode step now, so there is
                // no Reset-on-encode-failure path; a closed channel just
                // means nobody is listening.
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

    /// Relay the response HEAD. `fields` is the FULL decoded response
    /// field list (incl. pseudo-headers).
    ///
    /// `Wire`: re-encodes `:status` + the FULL non-pseudo response
    /// header set (content-type / cache-control / set-cookie / custom
    /// headers, with `content-length` retained as a regular header) via
    /// [`encode_h3_headers_frame_full`] — CF-H3H3-HEAD fix. Pre-S12 this
    /// arm projected to `:status` + `content-length` ONLY (lossy); it
    /// now forwards the full set, converging with the `Decoded` arm and
    /// the buffering `request_h3_upstream`.
    /// `Decoded`: forward `Head { status, headers }` with pseudo-
    /// headers filtered out and `content-length` retained as a regular
    /// header (FULL set — correct proxy behaviour for the L7 fronts).
    async fn on_head(&mut self, fields: &[(String, String)]) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, total, cap } => {
                // CF-H3H3-HEAD: forward the FULL non-pseudo set —
                // `:status` parsed out, every other non-pseudo field
                // re-encoded verbatim (`content-length` rides through as
                // a regular header). CF-H3-HEAD: strip response-direction
                // hop-by-hop fields. This is REQUIRED, not just R12
                // tidiness — RFC 9114 §4.2: "An endpoint MUST NOT
                // generate an HTTP/3 field section containing
                // connection-specific header fields." Forwarding the full
                // set WITHOUT this strip (as the bcb4f09a head fix did)
                // would relay a non-conformant H3 upstream's
                // `connection`/`transfer-encoding` onto the H3 client — a
                // §4.2 violation; the strip closes it. Result: this Wire
                // leg's transform is bit-for-bit equivalent to the
                // H3→H1 / H3→H2 wire legs on the same input (R12).
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
                // SESSION 24 / INC-3: emit the DECODED head (the actor
                // encodes via `send_response`). The hop-by-hop strip +
                // :status parse above are UNCHANGED (RFC 9114 §4.2);
                // `cap`/`total` counts decoded header bytes.
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
    ///
    /// `Wire`: emits `RespEvent::Body(slice)` (SESSION 24 / INC-3 — the
    /// actor encodes the DATA frame via `quiche::h3::send_body`).
    /// `Decoded`: `H3RespEvent::Body(slice)`.
    async fn on_data(&mut self, slice: &[u8]) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, total, cap } => {
                // SESSION 24 / INC-3: emit DECODED Body (the actor
                // encodes via `send_body`); `cap`/`total` counts payload.
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
    ///
    /// `Wire`: emits `RespEvent::Trailers(trailers)` (SESSION 24 / INC-3 —
    /// the actor encodes the trailing HEADERS via
    /// `quiche::h3::send_additional_headers`).
    /// `Decoded`: `H3RespEvent::Trailers(trailers)`.
    async fn on_trailers(&mut self, trailers: Vec<(String, String)>) -> Result<(), RespAbort> {
        match self {
            Self::Wire { tx, total, cap } => {
                // SESSION 24 / INC-3: emit DECODED trailers (the actor
                // encodes via `send_additional_headers`); `cap`/`total`
                // counts decoded trailer bytes.
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

    /// Best-effort abort signal — the actor / L7 front RESETs the
    /// client and never FINs. A closed channel is ignored (nobody
    /// listening).
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

/// SESSION 7 (H3→H3 R8) / SESSION 12 (CF-DEDUP-2): the H3→H3 cell's
/// streaming response producer. Since S12 this is a thin front for the
/// shared, front-agnostic connector [`stream_request_to_h3_upstream`]:
/// it builds the upstream request field list from the inbound
/// [`H3Request`] (verbatim pre-S12 order — `:method`, `:scheme=https`,
/// `:authority` with the `sni` fallback, `:path`) and drives the
/// connector with an [`H3RespOut::Wire`] sink (re-encoding the decoded
/// response back to BYTE-IDENTICAL H3 wire frames) and
/// `forward_req_trailers = false` (the H3→H3 request-trailer DROP —
/// parity H3→H1 P1-C / H3→H2 A3, preserved byte-identically). The
/// conn_actor call site + the H3→H3 e2e suite are unchanged.
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
    // Build the upstream request HEADERS field list — byte-identical
    // to the pre-S12 inline build (same order: method, scheme=https,
    // authority [with the `sni` fallback when empty], path) so the
    // QPACK header block + HEADERS frame bytes are identical.
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

/// SESSION 7 (H3→H3 R8): bounded streaming H3-upstream connector,
/// the H3→H3 analogue of [`h3_to_h2_stream_resp`]. Replaces the
/// former buffered, body-dropping H3→H3 round-trip (which accumulated
/// the whole response into a `decoded_body: Vec<u8>` and forwarded no
/// request body — deleted in J3) with a
/// connector that re-emits the upstream H3 response frame-by-frame
/// onto the bounded sink, retaining memory bounded ONLY by a
/// fixed in-flight window (`H3_RESP_CHANNEL_DEPTH ×
/// (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)` + one in-hand frame) —
/// response-size INDEPENDENT, never a `Vec<u8>` body, never
/// `.collect()`, never sized from `content-length` / the total-body
/// `cap` (which stays ONLY a DoS abort threshold, identical role to
/// [`stream_h1_response`]/[`stream_h2_response`]).
///
/// # SESSION 12 / CF-DEDUP-2 (mechanism A2)
///
/// Extracted from the former `h3_to_h3_stream_resp` so the SAME
/// transport driver serves all three `→H3` cells. The request field
/// list arrives PRE-BUILT (`headers`) — the caller (H3→H3 via
/// [`h3_to_h3_stream_resp`], or H1→H3 / H2→H3 via the `lb-l7` bridge)
/// owns building it. The response is relayed through the per-front
/// [`H3RespOut`] sink: `Wire` reproduces the H3→H3 wire bytes
/// byte-identically; `Decoded` yields [`H3RespEvent`] for an L7 front.
/// `forward_req_trailers` gates the request-trailer leg (see below).
///
/// # Build scope (J1 recv half + J2 send half)
///
/// J1 added the orchestrator skeleton + the M-C **recv half**
/// (response ingress). J2 added the M-C **request send half**: the
/// streaming request-DATA pump (peeked-first chunk, `stream_capacity`-
/// gated incremental DATA, mid-body abort). J3 made this the LIVE
/// H3→H3 path: [`crate::conn_actor`]'s `h3_backend` branch spawns it
/// on the bounded `resp_tasks` streaming path (the former buffered
/// round-trip + its legacy `request_tasks` Vec wiring were deleted).
///
/// ### Request-event peek (`body_rx`)
/// * `End` / channel-closed first ⇒ legitimately **bodyless** request:
///   send HEADERS + FIN, byte-identical to the former buffered H3→H3
///   path's bodyless GET — no regression.
/// * `Reset` first ⇒ pre-dial abort (oversized / cancel before any
///   data): inline `413`, dial NOTHING (smuggling-guard parity with
///   [`h3_to_h2_stream_resp`]).
/// * `Chunk(b0)` first ⇒ a **streaming request body** (J2): `b0` is
///   carried as the first in-hand chunk (parity with
///   [`H3ReqStreamBody`]'s peeked `first`); subsequent
///   [`ReqBodyEvent`]s are pulled one-at-a-time at the loop's single
///   park point, each forwarded as ONE bounded H3 DATA frame only
///   while `stream_capacity` has room. `End` ⇒ a QUIC stream FIN
///   (request trailers DROPPED — parity H3→H1 P1-C / H3→H2 A3; the
///   body is fully framed by the FIN, a lossless RFC-acceptable
///   downgrade, NOT silent loss). Mid-body `Reset` / producer dropped
///   before `End` ⇒ NO FIN +
///   `stream_shutdown(Write, H3_REQUEST_CANCELLED)` + non-reusable
///   (BINDING case-7: the upstream never sees a truncated-as-complete
///   request).
///
/// ### M-C recv half (the R8 core — replaces `decoded_body`)
/// Drives the pooled `quiche::Connection` send/recv/timeout loop (the
/// same proven pooled-quiche-conn driver shape [`request_h3_upstream`]
/// uses) but with the
/// whole-response `Vec<u8>` accumulation **deleted**. Because
/// [`lb_h3::decode_frame`] only yields a frame once its ENTIRE
/// payload is buffered (it would force buffering a multi-MiB DATA
/// frame — the R8 trap), this path parses the H3 frame **header
/// only** (frame-type + payload-length varints) via the already-
/// public [`lb_h3::decode_varint`] — the SAME discipline as the
/// R8-verified M-A ingress parser ([`StreamRxBuf::try_parse_frame_header`]
/// / its [`MAX_FRAME_HEADER_BYTES`] partial-header bound) — then:
/// * HEADERS / trailing-HEADERS / control frames: small; the declared
///   `payload_len` is bounded by `DEFAULT_MAX_PAYLOAD_SIZE` (the SAME
///   limit [`decode_frame`] enforced on the old buffered path — G1
///   DoS-rejection parity) BEFORE buffering exactly that payload for
///   QPACK.
/// * DATA frames: the declared `payload_len` is **never** used to
///   size a buffer (binding condition 3); the payload is streamed in
///   `≤ H3_RESP_CHUNK_MAX` slices through the per-front [`H3RespOut`]
///   sink (`Wire`: re-encoded via [`encode_h3_data_frame`];
///   `Decoded`: forwarded as [`H3RespEvent::Body`]) and dropped — with
///   the cumulative response total `cap`-tracked IN THE SINK ⇒
///   `Err(RespAbort::OverCap)` past the sink's `cap`, identical to
///   [`stream_h2_response`].
///
/// The sink's `send(..).await` is the response-direction backpressure
/// gate (native quiche, no hyper): a stalled client ⇒ the actor / L7
/// front stops draining ⇒ the bounded channel (depth 8) fills ⇒ this
/// fn parks ⇒ it stops calling `stream_recv` on the upstream conn ⇒
/// quiche withholds `MAX_STREAM_DATA` ⇒ the upstream H3 server's send
/// window closes.
///
/// On EVERY return path the pooled upstream conn is marked
/// non-reusable (parity with the former buffered H3→H3 path; one
/// request per pooled upstream conn — pooling efficiency is
/// explicitly out of R8 scope, S-2).
///
/// # Errors
///
/// Returns `Err(RespAbort)` (the SAME contract as
/// [`stream_h2_response`]): a partial / premature-FIN / decode-error /
/// upstream-reset response is **never** terminated with a clean end —
/// only a best-effort sink `Reset` + `Err(RespAbort::*)`, so the
/// actor / L7 front RESETs the client and never FINs (response-
/// splitting / cache-poisoning guard). A closed sink channel (client
/// cancelled) ⇒ `Err(RespAbort::ClientGone)`.
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
    // F-S7-6 no-forward-progress idle deadline (UNCHANGED): reset on every
    // bidirectional application-data progress event only (send_body n>0,
    // recv_body n>0, a successful sink relay); NEVER on keepalive/timer/0-byte.
    let mut idle_deadline = tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
    macro_rules! send_progress {
        ($call:expr) => {{
            $call?;
            idle_deadline = tokio::time::Instant::now() + H3_RESP_IDLE_TIMEOUT;
        }};
    }

    // --- peek the FIRST request body event (UNCHANGED) ---
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

    // --- acquire the pooled upstream H3 conn (UNCHANGED) ---
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

    // SESSION 25 / INC-4: wrap the established pooled QUIC conn as a quiche::h3
    // CLIENT (with_transport sends SETTINGS + opens the client control + QPACK
    // enc/dec uni streams — the hand-rolled `encode_frame(Headers)` /
    // `QpackEncoder` / control setup are GONE). The conn is established (the
    // pool guarantees it) + used once (set_reusable(false) every exit).
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

    // Build the upstream request HEADERS from the caller-supplied field list
    // (verbatim; pseudo-headers included). quiche QPACK-encodes (Huffman) +
    // frames the HEADERS internally — `headers_fin` matches the former path:
    // FIN here ONLY for a bodyless request with NO trailers to forward.
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

    // Bodyless request WITH forwarded trailers (L7 fronts only): a trailing
    // field section + FIN, no DATA (RFC 9114 §4.1). Clients may only send
    // trailer headers (quiche enforces is_trailer_section). On encode/limit
    // error: abort WITHOUT FIN (case-7 — never a truncated-as-complete req).
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

    // --- request-DATA send state. send_body frames the DATA internally, so
    // ReqSend::InHand holds RAW chunk bytes (no encode_h3_data_frame). The
    // real memory bound is the depth-8 body_rx (request-size INDEPENDENT). ---
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
        // Bodyless: HEADERS already FIN'd (headers_fin) OR the trailing field
        // section FIN'd it above. Nothing more to send.
        ReqSend::Ended
    };

    let mut scratch = [0u8; H3_RESP_CHUNK_MAX];
    let mut out_buf = vec![0u8; 65_535];
    let mut in_buf = vec![0u8; 65_535];
    let mut sent_head = false;
    let mut response_complete = false;
    let mut outcome: Result<(), RespAbort> = Ok(());

    // SESSION 25 / INC-4: drain the response body for `stream_id` into the
    // bounded sink, ≤`H3_RESP_CHUNK_MAX` per slice. `sink.on_data().await` is
    // the R8 RESPONSE backpressure point (blocks when the downstream channel is
    // full ⇒ we stop `recv_body` ⇒ quiche stops extending the response-stream
    // flow-control window ⇒ the backend pauses). The scratch is fixed; the body
    // is NEVER whole-buffered. A mid-body `recv_body` error (peer RESET_STREAM /
    // final-size — F-MD-4) maps to an upstream reset, NEVER a clean EOF.
    //
    // Used on a `Data` event AND unconditionally after the poll loop (PASS-3):
    // quiche-0.28 does not re-arm the `Data` event after a 0-length DATA frame
    // while the stream stays readable (`stream.rs` `try_consume_data` only
    // `reset_data_event`s when `!stream_readable`), so `poll` can advance the
    // NEXT DATA frame into `State::Data` WITHOUT emitting a fresh `Data` event;
    // the unconditional drain relays that body (server `poll_h3` PASS-1 analogue).
    // The outer-loop label is passed in (`$evloop`) because macro-hygienic
    // labels cannot otherwise break a label defined at the call site.
    macro_rules! drain_resp_body {
        ($evloop:lifetime, $progressed:ident) => {{
            loop {
                match h3.recv_body(qconn, stream_id, &mut scratch) {
                    Ok(0) => break,
                    Ok(n) => {
                        let slice = scratch.get(..n).unwrap_or(&[]);
                        match sink.on_data(slice).await {
                            Ok(()) => {
                                $progressed = true;
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
        // Set by `drain_resp_body!` on any relayed body this tick; drives the
        // post-poll "re-poll instead of park" decision (below).
        let mut progressed = false;
        // (a) request-DATA egress — send_body is flow-control-gated: Done ⇒ the
        // window is closed, keep the chunk in hand and do NOT pull body_rx, so
        // the depth-8 channel fills and the M-A pump pauses the downstream
        // client's upload (request-direction backpressure, native quiche).
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

        // (b) flush egress to the wire.
        while let Ok((n, info)) = qconn.send(&mut out_buf) {
            let bytes = out_buf.get(..n).unwrap_or(&[]);
            if socket_clone.send_to(bytes, info.to).await.is_err() {
                break;
            }
        }

        // (c) response ingress — one event per poll until Done.
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
                        // First HEADERS ⇒ response head. The sink owns
                        // :status parse + hop-by-hop strip + cap accounting.
                        send_progress!(sink.on_head(&fields).await);
                        sent_head = true;
                    } else {
                        // RFC 9114 §4.3: a pseudo-header in the trailing field
                        // section is malformed ⇒ Reset, never forwarded.
                        if fields.iter().any(|(n, _)| n.starts_with(':')) {
                            sink.on_reset().await;
                            outcome = Err(RespAbort::BadHead);
                            break 'evloop;
                        }
                        if !fields.is_empty() {
                            // Faithful to the original: explicit match→break
                            // (NOT send_progress!'s `?`), so the post-loop
                            // on_reset runs on a relay error. (on_head DOES use
                            // send_progress! `?` — the sink already Reset on its
                            // OverCap/ClientGone error paths.)
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
                    drain_resp_body!('evloop, progressed);
                }
                Ok((sid, quiche::h3::Event::Finished)) if sid == stream_id => {
                    // F-MD-4 MIRROR (E2's highest-risk property). quiche delivers
                    // Finished (NOT Reset) for a response stream the BACKEND RESET
                    // *after* its last DATA frame (quiche-0.28 mod.rs:2072 first
                    // finished_streams pop lacks the reset re-check the :2106 second
                    // pop has). Treating that as a clean end would FIN the
                    // DOWNSTREAM client on a truncated response (response-splitting,
                    // reversed). Probe the transport exactly as quiche's own guard
                    // does — a zero-length stream_recv returns StreamReset for a
                    // reset stream — and map it to on_reset, never on_end. A
                    // headerless Finished is a premature EOF (also never a clean
                    // end). Mirror of conn_actor.rs:1269.
                    let was_reset = matches!(
                        qconn.stream_recv(stream_id, &mut []),
                        Err(quiche::Error::StreamReset(_))
                    );
                    if was_reset {
                        tracing::debug!(
                            stream_id,
                            "INC-4 F-MD-4: Finished on a RESET response stream; \
                             Reset downstream (not a clean End)"
                        );
                        outcome = Err(RespAbort::UpstreamReset);
                    } else if sent_head {
                        response_complete = true;
                    } else {
                        outcome = Err(RespAbort::PrematureEof);
                    }
                    break 'evloop;
                }
                Ok((sid, quiche::h3::Event::Reset(code))) if sid == stream_id => {
                    // Genuine backend reset before/without Finished ⇒ never End.
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
                    // quiche enforces #11/#16-22/#24 itself: on a control / QPACK /
                    // frame-sequence violation it has already closed the conn. Map
                    // to an upstream reset (never a forwarded / false-complete
                    // response). The downstream client is RESET post-loop.
                    tracing::warn!(error = %e, "INC-4: h3 poll (upstream protocol error)");
                    outcome = Err(RespAbort::UpstreamReset);
                    break 'evloop;
                }
            }
        }

        // PASS-3 (edge-trigger safety): poll may have advanced the next DATA
        // frame into `State::Data` WITHOUT emitting a `Data` event (the
        // quiche-0.28 0-length-DATA re-arm gap above). Relay any such body now,
        // every tick, so it is never stranded behind an empty DATA frame.
        if sent_head && !response_complete {
            drain_resp_body!('evloop, progressed);
        }

        if response_complete {
            break 'evloop;
        }

        // If a drain made forward progress this tick, the next event(s) (more
        // body, trailing HEADERS, or `Finished`) are ALREADY queued in quiche
        // and need NO socket I/O — re-poll immediately rather than parking on
        // the socket (the park would wait out the full quiche timeout while the
        // `Finished`/trailer event sits ready, stalling the response). Bounded:
        // `progressed` requires ≥1 relayed body byte, so this cannot spin.
        if progressed {
            continue 'evloop;
        }

        // (d) the SINGLE park point — one await on {upstream socket readable |
        // next request-body event (ONLY while AwaitNext) | quiche timeout}.
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
                        // Mid-body client RESET / producer dropped before End ⇒ the
                        // upstream must NEVER see a completable (truncated-as-
                        // complete) request (BINDING case-7). No FIN; RESET the
                        // request stream with H3_REQUEST_CANCELLED.
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
        // Idle deadline fell through without a clean end — premature EOF;
        // NEVER End a partial response.
        sink.on_reset().await;
        return Err(RespAbort::PrematureEof);
    }
    // Aborted mid-response — ensure a Reset is sent (never End).
    sink.on_reset().await;
    outcome
}

/// SESSION 7 / J2: the request-send action the H3→H3 connector takes
/// for the next `ReqBodyEvent` pulled at its single park point. The
/// classification is factored out (module-level, like J1's
/// [`check_block_len`]) so the binding decision is exercised by the
/// `s7_j2_request_send_decision` pure unit test against the REAL code
/// — not a test-only re-statement.
#[derive(Debug, PartialEq, Eq)]
enum J2ReqAction {
    /// `Chunk` ⇒ forward as one bounded request-body chunk. SESSION 25 /
    /// INC-4: carries the RAW chunk bytes (the ONLY retained request
    /// bytes); `quiche::h3::send_body` frames the DATA internally, so there
    /// is no pre-encoded frame and no encode-failure path.
    SendData(Bytes),
    /// `End` ⇒ clean end-of-request: terminate the upstream request
    /// stream with a QUIC stream FIN (J2-G2), request trailers
    /// DROPPED (H3→H3 leg — parity H3→H1 P1-C / H3→H2 A3 — and the
    /// no-trailer / `forward_req_trailers=false` case generally).
    FinNoTrailers,
    /// SESSION 12 / RISK-3: `End { trailers }` with non-empty trailers
    /// AND `forward_req_trailers` ⇒ ship a post-DATA HEADERS(trailers)
    /// frame THEN FIN (RFC 9114 §4.1). Only produced for an L7 front
    /// (H1→H3 / H2→H3) that forwards request trailers; never for H3→H3
    /// (which passes `forward_req_trailers=false` ⇒ `FinNoTrailers`,
    /// byte-identical drop).
    FinWithTrailers(Vec<(String, String)>),
    /// `Reset` / channel-closed-before-`End` ⇒ mid-body abort: NO
    /// FIN, `stream_shutdown(Write, H3_REQUEST_CANCELLED)` (case-7
    /// request-smuggling parity).
    AbortNoFin,
}

/// SESSION 7 / J2 (+ SESSION 12 / RISK-3): classify the next
/// request-body event into its send action. `None` models a closed
/// `body_rx` (producer dropped before a clean `End`) — treated
/// identically to a mid-body `Reset` (never a truncated-as-complete
/// request). SESSION 25 / INC-4: a `Chunk` always maps to
/// `SendData(raw bytes)` (no encode step — `send_body` frames it).
///
/// `forward_req_trailers`: when `false` (H3→H3), `End { trailers }`
/// ALWAYS maps to `FinNoTrailers` — byte-identical to the pre-S12
/// behaviour (request trailers dropped). When `true` (L7 fronts) a
/// non-empty trailing field section maps to `FinWithTrailers` so the
/// connector ships it as a post-DATA HEADERS frame before FIN.
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

    // ── SESSION 22 (h3spec #12–15): request pseudo-header validation ──
    // Helper: build a (name, value) field list.
    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn pseudo_valid_request_accepted_negative_control() {
        // The load-bearing negative control: a well-formed request MUST
        // pass (so the validator does not reject legitimate traffic — R8
        // rejects only malformed input).
        let ok = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":authority", "example.com"),
            ("user-agent", "h3spec"),
        ]);
        assert!(validate_request_pseudo_headers(&ok).is_ok());
        // Minimal valid https request: :method/:scheme/:path + :authority
        // (§4.3.1 makes :authority-or-Host mandatory for http/https).
        let min = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":authority", "h"),
        ]);
        assert!(validate_request_pseudo_headers(&min).is_ok());
    }

    #[test]
    fn pseudo_13_absent_authority_rejected_for_http_scheme() {
        // #13 (owner ruling: strict) — an http/https request with neither
        // :authority nor Host is malformed (RFC 9114 §4.3.1).
        let neither = h(&[(":method", "GET"), (":scheme", "https"), (":path", "/")]);
        assert!(
            validate_request_pseudo_headers(&neither).is_err(),
            "https request with no :authority and no Host must be rejected"
        );
        // The Host field is the §4.3.1 alternative to :authority.
        let with_host = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            ("host", "example.com"),
        ]);
        assert!(
            validate_request_pseudo_headers(&with_host).is_ok(),
            "Host is a valid alternative to :authority (§4.3.1)"
        );
    }

    #[test]
    fn pseudo_12_duplicate_rejected() {
        // #12 — duplicated request pseudo-header (RFC 9114 §4.3.1).
        let dup_method = h(&[
            (":method", "GET"),
            (":method", "POST"),
            (":scheme", "https"),
            (":path", "/"),
        ]);
        assert!(validate_request_pseudo_headers(&dup_method).is_err());
        let dup_path = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":path", "/x"),
        ]);
        assert!(validate_request_pseudo_headers(&dup_path).is_err());
    }

    #[test]
    fn pseudo_13_missing_mandatory_rejected() {
        // #13 — mandatory pseudo-headers absent (RFC 9114 §4.3.1).
        let no_method = h(&[(":scheme", "https"), (":path", "/")]);
        assert!(validate_request_pseudo_headers(&no_method).is_err());
        let no_path = h(&[(":method", "GET"), (":scheme", "https")]);
        assert!(validate_request_pseudo_headers(&no_path).is_err());
        let no_scheme = h(&[(":method", "GET"), (":path", "/")]);
        assert!(validate_request_pseudo_headers(&no_scheme).is_err());
    }

    #[test]
    fn pseudo_14_prohibited_or_unknown_rejected() {
        // #14 — response-only :status and any unknown :-prefixed name are
        // prohibited in a request (RFC 9114 §4.3).
        let status_in_req = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":status", "200"),
        ]);
        assert!(validate_request_pseudo_headers(&status_in_req).is_err());
        let unknown = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            (":path", "/"),
            (":madeup", "x"),
        ]);
        assert!(validate_request_pseudo_headers(&unknown).is_err());
    }

    #[test]
    fn pseudo_15_after_regular_field_rejected() {
        // #15 — a pseudo-header after a regular field (RFC 9114 §4.3).
        let after = h(&[
            (":method", "GET"),
            (":scheme", "https"),
            ("user-agent", "h3spec"),
            (":path", "/"),
        ]);
        assert!(validate_request_pseudo_headers(&after).is_err());
    }

    // ── SESSION 22 (h3spec #11/#21): request-stream frame sequencing ──

    #[test]
    fn pseudo_connect_request_rules() {
        // RFC 9114 §4.4: CONNECT omits :scheme/:path and needs :authority.
        let ok = h(&[(":method", "CONNECT"), (":authority", "example.com:443")]);
        assert!(validate_request_pseudo_headers(&ok).is_ok());
        let bad_has_path = h(&[
            (":method", "CONNECT"),
            (":authority", "example.com:443"),
            (":path", "/"),
        ]);
        assert!(validate_request_pseudo_headers(&bad_has_path).is_err());
        let bad_no_authority = h(&[(":method", "CONNECT")]);
        assert!(validate_request_pseudo_headers(&bad_no_authority).is_err());
    }

    #[test]
    fn encode_h3_response_includes_status_and_body() {
        let bytes = encode_h3_response(200, b"hello").unwrap();
        // Decode our own output: two frames, HEADERS then DATA.
        let (f1, c1) = decode_frame(&bytes, 1 << 20).unwrap();
        let tail = bytes.get(c1..).unwrap();
        let (f2, _c2) = decode_frame(tail, 1 << 20).unwrap();
        let H3Frame::Headers { header_block } = f1 else {
            panic!("expected HEADERS");
        };
        let headers = QpackDecoder::new().decode(&header_block).unwrap();
        assert!(headers.iter().any(|(n, v)| n == ":status" && v == "200"));
        assert!(
            headers
                .iter()
                .any(|(n, v)| n == "content-length" && v == "5")
        );
        match f2 {
            H3Frame::Data { payload } => assert_eq!(payload.as_ref(), b"hello"),
            _ => panic!("expected DATA"),
        }
    }

    #[test]
    fn build_h1_request_none_body_is_byte_identical_to_legacy_bodyless() {
        // S1-B behaviour-preservation guard: `None` MUST reproduce the
        // exact pre-seam bytes (`Content-Length: 0` + `Connection:
        // close`, no body). This is what every datapath caller passes
        // in SESSION 1, so the seam changes nothing on the wire.
        let req = H3Request {
            method: "GET".to_string(),
            path: "/p".to_string(),
            authority: "host.test:443".to_string(),
            extra: Vec::new(),
            trailers: Vec::new(),
        };
        let got = build_h1_request(&req, None);
        let expected = "GET /p HTTP/1.1\r\n\
                        Host: host.test:443\r\n\
                        Content-Length: 0\r\n\
                        Connection: close\r\n\r\n";
        assert_eq!(got, expected.as_bytes());
    }

    /// Request-body forwarding through the H3→H1 bridge — the S1-B
    /// seam's CONTRACT (correct `Content-Length` + appended payload via
    /// `build_h1_request`'s `Some(body)` arm).
    ///
    /// F-COR-6 (auditor-4 F-2): the SESSION 2 datapath this test
    /// targets is BUILT. S2 P1-A (commit `f2af73c4`) landed
    /// `conn_actor::poll_h3` inbound H3 DATA-frame accumulation and the
    /// streaming forward path passes the accumulated body into
    /// `build_h1_request`; this is e2e-proven green (3/3 this session):
    /// `h3_h1_stream_body_e2e::t1_multi_data_frame_binary_body_
    /// forwarded_byte_identical`, `..::t5_single_large_data_frame_is_
    /// memory_bounded_through_stalled_upstream`, and
    /// `h3_to_h1_forwards_non_utf8_body_byte_for_byte`. The prior
    /// `#[ignore = "S2: request-body forwarding"]` + "datapath …
    /// UNBUILT … (no caller passes `Some` yet)" justification was
    /// STALE-FALSE: the assertion passes verbatim against shipped S2
    /// code (`build_h1_request` h3_bridge.rs `Some(..)` arm). The
    /// `#[ignore]` is removed so the now-passing contract is actually
    /// asserted in the gate (R5: never leave a passing test masked
    /// behind a false "unbuilt" marker).
    #[test]
    fn s2_target_build_h1_request_with_body_sets_content_length_and_appends_payload() {
        let req = H3Request {
            method: "POST".to_string(),
            path: "/submit".to_string(),
            authority: "api.test".to_string(),
            extra: Vec::new(),
            trailers: Vec::new(),
        };
        let body = b"hello-s2-body";
        let got = build_h1_request(&req, Some(body));
        let expected = format!(
            "POST /submit HTTP/1.1\r\n\
             Host: api.test\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             hello-s2-body",
            body.len()
        );
        assert_eq!(got, expected.as_bytes());
    }

    /// SESSION 2 / P1-C: the response-side total-size ceiling. A
    /// backend that streams MORE than the cap must make
    /// `read_h1_response_capped` return `Err` (which the caller maps to
    /// a clean H3 502) rather than buffering unboundedly → OOM; a
    /// conformant under-cap response must still parse correctly (no
    /// regression). Both halves are exercised against a REAL localhost
    /// TCP backend through the EXACT function `read_h1_response` calls
    /// in production (it is the sole production caller, passing
    /// `MAX_RESPONSE_BODY_BYTES`). A tiny `cap` is used so the test is
    /// fast/deterministic — a literal 64 MiB transfer is impractical on
    /// a 2-CPU/7 GB box and would only re-test a single `>` compare.
    /// The named const's production value is pinned below; sub-cap
    /// large-binary end-to-end correctness is
    /// `tests/h3_h1_trailers_resp_e2e.rs::pc2`.
    #[tokio::test]
    async fn read_h1_response_capped_rejects_over_cap_and_passes_under_cap() {
        use tokio::io::AsyncWriteExt as _;
        use tokio::net::{TcpListener, TcpStream};

        // Pin the production ceiling (single source of truth, mirrors
        // MAX_REQUEST_BODY_BYTES).
        assert_eq!(MAX_RESPONSE_BODY_BYTES, 64 * 1024 * 1024);

        // (A) OVER-CAP: backend sends a valid head then a body LARGER
        // than the (tiny) cap. Must return Err, not buffer it all.
        let l1 = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let a1 = l1.local_addr().unwrap();
        let big = vec![0xABu8; 70_000]; // > the 64 KiB test cap below
        let s1 = tokio::spawn(async move {
            let (mut s, _) = l1.accept().await.unwrap();
            let mut t = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut s, &mut t).await;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", big.len());
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(&big).await;
            let _ = s.shutdown().await;
        });
        let mut c1 = TcpStream::connect(a1).await.unwrap();
        c1.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
        let cap = 64 * 1024usize;
        let over = super::read_h1_response_capped(&mut c1, cap).await;
        let _ = s1.await;
        let err = over.expect_err("over-cap response must return Err");
        assert!(
            err.contains(&format!("exceeds {cap} bytes")),
            "Err must name the cap; got: {err}"
        );

        // (B) UNDER-CAP no-regression: a small conformant response
        // through the SAME function still parses status + body exactly.
        let l2 = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let a2 = l2.local_addr().unwrap();
        let s2 = tokio::spawn(async move {
            let (mut s, _) = l2.accept().await.unwrap();
            let mut t = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut s, &mut t).await;
            s.write_all(b"HTTP/1.1 206 X\r\nContent-Length: 3\r\n\r\n\xFF\x00\x80")
                .await
                .unwrap();
            s.shutdown().await.unwrap();
        });
        let mut c2 = TcpStream::connect(a2).await.unwrap();
        c2.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
        let ok = super::read_h1_response_capped(&mut c2, cap)
            .await
            .expect("under-cap response must parse");
        s2.await.unwrap();
        assert_eq!(ok.status, 206);
        assert_eq!(ok.body.as_ref(), &[0xFF, 0x00, 0x80]);
    }

    /// F-COR-1 (b) — RFC 9114 §4.3: a pseudo-header field in the H3
    /// trailing field section is malformed. `feed_body` MUST return
    /// `Err` (mapped to `ReqBodyEvent::Reset` / PROTOCOL_ERROR-class in
    /// conn_actor.rs), never push `BodyItem::Trailers`. Pre-fix it
    /// pushed the trailers with no pseudo check.
    /// No-regression: a VALID (non-pseudo) H3 trailer is still accepted
    /// and surfaced as `BodyItem::Trailers` — the §4.3 rejection must be
    /// surgical (only `:`-prefixed names), not a blanket trailer break.
    /// SESSION 4 / P1-A no-regression contract: the refactored
    /// `encode_h3_headers_frame(status, Some(len)) + encode_h3_data_frame`
    /// is BYTE-IDENTICAL to the monolithic `encode_h3_response` (every
    /// existing CL backend + test client depends on this).
    #[test]
    fn encode_h3_response_is_byte_identical_to_split_helpers() {
        for (status, body) in [
            (200u16, &b"hello world"[..]),
            (204, &b""[..]),
            (404, &b"nope"[..]),
            (500, &[0xFFu8, 0x00, 0x80, 0x01][..]),
        ] {
            let whole = encode_h3_response(status, body).unwrap();
            let mut split = Vec::new();
            split.extend_from_slice(&encode_h3_headers_frame(status, Some(body.len())).unwrap());
            split.extend_from_slice(&encode_h3_data_frame(body).unwrap());
            assert_eq!(whole, split, "status={status} body.len()={}", body.len());
        }
    }

    /// S6 I1 — `stream_h2_response`'s re-encode contract, asserted on
    /// the SHARED frame encoders it calls (a `Response<Incoming>`
    /// cannot be constructed without a live H2 conn — the full drive +
    /// backpressure + non-vacuous memory proof is the I4 real-H2 e2e;
    /// this locks the load-bearing framing decisions so an I4
    /// regression is bisectable to a single unit).
    ///
    /// (a) declared content-length ⇒ HEADERS carries `:status` +
    ///     `content-length` (matches `stream_h1_response`'s
    ///     `RespFraming::ContentLength` head); (b) unknown length ⇒
    ///     `:status` only; (c) a >chunk-max body splits into
    ///     `ceil(len / H3_RESP_CHUNK_MAX)` DATA frames each ≤ the cap;
    ///     (d) trailers re-encode to a decodable trailing HEADERS frame
    ///     with pseudo-headers filtered.
    #[test]
    fn s6_i1_stream_h2_response_reencode_framing_contract() {
        // (a) + (b): HEADERS framing parity with stream_h1_response.
        let with_len = encode_h3_headers_frame(200, Some(1234)).unwrap();
        let (f, _) = decode_frame(&with_len, 1 << 20).unwrap();
        let H3Frame::Headers { header_block } = f else {
            panic!("HEADERS");
        };
        let h = QpackDecoder::new().decode(&header_block).unwrap();
        assert!(h.iter().any(|(n, v)| n == ":status" && v == "200"));
        assert!(
            h.iter().any(|(n, v)| n == "content-length" && v == "1234"),
            "declared CL must be forwarded (parity w/ stream_h1_response)"
        );
        let no_len = encode_h3_headers_frame(204, None).unwrap();
        let (f2, _) = decode_frame(&no_len, 1 << 20).unwrap();
        let H3Frame::Headers { header_block: hb2 } = f2 else {
            panic!("HEADERS");
        };
        let h2 = QpackDecoder::new().decode(&hb2).unwrap();
        assert!(
            !h2.iter().any(|(n, _)| n == "content-length"),
            "no CL ⇒ content-length ABSENT (client relies on FIN)"
        );

        // (c) a body larger than H3_RESP_CHUNK_MAX is emitted as
        // multiple DATA frames, each payload ≤ H3_RESP_CHUNK_MAX —
        // exactly the `emit_data!` split loop in stream_h2_response.
        let big = vec![0xABu8; H3_RESP_CHUNK_MAX * 2 + 7];
        let mut frames = 0usize;
        let mut reassembled = Vec::new();
        for slice in big.chunks(H3_RESP_CHUNK_MAX) {
            assert!(slice.len() <= H3_RESP_CHUNK_MAX);
            let enc = encode_h3_data_frame(slice).unwrap();
            let (df, _) = decode_frame(&enc, 1 << 20).unwrap();
            let H3Frame::Data { payload } = df else {
                panic!("DATA");
            };
            reassembled.extend_from_slice(&payload);
            frames += 1;
        }
        assert_eq!(frames, 3, "2*chunk+7 ⇒ 3 DATA frames");
        assert_eq!(reassembled, big, "split is byte-identical");

        // (d) trailers → trailing HEADERS, pseudo-headers filtered (the
        // exact transform stream_h2_response applies to a trailers
        // frame before encode_h3_trailers_frame).
        let raw = [
            (":status".to_owned(), "200".to_owned()), // pseudo — DROP
            ("x-checksum".to_owned(), "deadbeef".to_owned()),
        ];
        let filtered: Vec<(String, String)> = raw
            .iter()
            .filter(|(n, _)| !n.starts_with(':'))
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 1);
        let tf = encode_h3_trailers_frame(&filtered).unwrap();
        let (tdec, _) = decode_frame(&tf, 1 << 20).unwrap();
        let H3Frame::Headers { header_block: tb } = tdec else {
            panic!("trailing HEADERS");
        };
        let td = QpackDecoder::new().decode(&tb).unwrap();
        assert!(td.iter().any(|(n, v)| n == "x-checksum" && v == "deadbeef"));
        assert!(
            !td.iter().any(|(n, _)| n.starts_with(':')),
            "pseudo-header must be filtered from trailers"
        );
    }

    /// S6 I2 — `H3ReqStreamBody` frame contract: chunks → DATA frames
    /// byte-identical, `End` → clean EOS (`None`), mid-body `Reset` →
    /// `Err` (so hyper RST_STREAMs — BINDING case 7), channel-closed
    /// before `End` → `Err` (producer dropped mid-body, never a
    /// truncated-as-complete request). Unit-testable (unlike
    /// `Response<Incoming>`): drive the body via `BodyExt::frame`.
    #[tokio::test]
    async fn s6_i2_h3_req_stream_body_frame_and_abort_contract() {
        use http_body_util::BodyExt as _;

        // (a) chunks then End ⇒ two DATA frames, byte-identical, then
        // clean end-of-stream.
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

        // (c) channel closed before End (producer dropped mid-body) ⇒
        // Err — never a silently-truncated request presented complete.
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

        // (d) framing decision: a leading `Reset` ⇒ pre-dial 413
        // (oversized / cancel-before-data), nothing dialled.
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

        // (e) bodyless (first == End) builds an empty-body request OK
        // (legitimately empty — NOT a dropped body).
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

    /// G5 remediation — `H3ReqAbort`'s `Display`/`Error` impls
    /// (h3_bridge.rs:2145-2147). Pure; exercises the exact wire-fault
    /// message used on the request-smuggling abort path.
    #[test]
    fn g5_h3reqabort_display_and_error_impl() {
        let e = H3ReqAbort;
        let s = e.to_string();
        assert!(
            s.contains("request body aborted"),
            "Display must describe the abort cause, got: {s}"
        );
        // Exercise the `std::error::Error` blanket use (source()=None).
        let dyn_err: &dyn std::error::Error = &e;
        assert!(dyn_err.source().is_none());
        // Boxed form is what the streaming body actually yields.
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(H3ReqAbort);
        assert!(boxed.to_string().contains("client RESET"));
    }

    /// G5 remediation — `h2_request_body_from_rx` head-construction
    /// arms that the I2 unit test did not exercise
    /// (h3_bridge.rs:2267 empty-authority fallback; 2274-2277
    /// pseudo-header skip + regular-header copy loop).
    #[tokio::test]
    async fn g5_h2_request_body_from_rx_head_construction_arms() {
        // (a) EMPTY authority ⇒ `addr.to_string()` fallback (2267),
        // and a `:`-pseudo header is SKIPPED while a regular header is
        // copied (2274-2277). Bodyless (first == End) so no dial.
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
        // The authority fell back to the socket addr in the URI.
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

        // (b) non-empty authority ⇒ that branch of the if/else (2269).
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

    /// G5 remediation — `h3_to_h2_stream_resp` pre-dial inline arms
    /// (h3_bridge.rs:2351-2356 + the 2340 `inline` happy branch): a
    /// pre-data `Reset` ⇒ inline 413 (no pool dial), and a
    /// builder-failure ⇒ inline 502. The pool is constructed but NEVER
    /// dialled (both arms return before `send_request`), so this is a
    /// fast, hermetic unit test.
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

        // --- 413 arm: first event is Reset ⇒ inline 413, Ok(()) ---
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
        // SESSION 24 / INC-3: the inline 413 is now DECODED — a
        // `Head { status: 413 }` then `End` on the channel (the actor
        // encodes). The assertion (status == 413, clean End, no Reset)
        // is unchanged; only the on-wire→decoded harness shape is.
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

        // --- 502 arm: builder failure ⇒ inline 502, Ok(()) ---
        // An invalid method byte makes `Request::builder().method(..)`
        // / `.body()` fail ⇒ `h2_request_body_from_rx` returns
        // Err(502) ⇒ the `Err(_) => inline 502` arm (2354-2356).
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
        // SESSION 24 / INC-3: decoded inline 502 (`Head { status: 502 }`
        // then `End`).
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
        // Pool was never dialled (both arms returned pre-send_request).
        assert_eq!(pool.peer_count(), 0, "no upstream dial on inline arms");
    }

    /// `content_length: None` emits `:status` only (no `content-length`).
    #[test]
    fn encode_h3_headers_frame_none_omits_content_length() {
        let f = encode_h3_headers_frame(200, None).unwrap();
        let (frame, _) = decode_frame(&f, 1 << 20).unwrap();
        let H3Frame::Headers { header_block } = frame else {
            panic!("expected HEADERS");
        };
        let headers = QpackDecoder::new().decode(&header_block).unwrap();
        assert!(headers.iter().any(|(n, v)| n == ":status" && v == "200"));
        assert!(
            !headers.iter().any(|(n, _)| n == "content-length"),
            "content-length must be ABSENT when length unknown"
        );
    }

    /// Happy-path chunked decode across a split feed: payload is exact
    /// and the zero-size terminator sets `done`.
    #[test]
    fn chunk_decoder_decodes_split_chunks() {
        let mut dec = ChunkDecoder::new();
        let mut out = Vec::new();
        // "Wiki" + "pedia" then terminator, fed in awkward splits.
        dec.feed(b"4\r\nWik", &mut out).unwrap();
        dec.feed(b"i\r\n5\r\npedia\r\n", &mut out).unwrap();
        assert!(!dec.done);
        dec.feed(b"0\r\n", &mut out).unwrap();
        assert!(dec.done);
        assert_eq!(out, b"Wikipedia");
    }

    /// SESSION 4 / P1-A approval condition C3: every malformed chunked
    /// framing ⇒ `RespAbort::ChunkedDecode` — NEVER a truncated or
    /// forwarded body presented as complete.
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

    /// A chunk extension (`;ext`) is tolerated; size is the hex before
    /// `;`. (Smuggling-relevant: a decoder that mis-parses the size
    /// past `;` would frame the body wrong.)
    #[test]
    fn chunk_decoder_tolerates_chunk_extension() {
        let mut dec = ChunkDecoder::new();
        let mut out = Vec::new();
        dec.feed(b"4;name=value\r\nbody\r\n0\r\n", &mut out)
            .unwrap();
        assert!(dec.done);
        assert_eq!(out, b"body");
    }

    /// SESSION 4 / P1-C (C4): the RFC 9112 §7.1.2 trailer-section
    /// parse. `done` (zero-size chunk seen) is distinct from `complete`
    /// (trailer section + terminating CRLF consumed); the producer
    /// loops on `complete`. PC-2: a trailer section coalesced into the
    /// SAME feed as the `0\r\n` size line parses identically to one
    /// split across feeds.
    #[test]
    fn chunk_decoder_parses_trailer_section_c4() {
        // (a) coalesced: `0\r\n<fields>\r\n` in one feed.
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

        // (b) split across feeds: size line, fields and the terminating
        //     CRLF in separate feeds ⇒ identical decoded outcome.
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

        // (c) no trailer section: bare `0\r\n\r\n` ⇒ complete, empty.
        let mut d = ChunkDecoder::new();
        let mut o = Vec::new();
        d.feed(b"3\r\nabc\r\n0\r\n\r\n", &mut o).unwrap();
        assert!(d.complete);
        assert_eq!(o, b"abc");
        assert!(d.take_trailers().is_empty());

        // (d) C3/C4 parity — junk (a no-colon line) after the
        //     zero-size terminator is NOT a valid trailer field ⇒
        //     ChunkedDecode, never accepted/forwarded.
        let mut d = ChunkDecoder::new();
        assert_eq!(
            d.feed(b"3\r\nabc\r\n0\r\nthis-is-junk\r\n\r\n", &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );

        // (e) a `:`-prefixed pseudo-header in the trailer section is
        //     rejected (RFC 9114 §4.3).
        let mut d = ChunkDecoder::new();
        assert_eq!(
            d.feed(b"0\r\n:status: 200\r\n\r\n", &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );

        // (f) an oversized trailer section is rejected (smuggling
        //     guard, MAX_TRAILER_SECTION).
        let mut d = ChunkDecoder::new();
        let mut huge = Vec::from(&b"0\r\n"[..]);
        huge.extend_from_slice(b"x-big: ");
        huge.extend(std::iter::repeat_n(b'A', MAX_TRAILER_SECTION + 16));
        assert_eq!(
            d.feed(&huge, &mut Vec::new()),
            Err(RespAbort::ChunkedDecode)
        );
    }

    /// SESSION 7 / J2 (H3→H3 R8) pure unit proof: the M-C request
    /// send-half decision table — the analogue of the H3→H2 I2 test
    /// `s6_i2_h3_req_stream_body_frame_and_abort_contract`. Exercises
    /// the REAL module-level `j2_req_event_action` (the same fn the
    /// event-loop park arm calls), no socket.
    #[test]
    fn s7_j2_request_send_decision() {
        // (a) Chunk ⇒ forward as ONE bounded request-body chunk. SESSION
        //     25 / INC-4: the action now carries the RAW chunk bytes
        //     verbatim (no encode step — `quiche::h3::send_body` frames
        //     the DATA); the bytes must equal the original payload exactly
        //     (no corruption, no accumulation).
        // (forward_req_trailers=false throughout (a)-(d): the H3→H3
        //  drop semantics — assertions byte-identical to pre-S12.)
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
        // An empty chunk still classifies as SendData (a zero-length
        // DATA frame is well-formed; never reclassified as End).
        assert!(matches!(
            j2_req_event_action(Some(ReqBodyEvent::Chunk(Bytes::new())), false),
            J2ReqAction::SendData(_)
        ));

        // (b) End ⇒ FIN-terminate, request trailers DROPPED (the
        //     action carries NO trailer payload — parity H3→H1 P1-C /
        //     H3→H2 A3; the body is framed by the QUIC FIN, J2-G2).
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

        // (c) mid-body Reset ⇒ abort WITHOUT FIN (BINDING case-7:
        //     never a truncated-as-complete request upstream).
        assert_eq!(
            j2_req_event_action(Some(ReqBodyEvent::Reset), false),
            J2ReqAction::AbortNoFin,
            "mid-body Reset MUST abort the upstream request with NO FIN"
        );

        // (d) channel closed before End (producer dropped mid-body) ⇒
        //     abort WITHOUT FIN — identical to a mid-body Reset, never
        //     a silently-truncated request presented as complete.
        assert_eq!(
            j2_req_event_action(None, false),
            J2ReqAction::AbortNoFin,
            "premature channel close MUST abort with NO FIN (truncation guard)"
        );

        // (e) SESSION 12 / RISK-3 — forward_req_trailers=true (L7
        //     fronts): a NON-EMPTY End{trailers} forwards as
        //     FinWithTrailers (post-DATA HEADERS then FIN); an EMPTY
        //     End{} stays FinNoTrailers (bare FIN — no spurious empty
        //     trailers frame). The mid-body abort + premature-close
        //     guards are UNAFFECTED by the flag (truncation guard holds
        //     regardless).
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

    /// SESSION 12 — the connector's `Decoded` sink (the H1/H2 fronts'
    /// per-front response handler) MUST surface an upstream response
    /// trailing field section as `H3RespEvent::Trailers`, with the
    /// fields intact. This is the half of the trailer mandate proven
    /// only by code-read until now: the H1→H3 / H2→H3 cells rely on the
    /// connector EMITTING `Trailers` so the L7 front can forward
    /// grpc-status etc.; a future connector trailer-DROP (replacing the
    /// `Trailers` emit with a no-op) would otherwise slip every test.
    /// (The H3→H3 `Wire` arm's trailer forwarding is already covered by
    /// `h3h3_e2e_response_trailers_forwarded`; THIS covers the `Decoded`
    /// arm's emission.)
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

    /// SESSION 12 — companion to the trailer assertion: the `Decoded`
    /// sink's `on_head` MUST forward the FULL non-pseudo response header
    /// set (filtering pseudo-headers, retaining `content-length` as a
    /// regular header) so the L7 front sees every header (CF-H3H3-HEAD
    /// parity: the `Wire` arm now matches this via
    /// `encode_h3_headers_frame_full`).
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
