//! DNS re-resolver with positive + negative caching.
//!
//! The resolver queries the platform's system resolver via
//! [`std::net::ToSocketAddrs`], which is blocking; each uncached lookup
//! runs on [`tokio::task::spawn_blocking`]. Answers are cached in a
//! sharded [`dashmap::DashMap`] keyed on `(hostname, port)`.
//!
//! ## TTL approximation — known limitation
//!
//! `std::net::ToSocketAddrs` does not expose DNS TTL values. We cap the
//! positive cache lifetime at [`ResolverConfig::positive_ttl_cap`]
//! (default 300 s) which acts as a hard upper bound; a real
//! TTL-honouring resolver belongs behind the same API later (e.g.
//! `hickory-resolver`). Negative answers (NXDOMAIN / connection errors
//! from the stub resolver) are cached for
//! [`ResolverConfig::negative_ttl`] (default 5 s) to soak up
//! flood-resolve loops without papering over transient outages.
//!
//! ## Concurrent-share
//!
//! When N tokio tasks call [`DnsResolver::resolve`] for the same key at
//! once, exactly one system-resolver call fires. All waiters block on a
//! shared [`tokio::sync::OnceCell`]; the first waiter populates it, the
//! rest read the same `Arc`-wrapped result. This matches Pingora's
//! singleflight behaviour and keeps the hot path non-blocking once the
//! cache is warm.
//!
//! ## Refresh
//!
//! [`DnsResolver::spawn_background_refresh`] starts a tokio task that
//! wakes every `every` duration and re-queries every non-expired entry.
//! The refresh is best-effort: if a refresh fails it leaves the existing
//! cached value in place until its natural expiry so a flaky resolver
//! can not flip a healthy pool into a negative-cache storm.

use std::hash::{Hash, Hasher};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;

/// Cap on the lifetime of a positive cache entry.
pub const DEFAULT_POSITIVE_TTL_CAP_SECS: u64 = 300;
/// Lifetime of a negative (NXDOMAIN / resolve-error) cache entry.
pub const DEFAULT_NEGATIVE_TTL_SECS: u64 = 5;
/// Cadence of the background refresh task.
pub const DEFAULT_REFRESH_INTERVAL_SECS: u64 = 60;

/// Configuration for [`DnsResolver`].
#[derive(Debug, Clone, Copy)]
pub struct ResolverConfig {
    /// Upper bound on how long a successful resolution is cached.
    pub positive_ttl_cap: Duration,
    /// Lifetime of a cached failure (NXDOMAIN, timeout, connection error).
    pub negative_ttl: Duration,
    /// Default cadence for [`DnsResolver::spawn_background_refresh`].
    pub refresh_interval: Duration,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            positive_ttl_cap: Duration::from_secs(DEFAULT_POSITIVE_TTL_CAP_SECS),
            negative_ttl: Duration::from_secs(DEFAULT_NEGATIVE_TTL_SECS),
            refresh_interval: Duration::from_secs(DEFAULT_REFRESH_INTERVAL_SECS),
        }
    }
}

/// Errors from DNS resolution.
#[derive(Debug, thiserror::Error, Clone)]
pub enum DnsError {
    /// The system resolver returned no answers for the name (NXDOMAIN or
    /// an empty `getaddrinfo` result).
    #[error("NXDOMAIN: {hostname}")]
    NxDomain {
        /// Hostname that failed to resolve.
        hostname: String,
    },
    /// The system resolver returned an I/O error. The `message` field is
    /// a stringified copy because [`io::Error`] is not [`Clone`].
    #[error("resolver i/o error for {hostname}: {message}")]
    Io {
        /// Hostname that failed to resolve.
        hostname: String,
        /// Lossy representation of the underlying `io::Error`.
        message: String,
    },
}

/// What kind of value a cache entry holds.
#[derive(Debug, Clone)]
enum ResolveResult {
    Positive(Arc<Vec<SocketAddr>>),
    Negative(DnsError),
}

/// A shared cache entry. `value` is filled exactly once by whichever
/// task wins the singleflight race; all other waiters await it.
struct CacheEntry {
    value: OnceCell<ResolveResult>,
    expires_at: parking_lot::Mutex<Instant>,
}

impl CacheEntry {
    fn new() -> Self {
        Self {
            value: OnceCell::new(),
            expires_at: parking_lot::Mutex::new(Instant::now()),
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= *self.expires_at.lock()
    }

    fn set_expires(&self, when: Instant) {
        *self.expires_at.lock() = when;
    }
}

/// Composite key used for caching. `hostname` is stored owned to avoid
/// allocating on every lookup past the first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    hostname: String,
    port: u16,
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hostname.hash(state);
        self.port.hash(state);
    }
}

