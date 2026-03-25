# pdf-to-typst

[English](README.md) | [한국어](README.ko.md)

> Convert PDF documents into editable Typst projects.

`pdf-to-typst` takes a single PDF and produces a ready-to-compile [Typst](https://typst.app/) project — `main.typ` plus an `assets/` directory — preserving text, images, tables, and layout as faithfully as possible.

## Features

- Native PDF text extraction and structural analysis
- OCR for scanned documents (Korean + English by default)
- Table, image, and caption preservation
- PDFKit-based text position recovery (macOS)
- Ghostscript-backed element image fallback for complex regions
- `--strict` mode for CI/pipeline use (warnings become errors)

## Quick Start

### macOS (Homebrew)

```sh
brew install channprj/tap/pdf-to-typst
brew install channprj/tap/pdf-to-typst@2026.0325.1
```

> **Note:** The tap builds from the tagged source archive for each release and
> places helper scripts under Homebrew's `lib/pdf-to-typst/tools` path.

### GitHub Releases

Pre-built archives are published on
[Releases](https://github.com/channprj/pdf-to-typst/releases) whenever a commit
landed on `main` includes the word `release` in its commit message, or when the
release workflow is triggered manually. The release version is read directly
from the repository's `VERSION` file, for example `v2026.0325.1`, and the same
value is used for Git tags, GitHub Releases, `pdf-to-typst --version`, and the
Homebrew formulas published in `channprj/tap`.
Archives are published for:

| Platform | Target |
|----------|--------|
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |

```sh
tar xzf pdf-to-typst-v*.tar.gz
cd pdf-to-typst-v*
./pdf-to-typst --version
./pdf-to-typst input.pdf output/
```

### Build from Source

```sh
cargo install --git https://github.com/channprj/pdf-to-typst
```

Or clone and build locally:

```sh
git clone https://github.com/channprj/pdf-to-typst.git
cd pdf-to-typst
cargo build --release
# Binary at target/release/pdf-to-typst
```

## Dependencies

### Required

- **Ghostscript** (`gs`) — renders OCR inputs and element-level fallback crops for complex PDF regions

### Optional

- **Tesseract** — OCR for scanned pages (default languages: `kor+eng`)
- **Python 3 + ImageMagick** (`convert`) — non-text region extraction
- **Xcode Command Line Tools** — PDFKit text position recovery (macOS only)
- **Typst** — only if you want to compile the output immediately

### Platform Install Commands

**macOS (Homebrew):**

```sh
brew install ghostscript tesseract imagemagick typst
# Xcode CLI Tools: xcode-select --install
```

**Ubuntu / Debian:**

```sh
sudo apt-get install ghostscript tesseract-ocr imagemagick
```

**Fedora:**

```sh
sudo dnf install ghostscript tesseract ImageMagick
```

## Usage

### Basic

```sh
pdf-to-typst input.pdf output/
```

On success, prints the path to the generated `main.typ` and exits with code `0`.

### Strict Mode

```sh
pdf-to-typst input.pdf output/ --strict
```

In strict mode, any warning (e.g., reusing a non-empty output directory) is promoted to a fatal error (exit code `2`).

### Environment Variables

| Variable | Description |
|----------|-------------|
| `PDF_TO_TYPST_TOOLS_DIR` | Override the path to the `tools/` helper scripts |

### Output Structure

```
output/
├── main.typ      # Primary Typst entrypoint
└── assets/       # Extracted images and resources
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0`  | Success (warnings may be printed to stderr) |
| `2`  | Fatal error — no output produced |

## How It Works

`pdf-to-typst` uses three conversion paths depending on the PDF content:

1. **Native text extraction** — Parses the PDF binary structure directly to extract text, fonts, and layout. This is the primary path for digitally-created PDFs.

2. **PDFKit scene analysis** (macOS) — Uses Apple's PDFKit framework via a Swift helper to recover precise text positions and render pages, improving fidelity for complex layouts.

3. **OCR fallback** — For scanned documents without embedded text, Tesseract performs optical character recognition with Korean and English language support by default.

When native parsing cannot safely reconstruct a complex table, diagram, or caption cluster, the converter falls back to cropped element images while leaving unrelated text on the same page as Typst text.

## Troubleshooting

### `error: gs not found`

Ghostscript is required. Install it with your package manager (see [Dependencies](#dependencies)).

### `warning: tesseract not available`

OCR will be skipped for scanned pages. Install Tesseract to enable OCR support.

### `warning: reusing non-empty output directory`

The output directory already contains files. In default mode this is a warning; in `--strict` mode it is a fatal error. Use a clean directory or remove existing files.

### PDFKit helper fails to compile

Ensure Xcode Command Line Tools are installed: `xcode-select --install`. This feature is macOS-only.

## Contributing

Contributions are welcome! Please open an issue or submit a pull request on [GitHub](https://github.com/channprj/pdf-to-typst).

## License

[MIT](LICENSE)
