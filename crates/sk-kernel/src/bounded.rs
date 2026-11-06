use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use crate::KernelError;

/// Source `Path.read_bytes()` byte-stream policy: follow the path, read until
/// EOF without an invented size bound or regular-file precondition.
///
/// Unlike [`read_bounded`], this explicitly Source-compatible operation MAY
/// block opening/reading a FIFO or device, just like the reference. Use the
/// bounded regular-file capability for inspection/security-constrained work;
/// do not impose that separate policy on compatible Source execution.
pub fn read_source_bytes(path: &Path) -> Result<Vec<u8>, KernelError> {
    std::fs::read(path).map_err(|source| KernelError::io("read", path, source))
}

/// Read at most `max_bytes` from `path`.
///
/// The file is rejected with [`KernelError::TooLarge`] when it is larger than
/// the limit. Both the file metadata and an `max_bytes + 1` read are checked,
/// so a file that grows concurrently cannot slip past the bound. A limit of
/// zero accepts only an empty file.
pub fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, KernelError> {
    let mut options = OpenOptions::new();
    options.read(true);
    // Opening a FIFO can block before metadata is available. Nonblocking open
    // closes that denial-of-service path even if the entry changes concurrently.
    // NOCTTY prevents an untrusted terminal node becoming our controlling tty.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOCTTY;
        options.custom_flags(flags.bits() as i32);
    }
    let file = options
        .open(path)
        .map_err(|source| KernelError::io("open", path, source))?;
    read_open_file(file, path, max_bytes)
}

/// Read a bounded regular file from an already-open descriptor.
///
/// Shared by [`read_bounded`] and the no-follow reader so both enforce exactly
/// the same regular-file, metadata-length, and `max_bytes + 1` policy.
pub(crate) fn read_open_file(
    file: File,
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, KernelError> {
    let metadata = file
        .metadata()
        .map_err(|source| KernelError::io("inspect", path, source))?;

    if !metadata.is_file() {
        return Err(KernelError::io(
            "read",
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "path is not a regular file"),
        ));
    }

    if metadata.len() > max_bytes {
        return Err(KernelError::TooLarge {
            path: path.to_path_buf(),
            limit: max_bytes,
        });
    }

    let mut bytes = Vec::new();
    let read_limit = max_bytes.saturating_add(1);
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|source| KernelError::io("read", path, source))?;

    if bytes.len() as u64 > max_bytes {
        return Err(KernelError::TooLarge {
            path: path.to_path_buf(),
            limit: max_bytes,
        });
    }

    Ok(bytes)
}
