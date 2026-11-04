# Source argv design: coordinator assessment, not an approval

Subject: published `ee60eb5f3eb614e2294f33ef01d7782703b5b7dc`.
Read the complete design `/tmp/sk-source-argv-execution-design.md` (sha256
`3e3c9e08247bba7e8d865d3ed35ac4f88d04f8f31cd4c03d4e154002c5a1b84a`).
Original provisional `/tmp/sk-source-argv-execution-design.provisional.copy`
(sha256 `fa7893462a72e4e82ba00c11693d5dbf81bf8e1c486f68c674ea06da084b406c`)
and evidence `/tmp/sk-source-argv-design.ejCWp1/command-log-index.tsv` remain
unchanged. Coordinator provisional assessment
`/tmp/sk-source-argv-design-coordinator-assessment.md` preceded its own probes.

## Useful findings retained

Actual Source ordinary uses `git commit --only -m`, index-deletion uses
`git commit -m` on its temporary index, and pinned uses a UTF8 private file,
`commit-tree -F`, then `update-ref -m <original message>`. These are distinct
protocols, not one interchangeable size validator or one pinned/delete route.

Host evidence shows initial131071 accepted /131072 E2BIG here, not a portable
constant. Hook-written160KiB bodies/subjects can commit because those bodies
and ordinary reflogs are handled inside Git, not another argv string. Raising
Native's cap alone can create a candidate and then fail at `update-ref -m`
where Source succeeds, or create a commit object before a Source-E2BIG call
would even start Git. The current valid-Source65537 false refusal remains real.
No guessed limit, synthetic readiness subprocess, truncation or disclosure
establishes closure. Pinned post-hook file transport can be corrected separately;
see [pinned-file-transport-disposition.md](pinned-file-transport-disposition.md).

## A3′ is NOT approved

The actual `optionc_prime_probe.py`:

1. creates a candidate with `commit-tree` BEFORE the publishing argv spawn;
2. installs a temporary pre-commit that executes `commit-tree` AGAIN;
3. injects a message later through temporary prepare-commit-msg;
4. publishes with different arguments/environment and asserts HEAD afterwards.

Repeating the same candidate computation is not proof of the object that the
later Git commit will create. The verifier itself writes an object; its failure
is not proof of abort-before-object. The initial131072 case already has an
object before exec fails, unlike Source. Fixed dates and fixed x/y files do not
establish metadata/config/encoding/cleanup invariants under real hooks. A
post-success identity check cannot undo a wrong publication and is not pre-CAS
proof. Holding the real lock while publishing from a private index does not
make its contents/phase exactly Source's real `commit --only` prepare step.

The proposal omits Source's path operands/`--only`, adds hooksPath configuration,
and changes the environment. Per-string success does not prove total argv/env
parity. These differences cannot be adopted merely as measured residuals where
Source-valid calls would diverge. Hook substitution/manual replay, once-only
behavior, actual candidate evidence, signing/encoding/config snapshots and
post-hook mutation windows all need real proofs, not declarations of required
preconditions. No new writer, hooksPath override or constructor-cap removal is
authorized by the POC. Plain real-Git delegation is also not adopted by silently
weakening existing candidate/publication contracts.

The design's Source phase diagram also places COMMIT_EDITMSG before pre-commit;
actual earlier Source-first artifact controls show OLD artifact at pre-commit,
then the initial message write. Its pinned recommendation to retain ordinary
empty-message classification is contradicted by a new actual Source empty-file
commit. Its blanket removal of prepared validation changes a public read-only
Native construction contract; that is not a safe consequence of the probes.

## Evidence qualifications / next decisions

The report says oracle first, but the host `/bin/true` exec-boundary probe is
13:09:39 and exact-oracle receipt13:10:21. Actual reported Python Source cases
follow verification; the stronger all-probes-first statement is not true.
Standalone POCs use fixed dates/limited script shapes and do not assert the
complete Source/native roots and full state/object contract of an integration
suite. They remain exploratory evidence, not accepted publication gates.

D1/D2/D3/D4/D5/D7/D8 remain unresolved for ordinary/delete closure. D6 pinned
must follow actual file/write/hooks/object/argv phases, not an invented safer
refusal. Existing prepared APIs stay unchanged in the bounded correction.
No production A3′, no hidden helper/runtime Python, no new dependency/unsafe,
no reset/ref rewind/object/audit deletion, no full platform/workflow/performance
approval. Full Source argv/process and actual candidate-preservation design is
still required; no completion promotion follows from rejecting a proposal.
