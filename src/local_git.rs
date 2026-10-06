use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::json;
use tokio::fs;
use tokio::process::Command;
use url::Url;

use crate::error::Error;
use crate::request::Request;
use crate::response::OperationResult;
use crate::result::Result;

#[derive(Debug)]
pub struct CheckoutSpec {
    pub id: String,
    pub source_ref: String,
    pub source_repository: String,
    pub source_https_url: Option<String>,
    pub source_ssh_url: Option<String>,
    pub target_https_url: Option<String>,
    pub target_ssh_url: Option<String>,
    pub web_link: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct CurrentRepository {
    pub working_directory: PathBuf,
    pub branch: String,
    pub remotes: Vec<GitRemote>,
    pub selected_remote: GitRemote,
}

pub async fn checkout(spec: CheckoutSpec, request: &Request) -> Result<OperationResult> {
    for remote in [
        spec.source_https_url.as_deref(),
        spec.source_ssh_url.as_deref(),
        spec.target_https_url.as_deref(),
        spec.target_ssh_url.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_launchpad_remote(remote)?;
    }
    let branch = spec
        .source_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(&spec.source_ref)
        .to_owned();
    let destination = checkout_destination(&spec, request)?;
    if destination.exists() {
        return Err(Error::invalid(format!(
            "checkout destination already exists: {}",
            destination.display()
        )));
    }

    let source_url = spec
        .source_https_url
        .as_deref()
        .or(spec.source_ssh_url.as_deref())
        .ok_or_else(|| Error::invalid("Launchpad did not return a source repository URL"))?;
    let source_url = match clone_repository(source_url, &branch, &destination).await {
        Ok(()) => source_url.to_owned(),
        Err(error) => {
            let Some(source_ssh_url) = spec.source_ssh_url.as_deref() else {
                return Err(error);
            };
            if source_ssh_url == source_url {
                return Err(error);
            }
            if destination.exists() {
                fs::remove_dir_all(&destination)
                    .await
                    .map_err(|source| Error::Io {
                        path: destination.clone(),
                        source,
                    })?;
            }
            clone_repository(source_ssh_url, &branch, &destination).await?;
            source_ssh_url.to_owned()
        }
    };

    let target_url = spec
        .target_https_url
        .as_deref()
        .or(spec.target_ssh_url.as_deref());
    if target_url.is_some_and(|target_url| target_url != source_url) {
        run_git([
            "-C",
            path_text(&destination)?,
            "remote",
            "add",
            "upstream",
            target_url.unwrap_or_default(),
        ])
        .await?;
    }

    let upstream = target_url
        .filter(|target_url| *target_url != source_url)
        .map(str::to_owned);
    let mut text = format!(
        "# Checked out Launchpad merge proposal {}\n\n- **Branch:** {}\n- **Path:** {}\n- **Origin:** {}",
        spec.id,
        branch,
        destination.display(),
        source_url
    );
    if let Some(upstream) = &upstream {
        text.push_str(&format!("\n- **Upstream:** {upstream}"));
    }
    let details = json!({
        "kind": "checkout",
        "proposalId": spec.id,
        "branch": branch,
        "directory": destination,
        "origin": source_url,
        "upstream": upstream,
    });
    Ok(OperationResult::new(text)
        .with_source_url(spec.web_link)
        .with_details(details))
}

pub async fn current_repository() -> Result<CurrentRepository> {
    let working_directory = std::env::current_dir().map_err(|source| Error::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let branch = git_stdout(["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map_err(|error| {
            Error::invalid(format!(
                "cannot inspect the current Git branch in {}: {error}; retry from a Git checkout or provide repository and branch",
                working_directory.display()
            ))
        })?;
    if branch.is_empty() || branch == "HEAD" {
        return Err(Error::invalid(format!(
            "cannot infer a merge proposal from detached HEAD in {}; retry with repository and branch or a full Launchpad merge proposal target",
            working_directory.display()
        )));
    }
    let remote_names = git_stdout(["remote"]).await.map_err(|error| {
        Error::invalid(format!(
            "cannot inspect Git remotes in {}: {error}; retry from a Git checkout or provide repository and branch",
            working_directory.display()
        ))
    })?;
    let mut remotes = Vec::new();
    for name in remote_names.lines().filter(|name| !name.trim().is_empty()) {
        let url = git_stdout(["remote", "get-url", name]).await?;
        remotes.push(GitRemote {
            name: name.to_owned(),
            url,
        });
    }
    let selected_remote = remotes
        .iter()
        .filter(|remote| validate_launchpad_remote(&remote.url).is_ok())
        .min_by_key(|remote| if remote.name == "origin" { 0 } else { 1 })
        .cloned()
        .ok_or_else(|| {
            let inspected = remotes
                .iter()
                .map(|remote| format!("{}={}", remote.name, remote.url))
                .collect::<Vec<_>>()
                .join(", ");
            Error::invalid(format!(
                "cannot infer a Launchpad repository in {}; inspected remotes: {}; accepted syntax: lp:<project>, lp://~owner/project/+git/repository, or a git.launchpad.net URL; retry with repository and branch or a full merge proposal target",
                working_directory.display(),
                if inspected.is_empty() { "none" } else { &inspected }
            ))
        })?;
    Ok(CurrentRepository {
        working_directory,
        branch,
        remotes,
        selected_remote,
    })
}

pub async fn current_repository_branch() -> Result<(String, String)> {
    let current = current_repository().await?;
    Ok((current.selected_remote.url, current.branch))
}

pub async fn push(request: &Request) -> Result<OperationResult> {
    let directory = request.directory.as_deref().map(PathBuf::from).unwrap_or(
        std::env::current_dir().map_err(|source| Error::Io {
            path: PathBuf::from("."),
            source,
        })?,
    );
    let directory = absolute_path(directory)?;
    let output = run_git([
        "-C",
        path_text(&directory)?,
        "rev-parse",
        "--abbrev-ref",
        "HEAD",
    ])
    .await?;
    let branch = String::from_utf8(output.stdout)
        .map_err(|source| Error::OutputEncoding { source })?
        .trim()
        .to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err(Error::invalid("cannot push a detached HEAD"));
    }

    let origin = git_stdout([
        "-C",
        path_text(&directory)?,
        "remote",
        "get-url",
        "--push",
        "--all",
        "origin",
    ])
    .await?;
    if origin.is_empty() {
        return Err(Error::invalid("origin has no push URL"));
    }
    for remote in origin.lines() {
        validate_launchpad_remote(remote)?;
    }
    let mut arguments = vec!["-C", path_text(&directory)?, "push"];
    if request.force_with_lease == Some(true) {
        arguments.push("--force-with-lease");
    }
    arguments.extend(["origin", "HEAD"]);
    let output = run_git(arguments).await?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|source| Error::OutputEncoding { source })?
        .trim()
        .to_owned();
    let mut text = format!(
        "# Pushed Launchpad merge proposal branch\n\n- **Branch:** {branch}\n- **Path:** {}",
        directory.display()
    );
    if !stderr.is_empty() {
        text.push_str(&format!("\n\n{stderr}"));
    }
    let details = json!({
        "kind": "push",
        "branch": branch,
        "directory": directory,
        "forceWithLease": request.force_with_lease == Some(true),
    });
    Ok(OperationResult::new(text).with_details(details))
}

