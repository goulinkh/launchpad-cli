use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value, json};

use crate::commands::{COMMANDS, CommandSpec};
use crate::error::Error;
use crate::request::Request;
use crate::result::Result;

const BOOL_FIELDS: &[&str] = &[
    "current_diff_only",
    "unresolved_only",
    "latest",
    "include_superseded",
    "unassign",
    "wait_for_index",
    "wait_for_preview",
    "needs_review",
    "force_with_lease",
];
const NUMBER_FIELDS: &[&str] = &[
    "limit",
    "preview_diff_id",
    "file_line",
    "bug_id",
    "index_timeout_seconds",
    "preview_timeout_seconds",
];
const LIST_FIELDS: &[&str] = &["status", "importance", "tags", "official_bug_tags"];

pub fn command() -> Command {
    let mut root = Command::new("launchpad-cli").version(env!("CARGO_PKG_VERSION"))
        .about("Standalone Launchpad tools for people and agents")
        .after_help("Discover: launchpad-cli schema\nAPI coverage: launchpad-cli api operations\nWrites require --yes. --dry-run validates without contacting Launchpad.\nLaunchpad bugs and target-specific bug tasks are distinct; merge proposals are not pull requests.")
        .subcommand_required(true)
        .arg(Arg::new("json").long("json").global(true).action(ArgAction::SetTrue).conflicts_with("text").help("Emit a versioned JSON envelope (default when piped)"))
        .arg(Arg::new("text").long("text").global(true).action(ArgAction::SetTrue).help("Emit plain text or Markdown"))
        .arg(Arg::new("yes").long("yes").global(true).action(ArgAction::SetTrue).help("Explicitly authorise this invocation's side effects"))
        .arg(Arg::new("dry-run").long("dry-run").global(true).action(ArgAction::SetTrue).help("Validate and print the request; do not execute"));
    let groups: std::collections::BTreeSet<_> = COMMANDS.iter().map(|spec| spec.group).collect();
    for group in groups {
        let mut noun = Command::new(group)
            .subcommand_required(true)
            .about(format!("Launchpad {group} operations"));
        for spec in COMMANDS.iter().filter(|spec| spec.group == group) {
            let mut verb = Command::new(spec.action)
                .about(spec.summary)
                .arg(input_arg());
            if let Some(field) = spec.positional {
                verb = verb.arg(
                    Arg::new("identifier")
                        .value_name(field.to_ascii_uppercase())
                        .index(1),
                );
            }
            for field in spec.fields {
                let mut arg = Arg::new(*field)
                    .long(field.replace('_', "-"))
                    .help(format!("Request field: {field}; see launchpad-cli schema"));
                if spec.positional == Some(*field) {
                    arg = arg.conflicts_with("identifier");
                }
                if BOOL_FIELDS.contains(field) {
                    arg = arg
                        .num_args(0..=1)
                        .require_equals(true)
                        .default_missing_value("true")
                        .value_parser(["true", "false"]);
                } else if LIST_FIELDS.contains(field) {
                    arg = arg.action(ArgAction::Append);
                }
                verb = verb.arg(arg);
            }
            noun = noun.subcommand(verb);
        }
        root = root.subcommand(noun);
    }
    root.subcommand(Command::new("schema").about("Print the offline CLI command catalog and JSON input schema"))
        .subcommand(Command::new("tool").about("Call a copied tool directly using its original JSON operation name").arg(input_arg().required(true)))
        .subcommand(Command::new("auth").subcommand_required(true)
            .subcommand(Command::new("status").about("Inspect local credentials without revealing secrets"))
            .subcommand(Command::new("login").about("Authorise launchpad-cli with Launchpad OAuth")
                .arg(Arg::new("start").long("start").action(ArgAction::SetTrue).conflicts_with("finish"))
                .arg(Arg::new("finish").long("finish").action(ArgAction::SetTrue)))
            .subcommand(Command::new("import").about("Import launchpad-cli OAuth credentials from JSON").arg(input_arg().required(true)))
            .subcommand(Command::new("logout").about("Remove this CLI's local credentials")))
        .subcommand(Command::new("api").subcommand_required(true)
            .subcommand(Command::new("operations").about("List converter-owned semantic operation IDs").arg(Arg::new("filter").long("filter")))
            .subcommand(Command::new("schema").about("Print the authoritative OpenAPI document or component schema").arg(Arg::new("component")))
            .subcommand(Command::new("decode").about("Decode JSON with a generated Rust response type").arg(Arg::new("component").required(true)).arg(input_arg().required(true)))
            .subcommand(Command::new("call").about("Call an OpenAPI operation ID; template and body fields come from --input JSON")
                .arg(Arg::new("operation").required(true)).arg(input_arg())))
}

pub fn catalog() -> Value {
    json!({
        "commands": COMMANDS,
        "request_schema": schemars::schema_for!(Request),
        "input": "--input FILE or --input - (stdin); high-level commands omit op",
        "output": { "schema_version": 1, "success": { "ok": true, "data": "command-specific object" }, "failure": { "ok": false, "error": { "code": "stable identifier", "message": "description" } } },
        "exit_codes": { "0": "success", "1": "transport or runtime error", "2": "invalid input or missing --yes", "3": "authentication required/rejected", "4": "permission denied", "5": "not found" },
        "safety": "All remote writes, Git pushes, and local checkout writes require --yes. --dry-run performs no network or Git activity.",
        "api": "api operations and api schema expose the converter-owned API contract; api call accepts an operation ID and --input with params and body"
    })
}

