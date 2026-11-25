# Maintainers

## Active Maintainers

| Maintainer | Role | Focus Areas |
| --- | --- | --- |
| `@countradooku` | Project creator and maintainer | Core architecture, store protocols, runtime management, releases, packaging |
| `@EdwardTheBanker` | Maintainer | UI and UX, account and sign-in flows, governance, documentation, issue triage |

## Review and Merge Policy

All changes land on `main` through pull requests. Direct pushes to `main` are disallowed.

Before merging a pull request:

1. Required automated checks (CI builds, tests, formatting, clippy, and `.github/scripts/pr_rules.py`) must pass.
2. At least one maintainer other than the pull request author must review and approve the change.
3. Changes touching sensitive areas require careful review of failure modes, input validation, and licensing:
   - [`crates/secrets`](crates/secrets/src/lib.rs) and [`crates/sign-in`](crates/sign-in/src/lib.rs)
   - [`crates/scripts`](crates/scripts/src/lib.rs) and [`crates/core/src/fs_guard.rs`](crates/core/src/fs_guard.rs)
   - `LICENSE` and dependency license policies
   - `.github/` workflows and release automation

## Decision Making

- Architectural changes, scope additions or removals, governance updates, and licensing changes are discussed openly in GitHub issues or pull requests before implementation.
- Decisions require consensus among active maintainers. When a change is out of scope for Irori, maintainers close the issue with a clear rationale so contributors do not spend time on rejected work.

## Becoming a Triager or Maintainer

Contributors do not need permission to help review pull requests, test bug reports on different systems, or improve documentation.

### Triage Role

Regular contributors who consistently help reproduce bugs, verify fixes, and keep issues actionable may be invited to join the triage team.

### Maintainer Role

Maintainer status is granted by consensus of existing active maintainers to contributors with a sustained record of well-reviewed pull requests, thoughtful code reviews, and alignment with the project's technical standards and scope.

## Emeritus Status

Maintainers who step back from active duties remain listed as emeritus maintainers out of respect for their work on Irori, and may return to active status by mutual agreement with the current maintainers.