fn checkout_destination(spec: &CheckoutSpec, request: &Request) -> Result<PathBuf> {
    let destination = if let Some(directory) = request.directory.as_deref() {
        PathBuf::from(directory)
    } else {
        let home = dirs::home_dir().ok_or(Error::HomeDirectory)?;
        let repository = Path::new(&spec.source_repository)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository");
        home.join(".launchpad-cli")
            .join("checkouts")
            .join(format!("lp-{}-{repository}", spec.id))
    };
    absolute_path(destination)
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let current_dir = std::env::current_dir().map_err(|source| Error::Io {
        path: PathBuf::from("."),
        source,
    })?;
    Ok(current_dir.join(path))
}

async fn clone_repository(source_url: &str, branch: &str, destination: &Path) -> Result<()> {
    run_git([
        "clone",
        "--branch",
        branch,
        "--single-branch",
        source_url,
        path_text(destination)?,
    ])
    .await?;
    Ok(())
}

async fn run_git<'argument>(
    arguments: impl IntoIterator<Item = &'argument str>,
) -> Result<std::process::Output> {
    let mut command = Command::new("git");
    command
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = command.output().await.map_err(|source| Error::Command {
        program: "git".to_owned(),
        source,
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let reason = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("exited with status {}", output.status)
        };
        return Err(Error::CommandFailed {
            program: "git".to_owned(),
            reason,
        });
    }
    Ok(output)
}

async fn git_stdout<'argument>(
    arguments: impl IntoIterator<Item = &'argument str>,
) -> Result<String> {
    let output = run_git(arguments).await?;
    String::from_utf8(output.stdout)
        .map_err(|source| Error::OutputEncoding { source })
        .map(|stdout| stdout.trim().to_owned())
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::invalid(format!("path is not valid UTF-8: {}", path.display())))
}

fn validate_launchpad_remote(remote: &str) -> Result<()> {
    let raw = if let Some(path) = remote.strip_prefix("git@git.launchpad.net:") {
        format!("ssh://git@git.launchpad.net/{path}")
    } else {
        remote.to_owned()
    };
    let url =
        Url::parse(&raw).map_err(|_| Error::invalid("Git remote must be a Launchpad Git URL"))?;
    if !matches!(url.scheme(), "https" | "ssh" | "git+ssh")
        || !matches!(
            url.host_str(),
            Some("git.launchpad.net" | "git.staging.launchpad.net" | "git.qastaging.launchpad.net")
        )
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().trim_matches('/').is_empty()
    {
        return Err(Error::invalid(
            "refusing a Git remote outside Launchpad Git hosting",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_operations_stay_on_launchpad() {
        for remote in [
            "https://git.launchpad.net/launchpad",
            "git+ssh://user@git.launchpad.net/~owner/project/+git/repo",
            "git@git.launchpad.net:launchpad",
        ] {
            validate_launchpad_remote(remote).unwrap();
        }
        for remote in [
            "https://github.com/owner/repo",
            "https://git.launchpad.net.evil.example/repo",
            "file:///tmp/repo",
            "ext::arbitrary-command",
        ] {
            assert!(validate_launchpad_remote(remote).is_err());
        }
    }
}
