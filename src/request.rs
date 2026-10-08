use std::path::Path;

use chrono::{DateTime, FixedOffset};
use percent_encoding::percent_decode_str;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::diff::DiffSide;
use crate::error::Error;
use crate::result::Result;

const MAX_COMMENTS: usize = 20;
const REVIEW_VOTES: &[&str] = &[
    "Approve",
    "Needs Fixing",
    "Needs Information",
    "Abstain",
    "Disapprove",
    "Needs Resubmitting",
];
const MAX_RESULTS: usize = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    ResourceView,
    RepoView,
    FileRead,
    SearchBugs,
    SearchMergeProposals,
    MergeProposalForBranch,
    CurrentMergeProposal,
    MergeProposalDiscussion,
    MergeProposalBugs,
    PreviewDiffs,
    InlineComments,
    ReviewDrafts,
    DiffLineMap,
    BugCreate,
    BugEdit,
    BugTaskEdit,
    ProjectEdit,
    RepositoryEdit,
    MergeProposalCreate,
    MergeProposalEdit,
    ReplaceMergeProposalPrerequisite,
    MergeProposalLinkBug,
    MergeProposalUnlinkBug,
    Comment,
    CommentEdit,
    ReviewDraftUpdate,
    ReviewSubmit,
    SetMergeProposalStatus,
    MergeProposalCheckout,
    MergeProposalPush,
}

impl Operation {
    pub fn check_supported(self) -> Result<()> {
        if self == Self::ReplaceMergeProposalPrerequisite {
            return Err(Error::UnsupportedOperation {
                reason: "Launchpad does not expose prerequisite resubmission through its API; use the web resubmit flow. No requests were sent".to_owned(),
            });
        }
        Ok(())
    }

