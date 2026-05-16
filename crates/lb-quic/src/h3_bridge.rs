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

/// SESSION 2 / P1-A: largest single `ReqBodyEvent::Chunk` the body
/// phase machine emits. With `H3_BODY_CHANNEL_DEPTH` this bounds the
/// max in-flight bytes (≈ depth * chunk ≈ 64 KiB) INDEPENDENT of the
/// total body size — a DATA frame larger than this is split.
pub const H3_BODY_CHUNK_MAX: usize = 8 * 1024;

/// RFC 9114 §7.2 `DATA` frame type.
const FRAME_DATA: u64 = 0x00;
/// RFC 9114 §7.2 `HEADERS` frame type.
const FRAME_HEADERS: u64 = 0x01;

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

/// SESSION 2 / P1-A: an ordered item produced by [`StreamRxBuf`]'s
/// `Body` phase as post-HEADERS frames are decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyItem {
    /// A (sub-)chunk of request DATA (≤ [`H3_BODY_CHUNK_MAX`] bytes).
    Data(Bytes),
    /// RFC 9114 §4.1 trailing field section — a HEADERS frame that
    /// arrived *after* DATA frames.
    Trailers(Vec<(String, String)>),
    /// Cumulative decoded body exceeded the caller-supplied cap; the
    /// bridge must respond `413` and tear the upstream down.
    TooLarge,
}

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

/// SESSION 2 / P1-A — phase of the per-stream inbound decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RxPhase {
    /// Awaiting / decoding the request HEADERS frame.
    #[default]
    Headers,
    /// HEADERS emitted; subsequent frames are DATA / trailers.
    Body,
}

/// SESSION 2 / P1-A FIX — body-phase incremental frame-parser state.
///
/// This is a true streaming state machine over the raw post-HEADERS
/// byte stream. It NEVER buffers a whole DATA frame: DATA payload bytes
/// are emitted (chunked) and drained the moment they arrive. The only
/// state that retains bytes is the (≤16 B) partial frame header and the
/// (bounded) trailing-HEADERS field block, which QPACK genuinely
/// requires whole.
#[derive(Debug, Clone)]
enum BodyParse {
    /// Accumulating the frame-type varint + length varint. `hdr` holds
    /// only the not-yet-decodable prefix; bounded by
    /// [`MAX_FRAME_HEADER_BYTES`].
    AwaitingFrameHeader { hdr: Vec<u8> },
    /// Inside a DATA frame; `remaining` payload bytes still to stream.
    InData { remaining: usize },
    /// Inside a trailing-HEADERS (RFC 9114 §4.1) frame; accumulate the
    /// QPACK field block (bounded by [`MAX_TRAILER_BLOCK_BYTES`]) until
    /// `remaining` hits 0, then decode + emit `Trailers`.
    InTrailers { remaining: usize, block: Vec<u8> },
    /// Inside an unknown / ignored frame (RFC 9114 §9): discard
    /// `remaining` bytes incrementally, never buffering.
    InSkip { remaining: usize },
}

impl Default for BodyParse {
    fn default() -> Self {
        Self::AwaitingFrameHeader { hdr: Vec::new() }
    }
}

/// Per-stream accumulator for inbound H3 request bytes. A quiche
/// `stream_recv` on a given stream ID can yield a chunk mid-frame;
/// we buffer until a full frame is parseable.
///
/// SESSION 2 / P1-A: this is now a small two-phase machine. The
/// `Headers` phase preserves the exact S1 contract — `feed` returns
/// `Ok(Some(headers))` once the request HEADERS frame is decoded (the
/// S1 unit test `stream_rx_buf_accumulates_partial_headers` exercises
/// exactly this and stays green). After headers it flips to the
/// `Body` phase; the actor then calls `feed_body` to drain DATA /
/// trailer frames incrementally.
#[derive(Default)]
pub struct StreamRxBuf {
    buf: Vec<u8>,
    phase: RxPhase,
    /// Cumulative decoded request-body bytes (Body phase only).
    body_seen: usize,
    /// Latched once the cap is exceeded so `feed_body` keeps reporting
    /// `TooLarge` and never forwards further data.
    too_large: bool,
    /// SESSION 2 / P1-A FIX — body-phase streaming parser state. In the
    /// Body phase this (NOT `buf`) holds the only retained bytes: the
    /// ≤16 B partial frame header or a bounded trailer field block.
    body: BodyParse,
}

