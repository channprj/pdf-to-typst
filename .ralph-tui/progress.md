# Ralph Progress Log

This file tracks progress across iterations. Agents update this file
after each iteration and it's included in prompts for context.

## Codebase Patterns (Study These First)

- Keep CLI parsing and conversion behavior in `src/lib.rs`, and keep `src/main.rs` as a thin layer that maps structured warnings/errors to stdout, stderr, and exit codes. Integration tests can then validate the real binary contract through `env!(\"CARGO_BIN_EXE_pdf-to-typst\")` without extra test-only dependencies.
- For PDF pipeline stories, keep low-level parsing and structural heuristics inside `src/lib.rs`, and generate compressed synthetic PDFs inside binary integration tests so the real CLI contract is exercised without relying on external PDF tooling.
- For OCR stories, normalize TSV OCR output back into the shared `ExtractedLine` model and drive binary integration tests with synthetic image-only PDFs plus a fake `tesseract` script so default language selection and diagnostics stay deterministic without machine-specific OCR data.

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

## [2026-03-13] - US-002
- Implemented digital PDF parsing in `src/lib.rs` by scanning PDF objects, walking the page tree in document order, decoding `FlateDecode` content streams, extracting text operators, and mapping detected headings, paragraphs, bullet lists, and code-like blocks into Typst.
- Added diagnostics for unsupported page content such as XObject/image invocations, vector drawing commands, unsupported stream filters, and pages with no extractable digital text, while preserving existing `--strict` warning behavior.
- Replaced the placeholder success coverage with binary integration tests that build compressed synthetic PDFs and verify structured Typst output, multi-page ordering, and unsupported-content warnings.
- Files changed: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `tests/cli.rs`, `.ralph-tui/progress.md`
- **Learnings:**
  - Patterns discovered
    - Compressed in-test PDF fixtures are a practical way to exercise real parsing paths and CLI output contracts without introducing shell-level PDF tool dependencies.
  - Gotchas encountered
    - PDF stream payloads must stay as raw bytes in tests; converting compressed objects through lossy UTF-8 corrupts the fixture and produces misleading parser failures.
    - Existing warning-path tests that used `%PDF-1.4` placeholders had to be upgraded to minimal valid PDFs once conversion started parsing the input for real.
---

## [2026-03-13] - US-003
- Implemented an OCR fallback in `src/lib.rs` for image-based scanned pages by detecting image XObjects, extracting supported image streams, invoking `tesseract` with the default `kor+eng` profile, and converting OCR TSV back into the same Typst-rendering pipeline used for digital PDFs.
- Added page-scoped diagnostics for unavailable OCR support, unsupported embedded image encodings, no-text OCR results, and low-confidence OCR output while preserving the existing warning/strict-mode contract.
- Added binary integration coverage for scanned OCR success, default Korean/English language selection, unavailable OCR diagnostics, and low-confidence OCR diagnostics using synthetic scanned PDFs and a fake `tesseract` executable.
- Files changed: `src/lib.rs`, `tests/cli.rs`, `.ralph-tui/progress.md`
- **Learnings:**
  - Patterns discovered
    - Translating OCR TSV rows into the existing extracted-line abstraction keeps scanned and digital PDFs on one rendering path instead of creating a second Typst formatter.
  - Gotchas encountered
    - Synthetic scanned-PDF fixtures must compress raw image bytes directly; lossy UTF-8 conversion breaks image stream lengths and makes OCR extraction look like a parser bug.
    - Real environments may have `tesseract` installed without Korean language data, so tests need a fake OCR binary to verify the default `kor+eng` contract deterministically.
---