pub fn build_request(spec: &CommandSpec, matches: &ArgMatches) -> Result<Request> {
    let mut fields = input_object(matches)?;
    for name in fields.keys() {
        if !spec.fields.contains(&name.as_str()) {
            return Err(Error::invalid(format!(
                "field {name} is not supported by {} {}",
                spec.group, spec.action
            )));
        }
    }
    if let Some(field) = spec.positional {
        if let Some(identifier) = matches.get_one::<String>("identifier") {
            insert(&mut fields, field, json!(identifier))?;
        }
    }
    for field in spec.fields {
        let Some(values) = matches.get_many::<String>(field) else {
            continue;
        };
        let values: Vec<_> = values.cloned().collect();
        let value = if BOOL_FIELDS.contains(field) {
            json!(values[0] == "true")
        } else if NUMBER_FIELDS.contains(field) {
            json!(
                values[0]
                    .parse::<u64>()
                    .map_err(|_| Error::invalid(format!(
                        "{field} must be a nonnegative integer"
                    )))?
            )
        } else if matches!(*field, "tags" | "official_bug_tags") || values.len() > 1 {
            json!(values)
        } else {
            json!(values[0])
        };
        insert(&mut fields, field, value)?;
    }
    for required in spec.required {
        if !fields.contains_key(*required) {
            return Err(Error::invalid(format!("{required} is required")));
        }
    }
    normalise_identifier(spec, &mut fields)?;
    fields.insert("op".to_owned(), json!(spec.operation));
    let request: Request = serde_json::from_value(Value::Object(fields))
        .map_err(|source| Error::invalid(source.to_string()))?;
    request.validate()?;
    Ok(request)
}

pub fn read_input(path: &str) -> Result<Value> {
    let bytes = if path == "-" {
        let mut bytes = Vec::new();
        io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| Error::Io {
                path: PathBuf::from("stdin"),
                source,
            })?;
        bytes
    } else {
        let file = fs::File::open(path).map_err(|source| Error::Io {
            path: PathBuf::from(path),
            source,
        })?;
        let mut bytes = Vec::new();
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| Error::Io {
                path: PathBuf::from(path),
                source,
            })?;
        bytes
    };
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(Error::invalid("JSON input exceeds 4 MiB"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|source| Error::invalid(format!("cannot decode input JSON: {source}")))
}

pub fn input_object(matches: &ArgMatches) -> Result<Map<String, Value>> {
    let Some(path) = matches.get_one::<String>("input") else {
        return Ok(Map::new());
    };
    let value = read_input(path)?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Error::invalid("input must be a JSON object"))
}

fn input_arg() -> Arg {
    Arg::new("input")
        .long("input")
        .value_name("FILE|-")
        .help("Read JSON from a file or stdin; rejects unknown or duplicated fields")
}

fn insert(fields: &mut Map<String, Value>, field: &str, value: Value) -> Result<()> {
    if fields.insert(field.to_owned(), value).is_some() {
        return Err(Error::invalid(format!(
            "{field} was supplied more than once"
        )));
    }
    Ok(())
}

fn normalise_identifier(spec: &CommandSpec, fields: &mut Map<String, Value>) -> Result<()> {
    if let Some(Value::String(target)) = fields.get_mut("target") {
        if spec.group == "bug"
            && spec.action != "search"
            && spec.action != "create"
            && target.bytes().all(|byte| byte.is_ascii_digit())
        {
            *target = format!("lp://bugs/{target}");
        }
        if spec.group == "merge-proposal" && spec.action == "diff" && !target.contains("/diff") {
            *target = format!("{}/diff", target.trim_end_matches('/'));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_is_valid() {
        command().debug_assert();
    }

    #[test]
    fn preserves_launchpad_statuses_and_repeated_filters() {
        let parsed = command()
            .try_get_matches_from([
                "launchpad-cli",
                "bug",
                "search",
                "ubuntu",
                "--status",
                "New",
                "--status",
                "Confirmed",
                "--limit",
                "2",
            ])
            .unwrap();
        let (_, group) = parsed.subcommand().unwrap();
        let (_, verb) = group.subcommand().unwrap();
        let spec = COMMANDS
            .iter()
            .find(|spec| spec.group == "bug" && spec.action == "search")
            .unwrap();
        let request = build_request(spec, verb).unwrap();
        assert_eq!(request.limit, Some(2));
        assert_eq!(
            request.status.unwrap().values().collect::<Vec<_>>(),
            ["New", "Confirmed"]
        );
    }

    #[test]
    fn every_original_tool_operation_has_a_command() {
        let schema = serde_json::to_value(schemars::schema_for!(Request)).unwrap();
        let operations = schema["$defs"]["Operation"]["enum"].as_array().unwrap();
        for operation in operations {
            assert!(
                COMMANDS
                    .iter()
                    .any(|spec| Some(spec.operation) == operation.as_str()),
                "{operation}"
            );
        }
    }
}
