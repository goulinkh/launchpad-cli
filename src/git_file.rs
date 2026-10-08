use std::path::Path;
use std::time::Duration;

use crate::error::Error;
use crate::local_git::{run_git, validate_launchpad_remote};
use crate::result::Result;

pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const FETCH_TIMEOUT_SECONDS: u64 = 120;

#[derive(Debug)]
pub struct GitFile {
    pub text: String,
    pub revision: String,
}

pub async fn read(remote: &str, path: &str, branch: Option<&str>) -> Result<GitFile> {
    validate_launchpad_remote(remote)?;
    let directory = tempfile::tempdir().map_err(|source| Error::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let operation = fetch_file(directory.path(), remote, path, branch.unwrap_or("HEAD"));
    tokio::time::timeout(Duration::from_secs(FETCH_TIMEOUT_SECONDS), operation)
        .await
        .map_err(|_| Error::CommandTimeout {
            program: "git file fetch".to_owned(),
            seconds: FETCH_TIMEOUT_SECONDS,
        })?
}

async fn fetch_file(directory: &Path, remote: &str, path: &str, branch: &str) -> Result<GitFile> {
    let directory = directory
        .to_str()
        .ok_or_else(|| Error::invalid("temporary Git directory is not valid UTF-8"))?;
    // No checkout: repository hooks, filters and symlinks must not execute or
    // cause file reads outside this disposable object database.
    run_git(["-C", directory, "init", "--bare", "--template="]).await?;
    run_git([
        "-C",
        directory,
        "fetch",
        "--depth=1",
        "--no-tags",
        "--recurse-submodules=no",
        "--",
        remote,
        branch,
    ])
    .await?;
    read_blob(directory, path).await
}

async fn read_blob(directory: &str, path: &str) -> Result<GitFile> {
    let revision = run_git([
        "-C",
        directory,
        "rev-parse",
        "--verify",
        "FETCH_HEAD^{commit}",
    ])
    .await?;
    let revision = String::from_utf8(revision.stdout)
        .map_err(|source| Error::OutputEncoding { source })?
        .trim()
        .to_owned();
    let object = format!("{revision}:{path}");
    let size = run_git(["-C", directory, "cat-file", "-s", &object]).await?;
    let size: usize = String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse()
        .map_err(|_| Error::GitFileResponse {
            reason: "Git returned an invalid object size".to_owned(),
        })?;
    if size > MAX_FILE_BYTES {
        return Err(Error::GitFileResponse {
            reason: format!("Launchpad file is larger than {MAX_FILE_BYTES} bytes"),
        });
    }
    let output = run_git(["-C", directory, "cat-file", "blob", &object]).await?;
    let text = String::from_utf8(output.stdout).map_err(|_| Error::GitFileResponse {
        reason: "file response is not UTF-8 text".to_owned(),
    })?;
    Ok(GitFile { text, revision })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_committed_bytes_without_checkout_filters_or_path_interpretation() {
        let source = tempfile::tempdir().unwrap();
        let source_path = source.path().to_str().unwrap();
        run_git(["-C", source_path, "init", "--initial-branch=main"])
            .await
            .unwrap();
        std::fs::write(source.path().join("a b#.txt"), "committed\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/outside-the-repository", source.path().join("link")).unwrap();
        std::fs::write(source.path().join("binary"), [0xff, 0xfe]).unwrap();
        std::fs::write(source.path().join("large"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        run_git(["-C", source_path, "add", "."]).await.unwrap();
        run_git([
            "-C",
            source_path,
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ])
        .await
        .unwrap();
        std::fs::write(source.path().join("a b#.txt"), "uncommitted\n").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let file = fetch_file(directory.path(), source_path, "a b#.txt", "main")
            .await
            .unwrap();
        assert_eq!(file.text, "committed\n");
        assert_eq!(file.revision.len(), 40);
        assert!(!directory.path().join("a b#.txt").exists());
        let directory_path = directory.path().to_str().unwrap();
        #[cfg(unix)]
        assert_eq!(
            read_blob(directory_path, "link").await.unwrap().text,
            "/outside-the-repository"
        );
        assert!(read_blob(directory_path, "missing").await.is_err());
        assert!(
            read_blob(directory_path, "binary")
                .await
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        assert!(
            read_blob(directory_path, "large")
                .await
                .unwrap_err()
                .to_string()
                .contains("larger than")
        );
        assert!(read_blob(directory_path, "").await.is_err());
    }
}