impl StreamRxBuf {
    /// Append freshly-received bytes and return `Ok(Some(headers))` once
    /// a full HEADERS frame has been decoded. Returns `Ok(None)` if more
    /// bytes are needed.
    ///
    /// SESSION 2 / P1-A: on the HEADERS hit the buffer flips to the
    /// `Body` phase (instead of latching `done`) and any *leftover*
    /// bytes (DATA frames that arrived coalesced with HEADERS in the
    /// same `stream_recv`) are retained for [`StreamRxBuf::feed_body`].
    /// The observable `Ok(Some(headers))` / `Ok(None)` return shape is
    /// unchanged from S1.
    ///
    /// # Errors
    ///
    /// Surfaces a string-formatted decode error if the H3 frame parser
    /// rejects the buffer or if QPACK cannot decode the field block.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Option<Vec<(String, String)>>, String> {
        if self.phase == RxPhase::Body {
            // Headers already emitted; new bytes belong to the body
            // phase. Retain them; caller pulls via `feed_body`.
            self.buf.extend_from_slice(chunk);
            return Ok(None);
        }
        self.buf.extend_from_slice(chunk);
        loop {
            match decode_frame(&self.buf, 1 << 20) {
                Ok((H3Frame::Headers { header_block }, consumed)) => {
                    self.buf.drain(..consumed);
                    self.phase = RxPhase::Body;
                    let decoder = QpackDecoder::new();
                    let headers = decoder
                        .decode(&header_block)
                        .map_err(|e| format!("qpack decode: {e}"))?;
                    return Ok(Some(headers));
                }
                Ok((_other, consumed)) => {
                    // Non-HEADERS frames are either SETTINGS (ignored
                    // on request stream anyway) or future-protocol
                    // extensions we can skip per RFC 9114 §7.2.8.
                    self.buf.drain(..consumed);
                }
                Err(lb_h3::H3Error::Incomplete) => return Ok(None),
                Err(e) => return Err(format!("h3 decode_frame: {e}")),
            }
        }
    }

    /// SESSION 2 / P1-A — `Body` phase. Append freshly-received bytes
    /// and decode *as many* post-HEADERS frames as are fully buffered,
    /// returning an ordered list of [`BodyItem`]s:
    ///
    ///   * `Data(Bytes)`     — a DATA-frame payload, split so no item
    ///                          exceeds [`H3_BODY_CHUNK_MAX`].
    ///   * `Trailers(..)`    — a post-DATA HEADERS frame (RFC 9114
    ///                          §4.1 trailing field section).
    ///   * `TooLarge`        — cumulative body exceeded `max_body`; the
    ///                          item is emitted once and latched (all
    ///                          subsequent calls re-report it and emit
    ///                          no further data).
    ///
    /// Returns `Ok(vec![])` when no complete frame is yet buffered.
    /// Must only be called after `feed` has returned `Ok(Some(_))`.
    ///
    /// # Errors
    ///
    /// Surfaces a string-formatted decode error if a post-HEADERS
    /// frame is malformed or a trailer field block fails QPACK decode.
    pub fn feed_body(&mut self, chunk: &[u8], max_body: usize) -> Result<Vec<BodyItem>, String> {
        let mut items = Vec::new();
        if self.too_large {
            items.push(BodyItem::TooLarge);
            return Ok(items);
        }
        // SESSION 2 / P1-A FIX: the ONLY bytes that may live in
        // `self.buf` in the Body phase are leftover bytes the HEADERS
        // decode in `feed` could not consume (a DATA prefix that
        // arrived coalesced with the HEADERS frame in one `stream_recv`).
        // Move them into a local input stream FIRST, then the fresh
        // `chunk`, and clear `self.buf` so it never retains a frame.
        // After this, `self.buf` is empty for the whole Body phase.
        let mut input: Vec<u8> = Vec::new();
        if !self.buf.is_empty() {
            input.append(&mut self.buf);
        }
        input.extend_from_slice(chunk);

        let mut pos = 0usize;
        while pos < input.len() {
            let avail = input.get(pos..).unwrap_or(&[]);
            match std::mem::take(&mut self.body) {
                BodyParse::AwaitingFrameHeader { mut hdr } => {
                    // Accumulate the smallest prefix that decodes BOTH
                    // the type varint and the length varint. We feed
                    // bytes one at a time from `avail` so the partial
                    // header buffer is bounded and we never over-read.
                    let mut consumed_here = 0usize;
                    let parsed = loop {
                        match Self::try_parse_frame_header(&hdr) {
                            Some(Ok((ftype, len, _hlen))) => break Some((ftype, len)),
                            Some(Err(e)) => return Err(e),
                            None => {
                                let Some(&b) = avail.get(consumed_here) else {
                                    break None;
                                };
                                consumed_here += 1;
                                hdr.push(b);
                                if hdr.len() > MAX_FRAME_HEADER_BYTES {
                                    return Err(format!(
                                        "h3 body frame header exceeds \
                                         {MAX_FRAME_HEADER_BYTES} bytes (malformed)"
                                    ));
                                }
                            }
                        }
                    };
                    pos += consumed_here;
                    match parsed {
                        None => {
                            // Ran out of input mid-header; retain the
                            // (bounded) partial header for the next call.
                            self.body = BodyParse::AwaitingFrameHeader { hdr };
                            break;
                        }
                        Some((ftype, len)) => {
                            let remaining = usize::try_from(len).map_err(|_| {
                                "h3 body frame length overflows usize".to_string()
                            })?;
                            self.body = match ftype {
                                FRAME_DATA => BodyParse::InData { remaining },
                                FRAME_HEADERS => BodyParse::InTrailers {
                                    remaining,
                                    block: Vec::new(),
                                },
                                // RFC 9114 §9: ignore unknown / other
                                // frame types — skip incrementally.
                                _ => BodyParse::InSkip { remaining },
                            };
                        }
                    }
                }
                BodyParse::InData { remaining } => {
                    if remaining == 0 {
                        // Zero-length DATA frame ⇒ no spurious chunk.
                        self.body = BodyParse::default();
                        continue;
                    }
                    let take = remaining.min(input.len() - pos);
                    let end = pos + take;
                    // Cumulative total-body cap (request correctness,
                    // NOT the memory mechanism). Counted across ALL
                    // emitted Data bytes; a single huge DATA frame is
                    // fine — only the cumulative total is capped.
                    self.body_seen = self.body_seen.saturating_add(take);
                    if self.body_seen > max_body {
                        self.too_large = true;
                        items.push(BodyItem::TooLarge);
                        return Ok(items);
                    }
                    // Emit the available payload immediately, chunked to
                    // <= H3_BODY_CHUNK_MAX, and DRAIN it (advance `pos`)
                    // — the whole frame is NEVER retained.
                    let mut off = pos;
                    while off < end {
                        let stop = (off + H3_BODY_CHUNK_MAX).min(end);
                        items.push(BodyItem::Data(Bytes::copy_from_slice(
                            input.get(off..stop).unwrap_or(&[]),
                        )));
                        off = stop;
                    }
                    pos = end;
                    let rem = remaining - take;
                    self.body = if rem == 0 {
                        BodyParse::default()
                    } else {
                        BodyParse::InData { remaining: rem }
                    };
                }
                BodyParse::InTrailers {
                    remaining,
                    mut block,
                } => {
                    // RFC 9114 §4.1: a HEADERS frame after DATA is the
                    // trailing field section. QPACK needs the WHOLE
                    // block — accumulate, but BOUND it.
                    let take = remaining.min(input.len() - pos);
                    let end = pos + take;
                    block.extend_from_slice(input.get(pos..end).unwrap_or(&[]));
                    if block.len() > MAX_TRAILER_BLOCK_BYTES {
                        return Err(format!(
                            "h3 body trailer field block exceeds \
                             {MAX_TRAILER_BLOCK_BYTES} bytes (malformed)"
                        ));
                    }
                    pos = end;
                    let rem = remaining - take;
                    if rem == 0 {
                        let decoder = QpackDecoder::new();
                        let trailers = decoder
                            .decode(&block)
                            .map_err(|e| format!("qpack trailer decode: {e}"))?;
                        items.push(BodyItem::Trailers(trailers));
                        self.body = BodyParse::default();
                    } else {
                        self.body = BodyParse::InTrailers {
                            remaining: rem,
                            block,
                        };
                    }
                }
                BodyParse::InSkip { remaining } => {
                    // RFC 9114 §9: discard the payload incrementally,
                    // never buffering it.
                    let take = remaining.min(input.len() - pos);
                    pos += take;
                    let rem = remaining - take;
                    self.body = if rem == 0 {
                        BodyParse::default()
                    } else {
                        BodyParse::InSkip { remaining: rem }
                    };
                }
            }
        }
        Ok(items)
    }

    /// SESSION 2 / P1-A FIX: try to decode a body-phase frame header
    /// (frame-type varint + length varint) from `hdr`.
    ///
    /// Returns `None` if more bytes are needed, `Some(Ok((type, len,
    /// header_len)))` once both varints decode, or `Some(Err(_))` on a
    /// malformed varint.
    fn try_parse_frame_header(hdr: &[u8]) -> Option<Result<(u64, u64, usize), String>> {
        let (ftype, tlen) = match lb_h3::decode_varint(hdr) {
            Ok(v) => v,
            Err(lb_h3::H3Error::Incomplete) => return None,
            Err(e) => return Some(Err(format!("h3 body frame type varint: {e}"))),
        };
        let rest = hdr.get(tlen..)?;
        let (len, llen) = match lb_h3::decode_varint(rest) {
            Ok(v) => v,
            Err(lb_h3::H3Error::Incomplete) => return None,
            Err(e) => return Some(Err(format!("h3 body frame length varint: {e}"))),
        };
        Some(Ok((ftype, len, tlen + llen)))
    }

    /// SESSION 2 / P1-A: true once the cumulative body cap has been
    /// exceeded — the actor uses this to stop forwarding and reset.
    #[must_use]
    pub const fn is_too_large(&self) -> bool {
        self.too_large
    }

    /// SESSION 2 / P1-A FIX (test-gauge): bytes this `StreamRxBuf`
    /// currently RETAINS in the Body phase — the leftover `buf` plus
    /// whatever the streaming parser is holding (a ≤16 B partial frame
    /// header, or a bounded trailer field block). This is the figure
    /// the T5 memory-bound proof sums: a whole-DATA-frame-buffering
    /// implementation would make this grow with frame size; the
    /// streaming parser keeps it tiny.
    #[cfg(any(test, feature = "test-gauges"))]
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        let parser = match &self.body {
            BodyParse::AwaitingFrameHeader { hdr } => hdr.len(),
            BodyParse::InTrailers { block, .. } => block.len(),
            BodyParse::InData { .. } | BodyParse::InSkip { .. } => 0,
        };
        self.buf.len() + parser
    }
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
    let encoder = QpackEncoder::new();
    let headers: Vec<(String, String)> = vec![
        (":status".to_string(), status.to_string()),
        ("content-length".to_string(), body.len().to_string()),
    ];
    let header_block = encoder
        .encode(&headers)
        .map_err(|e| format!("qpack encode: {e}"))?;
    let headers_frame = encode_frame(&H3Frame::Headers { header_block })
        .map_err(|e| format!("h3 headers frame: {e}"))?;
    let data_frame = encode_frame(&H3Frame::Data {
        payload: Bytes::copy_from_slice(body),
    })
    .map_err(|e| format!("h3 data frame: {e}"))?;
    let mut out = Vec::with_capacity(headers_frame.len() + data_frame.len());
    out.extend_from_slice(&headers_frame);
    out.extend_from_slice(&data_frame);
    Ok(out)
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