    pub fn requires_authentication(self) -> bool {
        matches!(
            self,
            Self::ReviewDrafts
                | Self::MergeProposalBugs
                | Self::BugCreate
                | Self::BugEdit
                | Self::BugTaskEdit
                | Self::ProjectEdit
                | Self::RepositoryEdit
                | Self::MergeProposalCreate
                | Self::MergeProposalEdit
                | Self::ReplaceMergeProposalPrerequisite
                | Self::MergeProposalLinkBug
                | Self::MergeProposalUnlinkBug
                | Self::Comment
                | Self::CommentEdit
                | Self::ReviewDraftUpdate
                | Self::ReviewSubmit
                | Self::SetMergeProposalStatus
        )
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn append_to(
        &self,
        query: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
        name: &str,
    ) {
        match self {
            Self::One(value) => {
                query.append_pair(name, value);
            }
            Self::Many(values) => {
                for value in values {
                    query.append_pair(name, value);
                }
            }
        }
    }

    pub fn values(&self) -> impl Iterator<Item = &str> {
        match self {
            Self::One(value) => std::slice::from_ref(value).iter().map(String::as_str),
            Self::Many(values) => values.iter().map(String::as_str),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscussionComments {
    #[default]
    All,
    General,
    Inline,
}

impl DiscussionComments {
    pub fn includes_general(self) -> bool {
        matches!(self, Self::All | Self::General)
    }

    pub fn includes_inline(self) -> bool {
        matches!(self, Self::All | Self::Inline)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscussionFormat {
    #[default]
    Summary,
    Structured,
    Both,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileTransport {
    #[default]
    Https,
    Ssh,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub op: Operation,
    pub target: Option<String>,
    pub repository: Option<String>,
    pub target_repository: Option<String>,
    pub path: Option<String>,
    pub branch: Option<String>,
    pub transport: Option<FileTransport>,
    pub query: Option<String>,
    pub status: Option<OneOrMany>,
    pub importance: Option<OneOrMany>,
    pub tags: Option<Vec<String>>,
    pub limit: Option<usize>,
    pub preview_diff_id: Option<u64>,
    pub current_diff_only: Option<bool>,
    pub unresolved_only: Option<bool>,
    pub comments: Option<DiscussionComments>,
    pub format: Option<DiscussionFormat>,
    pub since: Option<String>,
    pub reviewer: Option<String>,
    pub target_branch: Option<String>,
    pub latest: Option<bool>,
    pub include_superseded: Option<bool>,
    pub file_line: Option<u64>,
    pub side: Option<DiffSide>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub bug_reporting_guidelines: Option<String>,
    pub official_bug_tags: Option<Vec<String>>,
    pub default_branch: Option<String>,
    pub assignee: Option<String>,
    pub unassign: Option<bool>,
    pub bug_id: Option<u64>,
    pub description: Option<String>,
    pub information_type: Option<String>,
    pub source_ref: Option<String>,
    pub target_ref: Option<String>,
    pub merge_prerequisite: Option<String>,
    pub prerequisite_ref: Option<String>,
    pub prerequisite_repository: Option<String>,
    pub wait_for_index: Option<bool>,
    pub index_timeout_seconds: Option<u64>,
    pub wait_for_preview: Option<bool>,
    pub preview_timeout_seconds: Option<u64>,
    pub commit_message: Option<String>,
    pub reviewed_revid: Option<String>,
    pub needs_review: Option<bool>,
    pub body: Option<String>,
    pub subject: Option<String>,
    pub vote: Option<String>,
    pub directory: Option<String>,
    pub force_with_lease: Option<bool>,
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        let required: &[(&str, &Option<String>)] = match self.op {
            Operation::ResourceView => &[("target", &self.target)],
            Operation::RepoView => &[("repository", &self.repository)],
            Operation::FileRead => &[("repository", &self.repository), ("path", &self.path)],
            Operation::SearchBugs => &[("target", &self.target)],
            Operation::SearchMergeProposals => &[("repository", &self.repository)],
            Operation::MergeProposalForBranch
            | Operation::CurrentMergeProposal
            | Operation::MergeProposalDiscussion => &[],
            Operation::MergeProposalBugs
            | Operation::MergeProposalLinkBug
            | Operation::MergeProposalUnlinkBug
            | Operation::MergeProposalEdit
            | Operation::BugEdit
            | Operation::BugTaskEdit
            | Operation::ProjectEdit
            | Operation::RepositoryEdit
            | Operation::SetMergeProposalStatus
            | Operation::MergeProposalCheckout => &[("target", &self.target)],
            Operation::PreviewDiffs
            | Operation::InlineComments
            | Operation::ReviewDrafts
            | Operation::ReviewSubmit => &[("target", &self.target)],
            Operation::DiffLineMap | Operation::ReviewDraftUpdate => {
                &[("target", &self.target), ("path", &self.path)]
            }
            Operation::BugCreate => &[
                ("target", &self.target),
                ("title", &self.title),
                ("description", &self.description),
            ],
            Operation::MergeProposalCreate => &[
                ("repository", &self.repository),
                ("source_ref", &self.source_ref),
                ("target_ref", &self.target_ref),
                ("commit_message", &self.commit_message),
            ],
            Operation::ReplaceMergeProposalPrerequisite => &[
                ("target", &self.target),
                ("merge_prerequisite", &self.merge_prerequisite),
            ],
            Operation::Comment | Operation::CommentEdit => {
                &[("target", &self.target), ("body", &self.body)]
            }
            Operation::MergeProposalPush => &[],
        };
        match self.op {
            Operation::MergeProposalForBranch => self.validate_branch_selector(false)?,
            Operation::CurrentMergeProposal => self.validate_current_selector()?,
            Operation::MergeProposalDiscussion => self.validate_branch_selector(true)?,
            _ => {}
        }
        if matches!(
            self.op,
            Operation::MergeProposalForBranch
                | Operation::CurrentMergeProposal
                | Operation::MergeProposalDiscussion
        ) {
            self.validate_proposal_selection()?;
        }
        for (name, value) in required {
            if value.as_deref().is_none_or(str::is_empty) {
                return Err(Error::invalid(format!(
                    "{name} is required for {:?}",
                    self.op
                )));
            }
        }
        if matches!(
            self.op,
            Operation::InlineComments
                | Operation::ReviewDrafts
                | Operation::DiffLineMap
                | Operation::ReviewDraftUpdate
                | Operation::ReviewSubmit
        ) {
            self.preview_diff_id()?;
        }
        if matches!(
            self.op,
            Operation::DiffLineMap | Operation::ReviewDraftUpdate
        ) {
            self.file_line()?;
            self.side()?;
            validate_repository_path(self.path("path")?)?;
        }
        if let Some(prerequisite) = self.merge_prerequisite.as_deref() {
            if self.op != Operation::ReplaceMergeProposalPrerequisite {
                return Err(Error::invalid(
                    "merge_prerequisite is supported only for proposal replacement; use prerequisite_ref for creation",
                ));
            }
            if prerequisite.trim().is_empty() {
                return Err(Error::invalid("merge_prerequisite cannot be empty"));
            }
        }
        if self.op == Operation::MergeProposalCreate {
            if self
                .prerequisite_ref
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                return Err(Error::invalid("prerequisite_ref cannot be empty"));
            }
            if let Some(repository) = self.prerequisite_repository.as_deref() {
                if repository.trim().is_empty() {
                    return Err(Error::invalid("prerequisite_repository cannot be empty"));
                }
                if self.prerequisite_ref.is_none() {
                    return Err(Error::invalid(
                        "prerequisite_repository requires prerequisite_ref",
                    ));
                }
            }
            validate_creation_timeout(
                self.index_timeout_seconds,
                self.wait_for_index,
                "index_timeout_seconds",
                "wait_for_index",
            )?;
            validate_creation_timeout(
                self.preview_timeout_seconds,
                self.wait_for_preview,
                "preview_timeout_seconds",
                "wait_for_preview",
            )?;
        } else if self.prerequisite_ref.is_some()
            || self.prerequisite_repository.is_some()
            || self.wait_for_index.is_some()
            || self.index_timeout_seconds.is_some()
            || self.wait_for_preview.is_some()
            || self.preview_timeout_seconds.is_some()
        {
            return Err(Error::invalid(
                "prerequisite_ref, prerequisite_repository, and indexing/preview waits are supported only for merge_proposal_create",
            ));
        }
        if self.op == Operation::MergeProposalEdit
            && self.commit_message.is_none()
            && self.description.is_none()
            && self.reviewed_revid.is_none()
        {
            return Err(Error::invalid(
                "merge_proposal_edit requires commit_message, description, or reviewed_revid",
            ));
        }
        let has_changes = match self.op {
            Operation::BugEdit => {
                Some(self.title.is_some() || self.description.is_some() || self.tags.is_some())
            }
            Operation::BugTaskEdit => Some(
                self.status.is_some()
                    || self.importance.is_some()
                    || self.assignee.is_some()
                    || self.unassign == Some(true),
            ),
            Operation::ProjectEdit => Some(
                self.summary.is_some()
                    || self.description.is_some()
                    || self.bug_reporting_guidelines.is_some()
                    || self.official_bug_tags.is_some(),
            ),
            Operation::RepositoryEdit => {
                Some(self.description.is_some() || self.default_branch.is_some())
            }
            _ => None,
        };
        if has_changes == Some(false) {
            return Err(Error::invalid(format!(
                "{:?} requires at least one field to edit",
                self.op
            )));
        }
        if matches!(
            self.op,
            Operation::MergeProposalLinkBug | Operation::MergeProposalUnlinkBug
        ) && self.bug_id.is_none_or(|id| id == 0)
        {
            return Err(Error::invalid("bug_id must be a positive bug ID"));
        }
        if self.op == Operation::BugTaskEdit {
            if self.status.is_some() {
                self.status()?;
            }
            if self.importance.as_ref().is_some_and(|importance| {
                !matches!(importance, OneOrMany::One(value) if !value.trim().is_empty())
            }) {
                return Err(Error::invalid("importance must be a nonempty string"));
            }
            if self.assignee.is_some() && self.unassign == Some(true) {
                return Err(Error::invalid(
                    "assignee and unassign cannot be used together",
                ));
            }
            if self.unassign == Some(false) {
                return Err(Error::invalid("unassign must be true when specified"));
            }
            if let Some(assignee) = self.assignee.as_deref() {
                if assignee.is_empty()
                    || !assignee.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.' | b'_')
                    })
                {
                    return Err(Error::invalid("assignee must be a Launchpad username"));
                }
            }
        }
        if let Some(branch) = self.default_branch.as_deref() {
            if !branch.starts_with("refs/heads/") || branch == "refs/heads/" {
                return Err(Error::invalid(
                    "default_branch must be a full refs/heads/ Git ref",
                ));
            }
        }
        if self.commit_message.is_some()
            && matches!(
                self.op,
                Operation::MergeProposalCreate | Operation::MergeProposalEdit
            )
        {
            self.string(&self.commit_message, "commit_message")?;
        }
        if self.op == Operation::SetMergeProposalStatus
            && self.status()?.eq_ignore_ascii_case("Superseded")
        {
            Operation::ReplaceMergeProposalPrerequisite.check_supported()?;
        }
        if self.vote.is_some() {
            self.vote()?;
        }
        if self.op == Operation::ReviewDraftUpdate
            && self
                .body
                .as_deref()
                .is_some_and(|body| body.trim().is_empty())
        {
            return Err(Error::invalid("draft body cannot be empty"));
        }
        if self.limit == Some(0) {
            return Err(Error::invalid("limit must be greater than zero"));
        }
        if self.limit.is_some_and(|limit| limit > MAX_RESULTS) {
            return Err(Error::invalid(format!(
                "limit cannot exceed {MAX_RESULTS}; narrow the search or request multiple pages"
            )));
        }
        if self.op == Operation::FileRead {
            validate_repository_path(self.path("path")?)?;
            normalise_repository(self.repository()?)?;
            if self.branch.as_deref().is_some_and(|branch| {
                branch.trim().is_empty() || branch.starts_with('-') || branch.contains('\0')
            }) {
                return Err(Error::invalid(
                    "branch must be a nonempty Git ref, not an option",
                ));
            }
        } else if self.transport.is_some() {
            return Err(Error::invalid("transport is supported only for file_read"));
        }
        let has_discussion_filters = self.current_diff_only.is_some()
            || self.unresolved_only.is_some()
            || self.comments.is_some()
            || self.format.is_some()
            || self.since.is_some()
            || self.reviewer.is_some();
        if has_discussion_filters && self.op != Operation::MergeProposalDiscussion {
            return Err(Error::invalid(
                "discussion filters and format are supported only for merge_proposal_discussion",
            ));
        }
        let has_selection_filters = self.target_branch.is_some()
            || self.latest.is_some()
            || self.include_superseded.is_some();
        if has_selection_filters
            && !matches!(
                self.op,
                Operation::MergeProposalForBranch
                    | Operation::CurrentMergeProposal
                    | Operation::MergeProposalDiscussion
            )
        {
            return Err(Error::invalid(
                "proposal selection filters are supported only for branch and current merge proposal lookup",
            ));
        }
        if self.comments == Some(DiscussionComments::General)
            && (self.current_diff_only.unwrap_or_default()
                || self.unresolved_only.unwrap_or_default())
        {
            return Err(Error::invalid(
                "current_diff_only and unresolved_only require inline comments",
            ));
        }
        self.since()?;
        if self
            .reviewer
            .as_deref()
            .is_some_and(|reviewer| reviewer.trim().is_empty())
        {
            return Err(Error::invalid("reviewer cannot be empty"));
        }
        Ok(())
    }

    pub fn string<'value>(&self, value: &'value Option<String>, name: &str) -> Result<&'value str> {
        value
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::invalid(format!("{name} is required for {:?}", self.op)))
    }

