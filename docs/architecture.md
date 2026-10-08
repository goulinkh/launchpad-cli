# Architecture

## Two CLI surfaces, one safety boundary

The high-level noun–verb commands perform Launchpad workflows: a review can
combine a proposal, preview diff, comments, people, and local Git state. The
`api` commands expose individual operations from Canonical's OpenAPI document.
Neither surface should invent routes or silently retry writes. Named POST
operations copy the converter's fixed `ws.op` selector into form data, which is
where Launchpad dispatches POST operations.

An agent's API workflow is:

1. `api operations --compact --filter TEXT`: find operation IDs cheaply.
2. `api describe ID`: inspect the input schema, responses, and write policy.
3. `api call ID --input FILE|- --dry-run`: validate and inspect the actual request.
4. `api call ID --input FILE|-`: execute; add `--yes` only for authorised writes.

JSON output retains the versioned success/error envelope. Schema errors expose
JSON pointers under `error.details`; broken embedded contracts are runtime
errors (`api_contract_error`), not errors attributed to the user's input.

## Code map

| Location | Responsibility |
| --- | --- |
| `src/main.rs` | Dispatch, process exit codes, output envelope |
| `src/cli.rs`, `src/commands.rs` | Command definitions, input loading, workflow catalog |
| `src/api/command.rs` | API command dispatch |
| `src/api/contract.rs` | Typed document loading, effective parameters, Launchpad route alternatives |
| `src/api/validation.rs` | Reachable-component bundling, library-backed dialect conversion and validation |
| `src/api/plan.rs` | Offline request schema, URL/body preparation, supported encoding policy |
| `src/api/execute.rs` | Consent check, credential loading, prepared-request execution |
| `src/client.rs`, `src/auth.rs` | HTTP, origin checks, OAuth and credential storage |
| `src/request.rs`, `src/launchpad.rs` | High-level workflow validation and orchestration |
| `src/render.rs`, `src/diff.rs`, `src/local_git.rs` | Presentation, diff mapping, local Git workflows |
| `src/git_file.rs` | Explicit SSH file reads using a disposable shallow Git fetch |

`RequestPlan` is prepared before credentials are loaded. Dry runs and execution
share this path, including form encoding checks. The transport checks the
configured origin/version and never forwards OAuth over HTTP or redirects.

## Standards are dependencies, not local implementations

- `openapiv3` deserialises the OpenAPI 3.0 document into typed operations,
  parameters, media types, and schemas.
- `openapiv3-resolve` resolves path-item, parameter, and request-body references,
  including reference chains and cycle detection.
- `openapi-schema-to-json-schema` adapts the OpenAPI schema dialect to draft-04.
  Request schemas omit read-only fields; response schemas omit write-only
  fields. Missing values are not replaced by schema defaults.
- `jsonschema` evaluates types, required fields, enums, constraints, composition,
  and recursive schema references. File/HTTP reference retrieval is disabled.

A small dependency walk bundles only reachable component schemas for an
operation. It preserves references rather than recursively inlining them, and
ignores example/default data. The validator, not this walk, evaluates references.
`api schema` returns the original document; adapted schemas are derived views.
No generated Rust domain models are needed solely to validate JSON.

The application-specific work is interpreting `x-launchpad-route-alternatives`,
selecting the configured instance, enforcing consent, and encoding the supported
Launchpad requests. Generic HTTP execution deliberately does not promise every
OpenAPI transport feature. Unsupported locations/styles/media types fail rather
than falling back to a guessed request. Schema references must target local
component schemas. `format` validation is disabled; the dialect adapter still
translates numeric and byte formats. This is OpenAPI 3.0 support, not 3.1 support.

## Making changes

For contract changes, update the converter snapshot and provenance together.
Run `npm run openapi:check` and `cargo test --locked`. Do not repair an upstream
schema in a workflow or create a parallel route table.

For an encoding extension, change the planner, describe its policy, and add
fixtures that check both valid wire encoding and rejected input. Keep network
access out of contract, validation, and planning code. Use local HTTP fixtures
for transport tests; never use production mutations for verification.

## Remaining cleanup

The inherited high-level workflow layer remains large. `launchpad.rs` combines
lookup, review, creation/recovery, and editing; `Request` remains a broad legacy
operation envelope. These were intentionally not rewritten along with the API
foundation. Follow-up work should split workflows by domain and introduce
operation-specific request types while retaining the 30 compatibility operation
names and supported creation/recovery tests. Live testing established that
Launchpad does not accept `setStatus(Superseded)` and does not export resubmit.
The unsupported supersede/create/rollback implementation and its misleading
success mocks have been removed. Prerequisite replacement is listed as unsupported
in discovery and rejected offline, including on dry runs; use the web resubmit
flow. No alternative routes or reject-and-recreate semantics are fabricated.

High-level results still contain rendered text as well
as structured details; generic API calls already return untouched JSON bodies.

Automatic pagination, field projection, and response-size budgets are separate
agent-interface work. Generic API calls currently execute exactly one request.
Do not introduce unbounded page traversal or automatic mutation retries as part
of a readability refactor.
