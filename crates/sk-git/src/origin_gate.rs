//! Origin-freshness gate: the ONE intent module every evidence gate calls.
//!
//! This is the Rust projection of `specify_cli/git/origin_freshness.py`'s policy
//! half — mode resolution (`--origin-check` beats `SPEC_KITTY_ORIGIN_CHECK`
//! beats `enforce`), the per-gate policy table, the refusal codes and the
//! operator text. The *facts* come from
//! [`GitRepository::check_branch_freshness`](crate::GitRepository::check_branch_freshness),
//! which already classifies one branch against the remote that owns it.
//!
//! Data-only toward rendering: nothing here prints. A refusal is
//! [`OriginFreshnessRefused`], carrying the first code, every distinct code in
//! refusal order (evidence first), the failing verdicts, the first headline and
//! the full operator text.
//!
//! # Two observation paths, only one of them Source's
//!
//! Upstream's evidence gates FETCH before comparing
//! (`origin_freshness._check_remote`). This module therefore offers both:
//!
//! * [`refresh_branch_facts`] — the SOURCE-EXECUTION path every gate here uses.
//!   It runs [`GitRepository::refresh_branches`], i.e. one bounded `ls-remote`
//!   plus ONE bounded `fetch` of the listed tips into
//!   `refs/remotes/<remote>/<branch>`, then compares against that tracking ref.
//!   A remote that answers is compared for real; a failed contact is
//!   `unreachable`, never a fabricated `behind`/`ahead`.
//! * [`check_branches`] — the READ-ONLY query, kept as its own contract. It
//!   never fetches, so a remote tip absent locally is
//!   [`crate::FreshnessState::CompareFailed`] with that explanation. It is NOT a
//!   substitute for the Source path at an evidence gate.

use std::path::Path;

use crate::error::GitError;
use crate::refname::BranchName;
use crate::remote::{FreshnessState, FreshnessVerdict};
use crate::repository::GitRepository;

/// One branch's freshness facts, as the gate consumes them.
///
/// This is a plain data view of an already-observed verdict (or, in a policy
/// test, of a verdict the test states explicitly). The gate never observes
/// anything itself: [`check_branches`] is where a real contact happens, and
/// [`OriginFacts::from_verdict`] is the only way a real observation becomes a
/// gate input on the production path.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginFacts {
    /// The branch that was checked.
    pub branch: String,
    /// The remote that owns it, when resolved.
    pub remote: Option<String>,
    /// The classification.
    pub state: FreshnessState,
    /// Commits only the remote has.
    pub behind: u64,
    /// Commits only the local branch has.
    pub ahead: u64,
    /// Extra context (a vanished branch, an unresolved compare, a transport error).
    pub detail: Option<String>,
    /// The bound the contact was given when it timed out.
    pub timeout_seconds: Option<f64>,
}

impl OriginFacts {
    /// The facts of one real observation.
    pub fn from_verdict(verdict: &FreshnessVerdict) -> Self {
        Self {
            branch: verdict.branch().as_str().to_owned(),
            remote: verdict.remote().map(|remote| remote.as_str().to_owned()),
            state: verdict.state(),
            behind: verdict.behind(),
            ahead: verdict.ahead(),
            detail: verdict.detail().map(str::to_owned),
            timeout_seconds: verdict.timeout_seconds(),
        }
    }
}

/// The environment variable that opts out of (or into) the check.
pub const ORIGIN_CHECK_ENV: &str = "SPEC_KITTY_ORIGIN_CHECK";
/// The status-evidence branch is stale relative to origin.
pub const ORIGIN_STATUS_STALE: &str = "ORIGIN_STATUS_STALE";
/// An approved lane branch is stale relative to origin.
pub const ORIGIN_LANE_STALE: &str = "ORIGIN_LANE_STALE";
/// A lane branch diverged from its remote.
pub const ORIGIN_LANE_DIVERGED: &str = "ORIGIN_LANE_DIVERGED";
/// The remote could not be asked.
pub const ORIGIN_UNREACHABLE: &str = "ORIGIN_UNREACHABLE";
/// Several remotes exist and none owns the branch.
pub const ORIGIN_REMOTE_AMBIGUOUS: &str = "ORIGIN_REMOTE_AMBIGUOUS";
/// The remote answered but the two tips could not be compared locally.
pub const ORIGIN_COMPARE_FAILED: &str = "ORIGIN_COMPARE_FAILED";

