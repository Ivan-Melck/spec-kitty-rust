//! Thin same-item re-exports of the Kernel native-locale authority.
//!
//! The real glibc `LC_CTYPE` reader, alias handling, uncertainty model and
//! codeset normalization live in `sk_kernel::os_locale` (promoted from this
//! crate with its observations intact). This module adds no second locale
//! reader, environment policy or charset guess.

pub use sk_kernel::os_locale::{LocaleFact, canonical_codeset, resolve};
