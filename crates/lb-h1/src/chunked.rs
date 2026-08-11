//! Chunked transfer encoding encoder and decoder.

use bytes::{BufMut, Bytes, BytesMut};

use crate::H1Error;

/// Internal state of the chunked decoder state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecoderState {
    /// Expecting a chunk-size line.
    ReadingSize,
    /// Reading chunk data of the given length.
    ReadingData { remaining: usize },
    /// Expecting the CRLF after chunk data.
    ReadingDataCrlf,
    /// Reading optional trailers after the zero-length final chunk.
    ReadingTrailers,
    /// Transfer is complete.
    Done,
}

/// Incremental decoder for HTTP/1.1 chunked transfer encoding.
#[derive(Debug)]
pub struct ChunkedDecoder {
    state: DecoderState,
    buf: BytesMut,
    /// Accumulated decoded body chunks.
    body_chunks: Vec<Bytes>,
    /// Trailers parsed after the final chunk, if any.
    trailers: Vec<(String, String)>,
}

impl ChunkedDecoder {
    /// Create a new decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: DecoderState::ReadingSize,
            buf: BytesMut::new(),
            body_chunks: Vec::new(),
            trailers: Vec::new(),
        }
    }

    /// Feed more data; `Ok(true)` once the whole body has been decoded.
    ///
    /// # Errors
    /// `H1Error::InvalidChunkEncoding` on malformed input.
    pub fn feed(&mut self, data: &[u8]) -> Result<bool, H1Error> {
        self.buf.extend_from_slice(data);
        self.process()
    }

    /// Decoded body chunks accumulated so far.
    #[must_use]
    pub fn take_body(&mut self) -> Vec<Bytes> {
        core::mem::take(&mut self.body_chunks)
    }

    /// Trailers; only populated once decoding completes.
    #[must_use]
    pub fn trailers(&self) -> &[(String, String)] {
        &self.trailers
    }

    /// Has the decoder reached its terminal state?
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.state == DecoderState::Done
    }

    /// Drive the state machine until it blocks on more data.
    fn process(&mut self) -> Result<bool, H1Error> {
        loop {
            match self.state {
                DecoderState::ReadingSize => {
                    if !self.try_read_size()? {
                        return Ok(false);
                    }
                }
                DecoderState::ReadingData { remaining } => {
                    if remaining == 0 {
                        self.state = DecoderState::ReadingDataCrlf;
                        continue;
                    }
                    if self.buf.is_empty() {
                        return Ok(false);
                    }
                    let available = self.buf.len().min(remaining);
                    let chunk = self.buf.split_to(available).freeze();
                    let new_remaining = remaining - available;
                    self.body_chunks.push(chunk);
                    self.state = DecoderState::ReadingData {
                        remaining: new_remaining,
                    };
                }
                DecoderState::ReadingDataCrlf => {
                    if self.buf.len() < 2 {
                        return Ok(false);
                    }
                    if self.buf.first().copied() != Some(b'\r')
                        || self.buf.get(1).copied() != Some(b'\n')
                    {
                        return Err(H1Error::InvalidChunkEncoding);
                    }
                    let _ = self.buf.split_to(2);
                    self.state = DecoderState::ReadingSize;
                }
                DecoderState::ReadingTrailers => {
                    return self.try_read_trailers();
                }
                DecoderState::Done => return Ok(true),
            }
        }
    }

    /// Consume a chunk-size line from `self.buf`; `false` if incomplete.
    ///
    /// RFC 9112 §7.1.1 `chunk-size = 1*HEXDIG`: leading `+`/`-`, any internal
    /// whitespace, and more than 16 hex digits all reject (nginx
    /// CVE-2013-2028, hyper GHSA-5h46-h7hh-c6x9, HAProxy `BUG/MAJOR: mux_h1:
    /// fix stack buffer overflow in h1_append_chunk_size`).
    fn try_read_size(&mut self) -> Result<bool, H1Error> {
        let Some(crlf_pos) = find_crlf_in(&self.buf) else {
            return Ok(false);
        };

        let size_line = self
            .buf
            .get(..crlf_pos)
            .ok_or(H1Error::InvalidChunkEncoding)?;

        // RAW BYTES before the first `;` (chunk-ext) — do NOT decode as UTF-8
        // or trim: whitespace around the size token is a protocol violation.
        let hex_part: &[u8] = size_line.split(|&b| b == b';').next().unwrap_or(size_line);

        let chunk_size_u64 = parse_chunk_size_hex(hex_part)?;
        // Reject sizes that would not fit `usize` (32-bit truncation guard).
        let chunk_size =
            usize::try_from(chunk_size_u64).map_err(|_| H1Error::InvalidChunkEncoding)?;

        let _ = self.buf.split_to(crlf_pos + 2);

        if chunk_size == 0 {
            self.state = DecoderState::ReadingTrailers;
        } else {
            self.state = DecoderState::ReadingData {
                remaining: chunk_size,
            };
        }
        Ok(true)
    }

    /// Read trailers, or just the terminating CRLF.
    fn try_read_trailers(&mut self) -> Result<bool, H1Error> {
        if self.buf.len() < 2 {
            return Ok(false);
        }

        if self.buf.first().copied() == Some(b'\r') && self.buf.get(1).copied() == Some(b'\n') {
            let _ = self.buf.split_to(2);
            self.state = DecoderState::Done;
            return Ok(true);
        }

        let Some(end_pos) = find_double_crlf_in(&self.buf) else {
            return Ok(false);
        };

        // Include the trailing CRLF so the last line is terminated.
        let trailer_block = self
            .buf
            .get(..end_pos + 2)
            .ok_or(H1Error::InvalidChunkEncoding)?;

        let mut pos = 0;
        while pos < trailer_block.len() {
            let remaining = trailer_block
                .get(pos..)
                .ok_or(H1Error::InvalidChunkEncoding)?;
            let line_end = find_crlf_in(remaining).ok_or(H1Error::InvalidChunkEncoding)?;
            let line = remaining
                .get(..line_end)
                .ok_or(H1Error::InvalidChunkEncoding)?;
            let line_str = core::str::from_utf8(line).map_err(|_| H1Error::InvalidChunkEncoding)?;

            if let Some(colon) = line_str.find(':') {
                // ROUND8-L7-03: trailers get the SAME strict RFC 9110 §5.1
                // name rules (HAProxy CVE-2023-25725 / nginx CVE-2019-9516).
                let raw_name = line_str.get(..colon).ok_or(H1Error::InvalidChunkEncoding)?;
                if raw_name.is_empty()
                    || !raw_name.bytes().all(crate::parse::__is_tchar_for_trailer)
                {
                    return Err(H1Error::InvalidChunkEncoding);
                }
                let name = raw_name.to_string();
                let value = line_str
                    .get(colon + 1..)
                    .ok_or(H1Error::InvalidChunkEncoding)?
                    .trim()
                    .to_string();
                self.trailers.push((name, value));
            } else {
                return Err(H1Error::InvalidChunkEncoding);
            }
            pos += line_end + 2;
        }

        let _ = self.buf.split_to(end_pos + 4);
        self.state = DecoderState::Done;
        Ok(true)
    }
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Encoder that wraps body chunks in HTTP/1.1 chunked transfer encoding.
#[derive(Debug)]
pub struct ChunkedEncoder {
    finished: bool,
}