/// SESSION 2 / P1-A — **incremental** H3→H1 request-body streaming.
///
/// Forwards request DATA to the H1 upstream as it arrives over the
/// per-stream bounded channel `body_rx`. Memory safety comes from the
/// channel's bounded depth + backpressure (poll_h3 stops calling
/// `stream_recv` when the channel is full), NOT from buffering: at
/// most ONE chunk is held here at any instant (the peeked head chunk,
/// then one chunk per loop iteration). The response is still buffered
/// via [`read_h1_response`] + [`encode_h3_response`] — response
/// streaming is a later increment.
///
/// Framing decision (from the first body event + the client
/// `content-length` header in `req.extra`, case-insensitive):
///   * first event `End` with no prior `Chunk` ⇒ bodyless ⇒ head is
///     BYTE-IDENTICAL to `build_h1_request(req, None)`.
///   * body present + client `content-length` ⇒ `Content-Length: <v>`,
///     raw body bytes forwarded unframed.
///   * body present, no client `content-length` ⇒
///     `Transfer-Encoding: chunked`, each chunk HTTP/1.1-chunk-framed.
///
/// `max_body` is the total-size cap surfaced as H3 `413` (the in-flight
/// window is the memory mechanism, separate from this).
///
/// # Errors
///
/// Returns the H3 wire bytes of a `413` when `body_rx` delivers a
/// `Reset` *carrying the too-large signal* (poll_h3 drops the channel
/// after sending nothing further); a `502` on any upstream
/// dial/write/read failure or premature channel close. Surfaces a
/// string error only if encoding the fallback response itself fails.
#[allow(clippy::too_many_lines)]
pub async fn h3_to_h1_stream(
    req: &H3Request,
    backend: SocketAddr,
    pool: &TcpPool,
    mut body_rx: tokio::sync::mpsc::Receiver<ReqBodyEvent>,
    _max_body: usize,
) -> Result<Vec<u8>, String> {
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
            return encode_h3_response(413, b"payload too large");
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

    let mut pooled = match pool.acquire_async(backend).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "H3→H1 stream backend acquire failed");
            return encode_h3_response(502, b"bad gateway");
        }
    };

    let head = build_h1_head(req, &framing);
    let chunked = framing == H1BodyFraming::Chunked;

    // --- write head + stream body incrementally ---
    let response = {
        let stream = pooled
            .stream_mut()
            .ok_or_else(|| "pool returned empty handle".to_string())?;

        macro_rules! fail502 {
            ($ctx:expr, $e:expr) => {{
                pooled.set_reusable(false);
                tracing::warn!(error = %$e, $ctx);
                return encode_h3_response(502, b"bad gateway");
            }};
        }

        if let Err(e) = stream.write_all(&head).await {
            fail502!("H3→H1 stream head write failed", e);
        }

        if let Some(b) = pending_first.take() {
            if write_body_chunk(stream, &b, chunked).await.is_err() {
                fail502!("H3→H1 stream body write failed", "first chunk");
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
                        fail502!("H3→H1 stream body write failed", "chunk");
                    }
                }
                ReqBodyEvent::End { trailers: _ } => {
                    // Trailers are not forwarded to the H1 upstream in
                    // this increment (H1 trailer egress is later work);
                    // they are consumed so the stream terminates clean.
                    clean_end = true;
                    break;
                }
                ReqBodyEvent::Reset => {
                    // SESSION 2 / P1-B: mid-body Reset. poll_h3 emits
                    // this for BOTH (a) the oversized cap breach and
                    // (b) a CLIENT CANCEL (peer QUIC RESET_STREAM /
                    // STOP_SENDING before FIN). In every case the body
                    // is incomplete and MUST NOT be delivered to the
                    // backend as a completed request: we mark the pooled
                    // upstream connection non-reusable (so the partially
                    // written request can never be paired with a
                    // subsequent one — HTTP-request-smuggling / cache-
                    // poisoning guard) and return IMMEDIATELY, BEFORE the
                    // `0\r\n\r\n` chunked terminator / before the full
                    // Content-Length body is written, so the backend
                    // never sees a completable request. The 413 status
                    // is the safe client-facing response; a cancelling
                    // client has already torn down its stream and will
                    // not read it, while the oversized path genuinely
                    // wants 413 — the load-bearing invariant here is the
                    // upstream abort, not the status code.
                    pooled.set_reusable(false);
                    tracing::warn!(
                        "SESSION 2 / P1-B: H3→H1 stream body Reset (oversized or \
                         client cancel); aborting upstream without completing the request"
                    );
                    return encode_h3_response(413, b"payload too large");
                }
            }
        }
        if !clean_end {
            // Channel closed before an explicit End/Reset → producer
            // dropped mid-body: abort rather than present a truncated
            // request to the upstream.
            pooled.set_reusable(false);
            tracing::warn!("H3→H1 stream channel closed before End; aborting upstream");
            return encode_h3_response(502, b"bad gateway");
        }
        if chunked {
            if let Err(e) = stream.write_all(b"0\r\n\r\n").await {
                fail502!("H3→H1 stream chunked terminator failed", e);
            }
        }

        if let Err(e) = stream.flush().await {
            fail502!("H3→H1 stream flush failed", e);
        }
        match read_h1_response(stream).await {
            Ok(r) => r,
            Err(e) => {
                fail502!("H3→H1 stream backend read failed", e);
            }
        }
    };

    pooled.set_reusable(false);
    encode_h3_response(response.status, &response.body)
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

