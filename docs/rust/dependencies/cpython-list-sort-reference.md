# CPython list-sort algorithm reference — bounded custody

The uncommitted CF60 safe-Rust sequence sorter in
`crates/sk-kernel/src/python_json/sort.rs` references CPython 3.11.15's
`Objects/listobject.c` and `Objects/listsort.txt`, not an adopted C library.

## Exact reference

- Upstream: https://github.com/python/cpython/tree/2340a037f7450e70fccfe411e6531afb4d57a312
- Tag: `v3.11.15`.
- `Objects/listobject.c`: SHA256 `ae3b2ad9b993af5666c5e007319ba896d92dc6bcc5c1e4a1af7f97a7558001a2`.
- `Objects/listsort.txt`: SHA256 `5cab6ea08998ceec3a3e6a7c415b04a67f2e1c75d43a2a38bc3f361854dc4ee0`.
- Full upstream license retained verbatim in
  [`cpython-3.11.15-reference-LICENSE.txt`](cpython-3.11.15-reference-LICENSE.txt),
  SHA256 `3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf`.
- Powersort attribution: J. Ian Munro and Sebastian Wild,
  *Nearly-Optimal Mergesorts: Fast, Practical Sorting Methods That Optimally Adapt
  to Existing Runs*, ESA 2018. CPython's source and listsort discussion are the
  implementation reference; no separate paper code is incorporated.

## Change summary / safety boundary

The work is a Rust adaptation of ascending stable natural-run sorting, minimum
run/binary insertion, power-based run scheduling, galloping and low/high merges.
It sorts an index permutation using existing exact Python-value comparisons and
cached keys, then applies the permutation to the original items. It does not
compile or call CPython C, expose C pointers, add unsafe/FFI, import a parser,
install a package, or change manifests/locks. Rust's memory ownership and index
operations replace C pointer/storage operations. Error-state mutation and reverse
sorting are not claimed equivalent to Python `list.sort`.

## Disposition

License/copyright and change-summary custody is retained for this WIP reference.
This is not legal advice, independent safety approval, complete SBOM/release
approval, or a performance result. The parent read the complete upstream license;
source/notice bytes were checked in receipts `.Rwd1Dj` (license copied; subsequent
incorrect source subpaths failed) and `.Y7TT6a` (actual paths bound), under
`/tmp/sk-vertical-integration.TwDlpw`. Corrected receipts do not erase the failed
attempt. Before publication, the ordinary dependency/reference and independent
release gates still apply.
