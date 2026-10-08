# Development

Run from this directory. Rust 1.88+ builds the standalone binary; Node.js 24+
is needed for the npm launcher, converter updates, provenance checks, and
release packaging. Direct native binaries do not require Node.js.
No OMP, parent crate, `lpcli`, or parent `node_modules` is required.

```sh
npm ci
npm run check       # snapshot provenance, rustfmt, strict Clippy
npm test           # release, contract, workflow, transport, and CLI tests
cargo build --locked --release
```

For Rust-only work, `cargo fmt --check`, `cargo clippy --locked --all-targets --
-D warnings`, and `cargo test --locked` do not invoke npm or fetch Launchpad's
schema. Committed `Cargo.lock` and `package-lock.json` make dependencies
reproducible. Generated artifacts live inside this project and must not refer
back to its parent. Keep this directory outside a parent Cargo workspace.

## Updating the API contract

```sh
# When upgrading the converter, resolve the latest release to an exact pin:
npm install --save-dev --save-exact @canonical/launchpad-openapi@latest
npm run openapi:fetch
npm run check
npm test
```

`openapi:fetch` invokes the pinned `@canonical/launchpad-openapi` CLI and
compresses its exact JSON output. It records a SHA-256 of the uncompressed
bytes, with the installed converter's version and servers. Generation refuses
an installed version that differs from the exact manifest pin; checking also
compares provenance with the manifest and lockfile. Operation IDs may change
between converter releases, so review generic API scripts as well as schemas.
Do not hand-edit the snapshot or
provenance. Changes to response semantics, nullable fields, canonical routes,
alternatives, or operation IDs belong in the Canonical
converter, not here. Review contract changes before committing the snapshot;
`devel` can change without warning.

`src/api/` owns the typed contract boundary, schema validation, offline request
planning, and generic operation execution. It uses `openapiv3`,
`openapiv3-resolve`, `openapi-schema-to-json-schema`, and `jsonschema` rather than
a custom parser, validator, or model generator. Tests parse the entire embedded
snapshot and compile every component schema. The JSON Schema dependency has
file and HTTP retrieval disabled; keep validation and discovery offline.

`api decode` validates the component schema and preserves the original JSON.
High-level multi-resource workflows retain their JSON aggregation semantics.
`api call` reads routes and operation IDs from the embedded converter document,
including `x-launchpad-route-alternatives`. A route absent from the converter is
not synthesised locally. Unsupported serialisation fails before authentication
or network access. `api describe` exposes the input schema and write policy for
one operation; `api operations --compact` avoids dumping full definitions.

See [docs/architecture.md](docs/architecture.md) for boundaries, extension points,
and remaining workflow refactoring work.

## Development instance

Use `LAUNCHPAD_CLI_INSTANCE=development` for a development installation serving
**https://launchpad.test/**. Machine-specific SSH routing belongs in local SSH
configuration; checkout and backup locations belong in the git-ignored
`.development-instance.local.md`. Discover active sessions and backend addresses
from the running environment rather than recording them in shared instructions.

See [docs/development-instance.md](docs/development-instance.md) before using
an instance. [The live integration report](docs/dev-validation-2026-10-07.md) records
coverage, test resources, environment changes, and outstanding failures.
Production remains off limits for mutation testing.

## Verification without production writes

The Rust test suite uses local HTTP fixtures and subprocesses. It verifies
command coverage for every original operation, repeated filters, argument and
JSON validation, write consent, dry runs, authentication/permission/not-found
exit codes, component validation, API operation invocation, diff mapping, review
drafts, creation recovery, and offline rejection of unsupported resubmission.
File-transport tests cover bounded HTTP responses, redirect confinement, SSH
blob reads through a local Git transport fixture, temporary-repository cleanup,
and isolation from inherited Git repository environment variables.

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
require `--yes`; their remotes must match the selected instance. Tests must use
disposable directories and must not push to a production branch. `--dry-run` is entirely offline.

## GitHub and npm publishing

Three workflows are included:

- `.github/workflows/check.yml` verifies main-branch pushes and pull requests.
- `.github/workflows/security.yml` checks Actions security with zizmor without
  requiring paid GitHub Advanced Security features for a private repository.
- `.github/workflows/release.yml` verifies `v*` tags, builds and smoke-tests
  native binaries, assembles `@goulin/launchpad-cli`, and publishes a GitHub
  release only after every build passes. npm registry publishing is a separate,
  authenticated local step.

GitHub publishing uses the workflow's built-in `GITHUB_TOKEN`; no personal access
token, registry credentials, or extra secrets are required for that workflow.
Only the publishing job has `contents: write`. Actions are pinned to commit
SHAs, checkout does not persist credentials, and tagged source passes the full
test suite before packaging.
Keep GitHub Actions enabled and ensure repository or organisation policies allow
these pinned actions and the publishing job's write permission.

Supported release targets:

