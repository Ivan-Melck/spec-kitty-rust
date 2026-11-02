//! `spec-kitty-rust`: a partial, read-only Rust rewrite of the Spec Kitty CLI.
//!
//! This binary is an adapter over the domain crates. It calls implemented
//! behavior; it does not simulate absent behavior. Every command emits exactly
//! one JSON document on stdout and diagnostics on stderr.

mod cli;
mod commands;
mod output;

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{CharterCommand, Cli, Command, GlossaryCommand, MissionCommand, RuntimeCommand};
use crate::cli::{EventsCommand, GitCommand, ProjectCommand, TransitionCommand};
use crate::output::{CommandData, Success, emit_failure, emit_success};

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error),
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

fn handle_parse_error(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;

    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        _ => {
            // Usage failures still get exactly one JSON document on stdout.
            // The clap diagnostic goes to stderr through `emit_failure`.
            let message = error.to_string();
            let failure = output::CliError::usage(message.trim().to_owned());
            ExitCode::from(emit_failure("cli", &failure))
        }
    }
}
