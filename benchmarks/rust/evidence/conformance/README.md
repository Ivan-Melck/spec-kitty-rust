# Conformance evidence (untimed)

`status-replay-mutation.json` is the **untimed** equivalence proof for the one
frozen completion workload that has a native implementation today:
`status.replay`.

Produced by:

```sh
PYTHONPATH=src SPEC_KITTY_PACKS_ROOT="$PWD/packs" .venv-rust-oracle/bin/python \
  benchmarks/rust/tools/mutation_check.py \
  --rust-binary target/release/spec-kitty \
  --oracle-python .venv-rust-oracle/bin/python --packs-root "$PWD/packs" \
  --invocations 2
```

It records that each side, in its own fresh clone of `replay-project`
(2,800 events / 200 WPs):

- emits an identical normalized result document,
- writes a byte-identical `kitty-specs/002-bench-mission-002/status.json`,
- produces an identical post-state delta (only `.git/spec-kitty-locks/` is
  excluded, per the recorded ruling),
- leaves the pristine fixture unchanged.

**It is not certification.** No paired sampling, load control or ratio is
recorded, the release binary predates the current source, and 16 of the 17
strict completion workloads are still absent natively. See
[../CERTIFICATION-REPORT.md](../CERTIFICATION-REPORT.md).

The tool's per-invocation `duration_s` metadata is retained for audit only; it is
not a timing result and must not be quoted as one.
