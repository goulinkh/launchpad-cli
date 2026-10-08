# Provenance and licences

- `src/diff.rs`, `src/launchpad.rs`, `src/local_git.rs`, `src/render.rs`,
  `src/request.rs`, `src/error.rs`, `src/response.rs`, and `src/result.rs` began
  as physical copies from `omp-launchpad` and were adapted locally. The original
  declares GPL-3.0-or-later: <https://github.com/goulinkh/omp-launchpad>.
  This project uses the same licence; its full text is in `LICENSE`.
- `src/client.rs`, `src/auth.rs`, `src/git_file.rs`, the command interface, API adapter, and standalone
  build configuration are local implementations. There is no import, Git Cargo
  dependency, subprocess invocation, or credential-store dependency on `lpcli`
  or `omp-launchpad`.
- `@canonical/launchpad-openapi` is a generation-time dependency, pinned in
  `package-lock.json`. Copyright 2026 Canonical Ltd.; LGPL-3.0-only. Its licence
  is reproduced in `openapi/CONVERTER-LICENSE`. The converter's code is not
  vendored. Its output is stored at `openapi/launchpad.json.gz`, with provenance
  in `openapi/provenance.json`. The runtime reads this document with third-party
  OpenAPI and JSON Schema libraries; their licences are declared in their Cargo
  packages.
- No `gh` source code, command aliases, or GitHub resource model is copied.
