//! `sk-cli`: the Spec Kitty command-line adapters.
//!
//! Two binaries are thin shells over this library:
//!
//! - `spec-kitty` is the upstream-shaped compatibility surface. It routes the
//!   canonical command names, options and JSON payloads that generated agent
//!   prompts invoke. Commands are implemented only where a domain crate owns
//!   real behavior; every other route is refused before mutation.
//! - `spec-kitty-rust` is the original native inspection surface (the
//!   versioned envelope plus `project`, `git`, `mission`, `charter`,
//!   `glossary`, `runtime query`, `events` and `transition` reads). The
//!   compatibility binary also serves it under its `native` subcommand.
//!
//! Neither binary invokes Python at run time.

#![forbid(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod compat;
pub mod output;

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{
    CharterCommand, Cli, Command, EventsCommand, GitCommand, GlossaryCommand, MissionCommand,
    ProjectCommand, RuntimeCommand, TransitionCommand,
};
use crate::output::{CommandData, Success, emit_failure, emit_success};

/// Run the native inspection command tree.
pub fn native_main(arguments: impl IntoIterator<Item = OsString>) -> ExitCode {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let json_requested = arguments.iter().any(|argument| argument == "--json");
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error, json_requested),
    };

    let command_name = cli.command.name();
    let Cli {
        project_root,
        command,
        ..
    } = cli;

    match dispatch(project_root.as_deref(), command) {
        Ok(data) => {
            let exit_code = emit_success(&Success {
                command: command_name,
                data: data.data,
                exit_code: data.exit_code,
            });
            ExitCode::from(exit_code)
        }
        Err(error) => ExitCode::from(emit_failure(command_name, &error)),
    }
}

fn dispatch(root: Option<&Path>, command: Command) -> Result<CommandData, output::CliError> {
    match command {
        Command::Capabilities => Ok(commands::capabilities::report()),
        Command::Project {
            command: ProjectCommand::Root,
        } => commands::project::root(root),
        Command::Git {
            command: GitCommand::Inspect,
        } => commands::git::inspect(root),
        Command::Mission { command } => match command {
            MissionCommand::List => commands::mission::list(root),
            MissionCommand::Show { selector } => commands::mission::show(root, &selector),
            MissionCommand::Graph { selector } => commands::mission::graph(root, &selector),
        },
        Command::Charter {
            command: CharterCommand::Inspect { built_in },
        } => commands::charter::inspect(root, built_in),
        Command::Glossary { command } => match command {
            GlossaryCommand::Lookup {
                term,
                scopes,
                seed_files,
                confidence,
                critical_step,
            } => commands::glossary::lookup(
                root,
                &term,
                &scopes,
                &seed_files,
                confidence,
                critical_step,
            ),
        },
        Command::Runtime { command } => match command {
            RuntimeCommand::Query {
                template,
                state,
                current_template_hash,
            } => commands::runtime::query(&template, &state, current_template_hash.as_deref()),
        },
        Command::Events { command } => match command {
            EventsCommand::Inspect { path } => commands::events::inspect(&path),
        },
        Command::Transition { command } => match *command {
            TransitionCommand::Check(args) => commands::transition::check(&args),
        },
    }
}

fn handle_parse_error(error: clap::Error, json_requested: bool) -> ExitCode {
    use clap::error::ErrorKind;

    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            if json_requested {
                let kind = if error.kind() == ErrorKind::DisplayHelp {
                    "help"
                } else {
                    "version"
                };
                let command = if kind == "help" {
                    "cli.help"
                } else {
                    "cli.version"
                };
                ExitCode::from(emit_success(&Success {
                    command,
                    data: serde_json::json!({"format": kind, "text": error.to_string()}),
                    exit_code: output::EXIT_SUCCESS,
                }))
            } else {
                let _ = error.print();
                ExitCode::SUCCESS
            }
        }
        _ => {
            let message = error.to_string();
            let message = message
                .trim()
                .strip_prefix("error: ")
                .unwrap_or(message.trim());
            let failure = output::CliError::usage(message.to_owned());
            ExitCode::from(emit_failure("cli", &failure))
        }
    }
}
