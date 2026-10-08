use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(arguments)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", directory.join("missing.config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ssh_file_read_is_isolated_and_cleans_up_on_success_and_failure() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source");
    let temporary = fixture.path().join("temporary");
    let unrelated = fixture.path().join("unrelated");
    for directory in [&source, &temporary, &unrelated] {
        std::fs::create_dir(directory).unwrap();
    }
    git(&source, &["init", "--initial-branch=main", "--template="]);
    std::fs::write(source.join("README.md"), "committed\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-m", "fixture"]);
    std::fs::write(source.join("README.md"), "uncommitted\n").unwrap();

    // Git's user-controlled URL rewriting supplies a local transport fixture;
    // the CLI still constructs and checks its selected-instance SSH URL.
    let local_url = url::Url::from_directory_path(&source).unwrap();
    let remote = "git+ssh://git.launchpad.test/project";
    for (branch, expected) in [("main", true), ("missing-branch", false)] {
        let output = Command::new(env!("CARGO_BIN_EXE_launchpad-cli"))
            .args([
                "repository",
                "file",
                "project",
                "--path",
                "README.md",
                "--transport",
                "ssh",
                "--branch",
                branch,
                "--json",
            ])
            .env("LAUNCHPAD_CLI_INSTANCE", "development")
            .env_remove("LAUNCHPAD_CLI_API_BASE")
            .env("LAUNCHPAD_CLI_ANONYMOUS", "1")
            .env(
                "LAUNCHPAD_CLI_CREDENTIALS",
                fixture.path().join("missing.json"),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", fixture.path().join("missing.config"))
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", format!("url.{local_url}.insteadOf"))
            .env("GIT_CONFIG_VALUE_0", remote)
            .env("GIT_DIR", &unrelated)
            .env("GIT_WORK_TREE", &unrelated)
            .env("GIT_COMMON_DIR", &unrelated)
            .env("GIT_INDEX_FILE", unrelated.join("index"))
            .env("GIT_OBJECT_DIRECTORY", &unrelated)
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &unrelated)
            .env("TMPDIR", &temporary)
            .env("TEMP", &temporary)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.stderr.is_empty());
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.success(), expected, "{result}");
        if expected {
            assert_eq!(result["data"]["text"], "committed\n");
            assert_eq!(result["data"]["details"]["transport"], "ssh");
            assert_eq!(result["data"]["details"]["bytes"], 10);
            assert_eq!(result["data"]["source_url"], remote);
        }
        assert_eq!(std::fs::read_dir(&temporary).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&unrelated).unwrap().count(), 0);
    }
    assert_eq!(
        std::fs::read_to_string(source.join("README.md")).unwrap(),
        "uncommitted\n"
    );
}
