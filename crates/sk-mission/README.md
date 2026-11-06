# sk-mission

Mission planning for the Rust Spec Kitty implementation: inventory, dependency
planning/readiness, typed Mission creation, canonical-template artifact
authoring, and `tasks.md` dependency finalization.

The crate depends only on `sk-kernel` (plus serialization/ULID). It does **not**
depend on `sk-git`, `sk-workspace`, `sk-status`, or any Python process. It never
invokes Python at runtime.

## Boundary: planning artifacts are not an allocation

A successful `MissionCreatePlan::commit` writes the Mission's planning
scaffold. It does **not** mint/checkout a branch, create a lane manifest or
worktree, commit Git state, adopt ownership, or append status events. The
receipt says so explicitly:

```rust
receipt.allocation() == MissionAllocationState::PlanningArtifactsOnly
```

The workspace owner consumes the plan's identity, topology, and
coordination-branch requirement and performs those operations separately.
Canonical ULIDs are injected (`MissionIdentity`), so tests and callers control
identity and time. `mission_number` is display-only and stays `null` until the
later merge-time assignment.

## Supported APIs

### Read-only inventory (preserved)

- `list_missions`, `select_mission`, `Mission`, `MissionId`, `MissionSlug`
- `WorkPackage`, `WorkPackageGraph`, `dependency_readiness`,
  `ready_work_packages`, `WpStateMap`
- `MissionTopology`, `classify_topology`, `routes_through_coordination`
- `MissionPaths` and canonical artifact-name constants
- `split_frontmatter` (read-only; no reserialization)

### Typed Mission creation

- `MissionCreateSlug` — kebab-case request slug; canonical
  `<stripped-slug>-<mid8>` directory naming
- `MissionCreateRequest` — slug, branches, type, friendly name, purpose,
  topology (reference default `coord`), retention/binding flags, and the
  workspace-owner coordination branch required by coordination topologies
- `MissionIdentity::{new,mint}` — injected canonical ULID + timestamp
- `MissionCreateOptions` — duplicate policy, caller-classified abandoned prior
  missions, lock timeout
- `MissionCreatePlan::{plan,validate,commit,commit_with_observer}` — pure
  planning, stale-plan checks, lock, failure-atomic write
- `MissionCreateReceipt`, `PlannedFile`, `PlannedFileKind`, `PlannedFileMode`,
  `MissionWriteObserver`
- `CanonicalTemplate`, `MissionTemplates`, `MissionTemplateKind`,
  `WorkPackageTemplateContext`, `render_tasks_readme`, `TemplateError`

Creation scaffolds `meta.json`, `spec.md` (injected canonical template),
`tasks/README.md` (canonical Python-pinned rendering), `tasks/.gitkeep`,
`checklists/`, `research/`, and an empty `status.events.jsonl` for
non-coordination topologies. A coordination topology requires the
workspace-owner branch and deliberately does not create the status log locally.
Existing user files are preserved; an abandoned prior can be repaired in place
without losing custom metadata or files.

### Artifact authoring

- `MissionArtifactRequest`, `WorkPackageDraft`
- `MissionArtifactPlan::{plan,validate,commit,commit_with_observer}`
- `MissionArtifactReceipt`, `ArtifactFile`, `TaskArtifactError`

`plan.md`, `tasks.md`, and work-package prompts come from injected canonical
templates; the crate does not contain invented plan/tasks/WP prose. Existing
`spec.md`, `plan.md`, `tasks.md`, `tasks/README.md`, and WP prompts are never
overwritten. Work-package filenames are canonical `WP##-kebab-slug.md`;
duplicate ids, self-dependencies, unsafe filenames, unrendered placeholders, and
a same-id/different-filename prompt fail closed before writing.

### Task outline and dependency finalization

- `TasksOutline`, `TasksSection`, `parse_dependencies_from_tasks_md`
- `DependencyFinalizePlan::{compute,apply,apply_with_observer}`
- `DependencyUpdate`, `DependencyApplyReceipt`, `TasksAuthoringError`

The parser matches the pinned reference's three explicit declaration formats
(`Depends on …`, `**Dependencies**: …`, and a bullet list under a Dependencies
heading) with final-section prose bounding. Finalization:

1. parses `tasks.md` and refuses duplicate WP sections;
2. validates WP-file/section coverage;
3. reads and validates WP frontmatter, refusing id mismatches, duplicate keys,
   duplicate dependencies, and malformed YAML;
4. refuses genuine disagreement between non-empty tasks.md deps and non-empty
   frontmatter deps;
5. preserves frontmatter deps when tasks.md declares none;
6. validates the *effective persisted* graph (self, unknown, cycle);
7. patching only the `dependencies` value, preserving comments, key order,
   indentation, line endings, and the Markdown body;
8. applying under a lock with stale-plan checks and rollback of every prior byte
   on a partial failure.

A second apply is a no-op. `tasks.md` itself is never rewritten.

## Oracle fixtures and provenance

`tests/fixtures/oracle/` contains committed outputs from the pinned Python
reference. `tests/fixtures/oracle/provenance.json` records the interpreter,
Python version, upstream commit, and source modules.
`tests/oracle/generate_oracle_fixtures.py` regenerates them in temporary
directories; it never writes the repository's real `kitty-specs/` or the user's
home. Rust tests read the fixtures, not Python.

Committed canonical template copies in
`tests/fixtures/oracle/templates/` carry their own `PROVENANCE.json` and are
test inputs only; the production crate ships no template prose.

## Remaining gaps (owned elsewhere or still pending)

- CLI command/option wiring and the full command inventory (another owner).
- Coordination-branch mint/checkout, lane manifest generation (`lanes.json`),
  worktree creation, Git commits/branches, ownership/placement, and rollback of
  those Git resources.
- Canonical status bootstrap/event writes, snapshot reduction, and lane state;
  creation only writes an empty non-coordination status-log placeholder.
- `mission finalize`, acceptance, consolidation, merge/recovery, and migration
  protocols.
- Charter/pack template resolution and fetching; callers inject resolved
  `CanonicalTemplate`s.
- Duplicate-abandonment evidence computation (status replay/Git tracking);
  callers mark prior Mission ids as abandoned.
- Documentation-mission `documentation_state` initialization and other
  mission-type-specific optional metadata.
- The bulk-edit `occurrence_map.yaml` finalize gate and the tasks post-integration
  warning (both command-adapter concerns).
- `agent tasks move-task`, `mark-status`, `add-history`, `list-tasks`,
  requirements mapping, and task-status surfaces.
- Cross-platform checks; local Linux tests do not establish macOS/Windows
  behavior.
