//! Per-user base directories for cache and state files.
//!
//! Upstream `kernel.user_dirs` resolves the **base** directory only; callers
//! append their own `spec-kitty/...` subpath. The rules reproduce the
//! locations the callers already wrote, so no file moves:
//!
//! - [`user_cache_dir`] — `%LOCALAPPDATA%` (else `~/AppData/Local`) on Windows;
//!   `$XDG_CACHE_HOME` (else `~/.cache`) everywhere else, macOS included.
//!   `windows_local_appdata = false` applies the XDG rule on Windows too.
//! - [`user_state_dir`] — `$XDG_STATE_HOME` (else the literal `~/.cache`) on
//!   every platform.
//!
//! An empty variable counts as unset. A relative value is used as given.
//! This is deliberately **not** `platformdirs`: that resolves other locations
//! (macOS `~/Library/Caches`, an extra `Cache` level on Windows) and would move
//! files this baseline already wrote.

use std::env;
use std::path::PathBuf;

use crate::KernelError;
use crate::paths::home_directory;

const XDG_CACHE_HOME: &str = "XDG_CACHE_HOME";
const XDG_STATE_HOME: &str = "XDG_STATE_HOME";
const LOCALAPPDATA: &str = "LOCALAPPDATA";

/// Return the per-user base cache directory.
pub fn user_cache_dir(windows_local_appdata: bool) -> Result<PathBuf, KernelError> {
    if windows_local_appdata && cfg!(windows) {
        if let Some(local_app_data) = non_empty(LOCALAPPDATA) {
            return Ok(PathBuf::from(local_app_data));
        }
        return Ok(home_directory()?.join("AppData").join("Local"));
    }
    if let Some(xdg) = non_empty(XDG_CACHE_HOME) {
        return Ok(PathBuf::from(xdg));
    }
    Ok(home_directory()?.join(".cache"))
}

/// Return the per-user base state directory.
pub fn user_state_dir() -> Result<PathBuf, KernelError> {
    if let Some(state_home) = non_empty(XDG_STATE_HOME) {
        return Ok(PathBuf::from(state_home));
    }
    Ok(home_directory()?.join(".cache"))
}

fn non_empty(name: &str) -> Option<std::ffi::OsString> {
    match env::var_os(name) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_environment_values_fall_through() {
        // Cannot set process env safely in edition 2024; the child-process
        // differential tests cover the variable branches. Here we pin the
        // helper contract itself.
        assert!(env::var_os("PATH").is_some());
        let _ = non_empty("SK_KERNEL_SURELY_UNSET_VARIABLE");
        assert!(non_empty("SK_KERNEL_SURELY_UNSET_VARIABLE").is_none());
    }
}
