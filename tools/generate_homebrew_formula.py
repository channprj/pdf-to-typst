#!/usr/bin/env python3

import argparse
import pathlib
import re
import textwrap


VERSION_TAG_PATTERN = re.compile(r"^v(?P<version>\d{4}\.\d{4}\.\d+)$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate latest and versioned Homebrew formulas from a release tag."
    )
    parser.add_argument("--version-tag", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--out-dir", required=True, type=pathlib.Path)
    parser.add_argument("--repo", default="channprj/pdf-to-typst")
    return parser.parse_args()


def parse_version(version_tag: str) -> str:
    match = VERSION_TAG_PATTERN.fullmatch(version_tag)
    if match is None:
        raise ValueError(
            f"VERSION tag must match v{{YYYY}}.{{MMDD}}.{{N}}; got {version_tag}"
        )
    return match.group("version")


def versioned_class_name(version: str) -> str:
    return f"PdfToTypstAT{re.sub(r'[^0-9A-Za-z]', '', version)}"


def formula_contents(
    *,
    class_name: str,
    version_tag: str,
    version: str,
    sha256: str,
    repo: str,
) -> str:
    url = f"https://github.com/{repo}/archive/refs/tags/{version_tag}.tar.gz"
    return textwrap.dedent(
        f"""\
        class {class_name} < Formula
          desc "Convert PDF documents into editable Typst projects"
          homepage "https://github.com/{repo}"
          url "{url}"
          version "{version}"
          sha256 "{sha256}"
          license "MIT"

          depends_on "rust" => :build
          depends_on "ghostscript"

          def install
            system "cargo", "install", *std_cargo_args(path: ".")
            (lib/"pdf-to-typst").install "tools"
          end

          test do
            assert_match "{version_tag}", shell_output("#{{bin}}/pdf-to-typst --version")
            assert_path_exists lib/"pdf-to-typst/tools/extract_non_text_regions.py"
          end
        end
        """
    )


def write_formula(path: pathlib.Path, contents: str) -> None:
    path.write_text(contents)


def main() -> None:
    args = parse_args()
    version = parse_version(args.version_tag)

    args.out_dir.mkdir(parents=True, exist_ok=True)

    write_formula(
        args.out_dir / "pdf-to-typst.rb",
        formula_contents(
            class_name="PdfToTypst",
            version_tag=args.version_tag,
            version=version,
            sha256=args.sha256,
            repo=args.repo,
        ),
    )
    write_formula(
        args.out_dir / f"pdf-to-typst@{version}.rb",
        formula_contents(
            class_name=versioned_class_name(version),
            version_tag=args.version_tag,
            version=version,
            sha256=args.sha256,
            repo=args.repo,
        ),
    )


if __name__ == "__main__":
    main()
