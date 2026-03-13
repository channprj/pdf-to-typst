use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pdf-to-typst"))
}

fn test_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();

    std::env::temp_dir().join(format!(
        "pdf-to-typst-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn create_dir(path: &Path) {
    fs::create_dir_all(path).expect("directory should be created");
}

fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        create_dir(parent);
    }

    fs::write(path, contents).expect("file should be written");
}

fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path).expect("file should be readable as utf-8")
}

#[test]
fn help_text_documents_required_arguments_and_strict_mode() {
    let output = binary()
        .arg("--help")
        .output()
        .expect("help command should execute");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: pdf-to-typst <INPUT_PDF> <OUTPUT_DIR> [OPTIONS]"));
    assert!(stdout.contains("Required arguments:"));
    assert!(stdout.contains("<INPUT_PDF>"));
    assert!(stdout.contains("<OUTPUT_DIR>"));
    assert!(stdout.contains("--strict"));
}

#[test]
fn success_creates_main_typ_and_assets_layout() {
    let output_root = test_path("success");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");

    create_dir(&output_root);
    write_file(&input, b"%PDF-1.4\n");

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output_dir.join("main.typ").is_file());
    assert!(output_dir.join("assets").is_dir());

    let main_typ = read_to_string(&output_dir.join("main.typ"));
    assert!(main_typ.contains("Generated from input.pdf"));
}

#[test]
fn default_mode_succeeds_with_warning_when_output_directory_is_not_empty() {
    let output_root = test_path("warning");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");

    create_dir(&output_root);
    write_file(&input, b"%PDF-1.4\n");
    create_dir(&output_dir);
    write_file(&output_dir.join("keep.txt"), b"pre-existing");

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert!(output_dir.join("main.typ").is_file());
    assert!(output_dir.join("keep.txt").is_file());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: output directory is not empty"));
}

#[test]
fn strict_mode_turns_layout_warning_into_fatal_failure() {
    let output_root = test_path("strict");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");

    create_dir(&output_root);
    write_file(&input, b"%PDF-1.4\n");
    create_dir(&output_dir);
    write_file(&output_dir.join("keep.txt"), b"pre-existing");

    let output = binary()
        .arg("--strict")
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(!output_dir.join("main.typ").exists());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: output directory is not empty"));
}
