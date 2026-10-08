# Development instance

This is a mutable Launchpad development installation, not production.

| Component | Address |
| --- | --- |
| Web | `https://launchpad.test/` |
| API | `https://api.launchpad.test/devel` |
| Code | `https://code.launchpad.test/` |
| Bugs | `https://bugs.launchpad.test/` |

## Local connection settings

SSH aliases, checkout paths, backend addresses and service ports belong to the
operator's environment, not this project's shared configuration. Keep SSH routing
in a local SSH config and any checkout/backup locations in
`.development-instance.local.md` at the project root (git-ignored). The CLI does
not read that notes file. Ask the instance operator for current settings if it
is absent; do not assume a particular machine or directory layout.

Discover sessions rather than copying a session ID from an earlier run. Replace
`YOUR_DEV_SSH_ALIAS` with your configured SSH alias:

```sh
ssh YOUR_DEV_SSH_ALIAS screen -list
```

Read the remote checkout's `AGENTS.md` before changing Launchpad itself. Do not
restart the application or inject commands into its screen session merely to
run CLI tests.

## CLI configuration

```sh
export LAUNCHPAD_CLI_INSTANCE=development
# Avoid an inherited API override selecting a different server:
unset LAUNCHPAD_CLI_API_BASE
launchpad-cli auth status --json
launchpad-cli auth login --start --json
# Authorise using the returned development URL, then:
launchpad-cli auth login --finish --json
```

The development profile maps API, OAuth, Git and diff URLs to `.test` hosts.
Credentials are isolated by API origin. For disposable integration accounts,
set `LAUNCHPAD_CLI_CREDENTIALS` to a private file outside tracked source instead
of replacing an existing profile. OAuth still requires valid HTTPS; never turn
off certificate checks to make a dev test pass.

Prefer CLI/API creation of projects, repositories, bugs and proposals. Use a
unique `cli-smoke-<date>` prefix, public test-only content, and disposable Git
working directories. Database access is available for administrative test setup,
but take a backup before schema changes and scope mutations to test resources.
Do not run `make schema` against this instance: it resets the database.

## Git access

Discover the Turnip SSH listener port from the instance's service configuration.
The workstation and development server may resolve `git.launchpad.test`
differently. Resolve the backend hostname from the machine that connects to it,
rather than pinning a container's current IP address:

```sh
ssh YOUR_DEV_SSH_ALIAS getent ahosts git.launchpad.test
```

The following local SSH config resolves the Git hostname on the development
server via `nc`. Replace the `YOUR_*` placeholders and key paths with your local
settings; this is a template, not a ready-to-use configuration. If that server's
DNS does not resolve the Turnip backend correctly, ask the operator for its
service hostname and set `HostName` in your local SSH config.

```sshconfig
Host git.launchpad.test
    Port YOUR_TURNIP_SSH_PORT
    User YOUR_DEVELOPMENT_USERNAME
    IdentityFile /absolute/path/to/dedicated-dev-key
    IdentitiesOnly yes
    BatchMode yes
    StrictHostKeyChecking yes
    UserKnownHostsFile /absolute/path/to/dev-known-hosts
    ProxyCommand ssh -o BatchMode=yes YOUR_DEV_SSH_ALIAS nc %h %p
```

Point `GIT_SSH_COMMAND` at this configuration. Register only its public key on
the test identity. Verify the Turnip host key over the trusted administrative
SSH connection before pinning it; do not disable host-key checking or overwrite
an existing conflicting key. Locate the host key from the running Turnip
service's configuration; do not assume a checkout path or a particular test key.
Old Turnip versions may require `+ssh-rsa` scoped to this host only.

The CLI checks Git remotes against the selected API instance. A development
profile cannot push to `git.launchpad.net`. Repository file reads default to
anonymous HTTPS. Use `repository file ... --transport ssh` to use this SSH
configuration and Git credentials instead. Neither file transport uses OAuth;
HTTPS never silently switches to authenticated SSH.

## Background jobs and downloads

Check which services and workers are running; a web application session does not
imply that Git indexing and preview workers are active. Git pushes can succeed
before the API has indexed refs; proposal creation can succeed while its preview
remains pending. The CLI reports these states rather than claiming the operation
failed.

For live tests, use Launchpad's `JobRunner` with pending `GitRefScanJob` and
`UpdatePreviewDiffJob` instances filtered to the newly created test repository.
Do not indiscriminately drain unrelated jobs or edit GitRef rows to fake indexing.
`--wait-for-index` and `--wait-for-preview` need a worker to actually make progress.

Two environment limitations were observed during the dated integration run;
recheck them rather than assuming they apply to every installation:

- Git HTTPS has certificate/routing problems from the workstation. SSH checkout,
  push and ref checks work; anonymous HTTPS file reads remain blocked, but
  `repository file --transport ssh` now succeeds without weakening TLS.
- Signed preview-download URLs use an unconfigured restricted-librarian hostname.
  The integration run made **only its public test fixture's diff aliases** public
  to exercise real diff downloads without disabling TLS or changing the server.
  This is a fixture workaround, not a fix for restricted/private file hosting.

See [the integration report](dev-validation-2026-10-07.md) for exact changes and
remaining contract/server issues.
