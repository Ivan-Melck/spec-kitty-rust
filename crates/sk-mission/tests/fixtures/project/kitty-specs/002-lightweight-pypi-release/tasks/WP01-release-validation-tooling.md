---
work_package_id: WP01
title: Release Validation Tooling
dependencies: []
subtasks:
- T001
- T002
phase: Phase 1 - Foundations
history:
- timestamp: '2025-11-02T16:58:36Z'
  lane: planned
  agent: system
  shell_pid: ''
  action: Prompt generated via /spec-kitty.tasks
authoritative_surface: ''
execution_mode: code_change
mission_id: 01KN2371WNZX28ME4MN97B67PJ
owned_files:
- kitty-specs/002-lightweight-pypi-release/tasks/planned/WP01-release-validation-tooling.md
- tests/release/test_validate_release.py
wp_code: WP01
---
*Path: [kitty-specs/002-lightweight-pypi-release/tasks/planned/WP01-release-validation-tooling.md](kitty-specs/002-lightweight-pypi-release/tasks/planned/WP01-release-validation-tooling.md)*


# Work Package Prompt: WP01 – Release Validation Tooling

## Objectives & Success Criteria

- Provide a single entry point (`python scripts/release/validate_release.py`)
  that verifies readiness in both branch and tag contexts.
- Guarantee the validator is covered by automated tests to prevent regressions.

## Subtasks & Detailed Guidance

### Subtask T001 – Implement validator CLI

Use `argparse` and keep the script dependency-free beyond the standard library.
