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
    worker_threads: usize,
}

/// Configuration for building a [`Runtime`].
pub struct RuntimeConfig {
    /// I/O backend to use.
    pub backend: RuntimeBackend,
    /// Number of worker threads. `0` means use all available CPUs.
    pub worker_threads: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::detect(),
            worker_threads: 0,
        }
    }
}

impl Runtime {
    /// Create a new runtime, auto-detecting the best I/O backend and using
    /// all available CPUs as workers.
    pub fn new() -> std::io::Result<Self> {
        Self::with_config(RuntimeConfig::default())
    }

    /// Create a new runtime forcing a specific backend.
    ///
    /// Regardless of the chosen backend, the underlying event loop is always
    /// tokio today.  The `backend` value is recorded so that higher layers
    /// (e.g. the proxy data path) can branch on it.
    pub fn with_backend(backend: RuntimeBackend) -> std::io::Result<Self> {
        Self::with_config(RuntimeConfig {
            backend,
            worker_threads: 0,
        })
    }

    /// Create a new runtime with full configuration.
    pub fn with_config(config: RuntimeConfig) -> std::io::Result<Self> {
        let thread_count = if config.worker_threads == 0 {
            num_cpus()
        } else {
            config.worker_threads
        };

        let inner = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(thread_count)
            .enable_all()
            .thread_name("eg-worker")
            .build()?;

        tracing::info!(
            backend = %config.backend,
            worker_threads = thread_count,
            "runtime initialised"
        );

        Ok(Self {
            backend: config.backend,
            inner,
            worker_threads: thread_count,
        })
    }

    /// Get a handle to the underlying tokio runtime.
    ///
    /// Use this to spawn tasks or enter the runtime context from synchronous
    /// code.
    #[inline]
    pub fn handle(&self) -> &tokio::runtime::Handle {
        self.inner.handle()
    }

    /// The I/O backend in use.
    #[inline]
    pub fn backend(&self) -> RuntimeBackend {
        self.backend
    }

    /// The number of worker threads.
    #[inline]
    pub fn worker_threads(&self) -> usize {
        self.worker_threads
    }

    /// Block the current thread on a future.
    ///
    /// Convenience wrapper around `tokio::runtime::Runtime::block_on`.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.inner.block_on(future)
    }
}

/// Return the number of available CPUs (logical cores).
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_creates_with_auto_detect() {
        let rt = Runtime::new().expect("failed to create runtime");
        let backend = rt.backend();
        assert!(backend == RuntimeBackend::IoUring || backend == RuntimeBackend::Epoll);
        assert!(rt.worker_threads() >= 1);
    }

    #[test]
    fn runtime_creates_with_forced_epoll() {
        let rt = Runtime::with_backend(RuntimeBackend::Epoll)
            .expect("failed to create runtime with epoll");
        assert_eq!(rt.backend(), RuntimeBackend::Epoll);
    }

    #[test]
    fn runtime_creates_with_explicit_threads() {
        let rt = Runtime::with_config(RuntimeConfig {
            backend: RuntimeBackend::Epoll,
            worker_threads: 2,
        })
        .expect("failed to create runtime with 2 threads");
        assert_eq!(rt.worker_threads(), 2);
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
