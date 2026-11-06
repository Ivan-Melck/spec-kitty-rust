//! Thin same-item re-exports of the Kernel filesystem codec authority.
//!
//! The actual startup context, codec mappings, native locale resolution and
//! exact byte decoding live in `sk_kernel::python_os_encoding` (promoted from
//! this crate with its notices and algorithms intact). This module deliberately
//! contains no second codec, resolver, context or decoder; it keeps the
//! existing `crate::python_os_encoding` call sites and public re-exports on the
//! same nominal types.

pub use sk_kernel::python_os_encoding::{
    LocaleCodeset, OsEncodeError, OsEncodingContext, latin9_byte, locale_codeset, locale_request,
    utf8_mode_setting,
};
