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
///
/// Feed bytes via [`feed`](Self::feed) and collect decoded body chunks
/// and optional trailers.
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

    /// Feed more data into the decoder. Call repeatedly as data arrives.
    ///
    /// Returns `Ok(true)` when the entire chunked body has been decoded,
    /// `Ok(false)` when more data is needed.
    ///
    /// # Errors
    ///
    /// Returns `H1Error::InvalidChunkEncoding` on malformed input.
    pub fn feed(&mut self, data: &[u8]) -> Result<bool, H1Error> {
        self.buf.extend_from_slice(data);
        self.process()
    }

    /// Return all decoded body chunks accumulated so far.
    #[must_use]
    pub fn take_body(&mut self) -> Vec<Bytes> {
        core::mem::take(&mut self.body_chunks)
    }

    /// Return the trailers (available only after decoding completes).
    #[must_use]
    pub fn trailers(&self) -> &[(String, String)] {
        &self.trailers
    }

    /// Whether the decoder has reached the terminal state.
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

    /// Try to parse a chunk-size line from `self.buf`.
    /// Returns `true` if a size line was consumed, `false` if incomplete.
    fn try_read_size(&mut self) -> Result<bool, H1Error> {
        let Some(crlf_pos) = find_crlf_in(&self.buf) else {
            return Ok(false);
        };

        let size_line = self
            .buf
            .get(..crlf_pos)
            .ok_or(H1Error::InvalidChunkEncoding)?;
        let size_str =
            core::str::from_utf8(size_line).map_err(|_| H1Error::InvalidChunkEncoding)?;

        // Chunk extensions (`;key=value`) are allowed but we ignore them.
        let hex_part = size_str
            .split(';')
            .next()
            .ok_or(H1Error::InvalidChunkEncoding)?;
        let hex_trimmed = hex_part.trim();
        let chunk_size =
            usize::from_str_radix(hex_trimmed, 16).map_err(|_| H1Error::InvalidChunkEncoding)?;

        // Consume the size line + CRLF.
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

    /// Try to read trailers (or just the terminating CRLF).
    fn try_read_trailers(&mut self) -> Result<bool, H1Error> {
        if self.buf.len() < 2 {
            return Ok(false);
        }

        if self.buf.first().copied() == Some(b'\r') && self.buf.get(1).copied() == Some(b'\n') {
            let _ = self.buf.split_to(2);
            self.state = DecoderState::Done;
            return Ok(true);
        }

        // There are trailers — we need to find \r\n\r\n.
        let Some(end_pos) = find_double_crlf_in(&self.buf) else {
            return Ok(false);
        };

        // Include the trailing \r\n so the last trailer line has its CRLF terminator.
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
                let name = line_str
                    .get(..colon)
                    .ok_or(H1Error::InvalidChunkEncoding)?
                    .trim()
                    .to_string();
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

        // Consume trailers + \r\n\r\n.
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

    /// Encode a body chunk into chunked transfer encoding format.
    ///
    /// # Errors
    ///
    /// Returns `H1Error::InvalidChunkEncoding` if called after [`finish`](Self::finish).
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
    ///
    /// Returns `H1Error::InvalidChunkEncoding` if called more than once.
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

/// Find `\r\n` in `buf`, returning index of `\r`.
fn find_crlf_in(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(1))
        .find(|&i| buf.get(i).copied() == Some(b'\r') && buf.get(i + 1).copied() == Some(b'\n'))
}

/// Find `\r\n\r\n` in `buf`, returning index of the first `\r`.
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
