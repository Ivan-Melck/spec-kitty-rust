//! The upstream-shaped native CLI (`spec-kitty`).
//!
//! Every route either executes real domain behavior or refuses before any
//! mutation. See [`capabilities::matrix`] for the command-by-command state.

pub mod capabilities;
pub mod init;
pub mod mission;
pub mod runtime;
pub mod status;
pub mod support;
pub mod tasks;
pub mod workspace;

/// Core-version target of the pinned upstream data and command contracts.
/// This identifies compatibility, not completion; capabilities remain explicit.
pub const CORE_VERSION: &str = "4.0.0rc8";

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::compat::support::{CompatError, emit_json};

/// Native Spec Kitty CLI.
///
/// Implemented compatibility routes: init, specify, plan, tasks, implement,
/// next, agent status validate/materialize, agent tasks reads, agent mission
/// create/setup-plan/finalize-tasks. All other upstream routes are refused with
/// a typed message; `capabilities` reports the exact state.
#[derive(Debug, Parser)]
#[command(
    name = "spec-kitty",
    version = CORE_VERSION,
    about = "Spec Kitty native Rust CLI",
    long_about = None,
    disable_help_subcommand = true,
    after_help = "Native extension: `runtime` (start/query/advance/answer/recover), `workspace recover`, and `native` (the versioned inspection envelope). Unsupported upstream routes are refused before mutation; run `spec-kitty capabilities --json` for the exact matrix."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level routes.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new Spec Kitty project.
    Init(InitArgs),
    /// Create a mission scaffold in kitty-specs/.
    Specify(SpecifyArgs),
    /// Scaffold plan.md for a Mission.
    Plan(PlanArgs),
    /// Finalize tasks metadata after task generation.
    Tasks(TasksArgs),
    /// Allocate or reuse the lane worktree for a work package.
    Implement(ImplementArgs),
    /// Decide and emit the next agent action for the current mission.
    Next(NextArgs),
    /// Upgrade is not yet implemented by the native installer.
    Upgrade(UpgradeArgs),
    /// Report the compatibility capability matrix.
    Capabilities(JsonArgs),
    /// Serve the original versioned inspection envelope.
    Native {
        /// Arguments forwarded to the native inspection command tree.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Agent lifecycle commands.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Native extension: durable governed-run substrate.
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    /// Native extension: lane allocation recovery.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Consolidation is not yet implemented natively.
    Consolidate(UnsupportedArgs),
    /// Acceptance is not yet implemented natively.
    Accept(UnsupportedArgs),
    /// Review is not yet implemented natively.
    Review(UnsupportedArgs),
    /// Merging was renamed to consolidate and is not implemented natively.
    Merge(UnsupportedArgs),
}

/// `init` options.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Project directory name; omit or pass `.` for the current directory.
    #[arg(value_name = "PROJECT_NAME")]
    pub project_name: Option<String>,
    /// Comma-separated AI assistants (claude,codex,gemini,...).
    #[arg(long, value_name = "AGENTS")]
    pub ai: Option<String>,
    /// Run without interactive prompts.
    #[arg(long, alias = "yes")]
    pub non_interactive: bool,
}

