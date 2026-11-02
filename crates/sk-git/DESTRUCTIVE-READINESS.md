# Additional worktree-removal safety boundary

The untracked/ignored discard policies do not authorize deleting detected nested
repository history. Classification checks both collapsed directories and the
ancestors of expanded file entries, so ordinary embedded repositories and bare
repositories are covered. Ignored directory subtrees are walked without following
directory symlinks. Any unreadable metadata refuses rather than proving absence.

The checks run at planning and again at apply. An intervening status change may
already produce PlanStale; either refusal preserves the checkout and nested refs.
Tests use real temporary nested repositories with commits, including a bare repo
and a new repository created after planning. All 36 lifecycle tests pass locally.

Resource limits are explicit nondestructive refusals: 64 levels beneath a collapsed
directory, 16,384 entries per collapsed-directory scan, and 16,384 unique checked
directories across classification. These limits are not equivalent to unlimited
Python/Git deletion. No mutation begins after a safety-scan refusal.

This is static/apply-time detection, not a capability-based sandbox against an
adversary swapping ancestors or creating content during Git's eventual removal.
The caller still owns live protection resolution and coordinated mutation policy.
