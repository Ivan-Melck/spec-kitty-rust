# CF90 — fallible canonical graph text, partial bounded intake

Implementation follow-up to published `8867d1fbf7465054f4737039a53a10779260ab78`, not whole parity, independent acceptance, release or performance certification. Source `9aed3a8012cb5eb22c0128b14d525039f95fea7a` and root/migration/doctor accounting stay unchanged. Earlier partial lanes remain partial; CF90 is bounded and partial too.

## Actual capability

One graph repr traversal now supports a typed failure result at the exact demanded integer. The new try_repr/try_python_str paths propagate through list/dict/key/set/pairs traversal without a second walker or speculative whole-subtree prepass. Exact strings and ignored fields are not subjected to integer conversion. Existing alias/cycle/ordered-container spelling remains in the same renderer. Historical repr/python_str facades explicitly use unlimited integer formatting for inspection; they are NOT certified Source business entry points. Remaining business callers of those facades still need caller-specific integration.

The canonical integer owner now distinguishes int-to-text formatting from text-to-int parsing. Measured CPython str(int) ceiling text omits the observed digit count; int(str) includes it. The new format helper preserves that distinction and reuses the existing startup digit-policy authority (default, configured 640 and disabled 0), not a copied environment parser or new bound. Both eager artifact-hook id renders use the fallible path before field/model validation. Source ValueError aborts instead of becoming a healthy schema skip. An invalid startup setting is a typed Native IntegerTextPolicy cause, not a fictional Python business exception.

## Parent intake and current checks

Actual 258-line producer report `/tmp/sk-cf90-fallible-graph-text-report.md` read to EOF. FIRST report/orders/actual Kernel/Doctrine/Source/helper/map/test custody `.0tVBzc`; renderer, integer owner, eager caller and target guard/API inspection `.KLIdb3`. No parent production correction was necessary.

- `.GUBpfW`: current named CF90 target, two passed. This includes the independent plan under three configured policies and Native-only cache/startup controls; a helper entry returning in the parent is not an extra feature/pair.
- `.iX9dD5`: directly affected CF89 target, one passed.
- `.IUMOA5`: selected Kernel/Doctrine library strict Clippy clean.
- `.7szZBY`: named target strict Clippy, owned/changed-file check-format, three canonical prospective-map validators, narrow path/token inventory, whitespace and remote preflight passed.
- `.gYPVxZ`: current compatibility binary compiled with the changed error carrier.

The real Source/Native business children assert independent fresh CWD/HOME/seven-XDG/TMP roots, independently supplied expected Source/PYTHONPATH/packs/module origin/hashseed, actual fs/stdout/stderr policies and pinned CPython3.11.15/exact53 before business reads. Identical relative business arguments retain raw messages; no root rewriting/shared fixture. Source intended ValueError/inline-rejection classes are asserted before Native. Invalid startup controls are Native boundaries, not Source pairs. Result comparisons do not establish warning/traceback/global error-wire or filesystem side-effect parity.

Producer's Source measurements show shallow hex integer construction succeeds, then eager decimal str raises before field walks; nested demanded values fail, ignored note values do not, strings are unaffected, and 0 disables the ceiling. The pre-edit receipt contained decisive wrong-skip business rows AND an out-of-domain long flow-key row; the latter is a YAML/parser premise issue, not evidence for this render fix. It was corrected without old-byte replay. Pre-report unwrapped intake shells, failed/truncated census attempts, invalid-policy verifier setup mistake, format and result_large_err attempts remain disclosed. Boxing retained the original startup cause instead of suppressing lint or changing foreign profile code. No-mark freshness is not no-filesystem-effects proof.

## Required gaps

No caller/frame recursion budget or RecursionError closure; no new guessed depth cutoff was introduced. Identity declared-id, duplicate-key render_partial and other business str/repr callers still require actual Source catch/phase classification and integration. Exact non-scalar exception rendering, warnings/global wire, long flow-key parser behavior, platform/concurrency/workflows and genuine performance measurement remain open. No parser/dependency/unsafe/FFI/root/count/registration/writer/state/lock promotion or semantic exception. Only DecisionIndex exact-surrogate summary associations are authorized.
