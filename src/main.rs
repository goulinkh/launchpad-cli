mod api;
mod auth;
mod cli;
mod client;
mod commands;
mod diff;
mod error;
mod git_file;
mod launchpad;
mod local_git;
mod render;
mod request;
mod response;
mod result;

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::ArgMatches;
use serde_json::{Value, json};

use crate::commands::COMMANDS;
use crate::error::Error;
use crate::request::Request;
use crate::result::Result;

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args().collect();
    let json_output = arguments.iter().any(|argument| argument == "--json")
        || (!io::stdout().is_terminal() && !arguments.iter().any(|argument| argument == "--text"));
    let matches = match cli::command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(source) if !source.use_stderr() => {
            let _ = source.print();
            return ExitCode::SUCCESS;
        }
        Err(source) => {
            let error = Error::invalid(source.to_string());
            emit_error(&error, json_output);
            return ExitCode::from(2);
        }
    };
    match run(&matches).await {
        Ok(data) => {
            if let Err(error) = emit(&data, json_output) {
                if matches!(error, Error::Io { source, .. } if source.kind() == io::ErrorKind::BrokenPipe)
                {
                    return ExitCode::SUCCESS;
                }
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            emit_error(&error, json_output);
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run(matches: &ArgMatches) -> Result<Value> {
    let dry_run = matches.get_flag("dry-run");
    let yes = matches.get_flag("yes");
    let (group, arguments) = matches
        .subcommand()
        .ok_or_else(|| Error::invalid("command is required"))?;
    match group {
        "schema" => Ok(cli::catalog()),
        "auth" => run_auth(arguments, dry_run).await,
        "api" => api::run(arguments, dry_run, yes).await,
        "tool" => {
            let input = cli::read_input(
                arguments
                    .get_one::<String>("input")
                    .ok_or_else(|| Error::invalid("input is required"))?,
            )?;
            let request: Request = serde_json::from_value(input)
                .map_err(|source| Error::invalid(source.to_string()))?;
            execute(request, dry_run, yes).await
        }
        _ => {
            let (action, arguments) = arguments
                .subcommand()
                .ok_or_else(|| Error::invalid("action is required"))?;
            let spec = COMMANDS
                .iter()
                .find(|spec| spec.group == group && spec.action == action)
                .ok_or_else(|| Error::invalid("unknown command"))?;
            execute(cli::build_request(spec, arguments)?, dry_run, yes).await
        }
    }
}

async fn execute(request: Request, dry_run: bool, yes: bool) -> Result<Value> {
    request.validate()?;
    request.op.check_supported()?;
    let operation =
        serde_json::to_value(request.op).map_err(|source| Error::invalid(source.to_string()))?;
    let spec = COMMANDS
        .iter()
        .find(|spec| Some(spec.operation) == operation.as_str())
        .ok_or_else(|| Error::invalid("operation is not exposed"))?;
    if dry_run {
        let mut fields =
            serde_json::to_value(&request).map_err(|source| Error::invalid(source.to_string()))?;
        if let Some(fields) = fields.as_object_mut() {
            fields.retain(|_, value| !value.is_null());
        }
        return Ok(json!({ "request": fields, "effect": spec.effect, "executed": false }));
    }
    if spec.effect != "read" && !yes {
        return Err(Error::invalid(format!(
            "{} requires --yes; inspect it first with --dry-run",
            spec.operation
        )));
    }
    let response = launchpad::execute(&request).await?;
    serde_json::to_value(response).map_err(|source| Error::invalid(source.to_string()))
}

async fn run_auth(arguments: &ArgMatches, dry_run: bool) -> Result<Value> {
    let (action, arguments) = arguments
        .subcommand()
        .ok_or_else(|| Error::invalid("auth action is required"))?;
    if dry_run {
        return Ok(json!({ "auth_action": action, "executed": false }));
    }
    match action {
        "status" => Ok(auth::status()?),
        "logout" => Ok(auth::logout()?),
        "import" => {
            let input = cli::read_input(
                arguments
                    .get_one::<String>("input")
                    .ok_or_else(|| Error::invalid("input is required"))?,
            )?;
            let credentials: auth::Credentials = serde_json::from_value(input)
                .map_err(|source| Error::invalid(source.to_string()))?;
            auth::save_credentials(&credentials)?;
            Ok(auth::status()?)
        }
        "login" if arguments.get_flag("finish") => Ok(auth::login_finish().await?),
        "login" if arguments.get_flag("start") => Ok(auth::login_start().await?),
        "login" => {
            if !io::stdin().is_terminal() {
                return Err(Error::invalid(
                    "noninteractive login requires --start, then --finish after browser authorisation",
                ));
            }
            let pending = auth::login_start().await?;
            eprintln!(
                "Authorise launchpad-cli at {}\nPress Enter after authorising.",
                pending["authorization_url"].as_str().unwrap_or_default()
            );
            let mut line = String::new();
            io::stdin()
                .read_line(&mut line)
                .map_err(|source| Error::Io {
                    path: "stdin".into(),
                    source,
                })?;
            Ok(auth::login_finish().await?)
        }
        _ => Err(Error::invalid("unknown auth action")),
    }
}

fn emit(data: &Value, json_output: bool) -> Result<()> {
    let output = if json_output {
        serde_json::to_string(&json!({ "schema_version": 1, "ok": true, "data": data }))
            .map_err(|source| Error::invalid(source.to_string()))?
    } else if let Some(text) = data["text"].as_str() {
        text.to_owned()
    } else {
        serde_json::to_string_pretty(data).map_err(|source| Error::invalid(source.to_string()))?
    };
    writeln!(io::stdout().lock(), "{output}").map_err(|source| Error::Io {
        path: "stdout".into(),
        source,
    })?;
    Ok(())
}

fn emit_error(error: &Error, json_output: bool) {
    if json_output {
        let mut failure = json!({ "schema_version": 1, "ok": false, "error": { "code": error.stable_code(), "message": error.bridge_message() } });
        if let Some(details) = error.details() {
            failure["error"]["details"] = details;
        }
        // A failed output pipe must not cause a panic or contaminate stderr with JSON.
        let _ = writeln!(io::stdout().lock(), "{failure}");
    } else {
        eprintln!("{}: {}", error.stable_code(), error.bridge_message());
    }
}
