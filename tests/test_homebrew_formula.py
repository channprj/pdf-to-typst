import pathlib
import subprocess
import tempfile
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "tools" / "generate_homebrew_formula.py"


class HomebrewFormulaGenerationTest(unittest.TestCase):
    def run_generator(self, out_dir: pathlib.Path) -> None:
        subprocess.run(
            [
                "python3",
                str(SCRIPT),
                "--version-tag",
                "v2026.0325.1",
                "--sha256",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "--out-dir",
                str(out_dir),
            ],
            cwd=REPO_ROOT,
            check=True,
        )

    def test_generates_latest_formula_for_tag_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            out_dir = pathlib.Path(temp_dir)

            self.run_generator(out_dir)

            formula = (out_dir / "pdf-to-typst.rb").read_text()

        self.assertIn("class PdfToTypst < Formula", formula)
        self.assertIn(
            'url "https://github.com/channprj/pdf-to-typst/archive/refs/tags/v2026.0325.1.tar.gz"',
            formula,
        )
        self.assertIn('version "2026.0325.1"', formula)
        self.assertIn('(lib/"pdf-to-typst").install "tools"', formula)

    def test_generates_versioned_formula_for_exact_release(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            out_dir = pathlib.Path(temp_dir)

            self.run_generator(out_dir)

            formula = (out_dir / "pdf-to-typst@2026.0325.1.rb").read_text()

        self.assertIn("class PdfToTypstAT202603251 < Formula", formula)
        self.assertIn(
            'url "https://github.com/channprj/pdf-to-typst/archive/refs/tags/v2026.0325.1.tar.gz"',
            formula,
        )
        self.assertIn('version "2026.0325.1"', formula)
        self.assertIn(
            'assert_match "v2026.0325.1", shell_output("#{bin}/pdf-to-typst --version")',
            formula,
        )


if __name__ == "__main__":
    unittest.main()
