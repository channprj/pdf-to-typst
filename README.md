# pdf-to-typst

`pdf-to-typst` converts a single input PDF into a deterministic output directory.

## CLI contract

```text
pdf-to-typst <INPUT_PDF> <OUTPUT_DIR> [OPTIONS]
```

Required arguments:

- `<INPUT_PDF>`: path to the source PDF.
- `<OUTPUT_DIR>`: directory that will contain the generated Typst project files.

Key options:

- `--strict`: promote warnings to fatal errors.
- `-h`, `--help`: print usage and option details.

## Output layout

On success, the CLI writes:

- `main.typ`: the primary Typst entrypoint.
- `assets/`: the directory reserved for extracted asset files referenced by `main.typ`.

## Runtime behavior

- Success: exit code `0`, `main.typ` and `assets/` are created.
- Success with warnings: exit code `0`, output files are created, and warnings are printed to standard error.
- Fatal failure: exit code `2`, no new output is produced for that run, and the error is printed to standard error.

For the initial contract, reusing a non-empty output directory is treated as a warning in default mode and as a fatal failure in `--strict` mode.
