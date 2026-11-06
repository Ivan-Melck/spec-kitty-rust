# Next prompt-boundary corrections — coordinator delivery, review pending

Baseline: f42d5380b0d3be11773e0b2354eb73e9c2a2c47d. Inputs: /tmp/sk-rust-next-remediation-review.md N1/N3 and /tmp/sk-rust-native-invocation-composition-plan.md §13. Coordinator owns runtime.rs and runtime_next.rs; active status remediation owns disjoint task/status files. No aggregate or release while that owner is active.

Implemented: unknown activated mission-type refusal applies on the WP governance seam only, not legitimate non-WP template steps. `work_package_text` preserves full authored WP text including frontmatter and trailing whitespace inside the prompt block; parsing for metadata is unchanged.

Red `/tmp/sk-next-prompt-boundary-red.log`: **4 passed / 2 failed**, exactly the new cases. The committed demo-io oracle transcript's first issued non-WP step is `specify`, not `plan`; after its answer/issue, the next advance issues `plan`. The regression proves both activated custom steps get their templateless prompts without inventing a profile. The existing activated custom WP refusal still passes. The WP regression includes authored trailing whitespace, so merely restoring frontmatter but retaining trim_end cannot satisfy it.

Green `/tmp/sk-next-prompt-boundary-green.log`: **27 runtime_next tests passed / 0 failures** through `/tmp/sk-focused-cargo.sh`, with the shared whole-Cargo lease, existing target and temporary HOME/XDG/TMP. Status owner completed their disjoint edits before the production fix/green run. No aggregate/release or independent acceptance yet.

This does not implement StepContract/ProfileInvocation, decision logs, prompt-path parity, rich/compact governance rendering, language vocabulary union or lifecycle/coordination. The protected-main composition false-advance defect remains open. No whole-next or performance acceptance.