    pub fn target(&self) -> Result<&str> {
        self.string(&self.target, "target")
    }

    pub fn repository(&self) -> Result<&str> {
        self.string(&self.repository, "repository")
    }

    pub fn path(&self, name: &str) -> Result<&str> {
        self.string(&self.path, name)
    }

    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(10)
    }

    pub fn preview_diff_id(&self) -> Result<u64> {
        if self.preview_diff_id == Some(0) {
            return Err(Error::invalid("preview_diff_id must be greater than zero"));
        }
        let target_id = self
            .target
            .as_deref()
            .map(ResourceTarget::parse)
            .transpose()?
            .and_then(|target| target.preview_diff_id);
        if self
            .preview_diff_id
            .zip(target_id)
            .is_some_and(|(parameter_id, target_id)| parameter_id != target_id)
        {
            return Err(Error::invalid(
                "target and preview_diff_id select different snapshots",
            ));
        }
        self.preview_diff_id
            .or(target_id)
            .ok_or_else(|| Error::invalid(format!("preview_diff_id is required for {:?}", self.op)))
    }

    pub fn file_line(&self) -> Result<u64> {
        self.file_line
            .filter(|line| *line > 0)
            .ok_or_else(|| Error::invalid(format!("file_line is required for {:?}", self.op)))
    }

    pub fn side(&self) -> Result<DiffSide> {
        self.side
            .ok_or_else(|| Error::invalid(format!("side is required for {:?}", self.op)))
    }

    pub fn vote(&self) -> Result<&str> {
        self.vote
            .as_deref()
            .filter(|vote| REVIEW_VOTES.contains(vote))
            .ok_or_else(|| Error::invalid("vote is not a supported Launchpad review vote"))
    }

    pub fn status(&self) -> Result<&str> {
        match &self.status {
            Some(OneOrMany::One(status)) if !status.trim().is_empty() => Ok(status),
            _ => Err(Error::invalid(format!(
                "status must be a string for {:?}",
                self.op
            ))),
        }
    }

    pub fn since(&self) -> Result<Option<DateTime<FixedOffset>>> {
        self.since
            .as_deref()
            .map(|since| {
                DateTime::parse_from_rfc3339(since).map_err(|_| {
                    Error::invalid(
                        "since must be an RFC 3339 timestamp such as 2026-09-21T09:30:00Z",
                    )
                })
            })
            .transpose()
    }

    fn validate_current_selector(&self) -> Result<()> {
        if self.target.is_some() || self.repository.is_some() || self.branch.is_some() {
            return Err(Error::invalid(
                "current_merge_proposal uses the current Git checkout; omit target, repository, and branch",
            ));
        }
        Ok(())
    }

    fn validate_proposal_selection(&self) -> Result<()> {
        if self
            .target_branch
            .as_deref()
            .is_some_and(|branch| branch.trim().is_empty())
        {
            return Err(Error::invalid("target_branch cannot be empty"));
        }
        if self.status.as_ref().is_some_and(|statuses| {
            let values = statuses.values().collect::<Vec<_>>();
            values.is_empty() || values.iter().any(|status| status.trim().is_empty())
        }) {
            return Err(Error::invalid("status filters cannot be empty"));
        }
        Ok(())
    }

    fn validate_branch_selector(&self, allow_target: bool) -> Result<()> {
        let has_target = self
            .target
            .as_deref()
            .is_some_and(|target| !target.trim().is_empty());
        let has_repository = self
            .repository
            .as_deref()
            .is_some_and(|repository| !repository.trim().is_empty());
        let has_branch = self
            .branch
            .as_deref()
            .is_some_and(|branch| !branch.trim().is_empty());

        if has_target {
            if !allow_target {
                return Err(Error::invalid(format!(
                    "target is not supported for {:?}; provide repository and branch",
                    self.op
                )));
            }
            if has_repository || has_branch {
                return Err(Error::invalid(
                    "target cannot be combined with repository or branch",
                ));
            }
            if self.status.is_some()
                || self.target_branch.is_some()
                || self.latest.is_some()
                || self.include_superseded.is_some()
            {
                return Err(Error::invalid(
                    "proposal selection filters cannot be combined with an explicit target",
                ));
            }
            return Ok(());
        }
        if has_repository == has_branch {
            return Ok(());
        }
        Err(Error::invalid(
            "repository and branch must be provided together; omit both to infer them from the current Git checkout",
        ))
    }
}

