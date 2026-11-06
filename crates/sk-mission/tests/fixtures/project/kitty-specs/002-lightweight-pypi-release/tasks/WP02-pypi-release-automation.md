---
work_package_id: WP02
title: PyPI Release Automation
dependencies:
- WP01
subtasks:
- T003
- T004
phase: Phase 1 - Automation
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
- kitty-specs/002-lightweight-pypi-release/tasks/planned/WP02-pypi-release-automation.md
- .github/workflows/release.yml
wp_code: WP02
---
*Path: [kitty-specs/002-lightweight-pypi-release/tasks/planned/WP02-pypi-release-automation.md](kitty-specs/002-lightweight-pypi-release/tasks/planned/WP02-pypi-release-automation.md)*


# Work Package Prompt: WP02 – PyPI Release Automation

## Objectives & Success Criteria

- Prepare packaging metadata so `python -m build` followed by `twine check`
  succeeds with no warnings.
- Publish through PyPI trusted publishing; no long-lived API token is stored.

## Dependencies

WP01 validates the release before this package automates it.