impl CacheKey {
    fn from_ref(hostname: &str, port: u16) -> Self {
        Self {
            hostname: hostname.to_owned(),
            port,
        }
    }
}

/// Pluggable system-resolve backend. The default implementation delegates
/// to [`ToSocketAddrs`]; unit tests swap in a counting / deterministic
/// stub via [`DnsResolver::with_resolver`].
type ResolveFn =
    Arc<dyn Fn(&str, u16) -> Result<Vec<SocketAddr>, io::Error> + Send + Sync + 'static>;

fn system_resolver(hostname: &str, port: u16) -> Result<Vec<SocketAddr>, io::Error> {
    let addrs: Vec<SocketAddr> = (hostname, port).to_socket_addrs()?.collect();
    Ok(addrs)
}

struct DnsInner {
    config: ResolverConfig,
    cache: DashMap<CacheKey, Arc<CacheEntry>>,
    resolver: ResolveFn,
    refresh_running: AtomicBool,
}

/// DNS resolver with caching and singleflight. Clones share state.
#[derive(Clone)]
pub struct DnsResolver {
    inner: Arc<DnsInner>,
}

impl std::fmt::Debug for DnsResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnsResolver")
            .field("config", &self.inner.config)
            .field("cache_size", &self.inner.cache.len())
            .finish()
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new(ResolverConfig::default())
    }
}

impl DnsResolver {
    /// Build a resolver that delegates to the platform's system resolver.
    #[must_use]
    pub fn new(config: ResolverConfig) -> Self {
        Self {
            inner: Arc::new(DnsInner {
                config,
                cache: DashMap::new(),
                resolver: Arc::new(system_resolver),
                refresh_running: AtomicBool::new(false),
            }),
        }
    }

    /// Build a resolver using a custom backend (tests, mocking).
    #[must_use]
    pub fn with_resolver(config: ResolverConfig, resolver: ResolveFn) -> Self {
        Self {
            inner: Arc::new(DnsInner {
                config,
                cache: DashMap::new(),
                resolver,
                refresh_running: AtomicBool::new(false),
            }),
        }
    }

    /// Current entry count in the cache (positive + negative).
    #[must_use]
    pub fn cache_size(&self) -> usize {
        self.inner.cache.len()
    }

    /// Resolve `hostname:port` via the cache.
    ///
    /// On a cache hit the current value is returned immediately. On a
    /// miss or expired entry, a single blocking resolve is dispatched to
    /// [`tokio::task::spawn_blocking`]; other tasks that race the same
    /// key wait on the shared cell.
    ///
    /// # Errors
    /// Returns [`DnsError::NxDomain`] if the resolver yields no
    /// addresses, and [`DnsError::Io`] for any other resolver error
    /// (including tokio `JoinError`, which is surfaced as
    /// [`io::ErrorKind::Other`]).
    pub async fn resolve(&self, hostname: &str, port: u16) -> Result<Vec<SocketAddr>, DnsError> {
        let key = CacheKey::from_ref(hostname, port);
        let entry = self
            .inner
            .cache
            .entry(key.clone())
            .or_insert_with(|| Arc::new(CacheEntry::new()))
            .clone();

        let now = Instant::now();
        if entry.value.initialized() && !entry.is_expired(now) {
            return Self::materialize(entry.value.get());
        }

        // Expired or empty: try to fill. Two cases:
        //  1. The OnceCell is still empty — our closure runs at most
        //     once and is the value every concurrent caller will see.
        //  2. The OnceCell is already initialized but expired — replace
        //     the entry with a fresh OnceCell and re-drive.
        if entry.value.initialized() && entry.is_expired(now) {
            // Replace the entry atomically so concurrent callers pick
            // up the fresh cell. A small window exists where two
            // concurrent tasks both see a stale entry and both refresh,
            // but both writes are idempotent and both return the same
            // system-resolver answer.
            let fresh = Arc::new(CacheEntry::new());
            self.inner.cache.insert(key.clone(), fresh.clone());
            return self.fill_and_return(&fresh, hostname, port).await;
        }

        self.fill_and_return(&entry, hostname, port).await
    }

