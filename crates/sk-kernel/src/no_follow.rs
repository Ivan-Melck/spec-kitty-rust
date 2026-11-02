use std::fs::OpenOptions;
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::KernelError;

/// Open a path without following a final-component symbolic link.
///
/// On Unix the kernel enforces this in the open syscall (`O_NOFOLLOW`), so the
/// check-then-open race is closed. Other platforms perform a best-effort
/// pre-open `symlink_metadata` check; the residual race is documented rather
/// than hidden behind a silent weaker open.
///
/// The caller supplies its own [`OpenOptions`] (read/write/create/append/mode),
/// matching the upstream `open_no_follow(path, flags, mode)` contract. This
/// module owns the no-follow flag so callers cannot accidentally omit it.
pub fn open_no_follow(
    path: &Path,
    options: &mut OpenOptions,
) -> Result<std::fs::File, KernelError> {
    open_no_follow_with_flags(path, options, NoFollowFlags::default())
}

/// Extra Unix open flags that must travel with the no-follow flag.
///
/// `OpenOptionsExt::custom_flags` replaces the whole custom flag set, so the
/// no-follow door owns the combination rather than letting callers silently
/// drop `O_NOFOLLOW` (or the nonblocking read guard) when they add a flag.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoFollowFlags {
    pub(crate) nonblocking: bool,
}

pub(crate) fn open_no_follow_with_flags(
    path: &Path,
    options: &mut OpenOptions,
    flags: NoFollowFlags,
) -> Result<std::fs::File, KernelError> {
    #[cfg(unix)]
    {
        use rustix::fs::OFlags;
        // NOCTTY keeps an untrusted terminal node from becoming our
        // controlling tty; O_NOFOLLOW is the contract of this function.
        let mut custom = OFlags::NOFOLLOW | OFlags::NOCTTY;
        if flags.nonblocking {
            custom |= OFlags::NONBLOCK;
        }
        options.custom_flags(custom.bits() as i32);
    }
    #[cfg(not(unix))]
    {
        let _ = flags;
    }

    #[cfg(not(unix))]
    {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(KernelError::NoFollow {
                    path: path.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(KernelError::io("inspect", path, source)),
        }
    }

    match options.open(path) {
        Ok(file) => Ok(file),
        Err(source) if is_no_follow_error(&source) => Err(KernelError::NoFollow {
            path: path.to_path_buf(),
        }),
        Err(source) => Err(KernelError::io("open", path, source)),
    }
}

/// Read a regular file without following a final symlink, applying the same
/// bounded, nonblocking-on-Unix policy as [`crate::read_bounded`].
pub fn read_no_follow(path: &Path, max_bytes: u64) -> Result<Vec<u8>, KernelError> {
    let mut options = OpenOptions::new();
    options.read(true);
    let file = open_no_follow_with_flags(path, &mut options, NoFollowFlags { nonblocking: true })?;
    crate::bounded::read_open_file(file, path, max_bytes)
}

fn is_no_follow_error(source: &io::Error) -> bool {
    #[cfg(unix)]
    {
        // ELOOP is what Linux/macOS report for O_NOFOLLOW on a symlink.
        source.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error())
    }
    #[cfg(not(unix))]
    {
        let _ = source;
        false
    }
}

/// Read a UTF-8 text file without following a final symlink.
///
/// `max_bytes` bounds the read exactly like [`read_no_follow`]; invalid UTF-8
/// is [`KernelError::InvalidUtf8`], never lossy replacement.
pub fn read_text_no_follow(path: &Path, max_bytes: u64) -> Result<String, KernelError> {
    let bytes = read_no_follow(path, max_bytes)?;
    String::from_utf8(bytes).map_err(|_| KernelError::InvalidUtf8 {
        path: path.to_path_buf(),
    })
}

/// Write bytes to a regular file without following a final symlink.
///
/// The file is created when missing and truncated when present, matching
/// upstream `write_text_no_follow`'s open flags. This is a direct write, not
/// the atomic replacement helper; callers that need replacement use
/// [`crate::atomic_write`]. A planted symlink is [`KernelError::NoFollow`].
pub fn write_no_follow(path: &Path, bytes: &[u8], mode: u32) -> Result<(), KernelError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    let mut file = open_no_follow(path, &mut options)?;
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|source| KernelError::io("write", path, source))?;
    file.sync_data()
        .map_err(|source| KernelError::io("sync", path, source))
}

