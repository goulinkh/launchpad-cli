# Live development-instance validation — 2026-10-07

Tested the release binary against `https://api.launchpad.test/devel`, not mock
HTTP responses. All mutations were confined to the development installation.
The running application and detached screen session were left running.

## Coverage

The initial run recorded **103 CLI invocations**, including setup retries and
negative tests: **88 succeeded**. Every one of the **30 distinct high-level
operation names** was attempted; **28 succeeded**. There are 35 noun–verb command
entries because several commands share an operation. Earlier documentation's
claim of 31 distinct operations was incorrect.

| Area | Live results |
| --- | --- |
| Authentication | OAuth start, dev-admin approval, OAuth finish for author and reviewer; status, import, dry-run logout, logout using an isolated profile |
| Generic OpenAPI calls | Project/repository creation, project PATCH, repository lookup, discovery, description, input validation |
| Bugs | Create, view, search with repeated status filters, edit, clear tags, task status/importance/assignment/unassignment, comment and edit comment |
| Projects/repositories | View/edit, enable project bug tracking, set indexed default branch, read refs collection |
| Proposal creation | Same-repository, cross-repository, explicit prerequisite, index-pending diagnostics, preview wait, duplicate-creation recovery |
| Proposal reads | View, list, for-branch, current checkout, discussion, preview history, raw diff, linked bugs |
| Review | General comment and edit, file-to-diff line mapping, save/read/clear drafts, publish inline review, read inline comments, approve a later revision |
| Git | Initial branch push, feature push, HTTPS-to-SSH checkout fallback, push review fix, no-change force-with-lease, merge and push target branch |
| Safety | Missing resource, invalid schema input, stale-preview rejection, denied project edit; selected-instance Git host enforcement has regression tests |
| Compatibility | Original JSON `tool` invocation works alongside noun–verb commands |

Initially, the two high-level operations without a successful live result were
`file_read` and `replace_merge_proposal_prerequisite`. The follow-up below resolves
file reads using explicit SSH; API resubmission remains unsupported.

## Inspectable resources

All were newly created for this run:

- Project: <https://launchpad.test/cli-smoke-20261007>
- Source repository: <https://code.launchpad.test/~cli-smoke-20261007-author/cli-smoke-20261007/+git/agent-flow>
- Cross-repository target: <https://code.launchpad.test/~cli-smoke-20261007-author/cli-smoke-20261007/+git/target-flow>
- Bug **16**: <https://bugs.launchpad.test/cli-smoke-20261007/+bug/16> — **Fix Committed**
- Proposal **13**: <https://code.launchpad.test/~cli-smoke-20261007-author/cli-smoke-20261007/+git/agent-flow/+merge/13> — **Merged**
- Proposal **14**: same repository, `+merge/14` — **Rejected**, retained as the failed replacement fixture
- Proposal **15**: `+merge/15` — **Needs review**, created with `stack/base` as prerequisite
- Proposal **16**: `+merge/16` — **Needs review**, cross-repository target plus prerequisite
- Proposal **17**: `+merge/17` — **Needs review**, separately proposes `stack/base`; preview wait completed successfully

The identities are `cli-smoke-20261007-author` and
`cli-smoke-20261007-reviewer`, using `example.test` email addresses. They received
four-hour, public-write OAuth grants. A dedicated SSH public key was registered
on the author, not on an existing user's account. Credentials and private keys
are in the ignored, mode-0700 `target/dev-integration/private/` directory and
are not part of this report or tracked source.

Proposal 13 exercised the complete path: create repository → push branches →
create proposal → generate preview → reviewer comment and inline draft →
publish review → checkout → commit requested change → push → new preview →
reject a stale-snapshot draft → approve → merge locally → push `main` → verify
Launchpad reports **Merged**. Its linked bug was then marked **Fix Committed**.

## CLI defects found and corrected

1. **Development hosts were not supported consistently.** Added the
   `development` profile and `.test` OAuth, repository/resource parsing, Git-host,
   and preview-download mappings. Git operations now enforce the selected
   instance rather than accepting every official Launchpad host.
2. **Named generic POST operations failed with “No operation name given”.**
   The converter distinguishes operations using a URL query, but Launchpad
   dispatches POST operations from form data. Request planning now includes the
   fixed `ws.op` field, including for operations without a declared body, and
   rejects caller overrides. Actual project and repository creation then passed.
3. **Index checks could not use working SSH access.** They now fall back to SSH
   when HTTPS fails, still checking the selected Git host. A real pushed but
   unindexed branch now yields `ref_pending_index`, not `ref_visibility_unknown`.
4. **Checkout added a redundant upstream after SSH fallback.** When source and
   target are the same repository, it no longer mistakes their different
   transport URLs for different repositories. Retested with proposal 15.
5. **Repository collection URLs were mistaken for repository names.**
   `resource view .../+git/agent-flow/refs` now reads the actual collection.
6. **Some generated links pointed to production.** Bug-search links now use
   returned resource URLs; bug-comment and identity fallbacks no longer fabricate
   `launchpad.net` links while operating against another instance.
7. **Unsupported prerequisite replacement produced a server traceback.** The
   known server rejection is now reported as `unsupported_operation`, exit 2,
   with web-resubmit guidance. Other server failures remain runtime errors.

## Environment changes and fixture accommodations

