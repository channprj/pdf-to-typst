# Ralph Progress Log

This file tracks progress across iterations. Agents update this file
after each iteration and it's included in prompts for context.

## Codebase Patterns (Study These First)

- Keep CLI parsing and conversion behavior in `src/lib.rs`, and keep `src/main.rs` as a thin layer that maps structured warnings/errors to stdout, stderr, and exit codes. Integration tests can then validate the real binary contract through `env!(\"CARGO_BIN_EXE_pdf-to-typst\")` without extra test-only dependencies.

---

## [2026-03-13] - US-001
- Implemented the initial Rust CLI scaffold for converting one PDF path into a deterministic output directory with `main.typ` and `assets/`.
- Added manual help text, `--strict` handling, and output-directory reuse behavior that distinguishes clean success, warning-backed success, and fatal failure.
- Documented the CLI contract and runtime behaviors in `README.md`, and covered them with binary-level integration tests.
- Files changed: `.gitignore`, `Cargo.toml`, `README.md`, `src/lib.rs`, `src/main.rs`, `tests/cli.rs`, `.ralph-tui/progress.md`
- **Learnings:**
  - Patterns discovered
    - A thin-binary plus library-core split is a good fit for this repo because the CLI contract is testable through process execution while the behavior stays easy to extend for later PDF pipeline stories.
  - Gotchas encountered
    - The repository started without a Cargo project, so minimal crate scaffolding had to be established before the first meaningful red-green TDD cycle could happen.
---
