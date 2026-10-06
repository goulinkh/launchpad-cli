use std::io;
use std::path::PathBuf;
use std::string::FromUtf8Error;

use reqwest::StatusCode;
use thiserror::Error;
use url::ParseError;

use crate::client::LpError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot run {program}: {source}")]
    Command { program: String, source: io::Error },

    #[error("{context}: {source}")]
    Context { context: String, source: Box<Error> },

    #[error("cannot complete {program}: {reason}")]
    CommandFailed { program: String, reason: String },

    #[error("cannot determine the home directory")]
    HomeDirectory,

    #[error("cannot access Launchpad: {source}")]
    Launchpad { source: LpError },

    #[error("unexpected Git file response: {reason}")]
    GitFileResponse { reason: String },

    #[error("cannot access {url}: HTTP {status}")]
    HttpStatus { url: String, status: StatusCode },

    #[error("cannot read {path}: {source}")]
    Io { path: PathBuf, source: io::Error },

    #[error("invalid request: {reason}")]
    InvalidRequest { reason: String },

    #[error("ref_pending_index: {reason}")]
    RefPendingIndex { reason: String },
    #[error("ref_visibility_unknown: {reason}")]
    RefVisibilityUnknown { reason: String },
    #[error("cannot decode command output: {source}")]
    OutputEncoding { source: FromUtf8Error },

    #[error("cannot parse URL {url}: {source}")]
    Url { url: String, source: ParseError },

    #[error("cannot decode URL path: {reason}")]
    UrlEncoding { reason: String },

    #[error("cannot fetch {url}: {source}")]
    Web { url: String, source: reqwest::Error },
}

impl Error {
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidRequest {
            reason: reason.into(),
        }
    }

    pub fn context(context: impl Into<String>, source: Self) -> Self {
        Self::Context {
            context: context.into(),
            source: Box::new(source),
        }
    }

    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Launchpad {
                source: LpError::NotAuthenticated | LpError::Api { status: 401, .. },
            } => Some("not_authenticated"),
            Self::RefPendingIndex { .. } => Some("ref_pending_index"),
            Self::RefVisibilityUnknown { .. } => Some("ref_visibility_unknown"),
            Self::Context { source, .. } => source.code(),
            _ => None,
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Context { source, .. } => source.exit_code(),
            Self::InvalidRequest { .. } => 2,
            Self::Launchpad {
                source: LpError::NotAuthenticated | LpError::Api { status: 401, .. },
            } => 3,
            Self::Launchpad {
                source: LpError::Api { status: 403, .. },
            } => 4,
            Self::Launchpad {
                source: LpError::NotFound(_) | LpError::Api { status: 404, .. },
            } => 5,
            _ => 1,
        }
    }

    pub fn stable_code(&self) -> &'static str {
        if let Some(code) = self.code() {
            return code;
        }
        match self {
            Self::Context { source, .. } => source.stable_code(),
            Self::InvalidRequest { .. } => "invalid_request",
            Self::Launchpad {
                source: LpError::Api { status: 403, .. },
            } => "permission_denied",
            Self::Launchpad {
                source: LpError::NotFound(_) | LpError::Api { status: 404, .. },
            } => "not_found",
            Self::Launchpad {
                source: LpError::Timeout(_),
            } => "timeout",
            Self::Launchpad {
                source: LpError::Api { status: 429, .. },
            } => "rate_limited",
            _ => "runtime_error",
        }
    }

    pub fn bridge_message(&self) -> String {
        match self {
            Self::Launchpad {
                source: LpError::NotAuthenticated,
            } => "Launchpad credentials are missing".to_owned(),
            Self::Launchpad {
                source: LpError::Api { status: 401, .. },
            } => "Launchpad authentication required or rejected (HTTP 401)".to_owned(),
            Self::Context { context, source } => format!("{context}: {}", source.bridge_message()),
            _ => self.to_string(),
        }
    }
}

impl From<LpError> for Error {
    fn from(source: LpError) -> Self {
        Self::Launchpad { source }
    }
}

#[cfg(test)]
mod tests {
    use crate::client::LpError;

    use super::Error;

    #[test]
    fn distinguishes_rejected_authentication_from_permission_denial() {
        let rejected = Error::context(
            "cannot inspect merge proposal",
            LpError::Api {
                status: 401,
                message: "Invalid OAuth token".to_owned(),
            }
            .into(),
        );
        assert_eq!(rejected.code(), Some("not_authenticated"));
        assert!(rejected.bridge_message().contains("HTTP 401"));

        let forbidden: Error = LpError::Api {
            status: 403,
            message: "Permission denied".to_owned(),
        }
        .into();
        assert_eq!(forbidden.code(), None);
    }
}
