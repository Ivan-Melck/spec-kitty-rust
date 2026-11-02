//! Clap surface for the partial-rewrite binary.
//!
//! The help text is deliberately explicit: this adapter is a read-only slice of
//! Spec Kitty, not a drop-in replacement for the Python command surface.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Partial Rust rewrite of the Spec Kitty CLI.
///
/// This binary is a read-only inspection adapter. It never initializes a
/// project, upgrades generated files, creates or removes worktrees, merges,
/// activates charter artifacts, writes the event ledger, or runs the full
/// governed `next` workflow. Those operations are listed as unsupported by
/// `spec-kitty-rust capabilities` and are refused rather than simulated.
///
/// Operational commands emit exactly one JSON document on stdout. Help and
/// version output are human-readable unless `--json` is passed. Diagnostics go to
/// stderr. Exit codes: 0 success, 1 domain/data refusal, 2 usage error, 3 the
/// runtime query returned a blocked decision.
#[derive(Debug, Parser)]
#[command(
    name = "spec-kitty-rust",
    version,
    about = "Partial Rust rewrite of Spec Kitty: read-only inspection only",
    long_about = None,
    arg_required_else_help = true,
    disable_help_subcommand = true,
    after_help = "Unsupported by this partial rewrite: init, upgrade, worktrees, merge, activation, event-ledger writes, and the full governed next workflow.\nOperational commands emit one JSON document; help/version do so with --json. Diagnostics go to stderr."
)]
pub struct Cli {
    /// Emit the single JSON result document. JSON is the canonical output; this
    /// flag is accepted for scripting compatibility.
    #[arg(long, global = true)]
    pub json: bool,

    /// Authoritative root with .kittify/, kitty-specs/ or .git; not a subdirectory.
    /// Defaults to discovery from the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    pub project_root: Option<PathBuf>,

    /// Command to run.
    #[command(subcommand)]
    pub command: Command,
}

impl Command {
    /// Stable dotted command name used in the JSON envelope.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Project { command } => command.name(),
            Self::Git { command } => command.name(),
            Self::Mission { command } => command.name(),
            Self::Charter { command } => command.name(),
            Self::Glossary { command } => command.name(),
            Self::Runtime { command } => command.name(),
            Self::Events { command } => command.name(),
            Self::Transition { command } => command.name(),
        }
    }
}

/// Machine-readable capability and refusal matrix.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Report the implemented command surface and explicit non-capabilities.
    Capabilities,
    /// Read-only project-root operations.
    Project {
        /// Project command.
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Read-only Git inspection.
    Git {
        /// Git command.
        #[command(subcommand)]
        command: GitCommand,
    },
    /// Read-only Mission inventory and dependency graph.
    Mission {
        /// Mission command.
        #[command(subcommand)]
        command: MissionCommand,
    },
    /// Read-only charter, pack and asset inspection.
    Charter {
        /// Charter command.
        #[command(subcommand)]
        command: CharterCommand,
    },
    /// Read-only glossary lookup.
    Glossary {
        /// Glossary command.
        #[command(subcommand)]
        command: GlossaryCommand,
    },
    /// Pure mission-runtime planning (not the full governed `next`).
    Runtime {
        /// Runtime command.
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    /// Read-only event-stream statistics (never a state reducer).
    Events {
        /// Events command.
        #[command(subcommand)]
        command: EventsCommand,
    },
    /// Pure work-package lane FSM validation (never a persistence command).
    Transition {
        /// Transition command.
        #[command(subcommand)]
        command: Box<TransitionCommand>,
    },
}

/// Project-root subcommands.
#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Discover and report the Spec Kitty project root.
    Root,
}

impl ProjectCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Root => "project.root",
        }
    }
}

/// Git subcommands.
#[derive(Debug, Subcommand)]
pub enum GitCommand {
    /// Inspect branch, HEAD, dirtiness and porcelain status.
    Inspect,
}

impl GitCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Inspect => "git.inspect",
        }
    }
}

/// Mission subcommands.
#[derive(Debug, Subcommand)]
pub enum MissionCommand {
    /// List Missions under `kitty-specs/`.
    List,
    /// Show one Mission selected by full id, 8-character mid8, or exact slug.
    Show {
        /// Full 26-character mission id, 8-character mid8, or exact slug.
        #[arg(value_name = "SELECTOR")]
        selector: String,
    },
    /// Show one Mission's work-package dependency graph.
    Graph {
        /// Full 26-character mission id, 8-character mid8, or exact slug.
        #[arg(value_name = "SELECTOR")]
        selector: String,
    },
}

impl MissionCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::List => "mission.list",
            Self::Show { .. } => "mission.show",
            Self::Graph { .. } => "mission.graph",
        }
    }
}

/// Charter subcommands.
#[derive(Debug, Subcommand)]
pub enum CharterCommand {
    /// Inspect config, charter bundle, pack roots/descriptors and assets.
    Inspect {
        /// Also resolve and inspect the built-in pack tier.
        #[arg(long)]
        built_in: bool,
    },
}

