# Owning-guard release with a duplicated descriptor

Scope: coordinator correction to `sk-kernel/src/locks.rs`, uncommitted on
`d64468a76`. Independent review now accepts the correction within its scope;
this remains descriptor simulation, not a real-fork test.

## Reproduction and correction

The init owner reported two intermittent failures of
`lock_is_exclusive_then_released_on_drop`: a zero-wait acquire failed after the
original guard was dropped. Subsequent passing retries do not explain those
failures and are not the remediation.

A deterministic native regression duplicates the held file descriptor with
`File::try_clone`, drops the owning guard, keeps the duplicate alive and attempts
an independent non-reentrant acquire. Before the correction this fails: closing
only one descriptor does not release a Unix flock while another descriptor
references that same open file description.

`Holding::release` now explicitly unlocks after clearing/syncing the holder.
Only the owning PID reaches that operation. The existing inherited-PID guard
still prevents a forked child from truncating or unlocking its parent's shared
description. Guard depth/reentrancy, lock identity and persistent inode semantics
are unchanged.

- Red: `/tmp/sk-lock-duplicate-release-red.log` — new regression fails.
- Green: `/tmp/sk-lock-duplicate-release-green.log` — **190 kernel passes,
  0 failures, 1 parent-exercised helper**, 22 suite results including the
  compile-fail doctest. Existing mixed-language/path/holder tests are included.
- Subsequent installer scoped run: **227 passes, 0 failures, 2 helpers** in
  `/tmp/sk-init-remediation-coordinator.log`.

Independent report `/tmp/sk-rust-git-remediation-review.md` reproduces all
**190 kernel passes / 0 failures / 1 helper**, scoped Clippy/fmt clean. Its
public-API probe verifies that another thread still contends after the outer
guard drops while an inner guard remains, and that the holder clears only at
zero depth. A separate fs2 probe confirms the duplicated-description release
mechanism. This is scoped acceptance, not a cross-platform or aggregate gate.

A concurrent fork-before-exec can temporarily inherit a descriptor even when it
is close-on-exec, so this mechanism could explain the reported intermittent
release symptom. Its causal role in those particular executions is **not
proven**. The regression simulates the descriptor lifetime; it does not perform
or certify a real fork, platform-wide behavior, or the whole rewrite.
