//! Unified async runtime that wraps tokio and records the active I/O backend.
//!
//! [`Runtime`] owns a `tokio::runtime::Runtime` and exposes the handle for
//! spawning tasks.  It also tracks whether the underlying platform supports
//! io_uring so higher layers can choose optimised code paths.

use crate::backend::RuntimeBackend;

/// Unified async runtime for ExpressGateway.
///
/// Wraps a multi-threaded tokio runtime and records the detected (or forced)
/// I/O backend.
pub struct Runtime {
    backend: RuntimeBackend,
    inner: tokio::runtime::Runtime,
}

impl Runtime {
    /// Create a new runtime, auto-detecting the best I/O backend.
    ///
    /// Builds a multi-threaded tokio runtime with all features enabled.
    pub fn new() -> std::io::Result<Self> {
        Self::with_backend(RuntimeBackend::detect())
    }

    /// Create a new runtime forcing a specific backend.
    ///
    /// Regardless of the chosen backend, the underlying event loop is always
    /// tokio today.  The `backend` value is recorded so that higher layers
    /// (e.g. the proxy data path) can branch on it.
    pub fn with_backend(backend: RuntimeBackend) -> std::io::Result<Self> {
        let inner = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        tracing::info!(%backend, "runtime initialised");

        Ok(Self { backend, inner })
    }

    /// Get a handle to the underlying tokio runtime.
    ///
    /// Use this to spawn tasks or enter the runtime context from synchronous
    /// code.
    pub fn handle(&self) -> &tokio::runtime::Handle {
        self.inner.handle()
    }

    /// The I/O backend in use.
    pub fn backend(&self) -> RuntimeBackend {
        self.backend
    }

    /// Block the current thread on a future.
    ///
    /// Convenience wrapper around `tokio::runtime::Runtime::block_on`.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.inner.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_creates_with_auto_detect() {
        let rt = Runtime::new().expect("failed to create runtime");
        let backend = rt.backend();
        assert!(backend == RuntimeBackend::IoUring || backend == RuntimeBackend::Epoll);
    }

    #[test]
    fn runtime_creates_with_forced_epoll() {
        let rt = Runtime::with_backend(RuntimeBackend::Epoll)
            .expect("failed to create runtime with epoll");
        assert_eq!(rt.backend(), RuntimeBackend::Epoll);
    }

    #[test]
    fn handle_is_usable() {
        let rt = Runtime::new().expect("failed to create runtime");
        let result = rt.block_on(async { 42 });
        assert_eq!(result, 42);
    }

    #[test]
    fn spawn_and_await() {
        let rt = Runtime::new().expect("failed to create runtime");
        let result = rt.block_on(async {
            let handle = tokio::spawn(async { 7 + 8 });
            handle.await.unwrap()
        });
        assert_eq!(result, 15);
    }
}