fn validate_creation_timeout(
    timeout: Option<u64>,
    wait: Option<bool>,
    timeout_name: &str,
    wait_name: &str,
) -> Result<()> {
    if let Some(seconds) = timeout {
        if wait != Some(true) {
            return Err(Error::invalid(format!(
                "{timeout_name} requires {wait_name}: true"
            )));
        }
        if !(1..=300).contains(&seconds) {
            return Err(Error::invalid(format!(
                "{timeout_name} must be between 1 and 300 seconds"
            )));
        }
    }
    Ok(())
}

pub fn normalise_repository(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::invalid("repository cannot be empty"));
    }

    let path = if let Some(path) = raw.strip_prefix("lp://") {
        path.to_owned()
    } else if let Some(path) = raw.strip_prefix("lp:") {
        path.trim_start_matches('/').to_owned()
    } else if let Some((_, path)) = raw.split_once(':').filter(|(host, _)| {
        matches!(
            *host,
            "git@git.launchpad.net"
                | "git@git.staging.launchpad.net"
                | "git@git.qastaging.launchpad.net"
                | "git@git.launchpad.test"
        )
    }) {
        path.to_owned()
    } else if raw.contains("://") {
        repository_path_from_url(raw)?
    } else {
        raw.trim_start_matches('/').to_owned()
    };
    let path = path
        .split_once("/+merge/")
        .map_or(path.as_str(), |(repository, _)| repository);
    let path = path
        .split_once("/+ref/")
        .map_or(path, |(repository, _)| repository)
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        return Err(Error::invalid(
            "repository must identify a Launchpad Git repository",
        ));
    }
    Ok(path.to_owned())
}

