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

Native archives are also published to
[GitHub Releases](https://github.com/goulinkh/launchpad-cli/releases) when a
version tag is pushed. Linux (musl), macOS, and Windows builds are available
for x64 and arm64, with `SHA256SUMS`. Extract the archive and put
`launchpad-cli` (`launchpad-cli.exe` on Windows) on your `PATH`. Access to
releases follows the repository's visibility; private releases require GitHub
authentication. Node.js and Rust are not needed to run a downloaded binary.

This directory is a self-contained project. Move or copy it elsewhere without
its parent checkout. `package.json` is a private, **development-only** package
for OpenAPI snapshot updates and release tooling, not a JavaScript CLI wrapper.

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
Proposal creation retains index/preview waits and duplicate-creation recovery.
Prerequisite replacement is not exported by Launchpad's API. The compatibility
command reports `unsupported_operation` before loading credentials or sending
requests, including on dry runs; `schema` lists it as unsupported. Use the web
resubmit flow instead. Creating a new proposal with a prerequisite works, but
does not establish a supersedes relationship.

## Agents and automation

```sh
launchpad-cli schema --json
launchpad-cli api operations --compact --filter getByPath
launchpad-cli api describe git_repositories-getByPath
launchpad-cli merge-proposal review --help
printf '%s' '{"target":"lp://~owner/project/+git/repo/+merge/123","preview_diff_id":456}' |
  launchpad-cli merge-proposal inline-comments --input - --json
printf '%s' '{"op":"resource_view","target":"lp://bugs/1?comments=0"}' |
  launchpad-cli tool --input - --json
```

`schema` is offline and exposes the command catalog, allowed fields, required
fields, effect classifications, and a Rust-derived JSON input schema. All **30
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

For API tasks, discover a compact operation list, describe the exact operation,
then validate the input with `api call ... --dry-run` before executing it.
`api describe` returns a self-contained JSON Schema input contract, response
definitions, side-effect classification, and whether `--yes` is required.
Unsupported request encodings are reported explicitly. Schema validation errors
include `error.details.instance_path` and `error.details.schema_path` JSON
pointers so an agent can locate the invalid field without parsing prose.
Descriptions and schemas are offline; defaults are not silently inserted.

Exit codes: `0` success; `1` transport/runtime failure; `2` invalid input or
missing `--yes` or unsupported operation; `3` authentication required/rejected
or HTTP 401 access denial; `4` HTTP 403 permission denied; `5` resource not found.
Launchpad sometimes returns 401 for insufficient permissions even with valid
credentials. Its diagnostic reflects this ambiguity; API 401/403 errors also
expose `error.details.http_status`. Diagnostics never mix with successful JSON
on stdout.

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
| `LAUNCHPAD_CLI_INSTANCE` | `production` (default), `staging`, `qastaging`, or `development` (`launchpad.test`) |
| `LAUNCHPAD_CLI_API_BASE` | Complete API base override for a local server or other version |
| `LAUNCHPAD_CLI_ANONYMOUS=1` | Force anonymous API reads; reject authenticated API operations |
| `LAUNCHPAD_CLI_CREDENTIALS` | Explicit credentials-file override |

Authenticated API requests require HTTPS. API hypermedia and Location links
must stay within the configured origin and version; redirects are not followed
with OAuth. Writes are never automatically retried. Local Git workflows check
Launchpad Git remote hosts and use Git's own authentication, not API OAuth.
Git remotes must match the selected instance; development runs cannot push to
production. Index checks and checkouts can fall back from HTTPS to SSH.
Checkouts default to `~/.launchpad-cli/checkouts/`, or use `--directory`.
Repository file reads default to anonymous HTTPS. To use Git's SSH credentials
for private files or an instance without working Git HTTPS, explicitly select SSH:

```sh
launchpad-cli repository file '~owner/project/+git/repo' \
  --path README.md --branch main --transport ssh
```

SSH mode shallow-fetches the selected ref into a disposable bare repository,
reads committed blob bytes without a checkout or filters, and removes it on
success or failure. It does not change an existing working tree or fall back
silently between transports. Omitting `--branch` uses the remote's `HEAD`.
The result includes the resolved commit SHA. SSH uses the user's SSH config
and `GIT_SSH_COMMAND`, not API OAuth, even with anonymous API mode enabled.

Both modes accept only UTF-8 files up to 2 MiB. HTTPS reads enforce the limit
while streaming, time out after 30 seconds, and follow redirects only within
the same Git origin and plain-file routes. SSH reads time out after 120 seconds;
a shallow fetch can still transfer more data than the requested file. A custom
API host cannot silently fall back to production Git hosting.

The development server at `https://launchpad.test/` is supported with
`LAUNCHPAD_CLI_INSTANCE=development`. See
[Development instance](docs/development-instance.md) for SSH access, services,
and known environment limitations.

## OpenAPI authority and runtime validation

The [Launchpad OpenAPI converter](https://code.launchpad.net/~launchpad-committers/launchpad-openapi/+git/launchpad-openapi),
npm package **`@canonical/launchpad-openapi`** (pinned to **0.0.4**), owns response
schemas, nullability, route coverage, and semantic operation IDs. We do not parse WADL,
maintain a competing route catalog, or infer missing field types.

Converter 0.0.4 replaces numeric `-route-N` operation suffixes with semantic
labels. Generic API scripts using old IDs must rediscover them with
`api operations --compact --filter TEXT`; there are no compatibility aliases.
High-level noun–verb commands and their legacy tool names are unchanged.

The converter's compressed, unmodified output is committed at
`openapi/launchpad.json.gz`; `openapi/provenance.json` records its package
version, servers, and SHA-256. OpenAPI 3.0 parsing uses `openapiv3`, reference
resolution uses `openapiv3-resolve`, and validation uses `jsonschema` with
`openapi-schema-to-json-schema` for dialect conversion. There is no custom
OpenAPI parser, schema validator, or Rust model generator. Only Launchpad's
route-alternative extension and CLI request-encoding policy are handled locally.

Required fields, nullable values, enums, bounds, patterns, composition, and
recursive component references are validated without coercing the input.
Unknown fields follow the schema's `additionalProperties` policy. `format`
strings remain annotations (numeric/byte formats are converted by the dialect
adapter). Validation never fetches external references. Aggregate tool workflows
retain their dynamic JSON projections instead of applying full-resource schemas
to partial responses.

Discover and call converter-owned operations, including alternative routes:

```sh
launchpad-cli api operations --compact --filter getByPath
launchpad-cli api describe git_repositories-getByPath
launchpad-cli api schema git_ref-full
printf '%s' '{"params":{"path":"launchpad"}}' |
  launchpad-cli api call git_repositories-getByPath --input - --json
printf '%s' '{"start":0,"entries":[]}' |
  launchpad-cli api decode git_ref-page --input - --json
```

`api call` takes `{"params":{…},"body":{…}}`. Path/query fields and body media
types come from the selected operation, including inherited parameters and
component references. `api decode` validates against a component schema and
returns the original JSON unchanged; it does not repair incomplete schemas.
`api schema` always returns the original converter document or component.

Named POST operations send the converter's fixed `ws.op` selector in form data,
as required by Launchpad. Callers cannot override that selector.

API writes require `--yes`; `--dry-run` runs the same validation and encoding as
execution, then prints the request with `executed: false`. Supported encodings
are scalar simple-path parameters, form-style scalar/array query parameters,
JSON bodies, and scalar/repeated-field URL-encoded forms. Header/cookie
parameters, other styles, custom body encodings, binary/multipart bodies, and
external schema references fail explicitly. Use dedicated tools where available.
Unknown or ambiguous operation IDs fail rather than inventing routes. The
configured instance determines the API base, never a per-operation server URL.

Snapshot updates need Node.js 24+; Cargo builds do not run a generator or fetch
Launchpad's schema:

```sh
npm ci
npm run openapi:fetch   # invokes the pinned Canonical converter
npm run openapi:check  # verifies snapshot provenance
cargo test --locked   # checks runtime parsing and schema validation
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for verification. Code provenance and licence
information are in [NOTICE.md](NOTICE.md).