const STALE_STATES: [FreshnessState; 3] = [
    FreshnessState::Behind,
    FreshnessState::Diverged,
    FreshnessState::LocalMissing,
];
const OPT_OUT_LINE: &str =
    "Opt out (not recommended): --origin-check warn, or SPEC_KITTY_ORIGIN_CHECK=warn.";
const RERUN_LINE: &str = "  Then re-run.";
const NO_ANSWER: &str = "the remote did not answer";
/// Every value `--origin-check` accepts, in declaration order.
pub const ORIGIN_CHECK_CHOICES: [&str; 3] = ["enforce", "warn", "off"];

/// How an evidence gate treats a failing freshness verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginCheckMode {
    /// Refuse the gate.
    Enforce,
    /// Report a warning and continue.
    Warn,
    /// Contact nothing and accept stale evidence.
    Off,
}

impl OriginCheckMode {
    /// The CLI/environment spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Warn => "warn",
            Self::Off => "off",
        }
    }
}

/// The resolved mode, where it came from, and an optional diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginCheckSetting {
    /// The resolved mode.
    pub mode: OriginCheckMode,
    /// `default` | `flag` | `environment` | `environment-invalid` | `read-only`.
    pub source: &'static str,
    /// A diagnostic about the input (an invalid environment value).
    pub warning: Option<String>,
}

/// Read-only commands (`accept --no-commit` / `--diagnose`) always warn.
pub fn read_only_setting() -> OriginCheckSetting {
    OriginCheckSetting {
        mode: OriginCheckMode::Warn,
        source: "read-only",
        warning: None,
    }
}

/// The one warning `off` reports: no remote was contacted.
pub fn origin_not_checked_warning(setting: &OriginCheckSetting) -> String {
    format!(
        "Origin was not checked (origin check is {}, source: {}): no remote was contacted, so a \
         teammate's newer status evidence or lane work on origin is not seen and stale evidence is \
         accepted.",
        setting.mode.as_str(),
        setting.source
    )
}

fn parse_mode(value: &str) -> Option<OriginCheckMode> {
    match value.trim().to_lowercase().as_str() {
        "enforce" => Some(OriginCheckMode::Enforce),
        "warn" => Some(OriginCheckMode::Warn),
        "off" => Some(OriginCheckMode::Off),
        _ => None,
    }
}

/// Resolve the opt-out: the flag beats the environment, which beats `enforce`.
///
/// An unknown *flag* value is refused (Source's typer validates it as a choice
/// before this point); an unknown *environment* value enforces and carries a
/// warning naming it.
pub fn resolve_origin_check_mode(
    flag: Option<&str>,
    environment: Option<&str>,
) -> Result<OriginCheckSetting, String> {
    if let Some(flag) = flag {
        let Some(mode) = parse_mode(flag) else {
            let mut allowed = ORIGIN_CHECK_CHOICES.to_vec();
            allowed.sort_unstable();
            return Err(format!(
                "unknown --origin-check value {flag:?}; expected one of: {}",
                allowed.join(", ")
            ));
        };
        return Ok(OriginCheckSetting {
            mode,
            source: "flag",
            warning: None,
        });
    }
    let raw = environment.unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(OriginCheckSetting {
            mode: OriginCheckMode::Enforce,
            source: "default",
            warning: None,
        });
    }
    match parse_mode(raw) {
        Some(mode) => Ok(OriginCheckSetting {
            mode,
            source: "environment",
            warning: None,
        }),
        None => {
            let mut allowed = ORIGIN_CHECK_CHOICES.to_vec();
            allowed.sort_unstable();
            Ok(OriginCheckSetting {
                mode: OriginCheckMode::Enforce,
                source: "environment-invalid",
                warning: Some(format!(
                    "{ORIGIN_CHECK_ENV}={} is not one of {}; enforcing the origin freshness check.",
                    sk_kernel::python_json::python_repr(raw),
                    allowed.join(", ")
                )),
            })
        }
    }
}

/// A gate refusal: the codes, the failing verdicts and the operator text.
#[derive(Clone, Debug)]
pub struct OriginFreshnessRefused {
    /// The first refusal code.
    pub error_code: &'static str,
    /// Every distinct code in refusal order (evidence first).
    pub error_codes: Vec<&'static str>,
    /// The failing verdicts.
    pub verdicts: Vec<OriginFacts>,
    /// The first refusal's one-line summary.
    pub headline: String,
    /// The full operator text.
    pub message: String,
}

