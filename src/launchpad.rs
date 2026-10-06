use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset};
use futures::{StreamExt, stream};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::sleep;
use url::Url;

use crate::auth;
use crate::client::{Collection, GitRef, LaunchpadClient, LpError, list_git_refs, urlenc};
use crate::diff;
use crate::error::Error;
use crate::local_git::{self, CheckoutSpec};
use crate::render;
use crate::request::{
    DiscussionFormat, OneOrMany, Operation, Request, ResourceKind, ResourceTarget,
    normalise_repository,
};
use crate::response::OperationResult;
use crate::result::Result;

const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 50;
const MAX_BRANCH_FALLBACK_ITEMS: usize = 250;
const MAX_REPOSITORY_CANDIDATES: usize = 100;
const GIT_PATH_SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'+');

pub async fn execute(request: &Request) -> Result<OperationResult> {
    request.validate()?;
    if request.op == Operation::FileRead {
        return read_repository_file(request).await;
    }
    if request.op == Operation::MergeProposalPush {
        return local_git::push(request).await;
    }

    let client = launchpad_client(request.op.requires_authentication())?;
    match request.op {
        Operation::ResourceView => view_resource(&client, request).await,
        Operation::RepoView => view_repository(&client, request).await,
        Operation::SearchBugs => search_bugs(&client, request).await,
        Operation::SearchMergeProposals => search_merge_proposals(&client, request).await,
        Operation::MergeProposalForBranch => view_merge_proposal_for_branch(&client, request).await,
        Operation::CurrentMergeProposal => view_current_merge_proposal(&client, request).await,
        Operation::MergeProposalDiscussion => {
            view_merge_proposal_discussion(&client, request).await
        }
        Operation::MergeProposalBugs => view_merge_proposal_bugs(&client, request).await,
        Operation::PreviewDiffs => view_preview_diffs(&client, request).await,
        Operation::InlineComments => view_inline_comments(&client, request).await,
        Operation::ReviewDrafts => view_review_drafts(&client, request).await,
        Operation::DiffLineMap => map_diff_line(&client, request).await,
        Operation::BugCreate => create_bug(&client, request).await,
        Operation::BugEdit => edit_bug(&client, request).await,
        Operation::BugTaskEdit => edit_bug_task(&client, request).await,
        Operation::ProjectEdit => edit_project(&client, request).await,
        Operation::RepositoryEdit => edit_repository(&client, request).await,
        Operation::MergeProposalCreate => create_merge_proposal(&client, request).await,
        Operation::MergeProposalEdit => edit_merge_proposal(&client, request).await,
        Operation::ReplaceMergeProposalPrerequisite => {
            replace_merge_proposal_prerequisite(&client, request).await
        }
        Operation::MergeProposalLinkBug | Operation::MergeProposalUnlinkBug => {
            change_merge_proposal_bug_link(&client, request).await
        }
        Operation::Comment => add_comment(&client, request).await,
        Operation::CommentEdit => edit_comment(&client, request).await,
        Operation::ReviewDraftUpdate => update_review_draft(&client, request).await,
        Operation::ReviewSubmit => submit_review(&client, request).await,
        Operation::SetMergeProposalStatus => set_merge_proposal_status(&client, request).await,
        Operation::MergeProposalCheckout => checkout_merge_proposal(&client, request).await,
        Operation::FileRead | Operation::MergeProposalPush => {
            Err(Error::invalid("operation dispatch is inconsistent"))
        }
    }
}

fn launchpad_client(require_authentication: bool) -> Result<LaunchpadClient> {
    crate::api::client(require_authentication)
}

fn api_base_url() -> Result<String> {
    Ok(auth::api_base_url()?)
}

async fn view_resource(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let mut target = ResourceTarget::parse(request.target()?)?;
    select_preview_diff(&mut target, request.preview_diff_id)?;
    match &target.kind {
        ResourceKind::Bug { id } => view_bug(client, &target, *id).await,
        ResourceKind::MergeProposal { repository, id } => {
            view_merge_proposal(client, &target, repository, *id).await
        }
        ResourceKind::MergeProposalId { .. } => {
            let (_, proposal) = resolve_merge_proposal_target(client, request.target()?).await?;
            view_merge_proposal_value(client, &target, proposal).await
        }
        ResourceKind::Repository => {
            let repository = get_repository(client, &target.path).await?;
            let source_url = render::text_field(&repository, "web_link").map(str::to_owned);
            let text = render::render_repository(&repository);
            let details = json!({ "kind": "git_repository", "target": target.path, "repository": repository });
            Ok(OperationResult::new(text)
                .with_source_url(source_url)
                .with_details(details))
        }
        ResourceKind::Generic => {
            if target.diff {
                return Err(Error::invalid("/diff is only valid for a merge proposal"));
            }
            let resource: Value = client.get(&format!("/{}", target.path)).await?;
            let source_url = render::text_field(&resource, "web_link").map(str::to_owned);
            let kind = resource_kind(&resource);
            let text = render::render_generic(&resource);
            let details = json!({ "kind": kind, "target": target.path, "resource": resource });
            Ok(OperationResult::new(text)
                .with_source_url(source_url)
                .with_details(details))
        }
    }
}

fn select_preview_diff(target: &mut ResourceTarget, preview_diff_id: Option<u64>) -> Result<()> {
    let Some(preview_diff_id) = preview_diff_id else {
        return Ok(());
    };
    if preview_diff_id == 0 {
        return Err(Error::invalid("preview_diff_id must be greater than zero"));
    }
    if !matches!(
        &target.kind,
        ResourceKind::MergeProposal { .. } | ResourceKind::MergeProposalId { .. }
    ) {
        return Err(Error::invalid(
            "preview_diff_id requires a merge proposal target",
        ));
    }
    if target
        .preview_diff_id
        .is_some_and(|id| id != preview_diff_id)
    {
        return Err(Error::invalid(
            "target and preview_diff_id select different snapshots",
        ));
    }
    target.diff = true;
    target.preview_diff_id = Some(preview_diff_id);
    Ok(())
}

