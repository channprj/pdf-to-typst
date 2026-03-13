# Ralph Progress Log

This file tracks progress across iterations. Agents update this file
after each iteration and it's included in prompts for context.

## Codebase Patterns (Study These First)

- Keep CLI parsing and conversion behavior in `src/lib.rs`, and keep `src/main.rs` as a thin layer that maps structured warnings/errors to stdout, stderr, and exit codes. Integration tests can then validate the real binary contract through `env!(\"CARGO_BIN_EXE_pdf-to-typst\")` without extra test-only dependencies.
- For PDF pipeline stories, keep low-level parsing and structural heuristics inside `src/lib.rs`, and generate compressed synthetic PDFs inside binary integration tests so the real CLI contract is exercised without relying on external PDF tooling.
- For OCR stories, normalize TSV OCR output back into the shared `ExtractedLine` model and drive binary integration tests with synthetic image-only PDFs plus a fake `tesseract` script so default language selection and diagnostics stay deterministic without machine-specific OCR data.
- For rich PDF elements, keep raw text fragments and XObject placements available until page-level rendering so table heuristics and caption attachment can consume those fragments before paragraph collapsing, and cover the behavior with synthetic PDFs that mix text, captions, and image XObjects in one binary test.
- For strict-mode CLI stories, collect the full warning set in `src/lib.rs` and only translate severity at the boundary so default mode can emit warning lines while `--strict` reuses the same page- and element-scoped diagnostics as fatal errors.
- For sample-regression stories, keep a manifest of real `data/` fixtures in binary integration tests with sample- and stage-specific failures, and use fake OCR binaries plus emitted-Typst validation helpers so regressions stay attributable even when external Typst tooling is unavailable.

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

## [2026-03-13] - US-004
- Implemented page-level rich-element conversion in `src/lib.rs` so embedded image XObjects can be extracted into `assets/`, referenced from generated Typst, and paired with detected figure captions.
- Added table heuristics that preserve aligned multi-column text as Typst table elements, attach nearby table captions when present, and record degraded image conversions through warnings instead of silently dropping them.
- Added binary integration coverage for mixed rich-content PDFs and degraded-image diagnostics using synthetic PDFs built entirely inside `tests/cli.rs`.
- Files changed: `src/lib.rs`, `tests/cli.rs`, `.ralph-tui/progress.md`
- **Learnings:**
  - Patterns discovered
    - Rich-element conversion works best as a page renderer that interleaves text blocks with figures and tables after captions are matched, rather than trying to bolt tables and images onto the already-collapsed paragraph output.
  - Gotchas encountered
    - Reusing the old generic XObject warning for missing image resources was necessary to keep the US-002 binary contract stable while still adding richer degraded-element diagnostics for unsupported image encodings.
    - Raw `DeviceGray` and `DeviceRGB` image streams need a real raster asset format for Typst references, so the converter now emits minimal PNG files instead of OCR-only PNM intermediates.
---

## [2026-03-13] - US-005
- Implemented strict-mode escalation so conversion warnings are preserved as a full diagnostic set and emitted as fatal error lines under `--strict`, while default mode keeps best-effort output with warning lines.
- Added binary integration coverage for a single rich-content failure case that succeeds in default mode, fails in strict mode, and proves both diagnostics remain visible in each mode.
- Files changed: `src/lib.rs`, `tests/cli.rs`, `.ralph-tui/progress.md`
- **Learnings:**
  - Patterns discovered
    - Treating strict mode as a severity translation step avoids duplicating conversion logic and keeps warning/error text aligned across CLI modes.
  - Gotchas encountered
    - Existing strict handling only surfaced the first warning, so the regression test had to use one synthetic PDF that produced multiple diagnostics to prove the CLI was preserving context instead of accidentally passing on a single-warning fixture.
---

## [2026-03-13] - US-006
- Added sample-driven binary regression coverage in `tests/cli.rs` for `data/sample-00.pdf`, `data/sample-01.pdf`, and `data/sample-02.pdf`, with sample metadata, stage-specific failures, fake OCR for the scanned fixture, and emitted-Typst validation over generated `main.typ` files and referenced assets.
- Extended `src/lib.rs` so compact PDF dictionaries like `/Type/Catalog` and `/Subtype/Image` are parsed correctly, and so 1-bit `CCITTFaxDecode` scanned images can be wrapped as TIFF inputs for OCR without introducing external PDF tooling.
- Verified the story with `cargo fmt --check` and `cargo test`.
- Files changed: `src/lib.rs`, `tests/cli.rs`, `.ralph-tui/progress.md`
- **Learnings:**
  - Patterns discovered
    - Real `data/` fixtures benefit from an explicit sample manifest in binary tests because it keeps coverage intentional and makes sample/stage failures readable when one fixture regresses.
    - OCR support can be extended to new PDF image encodings by converting embedded streams into a format `tesseract` already understands, which keeps the shared OCR pipeline intact.
  - Gotchas encountered
    - Real PDFs may omit whitespace between dictionary keys and names, so string checks like `/Type /Catalog` are too brittle for fixture-backed regression coverage.
    - The environment had no `typst` binary and no network access for Typst crates, so compileability coverage had to be enforced through validation of the converter's emitted Typst subset plus asset references inside tests.
---
