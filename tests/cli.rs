use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::thread;

use serde_json::{Value, json};

fn run(arguments: &[&str], input: Option<&str>) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_launchpad-cli"));
    command
        .args(arguments)
        .env("LAUNCHPAD_CLI_ANONYMOUS", "1")
        .env("LAUNCHPAD_CLI_API_BASE", "http://127.0.0.1:1/devel")
        .env(
            "LAUNCHPAD_CLI_CREDENTIALS",
            directory.path().join("missing.json"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn document(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn unsupported_resubmission_fails_offline_including_dry_runs() {
    for flag in ["--json", "--yes", "--dry-run"] {
        let output = run(
            &[
                "merge-proposal",
                "replace-prerequisite",
                "42",
                "--merge-prerequisite",
                "base",
                flag,
            ],
            None,
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(document(&output)["error"]["code"], "unsupported_operation");
        assert!(
            document(&output)["error"]["message"]
                .as_str()
                .unwrap()
                .contains("No requests were sent")
        );
    }
    let output = run(
        &["tool", "--yes", "--input", "-"],
        Some(
            r#"{"op":"replace_merge_proposal_prerequisite","target":"42","merge_prerequisite":"base"}"#,
        ),
    );
    assert_eq!(document(&output)["error"]["code"], "unsupported_operation");
    let output = run(
        &["tool", "--dry-run", "--input", "-"],
        Some(r#"{"op":"set_merge_proposal_status","target":"42","status":"Superseded"}"#),
    );
    assert_eq!(document(&output)["error"]["code"], "unsupported_operation");
    let output = run(&["schema"], None);
    assert_eq!(
        document(&output)["data"]["unsupported_operations"],
        json!(["replace_merge_proposal_prerequisite"])
    );
}

#[test]
fn ssh_file_reads_are_explicit_and_validate_offline() {
    let output = run(
        &[
            "repository",
            "file",
            "project",
            "--path",
            "README",
            "--transport",
            "ssh",
            "--branch",
            "main",
            "--dry-run",
        ],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(document(&output)["data"]["request"]["transport"], "ssh");
    assert_eq!(document(&output)["data"]["executed"], false);
    for input in [
        json!({"op":"file_read", "repository":"project", "path":"README", "transport":"ftp"}),
        json!({"op":"file_read", "repository":"project", "path":"../secret", "transport":"ssh"}),
        json!({"op":"file_read", "repository":"project", "path":"README", "transport":"ssh", "branch":"--upload-pack=evil"}),
        json!({"op":"file_read", "repository":"project", "path":"README", "transport":"ssh", "branch":" "}),
        json!({"op":"file_read", "repository":"project", "path":"bad\0path", "transport":"ssh"}),
        json!({"op":"repo_view", "repository":"project", "transport":"ssh"}),
    ] {
        let output = run(
            &["tool", "--input", "-", "--dry-run"],
            Some(&input.to_string()),
        );
        assert_eq!(output.status.code(), Some(2), "{input}");
        assert_eq!(document(&output)["error"]["code"], "invalid_request");
    }
}

#[test]
fn development_instance_uses_test_endpoints_without_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_launchpad-cli"))
        .args([
            "api",
            "call",
            "bug-get",
            "--input",
            "-",
            "--dry-run",
            "--json",
        ])
        .env("LAUNCHPAD_CLI_INSTANCE", "development")
        .env_remove("LAUNCHPAD_CLI_API_BASE")
        .env(
            "LAUNCHPAD_CLI_CREDENTIALS",
            directory.path().join("absent.json"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"params":{"id":"16"}}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        document(&output)["data"]["url"],
        "https://api.launchpad.test/devel/bugs/16"
    );
    assert_eq!(document(&output)["data"]["executed"], false);
}

#[test]
fn repository_decode_preserves_nullable_live_fields_and_rejects_wrong_types() {
    let fixture = include_str!("fixtures/git-repository.json");
    let original: Value = serde_json::from_str(fixture).unwrap();
    let arguments = ["api", "decode", "git_repository-full", "--input", "-"];
    let output = run(&arguments, Some(fixture));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(document(&output)["data"], original);

    for count in [
        json!(null),
        json!(12),
        json!("tag:launchpad.net:2008:redacted"),
    ] {
        let mut input = original.clone();
        input["pack_count"] = count;
        input["date_last_scanned"] = Value::Null;
        let output = run(&arguments, Some(&input.to_string()));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(document(&output)["data"], input);
    }
    for (field, value) in [
        ("date_last_repacked", json!(false)),
        ("pack_count", json!("twelve")),
    ] {
        let mut input = original.clone();
        input[field] = value;
        let output = run(&arguments, Some(&input.to_string()));
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            document(&output)["error"]["details"]["instance_path"],
            format!("/{field}")
        );
    }
    let mut input = original;
    input.as_object_mut().unwrap().remove("date_last_repacked");
    assert_eq!(
        run(&arguments, Some(&input.to_string())).status.code(),
        Some(2)
    );
}

#[test]
fn agents_can_discover_commands_offline() {
    let output = run(&["schema"], None);
    assert!(output.status.success());
    let document = document(&output);
    assert_eq!(document["schema_version"], 1);
    assert!(document["data"]["commands"].as_array().unwrap().len() >= 31);
    assert_eq!(
        document["data"]["request_schema"]["additionalProperties"],
        false
    );
}

#[test]
fn writes_need_explicit_consent_and_dry_run_needs_no_credentials() {
    let arguments = ["bug", "edit", "1", "--title", "Updated"];
    let output = run(&arguments, None);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(document(&output)["error"]["code"], "invalid_request");
    let output = run(
        &["--dry-run", "bug", "edit", "1", "--title", "Updated"],
        None,
    );
    assert!(output.status.success());
    assert_eq!(
        document(&output)["data"]["request"]["target"],
        "lp://bugs/1"
    );
    assert_eq!(document(&output)["data"]["executed"], false);
    let output = run(&["--yes", "bug", "edit", "1", "--title", "Updated"], None);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(document(&output)["error"]["code"], "not_authenticated");
}

#[test]
fn rejects_unknown_json_fields_for_both_surfaces() {
    for arguments in [
        vec!["tool", "--input", "-"],
        vec!["bug", "view", "--input", "-"],
    ] {
        let output = run(
            &arguments,
            Some(r#"{"op":"resource_view","target":"lp://bugs/1","invented":true}"#),
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(document(&output)["ok"], false);
    }
}

#[test]
fn rejects_duplicate_flag_and_json_fields() {
    let output = run(
        &[
            "--dry-run",
            "bug",
            "edit",
            "1",
            "--title",
            "Flag",
            "--input",
            "-",
        ],
        Some(r#"{"title":"JSON"}"#),
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        document(&output)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("more than once")
    );
}

const PREVIEW_COMMANDS: [(&str, &str, &[&str]); 5] = [
    ("inline-comments", "inline_comments", &[]),
    ("drafts", "review_drafts", &[]),
    (
        "map-line",
        "diff_line_map",
        &[
            "--path",
            "README.md",
            "--file-line",
            "2",
            "--side",
            "modified",
        ],
    ),
    (
        "draft",
        "review_draft_update",
        &[
            "--path",
            "README.md",
            "--file-line",
            "2",
            "--side",
            "modified",
            "--body",
            "New review",
        ],
    ),
    ("review", "review_submit", &["--body", "New review"]),
];

#[test]
fn preview_defaults_stay_unresolved_on_offline_dry_runs_for_both_surfaces() {
    for (action, operation, fields) in PREVIEW_COMMANDS {
        let mut arguments = vec!["merge-proposal", action, "42", "--dry-run"];
        arguments.extend_from_slice(fields);
        let output = run(&arguments, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let result = document(&output);
        assert_eq!(result["data"]["executed"], false);
        assert_eq!(result["data"]["request"]["op"], operation);
        assert!(result["data"]["request"].get("preview_diff_id").is_none());
        let input = result["data"]["request"].to_string();
        let legacy = run(&["tool", "--input", "-", "--dry-run"], Some(&input));
        assert!(
            legacy.status.success(),
            "{}",
            String::from_utf8_lossy(&legacy.stdout)
        );
        assert_eq!(
            document(&legacy)["data"]["request"],
            result["data"]["request"]
        );
        if matches!(action, "draft" | "review") {
            arguments.retain(|argument| *argument != "--dry-run");
            let denied = run(&arguments, None);
            assert_eq!(denied.status.code(), Some(2));
            assert!(
                document(&denied)["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("--yes")
            );
        }
    }
}

#[test]
fn preview_command_discovery_and_validation_treat_snapshot_ids_as_optional() {
    let output = run(&["schema"], None);
    let catalog = document(&output);
    assert!(
        catalog["data"]["request_schema"]["properties"]["preview_diff_id"]["description"]
            .as_str()
            .unwrap()
            .contains("current preview")
    );
    for (action, _, fields) in PREVIEW_COMMANDS {
        let spec = catalog["data"]["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["group"] == "merge-proposal" && spec["action"] == action)
            .unwrap();
        assert!(
            !spec["required"]
                .as_array()
                .unwrap()
                .contains(&json!("preview_diff_id"))
        );
        let help = run(&["merge-proposal", action, "--help"], None);
        assert!(help.status.success());
        assert!(String::from_utf8_lossy(&help.stdout).contains("most recent preview"));
        for (target, id, message) in [
            ("42", "0", "must be greater than zero"),
            (
                "lp://~owner/project/+git/repo/+merge/42/diff/17",
                "18",
                "select different snapshots",
            ),
        ] {
            let mut arguments = vec![
                "merge-proposal",
                action,
                target,
                "--preview-diff-id",
                id,
                "--dry-run",
            ];
            arguments.extend_from_slice(fields);
            let invalid = run(&arguments, None);
            assert_eq!(invalid.status.code(), Some(2));
            assert!(
                document(&invalid)["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(message)
            );
        }
    }
}

#[test]
fn inline_comments_without_a_preview_id_use_the_latest_proposal_preview() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proposal_path = "/devel/~owner/project/+git/repo/+merge/42";
    let server = thread::spawn(move || {
        for expected_path in [
            proposal_path.to_owned(),
            "/devel/previews/102".to_owned(),
            format!("{proposal_path}?ws.op=getInlineComments&previewdiff_id=102"),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0; 4096];
            let size = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..size]);
            assert!(
                request.starts_with(&format!("GET {expected_path} ")),
                "{request}"
            );
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let body = if expected_path == proposal_path {
                json!({
                    "id": 42,
                    "self_link": format!("http://{address}{proposal_path}"),
                    "preview_diff_link": format!("http://{address}/devel/previews/102"),
                })
            } else if expected_path == "/devel/previews/102" {
                json!({"id": 102, "stale": false})
            } else {
                json!([])
            };
            let body = body.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_launchpad-cli"))
        .args([
            "merge-proposal",
            "inline-comments",
            "lp://~owner/project/+git/repo/+merge/42",
        ])
        .env("LAUNCHPAD_CLI_ANONYMOUS", "1")
        .env("LAUNCHPAD_CLI_API_BASE", format!("http://{address}/devel"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    server.join().unwrap();
    assert_eq!(document(&output)["data"]["details"]["preview_diff_id"], 102);
}

#[test]
fn original_operation_names_remain_directly_callable() {
    let output = run(
        &["tool", "--input", "-", "--dry-run"],
        Some(
            r#"{"op":"review_draft_update","target":"lp://~owner/project/+git/repo/+merge/1","preview_diff_id":2,"path":"src/main.rs","file_line":3,"side":"modified","body":"Comment"}"#,
        ),
    );
    assert!(output.status.success());
    assert_eq!(
        document(&output)["data"]["request"]["op"],
        "review_draft_update"
    );
}

#[test]
fn api_templates_keep_fixed_ws_op_and_parameter_encoding() {
    let output = run(
        &[
            "api",
            "call",
            "git_repositories-getByPath",
            "--input",
            "-",
            "--dry-run",
        ],
        Some(r#"{"params":{"path":"~owner/project/+git/repo"}}"#),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let url = document(&output)["data"]["url"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(url.contains("ws.op=getByPath"));
    assert!(url.contains("%7Eowner%2Fproject%2F%2Bgit%2Frepo"));
}

#[test]
fn component_schemas_enforce_declared_requiredness() {
    let output = run(
        &["api", "decode", "git_ref-page", "--input", "-"],
        Some(r#"{"start":0,"entries":[]}"#),
    );
    assert!(output.status.success());
    let output = run(
        &["api", "decode", "git_ref-page", "--input", "-"],
        Some(r#"{"entries":[]}"#),
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn agents_can_discover_one_operation_without_the_entire_contract() {
    let output = run(
        &[
            "api",
            "operations",
            "--compact",
            "--filter",
            "git_repositories-getByPath",
        ],
        None,
    );
    assert!(output.status.success());
    let listing = document(&output);
    let operations = listing["data"]["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 1);
    assert!(operations[0].get("definition").is_none());
    assert_eq!(operations[0]["effect"], "read");

    let output = run(&["api", "describe", "git_repositories-getByPath"], None);
    assert!(output.status.success());
    assert!(
        output.stdout.len() < 10_000,
        "description must remain focused"
    );
    let description = document(&output);
    assert_eq!(description["data"]["requires_yes"], false);
    assert!(description["data"]["unsupported_reason"].is_null());
    let validator = jsonschema::validator_for(&description["data"]["input_schema"]).unwrap();
    assert!(validator.is_valid(&json!({ "params": { "path": "launchpad" } })));
    assert!(!validator.is_valid(&json!({ "params": { "path": 42 } })));
}

#[test]
fn api_validation_errors_have_machine_readable_paths() {
    let output = run(
        &[
            "api",
            "call",
            "git_repositories-getByPath",
            "--input",
            "-",
            "--dry-run",
        ],
        Some(r#"{"params":{"path":42}}"#),
    );
    assert_eq!(output.status.code(), Some(2));
    let error = document(&output);
    assert_eq!(error["error"]["code"], "invalid_request");
    assert_eq!(error["error"]["details"]["instance_path"], "/params/path");
    assert!(
        error["error"]["details"]["schema_path"]
            .as_str()
            .unwrap()
            .ends_with("/type")
    );
}

#[test]
fn api_writes_require_consent_after_offline_validation() {
    let input = r#"{"params":{"id":"1"},"body":{"title":"Updated"}}"#;
    let arguments = ["api", "call", "bug-patch", "--input", "-"];
    let output = run(&arguments, Some(input));
    assert_eq!(output.status.code(), Some(2));
    assert!(
        document(&output)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--yes")
    );

    let mut arguments = arguments.to_vec();
    arguments.push("--dry-run");
    let output = run(&arguments, Some(input));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(document(&output)["data"]["effect"], "remote-write");
    assert_eq!(document(&output)["data"]["executed"], false);
    assert_eq!(
        document(&output)["data"]["content_type"],
        "application/json"
    );

    arguments.pop();
    arguments.push("--yes");
    let output = run(&arguments, Some(input));
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn production_mutations_are_not_needed_for_transport_tests() {
    for (status, expected_exit, expected_code) in [
        ("401 Unauthorized", 3, "not_authenticated"),
        ("403 Forbidden", 4, "permission_denied"),
        ("404 Not Found", 5, "not_found"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0; 4096];
            let size = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..size]);
            assert!(request.starts_with("GET /devel/bugs/1 "), "{request}");
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let output = Command::new(env!("CARGO_BIN_EXE_launchpad-cli"))
            .args(["--json", "bug", "view", "1"])
            .env("LAUNCHPAD_CLI_ANONYMOUS", "1")
            .env("LAUNCHPAD_CLI_API_BASE", format!("http://{address}/devel"))
            .output()
            .unwrap();
        server.join().unwrap();
        assert_eq!(output.status.code(), Some(expected_exit));
        assert_eq!(document(&output)["error"]["code"], expected_code);
    }
}

#[test]
fn read_only_api_calls_return_json_without_rendering() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = [0; 4096];
        let size = stream.read(&mut bytes).unwrap();
        assert!(
            String::from_utf8_lossy(&bytes[..size])
                .starts_with("GET /devel/+git?ws.op=getByPath&path=launchpad ")
        );
        let body =
            json!({ "name": "launchpad", "unique_name": "~launchpad/launchpad/+git/launchpad" })
                .to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_launchpad-cli"))
        .args(["api", "call", "git_repositories-getByPath", "--input", "-"])
        .env("LAUNCHPAD_CLI_ANONYMOUS", "1")
        .env("LAUNCHPAD_CLI_API_BASE", format!("http://{address}/devel"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"params":{"path":"launchpad"}}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    assert!(output.status.success());
    assert_eq!(document(&output)["data"]["body"]["name"], "launchpad");
}