impl ChunkedEncoder {
    /// Create a new encoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { finished: false }
    }

    /// Encode a body chunk.
    ///
    /// # Errors
    /// `H1Error::InvalidChunkEncoding` if called after [`finish`](Self::finish).
    pub fn encode(&mut self, data: &[u8]) -> Result<Bytes, H1Error> {
        if self.finished {
            return Err(H1Error::InvalidChunkEncoding);
        }
        if data.is_empty() {
            return Ok(Bytes::new());
        }
        let size_line = format!("{:x}\r\n", data.len());
        let mut out = BytesMut::with_capacity(size_line.len() + data.len() + 2);
        out.put_slice(size_line.as_bytes());
        out.put_slice(data);
        out.put_slice(b"\r\n");
        Ok(out.freeze())
    }

    /// Emit the final zero-length chunk, optionally with trailers.
    ///
    /// # Errors
    /// `H1Error::InvalidChunkEncoding` if called more than once.
    pub fn finish(&mut self, trailers: &[(String, String)]) -> Result<Bytes, H1Error> {
        if self.finished {
            return Err(H1Error::InvalidChunkEncoding);
        }
        self.finished = true;

        let mut out = BytesMut::new();
        out.put_slice(b"0\r\n");

        for (name, value) in trailers {
            let line = format!("{name}: {value}\r\n");
            out.put_slice(line.as_bytes());
        }
        out.put_slice(b"\r\n");

        Ok(out.freeze())
    }
}

