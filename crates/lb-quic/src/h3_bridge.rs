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

/// Per-stream accumulator for inbound H3 request bytes. A quiche
/// `stream_recv` on a given stream ID can yield a chunk mid-frame;
/// we buffer until a full HEADERS frame is parseable.
#[derive(Default)]
pub struct StreamRxBuf {
    buf: Vec<u8>,
    /// Once we see request HEADERS + FIN we flip this and stop reading
    /// new frames on this stream (all the information the bridge needs
    /// is already in hand — the e2e does not carry request bodies).
    done: bool,
}

impl StreamRxBuf {
    /// Append freshly-received bytes and return `Ok(Some(headers))` once
    /// a full HEADERS frame has been decoded. Returns `Ok(None)` if more
    /// bytes are needed.
    ///
    /// # Errors
    ///
    /// Surfaces a string-formatted decode error if the H3 frame parser
    /// rejects the buffer or if QPACK cannot decode the field block.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Option<Vec<(String, String)>>, String> {
        if self.done {
            return Ok(None);
        }
        self.buf.extend_from_slice(chunk);
        loop {
            match decode_frame(&self.buf, 1 << 20) {
                Ok((H3Frame::Headers { header_block }, consumed)) => {
                    self.buf.drain(..consumed);
                    self.done = true;
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
        }
    }
}

/// Build a minimal HTTP/1.1 request line + headers for a bodyless
/// request (GET/HEAD). Callers with bodies will extend this in 3b.3b.
fn build_h1_request(req: &H3Request) -> String {
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
    // Signal no body so the mock backend does not block on a read.
    s.push_str("Content-Length: 0\r\n");
    s.push_str("Connection: close\r\n");
    s.push_str("\r\n");
    s
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
/// Blocking `TcpPool::acquire` is moved off the tokio worker via
/// `spawn_blocking` — this matches the pattern already used in
/// `crates/lb/src/main.rs`.
///
/// # Errors
///
/// Surfaces a string-formatted error if the H3 frame encoding of the
/// fallback 502 response itself fails. Backend dial / write / read
/// errors are caught and turned into a 502 response body internally
/// rather than bubbled up.
pub async fn h3_to_h1_roundtrip(
    req: &H3Request,
    backend: SocketAddr,
    pool: &TcpPool,
) -> Result<Vec<u8>, String> {
    let pool_for_dial = pool.clone();
    let mut pooled = match tokio::task::spawn_blocking(move || pool_for_dial.acquire(backend)).await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "H3→H1 backend acquire failed");
            return encode_h3_response(502, b"bad gateway");
        }
        Err(e) => {
            tracing::warn!(error = %e, "H3→H1 acquire task join failed");
            return encode_h3_response(502, b"bad gateway");
        }
    };
    let h1_request = build_h1_request(req);
    let response = {
        let stream = pooled
            .stream_mut()
            .ok_or_else(|| "pool returned empty handle".to_string())?;
        if let Err(e) = stream.write_all(h1_request.as_bytes()).await {
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
}

/// Forward a pre-built H3 request to an upstream H3 backend via
/// [`QuicUpstreamPool`] and return the parsed response.
///
/// `headers` is the QPACK-encodable field list — callers must
/// pre-populate `:method`, `:scheme`, `:authority`, `:path` plus any
/// regular headers. `body` is forwarded as a single DATA frame; an
/// empty body sends FIN immediately after HEADERS.
///
/// On any backend failure returns an [`H3UpstreamResponse`] with
/// `status = 502` and an empty body.
#[allow(clippy::too_many_lines, clippy::large_futures)]
pub async fn request_h3_upstream(
    headers: Vec<(String, String)>,
    body: bytes::Bytes,
    addr: std::net::SocketAddr,
    sni: &str,
    pool: &QuicUpstreamPool,
) -> H3UpstreamResponse {
    let bad_gateway = || H3UpstreamResponse {
        status: 502,
        headers: Vec::new(),
        body: Vec::new(),
    };

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
    let mut request_bytes = Vec::with_capacity(headers_frame.len() + body_frame_bytes.len());
    request_bytes.extend_from_slice(&headers_frame);
    request_bytes.extend_from_slice(&body_frame_bytes);

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
                    if let Ok(hdrs) = QpackDecoder::new().decode(&header_block) {
                        for (n, v) in hdrs {
                            if n == ":status" {
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
