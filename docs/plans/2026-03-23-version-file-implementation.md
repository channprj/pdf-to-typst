# VERSION File Versioning Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace CI-generated release suffixes with a checked-in `VERSION` file and expose the same value through the CLI.

**Architecture:** The repository root gains a `VERSION` file containing the full release tag string. The Rust binary embeds that file at compile time for `--version` and `-v`, while the release workflow validates and reuses the same file for Git tags and GitHub Releases.

**Tech Stack:** Rust CLI, GitHub Actions, repository docs, integration tests

---

### Task 1: Add failing CLI regression tests

**Files:**
- Modify: `tests/cli.rs`

**Step 1: Write the failing test**

- Add one test that asserts `pdf-to-typst --version` prints the repository `VERSION` value to stdout and exits successfully.
- Add one test that asserts `pdf-to-typst -v` behaves the same way.
- Extend the help-text test so it expects `-v, --version`.

**Step 2: Run test to verify it fails**

Run: `cargo test version_flag_prints_version_from_embedded_version_file short_version_flag_prints_version_from_embedded_version_file help_text_documents_required_arguments_and_strict_mode`

Expected: version-related tests fail because the flags are not implemented yet.

### Task 2: Implement VERSION-based version output

**Files:**
- Create: `VERSION`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

**Step 1: Write minimal implementation**

- Add `VERSION` with initial value `v0.260323.1`.
- Add a small helper that returns the embedded version string with surrounding whitespace trimmed.
- Extend argument parsing to return a version-print action for `-v` and `--version`.
- Update help text to document the new flag.
- Teach `main` to print the version string and exit successfully for that parse result.

**Step 2: Run test to verify it passes**

Run: `cargo test version_flag_prints_version_from_embedded_version_file short_version_flag_prints_version_from_embedded_version_file help_text_documents_required_arguments_and_strict_mode`

Expected: all targeted tests pass.

### Task 3: Align release workflow and docs

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `README.md`
- Modify: `README.ko.md`

**Step 1: Update behavior**

- Remove dynamic tag composition from the workflow.
- Read `VERSION`, validate that it is non-empty, and use it as the release tag.
- Update documentation to describe `VERSION` as the release source of truth and explain how releases are triggered.

**Step 2: Verify**

Run: `cargo test`

Expected: full test suite passes after doc and workflow updates.

### Task 4: Land the change

**Files:**
- Review all modified files

**Step 1: Verify repository state**

Run:
- `cargo test`
- `git diff --stat`

Expected: tests pass and diff matches the approved design.

**Step 2: Commit and publish**

Run:
- `git add VERSION docs/plans src/lib.rs src/main.rs tests/cli.rs .github/workflows/release.yml README.md README.ko.md`
- `git commit -m "feat: version releases from VERSION file"`
- `git tag v0.260323.1`
- `git pull --rebase`
- `bd sync`
- `git push`
- `git push origin v0.260323.1`

Expected: branch and tag are both published.
