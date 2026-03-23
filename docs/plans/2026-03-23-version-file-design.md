# VERSION File Versioning Design

**Date:** 2026-03-23

**Goal:** Use a checked-in `VERSION` file as the single source of truth for the user-visible release version.

## Decision

- Add a root-level `VERSION` file.
- Store the full release string in the file, for example `v0.260323.1`.
- Make the GitHub release workflow read `VERSION` directly instead of composing a tag from `Cargo.toml`, the current date, and `github.run_id`.
- Make the CLI print the same `VERSION` value for `--version` and `-v`.
- Keep `Cargo.toml` package version as Rust crate metadata for now.

## Consequences

- Release tagging and CLI version output become consistent.
- Version bumps become explicit repository changes instead of CI-derived values.
- Docs and tests need to describe `VERSION` as the release source of truth.
