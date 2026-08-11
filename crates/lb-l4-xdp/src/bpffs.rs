//! ROUND8-L4-11: bpffs runtime check (Linux-only).
//!
//! `mount -t bpf bpf /sys/fs/bpf/` is a prerequisite for pinning; without it aya pins into a
//! regular tmpfs and the kernel rejects it deep in `bpf(BPF_OBJ_GET)` with an opaque EINVAL. An
//! explicit `statfs(2)` before handing the path to aya turns that into an actionable error.

#![cfg(target_os = "linux")]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::loader::XdpLoaderError;

/// Kernel `BPF_FS_MAGIC` (`include/uapi/linux/magic.h`). Stable kernel ABI that libc does not
/// export, so it is redeclared next to its use site.
pub const BPF_FS_MAGIC: i64 = 0xCAFE_4A11;

/// Verify that `path` resolves to a directory backed by bpffs, returning a typed error that carries
/// the path, the magic the kernel reported, and the remediation command.
pub fn assert_bpffs(path: &Path) -> Result<(), XdpLoaderError> {
    // libc::statfs needs a NUL-terminated C string; an interior NUL is a hard caller error.
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|e| {
        XdpLoaderError::PinPathStatFailed {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
        }
    })?;

    // SAFETY: `statfs(2)` is async-signal-safe; the buffer is owned
    // and large enough; the path pointer is valid for the duration
    // of the call.
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
    if rc != 0 {
        return Err(XdpLoaderError::PinPathStatFailed {
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }

    // `f_type` is `c_long` on 64-bit glibc — cast through the native width so the comparison holds
    // regardless of how libc exposes the field per architecture.
    #[allow(
        clippy::unnecessary_cast,
        clippy::cast_lossless,
        clippy::useless_conversion,
        reason = "buf.f_type type varies across libc versions / arches"
    )]
    let fs_type_wide: i64 = buf.f_type as i64;

    if fs_type_wide != BPF_FS_MAGIC {
        return Err(XdpLoaderError::PinPathNotBpffs {
            path: path.to_path_buf(),
            found_magic: fs_type_wide,
            hint: "mount bpffs: `mount -t bpf bpffs /sys/fs/bpf` (or declare \
                   RequiresMountsFor=/sys/fs/bpf in the systemd unit so the \
                   service does not start before the mount is ready)"
                .to_owned(),
        });
    }

    Ok(())
}

/// Convenience: the production default pin directory.
#[must_use]
pub fn default_pin_dir() -> PathBuf {
    PathBuf::from(crate::loader::DEFAULT_PIN_DIR)
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_bpffs_tempdir() {
        let tmp = std::env::temp_dir();
        // Some CI sandboxes block statfs; the contract under test is only no-false-positive-Ok, so
        // skip when statfs itself fails.
        let result = assert_bpffs(&tmp);
        match result {
            Err(XdpLoaderError::PinPathNotBpffs {
                hint, found_magic, ..
            }) => {
                assert_ne!(found_magic, BPF_FS_MAGIC);
                assert!(
                    hint.contains("mount -t bpf"),
                    "hint must surface the mount command, got: {hint}"
                );
            }
            Err(XdpLoaderError::PinPathStatFailed { .. }) => {
                // Acceptable if the sandbox blocks statfs; the contract under test is "no
                // false-positive Ok on non-bpffs".
            }
            Ok(()) => panic!("tempdir cannot be bpffs but assert_bpffs returned Ok"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_path() {
        let path = Path::new("/nonexistent/bpf/expressgateway-test");
        match assert_bpffs(path) {
            Err(XdpLoaderError::PinPathStatFailed { source, path: p }) => {
                assert_eq!(p, path);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected PinPathStatFailed(NotFound), got {other:?}"),
        }
    }

    #[test]
    fn rejects_interior_nul_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let bad = PathBuf::from(OsString::from_vec(b"/tmp/\0nul".to_vec()));
        assert!(matches!(
            assert_bpffs(&bad),
            Err(XdpLoaderError::PinPathStatFailed { .. })
        ));
    }

    #[test]
    fn bpf_fs_magic_constant_is_kernel_value() {
        assert_eq!(BPF_FS_MAGIC, 0xCAFE_4A11);
    }
}