struct Violation<'a> {
    code: &'static str,
    verdict: &'a OriginFacts,
    is_evidence: bool,
}

fn code_for(verdict: &OriginFacts, is_evidence: bool) -> Option<&'static str> {
    match verdict.state {
        FreshnessState::Unreachable => Some(ORIGIN_UNREACHABLE),
        FreshnessState::RemoteAmbiguous => Some(ORIGIN_REMOTE_AMBIGUOUS),
        FreshnessState::CompareFailed => Some(ORIGIN_COMPARE_FAILED),
        state if STALE_STATES.contains(&state) => Some(if is_evidence {
            ORIGIN_STATUS_STALE
        } else {
            ORIGIN_LANE_STALE
        }),
        _ => None,
    }
}

/// `<CODE>: <branch> is <state> (<n> behind / <m> ahead of <remote>/<branch>)`.
pub fn refusal_headline(code: &str, verdict: &OriginFacts) -> String {
    let branch = verdict.branch.as_str();
    let remote = verdict
        .remote
        .clone()
        .unwrap_or_else(|| "origin".to_owned());
    let where_ = format!("{remote}/{branch}");
    match verdict.state {
        FreshnessState::Unreachable => format!(
            "{code}: {branch} is unreachable ({})",
            verdict.detail.as_deref().unwrap_or(NO_ANSWER)
        ),
        FreshnessState::RemoteAmbiguous => format!(
            "{code}: {branch} has no single remote to check (remotes: {})",
            verdict.detail.as_deref().unwrap_or("none")
        ),
        FreshnessState::CompareFailed => format!(
            "{code}: {branch} could not be compared with {where_} ({})",
            verdict
                .detail
                .as_deref()
                .unwrap_or("git could not compare the refs")
        ),
        FreshnessState::LocalMissing => {
            format!("{code}: {branch} is missing locally (it exists as {where_})")
        }
        state => format!(
            "{code}: {branch} is {} ({} behind / {} ahead of {where_})",
            state.as_str(),
            verdict.behind,
            verdict.ahead
        ),
    }
}

fn status_update_steps(verdict: &OriginFacts, checkout: Option<&Path>) -> Vec<String> {
    let remote = verdict
        .remote
        .clone()
        .unwrap_or_else(|| "origin".to_owned());
    let branch = verdict.branch.as_str();
    if verdict.state != FreshnessState::Diverged {
        return vec![match checkout {
            None => format!("git fetch {remote} {branch}:{branch}"),
            Some(checkout) => {
                format!("git -C {} pull {remote} {branch}", checkout.display())
            }
        }];
    }
    match checkout {
        Some(checkout) => vec![format!(
            "git -C {} pull --no-rebase {remote} {branch}",
            checkout.display()
        )],
        None => vec![
            format!("git worktree add <path> {branch}    (or: git switch {branch})"),
            format!("git pull --no-rebase {remote} {branch}    (in that checkout)"),
        ],
    }
}

fn status_remedy(
    verdict: &OriginFacts,
    checkout: Option<&Path>,
    merge_record_exists: bool,
) -> Vec<String> {
    let steps = status_update_steps(verdict, checkout);
    if merge_record_exists {
        let mut lines = vec![
            "  A consolidation record exists. Recover with:".to_owned(),
            "    spec-kitty consolidate --abort".to_owned(),
        ];
        lines.extend(steps.iter().map(|step| format!("    {step}")));
        lines.push("    spec-kitty consolidate".to_owned());
        return lines;
    }
    if steps.len() == 1 {
        return vec![
            format!("  Update it with: {}", steps[0]),
            RERUN_LINE.to_owned(),
        ];
    }
    let mut lines = vec!["  Update it with:".to_owned()];
    lines.extend(steps.iter().map(|step| format!("    {step}")));
    lines.push(RERUN_LINE.to_owned());
    lines
}

fn diverged_lane_remedy(remote: &str, branch: &str) -> Vec<String> {
    vec![
        format!("  Inspect: git log {branch}...{remote}/{branch}"),
        format!(
            "  If the remote lane has work you need: git -C <lane worktree> merge {remote}/{branch}"
        ),
        format!(
            "  If the local lane is the truth: merge or rebase it onto {remote}/{branch} first, then git push {remote} {branch}"
        ),
        format!("    (git push --force-with-lease would instead discard the commits on {remote}/{branch}: use it only if you mean to lose them.)")
            .to_owned(),
        RERUN_LINE.to_owned(),
    ]
}