impl Default for ChunkedEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// RFC 9112 §7.1.1 `chunk-size` lexer, `1*HEXDIG` only: empty input, >16 hex
/// digits (the nginx CVE-2013-2028 leading-zero pad class), and any byte
/// outside `0-9A-Fa-f` all reject.
///
/// F-PARSE-3: the 16-digit cap IS the overflow defense — the
/// `value.checked_shl(4)` below is INERT belt-and-braces under it (the shift
/// amount is never ≥ the bit width, and it cannot see a high nibble shifted
/// out). Do NOT rely on it as the guard; keep the digit cap.
fn parse_chunk_size_hex(line: &[u8]) -> Result<u64, H1Error> {
    if line.is_empty() {
        return Err(H1Error::InvalidChunkEncoding);
    }
    if line.len() > 16 {
        return Err(H1Error::InvalidChunkEncoding);
    }
    let mut value: u64 = 0;
    for &b in line {
        let nibble: u64 = match b {
            b'0'..=b'9' => u64::from(b - b'0'),
            b'a'..=b'f' => u64::from(b - b'a' + 10),
            b'A'..=b'F' => u64::from(b - b'A' + 10),
            _ => return Err(H1Error::InvalidChunkEncoding),
        };
        value = value.checked_shl(4).ok_or(H1Error::InvalidChunkEncoding)?;
        value |= nibble;
    }
    Ok(value)
}

fn find_crlf_in(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(1))
        .find(|&i| buf.get(i).copied() == Some(b'\r') && buf.get(i + 1).copied() == Some(b'\n'))
}

fn find_double_crlf_in(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(3)).find(|&i| {
        buf.get(i).copied() == Some(b'\r')
            && buf.get(i + 1).copied() == Some(b'\n')
            && buf.get(i + 2).copied() == Some(b'\r')
            && buf.get(i + 3).copied() == Some(b'\n')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let mut enc = ChunkedEncoder::new();
        let mut output = BytesMut::new();
        output.extend_from_slice(&enc.encode(b"Hello").unwrap());
        output.extend_from_slice(&enc.encode(b" World").unwrap());
        output.extend_from_slice(&enc.finish(&[]).unwrap());

        let mut dec = ChunkedDecoder::new();
        let done = dec.feed(&output).unwrap();
        assert!(done);

        let body = dec.take_body();
        let full: Vec<u8> = body.iter().flat_map(|b| b.iter().copied()).collect();
        assert_eq!(full, b"Hello World");
    }

    #[test]
    fn encode_decode_with_trailers() {
        let mut enc = ChunkedEncoder::new();
        let mut output = BytesMut::new();
        output.extend_from_slice(&enc.encode(b"data").unwrap());
        let trailers = vec![("Checksum".to_string(), "abc123".to_string())];
        output.extend_from_slice(&enc.finish(&trailers).unwrap());

        let mut dec = ChunkedDecoder::new();
        let done = dec.feed(&output).unwrap();
        assert!(done);

        let body = dec.take_body();
        assert_eq!(body.len(), 1);
        assert_eq!(&body[0][..], b"data");

        let t = dec.trailers();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].0, "Checksum");
        assert_eq!(t[0].1, "abc123");
    }

    #[test]
    fn incremental_feed() {
        let input = b"5\r\nHello\r\n0\r\n\r\n";
        let mut dec = ChunkedDecoder::new();
        for &b in input.iter().take(input.len() - 1) {
            let done = dec.feed(&[b]).unwrap();
            assert!(!done);
        }
        let done = dec.feed(&[*input.last().unwrap()]).unwrap();
        assert!(done);
        let body = dec.take_body();
        let full: Vec<u8> = body.iter().flat_map(|b| b.iter().copied()).collect();
        assert_eq!(full, b"Hello");
    }
}