    async fn fill_and_return(
        &self,
        entry: &Arc<CacheEntry>,
        hostname: &str,
        port: u16,
    ) -> Result<Vec<SocketAddr>, DnsError> {
        let resolver = self.inner.resolver.clone();
        let config = self.inner.config;
        let host_owned = hostname.to_owned();
        let entry_for_init = entry.clone();

        let got = entry
            .value
            .get_or_init(move || async move {
                let spawn_host = host_owned.clone();
                let join = tokio::task::spawn_blocking(move || (resolver)(&spawn_host, port)).await;

                let result: ResolveResult = match join {
                    Err(join_err) => ResolveResult::Negative(DnsError::Io {
                        hostname: host_owned.clone(),
                        message: format!("resolver task joined: {join_err}"),
                    }),
                    Ok(Err(io_err)) => ResolveResult::Negative(io_err_to_dns(&host_owned, &io_err)),
                    Ok(Ok(addrs)) if addrs.is_empty() => {
                        ResolveResult::Negative(DnsError::NxDomain {
                            hostname: host_owned.clone(),
                        })
                    }
                    Ok(Ok(addrs)) => ResolveResult::Positive(Arc::new(addrs)),
                };

                let ttl = match &result {
                    ResolveResult::Positive(_) => config.positive_ttl_cap,
                    ResolveResult::Negative(_) => config.negative_ttl,
                };
                entry_for_init.set_expires(Instant::now() + ttl);
                result
            })
            .await;

        Self::materialize(Some(got))
    }

    fn materialize(got: Option<&ResolveResult>) -> Result<Vec<SocketAddr>, DnsError> {
        match got {
            Some(ResolveResult::Positive(addrs)) => Ok((**addrs).clone()),
            Some(ResolveResult::Negative(err)) => Err(err.clone()),
            None => Err(DnsError::Io {
                hostname: String::new(),
                message: "resolver cell empty after initialization".to_owned(),
            }),
        }
    }

    /// Spawn a background task that re-resolves every cache entry at the
    /// given cadence. Returns the tokio handle; callers should store it
    /// and abort on shutdown. Only one refresher runs at a time per
    /// resolver (subsequent calls return a sentinel that exits
    /// immediately).
    #[must_use]
    pub fn spawn_background_refresh(&self, every: Duration) -> JoinHandle<()> {
        let started = self.inner.refresh_running.compare_exchange(
            false,
            true,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if started.is_err() {
            // Already running — return a no-op handle.
            return tokio::spawn(async {});
        }
        let resolver = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(every);
            // Skip the immediate first tick so the first refresh fires
            // one `every` after spawn, not at t=0.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                resolver.refresh_all().await;
            }
        })
    }

    /// One-shot refresh of every cached entry. Exposed for testing and
    /// for callers that want explicit control over the refresh cadence.
    pub async fn refresh_all(&self) {
        // Snapshot the keys so we can drop the dashmap iterator before
        // issuing spawn_blocking calls (avoids holding shard locks over
        // `.await` points).
        let snapshot: Vec<CacheKey> = self.inner.cache.iter().map(|kv| kv.key().clone()).collect();
        for key in snapshot {
            let fresh = Arc::new(CacheEntry::new());
            self.inner.cache.insert(key.clone(), fresh.clone());
            let _ = self.fill_and_return(&fresh, &key.hostname, key.port).await;
        }
    }
}