/// Write UTF-8 text without following a final symlink.
pub fn write_text_no_follow(path: &Path, text: &str, mode: u32) -> Result<(), KernelError> {
    write_no_follow(path, text.as_bytes(), mode)
}

/// Apply permission bits to an open descriptor.
///
/// Windows has no `fchmod`; there the path-based fallback follows a symlink,
/// which is the same fallback upstream uses.
#[cfg(unix)]
pub fn chmod_fd(file: &std::fs::File, path: &Path, mode: u32) -> Result<(), KernelError> {
    use rustix::fs::{Mode, fchmod};
    fchmod(file, Mode::from_bits_truncate(mode))
        .map_err(|source| KernelError::io("chmod", path, std::io::Error::from(source)))
}

/// Apply permission bits to an open descriptor (path fallback without fchmod).
#[cfg(not(unix))]
pub fn chmod_fd(_file: &std::fs::File, path: &Path, mode: u32) -> Result<(), KernelError> {
    chmod_no_follow(path, mode)
}

/// Apply permission bits, using a no-follow form where the host supports it.
///
/// Linux has no `lchmod`, so this falls back to a default-follow chmod exactly
/// as upstream does; for a regular file the two are identical.
pub fn chmod_no_follow(path: &Path, mode: u32) -> Result<(), KernelError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|source| KernelError::io("chmod", path, source))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(path)
            .map_err(|source| KernelError::io("inspect for chmod", path, source))?
            .permissions();
        permissions.set_readonly(mode & 0o200 == 0);
        std::fs::set_permissions(path, permissions)
            .map_err(|source| KernelError::io("chmod", path, source))
    }
}

/// Apply atime/mtime nanoseconds, preferring `AT_SYMLINK_NOFOLLOW` where the
/// host supports it.
///
/// UNIX: `utimensat(..., SYMLINK_NOFOLLOW)`; if the platform reports
/// `EOPNOTSUPP`/`EINVAL` the call falls back to the default-follow form, which
/// is what upstream `utime_no_follow` does on hosts without the flag.
#[cfg(unix)]
pub fn utime_no_follow(path: &Path, atime_ns: i64, mtime_ns: i64) -> Result<(), KernelError> {
    use rustix::fs::{AtFlags, CWD, Timespec, utimensat};
    let times = rustix::fs::Timestamps {
        last_access: Timespec {
            tv_sec: atime_ns.div_euclid(1_000_000_000),
            tv_nsec: atime_ns.rem_euclid(1_000_000_000) as _,
        },
        last_modification: Timespec {
            tv_sec: mtime_ns.div_euclid(1_000_000_000),
            tv_nsec: mtime_ns.rem_euclid(1_000_000_000) as _,
        },
    };
    match utimensat(CWD, path, &times, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::INVAL) => {
            utimensat(CWD, path, &times, AtFlags::empty())
                .map_err(|source| KernelError::io("utime", path, std::io::Error::from(source)))
        }
        Err(source) => Err(KernelError::io("utime", path, std::io::Error::from(source))),
    }
}

/// Non-Unix fallback: the standard library cannot express no-follow times.
#[cfg(not(unix))]
pub fn utime_no_follow(path: &Path, _atime_ns: i64, _mtime_ns: i64) -> Result<(), KernelError> {
    // Windows cannot set times through a safe no-follow API; report the
    // platform limit rather than silently following a link.
    Err(KernelError::NoFollow {
        path: path.to_path_buf(),
    })
}

/// Whether fd-relative directory operations (`O_DIRECTORY` + `dir_fd`) are
/// available here.
///
/// Windows lacks both, so callers there must use the path-based containment
/// dance instead. This is the single capability check upstream exposes.
pub const fn fd_relative_dir_ops_supported() -> bool {
    cfg!(unix)
}

/// Read the mode bits of an existing file (Unix) or a portable approximation.
#[cfg(unix)]
pub fn file_mode(path: &Path) -> Result<u32, KernelError> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.mode() & 0o7777)
        .map_err(|source| KernelError::io("inspect mode of", path, source))
}

/// Read the mode bits of an existing file (non-Unix approximation).
#[cfg(not(unix))]
pub fn file_mode(path: &Path) -> Result<u32, KernelError> {
    let metadata = std::fs::metadata(path)
        .map_err(|source| KernelError::io("inspect mode of", path, source))?;
    Ok(if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    })
}