/// `specify` options.
#[derive(Debug, Args)]
pub struct SpecifyArgs {
    /// Mission name or slug.
    #[arg(value_name = "MISSION")]
    pub mission: String,
    /// Mission type (software-dev, research, ...).
    #[arg(long, value_name = "TYPE")]
    pub mission_type: Option<String>,
    /// Deprecated alias for --mission-type.
    #[arg(long = "mission", hide = true)]
    pub mission_alias: Option<String>,
    /// Create-time topology: single_branch | lanes | coord | lanes_with_coord.
    #[arg(long, value_name = "TOPOLOGY")]
    pub topology: Option<String>,
    /// Mark the mission PR-bound.
    #[arg(long)]
    pub pr_bound: bool,
    /// Human-friendly mission title.
    #[arg(long)]
    pub friendly_name: Option<String>,
    /// One-line stakeholder TLDR.
    #[arg(long)]
    pub purpose_tldr: Option<String>,
    /// Short stakeholder-facing paragraph.
    #[arg(long)]
    pub purpose_context: Option<String>,
    /// Target branch (defaults to the current branch).
    #[arg(long)]
    pub target_branch: Option<String>,
    /// Create even when a live prior mission shares slug and type.
    #[arg(long, alias = "allow-dup")]
    pub allow_duplicate: bool,
    /// Opt branches out of post-merge cleanup.
    #[arg(long)]
    pub retain_branches: bool,
    /// Opt worktrees out of post-merge cleanup.
    #[arg(long)]
    pub retain_worktrees: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `plan` options.
#[derive(Debug, Args)]
pub struct PlanArgs {
    /// Mission slug.
    #[arg(long)]
    pub mission: Option<String>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `tasks` options.
#[derive(Debug, Args)]
pub struct TasksArgs {
    /// Mission slug.
    #[arg(long)]
    pub mission: Option<String>,
    /// Validate without writing.
    #[arg(long)]
    pub validate_only: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `implement` options.
#[derive(Debug, Args)]
pub struct ImplementArgs {
    /// Work package ID.
    #[arg(value_name = "WP_ID")]
    pub wp_id: String,
    /// Mission slug (required).
    #[arg(long)]
    pub mission: Option<String>,
    /// Explicit base ref for the lane workspace.
    #[arg(long)]
    pub base: Option<String>,
    /// Recover an interrupted allocation instead of allocating.
    #[arg(long)]
    pub recover: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `next` options.
#[derive(Debug, Args)]
pub struct NextArgs {
    /// Agent name (required when advancing).
    #[arg(long)]
    pub agent: Option<String>,
    /// Result of the previous step: success|failed|blocked.
    #[arg(long)]
    pub result: Option<String>,
    /// Mission slug.
    #[arg(long)]
    pub mission: Option<String>,
    /// Answer a pending decision.
    #[arg(long)]
    pub answer: Option<String>,
    /// Decision id when several are pending.
    #[arg(long)]
    pub decision_id: Option<String>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `upgrade` options (accepted so the refusal names the real surface).
#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
    /// Agent upgrade check mode.
    #[arg(long)]
    pub agent_check: bool,
}

/// `capabilities` options.
#[derive(Debug, Args)]
pub struct JsonArgs {
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Unsupported route arguments.
#[derive(Debug, Args)]
pub struct UnsupportedArgs {
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
    /// Remaining arguments are refused verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// `agent` subcommands.
#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Canonical status management.
    Status {
        #[command(subcommand)]
        command: AgentStatusCommand,
    },
    /// Work-package reads and finalization.
    Tasks {
        #[command(subcommand)]
        command: AgentTasksCommand,
    },
    /// Mission lifecycle commands.
    Mission {
        #[command(subcommand)]
        command: AgentMissionCommand,
    },
}

/// `agent status` subcommands.
#[derive(Debug, Subcommand)]
pub enum AgentStatusCommand {
    /// Validate canonical status model integrity.
    Validate(MissionJsonArgs),
    /// Rebuild status.json from the canonical event log.
    Materialize(MissionJsonArgs),
    /// Emit a canonical status transition.
    Emit(Box<status::EmitArgs>),
    /// Show canonical mission lifecycle state.
    Lifecycle(MissionJsonArgs),
}

/// `agent tasks` subcommands.
#[derive(Debug, Subcommand)]
pub enum AgentTasksCommand {
    /// List tasks with optional lane filtering.
    #[command(name = "list-tasks")]
    ListTasks {
        /// Filter by lane.
        #[arg(long)]
        lane: Option<String>,
        /// Mission slug (required).
        #[arg(long)]
        mission: Option<String>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Display the kanban status board.
    Status {
        /// Mission slug (required).
        #[arg(long)]
        mission: Option<String>,
        /// Minutes of inactivity before a WP is considered stale.
        #[arg(long, default_value_t = 10)]
        stale_threshold: i64,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Find downstream dependents of a WP.
    #[command(name = "list-dependents")]
    ListDependents {
        /// Work package ID.
        #[arg(value_name = "WP_ID")]
        wp_id: String,
        /// Mission slug (required).
        #[arg(long)]
        mission: Option<String>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Finalize tasks.md dependencies into WP frontmatter.
    #[command(name = "finalize-tasks")]
    FinalizeTasks {
        /// Mission slug (required).
        #[arg(long)]
        mission: Option<String>,
        /// Validate without writing.
        #[arg(long)]
        validate_only: bool,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Move a work package between lanes.
    #[command(name = "move-task")]
    MoveTask(Box<tasks::MoveTaskArgs>),
    /// Record subtask status in the canonical event log.
    #[command(name = "mark-status")]
    MarkStatus(tasks::MarkStatusArgs),
}

/// `agent mission` subcommands.
#[derive(Debug, Subcommand)]
pub enum AgentMissionCommand {
    /// Create a new mission scaffold.
    Create(MissionCreateArgs),
    /// Scaffold plan.md.
    #[command(name = "setup-plan")]
    SetupPlan(PlanArgs),
    /// Return the deterministic branch contract for planning prompts.
    #[command(name = "branch-context")]
    BranchContext {
        /// Planned landing branch (defaults to the current branch).
        #[arg(long)]
        target_branch: Option<String>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Finalize tasks dependencies.
    #[command(name = "finalize-tasks")]
    FinalizeTasks {
        /// Deprecated selector name kept for compatibility.
        #[arg(long = "feature", hide = true)]
        feature: Option<String>,
        /// Mission slug (required).
        #[arg(long)]
        mission: Option<String>,
        /// Validate without writing.
        #[arg(long)]
        validate_only: bool,
        /// Override the recorded target branch (refused before mutation).
        #[arg(long)]
        target_branch: Option<String>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `agent mission create` options.
#[derive(Debug, Args)]
pub struct MissionCreateArgs {
    /// Mission slug.
    #[arg(value_name = "MISSION_SLUG")]
    pub mission_slug: String,
    /// Mission type.
    #[arg(long, value_name = "TYPE")]
    pub mission_type: Option<String>,
    /// Deprecated alias for --mission-type.
    #[arg(long, hide = true)]
    pub mission: Option<String>,
    /// Target branch (defaults to the current branch).
    #[arg(long)]
    pub target_branch: Option<String>,
    /// Human-friendly mission title.
    #[arg(long)]
    pub friendly_name: Option<String>,
    /// One-line stakeholder TLDR.
    #[arg(long)]
    pub purpose_tldr: Option<String>,
    /// Short stakeholder-facing paragraph.
    #[arg(long)]
    pub purpose_context: Option<String>,
    /// Mark the mission PR-bound.
    #[arg(long)]
    pub pr_bound: bool,
    /// Create-time topology.
    #[arg(long, value_name = "TOPOLOGY")]
    pub topology: Option<String>,
    /// Opt branches out of post-merge cleanup.
    #[arg(long)]
    pub retain_branches: bool,
    /// Opt worktrees out of post-merge cleanup.
    #[arg(long)]
    pub retain_worktrees: bool,
    /// Create even when a live prior shares slug and type.
    #[arg(long, alias = "allow-dup")]
    pub allow_duplicate: bool,
    /// Unsupported: branch-strategy gate control.
    #[arg(long, hide = true)]
    pub branch_strategy: Option<String>,
    /// Unsupported: create/switch a start branch before writing.
    #[arg(long, hide = true)]
    pub start_branch: Option<String>,
    /// Unsupported: recreate a diverged coordination branch.
    #[arg(long, hide = true)]
    pub force_recreate_coordination_branch: bool,
    /// Unsupported: commit directly to a protected target.
    #[arg(long, hide = true)]
    pub commit_to_target: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Mission + JSON arguments.
#[derive(Debug, Args)]
pub struct MissionJsonArgs {
    /// Mission slug (required).
    #[arg(long)]
    pub mission: Option<String>,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `runtime` subcommands (native extension).
#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    /// Start or reuse the mission's governed run.
    Start {
        #[arg(long)]
        mission: Option<String>,
        #[arg(long)]
        template: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Read the run without mutating it.
    Query {
        #[arg(long)]
        mission: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Advance the run with a step result.
    Advance {
        #[arg(long)]
        mission: Option<String>,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        result: String,
        #[arg(long)]
        expected_issued_step: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Answer a pending decision.
    Answer {
        #[arg(long)]
        mission: Option<String>,
        #[arg(long)]
        decision_id: String,
        #[arg(long)]
        answer: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        json: bool,
    },
    /// Reconcile the run cursor and journal.
    Recover {
        #[arg(long)]
        mission: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// `workspace` subcommands (native extension).
#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Reconcile interrupted lane allocations.
    Recover {
        #[arg(long)]
        mission: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Run the compatibility CLI.
pub fn main(arguments: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut arguments: Vec<OsString> = arguments.into_iter().collect();
    let json_requested = arguments.iter().any(|argument| argument == "--json");

    // `native` is a direct pass-through to the versioned inspection tree.
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "native")
    {
        let program = arguments
            .first()
            .cloned()
            .unwrap_or_else(|| OsString::from("spec-kitty-rust"));
        let mut forwarded = vec![program];
        forwarded.extend(arguments.drain(2..));
        return crate::native_main(forwarded);
    }

    // Upstream uses `-v` for the version. Clap's derive flag is `--version`.
    if arguments.len() == 2 && arguments[1] == "-v" {
        arguments[1] = OsString::from("--version");
    }

    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error, json_requested),
    };

    match dispatch(cli.command) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if json_requested {
                let _ = emit_json(&error.to_json());
            }
            eprintln!("Error: {}", error.message());
            ExitCode::from(error.exit())
        }
    }
}

fn handle_parse_error(error: clap::Error, json_requested: bool) -> ExitCode {
    use clap::error::ErrorKind;

    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        _ => {
            let message = error.to_string();
            let message = message
                .trim()
                .strip_prefix("error: ")
                .unwrap_or(message.trim());
            if json_requested {
                let _ = emit_json(&serde_json::json!({"error": message}));
            }
            eprintln!("Error: {message}");
            ExitCode::from(2)
        }
    }
}

fn unsupported(route: &str, json_output: bool) -> Result<u8, CompatError> {
    let _ = json_output;
    Err(CompatError::unsupported(
        "NOT_IMPLEMENTED",
        format!(
            "{route} is not implemented by the native Rust CLI; see `spec-kitty capabilities --json` for the blocker and implemented routes"
        ),
    ))
}

/// Dispatch one parsed command.
pub fn dispatch(command: Command) -> Result<u8, CompatError> {
    match command {
        Command::Init(args) => {
            init::init(
                args.project_name.as_deref(),
                args.ai.as_deref(),
                args.non_interactive,
            )?;
            Ok(0)
        }
        Command::Specify(args) => {
            let root = support::project_root()?;
            let mission_type = args.mission_type.or(args.mission_alias);
            let topology = match args.topology.as_deref() {
                Some(value) => Some(mission::TopologyChoice::parse(value)?),
                None => None,
            };
            let mut options = mission::CreateOptions::new(&args.mission);
            options.mission_type = mission_type.as_deref();
            options.target_branch = args.target_branch.as_deref();
            options.friendly_name = args.friendly_name.as_deref();
            options.purpose_tldr = args.purpose_tldr.as_deref();
            options.purpose_context = args.purpose_context.as_deref();
            options.topology = topology;
            options.pr_bound = args.pr_bound;
            options.retain_branches = args.retain_branches;
            options.retain_worktrees = args.retain_worktrees;
            options.allow_duplicate = args.allow_duplicate;
            let value = mission::specify(&root, &options)?;
            if args.json {
                emit_json(&value).map_err(|error| {
                    CompatError::domain(format!("could not write JSON: {error}"))
                })?;
            } else {
                println!(
                    "✓ Mission created: {}",
                    value["mission_slug"].as_str().unwrap_or("")
                );
                println!(
                    "   Directory: {}",
                    value["mission_dir"].as_str().unwrap_or("")
                );
                println!(
                    "Scaffold only: spec.md was created as a scaffold; no complete spec was authored."
                );
                println!("Next: {}", value["next_action"].as_str().unwrap_or(""));
            }
            Ok(0)
        }
        Command::Plan(args) => {
            let root = support::project_root()?;
            mission::plan(&root, args.mission.as_deref(), args.json)?;
            Ok(0)
        }
        Command::Tasks(args) => {
            let root = support::project_root()?;
            mission::finalize_tasks(
                &root,
                args.mission.as_deref(),
                args.validate_only,
                args.json,
            )?;
            Ok(0)
        }
        Command::Implement(args) => {
            if args.recover {
                let root = support::project_root()?;
                workspace::recover(&root, args.mission.as_deref(), args.json)?;
                return Ok(0);
            }
            let root = support::project_root()?;
            workspace::implement(
                &root,
                args.mission.as_deref(),
                &args.wp_id,
                args.base.as_deref(),
                args.json,
            )?;
            Ok(0)
        }
        Command::Next(args) => {
            let code = runtime::next(
                args.mission.as_deref(),
                args.agent.as_deref(),
                args.result.as_deref(),
                args.answer.as_deref(),
                args.decision_id.as_deref(),
                args.json,
            )?;
            Ok(code as u8)
        }
        Command::Upgrade(args) => {
            if args.agent_check {
                return unsupported("upgrade --agent-check", args.json);
            }
            unsupported("upgrade", args.json)
        }
        Command::Capabilities(args) => {
            let value = capabilities::matrix();
            if args.json {
                emit_json(&value).map_err(|error| {
                    CompatError::domain(format!("could not write JSON: {error}"))
                })?;
            } else {
                println!(
                    "compatibility matrix: {} implemented/partial routes, {} unsupported",
                    value["commands"].as_array().map(Vec::len).unwrap_or(0),
                    value["unsupported"].as_array().map(Vec::len).unwrap_or(0)
                );
                for command in value["commands"].as_array().into_iter().flatten() {
                    println!(
                        "  {:<32} {:<12} {}",
                        command["name"].as_str().unwrap_or(""),
                        command["state"].as_str().unwrap_or(""),
                        command["surface"].as_str().unwrap_or("")
                    );
                }
            }
            Ok(0)
        }
        Command::Native { args } => {
            let mut forwarded = vec![OsString::from("spec-kitty-rust")];
            forwarded.extend(args.into_iter().map(OsString::from));
            let code = crate::native_main(forwarded);
            Ok(code_value(code))
        }
        Command::Agent { command } => match command {
            AgentCommand::Status { command } => match command {
                AgentStatusCommand::Validate(args) => {
                    let root = support::project_root()?;
                    let passed = status::validate(&root, args.mission.as_deref(), args.json)?;
                    Ok(if passed { 0 } else { 1 })
                }
                AgentStatusCommand::Materialize(args) => {
                    let root = support::project_root()?;
                    status::materialize(&root, args.mission.as_deref(), args.json)?;
                    Ok(0)
                }
                AgentStatusCommand::Emit(args) => {
                    let root = support::project_root()?;
                    status::emit(&root, &args)?;
                    Ok(0)
                }
                AgentStatusCommand::Lifecycle(args) => {
                    let root = support::project_root()?;
                    status::lifecycle(&root, args.mission.as_deref(), args.json)?;
                    Ok(0)
                }
            },
            AgentCommand::Tasks { command } => match command {
                AgentTasksCommand::ListTasks {
                    lane,
                    mission,
                    json,
                } => {
                    let root = support::project_root()?;
                    tasks::list_tasks(&root, mission.as_deref(), lane.as_deref(), json)?;
                    Ok(0)
                }
                AgentTasksCommand::Status {
                    mission,
                    stale_threshold: _,
                    json,
                } => {
                    let root = support::project_root()?;
                    tasks::status(&root, mission.as_deref(), json)?;
                    Ok(0)
                }
                AgentTasksCommand::ListDependents {
                    wp_id,
                    mission,
                    json,
                } => {
                    let root = support::project_root()?;
                    tasks::list_dependents(&root, mission.as_deref(), &wp_id, json)?;
                    Ok(0)
                }
                AgentTasksCommand::FinalizeTasks {
                    mission,
                    validate_only,
                    json,
                } => {
                    let root = support::project_root()?;
                    mission_finalize(&root, mission.as_deref(), validate_only, json)?;
                    Ok(0)
                }
                AgentTasksCommand::MoveTask(args) => {
                    let root = support::project_root()?;
                    tasks::move_task(&root, &args)?;
                    Ok(0)
                }
                AgentTasksCommand::MarkStatus(args) => {
                    let root = support::project_root()?;
                    tasks::mark_status(&root, &args)?;
                    Ok(0)
                }
            },
            AgentCommand::Mission { command } => match command {
                AgentMissionCommand::Create(args) => {
                    let root = support::project_root()?;
                    let mission_type = args.mission_type.or(args.mission);
                    let topology = match args.topology.as_deref() {
                        Some(value) => Some(mission::TopologyChoice::parse(value)?),
                        None => None,
                    };
                    let mut options = mission::CreateOptions::new(&args.mission_slug);
                    options.mission_type = mission_type.as_deref();
                    options.target_branch = args.target_branch.as_deref();
                    options.friendly_name = args.friendly_name.as_deref();
                    options.purpose_tldr = args.purpose_tldr.as_deref();
                    options.purpose_context = args.purpose_context.as_deref();
                    options.topology = topology;
                    options.pr_bound = args.pr_bound;
                    options.retain_branches = args.retain_branches;
                    options.retain_worktrees = args.retain_worktrees;
                    options.allow_duplicate = args.allow_duplicate;
                    if args.branch_strategy.is_some() {
                        options.unsupported.push((
                            "--branch-strategy",
                            "branch-strategy gate is not integrated".to_owned(),
                        ));
                    }
                    if args.start_branch.is_some() {
                        options.unsupported.push((
                            "--start-branch",
                            "pre-create branch switching is not integrated".to_owned(),
                        ));
                    }
                    if args.force_recreate_coordination_branch {
                        options.unsupported.push((
                            "--force-recreate-coordination-branch",
                            "coordination branch minting is not integrated".to_owned(),
                        ));
                    }
                    if args.commit_to_target {
                        options.unsupported.push((
                            "--commit-to-target",
                            "direct target commits are not integrated".to_owned(),
                        ));
                    }
                    let value = mission::create(&root, &options)?;
                    if args.json {
                        emit_json(&value).map_err(|error| {
                            CompatError::domain(format!("could not write JSON: {error}"))
                        })?;
                    } else {
                        println!(
                            "✓ Mission created: {}",
                            value["mission_slug"].as_str().unwrap_or("")
                        );
                        println!(
                            "   Directory: {}",
                            value["mission_dir"].as_str().unwrap_or("")
                        );
                    }
                    Ok(0)
                }
                AgentMissionCommand::SetupPlan(args) => {
                    let root = support::project_root()?;
                    mission::plan(&root, args.mission.as_deref(), args.json)?;
                    Ok(0)
                }
                AgentMissionCommand::BranchContext {
                    target_branch,
                    json,
                } => {
                    let root = support::project_root()?;
                    mission::branch_context(&root, target_branch.as_deref(), json)?;
                    Ok(0)
                }
                AgentMissionCommand::FinalizeTasks {
                    feature,
                    mission,
                    validate_only,
                    target_branch,
                    json,
                } => {
                    if target_branch.is_some() {
                        return Err(CompatError::unsupported(
                            "UNSUPPORTED_REQUIRED_OPTION",
                            "--target-branch finalize override is not integrated; refusing before writing",
                        ));
                    }
                    let root = support::project_root()?;
                    let handle = mission.or(feature);
                    mission::finalize_tasks(&root, handle.as_deref(), validate_only, json)?;
                    Ok(0)
                }
            },
        },
        Command::Runtime { command } => match command {
            RuntimeCommand::Start {
                mission,
                template,
                json,
            } => {
                runtime::start(mission.as_deref(), template.as_deref(), json)?;
                Ok(0)
            }
            RuntimeCommand::Query { mission, json } => {
                runtime::query(mission.as_deref(), json)?;
                Ok(0)
            }
            RuntimeCommand::Advance {
                mission,
                agent,
                result,
                expected_issued_step,
                json,
            } => {
                runtime::advance(
                    mission.as_deref(),
                    &agent,
                    &result,
                    expected_issued_step.as_deref(),
                    json,
                )?;
                Ok(0)
            }
            RuntimeCommand::Answer {
                mission,
                decision_id,
                answer,
                actor,
                json,
            } => {
                runtime::answer(mission.as_deref(), &decision_id, &answer, &actor, json)?;
                Ok(0)
            }
            RuntimeCommand::Recover { mission, json } => {
                runtime::recover(mission.as_deref(), json)?;
                Ok(0)
            }
        },
        Command::Workspace { command } => match command {
            WorkspaceCommand::Recover { mission, json } => {
                let root = support::project_root()?;
                workspace::recover(&root, mission.as_deref(), json)?;
                Ok(0)
            }
        },
        Command::Consolidate(args) => unsupported("consolidate", args.json),
        Command::Accept(args) => unsupported("accept", args.json),
        Command::Review(args) => unsupported("review", args.json),
        Command::Merge(args) => unsupported("merge", args.json),
    }
}

fn mission_finalize(
    root: &std::path::Path,
    handle: Option<&str>,
    validate_only: bool,
    json: bool,
) -> Result<(), CompatError> {
    mission::finalize_tasks(root, handle, validate_only, json)?;
    Ok(())
}

fn code_value(code: ExitCode) -> u8 {
    if code == ExitCode::SUCCESS { 0 } else { 1 }
}
