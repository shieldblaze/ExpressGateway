//! Request body handling with size limits and timeout defenses.
//!
//! Provides per-request maximum body size enforcement (413 Content Too Large),
//! request header timeout (Slowloris defense), request body timeout (slow-POST
//! defense), and body streaming without full buffering.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body::Frame;
use pin_project_lite::pin_project;
use tokio::time::{Instant, Sleep};

use crate::errors::HttpError;

/// Configuration for body handling limits and timeouts.
#[derive(Debug, Clone)]
pub struct BodyConfig {
    /// Maximum request body size in bytes. `None` means unlimited.
    pub max_body_size: Option<u64>,
    /// Timeout for receiving request headers (Slowloris defense).
    pub header_timeout: Duration,
    /// Timeout for receiving the complete request body (slow-POST defense).
    pub body_timeout: Duration,
}

impl Default for BodyConfig {
    fn default() -> Self {
        Self {
            max_body_size: Some(10 * 1024 * 1024), // 10 MB
            header_timeout: Duration::from_secs(30),
            body_timeout: Duration::from_secs(60),
        }
    }
}

pin_project! {
    /// A body wrapper that enforces a maximum size limit and a read timeout.
    ///
    /// Wraps an inner [`http_body::Body`] and tracks total bytes received. If the
    /// limit is exceeded, the stream yields a 413 error. If no data arrives within
    /// `body_timeout`, the stream yields a timeout error.
    pub struct LimitedBody<B> {
        #[pin]
        inner: B,
        bytes_received: u64,
        max_size: Option<u64>,
        #[pin]
        deadline: Sleep,
    }
}

impl<B> LimitedBody<B> {
    /// Create a new limited body wrapper.
    pub fn new(inner: B, max_size: Option<u64>, timeout: Duration) -> Self {
        Self {
            inner,
            bytes_received: 0,
            max_size,
            deadline: tokio::time::sleep_until(Instant::now() + timeout),
        }
    }

    /// Total bytes received so far.
    pub fn bytes_received(&self) -> u64 {
        self.bytes_received
    }
}

impl<B> http_body::Body for LimitedBody<B>
where
    B: http_body::Body<Data = Bytes, Error = HttpError>,
{
    type Data = Bytes;
    type Error = HttpError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();

        // Check deadline first.
        if this.deadline.poll(cx).is_ready() {
            return Poll::Ready(Some(Err(HttpError::new(
                http::StatusCode::REQUEST_TIMEOUT,
                "Request body timeout",
            ))));
        }

        match this.inner.poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    *this.bytes_received += data.len() as u64;
                    if let Some(max) = this.max_size
                        && *this.bytes_received > *max
                    {
                        return Poll::Ready(Some(Err(HttpError::content_too_large(
                            *this.bytes_received,
                            *max,
                        ))));
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// Check a `Content-Length` header value against the max body size.
///
/// Returns `Err(HttpError)` with 413 if the declared size exceeds the limit.
pub fn check_content_length(content_length: u64, max_size: u64) -> Result<(), HttpError> {
    if content_length > max_size {
        Err(HttpError::content_too_large(content_length, max_size))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = BodyConfig::default();
        assert_eq!(config.max_body_size, Some(10 * 1024 * 1024));
        assert_eq!(config.header_timeout, Duration::from_secs(30));
        assert_eq!(config.body_timeout, Duration::from_secs(60));
    }

    #[test]
    fn check_content_length_ok() {
        assert!(check_content_length(500, 1000).is_ok());
    }

    #[test]
    fn check_content_length_exact() {
        assert!(check_content_length(1000, 1000).is_ok());
    }

    #[test]
    fn check_content_length_exceeds() {
        let err = check_content_length(1001, 1000).unwrap_err();
        assert_eq!(err.status, http::StatusCode::PAYLOAD_TOO_LARGE);
    }
}