async fn view_bug(
    client: &LaunchpadClient,
    target: &ResourceTarget,
    id: u64,
) -> Result<OperationResult> {
    if target.diff {
        return Err(Error::invalid("/diff is only valid for a merge proposal"));
    }
    let bug: Value = client.get(&format!("/bugs/{id}")).await?;
    let tasks = fetch_entries(
        client,
        &client.url(&format!("/bugs/{id}/bug_tasks")),
        MAX_ITEMS,
    )
    .await?;
    let comments = if target.include_comments {
        fetch_entries(
            client,
            &client.url(&format!("/bugs/{id}/messages")),
            target.comment_limit,
        )
        .await?
    } else {
        Vec::new()
    };
    let text = render::render_bug(
        &bug,
        &tasks,
        &comments,
        target.include_comments,
        target.comment_limit,
    );
    let source_url = render::text_field(&bug, "web_link").map(str::to_owned);
    let details = json!({ "kind": "bug", "target": target.path, "id": id, "bug": bug, "tasks": tasks, "comments": comments });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_merge_proposal(
    client: &LaunchpadClient,
    target: &ResourceTarget,
    repository: &str,
    id: u64,
) -> Result<OperationResult> {
    let proposal = get_merge_proposal(client, repository, id).await?;
    view_merge_proposal_value(client, target, proposal).await
}

async fn view_merge_proposal_value(
    client: &LaunchpadClient,
    target: &ResourceTarget,
    proposal: Value,
) -> Result<OperationResult> {
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    if target.diff {
        let preview_diff = get_preview_diff(client, &proposal, target.preview_diff_id).await?;
        let preview_diff_id = preview_diff_id(&preview_diff)?;
        let mut text = get_diff_text(&preview_diff).await?;
        let truncated = text.len() > MAX_FILE_BYTES;
        if truncated {
            text.truncate(floor_char_boundary(&text, MAX_FILE_BYTES));
            text.push_str("\n\n[Diff truncated at 2 MiB]");
        }
        let details = json!({
            "kind": "branch_merge_proposal",
            "diff": true,
            "preview_diff_id": preview_diff_id,
            "stale": preview_diff.get("stale").and_then(Value::as_bool).unwrap_or(false),
            "truncated": truncated,
            "prerequisite_ref": render::scalar_field(&proposal, "prerequisite_git_path"),
            "prerequisite_repository_link": render::scalar_field(&proposal, "prerequisite_git_repository_link"),
        });
        return Ok(OperationResult::new(text)
            .with_source_url(source_url)
            .with_details(details));
    }
    let comments_url = render::text_field(&proposal, "all_comments_collection_link")
        .ok_or_else(|| Error::invalid("merge proposal has no comments collection"))?;
    let comments = if target.include_comments {
        fetch_entries(client, comments_url, target.comment_limit).await?
    } else {
        Vec::new()
    };
    let votes_url = render::text_field(&proposal, "votes_collection_link")
        .ok_or_else(|| Error::invalid("merge proposal has no review vote collection"))?;
    let votes = fetch_entries(client, votes_url, MAX_ITEMS)
        .await
        .unwrap_or_default();
    let text = render::render_proposal(
        &proposal,
        &comments,
        &votes,
        target.include_comments,
        target.comment_limit,
    );
    let mut details = proposal_details(client, &proposal).await?;
    if let Some(details) = details.as_object_mut() {
        details.insert("proposal".to_owned(), proposal);
        details.insert("comments".to_owned(), json!(comments));
        details.insert("votes".to_owned(), json!(votes));
    }
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_merge_proposal_bugs(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let collection = render::text_field(&proposal, "bugs_collection_link")
        .ok_or_else(|| Error::invalid("merge proposal has no bugs collection"))?;
    let bugs = fetch_all_entries(client, collection).await?;
    let id = render::resource_id(&proposal).unwrap_or_else(|| "?".to_owned());
    let mut lines = vec![format!("# Bugs linked to merge proposal {id}")];
    for bug in &bugs {
        let bug_id = render::scalar_field(bug, "id").unwrap_or_else(|| "?".to_owned());
        let title = render::text_field(bug, "title").unwrap_or("Untitled");
        let url = render::text_field(bug, "web_link").unwrap_or_default();
        lines.push(format!("- [#{bug_id}: {title}]({url})"));
    }
    if bugs.is_empty() {
        lines.push("No linked bugs.".to_owned());
    }
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let details = json!({ "kind": "branch_merge_proposal_bugs", "proposal_id": id, "bugs": bugs });
    Ok(OperationResult::new(lines.join("\n"))
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_preview_diffs(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let mut preview_diffs =
        if render::text_field(&proposal, "preview_diffs_collection_link").is_some() {
            get_preview_diffs(client, &proposal).await?
        } else if render::text_field(&proposal, "preview_diff_link").is_none() {
            Vec::new()
        } else {
            return Err(Error::invalid(
                "merge proposal has a current preview diff but no preview diff history",
            ));
        };
    let mut deleted_files = Vec::new();
    for preview_diff in &mut preview_diffs {
        let Some(diffstat) = preview_diff
            .get_mut("diffstat")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        if !diffstat.contains_key("dev/null") && !diffstat.contains_key("/dev/null") {
            continue;
        }
        let diff_text = get_diff_text(preview_diff).await?;
        let deletions = diff::deleted_file_stats(&diff_text);
        if deletions.is_empty() {
            return Err(Error::invalid(
                "preview diff contains a dev/null diffstat entry without a deleted source path",
            ));
        }
        let id = preview_diff_id(preview_diff)?;
        let diffstat = preview_diff
            .get_mut("diffstat")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| Error::invalid("preview diff has invalid diffstat"))?;
        diffstat.remove("dev/null");
        diffstat.remove("/dev/null");
        for (path, counts) in deletions {
            diffstat
                .entry(path.clone())
                .or_insert_with(|| json!(counts));
            deleted_files.push(format!(
                "- Preview diff {id}: {path:?} (+{}/-{})",
                counts[0], counts[1]
            ));
        }
    }
    let current = if let Some(link) = render::text_field(&proposal, "preview_diff_link") {
        Some(client.get_url::<Value>(link).await?)
    } else {
        None
    };
    let current_id = current.as_ref().map(preview_diff_id).transpose()?;
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let proposal_id =
        render::resource_id(&proposal).ok_or_else(|| Error::invalid("merge proposal has no ID"))?;
    let mut text = render::render_preview_diffs(
        &preview_diffs,
        current_id.unwrap_or(0),
        &proposal_id,
        render::text_field(&proposal, "prerequisite_git_path"),
    );
    let mut revisions = String::new();
    if let Some(current) = &current {
        for (label, key) in [
            ("Prerequisite revision", "prerequisite_revision_id"),
            ("Source revision", "source_revision_id"),
            ("Target revision", "target_revision_id"),
        ] {
            if let Some(revision) = render::text_field(current, key) {
                revisions.push_str(&format!("\n- **{label}:** `{revision}`"));
            }
        }
        if !revisions.is_empty() {
            text.push_str("\n\n## Current preview revisions");
            text.push_str(&revisions);
        }
    } else {
        text.push_str("\n\nPreview generation is pending; retry this operation later.");
    }
    if !deleted_files.is_empty() {
        text.push_str("\n\n## Deleted files\n");
        text.push_str(&deleted_files.join("\n"));
    }
    let details = json!({
        "kind": "preview_diff_history",
        "proposal_id": proposal_id,
        "state": if current.is_some() { "ready" } else { "pending" },
        "retryable": current.is_none(),
        "diffs": preview_diffs,
        "preview_diffs": preview_diffs,
        "current_preview_diff_id": current_id,
        "current_preview_diff": current.as_ref().map(|preview| json!({
            "id": current_id,
            "prerequisite_revision_id": preview.get("prerequisite_revision_id"),
            "source_revision_id": preview.get("source_revision_id"),
            "target_revision_id": preview.get("target_revision_id"),
        })),
        "prerequisite_ref": render::scalar_field(&proposal, "prerequisite_git_path"),
        "prerequisite_repository_link": render::scalar_field(&proposal, "prerequisite_git_repository_link"),
    });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_inline_comments(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let preview_diff_id = request.preview_diff_id()?;
    let (_, proposal) = request_merge_proposal(client, request).await?;
    get_preview_diff(client, &proposal, Some(preview_diff_id)).await?;
    let comments = inline_comments(client, &proposal, preview_diff_id).await?;
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let proposal_id =
        render::resource_id(&proposal).ok_or_else(|| Error::invalid("merge proposal has no ID"))?;
    let text = render::render_inline_comments(&comments, preview_diff_id, &proposal_id);
    let details = json!({
        "kind": "inline_comments",
        "preview_diff_id": preview_diff_id,
        "comments": comments,
    });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_review_drafts(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let preview_diff_id = request.preview_diff_id()?;
    let (_, proposal) = request_merge_proposal(client, request).await?;
    get_preview_diff(client, &proposal, Some(preview_diff_id)).await?;
    let drafts = review_drafts(client, &proposal, preview_diff_id).await?;
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let proposal_id =
        render::resource_id(&proposal).ok_or_else(|| Error::invalid("merge proposal has no ID"))?;
    let text = render::render_review_drafts(&drafts, preview_diff_id, &proposal_id);
    let details = json!({
        "kind": "review_drafts",
        "preview_diff_id": preview_diff_id,
        "drafts": drafts,
    });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn map_diff_line(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let preview_diff_id = request.preview_diff_id()?;
    let file_line = request.file_line()?;
    let side = request.side()?;
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let preview_diff = get_preview_diff(client, &proposal, Some(preview_diff_id)).await?;
    let diff_text = get_diff_text(&preview_diff).await?;
    let location = diff::map_file_line(&diff_text, request.path("path")?, side, file_line)
        .ok_or_else(|| Error::invalid("file line is not present in the selected preview diff"))?;
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let proposal_id =
        render::resource_id(&proposal).ok_or_else(|| Error::invalid("merge proposal has no ID"))?;
    let text = render::render_diff_location(&location, preview_diff_id, &proposal_id);
    let details = json!({
        "kind": "diff_line_mapping",
        "preview_diff_id": preview_diff_id,
        "location": location,
    });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_repository(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let repository = get_repository(client, request.repository()?).await?;
    let source_url = render::text_field(&repository, "web_link").map(str::to_owned);
    let text = render::render_repository(&repository);
    let details = json!({ "kind": "git_repository", "repository": repository });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn search_bugs(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = request.target()?;
    let mut url = Url::parse(&client.url(&format!("/{target}"))).map_err(|source| Error::Url {
        url: client.url(&format!("/{target}")),
        source,
    })?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("ws.op", "searchTasks");
        query.append_pair("omit_duplicates", "true");
        query.append_pair("ws.size", &request.limit().to_string());
        append_values(&mut query, "status", request.status.as_ref());
        append_values(&mut query, "importance", request.importance.as_ref());
        if let Some(search_text) = request.query.as_deref() {
            query.append_pair("search_text", search_text);
        }
        if let Some(tags) = &request.tags {
            for tag in tags {
                query.append_pair("tags", tag);
            }
        }
    }
    let tasks = fetch_entries(client, url.as_str(), request.limit()).await?;
    let text = render::render_bug_search(target, request.query.as_deref(), &tasks);
    let details = json!({ "count": tasks.len(), "target": target, "tasks": tasks });
    Ok(OperationResult::new(text).with_details(details))
}

async fn search_merge_proposals(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let repository_path = normalise_repository(request.repository()?)?;
    let repository = get_repository(client, &repository_path).await?;
    let repository_url = render::text_field(&repository, "self_link")
        .ok_or_else(|| Error::invalid("Launchpad repository has no API link"))?;
    let mut url = Url::parse(repository_url).map_err(|source| Error::Url {
        url: repository_url.to_owned(),
        source,
    })?;
    let requested_limit = request.limit();
    let fetch_limit = requested_limit + 1;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("ws.op", "getMergeProposals");
        query.append_pair("ws.size", &fetch_limit.to_string());
        append_values(&mut query, "status", request.status.as_ref());
    }
    let mut proposals = fetch_entries(client, url.as_str(), fetch_limit).await?;
    let truncated = proposals.len() > requested_limit;
    proposals.truncate(requested_limit);
    let text = render::render_proposal_search(&repository, &proposals, request.limit, truncated);
    let name = render::text_field(&repository, "unique_name");
    let details = json!({
        "count": proposals.len(),
        "limit": request.limit,
        "truncated": truncated,
        "repository": name,
        "proposals": proposals,
    });
    Ok(OperationResult::new(text).with_details(details))
}

async fn view_merge_proposal_for_branch(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (repository_path, branch, inferred, requested_repository) =
        branch_selector(request).await?;
    let selection = merge_proposal_for_branch(client, &repository_path, &branch, request).await?;
    proposal_lookup_result(
        client,
        request,
        selection,
        ProposalLookupContext {
            requested_repository: &requested_repository,
            branch: &branch,
            inferred,
            kind: "merge_proposal_for_branch",
            metadata: None,
        },
    )
    .await
}

async fn view_current_merge_proposal(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let current = local_git::current_repository().await?;
    let requested_repository = &current.selected_remote.url;
    let repository_path = normalise_repository(requested_repository)?;
    let selection =
        merge_proposal_for_branch(client, &repository_path, &current.branch, request).await?;
    let context = json!({
        "working_directory": current.working_directory,
        "selected_remote": {
            "name": current.selected_remote.name,
            "url": current.selected_remote.url,
        },
        "inspected_remotes": current.remotes.into_iter().map(|remote| {
            json!({ "name": remote.name, "url": remote.url })
        }).collect::<Vec<_>>(),
    });
    proposal_lookup_result(
        client,
        request,
        selection,
        ProposalLookupContext {
            requested_repository,
            branch: &current.branch,
            inferred: true,
            kind: "current_merge_proposal",
            metadata: Some(context),
        },
    )
    .await
}

struct ProposalLookupContext<'value> {
    requested_repository: &'value str,
    branch: &'value str,
    inferred: bool,
    kind: &'value str,
    metadata: Option<Value>,
}

async fn proposal_lookup_result(
    client: &LaunchpadClient,
    request: &Request,
    selection: ProposalLookup,
    context: ProposalLookupContext<'_>,
) -> Result<OperationResult> {
    let selection = match selection {
        ProposalLookup::Found(selection) => selection,
        ProposalLookup::Missing {
            repository,
            inspected_repositories,
        } => {
            let branch = normalise_ref(context.branch);
            let text = format!("# No merge proposal found for {repository}:{branch}");
            let details = json!({
                "kind": context.kind,
                "found": false,
                "repository": repository,
                "requested_repository": context.requested_repository,
                "inspected_repositories": inspected_repositories,
                "branch": branch,
                "inferred": context.inferred,
                "selection_filters": proposal_selection_filters(request),
                "context": context.metadata,
            });
            return Ok(OperationResult::new(text).with_details(details));
        }
    };
    let canonical_repository = render::text_field(&selection.repository, "unique_name")
        .unwrap_or(repository_identifier(&selection.repository));
    let source_url = render::text_field(&selection.proposal, "web_link").map(str::to_owned);
    let resolution = if context.inferred {
        "current Git checkout"
    } else if selection.related_repository {
        "related target repository"
    } else {
        "explicit repository and branch"
    };
    let text = render::render_proposal_lookup(
        &selection.proposal,
        &selection.alternatives,
        canonical_repository,
        &normalise_ref(context.branch),
        resolution,
        &selection.reason,
    );
    let selected_proposal = proposal_details(client, &selection.proposal).await?;
    let details = json!({
        "kind": context.kind,
        "found": true,
        "repository": canonical_repository,
        "requested_repository": context.requested_repository,
        "branch": normalise_ref(context.branch),
        "inferred": context.inferred,
        "related_repository": selection.related_repository,
        "candidate_count": selection.candidate_count,
        "matching_proposals": selection.alternatives.len() + 1,
        "selected_proposal": selected_proposal,
        "selection_reason": selection.reason,
        "selection_filters": proposal_selection_filters(request),
        "other_proposals": selection.alternatives.iter().map(proposal_summary).collect::<Vec<_>>(),
        "context": context.metadata,
    });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn view_merge_proposal_discussion(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let has_target = request
        .target
        .as_deref()
        .is_some_and(|target| !target.trim().is_empty());
    let (proposal, lookup, selection_reason) = if has_target {
        let (_, proposal) = request_merge_proposal(client, request).await?;
        (
            proposal,
            None,
            "selected the explicitly requested merge proposal".to_owned(),
        )
    } else {
        let (repository_path, branch, inferred, requested_repository) =
            branch_selector(request).await?;
        let selection =
            merge_proposal_for_branch(client, &repository_path, &branch, request).await?;
        let selection = match selection {
            ProposalLookup::Found(selection) => selection,
            missing @ ProposalLookup::Missing { .. } => {
                let result = proposal_lookup_result(
                    client,
                    request,
                    missing,
                    ProposalLookupContext {
                        requested_repository: &requested_repository,
                        branch: &branch,
                        inferred,
                        kind: "merge_proposal_discussion",
                        metadata: None,
                    },
                )
                .await?;
                return Ok(format_discussion_result(
                    result,
                    request.format.unwrap_or_default(),
                ));
            }
        };
        let canonical_repository = render::text_field(&selection.repository, "unique_name")
            .unwrap_or(repository_identifier(&selection.repository));
        let lookup = json!({
            "repository": canonical_repository,
            "found": true,
            "requested_repository": requested_repository,
            "branch": normalise_ref(&branch),
            "inferred": inferred,
            "related_repository": selection.related_repository,
            "candidate_count": selection.candidate_count,
            "matching_proposals": selection.alternatives.len() + 1,
            "selection_filters": proposal_selection_filters(request),
        });
        (selection.proposal, Some(lookup), selection.reason)
    };

    let comments_kind = request.comments.unwrap_or_default();
    let since = request.since()?;
    let reviewer = request.reviewer.as_deref().map(str::trim);
    let (current_preview_diff_id, current_preview_diff_stale) =
        if render::text_field(&proposal, "preview_diff_link").is_some() {
            let current_preview_diff = get_preview_diff(client, &proposal, None).await?;
            (
                Some(preview_diff_id(&current_preview_diff)?),
                current_preview_diff
                    .get("stale")
                    .and_then(Value::as_bool)
                    .unwrap_or_default(),
            )
        } else {
            (None, false)
        };
    let mut people = HashMap::new();
    let mut identity_resolution_failures = HashSet::new();

    let (general_comments, review_votes, review_requests) = if comments_kind.includes_general() {
        let comments_url = render::text_field(&proposal, "all_comments_collection_link")
            .ok_or_else(|| Error::invalid("merge proposal has no comments collection"))?;
        let raw_comments = fetch_all_entries(client, comments_url).await?;
        let votes_url = render::text_field(&proposal, "votes_collection_link")
            .ok_or_else(|| Error::invalid("merge proposal has no review vote collection"))?;
        let raw_review_requests = fetch_all_entries(client, votes_url).await?;
        load_people(
            client,
            raw_comments.iter().chain(&raw_review_requests),
            &mut people,
            &mut identity_resolution_failures,
        )
        .await;
        let comments = raw_comments
            .iter()
            .map(|comment| normalise_general_comment(comment, &people))
            .collect::<Vec<_>>();
        let general_comments = comments
            .iter()
            .filter(|comment| {
                comment.get("body").is_some_and(|body| !body.is_null())
                    && matches_discussion_filters(comment, "author", since.as_ref(), reviewer)
            })
            .cloned()
            .collect();
        let review_votes = comments
            .into_iter()
            .filter(|comment| {
                comment.get("vote").is_some_and(|vote| !vote.is_null())
                    && matches_discussion_filters(comment, "author", since.as_ref(), reviewer)
            })
            .collect();
        let review_requests = raw_review_requests
            .iter()
            .map(|vote| normalise_review_assignment(vote, &people))
            .filter(|assignment| {
                matches_discussion_filters(assignment, "reviewer", since.as_ref(), reviewer)
            })
            .collect();
        (general_comments, review_votes, review_requests)
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    let preview_diffs = if comments_kind.includes_inline() {
        get_preview_diffs(client, &proposal).await?
    } else {
        Vec::new()
    };
    let mut diff_discussions = Vec::with_capacity(preview_diffs.len());
    let mut inline_threads = Vec::new();
    for preview_diff in &preview_diffs {
        let id = preview_diff_id(preview_diff)?;
        let current = current_preview_diff_id == Some(id);
        if request.current_diff_only.unwrap_or_default() && !current {
            continue;
        }
        let stale = preview_diff
            .get("stale")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = thread_state(current, stale);
        if request.unresolved_only.unwrap_or_default() && state != "open" {
            continue;
        }
        let comments = inline_comments(client, &proposal, id).await?;
        load_people(
            client,
            comments.iter(),
            &mut people,
            &mut identity_resolution_failures,
        )
        .await;
        let diff_text = if comments.is_empty() {
            None
        } else {
            Some(get_diff_text(preview_diff).await?)
        };
        let mut grouped = BTreeMap::<usize, Vec<&Value>>::new();
        for comment in &comments {
            let line = render::text_field(comment, "line_number")
                .and_then(|line| line.parse().ok())
                .ok_or_else(|| {
                    Error::invalid("Launchpad returned an inline comment without a diff line")
                })?;
            grouped.entry(line).or_default().push(comment);
        }
        let mut threads = Vec::with_capacity(grouped.len());
        for (line, mut comments) in grouped {
            comments.sort_by_key(|comment| render::text_field(comment, "date"));
            let location = diff_text
                .as_deref()
                .and_then(|diff_text| diff::locate_diff_line(diff_text, line));
            let comments = comments
                .into_iter()
                .enumerate()
                .map(|(index, comment)| normalise_inline_comment(comment, index, &people))
                .filter(|comment| {
                    matches_discussion_filters(comment, "author", since.as_ref(), reviewer)
                })
                .collect::<Vec<_>>();
            if comments.is_empty() {
                continue;
            }
            let source_line = location
                .as_ref()
                .filter(|location| location.side == diff::DiffSide::Original)
                .map(|location| location.file_line);
            let target_line = location
                .as_ref()
                .filter(|location| location.side == diff::DiffSide::Modified)
                .map(|location| location.file_line);
            let thread = json!({
                "thread_id": format!("{id}:{line}"),
                "preview_diff_id": id,
                "current": current,
                "stale": stale,
                "state": state,
                "diff_line": line,
                "file": location.as_ref().map(|location| location.path.as_str()),
                "source_line": source_line,
                "target_line": target_line,
                "location": location,
                "comments": comments,
            });
            inline_threads.push(thread.clone());
            threads.push(thread);
        }
        diff_discussions.push(json!({
            "preview_diff_id": id,
            "date_created": render::text_field(preview_diff, "date_created"),
            "current": current,
            "stale": stale,
            "state": state,
            "threads": threads,
        }));
    }
    diff_discussions.sort_by_key(|diff| {
        diff.get("preview_diff_id")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    });
    inline_threads.sort_by_key(|thread| {
        (
            thread
                .get("preview_diff_id")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            thread
                .get("diff_line")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        )
    });

    let preview_diff_summaries = diff_discussions
        .iter()
        .map(|preview_diff| {
            json!({
                "preview_diff_id": preview_diff.get("preview_diff_id"),
                "date_created": preview_diff.get("date_created"),
                "current": preview_diff.get("current"),
                "stale": preview_diff.get("stale"),
                "thread_count": preview_diff
                    .get("threads")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    let review_summary = build_review_summary(
        &general_comments,
        &review_votes,
        &review_requests,
        &inline_threads,
    );
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let markdown = render::render_proposal_discussion(
        &proposal,
        current_preview_diff_id,
        &review_summary,
        render::DiscussionSections {
            general: comments_kind.includes_general(),
            inline: comments_kind.includes_inline(),
            current_preview_diff_stale,
            identity_resolution_failures: identity_resolution_failures.len(),
        },
    );
    let coverage = if !comments_kind.includes_inline() {
        "not_requested"
    } else if request.current_diff_only.unwrap_or_default()
        || request.unresolved_only.unwrap_or_default()
    {
        "current_preview_diff"
    } else {
        "all_preview_diffs"
    };
    let format = request.format.unwrap_or_default();
    let details = json!({
        "kind": "merge_proposal_discussion",
        "found": true,
        "selected_proposal": proposal_details(client, &proposal).await?,
        "selection_reason": selection_reason,
        "lookup": lookup,
        "current_preview_diff_id": current_preview_diff_id,
        "current_preview_diff_stale": current_preview_diff_stale,
        "coverage": coverage,
        "format": format,
        "filters": {
            "comments": comments_kind,
            "current_diff_only": request.current_diff_only.unwrap_or_default(),
            "unresolved_only": request.unresolved_only.unwrap_or_default(),
            "since": request.since.as_deref(),
            "reviewer": reviewer,
        },
        "review_summary": review_summary,
        "identity_resolution_failures": identity_resolution_failures.len(),
        "review_votes": review_votes,
        "general_comments": general_comments,
        "review_requests": review_requests,
        "inline_threads": inline_threads,
        "preview_diffs": preview_diff_summaries,
    });
    Ok(format_discussion_result(
        OperationResult::new(markdown)
            .with_source_url(source_url)
            .with_details(details),
        format,
    ))
}

fn format_discussion_result(
    mut result: OperationResult,
    format: DiscussionFormat,
) -> OperationResult {
    if format == DiscussionFormat::Summary {
        return result;
    }
    let details = Value::Object(std::mem::take(&mut result.details));
    result.text = if format == DiscussionFormat::Structured {
        details.to_string()
    } else {
        format!(
            "{}\n\n## Structured data\n\n```json\n{details}\n```",
            result.text
        )
    };
    result.with_details(details)
}

struct ProposalSelection {
    repository: Value,
    proposal: Value,
    alternatives: Vec<Value>,
    related_repository: bool,
    reason: String,
    candidate_count: usize,
}

async fn branch_selector(request: &Request) -> Result<(String, String, bool, String)> {
    if let (Some(repository), Some(branch)) = (
        request
            .repository
            .as_deref()
            .filter(|repository| !repository.trim().is_empty()),
        request
            .branch
            .as_deref()
            .filter(|branch| !branch.trim().is_empty()),
    ) {
        return Ok((
            normalise_repository(repository)?,
            branch.trim().to_owned(),
            false,
            repository.to_owned(),
        ));
    }
    let (repository, branch) = local_git::current_repository_branch().await?;
    Ok((normalise_repository(&repository)?, branch, true, repository))
}

enum ProposalLookup {
    Found(ProposalSelection),
    Missing {
        repository: String,
        inspected_repositories: Vec<String>,
    },
}

async fn merge_proposal_for_branch(
    client: &LaunchpadClient,
    repository_path: &str,
    branch: &str,
    request: &Request,
) -> Result<ProposalLookup> {
    let repository = get_repository(client, repository_path)
        .await
        .map_err(|source| {
            Error::context(
                format!("cannot look up repository {repository_path} for branch {branch}"),
                source,
            )
        })?;
    let branch = normalise_ref(branch);
    let (proposals, requested_fallback_capped) =
        repository_branch_proposals(client, &repository, &branch).await?;
    let requested_candidate_count = proposals.len();
    let eligible = eligible_proposals(proposals, request);
    if !eligible.is_empty() {
        let (proposal, alternatives, reason) = select_proposal(eligible, request)?;
        return Ok(ProposalLookup::Found(ProposalSelection {
            repository,
            proposal,
            alternatives,
            related_repository: false,
            reason,
            candidate_count: requested_candidate_count,
        }));
    }

    let target_link = render::text_field(&repository, "target_link")
        .ok_or_else(|| Error::invalid("Launchpad repository has no target"))?;
    let repository_name = render::text_field(&repository, "name");
    let canonical_repository =
        render::text_field(&repository, "unique_name").unwrap_or(repository_path);
    let (related_repositories, truncated) =
        target_repositories(client, target_link, repository_name).await?;
    let mut inspected_repositories = Vec::new();
    for candidate in &related_repositories {
        if let Some(name) = render::text_field(candidate, "unique_name")
            && name != canonical_repository
            && !inspected_repositories
                .iter()
                .any(|inspected| inspected == name)
        {
            inspected_repositories.push(name.to_owned());
        }
    }
    let mut matches = stream::iter(related_repositories)
        .map(|related_repository| {
            let branch = branch.as_str();
            async move {
                if render::text_field(&related_repository, "unique_name")
                    == Some(canonical_repository)
                {
                    return Ok(None);
                }
                let proposals =
                    related_repository_branch_proposals(client, &related_repository, branch)
                        .await
                        .map_err(|error| {
                            Error::context(
                                format!(
                                    "cannot inspect related repository {} for {branch}",
                                    repository_identifier(&related_repository)
                                ),
                                error,
                            )
                        })?;
                let candidate_count = proposals.len();
                let proposals = eligible_proposals(proposals, request);
                Ok((!proposals.is_empty()).then_some((
                    related_repository,
                    proposals,
                    candidate_count,
                )))
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<Result<Option<(Value, Vec<Value>, usize)>>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    if matches.len() > 1 {
        let candidates = matches
            .iter()
            .map(|(repository, proposals, _)| {
                let repository =
                    render::text_field(repository, "unique_name").unwrap_or("unknown repository");
                let proposal_ids = proposals
                    .iter()
                    .filter_map(render::resource_id)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{repository}:{branch} (MP {proposal_ids})")
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::invalid(format!(
            "cannot select a merge proposal for {canonical_repository}:{branch}; matching repositories: {candidates}; retry with a full merge proposal target or narrow with status and target_branch"
        )));
    }
    let Some((repository, proposals, candidate_count)) = matches.pop() else {
        if requested_fallback_capped || truncated {
            let requested_cap = if requested_fallback_capped {
                format!(
                    "; requested repository search was limited to its newest {MAX_BRANCH_FALLBACK_ITEMS} proposals"
                )
            } else {
                String::new()
            };
            let related_cap = if truncated {
                format!(
                    "; related lookup was limited to the first {MAX_REPOSITORY_CANDIDATES} likely repositories"
                )
            } else {
                String::new()
            };
            let inspected = if inspected_repositories.is_empty() {
                "none".to_owned()
            } else {
                inspected_repositories.join(", ")
            };
            return Err(Error::invalid(format!(
                "cannot establish whether a merge proposal exists for {canonical_repository}:{branch}; inspected repositories: {canonical_repository}, {inspected}; filters: {}; retry with a full merge proposal target or narrow the search{requested_cap}{related_cap}",
                proposal_selection_filter_text(request)
            )));
        }
        return Ok(ProposalLookup::Missing {
            repository: canonical_repository.to_owned(),
            inspected_repositories,
        });
    };
    let (proposal, alternatives, reason) = select_proposal(proposals, request)?;
    Ok(ProposalLookup::Found(ProposalSelection {
        repository,
        proposal,
        alternatives,
        related_repository: true,
        reason,
        candidate_count: requested_candidate_count + candidate_count,
    }))
}

async fn repository_branch_proposals(
    client: &LaunchpadClient,
    repository: &Value,
    branch: &str,
) -> Result<(Vec<Value>, bool)> {
    let canonical_repository = render::text_field(repository, "unique_name").ok_or_else(|| {
        Error::invalid(format!(
            "cannot inspect repository {}; Launchpad omitted its unique name; accepted syntax: lp:<project>, lp://~owner/project/+git/repository, or a git.launchpad.net URL; retry with the canonical repository path",
            repository_identifier(repository)
        ))
    })?;
    let git_ref = list_git_refs(client, canonical_repository)
        .await?
        .into_iter()
        .find(|git_ref| git_ref.path.as_deref() == Some(branch));
    let ref_exists = git_ref.is_some();
    let collection_url = if let Some(self_link) = git_ref
        .as_ref()
        .and_then(|git_ref| git_ref.self_link.as_deref())
    {
        format!("{}/landing_targets", self_link.trim_end_matches('/'))
    } else {
        render::text_field(repository, "landing_targets_collection_link")
            .ok_or_else(|| Error::invalid("Launchpad repository has no merge proposal collection"))?
            .to_owned()
    };
    let mut entries = if ref_exists {
        fetch_all_entries(client, &collection_url).await?
    } else {
        fetch_entries(client, &collection_url, MAX_BRANCH_FALLBACK_ITEMS + 1).await?
    };
    let fallback_was_capped = !ref_exists && entries.len() > MAX_BRANCH_FALLBACK_ITEMS;
    if fallback_was_capped {
        entries.truncate(MAX_BRANCH_FALLBACK_ITEMS);
    }
    let proposals = entries
        .into_iter()
        .filter(|proposal| render::text_field(proposal, "source_git_path") == Some(branch))
        .collect();
    Ok((proposals, fallback_was_capped))
}

async fn related_repository_branch_proposals(
    client: &LaunchpadClient,
    repository: &Value,
    branch: &str,
) -> Result<Vec<Value>> {
    let repository_url = render::text_field(repository, "self_link")
        .ok_or_else(|| Error::invalid("Launchpad repository has no API link"))?;
    let mut ref_url = Url::parse(repository_url).map_err(|source| Error::Url {
        url: repository_url.to_owned(),
        source,
    })?;
    ref_url
        .query_pairs_mut()
        .append_pair("ws.op", "getRefByPath")
        .append_pair("path", branch);
    let git_ref: Value = match client.get_url(ref_url.as_str()).await {
        Ok(git_ref) => git_ref,
        Err(LpError::NotFound(_)) => return Ok(Vec::new()),
        Err(source) => return Err(source.into()),
    };
    if git_ref.is_null() {
        return Ok(Vec::new());
    }
    let ref_url = render::text_field(&git_ref, "self_link")
        .ok_or_else(|| Error::invalid("Launchpad Git reference has no API link"))?;
    let proposals = fetch_all_entries(
        client,
        &format!("{}/landing_targets", ref_url.trim_end_matches('/')),
    )
    .await?
    .into_iter()
    .filter(|proposal| render::text_field(proposal, "source_git_path") == Some(branch))
    .collect();
    Ok(proposals)
}

async fn target_repositories(
    client: &LaunchpadClient,
    target_link: &str,
    repository_name: Option<&str>,
) -> Result<(Vec<Value>, bool)> {
    let mut url = Url::parse(&client.url("/+git")).map_err(|source| Error::Url {
        url: client.url("/+git"),
        source,
    })?;
    url.query_pairs_mut()
        .append_pair("ws.op", "getRepositories")
        .append_pair("target", target_link)
        .append_pair("order_by", "most recently changed first")
        .append_pair("ws.size", &(MAX_REPOSITORY_CANDIDATES + 1).to_string());
    let mut repositories =
        fetch_entries(client, url.as_str(), MAX_REPOSITORY_CANDIDATES + 1).await?;
    let truncated = repositories.len() > MAX_REPOSITORY_CANDIDATES;
    repositories.truncate(MAX_REPOSITORY_CANDIDATES);
    repositories.retain(|repository| {
        render::text_field(repository, "name") == repository_name
            || repository
                .get("target_default")
                .and_then(Value::as_bool)
                .unwrap_or_default()
    });
    Ok((repositories, truncated))
}

fn eligible_proposals(proposals: Vec<Value>, request: &Request) -> Vec<Value> {
    let requested_statuses = request.status.as_ref();
    let explicitly_requests_superseded = requested_statuses.is_some_and(|statuses| {
        statuses
            .values()
            .any(|status| status.eq_ignore_ascii_case("Superseded"))
    });
    let include_superseded =
        request.include_superseded.unwrap_or_default() || explicitly_requests_superseded;
    let target_branch = request.target_branch.as_deref().map(normalise_ref);
    proposals
        .into_iter()
        .filter(|proposal| {
            let status = render::text_field(proposal, "queue_status").unwrap_or_default();
            let status_matches = requested_statuses.is_none_or(|statuses| {
                statuses
                    .values()
                    .any(|requested| requested.eq_ignore_ascii_case(status))
            });
            let target_matches = target_branch.as_deref().is_none_or(|target| {
                render::text_field(proposal, "target_git_path") == Some(target)
            });
            status_matches
                && target_matches
                && (include_superseded || !status.eq_ignore_ascii_case("Superseded"))
        })
        .collect()
}

fn select_proposal(
    mut proposals: Vec<Value>,
    request: &Request,
) -> Result<(Value, Vec<Value>, String)> {
    let latest = request.latest.unwrap_or_default();
    proposals.sort_by(|left, right| {
        if latest {
            proposal_sort_key(left).cmp(&proposal_sort_key(right))
        } else {
            proposal_selection_key(left).cmp(&proposal_selection_key(right))
        }
    });
    let proposal = proposals
        .pop()
        .ok_or_else(|| Error::invalid("cannot select a merge proposal from an empty result"))?;
    let status = render::text_field(&proposal, "queue_status").unwrap_or("unknown");
    let reason = if latest {
        "selected the latest matching proposal by creation date and ID".to_owned()
    } else if is_active_proposal_status(status) {
        "preferred an active proposal, then selected the newest by creation date and ID".to_owned()
    } else if status.eq_ignore_ascii_case("Merged") {
        "selected the newest merged proposal because no active proposal matched".to_owned()
    } else {
        format!(
            "selected the newest {status} proposal because no active or merged proposal matched"
        )
    };
    Ok((proposal, proposals, reason))
}

fn proposal_selection_key(proposal: &Value) -> (u8, &str, u64) {
    let status = render::text_field(proposal, "queue_status").unwrap_or_default();
    let priority = if is_active_proposal_status(status) {
        3
    } else if status.eq_ignore_ascii_case("Merged") {
        2
    } else if status.eq_ignore_ascii_case("Rejected") {
        1
    } else {
        0
    };
    let (date, id) = proposal_sort_key(proposal);
    (priority, date, id)
}

fn is_active_proposal_status(status: &str) -> bool {
    !["Merged", "Rejected", "Superseded"]
        .iter()
        .any(|terminal| terminal.eq_ignore_ascii_case(status))
}

fn proposal_sort_key(proposal: &Value) -> (&str, u64) {
    let date = render::text_field(proposal, "date_created").unwrap_or_default();
    let id = render::resource_id(proposal)
        .and_then(|id| id.parse().ok())
        .unwrap_or_default();
    (date, id)
}

fn proposal_selection_filters(request: &Request) -> Value {
    json!({
        "status": request.status.as_ref().map(|statuses| statuses.values().collect::<Vec<_>>()),
        "target_branch": request.target_branch.as_deref().map(normalise_ref),
        "latest": request.latest.unwrap_or_default(),
        "include_superseded": request.include_superseded.unwrap_or_default(),
    })
}

fn proposal_selection_filter_text(request: &Request) -> String {
    let filters = proposal_selection_filters(request);
    serde_json::to_string(&filters).unwrap_or_else(|_| "{}".to_owned())
}

fn proposal_summary(proposal: &Value) -> Value {
    json!({
        "id": render::resource_id(proposal),
        "status": render::text_field(proposal, "queue_status"),
        "url": render::text_field(proposal, "web_link"),
    })
}

fn build_review_summary(
    general_comments: &[Value],
    review_votes: &[Value],
    review_requests: &[Value],
    inline_threads: &[Value],
) -> Value {
    let mut vote_counts = BTreeMap::<String, usize>::new();
    let mut previous_votes = BTreeMap::<String, String>::new();
    let mut current_votes = BTreeMap::<String, Value>::new();
    let mut transitions = Vec::new();
    let mut ordered_votes = review_votes.iter().collect::<Vec<_>>();
    ordered_votes.sort_by_key(|vote| render::text_field(vote, "created_at"));
    for vote in ordered_votes {
        let Some(value) = render::text_field(vote, "vote") else {
            continue;
        };
        let reviewer = vote
            .get("author")
            .and_then(|author| render::text_field(author, "username"))
            .unwrap_or("unknown")
            .to_owned();
        *vote_counts.entry(value.to_owned()).or_default() += 1;
        let previous = previous_votes.insert(reviewer.clone(), value.to_owned());
        if previous
            .as_deref()
            .is_some_and(|previous| previous != value)
        {
            transitions.push(json!({
                "reviewer": reviewer.clone(),
                "from": previous,
                "to": value,
                "at": render::text_field(vote, "created_at"),
            }));
        }
        current_votes.insert(
            reviewer.clone(),
            json!({
                "reviewer": reviewer,
                "vote": value,
                "at": render::text_field(vote, "created_at"),
            }),
        );
    }
    let state_count = |state: &str| {
        inline_threads
            .iter()
            .filter(|thread| render::text_field(thread, "state") == Some(state))
            .count()
    };
    json!({
        "general_comment_count": general_comments.len(),
        "inline_thread_count": inline_threads.len(),
        "current_open_inline_thread_count": state_count("open"),
        "outdated_inline_thread_count": state_count("outdated"),
        "superseded_inline_thread_count": state_count("superseded"),
        "resolved_inline_thread_count": Value::Null,
        "resolution_tracking": "Launchpad does not expose resolved inline-thread state",
        "review_request_count": review_requests.len(),
        "pending_review_request_count": review_requests.iter().filter(|request| {
            request.get("pending").and_then(Value::as_bool).unwrap_or_default()
        }).count(),
        "review_vote_count": review_votes.len(),
        "review_vote_counts": vote_counts,
        "current_review_votes": current_votes.into_values().collect::<Vec<_>>(),
        "review_vote_transitions": transitions,
    })
}

fn repository_identifier(repository: &Value) -> &str {
    render::text_field(repository, "web_link").unwrap_or("repository")
}

async fn load_people<'entry>(
    client: &LaunchpadClient,
    entries: impl Iterator<Item = &'entry Value>,
    people: &mut HashMap<String, Value>,
    failures: &mut HashSet<String>,
) {
    let mut links = HashSet::new();
    for entry in entries {
        for field in ["author", "owner", "person", "reviewer", "registrant"] {
            if let Some(link) = person_link(entry, field)
                && !people.contains_key(link)
                && !failures.contains(link)
            {
                links.insert(link.to_owned());
            }
        }
    }
    let loaded = stream::iter(links)
        .map(|link| async move {
            let person = client.get_url(&link).await.ok();
            (link, person)
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    for (link, person) in loaded {
        if let Some(person) = person {
            people.insert(link, person);
        } else {
            failures.insert(link);
        }
    }
}

fn normalise_general_comment(comment: &Value, people: &HashMap<String, Value>) -> Value {
    let body = render::text_field(comment, "message_body")
        .or_else(|| render::text_field(comment, "content"))
        .filter(|body| !body.trim().is_empty());
    json!({
        "id": comment.get("id"),
        "author": normalise_identity(comment, "author", people)
            .or_else(|| normalise_identity(comment, "owner", people)),
        "created_at": render::text_field(comment, "date_created"),
        "vote": render::text_field(comment, "vote"),
        "title": render::text_field(comment, "title"),
        "body": body,
        "url": render::text_field(comment, "web_link"),
    })
}

fn normalise_inline_comment(
    comment: &Value,
    index: usize,
    people: &HashMap<String, Value>,
) -> Value {
    let body = render::text_field(comment, "text").filter(|body| !body.trim().is_empty());
    json!({
        "sequence": index + 1,
        "author": normalise_identity(comment, "person", people),
        "created_at": render::text_field(comment, "date"),
        "body": body,
    })
}

fn normalise_review_assignment(vote: &Value, people: &HashMap<String, Value>) -> Value {
    json!({
        "reviewer": normalise_identity(vote, "reviewer", people),
        "registrant": normalise_identity(vote, "registrant", people),
        "created_at": render::text_field(vote, "date_created"),
        "review_type": render::text_field(vote, "review_type"),
        "pending": vote.get("is_pending").and_then(Value::as_bool),
        "comment_url": render::text_field(vote, "comment_link"),
        "url": render::text_field(vote, "web_link"),
    })
}

fn normalise_identity(
    value: &Value,
    field: &str,
    people: &HashMap<String, Value>,
) -> Option<Value> {
    let link = person_link(value, field);
    let person = value
        .get(field)
        .filter(|person| person.is_object())
        .or_else(|| link.and_then(|link| people.get(link)));
    if person.is_none() && link.is_none() {
        return None;
    }
    let username = person
        .and_then(|person| render::text_field(person, "name"))
        .or_else(|| {
            link.and_then(|link| {
                link.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .map(|name| name.trim_start_matches('~'))
            })
        });
    let display_name = person
        .and_then(|person| render::text_field(person, "display_name"))
        .or(username);
    let url = person
        .and_then(|person| render::text_field(person, "web_link"))
        .map(str::to_owned)
        .or_else(|| username.map(|username| format!("https://launchpad.net/~{username}")));
    Some(json!({
        "username": username,
        "display_name": display_name,
        "url": url,
    }))
}

fn person_link<'value>(value: &'value Value, field: &str) -> Option<&'value str> {
    let link_field = match field {
        "author" => Some("author_link"),
        "owner" => Some("owner_link"),
        "person" => Some("person_link"),
        "reviewer" => Some("reviewer_link"),
        "registrant" => Some("registrant_link"),
        _ => None,
    };
    value
        .get(field)
        .and_then(|person| render::text_field(person, "self_link"))
        .or_else(|| link_field.and_then(|link_field| render::text_field(value, link_field)))
        .or_else(|| value.get(field).and_then(Value::as_str))
}

fn matches_discussion_filters(
    value: &Value,
    identity_field: &str,
    since: Option<&DateTime<FixedOffset>>,
    reviewer: Option<&str>,
) -> bool {
    let after_since = since.is_none_or(|since| {
        render::text_field(value, "created_at")
            .and_then(|created| DateTime::parse_from_rfc3339(created).ok())
            .is_none_or(|created| created >= *since)
    });
    let matches_reviewer = reviewer.is_none_or(|reviewer| {
        let reviewer = reviewer.trim_start_matches('~');
        value.get(identity_field).is_some_and(|identity| {
            ["username", "display_name"]
                .into_iter()
                .filter_map(|field| render::text_field(identity, field))
                .any(|candidate| candidate.eq_ignore_ascii_case(reviewer))
        })
    });
    after_since && matches_reviewer
}

fn thread_state(current: bool, stale: bool) -> &'static str {
    if stale {
        "outdated"
    } else if current {
        "open"
    } else {
        "superseded"
    }
}

async fn create_bug(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = request.target()?;
    let title = request.string(&request.title, "title")?;
    let description = request.string(&request.description, "description")?;
    let target_url = client.url(&format!("/{target}"));
    let mut parameters = vec![
        ("ws.op", "createBug"),
        ("title", title),
        ("description", description),
        ("target", target_url.as_str()),
    ];
    if let Some(information_type) = request.information_type.as_deref() {
        parameters.push(("information_type", information_type));
    }
    if let Some(tags) = &request.tags {
        parameters.extend(tags.iter().map(|tag| ("tags", tag.as_str())));
    }
    let location = client
        .post_pairs_created_location("/bugs", &parameters)
        .await?;
    let bug: Value = client.get_url(&location).await?;
    let id = bug
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid("Launchpad returned a bug without an ID"))?;
    let tasks = fetch_entries(
        client,
        &client.url(&format!("/bugs/{id}/bug_tasks")),
        MAX_ITEMS,
    )
    .await?;
    let text = format!(
        "# Created Launchpad bug\n\n{}",
        render::render_bug(&bug, &tasks, &[], false, 1)
    );
    let source_url = render::text_field(&bug, "web_link").map(str::to_owned);
    let details = json!({ "kind": "bug", "id": id });
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn edit_bug(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = ResourceTarget::parse(request.target()?)?;
    if !matches!(target.kind, ResourceKind::Bug { .. }) {
        return Err(Error::invalid("bug_edit requires a bug URL"));
    }
    let url = client.url(&format!("/{}", target.path));
    let mut changes = serde_json::Map::new();
    if let Some(title) = &request.title {
        changes.insert("title".to_owned(), json!(title));
    }
    if let Some(description) = &request.description {
        changes.insert("description".to_owned(), json!(description));
    }
    if let Some(tags) = &request.tags {
        changes.insert("tags".to_owned(), json!(tags));
    }
    let updated: Value = client
        .patch_url_with_value(&url, &Value::Object(changes))
        .await?;
    let source_url = render::text_field(&updated, "web_link").map(str::to_owned);
    let text = format!(
        "# Updated Launchpad bug\n\n{}",
        render::render_bug(&updated, &[], &[], false, 1)
    );
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(json!({ "kind": "bug", "id": updated.get("id") })))
}

async fn edit_bug_task(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = ResourceTarget::parse(request.target()?)?;
    let url = client.url(&format!("/{}", target.path));
    let task: Value = client.get_url(&url).await?;
    if resource_kind(&task) != "bug_task" {
        return Err(Error::invalid("bug_task_edit requires a bug task URL"));
    }
    let mut changes = serde_json::Map::new();
    if let Some(status) = &request.status {
        let OneOrMany::One(status) = status else {
            return Err(Error::invalid("status must be a string"));
        };
        changes.insert("status".to_owned(), json!(status));
    }
    if let Some(importance) = &request.importance {
        let OneOrMany::One(importance) = importance else {
            return Err(Error::invalid("importance must be a string"));
        };
        changes.insert("importance".to_owned(), json!(importance));
    }
    if let Some(assignee) = &request.assignee {
        changes.insert(
            "assignee_link".to_owned(),
            json!(client.url(&format!("/~{assignee}"))),
        );
    }
    if request.unassign == Some(true) {
        changes.insert("assignee_link".to_owned(), Value::Null);
    }
    let updated: Value = client
        .patch_url_with_value(&url, &Value::Object(changes))
        .await?;
    let source_url = render::text_field(&updated, "web_link").map(str::to_owned);
    let text = format!(
        "# Updated Launchpad bug task\n\n{}",
        render::render_generic(&updated)
    );
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(json!({ "kind": "bug_task", "target": target.path })))
}

async fn edit_project(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = ResourceTarget::parse(request.target()?)?;
    let url = client.url(&format!("/{}", target.path));
    let project: Value = client.get_url(&url).await?;
    if resource_kind(&project) != "project" {
        return Err(Error::invalid("project_edit requires a project URL"));
    }
    let mut changes = serde_json::Map::new();
    if let Some(summary) = &request.summary {
        changes.insert("summary".to_owned(), json!(summary));
    }
    if let Some(description) = &request.description {
        changes.insert("description".to_owned(), json!(description));
    }
    if let Some(guidelines) = &request.bug_reporting_guidelines {
        changes.insert("bug_reporting_guidelines".to_owned(), json!(guidelines));
    }
    if let Some(tags) = &request.official_bug_tags {
        changes.insert("official_bug_tags".to_owned(), json!(tags));
    }
    let updated: Value = client
        .patch_url_with_value(&url, &Value::Object(changes))
        .await?;
    let source_url = render::text_field(&updated, "web_link").map(str::to_owned);
    let text = format!(
        "# Updated Launchpad project\n\n{}",
        render::render_generic(&updated)
    );
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(json!({ "kind": "project", "target": target.path })))
}

async fn edit_repository(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = ResourceTarget::parse(request.target()?)?;
    if !matches!(target.kind, ResourceKind::Repository) {
        return Err(Error::invalid(
            "repository_edit requires a Git repository URL",
        ));
    }
    let repository = get_repository(client, &target.path).await?;
    let url = render::text_field(&repository, "self_link")
        .ok_or_else(|| Error::invalid("repository has no API link"))?;
    let mut changes = serde_json::Map::new();
    if let Some(description) = &request.description {
        changes.insert("description".to_owned(), json!(description));
    }
    if let Some(branch) = &request.default_branch {
        changes.insert("default_branch".to_owned(), json!(branch));
    }
    let updated: Value = client
        .patch_url_with_value(url, &Value::Object(changes))
        .await?;
    let source_url = render::text_field(&updated, "web_link").map(str::to_owned);
    let text = format!(
        "# Updated Launchpad Git repository\n\n{}",
        render::render_repository(&updated)
    );
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(json!({ "kind": "git_repository", "target": target.path })))
}

struct PreparedMergeProposal {
    source_link: String,
    target_link: String,
    prerequisite_link: Option<String>,
    source_repository_link: String,
    target_repository_link: String,
    prerequisite_repository_link: Option<String>,
    source_ref: String,
    target_ref: String,
    prerequisite_ref: Option<String>,
    source_commit_sha: Option<String>,
}

async fn prepare_merge_proposal(
    client: &LaunchpadClient,
    source_repository: &str,
    source_ref: &str,
    target_repository: &str,
    target_ref: &str,
    prerequisite: Option<(&str, &str)>,
    deadline: Option<Instant>,
) -> Result<PreparedMergeProposal> {
    let (source, source_repository) =
        find_git_ref(client, "source", source_repository, source_ref, deadline).await?;
    let (target, target_repository) =
        find_git_ref(client, "target", target_repository, target_ref, deadline).await?;
    let source_link = source.self_link.ok_or_else(|| {
        Error::invalid(format!(
            "source ref {source_ref} in repository {source_repository} has no API link"
        ))
    })?;
    let target_link = target.self_link.ok_or_else(|| {
        Error::invalid(format!(
            "target ref {target_ref} in repository {target_repository} has no API link"
        ))
    })?;
    let prerequisite = if let Some((repository, path)) = prerequisite {
        let (git_ref, repository) =
            find_git_ref(client, "prerequisite", repository, path, deadline).await?;
        let link = git_ref.self_link.ok_or_else(|| {
            Error::invalid(format!(
                "prerequisite ref {path} in repository {repository} has no API link"
            ))
        })?;
        Some((link, repository, path.to_owned()))
    } else {
        None
    };
    Ok(PreparedMergeProposal {
        source_link,
        target_link,
        prerequisite_link: prerequisite.as_ref().map(|(link, _, _)| link.clone()),
        source_repository_link: client.url(&format!("/{source_repository}")),
        target_repository_link: client.url(&format!("/{target_repository}")),
        prerequisite_repository_link: prerequisite
            .as_ref()
            .map(|(_, repository, _)| client.url(&format!("/{repository}"))),
        source_ref: source_ref.to_owned(),
        target_ref: target_ref.to_owned(),
        prerequisite_ref: prerequisite.map(|(_, _, path)| path),
        source_commit_sha: source.commit_sha1,
    })
}

async fn submit_merge_proposal(
    client: &LaunchpadClient,
    prepared: &PreparedMergeProposal,
    description: Option<&str>,
    commit_message: Option<&str>,
    needs_review: bool,
) -> Result<String> {
    let needs_review = needs_review.to_string();
    let mut parameters = vec![
        ("ws.op", "createMergeProposal"),
        ("merge_target", prepared.target_link.as_str()),
        ("initial_comment", description.unwrap_or_default()),
        ("needs_review", needs_review.as_str()),
    ];
    if let Some(link) = prepared.prerequisite_link.as_deref() {
        parameters.push(("merge_prerequisite", link));
    }
    if let Some(commit_message) = commit_message {
        parameters.push(("commit_message", commit_message));
    }
    client
        .post_pairs_url_created_location(&prepared.source_link, &parameters)
        .await
        .map_err(|source| {
            Error::context(
                format!(
                    "cannot create merge proposal from {} to {}",
                    prepared.source_link, prepared.target_link
                ),
                source.into(),
            )
        })
}

async fn create_merge_proposal(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let source_repository = normalise_repository(request.repository()?)?;
    let target_repository = request
        .target_repository
        .as_deref()
        .map(normalise_repository)
        .transpose()?
        .unwrap_or_else(|| source_repository.clone());
    let source_ref = normalise_ref(request.string(&request.source_ref, "source_ref")?);
    let target_ref = normalise_ref(request.string(&request.target_ref, "target_ref")?);
    let prerequisite_ref = request.prerequisite_ref.as_deref().map(normalise_ref);
    let prerequisite_repository = request
        .prerequisite_repository
        .as_deref()
        .map(normalise_repository)
        .transpose()?;
    let commit_message = request.string(&request.commit_message, "commit_message")?;
    let index_deadline = request.wait_for_index.unwrap_or(false).then(|| {
        Instant::now() + Duration::from_secs(request.index_timeout_seconds.unwrap_or(180))
    });
    let prepared = prepare_merge_proposal(
        client,
        &source_repository,
        &source_ref,
        &target_repository,
        &target_ref,
        prerequisite_ref.as_deref().map(|path| {
            (
                prerequisite_repository
                    .as_deref()
                    .unwrap_or(&source_repository),
                path,
            )
        }),
        index_deadline,
    )
    .await?;
    let existing = matching_proposals(client, &prepared, request, commit_message).await?;
    let mut recent = existing.iter().filter(|proposal| {
        render::text_field(proposal, "date_created")
            .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
            .is_some_and(|date| {
                let age = chrono::Utc::now().signed_duration_since(date).num_seconds();
                (-60..600).contains(&age)
            })
    });
    if let Some(proposal) = recent.next() {
        if recent.next().is_some() {
            return Err(Error::invalid(
                "multiple recent proposals match this creation request; inspect their IDs before retrying",
            ));
        }
        return creation_result(client, proposal.clone(), &prepared, request, "recovered").await;
    }
    let known_ids: HashSet<_> = existing.iter().filter_map(render::resource_id).collect();
    let location = match submit_merge_proposal(
        client,
        &prepared,
        request.description.as_deref(),
        Some(commit_message),
        request.needs_review.unwrap_or(true),
    )
    .await
    {
        Ok(location) => location,
        Err(error) if ambiguous_creation_error(&error) => {
            let matches = matching_proposals(client, &prepared, request, commit_message).await
                .map_err(|lookup| Error::invalid(format!(
                    "creation outcome unknown ({error}); cannot inspect proposals to recover it: {lookup}; do not blindly retry"
                )))?;
            let mut created = matches.into_iter().filter(|proposal| {
                render::resource_id(proposal).is_some_and(|id| !known_ids.contains(&id))
            });
            let proposal = created.next().ok_or_else(|| Error::invalid(format!(
                "creation outcome unknown ({error}); no matching new proposal is visible yet; retry after Launchpad indexing, not by blindly submitting again"
            )))?;
            if created.next().is_some() {
                return Err(Error::invalid(
                    "creation outcome unknown; multiple matching new proposals found; inspect their IDs before retrying",
                ));
            }
            return creation_result(client, proposal, &prepared, request, "recovered").await;
        }
        Err(error) => return Err(error),
    };
    let proposal: Value = client.get_url(&location).await.map_err(|source| {
        Error::context(
            format!(
                "proposal created at {location} but cannot load it; do not submit again blindly"
            ),
            source.into(),
        )
    })?;
    creation_result(client, proposal, &prepared, request, "new").await
}

async fn creation_result(
    client: &LaunchpadClient,
    mut proposal: Value,
    prepared: &PreparedMergeProposal,
    request: &Request,
    creation_state: &str,
) -> Result<OperationResult> {
    let url = proposal_api_url(&proposal)?;
    if request.wait_for_preview.unwrap_or(false) {
        let deadline =
            Instant::now() + Duration::from_secs(request.preview_timeout_seconds.unwrap_or(180));
        let mut delay = Duration::from_secs(2);
        while render::text_field(&proposal, "preview_diff_link").is_none()
            && Instant::now() < deadline
        {
            sleep(delay.min(deadline.saturating_duration_since(Instant::now()))).await;
            proposal = client.get_url(url.as_str()).await.map_err(|source| Error::context(
                format!("proposal exists at {url}, but preview readiness could not be checked; do not create it again"),
                source.into(),
            ))?;
            delay = (delay * 2).min(Duration::from_secs(16));
        }
    }
    confirm_proposal(&proposal, prepared)?;
    let preview_state = if render::text_field(&proposal, "preview_diff_link").is_some() {
        "ready"
    } else {
        "pending"
    };
    let mut details = proposal_details(client, &proposal).await.map_err(|source| Error::context(
        format!("proposal exists at {url}, but its repository metadata could not be loaded; do not create it again"),
        source,
    ))?;
    details["source_commit_sha"] = json!(prepared.source_commit_sha);
    details["preview_state"] = json!(preview_state);
    details["creation_state"] = json!(creation_state);
    let text = format!(
        "# {} Launchpad merge proposal\n\n{}\n\n- **Source commit:** {}\n- **Prerequisite repository:** {}\n- **Preview:** {preview_state}",
        if creation_state == "new" {
            "Created"
        } else {
            "Recovered"
        },
        render::render_proposal(&proposal, &[], &[], false, 1),
        prepared
            .source_commit_sha
            .as_deref()
            .unwrap_or("unavailable"),
        details["prerequisite_repository"]
            .as_str()
            .unwrap_or("none"),
    );
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

fn confirm_proposal(proposal: &Value, prepared: &PreparedMergeProposal) -> Result<()> {
    for (field, expected) in [
        ("source_git_path", Some(prepared.source_ref.as_str())),
        ("target_git_path", Some(prepared.target_ref.as_str())),
        (
            "prerequisite_git_path",
            prepared.prerequisite_ref.as_deref(),
        ),
        (
            "source_git_repository_link",
            Some(prepared.source_repository_link.as_str()),
        ),
        (
            "target_git_repository_link",
            Some(prepared.target_repository_link.as_str()),
        ),
        (
            "prerequisite_git_repository_link",
            prepared.prerequisite_repository_link.as_deref(),
        ),
    ] {
        if render::text_field(proposal, field) != expected {
            return Err(Error::invalid(format!(
                "created proposal {} has {field} {:?}, expected {expected:?}; inspect the proposal before retrying",
                render::resource_id(proposal).unwrap_or_else(|| "?".to_owned()),
                render::text_field(proposal, field),
            )));
        }
    }
    if let (Some(actual), Some(expected)) = (
        render::text_field(proposal, "source_git_commit_sha1"),
        prepared.source_commit_sha.as_deref(),
    ) && actual != expected
    {
        return Err(Error::invalid(format!(
            "created proposal source commit {actual} differs from resolved source commit {expected}; inspect the proposal before retrying"
        )));
    }
    Ok(())
}

async fn matching_proposals(
    client: &LaunchpadClient,
    prepared: &PreparedMergeProposal,
    request: &Request,
    commit_message: &str,
) -> Result<Vec<Value>> {
    let collection = format!(
        "{}/landing_targets",
        prepared.source_link.trim_end_matches('/')
    );
    Ok(fetch_all_entries(client, &collection)
        .await?
        .into_iter()
        .filter(|proposal| {
            confirm_proposal(proposal, prepared).is_ok()
                && render::text_field(proposal, "commit_message") == Some(commit_message)
                && render::text_field(proposal, "description").unwrap_or_default()
                    == request.description.as_deref().unwrap_or_default()
        })
        .collect())
}

fn ambiguous_creation_error(error: &Error) -> bool {
    matches!(error, Error::Context { source, .. } if matches!(
        source.as_ref(),
        Error::Launchpad { source: LpError::Timeout(_) | LpError::Connect(_) }
    ) || matches!(source.as_ref(), Error::Launchpad {
        source: LpError::Http(request)
    } if request.is_timeout() || request.is_connect()))
}

async fn edit_merge_proposal(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let url = proposal_api_url(&proposal)?;
    let mut changes = serde_json::Map::new();
    if let Some(message) = request.commit_message.as_deref() {
        changes.insert("commit_message".to_owned(), json!(message));
    }
    if let Some(description) = request.description.as_deref() {
        changes.insert("description".to_owned(), json!(description));
    }
    if let Some(revid) = request.reviewed_revid.as_deref() {
        changes.insert("reviewed_revid".to_owned(), json!(revid));
    }
    let updated: Value = client
        .patch_url_with_value(url.as_str(), &Value::Object(changes))
        .await
        .map_err(|source| {
            Error::context(format!("cannot edit merge proposal {url}"), source.into())
        })?;
    let source_url = render::text_field(&updated, "web_link").map(str::to_owned);
    let text = format!(
        "# Updated Launchpad merge proposal\n\n{}",
        render::render_proposal(&updated, &[], &[], false, 1)
    );
    let details = proposal_details(client, &updated).await?;
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(details))
}

async fn replace_merge_proposal_prerequisite(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, previous) = request_merge_proposal(client, request).await?;
    let previous_status = render::text_field(&previous, "queue_status")
        .ok_or_else(|| Error::invalid("merge proposal has no status"))?;
    if !matches!(
        previous_status,
        "Work in progress" | "Needs review" | "Approved" | "Rejected"
    ) {
        return Err(Error::invalid(format!(
            "cannot replace a merge proposal in status {previous_status}"
        )));
    }
    let prerequisite = request.string(&request.merge_prerequisite, "merge_prerequisite")?;
    if render::text_field(&previous, "prerequisite_git_path")
        == Some(normalise_ref(prerequisite).as_str())
    {
        return Err(Error::invalid(
            "merge proposal already has this prerequisite",
        ));
    }
    let source_repository =
        linked_resource(client, &previous, "source_git_repository_link").await?;
    let target_repository =
        linked_resource(client, &previous, "target_git_repository_link").await?;
    let source_repository = render::text_field(&source_repository, "unique_name")
        .ok_or_else(|| Error::invalid("merge proposal has no source repository name"))?;
    let target_repository = render::text_field(&target_repository, "unique_name")
        .ok_or_else(|| Error::invalid("merge proposal has no target repository name"))?;
    let source_ref = render::text_field(&previous, "source_git_path")
        .ok_or_else(|| Error::invalid("merge proposal has no source ref"))?;
    let target_ref = render::text_field(&previous, "target_git_path")
        .ok_or_else(|| Error::invalid("merge proposal has no target ref"))?;
    let prerequisite_ref = normalise_ref(prerequisite);
    let previous_url = proposal_api_url(&previous)?;
    let prepared = prepare_merge_proposal(
        client,
        source_repository,
        source_ref,
        target_repository,
        target_ref,
        Some((source_repository, &prerequisite_ref)),
        None,
    )
    .await?;
    client
        .post_pairs_url_ok(
            previous_url.as_str(),
            &[("ws.op", "setStatus"), ("status", "Superseded")],
        )
        .await?;
    let location = match submit_merge_proposal(
        client,
        &prepared,
        render::text_field(&previous, "description"),
        render::text_field(&previous, "commit_message"),
        previous_status != "Work in progress",
    )
    .await
    {
        Ok(location) => location,
        Err(source) => {
            let rollback = client
                .post_pairs_url_ok(
                    previous_url.as_str(),
                    &[("ws.op", "setStatus"), ("status", previous_status)],
                )
                .await;
            return Err(match rollback {
                Ok(()) => Error::context(
                    "cannot create replacement; previous status restored",
                    source,
                ),
                Err(rollback_error) => Error::invalid(format!(
                    "cannot create replacement: {source}; previous proposal {previous_url} remains Superseded because rollback failed: {rollback_error}"
                )),
            });
        }
    };
    let replacement: Value = client.get_url(&location).await.map_err(|source| {
        Error::context(
            format!("replacement created at {location}, but cannot load it; previous proposal {previous_url} is Superseded"),
            source.into(),
        )
    })?;
    let replacement_url = render::text_field(&replacement, "web_link").unwrap_or(&location);
    let text = format!(
        "# Replaced Launchpad merge proposal\n\n- **Previous:** {previous_url} (Superseded)\n- **Replacement:** {replacement_url}\n\n{}",
        render::render_proposal(&replacement, &[], &[], false, 1)
    );
    let mut details = proposal_details(client, &replacement).await?;
    details["supersedes"] = json!(previous_url.as_str());
    Ok(OperationResult::new(text)
        .with_source_url(Some(replacement_url.to_owned()))
        .with_details(details))
}

async fn change_merge_proposal_bug_link(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let url = proposal_api_url(&proposal)?;
    let bug_id = request
        .bug_id
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::invalid("bug_id must be a positive bug ID"))?;
    let bug_link = client.url(&format!("/bugs/{bug_id}"));
    let (operation, verb) = match request.op {
        Operation::MergeProposalLinkBug => ("linkBug", "linked to"),
        Operation::MergeProposalUnlinkBug => ("unlinkBug", "unlinked from"),
        _ => return Err(Error::invalid("invalid merge proposal bug link operation")),
    };
    client
        .post_pairs_url_ok(url.as_str(), &[("ws.op", operation), ("bug", &bug_link)])
        .await?;
    let source_url = render::text_field(&proposal, "web_link").map(str::to_owned);
    let id = render::resource_id(&proposal).unwrap_or_else(|| "?".to_owned());
    Ok(
        OperationResult::new(format!("Bug #{bug_id} {verb} merge proposal {id}"))
            .with_source_url(source_url)
            .with_details(
                json!({ "kind": "branch_merge_proposal", "proposal_id": id, "bug_id": bug_id }),
            ),
    )
}

async fn add_comment(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = ResourceTarget::parse(request.target()?)?;
    let body = request.string(&request.body, "body")?;
    let (kind, source_url) = match &target.kind {
        ResourceKind::Bug { id } => {
            let url = client.url(&format!("/bugs/{id}"));
            let mut parameters = vec![("ws.op", "newMessage"), ("content", body)];
            if let Some(subject) = request.subject.as_deref() {
                parameters.push(("subject", subject));
            }
            client.post_pairs_url_ok(&url, &parameters).await?;
            ("bug", launchpad_web_url(&target.path, "bug"))
        }
        ResourceKind::MergeProposal { .. } | ResourceKind::MergeProposalId { .. } => {
            let (_, proposal) = resolve_merge_proposal_target(client, request.target()?).await?;
            let url = proposal_api_url(&proposal)?;
            let mut parameters = vec![("ws.op", "createComment"), ("content", body)];
            if let Some(subject) = request.subject.as_deref() {
                parameters.push(("subject", subject));
            }
            if let Some(vote) = request.vote.as_deref() {
                parameters.push(("vote", vote));
            }
            client.post_pairs_url_ok(url.as_str(), &parameters).await?;
            (
                "branch_merge_proposal",
                render::text_field(&proposal, "web_link")
                    .unwrap_or(request.target()?)
                    .to_owned(),
            )
        }
        ResourceKind::Repository | ResourceKind::Generic => {
            return Err(Error::invalid(
                "comments are supported only for bugs and merge proposals",
            ));
        }
    };
    let text = format!("Comment added to {source_url}");
    let details = json!({ "kind": kind });
    Ok(OperationResult::new(text)
        .with_source_url(Some(source_url))
        .with_details(details))
}

async fn edit_comment(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let target = ResourceTarget::parse(request.target()?)?;
    let url = client.url(&format!("/{}", target.path));
    let comment: Value = client.get_url(&url).await?;
    if !matches!(
        resource_kind(&comment).as_str(),
        "code_review_comment" | "message"
    ) {
        return Err(Error::invalid(
            "comment_edit requires an individual bug or merge-proposal comment URL",
        ));
    }
    let comment_url = render::text_field(&comment, "self_link")
        .ok_or_else(|| Error::invalid("comment has no API link"))?;
    let body = request.string(&request.body, "body")?;
    client
        .post_pairs_url_ok(
            comment_url,
            &[("ws.op", "editContent"), ("new_content", body)],
        )
        .await?;
    let updated: Value = client.get_url(comment_url).await?;
    let source_url = render::text_field(&updated, "web_link").map(str::to_owned);
    let text = format!(
        "# Updated Launchpad comment\n\n{}",
        render::render_generic(&updated)
    );
    Ok(OperationResult::new(text)
        .with_source_url(source_url)
        .with_details(json!({ "kind": resource_kind(&updated), "target": target.path })))
}

async fn update_review_draft(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let preview_diff_id = request.preview_diff_id()?;
    let file_line = request.file_line()?;
    let side = request.side()?;
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let preview_diff = enforce_current_preview_diff(client, &proposal, preview_diff_id).await?;
    let diff_text = get_diff_text(&preview_diff).await?;
    let location = diff::map_file_line(&diff_text, request.path("path")?, side, file_line)
        .ok_or_else(|| Error::invalid("file line is not present in the selected preview diff"))?;
    if !location.is_commentable() {
        return Err(Error::invalid(
            "original-side comments require a removed line",
        ));
    }

    let mut drafts = review_drafts(client, &proposal, preview_diff_id).await?;
    let draft_map = drafts
        .as_object_mut()
        .ok_or_else(|| Error::invalid("Launchpad returned invalid review drafts"))?;
    let diff_line = location.diff_line.to_string();
    let action = if let Some(body) = request.body.as_deref() {
        let body = body.trim();
        if body.is_empty() {
            return Err(Error::invalid("draft body cannot be empty"));
        }
        draft_map.insert(diff_line, Value::String(body.to_owned()));
        "saved"
    } else {
        draft_map.remove(&diff_line);
        "deleted"
    };
    let (_, current_proposal) = request_merge_proposal(client, request).await?;
    enforce_current_preview_diff(client, &current_proposal, preview_diff_id).await?;
    save_review_drafts(client, &current_proposal, preview_diff_id, &drafts).await?;

    let source_url = render::text_field(&proposal, "web_link")
        .unwrap_or(request.target()?)
        .to_owned();
    let side = match location.side {
        crate::diff::DiffSide::Original => "original",
        crate::diff::DiffSide::Modified => "modified",
    };
    let text = format!(
        "Review draft {action} on {}:{} ({side}, global diff line {})",
        location.path, location.file_line, location.diff_line
    );
    let details = json!({
        "kind": "review_draft_update",
        "preview_diff_id": preview_diff_id,
        "action": action,
        "location": location,
        "drafts": drafts,
    });
    Ok(OperationResult::new(text)
        .with_source_url(Some(source_url))
        .with_details(details))
}

async fn submit_review(client: &LaunchpadClient, request: &Request) -> Result<OperationResult> {
    let preview_diff_id = request.preview_diff_id()?;
    let (_, proposal) = request_merge_proposal(client, request).await?;
    enforce_current_preview_diff(client, &proposal, preview_diff_id).await?;
    let drafts = review_drafts(client, &proposal, preview_diff_id).await?;
    let draft_count = drafts.as_object().map_or(0, serde_json::Map::len);
    let content = request.body.as_deref().unwrap_or_default();
    if content.trim().is_empty() && request.vote.is_none() && draft_count == 0 {
        return Err(Error::invalid(
            "review must include content, a vote, or inline drafts",
        ));
    }
    let (_, current_proposal) = request_merge_proposal(client, request).await?;
    enforce_current_preview_diff(client, &current_proposal, preview_diff_id).await?;
    create_inline_review(
        client,
        &current_proposal,
        preview_diff_id,
        content,
        &drafts,
        request.vote.as_deref(),
    )
    .await?;

    let source_url = render::text_field(&proposal, "web_link")
        .unwrap_or(request.target()?)
        .to_owned();
    let text = format!(
        "Review submitted with {draft_count} inline comment{}: {source_url}",
        if draft_count == 1 { "" } else { "s" }
    );
    let details = json!({
        "kind": "inline_review",
        "preview_diff_id": preview_diff_id,
        "inline_comment_count": draft_count,
        "vote": request.vote,
    });
    Ok(OperationResult::new(text)
        .with_source_url(Some(source_url))
        .with_details(details))
}

async fn set_merge_proposal_status(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let status = request.status()?;
    let url = proposal_api_url(&proposal)?;
    client
        .post_pairs_url_ok(url.as_str(), &[("ws.op", "setStatus"), ("status", status)])
        .await?;
    let source_url = render::text_field(&proposal, "web_link")
        .unwrap_or(request.target()?)
        .to_owned();
    let text = format!("Merge proposal status set to {status}: {source_url}");
    let details = json!({ "status": status });
    Ok(OperationResult::new(text)
        .with_source_url(Some(source_url))
        .with_details(details))
}

async fn checkout_merge_proposal(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<OperationResult> {
    let (_, proposal) = request_merge_proposal(client, request).await?;
    let spec = checkout_spec(client, &proposal).await?;
    local_git::checkout(spec, request).await
}

async fn read_repository_file(request: &Request) -> Result<OperationResult> {
    let repository = normalise_repository(request.repository()?)?;
    let repository = repository.trim_matches('/');
    let path = request.path("path")?.trim();
    let branch = request.branch.as_deref().unwrap_or("(default)");
    let context =
        format!("cannot read Launchpad repository file {repository}:{path} at branch {branch}");
    let encoded_repository = encode_path(repository);
    let encoded_path = encode_path(path);
    let git_host = auth::git_host()?;
    let mut url = Url::parse(&format!(
        "https://{git_host}/{encoded_repository}/plain/{encoded_path}"
    ))
    .map_err(|source| Error::Url {
        url: format!("https://{git_host}/{encoded_repository}/plain/{encoded_path}"),
        source,
    })?;
    if let Some(branch) = request.branch.as_deref() {
        url.query_pairs_mut().append_pair("h", branch);
    }
    let text = fetch_git_plain_file(&url, &context).await?;
    let details = json!({
        "kind": "file",
        "repository": request.repository()?,
        "path": request.path("path")?,
        "branch": request.branch,
        "bytes": text.len(),
    });
    Ok(OperationResult::new(text)
        .with_source_url(Some(url.to_string()))
        .with_details(details))
}

async fn fetch_git_plain_file(url: &Url, context: &str) -> Result<String> {
    const FILE_TRANSPORT_GUIDANCE: &str = "git.launchpad.net/plain uses anonymous Git HTTP, not Launchpad API login; check repository, path and ref or use an authenticated Git checkout";

    let response = reqwest::Client::new()
        .get(url.clone())
        .send()
        .await
        .map_err(|source| {
            Error::context(
                context,
                Error::Web {
                    url: url.to_string(),
                    source,
                },
            )
        })?;
    if response.url().scheme() != url.scheme()
        || response.url().host_str() != url.host_str()
        || !response.url().path().contains("/plain/")
    {
        let host = response.url().host_str().unwrap_or("unknown host");
        return Err(Error::context(
            context,
            Error::GitFileResponse {
                reason: format!(
                    "redirected to {host} outside a Git plain-file URL; {FILE_TRANSPORT_GUIDANCE}"
                ),
            },
        ));
    }
    let status = response.status();
    if !status.is_success() {
        let guidance = if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            format!("; {FILE_TRANSPORT_GUIDANCE}")
        } else {
            String::new()
        };
        return Err(Error::context(
            context,
            Error::GitFileResponse {
                reason: format!("git.launchpad.net returned HTTP {status}{guidance}"),
            },
        ));
    }
    let html_response = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|content_type| {
            content_type.trim().eq_ignore_ascii_case("text/html")
                || content_type
                    .trim()
                    .eq_ignore_ascii_case("application/xhtml+xml")
        });
    if html_response {
        return Err(Error::context(
            context,
            Error::GitFileResponse {
                reason: format!(
                    "git.launchpad.net returned an HTML page instead of file content; {FILE_TRANSPORT_GUIDANCE}"
                ),
            },
        ));
    }
    let bytes = response.bytes().await.map_err(|source| {
        Error::context(
            context,
            Error::Web {
                url: url.to_string(),
                source,
            },
        )
    })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::context(
            context,
            Error::GitFileResponse {
                reason: format!("Launchpad file is larger than {MAX_FILE_BYTES} bytes"),
            },
        ));
    }
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
        Error::context(
            context,
            Error::GitFileResponse {
                reason: "file response is not UTF-8 text".to_owned(),
            },
        )
    })?;
    if text.trim() == "Invalid OpenID transaction" {
        return Err(Error::context(
            context,
            Error::GitFileResponse {
                reason: format!(
                    "git.launchpad.net returned an OpenID error instead of file content; {FILE_TRANSPORT_GUIDANCE}"
                ),
            },
        ));
    }
    Ok(text)
}

async fn request_merge_proposal(
    client: &LaunchpadClient,
    request: &Request,
) -> Result<(ResourceTarget, Value)> {
    resolve_merge_proposal_target(client, request.target()?).await
}

async fn resolve_merge_proposal_target(
    client: &LaunchpadClient,
    raw_target: &str,
) -> Result<(ResourceTarget, Value)> {
    let target = ResourceTarget::parse(raw_target)?;
    let proposal = match &target.kind {
        ResourceKind::MergeProposal { repository, id } => {
            get_merge_proposal(client, repository, *id).await?
        }
        ResourceKind::MergeProposalId { id } => resolve_merge_proposal_id(client, *id).await?,
        ResourceKind::Bug { .. } | ResourceKind::Repository | ResourceKind::Generic => {
            return Err(Error::invalid(
                "operation requires a merge proposal target; accepted syntax: numeric ID or lp://~owner/project/+git/repository/+merge/ID",
            ));
        }
    };
    Ok((target, proposal))
}

async fn resolve_merge_proposal_id(client: &LaunchpadClient, id: u64) -> Result<Value> {
    let current = local_git::current_repository().await.map_err(|error| {
        Error::invalid(format!(
            "cannot resolve merge proposal ID {id} without a Launchpad checkout: {error}; accepted syntax: numeric ID in a checkout with a Launchpad remote, or lp://~owner/project/+git/repository/+merge/{id}"
        ))
    })?;
    let mut repository_paths = Vec::new();
    for remote in &current.remotes {
        if let Ok(path) = normalise_repository(&remote.url)
            && !repository_paths.contains(&path)
        {
            repository_paths.push(path);
        }
    }
    let mut inspected_repositories = Vec::new();
    for path in &repository_paths {
        let Ok(repository) = get_repository(client, path).await else {
            continue;
        };
        inspected_repositories.push(
            render::text_field(&repository, "unique_name")
                .unwrap_or(path)
                .to_owned(),
        );
        for field in [
            "landing_targets_collection_link",
            "landing_candidates_collection_link",
        ] {
            let Some(collection_link) = render::text_field(&repository, field) else {
                continue;
            };
            if let Some(proposal) = find_proposal_by_id(client, collection_link, id).await? {
                return Ok(proposal);
            }
        }
    }
    let remotes = current
        .remotes
        .iter()
        .map(|remote| format!("{}={}", remote.name, remote.url))
        .collect::<Vec<_>>()
        .join(", ");
    let repositories = if inspected_repositories.is_empty() {
        "none".to_owned()
    } else {
        inspected_repositories.join(", ")
    };
    Err(Error::invalid(format!(
        "cannot resolve merge proposal ID {id}; working directory: {}; inspected remotes: {}; inspected Launchpad repositories: {repositories}; accepted syntax: numeric ID in a related Launchpad checkout or lp://~owner/project/+git/repository/+merge/{id}; retry with the full merge proposal target",
        current.working_directory.display(),
        if remotes.is_empty() { "none" } else { &remotes }
    )))
}

async fn find_proposal_by_id(
    client: &LaunchpadClient,
    first_url: &str,
    id: u64,
) -> Result<Option<Value>> {
    let mut seen = HashSet::new();
    let mut url = first_url.to_owned();
    loop {
        if !seen.insert(url.clone()) {
            return Err(Error::invalid(
                "Launchpad proposal pagination repeated a page",
            ));
        }
        let page: Collection<Value> = client.get_url(&url).await?;
        if let Some(proposal) = page.entries.into_iter().find(|proposal| {
            render::resource_id(proposal).and_then(|value| value.parse().ok()) == Some(id)
        }) {
            return Ok(Some(proposal));
        }
        let Some(next_url) = page.next_collection_link else {
            return Ok(None);
        };
        url = next_url;
    }
}

async fn get_preview_diffs(client: &LaunchpadClient, proposal: &Value) -> Result<Vec<Value>> {
    let collection_link = render::text_field(proposal, "preview_diffs_collection_link")
        .ok_or_else(|| Error::invalid("merge proposal has no preview diff history"))?;
    fetch_all_entries(client, collection_link).await
}

async fn get_preview_diff(
    client: &LaunchpadClient,
    proposal: &Value,
    requested_id: Option<u64>,
) -> Result<Value> {
    if let Some(requested_id) = requested_id {
        return get_preview_diffs(client, proposal)
            .await?
            .into_iter()
            .find(|preview_diff| {
                preview_diff.get("id").and_then(Value::as_u64) == Some(requested_id)
            })
            .ok_or_else(|| {
                Error::invalid(format!(
                    "preview diff {requested_id} does not belong to this merge proposal"
                ))
            });
    }

    let preview_diff_link = render::text_field(proposal, "preview_diff_link")
        .ok_or_else(|| Error::invalid("merge proposal has no current preview diff"))?;
    client.get_url(preview_diff_link).await.map_err(Into::into)
}

fn preview_diff_id(preview_diff: &Value) -> Result<u64> {
    preview_diff
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::invalid("Launchpad returned a preview diff without an ID"))
}

async fn get_diff_text(preview_diff: &Value) -> Result<String> {
    let diff_text_link = render::text_field(preview_diff, "diff_text_link")
        .ok_or_else(|| Error::invalid("preview diff has no diff text"))?;
    let url = diff_download_url(diff_text_link)?;
    let response = reqwest::get(url.clone())
        .await
        .map_err(|source| Error::Web {
            url: url.to_string(),
            source,
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::HttpStatus {
            url: url.to_string(),
            status,
        });
    }
    response.text().await.map_err(|source| Error::Web {
        url: url.to_string(),
        source,
    })
}

fn diff_download_url(diff_text_link: &str) -> Result<Url> {
    let base_url = api_base_url()?;
    let base = Url::parse(&base_url).map_err(|source| Error::Url {
        url: base_url,
        source,
    })?;
    let mut url = Url::parse(diff_text_link).map_err(|source| Error::Url {
        url: diff_text_link.to_owned(),
        source,
    })?;
    let base_path = base.path().trim_end_matches('/');
    let path_matches = base_path.is_empty()
        || url.path() == base_path
        || url
            .path()
            .strip_prefix(base_path)
            .is_some_and(|path| path.starts_with('/'));
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
        || !path_matches
    {
        return Err(Error::invalid(
            "preview diff text URL is outside the configured Launchpad API",
        ));
    }

    let web_host = match base.host_str() {
        Some("api.launchpad.net") => Some("code.launchpad.net"),
        Some("api.staging.launchpad.net") => Some("code.staging.launchpad.net"),
        Some("api.qastaging.launchpad.net") => Some("code.qastaging.launchpad.net"),
        _ => None,
    };
    if let Some(web_host) = web_host {
        let relative_path = url
            .path()
            .strip_prefix(base_path)
            .and_then(|path| path.strip_suffix("/diff_text"))
            .ok_or_else(|| Error::invalid("preview diff text URL has an invalid path"))?;
        let path = format!("{relative_path}/+files/preview.diff");
        url.set_host(Some(web_host))
            .map_err(|_| Error::invalid("preview diff text URL has an invalid host"))?;
        url.set_path(&path);
        url.set_query(None);
    }
    Ok(url)
}

async fn inline_comments(
    client: &LaunchpadClient,
    proposal: &Value,
    preview_diff_id: u64,
) -> Result<Vec<Value>> {
    let url = review_operation_url(proposal, "getInlineComments", preview_diff_id)?;
    let comments: Value = client.get_url(url.as_str()).await?;
    comments
        .as_array()
        .cloned()
        .ok_or_else(|| Error::invalid("Launchpad returned invalid inline comments"))
}

async fn review_drafts(
    client: &LaunchpadClient,
    proposal: &Value,
    preview_diff_id: u64,
) -> Result<Value> {
    let url = review_operation_url(proposal, "getDraftInlineComments", preview_diff_id)?;
    let drafts: Value = client.get_url(url.as_str()).await?;
    if drafts.is_null() {
        return Ok(json!({}));
    }
    let Some(drafts) = drafts.as_object() else {
        return Err(Error::invalid("Launchpad returned invalid review drafts"));
    };
    if drafts
        .iter()
        .any(|(line, body)| line.parse::<usize>().is_err() || !body.is_string())
    {
        return Err(Error::invalid("Launchpad returned invalid review drafts"));
    }
    Ok(Value::Object(drafts.clone()))
}

async fn save_review_drafts(
    client: &LaunchpadClient,
    proposal: &Value,
    preview_diff_id: u64,
    drafts: &Value,
) -> Result<()> {
    let url = proposal_api_url(proposal)?;
    let preview_diff_id = preview_diff_id.to_string();
    let drafts = drafts.to_string();
    client
        .post_pairs_url_ok(
            url.as_str(),
            &[
                ("ws.op", "saveDraftInlineComment"),
                ("previewdiff_id", preview_diff_id.as_str()),
                ("comments", drafts.as_str()),
            ],
        )
        .await?;
    Ok(())
}

async fn create_inline_review(
    client: &LaunchpadClient,
    proposal: &Value,
    preview_diff_id: u64,
    content: &str,
    drafts: &Value,
    vote: Option<&str>,
) -> Result<()> {
    let url = proposal_api_url(proposal)?;
    let preview_diff_id = preview_diff_id.to_string();
    let drafts = drafts.to_string();
    let mut parameters = vec![
        ("ws.op", "createComment"),
        ("content", content),
        ("previewdiff_id", preview_diff_id.as_str()),
        ("inline_comments", drafts.as_str()),
    ];
    if let Some(vote) = vote {
        parameters.push(("vote", vote));
    }
    client.post_pairs_url_ok(url.as_str(), &parameters).await?;
    Ok(())
}

async fn enforce_current_preview_diff(
    client: &LaunchpadClient,
    proposal: &Value,
    requested_id: u64,
) -> Result<Value> {
    let current = get_preview_diff(client, proposal, None).await?;
    validate_current_preview_diff(&current, requested_id)?;
    Ok(current)
}

fn validate_current_preview_diff(current: &Value, requested_id: u64) -> Result<()> {
    let current_id = preview_diff_id(current)?;
    if requested_id != current_id {
        return Err(Error::invalid(format!(
            "preview diff {requested_id} is stale; current preview diff is {current_id}"
        )));
    }
    if current.get("stale").and_then(Value::as_bool) == Some(true) {
        return Err(Error::invalid(format!(
            "preview diff {requested_id} is marked stale"
        )));
    }
    Ok(())
}

fn review_operation_url(proposal: &Value, operation: &str, preview_diff_id: u64) -> Result<Url> {
    let mut url = proposal_api_url(proposal)?;
    url.query_pairs_mut()
        .append_pair("ws.op", operation)
        .append_pair("previewdiff_id", &preview_diff_id.to_string());
    Ok(url)
}

fn proposal_api_url(proposal: &Value) -> Result<Url> {
    let self_link = render::text_field(proposal, "self_link")
        .ok_or_else(|| Error::invalid("merge proposal has no API link"))?;
    Url::parse(self_link).map_err(|source| Error::Url {
        url: self_link.to_owned(),
        source,
    })
}

async fn fetch_all_entries(client: &LaunchpadClient, first_url: &str) -> Result<Vec<Value>> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut url = first_url.to_owned();
    loop {
        if !seen.insert(url.clone()) {
            return Err(Error::invalid(
                "Launchpad collection pagination repeated a page",
            ));
        }
        let page: Collection<Value> = client.get_url(&url).await?;
        entries.extend(page.entries);
        let Some(next_url) = page.next_collection_link else {
            break;
        };
        url = next_url;
    }
    Ok(entries)
}