| Platform | Runner | Rust target | Archive |
| --- | --- | --- | --- |
| Linux x64 | `ubuntu-24.04` | `x86_64-unknown-linux-musl` | `.tar.gz` |
| Linux arm64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-musl` | `.tar.gz` |
| macOS x64 | `macos-15-intel` | `x86_64-apple-darwin` | `.tar.gz` |
| macOS arm64 | `macos-15` | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x64 | `windows-2025` | `x86_64-pc-windows-msvc` | `.zip` |
| Windows arm64 | `windows-11-arm` | `aarch64-pc-windows-msvc` | `.zip` |

Each archive contains the executable, README, NOTICE, and applicable licence
texts. Archives are named `launchpad-cli-<version>-<platform>-<architecture>`;
platform values are `linux`, `darwin`, and `win32`. Packaging checks the binary's
version and offline agent schema without contacting Launchpad. The publish job
requires exactly six nonempty native archives plus the universal npm tarball,
and attaches `SHA256SUMS` for all seven files. Linux builds use musl to avoid
a host glibc dependency. Hosted runner availability and private
repository Actions usage depend on the account's GitHub plan and policies.

To publish the current version after committing the source:

```sh
npm run check
npm test
node scripts/release-metadata.mjs v0.1.2
# No release is created until the tag is pushed:
git tag -a v0.1.2 -m 'launchpad-cli v0.1.2'
git push origin main
git push origin v0.1.2
```

For later releases, update the `[package]` version in `Cargo.toml` and the
version in `package.json`, then run `cargo check` and
`npm install --package-lock-only --ignore-scripts` to update the lockfiles.
Commit the changes and push a matching tag. Mismatched or invalid tags fail;
prerelease versions such as `v0.2.0-rc.1` are marked as GitHub prereleases and do
not become the latest stable release. Existing tags and releases are not
rewritten or replaced by the workflow. Choose a new version instead of reusing
an already published tag.

To package one native target locally:

```sh
cargo build --locked --release --target aarch64-apple-darwin
npm run release:package -- aarch64-apple-darwin
```

Packaging must run on the binary's matching OS and architecture so the smoke
test actually executes it. Native archives go to ignored `dist/`; each verified
executable is also staged under ignored `bin/` with a platform-specific name.
The matrix uploads both, and the publishing job combines all six builds.
Windows ZIP packaging invokes the system's native bsdtar explicitly rather than
Git Bash's GNU tar, which interprets drive-letter paths as remote hosts.

After collecting the current version's six verified binaries in `bin/`, run:

```sh
npm run release:npm
npm run release:checksums
```

`release:npm` produces `dist/goulin-launchpad-cli-<version>.tgz`. The assembler
requires all six nonempty, regular binaries, normalises executable permissions,
and packs a temporary allowlisted tree without development dependencies or
scripts. Corresponding Rust source, lockfile, toolchain and embedded contract
build inputs are included for GPL compliance while the source repository is
private; npm installation never compiles them. The source checkout's `prepack`
guard also refuses incomplete binary sets, preventing an accidental source-only
publication. Tests install a fixture tarball offline with lifecycle scripts
disabled and exercise the launcher.
No Rust compilation, Cargo fallback, install-time download, or Launchpad access
is used by npm consumers.

### First npm publication

The `v0.1.0` build failed at Windows ZIP packaging before publishing any release.
The `v0.1.1` tag still used the unavailable `@goulinkh` npm scope. Both tags remain
unchanged; the first npm release uses `v0.1.2` and **`@goulin/launchpad-cli`**.

The npm scope is independent of GitHub. Authenticate as the npm user **`goulin`**
to publish in that personal namespace; the repository remains under GitHub's
`goulinkh` account. The unscoped `launchpad-cli` name belongs to another project;
do not use it.
npm installs both `launchpad-cli` and the shorter alias `lpci`, backed by the
same launcher and native binary.

After committing and tagging the source as above, wait for the release workflow
to finish. Download its already-built npm tarball and checksum manifest:

```sh
gh release download v0.1.2 --repo goulinkh/launchpad-cli --dir dist \
  --pattern 'goulin-launchpad-cli-0.1.2.tgz' --pattern SHA256SUMS
(cd dist && grep '  goulin-launchpad-cli-0.1.2.tgz$' SHA256SUMS | shasum -a 256 --check)
npm login
npm whoami
npm publish ./dist/goulin-launchpad-cli-0.1.2.tgz --access public --tag latest
npm view @goulin/launchpad-cli version
```

Follow npm's interactive authentication and two-factor prompts; never commit
registry tokens. For prereleases, use `--tag next` rather than `latest`.
Publication exposes the bundled binaries and included documentation publicly,
even though GitHub Releases inherit the repository's visibility (currently
private). Existing npm versions are immutable. This workflow does not publish
to crates.io or a container registry.

## Git

Use Conventional Commits: `feat`, `fix`, `refactor`, `perf`, `style`, `test`,
`docs`, `build`, `ops`, or `chore`. Descriptions are imperative, lowercase, and
have no trailing period. Mark breaking changes with `!` and a `BREAKING CHANGE:`
footer. Treat JSON envelope changes and CLI operation removal as interface
changes.
