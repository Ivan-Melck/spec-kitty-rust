//! Compatibility adapter: CPython set hashing/order now lives in the Kernel floor.
//!
//! A YAML `!!set` constructs a Python `set`, whose iteration order is a function
//! of CPython's SipHash secret and its open-addressing table. That algorithm is
//! shared by both Source dialects, so it lives in `sk_kernel::python_yaml`; this
//! module keeps the three names `sk-doctrine` exports (the process fatal and its
//! validator) so no consumer changes.

pub use sk_kernel::python_yaml::{HASH_SEED_FATAL, HashSeedError, validate_hash_seed_from_env};