async fn fetch_entries(
    client: &LaunchpadClient,
    first_url: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    let mut entries = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    let mut url = first_url.to_owned();
    while entries.len() < limit {
        if !seen.insert(url.clone()) {
            return Err(Error::invalid(
                "Launchpad collection pagination repeated a page",
            ));
        }
        let page: Collection<Value> = client.get_url(&url).await?;
        let remaining = limit - entries.len();
        entries.extend(page.entries.into_iter().take(remaining));
        let Some(next_url) = page.next_collection_link else {
            break;
        };
        url = next_url;
    }
    Ok(entries)
}

async fn get_repository(client: &LaunchpadClient, path: &str) -> Result<Value> {
    let path = normalise_repository(path)?;
    let url = client.url(&format!("/+git?ws.op=getByPath&path={}", urlenc(&path)));
    let repository: Value = client.get_url(&url).await?;
    if repository.is_null() {
        return Err(LpError::NotFound(path).into());
    }
    Ok(repository)
}

async fn get_merge_proposal(client: &LaunchpadClient, repository: &str, id: u64) -> Result<Value> {
    let proposal = client.get(&format!("/{repository}/+merge/{id}")).await?;
    Ok(proposal)
}

async fn find_git_ref(
    client: &LaunchpadClient,
    role: &str,
    repository: &str,
    requested_path: &str,
    deadline: Option<Instant>,
) -> Result<(GitRef, String)> {
    let context = format!("cannot resolve {role} ref {requested_path} in repository {repository}");
    let resource = match get_repository(client, repository).await {
        Ok(resource) => resource,
        Err(source) => {
            let lookup_context =
                format!("cannot resolve {role} repository {repository} for ref {requested_path}");
            if !matches!(
                &source,
                Error::Launchpad {
                    source: LpError::NotFound(_) | LpError::Api { status: 404, .. }
                }
            ) {
                return Err(Error::context(lookup_context, source));
            }
            let path = normalise_repository(repository)
                .map_err(|error| Error::context(lookup_context.clone(), error))?;
            let Some((owner, name)) = path.split_once('/') else {
                return Err(Error::context(lookup_context, source));
            };
            if !owner.starts_with('~') || owner.len() == 1 || name.is_empty() || name.contains('/')
            {
                return Err(Error::context(lookup_context, source));
            }
            let alias = format!("{path}/+git/{name}");
            get_repository(client, &alias).await.map_err(|source| {
                let guidance = if matches!(
                    &source,
                    Error::Launchpad {
                        source: LpError::NotFound(_) | LpError::Api { status: 404, .. }
                    }
                ) {
                    format!(
                        "{lookup_context}; neither {path} nor {alias} was found; provide the exact canonical repository path lp://{path}/+git/<repository>"
                    )
                } else {
                    format!("{lookup_context}; alias lookup {alias} failed")
                };
                Error::context(guidance, source)
            })?
        }
    };
    let canonical_repository = render::text_field(&resource, "unique_name")
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| {
            Error::invalid(format!(
                "{context}; Launchpad repository has no unique name"
            ))
        })?;
    let mut delay = Duration::from_secs(2);
    loop {
        let git_refs = list_git_refs(client, canonical_repository)
            .await
            .map_err(|source| {
                Error::context(
                    format!("{context} (canonical repository {canonical_repository})"),
                    source.into(),
                )
            })?;
        let git_ref = git_refs
            .into_iter()
            .find(|git_ref| git_ref.path.as_deref() == Some(requested_path));
        let (commit, indexed_commit) = if let Some(git_ref) = git_ref {
            if deadline.is_none() || git_ref.commit_sha1.is_none() {
                return Ok((git_ref, canonical_repository.to_owned()));
            }
            match git_server_commit(&resource, requested_path).await {
                GitVisibility::Present(commit)
                    if git_ref.commit_sha1.as_deref() != Some(commit.as_str()) =>
                {
                    (commit, git_ref.commit_sha1)
                }
                _ => return Ok((git_ref, canonical_repository.to_owned())),
            }
        } else {
            match git_server_commit(&resource, requested_path).await {
                GitVisibility::Present(commit) => (commit, None),
                GitVisibility::Absent => {
                    return Err(Error::invalid(format!(
                        "{context}; ref absent from both the canonical repository's Git server and Launchpad API"
                    )));
                }
                GitVisibility::Unknown => {
                    return Err(Error::RefVisibilityUnknown {
                        reason: format!(
                            "{context}; ref absent from Launchpad API but Git server visibility cannot be confirmed (transport, authentication, or permission error)"
                        ),
                    });
                }
            }
        };
        let last_scan = render::text_field(&resource, "date_last_scanned").unwrap_or("unavailable");
        let pending = || Error::RefPendingIndex {
            reason: format!(
                "{role} ref {requested_path} in canonical repository {canonical_repository}; Git commit {commit}; Launchpad indexed commit {}; Launchpad last scan {last_scan}; Git publication succeeded, but Launchpad API indexing remains pending; retry later",
                indexed_commit.as_deref().unwrap_or("unavailable")
            ),
        };
        let Some(end) = deadline else {
            return Err(pending());
        };
        let remaining = end.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(pending());
        }
        sleep(delay.min(remaining)).await;
        delay = (delay * 2).min(Duration::from_secs(16));
    }
}

