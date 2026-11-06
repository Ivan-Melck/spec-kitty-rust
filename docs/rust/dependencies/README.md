# Locked Rust dependency inventory / file-identity decision

`cargo-lock.sbom.json` is a portable software bill of materials derived from ALL
**164** Cargo.lock package rows and dependency edges, including workspace rows.
It embeds the Cargo.lock SHA-256 and preserves registry checksums/versions. It is
not a release build receipt, vulnerability certification or an SPDX/CycloneDX
conformance claim. Cargo.lock remains the resolution authority.

## Coordinator-approved identity addition

Why: the artifact owner's Windows `MetadataExt::volume_serial_number/file_index`
methods are `#[unstable(feature="windows_by_handle")]` in pinned Rust
58cf8f9c3. The owner's size/mtime fallback and `unwrap_or(0)` also cannot establish
file ownership. `same-file::Handle` supplies a safe held-file comparison across
Unix/Windows without unsafe workspace code. Existing fs2 is an advisory-lock
primitive, not an identity API. The actual captured Git index lock path is still
the sole lock authority. No new Python/Source/pack pin or global registry config.

Minimal closure change (no other version upgraded):

| Package | Registry publish timestamp | SHA-256 |
| --- | --- | --- |
| same-file1.0.6, exact direct pin | 2020-01-11T17:02:30Z | 93fc1dc3aaa9bfed95e02e6eadabb4baf7e3078b0bd1b4d7b6b0b68378900502 |
| winapi-util0.1.11, Windows-only locked transitive | 2025-09-07T12:24:31Z | c2a7b1c03c876122aa43f3020e6c3c3ee5c05081c9a00739faf7503aeba10d22 |

Evidence root `/tmp/sk-commit-artifact-integration.96Bf02/command-log-index.tsv`:

1. **Registry:** official crates.io downloads + index.crates.io sparse rows;
   downloaded archive, registry and Cargo.lock checksums independently equal.
   Both versions not yanked. Initial crates.io metadata API403 is retained in
   `file-identity-package-source.NqbbBo.log`; official sparse metadata succeeded,
   no mirror/unreviewed archive substituted.
2. **Freshness:** exact version publication timestamps above, neither newly
   published. Sparse data identifies the immutable version record, not a mutable
   republish assumption. No latest-version upgrade was attempted.
3. **Install scripts / license:** downloaded package manifests and sources read;
   no build.rs in same-file, winapi-util has `build=false`. No new lifecycle
   scripts executed/allowlisted. Both dual Unlicense/MIT: MIT is acceptable for
   this MIT workspace. Windows safe wrapper uses existing locked windows-sys.
4. **Reproducibility:** Cargo resolution changed only the two new package rows
   and sk-git's edge; all later builds/tests use `--locked`, and offline check
   passes. Full lock-derived inventory regenerated; no handwritten lock edits.
5. **Incident posture:** OSV API (RustSec-advisory aggregator) exact-version
   queries on2026-11-04 returned `{}` for BOTH packages; retained JSON and
   `dependency-incident-feed.RK1qXC.log`. Inspected sources/manifests have no
   install-time network/credential/persistence behavior. This limited check is
   not a whole-dependency audit or a guarantee against undisclosed issues.

The dependency source documents Windows file-ID collision limitations on unusual
filesystems; keep both identity handles alive and do not claim real Windows/ReFS
certification. No Windows target was installed/run. Linux controls do not waive
platform acceptance. Unknown/failed identity is never fabricated as zero.

## Reproduce the portable inventory

Run from the repository with Python3.11 (verification attempts must still use the
unique evidence wrapper):

```python
import hashlib, json, pathlib, tomllib
lock = pathlib.Path("Cargo.lock").read_bytes()
data = {
    "schema_version": 1,
    "inventory_kind": "Cargo.lock software bill of materials",
    "source": "Cargo.lock",
    "source_sha256": hashlib.sha256(lock).hexdigest(),
    "packages": tomllib.loads(lock.decode())["package"],
}
pathlib.Path("docs/rust/dependencies/cargo-lock.sbom.json").write_text(
    json.dumps(data, indent=2, sort_keys=True) + "\n"
)
```

Canonical parsed package rows and lock digest must match before publication;
absolute checkout/cache paths are deliberately absent.