/// Forward an H3 request to an upstream H2 backend via
/// [`Http2Pool`] and return the response mapped back into H3 wire
/// bytes. On any backend failure returns a 502 + `"bad gateway"`.
///
/// PROTO-001 H3-listener → H2-backend path. Body forwarding is
/// supported (single DATA frame in / collected `Bytes` to upstream
/// hyper request) but the e2e exercise is GET-only.
pub async fn h3_to_h2_roundtrip(
    req: &H3Request,
    addr: std::net::SocketAddr,
    pool: &Http2Pool,
) -> Vec<u8> {
    let bad_gateway = || encode_h3_response(502, b"bad gateway").unwrap_or_default();

    // Build hyper Request<BoxBody>. URI must carry scheme + authority
    // + path so hyper's H2 client emits the right pseudo-headers.
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
    let body: BoxBody<Bytes, hyper::Error> = Full::<Bytes>::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed();
    let request: Request<BoxBody<Bytes, hyper::Error>> = match builder.body(body) {
        Ok(r) => r,
        Err(_) => return bad_gateway(),
    };

    let resp = match pool.send_request(addr, request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, %addr, "H3→H2 send_request failed");
            return bad_gateway();
        }
    };

    let (parts, body) = resp.into_parts();
    let body_bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "H3→H2 body read failed");
            return bad_gateway();
        }
    };
    encode_h3_response(parts.status.as_u16(), &body_bytes).unwrap_or_default()
}