enum GitVisibility {
    Present(String),
    Absent,
    Unknown,
}

async fn git_server_commit(repository: &Value, path: &str) -> GitVisibility {
    let Some(url) = render::text_field(repository, "git_https_url") else {
        return GitVisibility::Unknown;
    };
    let Ok(parsed) = Url::parse(url) else {
        return GitVisibility::Unknown;
    };
    if parsed.scheme() != "https" && !(cfg!(test) && parsed.scheme() == "file") {
        return GitVisibility::Unknown;
    }
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("git")
            .args(["ls-remote", "--refs", url, path])
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(result)) = result else {
        return GitVisibility::Unknown;
    };
    if !result.status.success() {
        return GitVisibility::Unknown;
    }
    let Ok(output) = std::str::from_utf8(&result.stdout) else {
        return GitVisibility::Unknown;
    };
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(_, git_path)| *git_path == path)
        .map_or(GitVisibility::Absent, |(sha, _)| {
            GitVisibility::Present(sha.to_owned())
        })
}

async fn proposal_details(client: &LaunchpadClient, proposal: &Value) -> Result<Value> {
    let source_repository = linked_resource(client, proposal, "source_git_repository_link").await?;
    let target_repository = linked_resource(client, proposal, "target_git_repository_link").await?;
    let prerequisite_repository =
        if let Some(link) = render::text_field(proposal, "prerequisite_git_repository_link") {
            Some(client.get_url::<Value>(link).await?)
        } else {
            None
        };
    Ok(json!({
        "kind": "branch_merge_proposal",
        "id": render::resource_id(proposal),
        "url": render::scalar_field(proposal, "web_link"),
        "status": render::scalar_field(proposal, "queue_status"),
        "reviewed_revid": render::scalar_field(proposal, "reviewed_revid"),
        "source_ref": render::scalar_field(proposal, "source_git_path"),
        "target_ref": render::scalar_field(proposal, "target_git_path"),
        "prerequisite_ref": render::scalar_field(proposal, "prerequisite_git_path"),
        "prerequisite_repository": prerequisite_repository.as_ref().and_then(|repository| render::scalar_field(repository, "unique_name")),
        "source_repository": render::scalar_field(&source_repository, "unique_name"),
        "target_repository": render::scalar_field(&target_repository, "unique_name"),
        "source_repository_target_link": render::scalar_field(&source_repository, "target_link"),
        "target_repository_target_link": render::scalar_field(&target_repository, "target_link"),
        "source_https_url": render::scalar_field(&source_repository, "git_https_url"),
        "source_ssh_url": render::scalar_field(&source_repository, "git_ssh_url"),
        "target_https_url": render::scalar_field(&target_repository, "git_https_url"),
        "target_ssh_url": render::scalar_field(&target_repository, "git_ssh_url"),
    }))
}