fn lane_remedy(verdict: &OriginFacts) -> Vec<String> {
    let remote = verdict
        .remote
        .clone()
        .unwrap_or_else(|| "origin".to_owned());
    let lane = verdict.branch.as_str();
    if verdict.state == FreshnessState::Diverged {
        return diverged_lane_remedy(&remote, lane);
    }
    if verdict.state == FreshnessState::LocalMissing {
        return vec![
            format!("  Create it with: git branch {lane} {remote}/{lane}"),
            RERUN_LINE.to_owned(),
        ];
    }
    vec![
        format!("  Update it with: git fetch {remote} {lane}"),
        format!("    then git branch -f {lane} {remote}/{lane}"),
        format!(
            "    (or, when the lane is checked out: git -C <lane worktree> merge --ff-only {remote}/{lane})"
        ),
        RERUN_LINE.to_owned(),
    ]
}

fn unreachable_remedy(verdict: &OriginFacts) -> Vec<String> {
    let remote = verdict.remote.as_deref().unwrap_or("origin");
    if let Some(seconds) = verdict.timeout_seconds {
        return vec![
            format!(
                "  Remote {remote} did not answer within {} s. It may only be slow: retry first.",
                python_seconds_text(seconds)
            ),
            format!(
                "  If it stays slow, opt out as below, or {ORIGIN_CHECK_ENV}=off to skip the contact."
            ),
            RERUN_LINE.to_owned(),
        ];
    }
    vec![
        format!("  Check network access and credentials for remote {remote}."),
        format!(
            "  If you cannot reach it from here at all, {ORIGIN_CHECK_ENV}=off (or --origin-check off) skips the contact and accepts stale evidence."
        ),
        RERUN_LINE.to_owned(),
    ]
}

/// The `%g` spelling of a contact bound for the operator text.
///
/// The bound is always a whole number of seconds from `GitOptions`
/// (`FETCH_TIMEOUT`/`LS_REMOTE_TIMEOUT` upstream), where `%g` prints the bare
/// integer (`10`, not `10.0`). A sub-second bound falls back to CPython's float
/// repr, which is what `%g` produces for the values this can carry.
fn python_seconds_text(seconds: f64) -> String {
    if seconds.fract() == 0.0 && seconds.abs() < 1e6 {
        return format!("{}", seconds as i64);
    }
    sk_kernel::python_json::python_float_repr(seconds)
}

fn ambiguous_remote_remedy(verdict: &OriginFacts) -> Vec<String> {
    vec![
        format!(
            "  Name the remote that owns it: git config branch.{}.remote <name>    (remotes: {})",
            verdict.branch.as_str(),
            verdict.detail.as_deref().unwrap_or("none")
        ),
        RERUN_LINE.to_owned(),
    ]
}

fn compare_failed_remedy(verdict: &OriginFacts) -> Vec<String> {
    let remote = verdict
        .remote
        .clone()
        .unwrap_or_else(|| "origin".to_owned());
    vec![
        format!(
            "  The remote answered, but git could not compare {} with {remote}/{} here (a shallow clone or a damaged ref).",
            verdict.branch.as_str(),
            verdict.branch.as_str()
        ),
        format!(
            "  Deepen the history with: git fetch --unshallow {remote}    (or repair the ref), then re-run."
        ),
        format!(
            "  If you cannot repair it, {ORIGIN_CHECK_ENV}=off (or --origin-check off) skips the check and accepts stale evidence."
        ),
    ]
}

fn remedy(
    violation: &Violation<'_>,
    checkout: Option<&Path>,
    merge_record_exists: bool,
) -> Vec<String> {
    let verdict = violation.verdict;
    match verdict.state {
        FreshnessState::RemoteAmbiguous => ambiguous_remote_remedy(verdict),
        FreshnessState::Unreachable => unreachable_remedy(verdict),
        FreshnessState::CompareFailed => compare_failed_remedy(verdict),
        state if violation.is_evidence && state != FreshnessState::LocalMissing => {
            status_remedy(verdict, checkout, merge_record_exists)
        }
        _ => lane_remedy(verdict),
    }
}