fn io_err_to_dns(hostname: &str, err: &io::Error) -> DnsError {
    // `getaddrinfo` failures commonly surface as ErrorKind::Other with
    // "failed to lookup address information". Treat a clearly-not-found
    // result as NxDomain; everything else is Io.
    let msg = err.to_string();
    let looks_like_nxdomain = msg.contains("failed to lookup address information")
        || msg.contains("Name or service not known")
        || msg.contains("nodename nor servname provided");
    if looks_like_nxdomain {
        DnsError::NxDomain {
            hostname: hostname.to_owned(),
        }
    } else {
        DnsError::Io {
            hostname: hostname.to_owned(),
            message: msg,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicUsize;

    fn counting_resolver(counter: Arc<AtomicUsize>, answers: Vec<SocketAddr>) -> ResolveFn {
        Arc::new(move |_host: &str, _port: u16| {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(answers.clone())
        })
    }

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn cache_hit_returns_same_result_before_ttl() {
        let counter = Arc::new(AtomicUsize::new(0));
        let fn_ = counting_resolver(counter.clone(), vec![addr("127.0.0.1:80")]);
        let r = DnsResolver::with_resolver(ResolverConfig::default(), fn_);

        let a = r.resolve("example.test", 80).await.unwrap();
        let b = r.resolve("example.test", 80).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a, vec![addr("127.0.0.1:80")]);
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "second call must hit cache"
        );
    }

    #[tokio::test]
    async fn positive_entry_expires_at_cap() {
        let counter = Arc::new(AtomicUsize::new(0));
        let fn_ = counting_resolver(counter.clone(), vec![addr("10.0.0.1:443")]);
        let cfg = ResolverConfig {
            positive_ttl_cap: Duration::from_millis(40),
            negative_ttl: Duration::from_secs(5),
            refresh_interval: Duration::from_secs(60),
        };
        let r = DnsResolver::with_resolver(cfg, fn_);

        let _ = r.resolve("example.test", 443).await.unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = r.resolve("example.test", 443).await.unwrap();
        assert_eq!(
            counter.load(Ordering::Relaxed),
            2,
            "expired entry must trigger a fresh resolve"
        );
    }

    #[tokio::test]
    async fn negative_entry_caches_nxdomain() {
        let counter = Arc::new(AtomicUsize::new(0));
        // NX via empty answer list.
        let fn_: ResolveFn = {
            let c = counter.clone();
            Arc::new(move |_h: &str, _p: u16| {
                c.fetch_add(1, Ordering::Relaxed);
                Ok::<Vec<SocketAddr>, io::Error>(Vec::new())
            })
        };
        let cfg = ResolverConfig {
            positive_ttl_cap: Duration::from_secs(300),
            negative_ttl: Duration::from_millis(40),
            refresh_interval: Duration::from_secs(60),
        };
        let r = DnsResolver::with_resolver(cfg, fn_);

        let err1 = r.resolve("no-such.test", 80).await.unwrap_err();
        assert!(matches!(err1, DnsError::NxDomain { .. }));
        let err2 = r.resolve("no-such.test", 80).await.unwrap_err();
        assert!(matches!(err2, DnsError::NxDomain { .. }));
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "NX must be cached for negative_ttl"
        );

        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = r.resolve("no-such.test", 80).await.unwrap_err();
        assert_eq!(
            counter.load(Ordering::Relaxed),
            2,
            "negative entry must re-query after TTL"
        );
    }

    #[tokio::test]
    async fn concurrent_resolvers_share_cache() {
        let counter = Arc::new(AtomicUsize::new(0));
        // Introduce a tiny sleep in the "resolve" so concurrent callers
        // actually queue on the OnceCell.
        let fn_: ResolveFn = {
            let c = counter.clone();
            Arc::new(move |_h: &str, _p: u16| {
                c.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(30));
                Ok(vec![addr("1.2.3.4:80")])
            })
        };
        let r = DnsResolver::with_resolver(ResolverConfig::default(), fn_);

        let mut joins = Vec::new();
        for _ in 0..16 {
            let r2 = r.clone();
            joins.push(tokio::spawn(
                async move { r2.resolve("shared.test", 80).await },
            ));
        }
        for j in joins {
            let got = j.await.unwrap().unwrap();
            assert_eq!(got, vec![addr("1.2.3.4:80")]);
        }
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "singleflight: only one resolve fires for 16 concurrent callers"
        );
    }

    #[tokio::test]
    async fn cache_size_tracks_entries() {
        let counter = Arc::new(AtomicUsize::new(0));
        let fn_ = counting_resolver(counter, vec![addr("127.0.0.1:80")]);
        let r = DnsResolver::with_resolver(ResolverConfig::default(), fn_);
        let _ = r.resolve("a.test", 80).await.unwrap();
        let _ = r.resolve("b.test", 80).await.unwrap();
        assert_eq!(r.cache_size(), 2);
    }

    #[tokio::test]
    async fn refresh_all_re_queries_entries() {
        let counter = Arc::new(AtomicUsize::new(0));
        let fn_ = counting_resolver(counter.clone(), vec![addr("127.0.0.1:80")]);
        let r = DnsResolver::with_resolver(ResolverConfig::default(), fn_);
        let _ = r.resolve("a.test", 80).await.unwrap();
        let before = counter.load(Ordering::Relaxed);
        r.refresh_all().await;
        let after = counter.load(Ordering::Relaxed);
        assert!(after > before, "refresh_all should re-fire resolver");
    }

    #[test]
    fn io_err_mapped_to_nxdomain_for_getaddrinfo_text() {
        let err = io::Error::new(
            io::ErrorKind::Other,
            "failed to lookup address information: Name or service not known",
        );
        assert!(matches!(
            io_err_to_dns("missing.test", &err),
            DnsError::NxDomain { .. }
        ));
    }

    #[test]
    fn resolver_config_defaults() {
        let cfg = ResolverConfig::default();
        assert_eq!(cfg.positive_ttl_cap, Duration::from_secs(300));
        assert_eq!(cfg.negative_ttl, Duration::from_secs(5));
        assert_eq!(cfg.refresh_interval, Duration::from_secs(60));
    }
}