async fn checkout_spec(client: &LaunchpadClient, proposal: &Value) -> Result<CheckoutSpec> {
    let source_repository = linked_resource(client, proposal, "source_git_repository_link").await?;
    let target_repository = linked_resource(client, proposal, "target_git_repository_link").await?;
    let id =
        render::resource_id(proposal).ok_or_else(|| Error::invalid("merge proposal has no ID"))?;
    let source_ref = render::scalar_field(proposal, "source_git_path")
        .ok_or_else(|| Error::invalid("merge proposal has no source ref"))?;
    let source_repository_name = render::scalar_field(&source_repository, "unique_name")
        .ok_or_else(|| Error::invalid("merge proposal has no source repository"))?;
    Ok(CheckoutSpec {
        id,
        source_ref,
        source_repository: source_repository_name,
        source_https_url: render::scalar_field(&source_repository, "git_https_url"),
        source_ssh_url: render::scalar_field(&source_repository, "git_ssh_url"),
        target_https_url: render::scalar_field(&target_repository, "git_https_url"),
        target_ssh_url: render::scalar_field(&target_repository, "git_ssh_url"),
        web_link: render::scalar_field(proposal, "web_link"),
    })
}

async fn linked_resource(
    client: &LaunchpadClient,
    resource: &Value,
    link_name: &str,
) -> Result<Value> {
    let link = render::text_field(resource, link_name)
        .ok_or_else(|| Error::invalid(format!("resource has no {link_name}")))?;
    let resource = client.get_url(link).await?;
    Ok(resource)
}