fn vanished_notes(evidence: Option<&OriginFacts>, lanes: &[OriginFacts]) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    for verdict in evidence.into_iter().chain(lanes.iter()) {
        if matches!(
            verdict.state,
            FreshnessState::RemoteMissing | FreshnessState::NoRemote
        ) && let Some(detail) = verdict.detail.as_deref()
            && !notes.iter().any(|existing| existing == detail)
        {
            notes.push(detail.to_owned());
        }
    }
    notes
}

/// Apply the merge-path policy: return warnings, or refuse.
///
/// Evidence `local_missing` refuses `ORIGIN_STATUS_STALE`; a caller whose
/// evidence branch is a coordination branch handles that case before calling
/// (Source's `COORDINATION_WORKTREE_UNMATERIALIZED` path). In `warn` mode every
/// would-be refusal becomes a warning naming the verdict and the setting source.
pub fn enforce_merge_gate(
    evidence: Option<&OriginFacts>,
    lanes: &[OriginFacts],
    setting: &OriginCheckSetting,
    merge_record_exists: bool,
    evidence_checkout: Option<&Path>,
) -> Result<Vec<String>, OriginFreshnessRefused> {
    let mut warnings: Vec<String> = setting.warning.iter().cloned().collect();
    warnings.extend(vanished_notes(evidence, lanes));
    let mut violations: Vec<Violation<'_>> = Vec::new();
    if let Some(verdict) = evidence
        && let Some(code) = code_for(verdict, true)
    {
        violations.push(Violation {
            code,
            verdict,
            is_evidence: true,
        });
    }
    for verdict in lanes {
        if let Some(code) = code_for(verdict, false) {
            violations.push(Violation {
                code,
                verdict,
                is_evidence: false,
            });
        }
    }
    if violations.is_empty() {
        return Ok(warnings);
    }
    if setting.mode == OriginCheckMode::Warn {
        for violation in &violations {
            warnings.push(format!(
                "{} (origin check is {}, source: {}; continuing)",
                refusal_headline(violation.code, violation.verdict),
                setting.mode.as_str(),
                setting.source
            ));
        }
        return Ok(warnings);
    }
    let mut lines = warnings.clone();
    let mut headlines: Vec<String> = Vec::new();
    for violation in &violations {
        let headline = refusal_headline(violation.code, violation.verdict);
        headlines.push(headline.clone());
        lines.push(headline);
        lines.extend(remedy(violation, evidence_checkout, merge_record_exists));
    }
    lines.push(OPT_OUT_LINE.to_owned());
    let mut codes: Vec<&'static str> = Vec::new();
    for violation in &violations {
        if !codes.contains(&violation.code) {
            codes.push(violation.code);
        }
    }
    Err(OriginFreshnessRefused {
        error_code: codes[0],
        error_codes: codes,
        verdicts: violations
            .iter()
            .map(|violation| violation.verdict.clone())
            .collect(),
        headline: headlines[0].clone(),
        message: lines.join("\n"),
    })
}

/// Source-execution freshness facts for `branches`, in the order given.
///
/// This is the path a gate must use: it performs Source's own bounded `fetch`
/// (through [`GitRepository::refresh_branches`]) and then compares each local
/// branch with its tracking ref. A branch that is not a legal branch name is a
/// refusal-grade error rather than a silent skip — the caller names what it is
/// about to judge.
pub fn refresh_branch_facts(
    repository: &GitRepository,
    branches: &[&str],
) -> Result<Vec<OriginFacts>, GitError> {
    let parsed: Vec<BranchName> = branches
        .iter()
        .map(|branch| BranchName::parse(branch))
        .collect::<Result<_, _>>()?;
    Ok(repository
        .refresh_branches(&parsed)?
        .iter()
        .map(|refreshed| OriginFacts::from_verdict(refreshed.verdict()))
        .collect())
}

/// Read-only freshness verdicts for `branches`, in the order given.
///
/// NEVER fetches (see the module docs): a tip absent locally is
/// `compare_failed`. Evidence gates use [`refresh_branch_facts`] instead.
///
/// A branch that is not a legal branch name is reported as a refusal-grade error
/// rather than silently skipped: the caller names what it is about to judge.
pub fn check_branches(
    repository: &GitRepository,
    branches: &[&str],
) -> Result<Vec<OriginFacts>, GitError> {
    let mut verdicts = Vec::new();
    for branch in branches {
        let parsed = BranchName::parse(branch)?;
        verdicts.push(OriginFacts::from_verdict(
            &repository.check_branch_freshness(&parsed)?,
        ));
    }
    Ok(verdicts)
}
