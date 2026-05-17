//! POSIX file-permission helpers for sensitive on-disk material
//! (SEC-2-08).
//!
//! Wave-2a delivers the helper. The call-site insertion in
//! `crates/lb/src/main.rs::{load_private_key, load_cert_chain}` is a
//! Wave-2c follow-up owned by `code` (main.rs is code-§D-assigned);
//! this module ships the API + test sweep so the wiring is two
//! one-liners.
//!
//! Policy
//! ------
//!
//! [`assert_owner_only`] inspects the POSIX permission bits of
//! `path` and:
//!
//! * In **lax** mode (`strict=false`) — warns via a returned
//!   [`KeyPermAdvice::TooPermissive`] and returns `Ok`. The caller
//!   is expected to thread the advice into a `tracing::warn!`.
//! * In **strict** mode (`strict=true`) — returns
//!   [`KeyPermError::TooPermissive`].
//!
//! "Too permissive" = any of the group / other bits are set
//! (`mode & 0o077 != 0`). The check covers regular files; symlinks
//! are resolved via `fs::metadata` (which follows them), and
//! directories are accepted unchanged because the caller does not
//! pass directory paths here.
//!
//! Non-Unix targets
//! ----------------
//!
//! On non-Unix the function is a no-op (returns
//! `Ok(KeyPermAdvice::NotApplicable)`); operators on those targets
//! enforce permissions via the platform-native ACL surface, not
//! through this helper.

use std::path::Path;

/// Result class returned in the **lax** (non-strict) mode.
///
/// Conveyed via `Ok` so the caller can keep going (load the key)
/// while still surfacing the deviation through a tracing layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPermAdvice {
    /// Permissions are tight (`mode & 0o077 == 0` on Unix; no
    /// non-owner read/write/exec bits).
    Ok,
    /// At least one group/other bit is set. The caller should
    /// `tracing::warn!`.
    TooPermissive {
        /// Observed mode bits (low 12, i.e. `mode & 0o7777`).
        mode: u32,
    },
    /// Non-Unix target — the helper performed no check.
    NotApplicable,
}

/// Error class returned in **strict** mode.
#[derive(Debug, thiserror::Error)]
pub enum KeyPermError {
    /// File exists, has loose permissions, strict mode is on.
    #[error(
        "key file {path}: mode {mode:#o} permits group/other access (strict mode); \
         chmod 0600 to fix"
    )]
    TooPermissive {
        /// Path that triggered the failure.
        path: String,
        /// Observed mode bits.
        mode: u32,
    },

    /// IO failure (file not found, no permission to stat, ...).
    #[error("key file {path}: {source}")]
    Io {
        /// Path that triggered the failure.
        path: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}

/// Inspect `path` and either pass, advise (lax mode), or fail
/// (strict mode).
///
/// # Arguments
///
/// * `path` — file to inspect. Must be a regular file. Symlinks are
///   followed.
/// * `strict` — when `true`, return [`KeyPermError::TooPermissive`]
///   on loose perms instead of [`KeyPermAdvice::TooPermissive`].
///
/// # Errors
///
/// * [`KeyPermError::TooPermissive`] — strict mode, loose perms.
/// * [`KeyPermError::Io`] — file system access failed.
pub fn assert_owner_only<P: AsRef<Path>>(
    path: P,
    strict: bool,
) -> Result<KeyPermAdvice, KeyPermError> {
    let path = path.as_ref();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).map_err(|e| KeyPermError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let mode = meta.permissions().mode() & 0o7777;
        if mode & 0o077 != 0 {
            if strict {
                return Err(KeyPermError::TooPermissive {
                    path: path.display().to_string(),
                    mode,
                });
            }
            Ok(KeyPermAdvice::TooPermissive { mode })
        } else {
            Ok(KeyPermAdvice::Ok)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, strict);
        Ok(KeyPermAdvice::NotApplicable)
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests_unix {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        // Each test gets a unique-enough name; we don't pull
        // `tempfile` for one test file.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        p.push(format!("lb-security-keyperm-{nanos}-{name}"));
        p
    }

    fn write_with_mode(path: &PathBuf, mode: u32) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(b"-----BEGIN FAKE KEY-----\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn mode_0600_passes_lax() {
        let p = temp_path("0600-lax");
        write_with_mode(&p, 0o600);
        let advice = assert_owner_only(&p, false).unwrap();
        assert_eq!(advice, KeyPermAdvice::Ok);
        fs::remove_file(&p).ok();
    }

    #[test]
    fn mode_0600_passes_strict() {
        let p = temp_path("0600-strict");
        write_with_mode(&p, 0o600);
        let advice = assert_owner_only(&p, true).unwrap();
        assert_eq!(advice, KeyPermAdvice::Ok);
        fs::remove_file(&p).ok();
    }

    #[test]
    fn mode_0644_advises_in_lax() {
        let p = temp_path("0644-lax");
        write_with_mode(&p, 0o644);
        let advice = assert_owner_only(&p, false).unwrap();
        assert!(matches!(advice, KeyPermAdvice::TooPermissive { .. }));
        fs::remove_file(&p).ok();
    }

    #[test]
    fn mode_0644_errors_in_strict() {
        let p = temp_path("0644-strict");
        write_with_mode(&p, 0o644);
        let err = assert_owner_only(&p, true).unwrap_err();
        match err {
            KeyPermError::TooPermissive { mode, .. } => {
                assert_eq!(mode & 0o7777, 0o644);
            }
            other => panic!("expected TooPermissive, got {other:?}"),
        }
        fs::remove_file(&p).ok();
    }

    #[test]
    fn mode_0640_group_read_only_still_advises() {
        // 0o040 = group read. Any non-zero in `& 0o077` triggers.
        let p = temp_path("0640-lax");
        write_with_mode(&p, 0o640);
        let advice = assert_owner_only(&p, false).unwrap();
        assert!(matches!(advice, KeyPermAdvice::TooPermissive { .. }));
        fs::remove_file(&p).ok();
    }

    #[test]
    fn mode_0700_passes() {
        // 0o700 has no non-owner bits set.
        let p = temp_path("0700-strict");
        write_with_mode(&p, 0o700);
        let advice = assert_owner_only(&p, true).unwrap();
        assert_eq!(advice, KeyPermAdvice::Ok);
        fs::remove_file(&p).ok();
    }

    #[test]
    fn missing_file_returns_io_error() {
        let p = temp_path("nonexistent");
        let err = assert_owner_only(&p, false).unwrap_err();
        assert!(matches!(err, KeyPermError::Io { .. }));
    }
}