fn repository_path_from_url(raw: &str) -> Result<String> {
    let url = Url::parse(raw).map_err(|source| Error::Url {
        url: raw.to_owned(),
        source,
    })?;
    let host = url.host_str().unwrap_or_default();
    if !matches!(
        host,
        "code.launchpad.net"
            | "git.launchpad.net"
            | "api.launchpad.net"
            | "code.staging.launchpad.net"
            | "git.staging.launchpad.net"
            | "api.staging.launchpad.net"
            | "code.qastaging.launchpad.net"
            | "git.qastaging.launchpad.net"
            | "api.qastaging.launchpad.net"
            | "code.launchpad.test"
            | "git.launchpad.test"
            | "api.launchpad.test"
    ) {
        return Err(Error::invalid(format!(
            "repository URL host {host:?} is not a Launchpad host; use a Launchpad clone URL, web URL, lp:// identifier, or repository path"
        )));
    }
    let mut path = percent_decode_str(url.path().trim_start_matches('/'))
        .decode_utf8()
        .map_err(|source| Error::UrlEncoding {
            reason: source.to_string(),
        })?
        .into_owned();
    if host.starts_with("api.") {
        path = path
            .split_once('/')
            .map_or(String::new(), |(_, resource)| resource.to_owned());
    }
    Ok(path)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    Bug { id: u64 },
    MergeProposal { repository: String, id: u64 },
    MergeProposalId { id: u64 },
    Repository,
    Generic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceTarget {
    pub path: String,
    pub kind: ResourceKind,
    pub include_comments: bool,
    pub comment_limit: usize,
    pub diff: bool,
    pub preview_diff_id: Option<u64>,
}

impl ResourceTarget {
    pub fn parse(raw: &str) -> Result<Self> {
        let (path, query) = split_target(raw)?;
        let include_comments = query
            .iter()
            .find(|(name, _)| name == "comments")
            .is_none_or(|(_, value)| value != "0");
        let comment_limit = query
            .iter()
            .find(|(name, _)| name == "limit")
            .map(|(_, value)| {
                value
                    .parse::<usize>()
                    .map(|limit| limit.clamp(1, MAX_COMMENTS))
                    .map_err(|_| Error::invalid("limit must be an integer"))
            })
            .transpose()?
            .unwrap_or(MAX_COMMENTS);
        let query_preview_diff_id = query
            .iter()
            .find(|(name, _)| name == "preview_diff")
            .map(|(_, value)| parse_positive_id(value, "preview_diff"))
            .transpose()?;
        let (path, diff, path_preview_diff_id) = strip_diff_suffix(path)?;
        if query_preview_diff_id.is_some()
            && path_preview_diff_id.is_some()
            && query_preview_diff_id != path_preview_diff_id
        {
            return Err(Error::invalid(
                "preview diff path and preview_diff query select different snapshots",
            ));
        }
        let preview_diff_id = query_preview_diff_id.or(path_preview_diff_id);
        if preview_diff_id.is_some() && !diff {
            return Err(Error::invalid(
                "preview_diff requires a merge proposal diff target such as lp://~owner/project/+git/repository/+merge/123/diff/456; use inline_comments with preview_diff_id to read published comments",
            ));
        }
        let path = normalise_bug_path(path);
        let kind = classify_resource(&path)?;
        Ok(Self {
            path,
            kind,
            include_comments,
            comment_limit,
            diff,
            preview_diff_id,
        })
    }
}

pub fn validate_repository_path(path: &str) -> Result<()> {
    if path.contains('\0') {
        return Err(Error::invalid("path cannot contain NUL bytes"));
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| component.as_os_str() == "..")
    {
        return Err(Error::invalid(
            "path must be repository-relative and cannot contain '..'",
        ));
    }
    Ok(())
}

fn split_target(raw: &str) -> Result<(String, Vec<(String, String)>)> {
    let raw = raw.trim();
    if raw.starts_with("lp://") || raw.starts_with("http://") || raw.starts_with("https://") {
        let url = Url::parse(raw).map_err(|source| Error::Url {
            url: raw.to_owned(),
            source,
        })?;
        let mut path = String::new();
        if url.scheme() == "lp" {
            if let Some(host) = url.host_str() {
                path.push_str(host);
            }
        } else if !matches!(
            url.host_str(),
            Some(
                "launchpad.net"
                    | "bugs.launchpad.net"
                    | "code.launchpad.net"
                    | "api.launchpad.net"
                    | "api.staging.launchpad.net"
                    | "api.qastaging.launchpad.net"
                    | "launchpad.test"
                    | "bugs.launchpad.test"
                    | "code.launchpad.test"
                    | "api.launchpad.test"
            )
        ) {
            return Err(Error::invalid("target URL is not a Launchpad URL"));
        }
        let url_path = url.path().trim_start_matches('/');
        let url_path = if url.host_str().is_some_and(|host| host.starts_with("api.")) {
            url_path
                .split_once('/')
                .map(|(_, resource)| resource)
                .filter(|resource| !resource.is_empty())
                .ok_or_else(|| Error::invalid("API target URL has no resource path"))?
        } else {
            url_path
        };
        if !path.is_empty() && !url_path.is_empty() {
            path.push('/');
        }
        path.push_str(url_path);
        let path = percent_decode_str(&path)
            .decode_utf8()
            .map_err(|source| Error::UrlEncoding {
                reason: source.to_string(),
            })?
            .into_owned();
        let query = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        return Ok((path, query));
    }
    Ok((raw.trim_start_matches('/').to_owned(), Vec::new()))
}

fn strip_diff_suffix(path: String) -> Result<(String, bool, Option<u64>)> {
    for suffix in ["/diff/all", "/diff"] {
        if let Some(path) = path.strip_suffix(suffix) {
            return Ok((path.to_owned(), true, None));
        }
    }
    if let Some((path, id)) = path.rsplit_once("/diff/") {
        let id = parse_positive_id(id, "preview diff path")?;
        return Ok((path.to_owned(), true, Some(id)));
    }
    Ok((path, false, None))
}

fn parse_positive_id(value: &str, name: &str) -> Result<u64> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::invalid(format!("{name} must be a positive integer")))
}

