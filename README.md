# launchpad-cli

A standalone Rust CLI for Launchpad, with copied and adapted operations from
[`omp-launchpad`](https://github.com/goulinkh/omp-launchpad). It imports neither
that project nor `lpcli`, and needs no OMP, Bun, Node.js, or Python at runtime.
Its own HTTP client implements Launchpad OAuth, JSON PATCH, repeated form
parameters, created-resource locations, and checked hypermedia links.

The noun–verb layout is familiar to users of tools such as `gh`, but uses
Launchpad's domain model, not GitHub's: **bugs**, target-specific **bug tasks**,
**projects**, **Git repositories**, and **merge proposals**. Repository identities
retain `~owner/project/+git/name`, distributions and source packages; merge
prerequisites, preview-diff snapshots, and review queue statuses remain native
Launchpad concepts. There are no `issue` or `pr` aliases.

## Install

Rust 1.88 or newer is required to build:

```sh
cd launchpad-cli
cargo install --locked --path .
launchpad-cli --help
```

This directory is a self-contained project. Move or copy it elsewhere without
its parent checkout. `package.json` is a private, **development-only** package
for OpenAPI generation, not a JavaScript CLI wrapper.

## Quick start

```sh
launchpad-cli bug view 1
launchpad-cli bug search ubuntu --query 'crash' --status New --limit 5
launchpad-cli repository view launchpad
launchpad-cli repository file launchpad --path README --branch master
launchpad-cli project view launchpad
launchpad-cli merge-proposal list launchpad --status 'Needs review' --limit 5
launchpad-cli merge-proposal for-branch --repository launchpad-ui --branch feature
launchpad-cli merge-proposal current
launchpad-cli merge-proposal discussion 'lp://~owner/project/+git/repo/+merge/123'
launchpad-cli merge-proposal diff 'lp://~owner/project/+git/repo/+merge/123'
```

Proposal lookup may use a numeric ID, a canonical Launchpad URL, or an `lp://`
identifier. Full canonical targets are preferable for unambiguous lookup.
The `current` command infers the source repository and ref from local Git.
A discussion is a compact review summary by default; `--format structured`
puts the complete discussion in `data.text`, with structured fields also in
`data.details`.

Bugs and bug tasks are separate resources. Editing a bug's title does not
change the status of its Ubuntu or project task:

```sh
launchpad-cli bug edit 123 --title 'Clarify the failure' --dry-run
launchpad-cli bug edit 123 --title 'Clarify the failure' --yes
launchpad-cli bug-task edit 'lp://ubuntu/+bug/123' --status Confirmed --yes
launchpad-cli merge-proposal create \
  --repository '~owner/project/+git/repo' \
  --source-ref feature --target-repository project --target-ref master \
  --commit-message 'Fix the failure' --dry-run
```

Use real resource identifiers and explicit authorisation for writes. Local
checkout and Git push also require `--yes`. `--dry-run` validates and prints
input **without network calls, credentials loading, or Git activity**; it does
not promise that a remote resource exists or that your account may modify it.
Merge prerequisite replacement retains the copied recovery behaviour; proposal
creation retains index/preview waits and duplicate-creation recovery.

## Agents and automation

```sh
launchpad-cli schema --json
launchpad-cli merge-proposal review --help
printf '%s' '{"target":"lp://~owner/project/+git/repo/+merge/123","preview_diff_id":456}' |
  launchpad-cli merge-proposal inline-comments --input - --json
printf '%s' '{"op":"resource_view","target":"lp://bugs/1?comments=0"}' |
  launchpad-cli tool --input - --json
```

`schema` is offline and exposes the command catalog, allowed fields, required
fields, effect classifications, and a Rust-derived JSON input schema. All **31
original tool operation names** are callable through `tool --input`. Noun–verb
commands use the same validation and execution code, but omit `op` from their
JSON input. Flags use hyphens; JSON keys use underscores. Boolean flags accept
`--latest` or `--latest=false`; list filters may be repeated. Use JSON arrays
when clearing tags (`"tags": []`). Unknown fields and fields supplied through
both JSON and flags are rejected.

Output defaults to text on a terminal and JSON when piped. Explicit `--json`
or `--text` overrides that choice. JSON is one object per invocation:

```json
{"schema_version":1,"ok":true,"data":{"text":"…","source_url":null,"details":{}}}
```

```json
{"schema_version":1,"ok":false,"error":{"code":"not_authenticated","message":"Launchpad credentials are missing"}}
```

High-level reads include Markdown plus structured metadata; bug, repository,
and search reads also include their raw resources in `data.details`. `api call`
returns the HTTP status, Location header, and untouched JSON body. There are no
colour codes, progress spinners, browser launches, or confirmation prompts in
machine workflows. Interactive `auth login` is the only command that waits for
user input; use its split flow for automation. Help/version output remains text.

Exit codes: `0` success; `1` transport/runtime failure; `2` invalid input or
missing `--yes`; `3` authentication required/rejected; `4` permission denied;
`5` resource not found. Diagnostics never mix with successful JSON on stdout.

## Authentication and instances

```sh
launchpad-cli auth login
launchpad-cli auth status
launchpad-cli auth logout
# No terminal required; the user still authorises in a browser:
launchpad-cli auth login --start --json
launchpad-cli auth login --finish --json
```

OAuth uses consumer key `launchpad-cli` and Launchpad's PLAINTEXT-over-TLS
signing protocol. Credentials are JSON in the platform config directory under
`launchpad-cli/`, isolated by API origin, and created with mode `0600` on Unix.
They are **not** read from or written to `lpcli` or OMP credential storage.
`auth status` checks local storage, not remote validity (`verified: false`).
For an existing token, `auth import --input FILE|-` accepts:

```json
{"consumer_key":"launchpad-cli","token":"YOUR_TOKEN","secret":"YOUR_SECRET"}
```

Avoid putting secrets in command-line arguments. Protect input files and, on
Windows, the config directory with appropriate user-only ACLs.

| Environment variable | Purpose |
| --- | --- |
| `LAUNCHPAD_CLI_INSTANCE` | `production` (default), `staging`, or `qastaging` |
| `LAUNCHPAD_CLI_API_BASE` | Complete API base override for a local server or other version |
| `LAUNCHPAD_CLI_ANONYMOUS=1` | Force anonymous reads; reject authenticated operations |
| `LAUNCHPAD_CLI_CREDENTIALS` | Explicit credentials-file override |

Authenticated API requests require HTTPS. API hypermedia and Location links
must stay within the configured origin and version; redirects are not followed
with OAuth. Writes are never automatically retried. Local Git workflows check
Launchpad Git remote hosts and use Git's own authentication, not API OAuth.
Checkouts default to `~/.launchpad-cli/checkouts/`, or use `--directory`.
Repository file reads are anonymous Git HTTP on the selected official
Launchpad instance; private files need an authenticated Git checkout. A custom
API host cannot silently fall back to production Git hosting.

## OpenAPI authority and generated Rust types

The [Launchpad OpenAPI converter](https://code.launchpad.net/~launchpad-committers/launchpad-openapi/+git/launchpad-openapi),
npm package **`@canonical/launchpad-openapi`**, owns response schemas,
nullability, route coverage, and semantic operation IDs. We do not parse WADL,
maintain a competing route catalog, or infer missing field types.

The converter's compressed, unmodified output is committed at
`openapi/launchpad.json.gz`; `openapi/provenance.json` records its package
version, servers, and SHA-256. `src/generated.rs` contains all component response
types generated from that snapshot. Required fields stay required, nullable
fields remain nullable, missing optional fields remain distinct from explicit
null, enums retain exact Launchpad values, and unconstrained fields remain
`serde_json::Value`. Aggregate tool workflows retain their copied dynamic JSON
projections rather than imposing extra schema assumptions on partial resources.

Discover and call converter-owned operations, including alternative routes:

```sh
launchpad-cli api operations --filter getByPath
launchpad-cli api schema git_ref-full
printf '%s' '{"params":{"path":"launchpad"}}' |
  launchpad-cli api call git_repositories-getByPath --input - --json
printf '%s' '{"start":0,"entries":[]}' |
  launchpad-cli api decode git_ref-page --input - --json
```

`api call` takes `{"params":{…},"body":{…}}`. Path/query fields and body media
types come from the selected operation. Required parameters and declared scalar
and enum types are checked. `api decode` uses the generated Rust response types
for strict decoding; it does not silently repair incomplete upstream schemas.
API writes require `--yes`; `--dry-run` prints the resolved request. Binary and
multipart bodies are not supported by this generic JSON interface; use dedicated
tools where available. Unknown operation IDs fail, rather than inventing routes.

Regeneration needs Node.js 24+ but ordinary Cargo builds are offline with respect
to schema generation:

```sh
npm ci
npm run openapi:fetch   # invokes the pinned Canonical converter
npm run generate       # creates Rust types from its exact output
npm run generate:check # detects stale types or altered provenance
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for verification. Code provenance and licence
information are in [NOTICE.md](NOTICE.md).
