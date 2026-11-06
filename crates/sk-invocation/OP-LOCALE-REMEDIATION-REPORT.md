# Op filesystem-encoding locale remediation — implementation report

Status: **historical remediation-owner snapshot**, 219 scoped tests in the
UNPUBLISHED dirty a6a tree (not a published-parent baseline). Current integration,
small coordinator follow-up, hashes/counts/limits: `OP-BYTE-BRIDGE-INTEGRATION-REPORT.md`.
Not an acceptance verdict, not whole-Op/CLI/J-P/platform/publication approval,
and not a waiver for the native gaps listed under *Limits*.

Evidence root: `/tmp/sk-op-locale-remediation.DyCscC` — one fresh `mktemp` log
per attempt, including failures, indexed in `command-log-index.tsv`. Base
commit `a6a2c0cf100c178a936b2d3c2974b1f22658a53b` (12 dirty paths from the
completed bridge owner plus this remediation, none committed here). Source
oracle pinned `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython 3.11.15,
exact 53 runtime requirements re-verified at the final tree.

## Defect class closed

The encoder decided the filesystem charset by *guessing from the locale-name
suffix* and also guessed availability. Valid source settings therefore either
committed with a guessed codec or were refused by the native adapter. The
coordinator's pre-remediation 0 pass / 4 fail log is preserved at
`/tmp/sk-op-byte-coordinator.JH8uPD/locale-existing-service-red.RGR7r4.log`.

The fix replaces that guess with one read-only native locale-fact authority
and an actual ISO-8859-1 codec:

* **`src/os_locale.rs` (new, private, 723 lines)** — the only native authority
  for locale availability and the actual `LC_CTYPE` codeset. On Linux/glibc it
  expands `locale.alias`, searches `LOCPATH` then `/usr/lib/locale`, tries the
  literal name, the normalized codeset spelling (`en_US.UTF-8` →
  `en_US.utf8`), the code-set-stripped and modifier-stripped fallbacks in
  libc's order, and accepts a base locale only when its real codeset equals
  the request (normalized name or documented charmap alias). It parses the
  glibc `LC_CTYPE` header (native/byte-swapped magic `0x20090720`, string
  count, `u32` offsets from byte 8, slot 14
  `_NL_CTYPE_CODESET_NAME`). Results are `C`, `Resolved{codeset}`,
  `Unavailable`, or `Uncertain`. Unreadable data, unsupported format,
  `locale-archive`, path-shaped names and undecidable codeset relations are
  `Uncertain` (operational refusal), never a C-locale guess.
* **`src/python_os_encoding.rs`** — startup policy now uses those facts;
  `PYTHONUTF8=1` is applied before any locale read (an uninterpretable
  irrelevant locale cannot pre-empt it), an actual failed lookup is the C
  state (PEP 540 auto UTF-8 when the mode is unset), mode 0 respects
  `LC_ALL`/`PYTHONCOERCECLOCALE` and chooses only an installed
  `C.UTF-8`/`C.utf8`/`UTF-8` coercion target. New strict ISO-8859-1 codec:
  `U+0000..=U+00FF` → its one byte, `\udc80..\udcff` → raw `80..ff`,
  `>U+00FF` → truthful `Unencodable`; no Latin-1 reinterpretation elsewhere.
  A bounded, named list of real ASCII-transparent legacy charsets (EUC-JP,
  EUC-KR, Shift_JIS, Big5, GBK/GB18030, KOI8-*, TIS-620, CP125x, ISO-2022-*,
  …) keeps ASCII and escaped bytes publishable while non-ASCII needs the
  unimplemented codec; an unknown charset refuses the whole message.
* **`src/lib.rs`** — private `mod os_locale;` and the crate-doc correction.
  No other production file changed.

No environment mutation, no libc call, no runtime Python in production, no
unsafe, no new dependency, no global state.

## Four existing-service reds now green (real service, real Git source oracle)

`locale-reds-green-2.sonsDJ.log` (7 filtered locale tests, 0 failed). The four
coordinator cases are still full source/native commit pairs; on this host the
Latin-1 branches really execute (the measured source codec is printed for each
child and forwarded into the log):

| Case | Measured source | Native after fix |
| --- | --- | --- |
| `LC_ALL=xx_YY.BOGUS-8` | `utf-8` | actual C fallback → auto UTF-8; commits `é` = `C3 A9`; equal object/reflog |
| `LC_ALL=xx_YY.UTF-8 PYTHONUTF8=0 PYTHONCOERCECLOCALE=0` | `ascii` | actual C + mode 0 → ASCII; refuses `U+00E9`, no commit, tip unchanged (source also commits nothing) |
| `LC_ALL=en_US` | `iso8859-1` | actual locale file `ISO-8859-1`; ASCII payload commits; full object equality |
| `LC_ALL=en_US.ISO-8859-1` | `iso8859-1` | base `en_US` actual Latin-1 + codeset match; reflog `E9 80`, object body `C3 A9 C2 80`; full equality |

The four tests were made portable **without skipping a business call**: they
always run the pinned source commit and the real native service, require the
same committed/refused verdict, and apply byte/OID assertions when committed.
On a host without those locales both sides take the same C fallback and the
pair must still agree; on this host the Latin-1 byte assertions are real.

Additional real controls added:

* `utf8_mode_override_wins_before_an_unavailable_locale` — `PYTHONUTF8=1`
  commits UTF-8 `é`/raw `80` under a bogus locale.
* `unavailable_locale_coercion_matches_the_installed_target` — mode 0 with
  `LC_ALL` unset: source and native coerce to the installed `C.UTF-8`
  (`utf-8` here) for the scalar, and the ASCII/escaped row always commits.
* `installed_latin1_locale_matches_real_payload_and_reflog_bytes` — see above.
* `ascii_transparent_locale_keeps_ascii_and_escape_payloads` — actual
  `ja_JP.EUC-JP` source codec `euc_jp`: ASCII and raw escaped bytes commit with
  equal bytes; a non-ASCII scalar is a source commit but a truthful native
  codec-gap refusal naming `EUC-JP`/`U+65E5` (not “invalid record”). On a host
  without the locale both sides use the C fallback and must agree.
* `locpath_fixture_resolves_real_ctype_files_like_the_source` — copies this
  host's actual `C.utf8` `LC_CTYPE` into a temporary `LOCPATH` tree as
  `zz_ZZ`, then `zz_ZZ.UTF-8` resolves through `LOCPATH`/normalized matching
  and commits like the source.
* `locale_alias_resolves_to_the_installed_codeset` — `LC_ALL=german` (this
  host's `locale.alias` → `de_DE.ISO-8859-1`) resolves to the installed
  `de_DE` Latin-1 data and commits `E9`; a host without the alias takes the
  same C/ASCII fallback on both sides.

Focused unit controls in `python_os_encoding` cover the policy seams, and
`os_locale` unit tests cover both header byte orders, malformed/truncated data,
candidate order, the observed charmap alias set (accepted and rejected
spellings), bounded/case-insensitive alias expansion, actual-file `LOCPATH`
resolution, C/POSIX, and path-shaped uncertainty.

## Measured native facts bound by this change (oracle probes)

* `/usr/lib/locale` has individual `LC_CTYPE` files and **no**
  `locale-archive` on this host; `en_US/LC_CTYPE` is a symlink into `aa_DJ`.
* Actual codesets read from the files: `C.utf8`→`UTF-8`, `en_US`→`ISO-8859-1`,
  `en_US.utf8`→`UTF-8`, `en_US.iso885915`→`ISO-8859-15`,
  `ja_JP.eucjp`→`EUC-JP`.
* glibc name behaviour: `en_US.UTF-8`/`UTF8` → `en_US.utf8`; the observed
  ISO-8859-1 aliases (`ISO8859-1`, `ISO88591`, `ISO_8859-1`,
  `ISO_8859-1:1987`, `LATIN1`, `L1`, `CP819`, `IBM819`, `ISO-IR-100`,
  `CSISOLATIN1`) resolve to the base Latin-1 data; `ISO8859_1`, `LATIN-1`,
  `819`, `8859-1`, `BOGUS-8`, `ISO-8859-2` do not. `LOCPATH` precedes the
  default root (which is still consulted); the `LOCALE_ALIAS_PATH` override is
  **not** honoured by this glibc, so the code reads the fixed
  `/usr/local/share/locale` then `/usr/share/locale` alias files.
* Startup matrix reconfirmed: `PYTHONUTF8=1` first; bogus/unavailable locale +
  mode unset → auto UTF-8; mode 0 with `LC_ALL` (or coercion disabled) →
  ASCII; mode 0 with `LC_ALL` unset → installed `C.UTF-8`; empty env behaves
  like the C locale; `en_US` Latin-1 maps `é`→`E9`, `\udc80`→`80`,
  `U+0100`→`UnicodeEncodeError`; EUC-JP keeps ASCII/escape bytes.
* No `locale-archive`/non-Linux verdict is fabricated from this host.

## Verification at the final tree (scoped `sk-invocation`, 2 jobs)

Every command has a unique log; every whole Cargo command went through
`/tmp/sk-focused-cargo.sh` (whole-workspace flock, existing target,
`CARGO_INCREMENTAL=0`, isolated `HOME`/XDG/TMP); Python probes use a fresh
temp CWD plus isolated roots and the pinned oracle.

* `all-tests-locale-final.pHFBea.log` — **218 passed / 0 failed**, 2
  subprocess-helper tests ignored by design. Per target: lib 73 (was 58),
  lock_contention 4, op_admission_commit 46 (+1 ignored), **op_byte_message 18
  (was 9 at the published parent; the 4 coordinator reds plus 5 new controls)**,
  op_commit_policy 30 (+1 ignored), op_custody_roots 15, oracle_vectors 4,
  reference_path_oracle 2, writer_filesystem 26.
* `doctests-locale-final.ktf6ip.log` — 1 passed. Total **219 pass / 0 fail**
  (published-parent scoped baseline was 194 + 1 doctest; +15 focused unit
  tests, +9 focused integration tests; no pre-existing test was deleted,
  skipped or weakened to pass).
* `clippy-locale-final.840zaG.log` — strict
  `clippy -p sk-invocation --all-targets -- -D warnings` clean.
* `fmt-check-4.hTqxgs.log` — `cargo fmt -p sk-invocation -- --check` clean.
* `check-locale-final-2.RS9qAc.log` — `cargo check -p sk-invocation
  --all-targets --locked --offline` clean.
* `oracle-locale-final.sJLGWk.log` — exact 53 runtime requirements,
  `ok: true`, Python 3.11.15, no missing/drift/duplicates.

Implementation first failures (non-product, fixed; not counted as product
reds): `check-locale-1.qJWe04.log` (unused test import + a Rust 2024 temporary
lifetime), `unit-lib-1.VNon06.log` (a wrong alias-cycle test expectation),
`clippy-locale-1.J8MqzE.log` (`collapsible_if`), `fmt-check-1.WaKERI.log`
(rustfmt layout). The product red baseline itself was the coordinator's log
above, preserved untouched.

Source hashes at this tree (sha256):
`os_locale.rs 4c959fe41a1f4604ad5be3f984f821fbed2adf317c1ccf03a054d07bf5e12b3b`,
`python_os_encoding.rs 395e407f3c2f692e75c0a43077aaf6f80705cc5964375ba24d0edb60c6fb96b1`,
`lib.rs e1c0ade55dc37a004c8999ada2bef34d78d28a42ee240338ce9a9325262267bc`,
`tests/op_byte_message.rs 64957119510c904e280060609a53618c0b6c40c1b4ef2b3bf391a7f9bcf4999d`.
The report hash is recorded by the final hash log in the evidence root.

No `sk-git`, kernel, CLI, manifest, lock, fixture, golden, doctrine,
inventory, dispatcher, or owner-report change. The completed bridge work in
the dirty tree (byte payload, guard order, captured index/candidate/evidence,
spine/archive/skip behaviour) is preserved.

## Limits (not waived, not source verdicts)

* `locale-archive` is not read. A name present only in an archive is
  `Uncertain` and refuses; on an archive-only host the four pair tests would
  fail because the native side refuses while the source resolves. Implementing
  archive reading is required later native work.
* Non-glibc Unix returns `Uncertain` for any non-C locale (no data format read)
  and non-Unix keeps the pre-existing UTF-8 projection (a documented,
  unverified gap). No Windows/macOS certification.
* Path-shaped locale names (absolute or `..`-bearing) refuse as uncertain; the
  libc path-locale feature is not implemented.
* Legacy non-Latin1 charsets implement ASCII and escaped bytes only (bounded
  ASCII-transparent list, host-verified for EUC-JP); their non-ASCII scalars
  remain required codec work. A charset outside the list refuses the whole
  message, and an undecidable alias relation for an unimplemented actual
  codec refuses rather than guessing a base codec.
* Alias matching covers the observed charmap aliases of the implemented
  codecs; it is not a claim about every glibc alias table.
* `PYTHONUTF8` values other than `0`/`1`, non-text environment values and
  undecidable startup facts remain truthful native capability failures.
* No JP/F4/source-path/stage-index-footprint/unborn/exact-ref/whole-Op/CLI/
  platform/performance claim. Native commit-payload limit remains 64 KiB and
  Git's reflog truncation remains 2000 bytes (this report makes no different
  claim). No commit, push, aggregate or release was performed; the coordinator
  owns that after correctness and fresh independent review.