fn normalise_bug_path(path: String) -> String {
    if let Some(id) = path.strip_prefix("bug/") {
        format!("bugs/{id}")
    } else if let Some(id) = path.strip_prefix("+bug/") {
        format!("bugs/{id}")
    } else {
        path
    }
}

fn classify_resource(path: &str) -> Result<ResourceKind> {
    if let Some((_, comment_id)) = path.rsplit_once("/comments/") {
        comment_id
            .parse::<u64>()
            .map_err(|_| Error::invalid("comment target must end with a numeric ID"))?;
        return Ok(ResourceKind::Generic);
    }
    if let Some(id) = path.strip_prefix("bugs/") {
        let id = id
            .parse()
            .map_err(|_| Error::invalid("bug target must end with a numeric ID"))?;
        return Ok(ResourceKind::Bug { id });
    }
    if let Some((repository, id)) = path.rsplit_once("/+merge/") {
        let id = parse_positive_id(id, "merge proposal target")?;
        return Ok(ResourceKind::MergeProposal {
            repository: repository.to_owned(),
            id,
        });
    }
    if path.bytes().all(|byte| byte.is_ascii_digit()) && !path.is_empty() {
        return Ok(ResourceKind::MergeProposalId {
            id: parse_positive_id(path, "merge proposal ID")?,
        });
    }
    if path
        .split_once("/+git/")
        .is_some_and(|(_, name)| !name.trim_end_matches('/').contains('/'))
    {
        return Ok(ResourceKind::Repository);
    }
    Ok(ResourceKind::Generic)
}

#[cfg(test)]
mod tests {
    use super::{
        DiscussionComments, DiscussionFormat, Request, ResourceKind, ResourceTarget,
        normalise_repository, validate_repository_path,
    };

    #[test]
    fn accepts_development_resource_and_repository_urls() {
        let repository = "~owner/project/+git/repo";
        for url in [
            format!("https://code.launchpad.test/{repository}"),
            format!("git+ssh://owner@git.launchpad.test/{repository}"),
            format!("git@git.launchpad.test:{repository}"),
            format!("https://api.launchpad.test/devel/{repository}"),
        ] {
            assert_eq!(super::normalise_repository(&url).unwrap(), repository);
        }
        assert_eq!(
            super::ResourceTarget::parse("https://bugs.launchpad.test/project/+bug/16")
                .unwrap()
                .path,
            "project/+bug/16"
        );
        assert!(
            super::normalise_repository("https://git.launchpad.test.evil.example/repo").is_err()
        );
        assert!(
            super::ResourceTarget::parse("https://bugs.launchpad.test.evil.example/bugs/16")
                .is_err()
        );
    }

    #[test]
    fn repository_collections_are_generic_resources_not_repository_names() {
        let target = super::ResourceTarget::parse(
            "https://api.launchpad.test/devel/~owner/project/+git/repo/refs",
        )
        .unwrap();
        assert_eq!(target.kind, super::ResourceKind::Generic);
        assert_eq!(target.path, "~owner/project/+git/repo/refs");
    }

    #[test]
    fn parses_bug_url_options() {
        let target = ResourceTarget::parse("lp://bugs/1?comments=0&limit=200").unwrap();
        assert_eq!(target.kind, ResourceKind::Bug { id: 1 });
        assert!(!target.include_comments);
        assert_eq!(target.comment_limit, 20);
    }

    #[test]
    fn parses_merge_proposal_diff() {
        let target =
            ResourceTarget::parse("lp://~owner/project/+git/repository/+merge/42/diff").unwrap();
        assert_eq!(
            target.kind,
            ResourceKind::MergeProposal {
                repository: "~owner/project/+git/repository".to_owned(),
                id: 42,
            }
        );
        assert!(target.diff);
    }

    #[test]
    fn parses_numeric_merge_proposal_id() {
        let target = ResourceTarget::parse("511601").unwrap();
        assert_eq!(target.kind, ResourceKind::MergeProposalId { id: 511601 });
    }