fn append_values(
    query: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
    name: &str,
    values: Option<&OneOrMany>,
) {
    if let Some(values) = values {
        values.append_to(query, name);
    }
}

fn normalise_ref(path: &str) -> String {
    if path.starts_with("refs/") {
        path.to_owned()
    } else {
        format!("refs/heads/{path}")
    }
}

fn resource_kind(resource: &Value) -> String {
    render::text_field(resource, "resource_type_link")
        .and_then(|link| link.rsplit('#').next())
        .unwrap_or("resource")
        .to_owned()
}

fn launchpad_web_url(path: &str, kind: &str) -> String {
    let host = if kind == "bug" {
        "https://bugs.launchpad.net"
    } else {
        "https://code.launchpad.net"
    };
    format!("{host}/{path}")
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|segment| utf8_percent_encode(segment, GIT_PATH_SEGMENT_ENCODE_SET).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn floor_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::client::LaunchpadClient;
    use chrono::DateTime;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use url::Url;

    use super::{
        ProposalLookup, ProposalLookupContext, eligible_proposals, encode_path,
        fetch_git_plain_file, find_git_ref, format_discussion_result, matches_discussion_filters,
        normalise_inline_comment, proposal_lookup_result, related_repository_branch_proposals,
        repository_branch_proposals, select_preview_diff, select_proposal, thread_state,
        validate_current_preview_diff, view_merge_proposal_for_branch,
    };
    use crate::request::{DiscussionFormat, Request, ResourceTarget};
    use tokio::sync::mpsc;

    async fn read_request_headers(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 512];
            let size = stream.read(&mut chunk).await.unwrap();
            assert!(size > 0, "client closed before completing request headers");
            request.extend_from_slice(&chunk[..size]);
            if request.windows(4).any(|part| part == b"\r\n\r\n") {
                return String::from_utf8(request).unwrap();
            }
        }
    }

    async fn read_http_request(stream: &mut TcpStream) -> (String, String) {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut chunk = [0; 1024];
            let size = stream.read(&mut chunk).await.unwrap();
            assert!(size > 0, "client closed before completing request");
            bytes.extend_from_slice(&chunk[..size]);
            if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let mut chunk = [0; 1024];
            let size = stream.read(&mut chunk).await.unwrap();
            assert!(size > 0, "client closed before completing request body");
            bytes.extend_from_slice(&chunk[..size]);
        }
        let body =
            String::from_utf8(bytes[header_end..header_end + content_length].to_vec()).unwrap();
        (headers, body)
    }

    #[tokio::test]
    async fn rejects_web_login_responses_without_rejecting_source_text() {
        let cases = [
            (
                "Text/Html; charset=utf-8",
                "<html>Invalid OpenID transaction</html>",
                Some("HTML"),
            ),
            ("text/plain", "Invalid OpenID transaction", Some("OpenID")),
            ("text/plain", "Example: Invalid OpenID transaction", None),
        ];
        for (content_type, body, expected_error) in cases {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_request_headers(&mut stream).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let url = Url::parse(&format!("http://{address}/repo/plain/README?h=main")).unwrap();
            let result = fetch_git_plain_file(
                &url,
                "cannot read Launchpad repository file repo:README at branch main",
            )
            .await;
            server.await.unwrap();
            if let Some(expected_error) = expected_error {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("repo:README at branch main"));
                assert!(error.contains(expected_error));
            } else {
                assert_eq!(result.unwrap(), body);
            }
        }
    }

    #[tokio::test]
    async fn redirected_plain_file_reports_anonymous_access_without_assuming_privacy() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            let redirect = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{address}/login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(redirect.as_bytes()).await.unwrap();
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\nConnection: close\r\n\r\nlogin")
                .await
                .unwrap();
        });
        let url = Url::parse(&format!("http://{address}/repo/plain/package.json")).unwrap();
        let error = fetch_git_plain_file(&url, "cannot read repo:package.json")
            .await
            .unwrap_err()
            .to_string();
        server.await.unwrap();
        assert!(error.contains("redirected"));
        assert!(error.contains("anonymous Git HTTP"));
        assert!(!error.contains("private"));
    }

    #[test]
    fn git_plain_paths_preserve_repository_names_and_escape_unsafe_bytes() {
        let public = Url::parse(&format!(
            "https://git.launchpad.net/{}/plain/{}",
            encode_path("launchpad-ui"),
            encode_path("package.json")
        ))
        .unwrap();
        assert_eq!(public.path(), "/launchpad-ui/plain/package.json");
        let canonical = Url::parse(&format!(
            "https://git.launchpad.net/{}/plain/src/main.rs",
            encode_path("~owner/my-project/+git/my.repo")
        ))
        .unwrap();
        assert_eq!(
            canonical.path(),
            "/~owner/my-project/+git/my.repo/plain/src/main.rs"
        );
        assert_eq!(encode_path("a b/c?#.txt"), "a%20b/c%3F%23.txt");
    }

    #[tokio::test]
    async fn missing_discussion_respects_structured_and_both_formats() {
        let client = LaunchpadClient::new(None);
        let request: Request = serde_json::from_str(
            r#"{"op":"merge_proposal_discussion","repository":"launchpad-ui","branch":"missing"}"#,
        )
        .unwrap();
        for format in [DiscussionFormat::Structured, DiscussionFormat::Both] {
            let lookup = proposal_lookup_result(
                &client,
                &request,
                ProposalLookup::Missing {
                    repository: "~owner/launchpad-ui/+git/launchpad-ui".to_owned(),
                    inspected_repositories: vec![
                        "~other/launchpad-ui/+git/launchpad-ui".to_owned(),
                    ],
                },
                ProposalLookupContext {
                    requested_repository: "launchpad-ui",
                    branch: "missing",
                    inferred: false,
                    kind: "merge_proposal_discussion",
                    metadata: None,
                },
            )
            .await
            .unwrap();
            let result = format_discussion_result(lookup, format);
            let details = Value::Object(result.details);
            assert_eq!(details["found"], false);
            assert_eq!(
                details["repository"],
                "~owner/launchpad-ui/+git/launchpad-ui"
            );
            assert_eq!(details["requested_repository"], "launchpad-ui");
            assert_eq!(
                details["inspected_repositories"],
                json!(["~other/launchpad-ui/+git/launchpad-ui"])
            );
            if format == DiscussionFormat::Structured {
                assert_eq!(
                    serde_json::from_str::<Value>(&result.text).unwrap(),
                    details
                );
            } else {
                assert!(result.text.starts_with("# No merge proposal found"));
                let (_, json_block) = result.text.split_once("```json\n").unwrap();
                let (json_block, _) = json_block.split_once("\n```").unwrap();
                assert_eq!(serde_json::from_str::<Value>(json_block).unwrap(), details);
            }
        }
    }

    #[tokio::test]
    async fn absent_branch_lookup_reports_canonical_and_inspected_repositories() {
        let canonical = "~owner/launchpad-ui/+git/launchpad-ui";
        let related = "~other/launchpad-ui/+git/launchpad-ui";
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let refs_path = format!("/devel/{canonical}/refs");
        let related_ref_path = format!("/devel/{related}?ws.op=getRefByPath&");
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request_headers(&mut stream).await;
                let path = request.split_ascii_whitespace().nth(1).unwrap_or_default();
                let body = if path == "/devel/+git?ws.op=getByPath&path=launchpad-ui" {
                    json!({
                        "unique_name": canonical,
                        "name": "launchpad-ui",
                        "target_link": format!("http://{address}/devel/launchpad-ui"),
                        "landing_targets_collection_link": format!("http://{address}/devel/landing_targets"),
                    })
                } else if path == refs_path || path == "/devel/landing_targets" {
                    json!({ "entries": [], "next_collection_link": null })
                } else if path.starts_with("/devel/+git?ws.op=getRepositories&") {
                    json!({
                        "entries": [{
                            "unique_name": related,
                            "name": "launchpad-ui",
                            "self_link": format!("http://{address}/devel/{related}"),
                        }],
                        "next_collection_link": null,
                    })
                } else if path.starts_with(&related_ref_path) {
                    Value::Null
                } else {
                    panic!("unexpected request: {path}");
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
        let request: Request = serde_json::from_str(
            r#"{"op":"merge_proposal_for_branch","repository":"lp:launchpad-ui","branch":"missing"}"#,
        )
        .unwrap();
        let result = view_merge_proposal_for_branch(&client, &request).await;
        server.abort();
        let details = result.unwrap().details;
        assert_eq!(details["found"], false);
        assert_eq!(details["repository"], canonical);
        assert_eq!(details["requested_repository"], "lp:launchpad-ui");
        assert_eq!(details["inspected_repositories"], json!([related]));
    }

    #[tokio::test]
    async fn fallback_only_caps_when_more_than_250_proposals_exist() {
        let canonical = "~owner/project/+git/repo";
        for count in [250, 251] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let request = read_request_headers(&mut stream).await;
                    let path = request.split_ascii_whitespace().nth(1).unwrap_or_default();
                    let body = if path == format!("/devel/{canonical}/refs") {
                        json!({ "entries": [], "next_collection_link": null }).to_string()
                    } else if path == "/devel/landing_targets" {
                        let entries = (0..count)
                            .map(|index| {
                                json!({
                                    "source_git_path": if index == 250 {
                                        "refs/heads/feature"
                                    } else {
                                        "refs/heads/other"
                                    },
                                })
                            })
                            .collect::<Vec<_>>();
                        json!({ "entries": entries, "next_collection_link": null }).to_string()
                    } else {
                        panic!("unexpected request: {path}");
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            });
            let client =
                LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
            let repository = json!({
                "unique_name": canonical,
                "landing_targets_collection_link": format!("http://{address}/devel/landing_targets"),
            });
            let lookup =
                repository_branch_proposals(&client, &repository, "refs/heads/feature").await;
            server.abort();
            let (matches, capped) = lookup.unwrap();
            assert!(matches.is_empty());
            assert_eq!(capped, count > 250);
        }
    }

    #[tokio::test]
    async fn resolves_git_ssh_alias_refs_through_canonical_repository() {
        let canonical = "~launchpad-committers/mobot/+git/mobot";
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let refs_path = format!("/devel/{canonical}/refs");
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request_headers(&mut stream).await;
                let path = request.split_ascii_whitespace().nth(1).unwrap_or_default();
                let (status, body) = if path == "/devel/+git?ws.op=getByPath&path=mobot" {
                    ("200 OK", json!({ "unique_name": canonical }).to_string())
                } else if path == refs_path {
                    (
                        "200 OK",
                        json!({
                            "entries": [{
                                "path": "refs/heads/feature",
                                "self_link": format!("http://{address}/devel/{canonical}/+ref/feature"),
                            }],
                            "next_collection_link": null,
                        }).to_string(),
                    )
                } else {
                    ("404 Not Found", "{}".to_owned())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
        let result = find_git_ref(
            &client,
            "source",
            "git+ssh://goulinkh@git.launchpad.net/mobot",
            "refs/heads/feature",
            None,
        )
        .await;
        server.abort();
        let (git_ref, repository) = result.unwrap();
        assert_eq!(repository, canonical);
        assert_eq!(git_ref.path.as_deref(), Some("refs/heads/feature"));
        assert_eq!(
            git_ref.self_link.as_deref(),
            Some(format!("http://{address}/devel/{canonical}/+ref/feature").as_str())
        );
    }

    #[tokio::test]
    async fn resolves_two_segment_ssh_alias_after_canonical_lookup_misses() {
        let canonical = "~owner/project/+git/project";
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected, status, body) in [
                ("~owner/project", "404 Not Found", "{}".to_owned()),
                (
                    canonical,
                    "200 OK",
                    json!({ "unique_name": canonical }).to_string(),
                ),
                (
                    "~owner/project/+git/project/refs",
                    "200 OK",
                    json!({
                        "entries": [{
                            "path": "refs/heads/feature",
                            "self_link": format!("http://{address}/devel/{canonical}/+ref/feature"),
                        }],
                        "next_collection_link": null,
                    })
                    .to_string(),
                ),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request_headers(&mut stream).await;
                let path = request.split_ascii_whitespace().nth(1).unwrap_or_default();
                let url = Url::parse(&format!("http://localhost{path}")).unwrap();
                if let Some(requested) = url.query_pairs().find(|(key, _)| key == "path") {
                    assert_eq!(requested.1, expected);
                } else {
                    assert_eq!(url.path(), format!("/devel/{expected}"));
                }
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
        let (git_ref, repository) = find_git_ref(
            &client,
            "source",
            "git+ssh://git@git.launchpad.net/~owner/project",
            "refs/heads/feature",
            None,
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(repository, canonical);
        assert_eq!(git_ref.path.as_deref(), Some("refs/heads/feature"));
        assert_eq!(
            git_ref.self_link.as_deref(),
            Some(format!("http://{address}/devel/{canonical}/+ref/feature").as_str())
        );
    }

    #[tokio::test]
    async fn git_visible_ref_waits_for_launchpad_index() {
        let directory = std::env::temp_dir().join(format!(
            "launchpad-cli-ref-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q", "-b", "feature"])
                .arg(&directory)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&directory)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--allow-empty",
                    "-q",
                    "-m",
                    "initial"
                ])
                .status()
                .unwrap()
                .success()
        );
        let sha = String::from_utf8(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&directory)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        let git_url = Url::from_directory_path(&directory).unwrap().to_string();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_sha = sha.clone();
        let server = tokio::spawn(async move {
            let mut scans = 0;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let headers = read_request_headers(&mut stream).await;
                let path = headers.split_ascii_whitespace().nth(1).unwrap();
                let body = if path.starts_with("/devel/+git?") {
                    json!({
                        "unique_name": "~owner/project/+git/repo",
                        "git_https_url": git_url,
                        "date_last_scanned": "2026-09-28T00:00:00Z"
                    })
                } else if path == "/devel/~owner/project/+git/repo/refs" {
                    scans += 1;
                    json!({
                        "entries": if scans >= 2 { vec![json!({
                            "path": "refs/heads/feature",
                            "commit_sha1": if scans == 2 { "stale-sha" } else { &server_sha },
                            "self_link": format!("http://{address}/devel/~owner/project/+git/repo/+ref/feature")
                        })] } else { vec![] },
                        "next_collection_link": null
                    })
                } else {
                    panic!("unexpected request: {path}");
                }.to_string();
                stream.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
        let pending = find_git_ref(
            &client,
            "source",
            "~owner/project/+git/repo",
            "refs/heads/feature",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(pending.code(), Some("ref_pending_index"));
        assert!(pending.to_string().contains(&sha));
        let (indexed, _) = find_git_ref(
            &client,
            "source",
            "~owner/project/+git/repo",
            "refs/heads/feature",
            Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
        )
        .await
        .unwrap();
        assert_eq!(indexed.commit_sha1.as_deref(), Some(sha.as_str()));
        let missing = find_git_ref(
            &client,
            "source",
            "~owner/project/+git/repo",
            "refs/heads/never-created",
            None,
        )
        .await
        .unwrap_err();
        assert_ne!(missing.code(), Some("ref_pending_index"));
        assert!(missing.to_string().contains("absent from both"));
        server.abort();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn missing_related_git_ref_returns_no_proposals() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnull")
                .await
                .unwrap();
        });
        let client = LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
        let repository = json!({
            "self_link": format!("http://{address}/devel/~owner/project/+git/repo"),
        });
        let result =
            related_repository_branch_proposals(&client, &repository, "refs/heads/never-created")
                .await;
        server.await.unwrap();
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn selects_preview_diff_for_numeric_merge_proposal() {
        let mut target = ResourceTarget::parse("510799").unwrap();
        select_preview_diff(&mut target, Some(1_142_878)).unwrap();
        assert!(target.diff);
        assert_eq!(target.preview_diff_id, Some(1_142_878));
    }

    #[test]
    fn selects_preview_diff_for_merge_proposal_url() {
        let mut target =
            ResourceTarget::parse("lp://~owner/project/+git/repository/+merge/510799").unwrap();
        select_preview_diff(&mut target, Some(1_142_878)).unwrap();
        assert!(target.diff);
        assert_eq!(target.preview_diff_id, Some(1_142_878));
    }

    #[test]
    fn accepts_matching_resource_preview_diff() {
        let mut target =
            ResourceTarget::parse("lp://~owner/project/+git/repository/+merge/42/diff/17").unwrap();
        select_preview_diff(&mut target, Some(17)).unwrap();
        assert!(target.diff);
        assert_eq!(target.preview_diff_id, Some(17));
    }

    #[test]
    fn rejects_conflicting_resource_preview_diff() {
        let mut target =
            ResourceTarget::parse("lp://~owner/project/+git/repository/+merge/42/diff/17").unwrap();
        let error = select_preview_diff(&mut target, Some(18)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("target and preview_diff_id select different snapshots")
        );
    }

    #[test]
    fn rejects_preview_diff_for_non_merge_proposal() {
        let mut target = ResourceTarget::parse("lp://bugs/1").unwrap();
        let error = select_preview_diff(&mut target, Some(17)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("preview_diff_id requires a merge proposal target")
        );
    }

    #[test]
    fn prefers_active_proposal_unless_latest_is_requested() {
        let proposals = vec![
            json!({
                "self_link": "https://api.launchpad.net/devel/~owner/project/+git/repository/+merge/10",
                "queue_status": "Needs review",
                "date_created": "2026-01-01T00:00:00Z",
                "target_git_path": "refs/heads/main",
            }),
            json!({
                "self_link": "https://api.launchpad.net/devel/~owner/project/+git/repository/+merge/20",
                "queue_status": "Merged",
                "date_created": "2026-02-01T00:00:00Z",
                "target_git_path": "refs/heads/main",
            }),
            json!({
                "self_link": "https://api.launchpad.net/devel/~owner/project/+git/repository/+merge/30",
                "queue_status": "Superseded",
                "date_created": "2026-03-01T00:00:00Z",
                "target_git_path": "refs/heads/main",
            }),
        ];
        let preferred: Request = serde_json::from_str(
            r#"{"op":"merge_proposal_for_branch","repository":"project","branch":"feature"}"#,
        )
        .unwrap();
        let eligible = eligible_proposals(proposals.clone(), &preferred);
        assert_eq!(eligible.len(), 2);
        let (selected, _, _) = select_proposal(eligible, &preferred).unwrap();
        assert_eq!(selected["queue_status"], "Needs review");

        let latest: Request = serde_json::from_str(
            r#"{"op":"merge_proposal_for_branch","repository":"project","branch":"feature","latest":true}"#,
        )
        .unwrap();
        let eligible = eligible_proposals(proposals, &latest);
        let (selected, _, _) = select_proposal(eligible, &latest).unwrap();
        assert_eq!(selected["queue_status"], "Merged");
    }

    #[test]
    fn classifies_thread_state_from_preview_diff() {
        assert_eq!(thread_state(true, false), "open");
        assert_eq!(thread_state(false, false), "superseded");
        assert_eq!(thread_state(false, true), "outdated");
    }

    #[test]
    fn filters_comments_by_timestamp_and_normalised_identity() {
        let comment = normalise_inline_comment(
            &json!({
                "person": {
                    "name": "alice",
                    "display_name": "Alice Example",
                    "web_link": "https://launchpad.net/~alice"
                },
                "date": "2026-09-21T10:00:00+00:00",
                "text": "Review comment"
            }),
            0,
            &HashMap::new(),
        );
        let since = DateTime::parse_from_rfc3339("2026-09-21T09:59:00Z").unwrap();
        assert!(matches_discussion_filters(
            &comment,
            "author",
            Some(&since),
            Some("alice"),
        ));
        assert!(matches_discussion_filters(
            &comment,
            "author",
            Some(&since),
            Some("Alice Example"),
        ));
        assert!(!matches_discussion_filters(
            &comment,
            "author",
            Some(&since),
            Some("bob"),
        ));
        let malformed = json!({
            "author": {
                "username": "alice",
                "display_name": "Alice Example"
            },
            "created_at": "not-a-timestamp"
        });
        assert!(matches_discussion_filters(
            &malformed,
            "author",
            Some(&since),
            None,
        ));
    }

    #[test]
    fn rejects_non_current_preview_diff() {
        let current = json!({ "id": 102, "stale": false });
        let error = validate_current_preview_diff(&current, 101).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("preview diff 101 is stale; current preview diff is 102")
        );
    }

    #[test]
    fn rejects_current_preview_diff_marked_stale() {
        let current = json!({ "id": 102, "stale": true });
        let error = validate_current_preview_diff(&current, 102).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("preview diff 102 is marked stale")
        );
    }
    async fn proposal_scenario(
        reject_creation: bool,
        create_only: bool,
        cross_repository: bool,
        recover_existing: bool,
    ) -> (
        crate::result::Result<crate::response::OperationResult>,
        Vec<String>,
        Vec<(String, String)>,
    ) {
        let repository = "~owner/project/+git/repo";
        let prerequisite_repository = if cross_repository {
            "~owner/project/+git/base-repo"
        } else {
            repository
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let (statuses_tx, mut statuses_rx) = mpsc::unbounded_channel();
        let (creation_tx, mut creation_rx) = mpsc::unbounded_channel();
        let server_base = base.clone();
        let server = tokio::spawn(async move {
            let mut status = "Needs review".to_owned();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut stream).await;
                let mut request = headers.split_ascii_whitespace();
                let method = request.next().unwrap();
                let path = request.next().unwrap();
                let proposal_path = format!("/devel/{repository}/+merge/42");
                let new_path = format!("/devel/{repository}/+merge/43");
                let refs_path = format!("/devel/{repository}/refs");
                let source_path = format!("/devel/{repository}/+ref/feature");
                let parameters: Vec<_> = url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
                let (code, response, location) = if method == "GET" && path == proposal_path {
                    (
                        "200 OK",
                        json!({
                            "id": 42,
                            "self_link": format!("{server_base}/{repository}/+merge/42"),
                            "web_link": "https://code.launchpad.net/old/+merge/42",
                            "queue_status": status,
                            "source_git_repository_link": format!("{server_base}/{repository}"),
                            "target_git_repository_link": format!("{server_base}/{repository}"),
                            "source_git_path": "refs/heads/feature",
                            "target_git_path": "refs/heads/main",
                            "description": "Stacked change",
                            "commit_message": "Add dependent feature"
                        })
                        .to_string(),
                        None,
                    )
                } else if method == "GET" && path.starts_with("/devel/+git?") {
                    let requested = Url::parse(&format!("http://localhost{path}"))
                        .unwrap()
                        .query_pairs()
                        .find(|(key, _)| key == "path")
                        .unwrap()
                        .1
                        .into_owned();
                    assert!(
                        requested == repository
                            || (cross_repository && requested == prerequisite_repository)
                    );
                    (
                        "200 OK",
                        json!({ "unique_name": requested }).to_string(),
                        None,
                    )
                } else if method == "GET" && path == refs_path {
                    let entries: Vec<_> = ["feature", "main", "base"]
                        .iter()
                        .map(|name| {
                            json!({
                                "path": format!("refs/heads/{name}"),
                                "commit_sha1": format!("sha-{name}"),
                                "self_link": format!("{server_base}/{repository}/+ref/{name}")
                            })
                        })
                        .collect();
                    (
                        "200 OK",
                        json!({ "entries": entries, "next_collection_link": null }).to_string(),
                        None,
                    )
                } else if cross_repository
                    && method == "GET"
                    && path == format!("/devel/{prerequisite_repository}/refs")
                {
                    ("200 OK", json!({
                        "entries": [{
                            "path": "refs/heads/base",
                            "self_link": format!("{server_base}/{prerequisite_repository}/+ref/base")
                        }], "next_collection_link": null
                    }).to_string(), None)
                } else if method == "GET" && path == format!("{source_path}/landing_targets") {
                    let entries = if recover_existing {
                        vec![json!({
                            "id": 43,
                            "self_link": format!("{server_base}/{repository}/+merge/43"),
                            "web_link": "https://code.launchpad.net/new/+merge/43",
                            "date_created": chrono::Utc::now().to_rfc3339(),
                            "commit_message": "Add dependent feature",
                            "description": "Stacked change",
                            "source_git_repository_link": format!("{server_base}/{repository}"),
                            "target_git_repository_link": format!("{server_base}/{repository}"),
                            "source_git_path": "refs/heads/feature",
                            "target_git_path": "refs/heads/main",
                            "prerequisite_git_repository_link": format!("{server_base}/{prerequisite_repository}"),
                            "prerequisite_git_path": "refs/heads/base",
                            "source_git_commit_sha1": "sha-feature"
                        })]
                    } else {
                        Vec::new()
                    };
                    (
                        "200 OK",
                        json!({ "entries": entries, "next_collection_link": null }).to_string(),
                        None,
                    )
                } else if method == "POST" && path == proposal_path {
                    let new_status = parameters
                        .iter()
                        .find(|(key, _)| key == "status")
                        .unwrap()
                        .1
                        .clone();
                    status = new_status.clone();
                    statuses_tx.send(new_status).unwrap();
                    ("200 OK", "{}".to_owned(), None)
                } else if method == "POST" && path == source_path {
                    assert_eq!(
                        status,
                        if create_only {
                            "Needs review"
                        } else {
                            "Superseded"
                        }
                    );
                    creation_tx.send(parameters).unwrap();
                    if reject_creation {
                        (
                            "400 Bad Request",
                            r#"{"message":"rejected"}"#.to_owned(),
                            None,
                        )
                    } else {
                        (
                            "201 Created",
                            "{}".to_owned(),
                            Some(format!("{server_base}/{repository}/+merge/43")),
                        )
                    }
                } else if method == "GET" && path == new_path {
                    (
                        "200 OK",
                        json!({
                            "id": 43,
                            "self_link": format!("{server_base}/{repository}/+merge/43"),
                            "web_link": "https://code.launchpad.net/new/+merge/43",
                            "queue_status": "Needs review",
                            "commit_message": "Add dependent feature",
                            "description": "Stacked change",
                            "source_git_repository_link": format!("{server_base}/{repository}"),
                            "target_git_repository_link": format!("{server_base}/{repository}"),
                            "source_git_path": "refs/heads/feature",
                            "target_git_path": "refs/heads/main",
                            "prerequisite_git_repository_link": format!("{server_base}/{prerequisite_repository}"),
                            "prerequisite_git_path": "refs/heads/base"
                        })
                        .to_string(),
                        None,
                    )
                } else if method == "GET"
                    && (path == format!("/devel/{repository}")
                        || path == format!("/devel/{prerequisite_repository}"))
                {
                    (
                        "200 OK",
                        json!({ "unique_name": if path == format!("/devel/{repository}") { repository } else { prerequisite_repository } }).to_string(),
                        None,
                    )
                } else {
                    panic!("unexpected request: {method} {path}");
                };
                let location_header =
                    location.map_or_else(String::new, |url| format!("Location: {url}\r\n"));
                let reply = format!(
                    "HTTP/1.1 {code}\r\nContent-Type: application/json\r\n{location_header}Content-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let input = if create_only {
            json!({
                "op": "merge_proposal_create",
                "repository": repository,
                "source_ref": "feature",
                "target_ref": "main",
                "prerequisite_ref": "base",
                "prerequisite_repository": cross_repository.then_some(prerequisite_repository),
                "description": "Stacked change",
                "commit_message": "Add dependent feature"
            })
        } else {
            json!({
                "op": "replace_merge_proposal_prerequisite",
                "target": format!("lp://{repository}/+merge/42"),
                "merge_prerequisite": "base"
            })
        };
        let request: Request = serde_json::from_value(input).unwrap();
        request.validate().unwrap();
        let result = if create_only {
            super::create_merge_proposal(&client, &request).await
        } else {
            super::replace_merge_proposal_prerequisite(&client, &request).await
        };
        server.abort();
        let mut statuses = Vec::new();
        while let Ok(status) = statuses_rx.try_recv() {
            statuses.push(status);
        }
        let parameters = creation_rx.try_recv().unwrap_or_default();
        (result, statuses, parameters)
    }

    #[tokio::test]
    async fn replacement_preserves_main_target_and_sets_prerequisite() {
        let (result, statuses, parameters) = proposal_scenario(false, false, false, false).await;
        assert_eq!(statuses, ["Superseded"]);
        let fields: HashMap<_, _> = parameters.into_iter().collect();
        assert!(fields["merge_target"].ends_with("/+ref/main"));
        assert!(fields["merge_prerequisite"].ends_with("/+ref/base"));
        assert_eq!(fields["needs_review"], "true");
        assert_eq!(fields["initial_comment"], "Stacked change");
        let result = result.unwrap();
        assert!(result.text.contains("**Prerequisite:** refs/heads/base"));
        assert!(result.text.contains("**Target:** refs/heads/main"));
        assert!(
            result
                .text
                .contains("https://code.launchpad.net/new/+merge/43")
        );
    }

    #[tokio::test]
    async fn failed_replacement_restores_old_status() {
        let (result, statuses, _) = proposal_scenario(true, false, false, false).await;
        assert_eq!(statuses, ["Superseded", "Needs review"]);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("previous status restored")
        );
    }

    #[tokio::test]
    async fn creation_keeps_main_target_separate_from_prerequisite() {
        let (result, statuses, parameters) = proposal_scenario(false, true, false, false).await;
        assert!(statuses.is_empty());
        let fields: HashMap<_, _> = parameters.into_iter().collect();
        assert!(fields["merge_target"].ends_with("/+ref/main"));
        assert!(fields["merge_prerequisite"].ends_with("/+ref/base"));
        assert_eq!(fields["commit_message"], "Add dependent feature");
        let result = result.unwrap();
        assert!(result.text.contains("**Prerequisite:** refs/heads/base"));
        assert!(result.text.contains("**Target:** refs/heads/main"));
        assert!(
            result
                .text
                .contains("Merge proposal 43: Add dependent feature")
        );
        assert_eq!(result.details["source_commit_sha"], "sha-feature");
        assert_eq!(result.details["preview_state"], "pending");
        assert_eq!(result.details["creation_state"], "new");
        assert_eq!(
            result.details["prerequisite_repository"],
            "~owner/project/+git/repo"
        );
    }

    #[tokio::test]
    async fn creation_confirms_cross_repository_prerequisite() {
        let (result, _, parameters) = proposal_scenario(false, true, true, false).await;
        let fields: HashMap<_, _> = parameters.into_iter().collect();
        assert!(fields["merge_target"].ends_with("/repo/+ref/main"));
        assert!(fields["merge_prerequisite"].ends_with("/base-repo/+ref/base"));
        let result = result.unwrap();
        assert_eq!(result.details["prerequisite_ref"], "refs/heads/base");
        assert_eq!(
            result.details["prerequisite_repository"],
            "~owner/project/+git/base-repo"
        );
    }

    #[tokio::test]
    async fn creation_retry_recovers_recent_matching_proposal_without_posting() {
        let (result, statuses, parameters) = proposal_scenario(false, true, false, true).await;
        assert!(statuses.is_empty());
        assert!(parameters.is_empty());
        let result = result.unwrap();
        assert_eq!(result.details["id"], "43");
        assert_eq!(result.details["creation_state"], "recovered");
        assert_eq!(result.details["source_commit_sha"], "sha-feature");
        assert!(
            result
                .text
                .starts_with("# Recovered Launchpad merge proposal")
        );
    }

    #[tokio::test]
    async fn preview_history_reports_pending_before_first_diff() {
        let repository = "~owner/project/+git/repo";
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for expected in [
                format!("/devel/{repository}/+merge/42"),
                format!("/devel/{repository}/+merge/42/preview_diffs"),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let headers = read_request_headers(&mut stream).await;
                assert!(headers.contains(&format!("GET {expected} ")));
                let body = if expected.ends_with("/preview_diffs") {
                    json!({ "entries": [], "next_collection_link": null })
                } else {
                    json!({
                        "id": 42,
                        "self_link": format!("http://{address}/devel/{repository}/+merge/42"),
                        "preview_diffs_collection_link": format!("http://{address}/devel/{repository}/+merge/42/preview_diffs"),
                        "web_link": "https://code.launchpad.net/old/+merge/42"
                    })
                }.to_string();
                stream.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(format!("http://{address}/devel"));
        let request: Request = serde_json::from_value(json!({
            "op": "preview_diffs",
            "target": format!("lp://{repository}/+merge/42")
        }))
        .unwrap();
        let result = super::view_preview_diffs(&client, &request).await.unwrap();
        server.await.unwrap();
        assert_eq!(result.details["state"], "pending");
        assert_eq!(result.details["retryable"], true);
        assert_eq!(result.details["diffs"], json!([]));
        assert!(result.text.contains("Preview generation is pending"));
    }

    #[tokio::test]
    async fn creation_wait_reports_preview_when_it_becomes_ready() {
        let repository = "~owner/project/+git/repo";
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let repository_link = format!("{base}/{repository}");
        let proposal_link = format!("{repository_link}/+merge/42");
        let server_repository = repository_link.clone();
        let server_proposal = proposal_link.clone();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let headers = read_request_headers(&mut stream).await;
                let path = headers.split_ascii_whitespace().nth(1).unwrap();
                let body = if path == format!("/devel/{repository}/+merge/42") {
                    json!({
                        "self_link": server_proposal,
                        "web_link": "https://code.launchpad.net/new/+merge/42",
                        "source_git_repository_link": server_repository,
                        "target_git_repository_link": server_repository,
                        "source_git_path": "refs/heads/feature",
                        "target_git_path": "refs/heads/main",
                        "preview_diff_link": format!("{server_proposal}/+preview-diff/7")
                    })
                } else {
                    assert_eq!(path, format!("/devel/{repository}"));
                    json!({ "unique_name": repository })
                }
                .to_string();
                stream.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).as_bytes()).await.unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let request: Request = serde_json::from_value(json!({
            "op": "merge_proposal_create",
            "repository": repository,
            "source_ref": "feature",
            "target_ref": "main",
            "commit_message": "Feature",
            "wait_for_preview": true,
            "preview_timeout_seconds": 3
        }))
        .unwrap();
        let prepared = super::PreparedMergeProposal {
            source_link: format!("{repository_link}/+ref/feature"),
            target_link: format!("{repository_link}/+ref/main"),
            prerequisite_link: None,
            source_repository_link: repository_link.clone(),
            target_repository_link: repository_link.clone(),
            prerequisite_repository_link: None,
            source_ref: "refs/heads/feature".to_owned(),
            target_ref: "refs/heads/main".to_owned(),
            prerequisite_ref: None,
            source_commit_sha: Some("abc123".to_owned()),
        };
        let proposal = json!({
            "self_link": proposal_link,
            "source_git_repository_link": repository_link,
            "target_git_repository_link": repository_link,
            "source_git_path": "refs/heads/feature",
            "target_git_path": "refs/heads/main"
        });
        let result = super::creation_result(&client, proposal, &prepared, &request, "new")
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(result.details["preview_state"], "ready");
        assert_eq!(result.details["source_commit_sha"], "abc123");
    }

    #[tokio::test]
    async fn editing_commit_message_keeps_the_proposal_and_description() {
        let repository = "~owner/project/+git/repo";
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let server_base = base.clone();
        let server = tokio::spawn(async move {
            let proposal_path = format!("/devel/{repository}/+merge/42");
            let repository_path = format!("/devel/{repository}");
            let mut proposal = json!({
                "id": 42,
                "self_link": format!("{server_base}/{repository}/+merge/42"),
                "web_link": "https://code.launchpad.net/~owner/project/+git/repo/+merge/42",
                "queue_status": "Needs review",
                "source_git_repository_link": format!("{server_base}/{repository}"),
                "target_git_repository_link": format!("{server_base}/{repository}"),
                "source_git_path": "refs/heads/feature",
                "target_git_path": "refs/heads/main",
                "description": "The original explanation of this change",
                "commit_message": null
            });
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut stream).await;
                let mut request = headers.split_ascii_whitespace();
                let method = request.next().unwrap();
                let path = request.next().unwrap();
                let response = match (method, path) {
                    ("GET", requested) if requested == proposal_path => proposal.clone(),
                    ("PATCH", requested) if requested == proposal_path => {
                        assert_eq!(
                            serde_json::from_str::<Value>(&body).unwrap(),
                            json!({ "commit_message": "Correct concise title" })
                        );
                        proposal["commit_message"] = json!("Correct concise title");
                        proposal.clone()
                    }
                    ("GET", requested) if requested == repository_path => {
                        json!({ "unique_name": repository })
                    }
                    _ => panic!("unexpected request: {method} {path}"),
                };
                let body = response.to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
            assert_eq!(proposal["commit_message"], "Correct concise title");
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let request: Request = serde_json::from_value(json!({
            "op": "merge_proposal_edit",
            "target": format!("lp://{repository}/+merge/42"),
            "commit_message": "Correct concise title"
        }))
        .unwrap();
        request.validate().unwrap();
        let result = super::edit_merge_proposal(&client, &request).await.unwrap();
        server.await.unwrap();
        assert!(
            result
                .text
                .contains("Merge proposal 42: Correct concise title")
        );
        assert!(
            result
                .text
                .contains("The original explanation of this change")
        );
        assert_eq!(
            result.source_url.as_deref(),
            Some("https://code.launchpad.net/~owner/project/+git/repo/+merge/42")
        );
    }
    #[tokio::test]
    async fn linked_bugs_can_be_read_and_updated_from_a_proposal() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let proposal_path = "/devel/~owner/project/+git/repo/+merge/42";
        let bugs_path = format!("{proposal_path}/bugs");
        let proposal = json!({
            "self_link": format!("{base}/~owner/project/+git/repo/+merge/42"),
            "web_link": "https://code.launchpad.net/~owner/project/+git/repo/+merge/42",
            "bugs_collection_link": format!("{base}/~owner/project/+git/repo/+merge/42/bugs")
        });
        let server = tokio::spawn(async move {
            for expected in ["GET", "GET", "GET", "POST"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut stream).await;
                let mut words = headers.split_ascii_whitespace();
                let method = words.next().unwrap();
                let path = words.next().unwrap();
                assert_eq!(method, expected);
                let response = match (method, path) {
                    ("GET", p) if p == proposal_path => proposal.clone(),
                    ("GET", p) if p == bugs_path => json!({
                        "start": 0, "total_size": 1,
                        "entries": [{
                            "id": 17,
                            "title": "Fix the failing migration",
                            "web_link": "https://bugs.launchpad.net/bugs/17"
                        }]
                    }),
                    ("POST", p) if p == proposal_path => {
                        let fields: HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
                            .into_owned()
                            .collect();
                        assert_eq!(fields["ws.op"], "linkBug");
                        assert_eq!(fields["bug"], format!("http://{address}/devel/bugs/17"));
                        json!({})
                    }
                    _ => panic!("unexpected request: {method} {path}"),
                };
                let body = response.to_string();
                stream
                    .write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let target = "lp://~owner/project/+git/repo/+merge/42";
        let read: Request = serde_json::from_value(json!({
            "op": "merge_proposal_bugs", "target": target
        }))
        .unwrap();
        read.validate().unwrap();
        let result = super::view_merge_proposal_bugs(&client, &read)
            .await
            .unwrap();
        assert!(result.text.contains("#17: Fix the failing migration"));
        let link: Request = serde_json::from_value(json!({
            "op": "merge_proposal_link_bug", "target": target, "bug_id": 17
        }))
        .unwrap();
        link.validate().unwrap();
        let result = super::change_merge_proposal_bug_link(&client, &link)
            .await
            .unwrap();
        assert!(result.text.contains("Bug #17 linked to merge proposal 42"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bug_task_edit_changes_the_selected_task_not_the_parent_bug() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let server = tokio::spawn(async move {
            for expected in ["GET", "PATCH"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut stream).await;
                let mut words = headers.split_ascii_whitespace();
                let method = words.next().unwrap();
                let path = words.next().unwrap();
                assert_eq!((method, path), (expected, "/devel/project/+bug/42"));
                if method == "PATCH" {
                    assert_eq!(
                        serde_json::from_str::<Value>(&body).unwrap(),
                        json!({
                            "status": "Triaged",
                            "importance": "High",
                            "assignee_link": format!("http://{address}/devel/~alice")
                        })
                    );
                }
                let response = json!({
                    "resource_type_link": "https://api.launchpad.net/devel/#bug_task",
                    "web_link": "https://bugs.launchpad.net/project/+bug/42",
                    "status": if method == "PATCH" { "Triaged" } else { "New" },
                    "importance": if method == "PATCH" { "High" } else { "Undecided" }
                });
                let body = response.to_string();
                stream
                    .write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let request: Request = serde_json::from_value(json!({
            "op": "bug_task_edit",
            "target": "lp://project/+bug/42",
            "status": "Triaged",
            "importance": "High",
            "assignee": "alice"
        }))
        .unwrap();
        request.validate().unwrap();
        let result = super::edit_bug_task(&client, &request).await.unwrap();
        assert!(result.text.contains("Triaged"));
        assert!(result.text.contains("High"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn editing_a_comment_uses_its_own_resource_and_returns_new_content() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let server_base = base.clone();
        let server = tokio::spawn(async move {
            let mut edited = false;
            for expected in ["GET", "POST", "GET"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut stream).await;
                let mut words = headers.split_ascii_whitespace();
                let method = words.next().unwrap();
                let path = words.next().unwrap();
                assert_eq!(
                    (method, path),
                    (
                        expected,
                        "/devel/~owner/project/+git/repo/+merge/42/comments/7"
                    )
                );
                if method == "POST" {
                    let fields: HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
                        .into_owned()
                        .collect();
                    assert_eq!(fields["ws.op"], "editContent");
                    assert_eq!(fields["new_content"], "Corrected analysis");
                    edited = true;
                }
                let response = json!({
                    "self_link": format!("{server_base}/~owner/project/+git/repo/+merge/42/comments/7"),
                    "web_link": "https://code.launchpad.net/~owner/project/+git/repo/+merge/42/comments/7",
                    "resource_type_link": "https://api.launchpad.net/devel/#code_review_comment",
                    "content": if edited { "Corrected analysis" } else { "Earlier analysis" }
                });
                let body = response.to_string();
                stream
                    .write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let request: Request = serde_json::from_value(json!({
            "op": "comment_edit",
            "target": "lp://~owner/project/+git/repo/+merge/42/comments/7",
            "body": "Corrected analysis"
        }))
        .unwrap();
        request.validate().unwrap();
        let result = super::edit_comment(&client, &request).await.unwrap();
        assert!(result.text.contains("Corrected analysis"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn metadata_edits_target_the_bug_project_and_repository() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}/devel");
        let repo_path = "/devel/~owner/project/+git/repo";
        let repo_link = format!("{base}/~owner/project/+git/repo");
        let server = tokio::spawn(async move {
            let mut bug = json!({
                "id": 17, "title": "Old title", "description": "Existing description",
                "web_link": "https://bugs.launchpad.net/bugs/17"
            });
            let mut project = json!({
                "resource_type_link": "https://api.launchpad.net/devel/#project",
                "summary": "Old summary", "name": "project"
            });
            let mut repository = json!({
                "self_link": repo_link, "unique_name": "~owner/project/+git/repo",
                "default_branch": "refs/heads/master"
            });
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = read_http_request(&mut stream).await;
                let mut words = headers.split_ascii_whitespace();
                let method = words.next().unwrap();
                let path = words.next().unwrap();
                let response = match (method, path) {
                    ("PATCH", "/devel/bugs/17") => {
                        assert_eq!(
                            serde_json::from_str::<Value>(&body).unwrap(),
                            json!({"title": "New title"})
                        );
                        bug["title"] = json!("New title");
                        bug.clone()
                    }
                    ("GET", "/devel/project") => project.clone(),
                    ("PATCH", "/devel/project") => {
                        assert_eq!(
                            serde_json::from_str::<Value>(&body).unwrap(),
                            json!({"summary": "New summary"})
                        );
                        project["summary"] = json!("New summary");
                        project.clone()
                    }
                    ("GET", path) if path.starts_with("/devel/+git?") => {
                        let url = Url::parse(&format!("http://{address}{path}")).unwrap();
                        assert!(url.query_pairs().any(
                            |(key, value)| key == "path" && value == "~owner/project/+git/repo"
                        ));
                        repository.clone()
                    }
                    ("PATCH", path) if path == repo_path => {
                        assert_eq!(
                            serde_json::from_str::<Value>(&body).unwrap(),
                            json!({"default_branch": "refs/heads/main"})
                        );
                        repository["default_branch"] = json!("refs/heads/main");
                        repository.clone()
                    }
                    _ => panic!("unexpected request: {method} {path}"),
                };
                let body = response.to_string();
                stream
                    .write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = LaunchpadClient::new(None).with_base_url(base);
        let bug: Request = serde_json::from_value(json!({
            "op": "bug_edit", "target": "lp://bugs/17", "title": "New title"
        }))
        .unwrap();
        bug.validate().unwrap();
        assert!(
            super::edit_bug(&client, &bug)
                .await
                .unwrap()
                .text
                .contains("New title")
        );
        let project: Request = serde_json::from_value(json!({
            "op": "project_edit", "target": "lp://project", "summary": "New summary"
        }))
        .unwrap();
        project.validate().unwrap();
        assert!(
            super::edit_project(&client, &project)
                .await
                .unwrap()
                .text
                .contains("New summary")
        );
        let repository: Request = serde_json::from_value(json!({
            "op": "repository_edit", "target": "lp://~owner/project/+git/repo",
            "default_branch": "refs/heads/main"
        }))
        .unwrap();
        repository.validate().unwrap();
        assert!(
            super::edit_repository(&client, &repository)
                .await
                .unwrap()
                .text
                .contains("refs/heads/main")
        );
        server.await.unwrap();
    }
}