impl CharterCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Inspect { .. } => "charter.inspect",
        }
    }
}

/// Glossary subcommands.
#[derive(Debug, Subcommand)]
pub enum GlossaryCommand {
    /// Resolve one term against explicit scopes and seed files.
    Lookup {
        /// Term to resolve (normalized by the glossary domain).
        #[arg(value_name = "TERM")]
        term: String,
        /// Scope to search, in precedence order. Repeat for several scopes;
        /// omit to use the canonical mission_local > team_domain >
        /// audience_domain > spec_kitty_core order.
        #[arg(long = "scope", value_name = "SCOPE")]
        scopes: Vec<String>,
        /// Override one scope's seed file, as SCOPE=PATH. Repeatable.
        #[arg(long = "seed-file", value_name = "SCOPE=PATH")]
        seed_files: Vec<String>,
        /// Confidence used for conflict severity (0.0..=1.0).
        #[arg(long, default_value_t = 1.0, value_name = "CONFIDENCE")]
        confidence: f64,
        /// Treat the lookup as occurring in a critical step.
        #[arg(long)]
        critical_step: bool,
    },
}

impl GlossaryCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Lookup { .. } => "glossary.lookup",
        }
    }
}

/// Runtime subcommands.
#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    /// Run the pure planner over an explicit frozen template and run snapshot.
    ///
    /// This does not create a run, persist a snapshot, emit a prompt file, or
    /// advance anything. Use the Python command surface for full `next`.
    Query {
        /// Mission-template YAML file.
        #[arg(long, value_name = "FILE")]
        template: PathBuf,
        /// Frozen `RunSnapshot` JSON file.
        #[arg(long, value_name = "FILE")]
        state: PathBuf,
        /// Live template hash used for drift detection. Omit to skip drift
        /// evaluation, matching the upstream "no live template" rule.
        #[arg(long, value_name = "HASH")]
        current_template_hash: Option<String>,
    },
}

impl RuntimeCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Query { .. } => "runtime.query",
        }
    }
}

/// Event-stream subcommands.
#[derive(Debug, Subcommand)]
pub enum EventsCommand {
    /// Report stream statistics without reconstructing a current lane.
    Inspect {
        /// Path to a `status.events.jsonl` stream.
        #[arg(value_name = "EVENTS_JSONL")]
        path: PathBuf,
    },
}

impl EventsCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Inspect { .. } => "events.inspect",
        }
    }
}

/// Transition subcommands.
#[derive(Debug, Subcommand)]
pub enum TransitionCommand {
    /// Validate one lane transition through the pure FSM.
    Check(TransitionCheck),
}

impl TransitionCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Check(_) => "transition.check",
        }
    }
}

/// Guard inputs for one pure FSM check.
#[derive(Debug, Args)]
pub struct TransitionCheck {
    /// Source lane (accepts the `doing` input alias).
    #[arg(value_name = "FROM")]
    pub from: String,
    /// Target lane (accepts the `doing` input alias).
    #[arg(value_name = "TO")]
    pub to: String,
    /// Actor identity.
    #[arg(long)]
    pub actor: Option<String>,
    /// Reason text.
    #[arg(long)]
    pub reason: Option<String>,
    /// Review-feedback reference.
    #[arg(long)]
    pub review_ref: Option<String>,
    /// Request a forced transition. The FSM still requires actor and reason.
    #[arg(long)]
    pub force: bool,
    /// Workspace context (required by `claimed -> in_progress`).
    #[arg(long)]
    pub workspace_context: Option<String>,
    /// All subtasks complete (tri-state guard input).
    #[arg(long, value_name = "BOOL")]
    pub subtasks_complete: Option<bool>,
    /// Implementation evidence present (tri-state guard input).
    #[arg(long, value_name = "BOOL")]
    pub implementation_evidence_present: Option<bool>,
    /// Dependency readiness (tri-state guard input).
    #[arg(long, value_name = "BOOL")]
    pub dependency_ready: Option<bool>,
    /// Current actor, for reviewer-collision probes.
    #[arg(long)]
    pub current_actor: Option<String>,
    /// Review evidence reviewer. Requires `--evidence-verdict` and `--evidence-reference`.
    #[arg(long)]
    pub evidence_reviewer: Option<String>,
    /// Review evidence verdict (`approved` or `changes_requested`).
    #[arg(long)]
    pub evidence_verdict: Option<String>,
    /// Review evidence reference.
    #[arg(long)]
    pub evidence_reference: Option<String>,
    /// Structured review-result reviewer. Requires verdict and reference.
    #[arg(long)]
    pub review_result_reviewer: Option<String>,
    /// Structured review-result verdict.
    #[arg(long)]
    pub review_result_verdict: Option<String>,
    /// Structured review-result reference.
    #[arg(long)]
    pub review_result_reference: Option<String>,
    /// Structured review-result feedback path.
    #[arg(long)]
    pub review_result_feedback_path: Option<String>,
}
