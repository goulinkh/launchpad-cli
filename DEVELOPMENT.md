# Development

Run from this directory. Rust 1.88+ builds the standalone binary; Node.js 24+
is needed only for converter updates and generator checks. No OMP, parent crate,
`lpcli`, or parent `node_modules` is required.

```sh
npm ci
npm run check       # generated-code freshness, rustfmt, strict Clippy
npm test           # generator, copied workflow, transport, and CLI tests
cargo build --locked --release
```

For Rust-only work, `cargo fmt --check`, `cargo clippy --locked --all-targets --
-D warnings`, and `cargo test --locked` do not invoke npm or fetch Launchpad's
schema. Committed `Cargo.lock` and `package-lock.json` make dependencies
reproducible. Generated artifacts live inside this project and must not refer
back to its parent. Keep this directory outside a parent Cargo workspace.

## Updating the API contract

```sh
npm run openapi:fetch
npm run generate
npm run check
npm test
```

`openapi:fetch` invokes the pinned `@canonical/launchpad-openapi` CLI and
compresses its exact JSON output. It records a SHA-256 of the uncompressed
bytes, with converter version and servers. Do not hand-edit the snapshot,
provenance, or generated types. Changes to response semantics, nullable fields,
canonical routes, alternatives, or operation IDs belong in the Canonical
converter, not here. Review contract changes before committing regenerated
artifacts; `devel` can change without warning.

The generator intentionally fails on unsupported schema constructs instead of
guessing. Unknown schemas remain `serde_json::Value`. Generated types are used
by `api decode`; high-level multi-resource workflows retain the original JSON
aggregation semantics. `api call` reads routes and operation IDs directly from
the embedded converter document, including `x-launchpad-route-alternatives`.
A route absent from the converter is not synthesised locally.

## Verification without production writes

The Rust test suite uses local HTTP fixtures and subprocesses. It verifies
command coverage for every original operation, repeated filters, argument and
JSON validation, write consent, dry runs, authentication/permission/not-found
exit codes, generated decoding, API operation invocation, diff mapping, review
drafts, creation recovery, and prerequisite-replacement recovery.

Use anonymous production reads only for optional smoke tests:

```sh
LAUNCHPAD_CLI_ANONYMOUS=1 ./target/release/launchpad-cli bug view 1 --text
printf '%s' '{"params":{"path":"launchpad"}}' |
  LAUNCHPAD_CLI_ANONYMOUS=1 ./target/release/launchpad-cli \
    api call git_repositories-getByPath --input - --json
```

Never mutate production merely to prove wiring. Use mock fixtures, an
expendable resource, or a non-production instance for authorised write testing:

```sh
export LAUNCHPAD_CLI_INSTANCE=staging
./target/release/launchpad-cli auth login
```

`LAUNCHPAD_CLI_API_BASE` selects a local fixture server. Anonymous HTTP fixtures
are allowed; OAuth is never sent over HTTP. Git workflows use actual Git and
require `--yes`; tests must use disposable directories and must not push to a
production branch. `--dry-run` is entirely offline.

## Git

Use Conventional Commits: `feat`, `fix`, `refactor`, `perf`, `style`, `test`,
`docs`, `build`, `ops`, or `chore`. Descriptions are imperative, lowercase, and
have no trailing period. Mark breaking changes with `!` and a `BREAKING CHANGE:`
footer. Treat JSON envelope changes and CLI operation removal as interface
changes.