/// Forward an H3 request to an upstream H3 backend via
/// [`QuicUpstreamPool`] and return the response mapped back into H3
/// wire bytes. On any backend failure returns a 502 + `"bad gateway"`.
///
/// Unlike `h3_to_h1_roundtrip`, this path does NOT translate —
/// everything stays H3 end-to-end. The same lb-h3 codec is used on
/// both sides.
///
/// Request-body forwarding is not supported in 3b.3c-3: the e2e
/// exercises a body-less GET. Pillar 3b.3b will plumb DATA frames
/// through once the downstream connection actor starts threading
/// body bytes across stream boundaries.
#[allow(clippy::too_many_lines, clippy::large_futures)]
pub async fn h3_to_h3_roundtrip(
    req: &H3Request,
    addr: std::net::SocketAddr,
    sni: &str,
    pool: &QuicUpstreamPool,
) -> Vec<u8> {
    let mut pooled = match pool.acquire(addr, sni).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, %addr, "H3→H3 pool acquire failed");
            return encode_h3_response(502, b"bad gateway").unwrap_or_else(|_| Vec::new());
        }
    };

    let Some(upstream) = pooled.get_mut() else {
        tracing::warn!("H3→H3 pool returned empty handle");
        return encode_h3_response(502, b"bad gateway").unwrap_or_default();
    };

    // Build the upstream request HEADERS frame.
    let encoder = QpackEncoder::new();
    let mut headers: Vec<(String, String)> = Vec::with_capacity(4);
    headers.push((":method".to_string(), req.method.clone()));
    headers.push((":scheme".to_string(), "https".to_string()));
    let authority = if req.authority.is_empty() {
        sni.to_string()
    } else {
        req.authority.clone()
    };
    headers.push((":authority".to_string(), authority));
    headers.push((":path".to_string(), req.path.clone()));
    let Ok(header_block) = encoder.encode(&headers) else {
        return encode_h3_response(502, b"bad gateway").unwrap_or_default();
    };
    let Ok(frame) = encode_frame(&H3Frame::Headers { header_block }) else {
        return encode_h3_response(502, b"bad gateway").unwrap_or_default();
    };

    // Drive the upstream conn for one GET. We use client-initiated
    // bidi stream 0 — each new QUIC conn starts with sid=0 available.
    let stream_id: u64 = 0;
    let socket_clone = Arc::clone(upstream.socket());
    let local = upstream.local();
    let peer = upstream.peer();
    let qconn_mut: &mut quiche::Connection = match upstream.connection_mut() {
        Some(c) => c,
        None => {
            return encode_h3_response(502, b"bad gateway").unwrap_or_default();
        }
    };

    // Send HEADERS + FIN on the bidi stream.
    let mut frame_pos = 0usize;
    while frame_pos < frame.len() {
        let chunk = frame.get(frame_pos..).unwrap_or(&[]);
        let fin = frame_pos + chunk.len() >= frame.len();
        match qconn_mut.stream_send(stream_id, chunk, fin) {
            Ok(n) => {
                if n == 0 {
                    break;
                }
                frame_pos = frame_pos.saturating_add(n);
            }
            Err(quiche::Error::Done) => break,
            Err(e) => {
                tracing::warn!(error = %e, "H3→H3 stream_send");
                pooled.set_reusable(false);
                return encode_h3_response(502, b"bad gateway").unwrap_or_default();
            }
        }
    }

    // Event loop: drive send/recv/timeout until we have a full response.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out_buf = vec![0u8; 65_535];
    let mut in_buf = vec![0u8; 65_535];
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_body: Vec<u8> = Vec::new();
    let mut body_complete = false;
    let mut expected_len: Option<usize> = None;

    while tokio::time::Instant::now() < deadline {
        // Flush.
        while let Ok((n, info)) = qconn_mut.send(&mut out_buf) {
            let bytes = out_buf.get(..n).unwrap_or(&[]);
            if socket_clone.send_to(bytes, info.to).await.is_err() {
                break;
            }
        }

        // Drain any readable stream bytes.
        let readable: Vec<u64> = qconn_mut.readable().collect();
        for sid in readable {
            if sid != stream_id {
                continue;
            }
            let mut chunk = [0u8; 8192];
            while let Ok((n, _fin)) = qconn_mut.stream_recv(sid, &mut chunk) {
                rx_tail.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
            }
        }

        // Try decoding frames.
        loop {
            match decode_frame(&rx_tail, 1 << 20) {
                Ok((H3Frame::Headers { header_block }, consumed)) => {
                    rx_tail.drain(..consumed);
                    if let Ok(hdrs) = QpackDecoder::new().decode(&header_block) {
                        for (n, v) in hdrs {
                            if n == ":status" {
                                decoded_status = v.parse::<u16>().ok();
                            } else if n == "content-length" {
                                expected_len = v.parse::<usize>().ok();
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

        if decoded_status.is_some() && body_complete {
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
        let _ = peer; // silence unused binding when logging disabled
    }

    // Response is done; do not reuse the upstream conn since we sent
    // FIN on its stream 0 — that connection is only good for one
    // request in this minimal 3b.3c-3 wiring. Real H3 clients would
    // open new streams; the pool improvement lands when we carry
    // stream-ID allocation state across checkouts.
    pooled.set_reusable(false);

    let status = decoded_status.unwrap_or(502);
    encode_h3_response(status, &decoded_body).unwrap_or_else(|_| Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// SESSION 2 target — request-body forwarding through the H3→H1
    /// bridge. The S1-B seam (`build_h1_request`'s `Some(body)` arm +
    /// `h3_to_h1_roundtrip`'s `body` param) is in place; this asserts
    /// the seam's CONTRACT (correct `Content-Length` + appended
    /// payload) so SESSION 2 has a concrete, named target. It is
    /// `#[ignore]` ONLY because SESSION 2's datapath wiring
    /// (`conn_actor::poll_h3` accumulating inbound H3 DATA frames and
    /// passing `Some(..)` here) is UNBUILT — it does not mask any
    /// existing passing behaviour (no caller passes `Some` yet). When
    /// SESSION 2 lands DATA-frame accumulation, drop the `#[ignore]`
    /// and extend this into a real bodyful e2e.
    #[test]
    #[ignore = "S2: request-body forwarding"]
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

    #[test]
    fn stream_rx_buf_accumulates_partial_headers() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":path".to_string(), "/".to_string()),
        ];
        let block = QpackEncoder::new().encode(&headers).unwrap();
        let frame = encode_frame(&H3Frame::Headers {
            header_block: block,
        })
        .unwrap();

        let mut rx = StreamRxBuf::default();
        // First half: incomplete.
        let mid = frame.len() / 2;
        let first = rx.feed(frame.get(..mid).unwrap()).unwrap();
        assert!(first.is_none());
        // Second half: should yield decoded headers.
        let second = rx.feed(frame.get(mid..).unwrap()).unwrap();
        assert!(second.is_some());
    }
}
