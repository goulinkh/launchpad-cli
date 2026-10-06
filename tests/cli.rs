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
fn generated_types_enforce_declared_requiredness() {
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
