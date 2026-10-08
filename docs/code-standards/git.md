# Git Standards

Standards for git development.

## cs:git.branch.name

Branch names must follow a semantic prefix/description pattern using `/` as separator. The prefix communicates the intent of the branch (feature, fix, chore, etc.) and the description uses kebab-case to summarize the work. This convention enables CI filtering, changelog grouping, and at-a-glance understanding of what a branch carries.

---

## cs:git.commit.message

Commits that land on the main branch must follow the Conventional Commits specification. The format is `type(scope): description` where the type communicates the nature of the change, the optional scope narrows it to a package or module, and the description completes the sentence 'this commit will...'. This enables automated changelog generation, semantic version bumping, and scannable git history.

---

## cs:git.commit.scope

The scope in a conventional commit should identify the package, module, or domain affected by the change. In a monorepo, the scope is typically the package name (without the namespace prefix). In a single-package repo, it is the module or feature area. Consistent scopes make `git log --grep` useful and enable per-package changelogs.

---

## cs:git.merge-proposal.description

Write a brief, reviewer-facing summary of the problem and intended outcome. Leave implementation details in the diff. If useful, add a short note on how to verify the change.

---

## cs:git.tag.versioning

Release tags must follow semantic versioning prefixed with `v` (e.g. `v1.2.3`). Tags must be annotated (not lightweight) so they carry metadata for tooling. Pre-release versions use a hyphenated identifier after the patch number. Automated tooling (Lerna, changesets, etc.) should derive version bumps from conventional commit types: `feat` → minor, `fix` → patch, `BREAKING CHANGE` → major.

---

## cs.git.commit.context

In case of coding harness usage, the harness should include the relative information, considerations, decisions, and scope of spec used to work on the commit.