- OAuth initially failed because the checkout expected
  `OAuthAccessToken.refresh_token`, but the dev database stopped at revision
  `2211-60-0`. A private `pg_dump -Fc` backup was taken on the development
  server; its location is recorded in the git-ignored local instance notes.
  Existing migrations **61 and 62** were applied with Launchpad's upgrade tool,
  followed by additive security grants. No database reset or
  application restart occurred. Token exchange then succeeded.
- Background Git/preview workers were not active. Launchpad's normal `JobRunner`
  processed only `GitRefScanJob` and `UpdatePreviewDiffJob` instances belonging to
  this run's repositories. No Git refs, commit metadata, or diff contents were
  fabricated in the database.
- Restricted librarian URLs pointed at an unconfigured hostname and HTTPS port.
  To test real diff downloads, only diff aliases associated with public test
  proposals **13, 15 and 16** were marked public. The diff bytes were unchanged.
  This deliberately scoped fixture accommodation is not proof that private or
  restricted downloads work in this environment. Proposal 17's preview readiness
  was tested without that accommodation.
- Git SSH used the configured Turnip listener through the development SSH host,
  with the host key pinned after verification against the dev checkout's Turnip
  key. Connection settings are local to the test environment. No TLS or SSH
  host-key checks were disabled. A conflicting key on a different SSH listener
  was not overwritten.

## Follow-up CLI fixes

After the initial run:

- Added explicit `repository file --transport ssh`. Live reads of `README.md`
  succeeded for both `main` and the remote's default ref. This brings successful
  high-level coverage to **29 of 30 operations**; resubmission remains unsupported.
- Removed the unsupported supersede/create/rollback sequence. Replacement now
  fails offline before credentials or network access, even with `--yes` or
  `--dry-run`. Discovery marks it unsupported. The high-level status command also
  rejects `Superseded` without sending it to the server.
- Cross-repository checkout now selects an SSH upstream when the source required
  SSH fallback. A fresh live checkout of proposal 16 verified the target remote.
- HTTP file reads now enforce their size limit while streaming, have a timeout,
  and reject off-origin/login redirects before following them. SSH reads use
  a disposable shallow fetch with a deadline and no worktree checkout.
- HTTP 401 diagnostics now acknowledge possible insufficient permissions while
  preserving the existing exit code. API errors expose the HTTP status in details.

These changes did not require additional server/database mutations. The SSH
routing used backend DNS from the development host, not a pinned container IP.

## Outstanding findings

### Anonymous Git HTTPS file reads

From the workstation, `git.launchpad.test` resolves to the web frontend, whose
certificate does not cover that hostname. Direct backend HTTPS also has a
self-signed certificate. `repository file` therefore fails correctly rather
than disabling certificate checks. Configure trusted Git HTTPS routing to test
anonymous HTTPS successfully; explicit SSH file reads, checkout and push work.
The CLI fix does not repair the server's HTTPS certificate/routing.

### Replacing a merge prerequisite

The server's `setStatus` implementation does not accept **Superseded**, despite
that value existing in the status enum. Its `resubmit` method is not exported as
an API operation. Proposal 14 was unchanged by the failed attempt; no replacement
was created. Do not claim mocked recovery tests establish this capability, call
an invented API route, or silently replace it with reject-and-recreate semantics.

The CLI now rejects this operation offline instead of probing the failing server
operation. Use the web resubmit flow until server/API support is available. Creating a new
proposal with a prerequisite was independently verified with proposals 15/16;
it does not preserve a supersedes chain and is not a substitute for resubmit.

### Converter nullability mismatch — resolved in 0.0.4

The initial converter 0.0.3 snapshot rejected the actual `git_repository-full`
response at `/date_last_repacked`: the live value was `null`, but the schema
declared a non-nullable string.

Upgraded the exact npm pin and regenerated the snapshot with converter **0.0.4**.
Both the previously captured response and a freshly fetched development-server
response now pass `api decode git_repository-full` unchanged, including null
repack dates and reviewer links. The first fresh fetch had a transient connection
failure; a read-only retry and the subsequent decode succeeded. No local schema
patch or relaxed validation was needed. Regression tests cover null values,
redacted counters, rejected wrong types, and required-but-nullable fields.

The regenerated contract has **4,154 operations and 404 schemas**. It also
replaces **3,574 operation IDs** with semantic names instead of numeric route
suffixes. Generic scripts must rediscover these IDs; high-level command names
are unaffected. Provenance now derives the version from the installed converter
and is checked against the manifest and lockfile.

### Permission-denial status

A valid reviewer token could post reviews but could not edit the author's
project. Launchpad returned **401**, not 403. The CLI therefore emitted its
HTTP-401 authentication error and exit **3**. Do not infer that every Launchpad
permission denial will produce exit 4.

## Automated verification

After the follow-up fixes, `npm run check`, `npm test`, and
`cargo build --locked --release` pass: **105 Rust tests** and **9 tooling tests**.
Regression coverage includes development URLs,
instance-isolated Git remotes, named-POST form dispatch, repository collections,
rendered links, offline unsupported-operation rejection, file transport limits,
and temporary Git repository isolation and cleanup.

## Local evidence

Ignored artifacts under `target/dev-integration/` contain `results.jsonl`,
`coverage-summary.json`, per-invocation JSON, scoped job logs, and disposable Git
checkouts. These files are local evidence, not required build inputs. The
private subdirectory and raw signed-download headers must not be committed.