    #[test]
    fn parses_explicit_preview_diff() {
        let target =
            ResourceTarget::parse("lp://~owner/project/+git/repository/+merge/42/diff/17").unwrap();
        assert!(target.diff);
        assert_eq!(target.preview_diff_id, Some(17));
    }

    #[test]
    fn uses_preview_diff_from_target() {
        let request: Request = serde_json::from_str(
            r#"{
                "op": "inline_comments",
                "target": "lp://~owner/project/+git/repository/+merge/42/diff/17"
            }"#,
        )
        .unwrap();
        request.validate().unwrap();
        assert_eq!(request.preview_diff_id().unwrap(), 17);
    }

    #[test]
    fn rejects_conflicting_preview_diff_selectors() {
        let request: Request = serde_json::from_str(
            r#"{
                "op": "inline_comments",
                "target": "lp://~owner/project/+git/repository/+merge/42/diff/17",
                "preview_diff_id": 18
            }"#,
        )
        .unwrap();
        let error = request.validate().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("target and preview_diff_id select different snapshots")
        );
    }

    #[test]
    fn normalises_repository_identifiers() {
        let cases = [
            ("launchpad-ui", "launchpad-ui"),
            (
                "lp://~goulinkh/launchpad-ui/+git/launchpad-ui",
                "~goulinkh/launchpad-ui/+git/launchpad-ui",
            ),
            (
                "https://code.launchpad.net/~goulinkh/launchpad-ui/+git/launchpad-ui",
                "~goulinkh/launchpad-ui/+git/launchpad-ui",
            ),
            (
                "https://git.launchpad.net/~goulinkh/launchpad-ui",
                "~goulinkh/launchpad-ui",
            ),
            (
                "git+ssh://git.launchpad.net/~goulinkh/launchpad-ui",
                "~goulinkh/launchpad-ui",
            ),
            (
                "ssh://git@git.launchpad.net/~goulinkh/launchpad-ui.git",
                "~goulinkh/launchpad-ui",
            ),
            (
                "git@git.launchpad.net:~goulinkh/launchpad-ui",
                "~goulinkh/launchpad-ui",
            ),
            (
                "https://code.launchpad.net/~owner/project/+git/repository/+ref/main",
                "~owner/project/+git/repository",
            ),
            (
                "lp://~owner/project/+git/repository/+merge/42",
                "~owner/project/+git/repository",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(normalise_repository(input).unwrap(), expected);
        }
    }

    #[test]
    fn validates_branch_lookup_selectors() {
        let explicit: Request = serde_json::from_str(
            r#"{
                "op": "merge_proposal_for_branch",
                "repository": "launchpad-ui",
                "branch": "main"
            }"#,
        )
        .unwrap();
        explicit.validate().unwrap();

        let inferred: Request =
            serde_json::from_str(r#"{"op": "merge_proposal_for_branch"}"#).unwrap();
        inferred.validate().unwrap();

        let partial: Request = serde_json::from_str(
            r#"{"op": "merge_proposal_for_branch", "repository": "launchpad-ui"}"#,
        )
        .unwrap();
        assert!(
            partial
                .validate()
                .unwrap_err()
                .to_string()
                .contains("must be provided together")
        );
    }

    #[test]
    fn validates_discussion_filters() {
        let request: Request = serde_json::from_str(
            r#"{
                "op": "merge_proposal_discussion",
                "target": "lp://~owner/project/+git/repository/+merge/42",
                "current_diff_only": true,
                "unresolved_only": true,
                "comments": "inline",
                "since": "2026-09-21T09:30:00Z",
                "reviewer": "alice"
            }"#,
        )
        .unwrap();
        request.validate().unwrap();
        assert_eq!(request.comments, Some(DiscussionComments::Inline));
        assert_eq!(
            request.format.unwrap_or_default(),
            DiscussionFormat::Summary
        );
        assert!(request.since().unwrap().is_some());

        let invalid_timestamp: Request = serde_json::from_str(
            r#"{
                "op": "merge_proposal_discussion",
                "target": "lp://~owner/project/+git/repository/+merge/42",
                "since": "yesterday"
            }"#,
        )
        .unwrap();
        assert!(
            invalid_timestamp
                .validate()
                .unwrap_err()
                .to_string()
                .contains("RFC 3339")
        );

        let incompatible: Request = serde_json::from_str(
            r#"{
                "op": "merge_proposal_discussion",
                "target": "lp://~owner/project/+git/repository/+merge/42",
                "comments": "general",
                "unresolved_only": true
            }"#,
        )
        .unwrap();
        assert!(
            incompatible
                .validate()
                .unwrap_err()
                .to_string()
                .contains("require inline comments")
        );
    }

    #[test]
    fn creation_accepts_formal_prerequisite_and_bounded_waits() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "op": "merge_proposal_create",
            "repository": "~owner/project/+git/source",
            "source_ref": "feature",
            "target_ref": "main",
            "commit_message": "Dependent feature",
            "prerequisite_ref": "feature-base",
            "prerequisite_repository": "~owner/project/+git/base",
            "wait_for_index": true,
            "index_timeout_seconds": 300,
            "wait_for_preview": true,
            "preview_timeout_seconds": 1
        }))
        .unwrap();
        request.validate().unwrap();
        assert_eq!(request.prerequisite_ref.as_deref(), Some("feature-base"));
        assert_eq!(
            request.prerequisite_repository.as_deref(),
            Some("~owner/project/+git/base")
        );
    }

    #[test]
    fn rejects_invalid_creation_field_combinations() {
        let cases = [
            (
                serde_json::json!({"merge_prerequisite": "base"}),
                "use prerequisite_ref",
            ),
            (
                serde_json::json!({"prerequisite_ref": " \t"}),
                "prerequisite_ref cannot be empty",
            ),
            (
                serde_json::json!({"prerequisite_repository": "other"}),
                "requires prerequisite_ref",
            ),
            (
                serde_json::json!({"prerequisite_ref": "base", "prerequisite_repository": "  "}),
                "prerequisite_repository cannot be empty",
            ),
            (
                serde_json::json!({"index_timeout_seconds": 10}),
                "requires wait_for_index",
            ),
            (
                serde_json::json!({"wait_for_index": false, "index_timeout_seconds": 10}),
                "requires wait_for_index",
            ),
            (
                serde_json::json!({"wait_for_index": true, "index_timeout_seconds": 0}),
                "index_timeout_seconds must be between 1 and 300",
            ),
            (
                serde_json::json!({"wait_for_index": true, "index_timeout_seconds": 301}),
                "index_timeout_seconds must be between 1 and 300",
            ),
            (
                serde_json::json!({"preview_timeout_seconds": 10}),
                "requires wait_for_preview",
            ),
            (
                serde_json::json!({"wait_for_preview": false, "preview_timeout_seconds": 10}),
                "requires wait_for_preview",
            ),
            (
                serde_json::json!({"wait_for_preview": true, "preview_timeout_seconds": 0}),
                "preview_timeout_seconds must be between 1 and 300",
            ),
            (
                serde_json::json!({"wait_for_preview": true, "preview_timeout_seconds": 301}),
                "preview_timeout_seconds must be between 1 and 300",
            ),
        ];
        for (fields, expected) in cases {
            let mut input = serde_json::json!({
                "op": "merge_proposal_create",
                "repository": "~owner/project/+git/source",
                "source_ref": "feature",
                "target_ref": "main",
                "commit_message": "Dependent feature"
            });
            for (key, value) in fields.as_object().unwrap() {
                input[key.as_str()] = value.clone();
            }
            let request: Request = serde_json::from_value(input).unwrap();
            assert!(
                request
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains(expected),
                "{fields}"
            );
        }
    }

    #[test]
    fn creation_fields_are_not_accepted_for_other_operations() {
        let replacement: Request = serde_json::from_value(serde_json::json!({
            "op": "replace_merge_proposal_prerequisite",
            "target": "42",
            "merge_prerequisite": "base"
        }))
        .unwrap();
        replacement.validate().unwrap();

        for (field, value) in [
            ("prerequisite_ref", serde_json::json!("base")),
            ("prerequisite_repository", serde_json::json!("other")),
            ("wait_for_index", serde_json::json!(true)),
            ("index_timeout_seconds", serde_json::json!(10)),
            ("wait_for_preview", serde_json::json!(true)),
            ("preview_timeout_seconds", serde_json::json!(10)),
        ] {
            let mut input = serde_json::json!({
                "op": "replace_merge_proposal_prerequisite",
                "target": "42",
                "merge_prerequisite": "base"
            });
            input[field] = value;
            let request: Request = serde_json::from_value(input).unwrap();
            assert!(request.validate().is_err(), "{field}");
        }
    }

    #[test]
    fn creation_requires_a_nonblank_commit_message() {
        for message in [None, Some("  ")] {
            let request: Request = serde_json::from_value(serde_json::json!({
                "op": "merge_proposal_create",
                "repository": "~owner/project/+git/repo",
                "source_ref": "feature",
                "target_ref": "main",
                "commit_message": message
            }))
            .unwrap();
            assert!(
                request
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("commit_message")
            );
        }
    }

    #[test]
    fn accepts_paginated_search_limits() {
        let request: Request = serde_json::from_str(
            r#"{
                "op": "search_merge_proposals",
                "repository": "launchpad-ui",
                "limit": 100
            }"#,
        )
        .unwrap();
        request.validate().unwrap();
        assert_eq!(request.limit(), 100);
    }

    #[test]
    fn comment_urls_are_resources_not_merge_proposals_or_bugs() {
        for url in [
            "lp://~owner/project/+git/repository/+merge/42/comments/1392995",
            "https://bugs.launchpad.net/ubuntu/+bug/1/comments/0",
            "https://api.launchpad.net/devel/~owner/project/+git/repository/+merge/42/comments/1392995",
        ] {
            let target = ResourceTarget::parse(url).unwrap();
            assert_eq!(target.kind, ResourceKind::Generic);
        }
        let project = ResourceTarget::parse("https://api.launchpad.net/devel/project").unwrap();
        assert_eq!(project.path, "project");
    }

    #[test]
    fn task_edit_rejects_ambiguous_assignee_changes() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "op": "bug_task_edit",
            "target": "lp://project/+bug/42",
            "status": "Triaged",
            "assignee": "alice",
            "unassign": true
        }))
        .unwrap();
        assert!(
            request
                .validate()
                .unwrap_err()
                .to_string()
                .contains("assignee and unassign")
        );
    }

    #[test]
    fn rejects_parent_repository_path() {
        let error = validate_repository_path("src/../secret").unwrap_err();
        assert!(error.to_string().contains("cannot contain '..'"));
    }
}
