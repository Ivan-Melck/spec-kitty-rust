# Bounded canonical status-replay oracle corpus

These fixtures pin the Rust consumer-side `status.events.jsonl -> status.json`
projection against the installed oracle, not against hand-written expectations.

## Provenance

- Oracle interpreter: `/home/exedev/.local/share/uv/tools/spec-kitty-cli/bin/python`
- `spec_kitty_events` version: `11.3.0` (matches the `uv.lock` pin)
- External reducer: `spec_kitty_events.diary.reduce_parsed`
- CLI projection entry point: `specify_cli.status.reducer.materialize_snapshot`
- Generator: [`generate_fixtures.py`](generate_fixtures.py)
- The checked-in [`provenance.json`](provenance.json) records the generation-time
  source revision, per-fixture input/expected SHA-256 digests, and corpus bounds.

The generator refuses to run when the installed `spec_kitty_events` version
differs from `uv.lock`. Python is a development-time oracle only; no Rust test
or production path imports Python or vendors the external package.

Regenerate from the repository root:

```bash
SPEC_KITTY_PACKS_ROOT="$HOME/.cache/spec-kitty/open-packs/<pinned-sha>/packs" \
PYTHONPATH=src \
"$HOME/.local/share/uv/tools/spec-kitty-cli/bin/python" \
crates/sk-status/tests/fixtures/replay/generate_fixtures.py
```

## Layout and bounds

```text
cases/<name>/status.events.jsonl   synthetic semantic cases
cases/<name>/meta.json             optional mission identity
cases/<name>/status.json           optional existing-schema marker
cases/<name>/expected.json         oracle materialize_snapshot output (exact bytes)
archives/<mission>/...             bounded copies of real archived mission inputs
```

The committed corpus is 27 fixtures, 455 events and 296,824 input bytes; the
generator enforces `<= 500` events and `<= 400,000` input bytes.

Synthetic cases cover shuffled rows, real and seed dedupe, stale approvals,
same-timestamp rollback arbitration, rollback seeds and their bare sibling
approval, approved-departure clock-skew lifting/carry floor, losing-seed payload
folding, runtime-slot preservation, annotation-only WPs, cancellation
provenance, retrospective attachment, legacy versus v2 schema replay, empty
streams and unsafe mission slugs.

The archive corpus intentionally includes missions whose committed
`status.json` predates or diverges from the current reducer. The authority for
each fixture is the oracle output in `expected.json`, while the presence or
absence of the on-disk `schema_version` key decides the replay generation:

- legacy (no marker): no `schema_version`, no `implementer_of_record`;
- v2 (`"schema_version": 2`): marker plus `implementer_of_record`.

During implementation a development-only sweep additionally replayed all 719
archived missions with a diary in `kitty-specs/`; all 719 rendered snapshots
matched the oracle byte-for-byte. That sweep is not committed because it
depends on the full archive tree.
