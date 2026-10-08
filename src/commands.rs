use serde::Serialize;

#[derive(Serialize)]
pub struct CommandSpec {
    pub group: &'static str,
    pub action: &'static str,
    pub operation: &'static str,
    pub summary: &'static str,
    pub fields: &'static [&'static str],
    pub required: &'static [&'static str],
    pub positional: Option<&'static str>,
    pub effect: &'static str,
}

macro_rules! command {
    ($group:literal, $action:literal, $op:literal, $summary:literal, [$($field:literal),*], [$($required:literal),*], $positional:expr, $effect:literal) => {
        CommandSpec { group: $group, action: $action, operation: $op, summary: $summary,
            fields: &[$($field),*], required: &[$($required),*], positional: $positional, effect: $effect }
    };
}

pub const COMMANDS: &[CommandSpec] = &[
    command!(
        "resource",
        "view",
        "resource_view",
        "Read a Launchpad resource or preview diff",
        ["target", "preview_diff_id"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "bug",
        "view",
        "resource_view",
        "Read a bug, its tasks, and comments",
        ["target"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "bug",
        "search",
        "search_bugs",
        "Search bug tasks on a project or distribution",
        ["target", "query", "status", "importance", "tags", "limit"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "bug",
        "create",
        "bug_create",
        "File a bug against a Launchpad target",
        ["target", "title", "description", "information_type", "tags"],
        ["target", "title", "description"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "bug",
        "edit",
        "bug_edit",
        "Edit bug metadata, not task status",
        ["target", "title", "description", "tags"],
        ["target"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "bug",
        "comment",
        "comment",
        "Add a bug message",
        ["target", "body", "subject"],
        ["target", "body"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "bug-task",
        "edit",
        "bug_task_edit",
        "Edit a target-specific bug task",
        ["target", "status", "importance", "assignee", "unassign"],
        ["target"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "project",
        "view",
        "resource_view",
        "Read a Launchpad project",
        ["target"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "project",
        "edit",
        "project_edit",
        "Edit project settings",
        [
            "target",
            "summary",
            "description",
            "bug_reporting_guidelines",
            "official_bug_tags"
        ],
        ["target"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "repository",
        "view",
        "repo_view",
        "Resolve and read a Launchpad Git repository",
        ["repository"],
        ["repository"],
        Some("repository"),
        "read"
    ),
    command!(
        "repository",
        "file",
        "file_read",
        "Read a file over HTTPS (default) or authenticated SSH",
        ["repository", "path", "branch", "transport"],
        ["repository", "path"],
        Some("repository"),
        "read"
    ),
    command!(
        "repository",
        "edit",
        "repository_edit",
        "Edit Git repository settings",
        ["target", "description", "default_branch"],
        ["target"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "view",
        "resource_view",
        "Read a Launchpad merge proposal",
        ["target", "preview_diff_id"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "diff",
        "resource_view",
        "Read a merge proposal preview diff",
        ["target", "preview_diff_id"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "list",
        "search_merge_proposals",
        "Search proposals for a repository",
        ["repository", "status", "limit"],
        ["repository"],
        Some("repository"),
        "read"
    ),
    command!(
        "merge-proposal",
        "for-branch",
        "merge_proposal_for_branch",
        "Find proposals by source repository and ref",
        [
            "repository",
            "branch",
            "target_repository",
            "target_branch",
            "status",
            "latest",
            "include_superseded",
            "limit"
        ],
        [],
        None,
        "read"
    ),
    command!(
        "merge-proposal",
        "current",
        "current_merge_proposal",
        "Find proposals from the current Launchpad Git checkout",
        [
            "target_repository",
            "target_branch",
            "status",
            "latest",
            "include_superseded",
            "limit"
        ],
        [],
        None,
        "read"
    ),
    command!(
        "merge-proposal",
        "discussion",
        "merge_proposal_discussion",
        "Read the review summary and structured discussion",
        [
            "target",
            "repository",
            "branch",
            "target_repository",
            "target_branch",
            "status",
            "latest",
            "include_superseded",
            "limit",
            "current_diff_only",
            "unresolved_only",
            "comments",
            "format",
            "since",
            "reviewer"
        ],
        [],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "bugs",
        "merge_proposal_bugs",
        "Read linked bugs (requires authentication)",
        ["target", "limit"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "preview-diffs",
        "preview_diffs",
        "List preview diff snapshots",
        ["target", "limit"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "inline-comments",
        "inline_comments",
        "Read inline comments on a preview diff",
        ["target", "preview_diff_id", "limit"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "drafts",
        "review_drafts",
        "Read private review drafts",
        ["target", "preview_diff_id"],
        ["target"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "map-line",
        "diff_line_map",
        "Map a file line to a preview diff line",
        ["target", "preview_diff_id", "path", "file_line", "side"],
        ["target", "path", "file_line", "side"],
        Some("target"),
        "read"
    ),
    command!(
        "merge-proposal",
        "create",
        "merge_proposal_create",
        "Propose merging one Git ref into another",
        [
            "repository",
            "source_ref",
            "target_repository",
            "target_ref",
            "commit_message",
            "description",
            "prerequisite_ref",
            "prerequisite_repository",
            "needs_review",
            "wait_for_index",
            "index_timeout_seconds",
            "wait_for_preview",
            "preview_timeout_seconds"
        ],
        ["repository", "source_ref", "target_ref", "commit_message"],
        None,
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "edit",
        "merge_proposal_edit",
        "Edit proposal metadata",
        ["target", "commit_message", "description", "reviewed_revid"],
        ["target"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "replace-prerequisite",
        "replace_merge_proposal_prerequisite",
        "Unsupported by the API; use Launchpad's web resubmit flow",
        ["target", "merge_prerequisite"],
        ["target", "merge_prerequisite"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "link-bug",
        "merge_proposal_link_bug",
        "Link a bug to a proposal",
        ["target", "bug_id"],
        ["target", "bug_id"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "unlink-bug",
        "merge_proposal_unlink_bug",
        "Unlink a bug from a proposal",
        ["target", "bug_id"],
        ["target", "bug_id"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "comment",
        "comment",
        "Add a proposal comment and optional review vote",
        ["target", "body", "subject", "vote"],
        ["target", "body"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "draft",
        "review_draft_update",
        "Save or clear an inline review draft",
        [
            "target",
            "preview_diff_id",
            "path",
            "file_line",
            "side",
            "body"
        ],
        ["target", "path", "file_line", "side"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "review",
        "review_submit",
        "Publish drafts and an optional review vote",
        ["target", "preview_diff_id", "body", "vote"],
        ["target"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "set-status",
        "set_merge_proposal_status",
        "Set the Launchpad review queue status",
        ["target", "status", "reviewed_revid"],
        ["target", "status"],
        Some("target"),
        "remote-write"
    ),
    command!(
        "merge-proposal",
        "checkout",
        "merge_proposal_checkout",
        "Clone the source ref into a new local directory",
        ["target", "directory"],
        ["target"],
        Some("target"),
        "local-write"
    ),
    command!(
        "merge-proposal",
        "push",
        "merge_proposal_push",
        "Push the current branch to its Launchpad origin",
        ["directory", "force_with_lease"],
        [],
        None,
        "git-push"
    ),
    command!(
        "comment",
        "edit",
        "comment_edit",
        "Edit an existing Launchpad comment",
        ["target", "body"],
        ["target", "body"],
        Some("target"),
        "remote-write"
    ),
];
