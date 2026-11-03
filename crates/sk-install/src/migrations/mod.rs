//! Native ports of declared upgrade migrations.
//!
//! This module is the implementation side of [`crate::upgrade`]: the embedded
//! migration catalog declares 136 migrations, and `upgrade` refuses a selection
//! that contains one without a real implementation here. Each submodule owns
//! one coherent family and mirrors the pinned Python source it replaces,
//! including the source's preconditions, idempotency guard, file effects and
//! non-fatal warnings.
//!
//! The families in this tranche are:
//!
//! | Family | Migrations | Effect |
//! | --- | --- | --- |
//! | [`merge_drivers`] | 7 | tracked `.gitattributes` line + clone-local `git config` |
//! | [`run_index`] | 1 | portable `run_dir` tokens in `feature-runs.json` |
//! | [`lane_tip`] | 1 | clone-local `post-commit`/`post-rewrite` recorder hooks |
//! | [`pack_level`] | 1 | project `pack.yaml` `pack_level` descriptor |
//! | [`charter_pack_gitignore`] | 1 | `pack.yaml` allow-rule in `.gitignore` |
//! | [`narrow_agent_gitignore`] | 1 | replace whole-folder agent ignores |
//! | [`mission_template_forks`] | 1 | retire stale documentation/research forks |
//!
//! Deliberately absent: every other declared id. They stay `unimplemented`, so
//! an upgrade that selects one refuses before the first write rather than
//! claiming a compatibility this build does not have.

pub mod charter_pack_gitignore;
pub mod git;
pub mod json;
pub mod lane_tip;
pub mod merge_drivers;
pub mod mission_template_forks;
pub mod narrow_agent_gitignore;
pub mod pack_level;
pub mod run_index;
