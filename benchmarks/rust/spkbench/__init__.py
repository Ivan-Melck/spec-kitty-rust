"""Spec Kitty Rust-vs-Python benchmark harness (development tooling only).

This package is *not* shipped, imported, or invoked by the Rust binary. It
exists to measure the native rewrite against the pinned Python reference with
reproducible fixtures, output-equivalence assertions, and honest confidence
metadata.

Run ``python3 benchmarks/rust/bench.py --help``.
"""

__all__ = ["__version__"]

__version__ = "1.0.0"
