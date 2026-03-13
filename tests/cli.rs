use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::ZlibEncoder;

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

fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("file metadata should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("file should be executable");
}

fn write_script(path: &Path, contents: &str) {
    write_file(path, contents.as_bytes());
    make_executable(path);
}

#[derive(Clone, Copy)]
struct TextLine<'a> {
    font: &'a str,
    size: f32,
    x: f32,
    y: f32,
    text: &'a str,
}

struct PageSpec<'a> {
    lines: &'a [TextLine<'a>],
    extra_commands: &'a [&'a str],
}

struct ScannedPageSpec<'a> {
    width: usize,
    height: usize,
    pixels: &'a [u8],
    draw_x: f32,
    draw_y: f32,
    draw_width: f32,
    draw_height: f32,
}

enum ImageObjectFilter {
    Flate,
}

struct ImageObjectSpec<'a> {
    name: &'a str,
    width: usize,
    height: usize,
    color_space: &'a str,
    bits_per_component: usize,
    filter: ImageObjectFilter,
    bytes: &'a [u8],
}

struct RichPageSpec<'a> {
    lines: &'a [TextLine<'a>],
    extra_commands: &'a [&'a str],
    xobjects: &'a [&'a str],
}

fn pdf_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());

    for ch in text.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '(' => escaped.push_str("\\("),
            ')' => escaped.push_str("\\)"),
            _ => escaped.push(ch),
        }
    }

    escaped
}

fn compressed_stream(contents: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, contents.as_bytes())
        .expect("stream should be compressed");
    encoder.finish().expect("compression should succeed")
}

fn compressed_bytes(contents: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, contents).expect("stream should be compressed");
    encoder.finish().expect("compression should succeed")
}

fn page_stream(page: &PageSpec<'_>) -> String {
    let mut stream = String::new();

    for line in page.lines {
        stream.push_str("BT\n");
        stream.push_str(&format!("/{font} {} Tf\n", line.size, font = line.font));
        stream.push_str(&format!("1 0 0 1 {} {} Tm\n", line.x, line.y));
        stream.push_str(&format!("({}) Tj\n", pdf_escape(line.text)));
        stream.push_str("ET\n");
    }

    for command in page.extra_commands {
        stream.push_str(command);
        stream.push('\n');
    }

    stream
}

fn build_pdf(pages: &[PageSpec<'_>]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|index| format!("{} 0 R", 3 + index as u32))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        )
        .into_bytes(),
    ];

    let font_start = 3 + pages.len() as u32;
    let body_font = font_start;
    let heading_font = font_start + 1;
    let code_font = font_start + 2;
    let contents_start = font_start + 3;

    for page_index in 0..pages.len() {
        let contents_ref = contents_start + page_index as u32;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 {body_font} 0 R /F2 {heading_font} 0 R /F3 {code_font} 0 R >> >> /Contents {contents_ref} 0 R >>"
        ).into_bytes());
    }

    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_vec());

    for page in pages {
        let stream = compressed_stream(&page_stream(page));
        let mut object = format!(
            "<< /Length {} /Filter /FlateDecode >>\nstream\n",
            stream.len()
        )
        .into_bytes();
        object.extend_from_slice(&stream);
        object.extend_from_slice(b"\nendstream");
        objects.push(object);
    }

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];

    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );

    pdf
}

fn build_scanned_pdf(pages: &[ScannedPageSpec<'_>]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|index| format!("{} 0 R", 3 + index as u32))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        )
        .into_bytes(),
    ];

    let contents_start = 3 + pages.len() as u32;
    let image_start = contents_start + pages.len() as u32;

    for page_index in 0..pages.len() {
        let contents_ref = contents_start + page_index as u32;
        let image_ref = image_start + page_index as u32;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im1 {image_ref} 0 R >> >> /Contents {contents_ref} 0 R >>"
            )
            .into_bytes(),
        );
    }

    for page in pages {
        let stream = compressed_stream(&format!(
            "q\n{} 0 0 {} {} {} cm\n/Im1 Do\nQ\n",
            page.draw_width, page.draw_height, page.draw_x, page.draw_y
        ));
        let mut object = format!(
            "<< /Length {} /Filter /FlateDecode >>\nstream\n",
            stream.len()
        )
        .into_bytes();
        object.extend_from_slice(&stream);
        object.extend_from_slice(b"\nendstream");
        objects.push(object);
    }

    for page in pages {
        let stream = compressed_bytes(page.pixels);
        let mut object = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} /Filter /FlateDecode >>\nstream\n",
            page.width,
            page.height,
            stream.len()
        )
        .into_bytes();
        object.extend_from_slice(&stream);
        object.extend_from_slice(b"\nendstream");
        objects.push(object);
    }

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];

    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );

    pdf
}

fn build_rich_pdf(pages: &[RichPageSpec<'_>], images: &[ImageObjectSpec<'_>]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|index| format!("{} 0 R", 3 + index as u32))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        )
        .into_bytes(),
    ];

    let font_start = 3 + pages.len() as u32;
    let body_font = font_start;
    let heading_font = font_start + 1;
    let code_font = font_start + 2;
    let contents_start = font_start + 3;
    let image_start = contents_start + pages.len() as u32;

    for page_index in 0..pages.len() {
        let contents_ref = contents_start + page_index as u32;
        let xobject_map = pages[page_index]
            .xobjects
            .iter()
            .map(|name| {
                let image_index = images
                    .iter()
                    .position(|image| image.name == *name)
                    .expect("page xobject should match an image object");
                format!("/{name} {} 0 R", image_start + image_index as u32)
            })
            .collect::<Vec<_>>()
            .join(" ");
        let xobject_resources = if xobject_map.is_empty() {
            String::new()
        } else {
            format!(" /XObject << {xobject_map} >>")
        };

        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 {body_font} 0 R /F2 {heading_font} 0 R /F3 {code_font} 0 R >>{xobject_resources} >> /Contents {contents_ref} 0 R >>"
        ).into_bytes());
    }

    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_vec());

    for page in pages {
        let stream = compressed_stream(&page_stream(&PageSpec {
            lines: page.lines,
            extra_commands: page.extra_commands,
        }));
        let mut object = format!(
            "<< /Length {} /Filter /FlateDecode >>\nstream\n",
            stream.len()
        )
        .into_bytes();
        object.extend_from_slice(&stream);
        object.extend_from_slice(b"\nendstream");
        objects.push(object);
    }

    for image in images {
        let (stream, filter_name) = match image.filter {
            ImageObjectFilter::Flate => (compressed_bytes(image.bytes), "FlateDecode"),
        };
        let mut object = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /{} /BitsPerComponent {} /Length {} /Filter /{} >>\nstream\n",
            image.width,
            image.height,
            image.color_space,
            image.bits_per_component,
            stream.len(),
            filter_name
        )
        .into_bytes();
        object.extend_from_slice(&stream);
        object.extend_from_slice(b"\nendstream");
        objects.push(object);
    }

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];

    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );

    pdf
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
fn digital_pdf_text_is_converted_into_typst_structures() {
    let output_root = test_path("success");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = PageSpec {
        lines: &[
            TextLine {
                font: "F2",
                size: 20.0,
                x: 72.0,
                y: 720.0,
                text: "Release Notes",
            },
            TextLine {
                font: "F1",
                size: 12.0,
                x: 72.0,
                y: 690.0,
                text: "This PDF stays digital.",
            },
            TextLine {
                font: "F1",
                size: 12.0,
                x: 72.0,
                y: 676.0,
                text: "The next line should join the same paragraph.",
            },
            TextLine {
                font: "F1",
                size: 12.0,
                x: 90.0,
                y: 640.0,
                text: "- First item",
            },
            TextLine {
                font: "F1",
                size: 12.0,
                x: 90.0,
                y: 626.0,
                text: "- Second item",
            },
            TextLine {
                font: "F3",
                size: 10.0,
                x: 108.0,
                y: 590.0,
                text: "fn main() {",
            },
            TextLine {
                font: "F3",
                size: 10.0,
                x: 108.0,
                y: 578.0,
                text: "  println(\"hi\");",
            },
            TextLine {
                font: "F3",
                size: 10.0,
                x: 108.0,
                y: 566.0,
                text: "}",
            },
        ],
        extra_commands: &[],
    };

    create_dir(&output_root);
    write_file(&input, &build_pdf(&[page]));

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
    assert_eq!(
        main_typ,
        "= Release Notes\n\nThis PDF stays digital. The next line should join the same paragraph.\n\n- First item\n- Second item\n\n```\nfn main() {\n  println(\"hi\");\n}\n```\n"
    );
}

#[test]
fn multi_page_documents_preserve_reading_order() {
    let output_root = test_path("multi-page");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let first_page = PageSpec {
        lines: &[TextLine {
            font: "F1",
            size: 12.0,
            x: 72.0,
            y: 720.0,
            text: "First page content comes first.",
        }],
        extra_commands: &[],
    };
    let second_page = PageSpec {
        lines: &[
            TextLine {
                font: "F2",
                size: 18.0,
                x: 72.0,
                y: 720.0,
                text: "Second Page",
            },
            TextLine {
                font: "F1",
                size: 12.0,
                x: 72.0,
                y: 690.0,
                text: "Second page content follows the first.",
            },
        ],
        extra_commands: &[],
    };

    create_dir(&output_root);
    write_file(&input, &build_pdf(&[first_page, second_page]));

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());

    let main_typ = read_to_string(&output_dir.join("main.typ"));
    assert_eq!(
        main_typ,
        "First page content comes first.\n\n= Second Page\n\nSecond page content follows the first.\n"
    );
}

#[test]
fn unsupported_non_text_content_is_reported_as_a_warning() {
    let output_root = test_path("unsupported");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = PageSpec {
        lines: &[TextLine {
            font: "F1",
            size: 12.0,
            x: 72.0,
            y: 720.0,
            text: "Text survives unsupported content.",
        }],
        extra_commands: &["/Im1 Do"],
    };

    create_dir(&output_root);
    write_file(&input, &build_pdf(&[page]));

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: unsupported content on page 1: XObject invocation"));
}

#[test]
fn default_mode_succeeds_with_warning_when_output_directory_is_not_empty() {
    let output_root = test_path("warning");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = PageSpec {
        lines: &[TextLine {
            font: "F1",
            size: 12.0,
            x: 72.0,
            y: 720.0,
            text: "Warnings should not block a default-mode conversion.",
        }],
        extra_commands: &[],
    };

    create_dir(&output_root);
    write_file(&input, &build_pdf(&[page]));
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
    let page = PageSpec {
        lines: &[TextLine {
            font: "F1",
            size: 12.0,
            x: 72.0,
            y: 720.0,
            text: "Strict mode should stop before writing output.",
        }],
        extra_commands: &[],
    };

    create_dir(&output_root);
    write_file(&input, &build_pdf(&[page]));
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

#[test]
fn scanned_pdf_uses_ocr_with_default_korean_and_english_profile() {
    let output_root = test_path("ocr-success");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let fake_tesseract = output_root.join("fake-tesseract.sh");
    let invocation_log = output_root.join("invocation.log");
    let page = ScannedPageSpec {
        width: 8,
        height: 8,
        pixels: &[255; 64],
        draw_x: 72.0,
        draw_y: 540.0,
        draw_width: 468.0,
        draw_height: 180.0,
    };

    create_dir(&output_root);
    write_file(&input, &build_scanned_pdf(&[page]));
    write_script(
        &fake_tesseract,
        &format!(
            r#"#!/bin/sh
set -eu
if [ "${{1:-}}" = "--list-langs" ]; then
  printf 'List of available languages in "/tmp/tessdata/" (2):\neng\nkor\n'
  exit 0
fi
printf '%s\n' "$@" > "{}"
found_lang=0
found_tsv=0
prev=""
for arg in "$@"; do
  if [ "$prev" = "-l" ] && [ "$arg" = "kor+eng" ]; then
    found_lang=1
  fi
  if [ "$arg" = "tsv" ]; then
    found_tsv=1
  fi
  prev="$arg"
done
if [ "$found_lang" -ne 1 ] || [ "$found_tsv" -ne 1 ]; then
  echo "unexpected tesseract arguments" >&2
  exit 9
fi
cat <<'EOF'
level	page_num	block_num	par_num	line_num	word_num	left	top	width	height	conf	text
1	1	0	0	0	0	0	0	1000	1400	-1	
2	1	1	0	0	0	60	80	620	290	-1	
3	1	1	1	0	0	60	80	620	290	-1	
4	1	1	1	1	0	60	80	420	42	-1	
5	1	1	1	1	1	60	80	150	42	96	회의록
5	1	1	1	1	2	224	80	108	42	94	Meeting
5	1	1	1	1	3	344	80	180	42	94	Notes
4	1	1	1	2	0	60	136	520	18	-1	
5	1	1	1	2	1	60	136	118	18	93	스캔된
5	1	1	1	2	2	190	136	162	18	92	문서입니다.
4	1	1	1	3	0	60	162	650	18	-1	
5	1	1	1	3	1	60	162	86	18	92	English
5	1	1	1	3	2	158	162	58	18	91	text
5	1	1	1	3	3	228	162	66	18	91	joins
5	1	1	1	3	4	306	162	54	18	90	same
5	1	1	1	3	5	372	162	132	18	90	paragraph.
4	1	1	1	4	0	78	224	300	18	-1	
5	1	1	1	4	1	78	224	18	18	91	-
5	1	1	1	4	2	108	224	92	18	91	첫
5	1	1	1	4	3	212	224	64	18	90	번째
5	1	1	1	4	4	288	224	74	18	90	item
4	1	1	1	5	0	78	252	328	18	-1	
5	1	1	1	5	1	78	252	18	18	91	-
5	1	1	1	5	2	108	252	92	18	91	Second
5	1	1	1	5	3	212	252	74	18	90	항목
EOF
"#,
            invocation_log.display()
        ),
    );

    let output = binary()
        .env("PDF_TO_TYPST_TESSERACT_BIN", &fake_tesseract)
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        read_to_string(&output_dir.join("main.typ")),
        "= 회의록 Meeting Notes\n\n스캔된 문서입니다. English text joins same paragraph.\n\n- 첫 번째 item\n- Second 항목\n"
    );
    assert!(
        read_to_string(&invocation_log).contains("kor+eng"),
        "default OCR language profile should target Korean and English"
    );
}

#[test]
fn scanned_pdf_reports_when_ocr_is_unavailable() {
    let output_root = test_path("ocr-unavailable");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = ScannedPageSpec {
        width: 4,
        height: 4,
        pixels: &[0; 16],
        draw_x: 72.0,
        draw_y: 600.0,
        draw_width: 300.0,
        draw_height: 120.0,
    };

    create_dir(&output_root);
    write_file(&input, &build_scanned_pdf(&[page]));

    let output = binary()
        .env(
            "PDF_TO_TYPST_TESSERACT_BIN",
            output_root.join("missing-tesseract"),
        )
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert_eq!(
        read_to_string(&output_dir.join("main.typ")),
        "// No digital text could be extracted from the PDF.\n"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: OCR unavailable on page 1"));
    assert!(stderr.contains("scanned page content could not be extracted"));
}

#[test]
fn scanned_pdf_warns_when_ocr_confidence_is_low() {
    let output_root = test_path("ocr-low-confidence");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let fake_tesseract = output_root.join("fake-tesseract.sh");
    let page = ScannedPageSpec {
        width: 4,
        height: 4,
        pixels: &[128; 16],
        draw_x: 72.0,
        draw_y: 600.0,
        draw_width: 300.0,
        draw_height: 120.0,
    };

    create_dir(&output_root);
    write_file(&input, &build_scanned_pdf(&[page]));
    write_script(
        &fake_tesseract,
        r#"#!/bin/sh
set -eu
if [ "${1:-}" = "--list-langs" ]; then
  printf 'List of available languages in "/tmp/tessdata/" (2):\neng\nkor\n'
  exit 0
fi
cat <<'EOF'
level	page_num	block_num	par_num	line_num	word_num	left	top	width	height	conf	text
1	1	0	0	0	0	0	0	1000	1400	-1	
2	1	1	0	0	0	60	120	220	24	-1	
3	1	1	1	0	0	60	120	220	24	-1	
4	1	1	1	1	0	60	120	220	24	-1	
5	1	1	1	1	1	60	120	110	24	31	희미한
5	1	1	1	1	2	184	120	96	24	29	text
EOF
"#,
    );

    let output = binary()
        .env("PDF_TO_TYPST_TESSERACT_BIN", &fake_tesseract)
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert_eq!(
        read_to_string(&output_dir.join("main.typ")),
        "희미한 text\n"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: low-confidence OCR on page 1"));
    assert!(stderr.contains("generated Typst may contain recognition errors"));
}

#[test]
fn rich_pdf_extracts_images_tables_and_captions_into_typst() {
    let output_root = test_path("rich-elements");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = RichPageSpec {
        lines: &[
            TextLine {
                font: "F2",
                size: 18.0,
                x: 72.0,
                y: 736.0,
                text: "Quarterly Summary",
            },
            TextLine {
                font: "F1",
                size: 12.0,
                x: 72.0,
                y: 706.0,
                text: "Rich content should survive the conversion.",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 72.0,
                y: 500.0,
                text: "Figure 1: Revenue heatmap",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 72.0,
                y: 440.0,
                text: "Table 1: Regional metrics",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 72.0,
                y: 410.0,
                text: "Region",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 220.0,
                y: 410.0,
                text: "Q1",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 340.0,
                y: 410.0,
                text: "Q2",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 72.0,
                y: 392.0,
                text: "APAC",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 220.0,
                y: 392.0,
                text: "12",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 340.0,
                y: 392.0,
                text: "18",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 72.0,
                y: 374.0,
                text: "EMEA",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 220.0,
                y: 374.0,
                text: "9",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 340.0,
                y: 374.0,
                text: "11",
            },
        ],
        extra_commands: &["q", "192 0 0 108 72 548 cm", "/Im1 Do", "Q"],
        xobjects: &["Im1"],
    };
    let image = ImageObjectSpec {
        name: "Im1",
        width: 2,
        height: 2,
        color_space: "DeviceRGB",
        bits_per_component: 8,
        filter: ImageObjectFilter::Flate,
        bytes: &[
            255, 0, 0, 0, 255, 0, //
            0, 0, 255, 255, 255, 0,
        ],
    };

    create_dir(&output_root);
    write_file(&input, &build_rich_pdf(&[page], &[image]));

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        output_dir
            .join("assets")
            .join("page-1-image-1.png")
            .is_file()
    );

    let main_typ = read_to_string(&output_dir.join("main.typ"));
    assert!(main_typ.contains("= Quarterly Summary"));
    assert!(main_typ.contains("Rich content should survive the conversion."));
    assert!(main_typ.contains("#figure("));
    assert!(main_typ.contains("image(\"assets/page-1-image-1.png\")"));
    assert!(main_typ.contains("caption: [Figure 1: Revenue heatmap]"));
    assert!(main_typ.contains("kind: table"));
    assert!(main_typ.contains("caption: [Table 1: Regional metrics]"));
    assert!(main_typ.contains("[Region]"));
    assert!(main_typ.contains("[APAC]"));
    assert!(main_typ.contains("[11]"));
}

#[test]
fn degraded_rich_elements_are_recorded_when_images_cannot_be_extracted() {
    let output_root = test_path("rich-degraded");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = RichPageSpec {
        lines: &[
            TextLine {
                font: "F1",
                size: 12.0,
                x: 72.0,
                y: 720.0,
                text: "This page still has readable text.",
            },
            TextLine {
                font: "F1",
                size: 11.0,
                x: 72.0,
                y: 500.0,
                text: "Figure 2: Unsupported color profile",
            },
        ],
        extra_commands: &["q", "160 0 0 90 72 548 cm", "/ImBad Do", "Q"],
        xobjects: &["ImBad"],
    };
    let image = ImageObjectSpec {
        name: "ImBad",
        width: 2,
        height: 2,
        color_space: "DeviceCMYK",
        bits_per_component: 8,
        filter: ImageObjectFilter::Flate,
        bytes: &[
            0, 255, 255, 0, 255, 0, 255, 0, //
            255, 255, 0, 0, 0, 0, 0, 255,
        ],
    };

    create_dir(&output_root);
    write_file(&input, &build_rich_pdf(&[page], &[image]));

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert_eq!(
        read_to_string(&output_dir.join("main.typ")),
        "This page still has readable text.\n\nFigure 2: Unsupported color profile\n"
    );
    assert!(
        !output_dir
            .join("assets")
            .join("page-1-image-1.png")
            .exists()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: degraded rich element on page 1"));
    assert!(stderr.contains("image ImBad could not be extracted"));
}

#[test]
fn default_mode_preserves_output_when_conversion_reports_multiple_diagnostics() {
    let output_root = test_path("default-multi-diagnostic");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = RichPageSpec {
        lines: &[TextLine {
            font: "F1",
            size: 12.0,
            x: 72.0,
            y: 720.0,
            text: "Best-effort mode should keep readable text.",
        }],
        extra_commands: &[
            "q",
            "160 0 0 90 72 548 cm",
            "/ImBad Do",
            "Q",
            "q",
            "160 0 0 90 260 548 cm",
            "/ImMissing Do",
            "Q",
        ],
        xobjects: &["ImBad"],
    };
    let image = ImageObjectSpec {
        name: "ImBad",
        width: 2,
        height: 2,
        color_space: "DeviceCMYK",
        bits_per_component: 8,
        filter: ImageObjectFilter::Flate,
        bytes: &[
            0, 255, 255, 0, 255, 0, 255, 0, //
            255, 255, 0, 0, 0, 0, 0, 255,
        ],
    };

    create_dir(&output_root);
    write_file(&input, &build_rich_pdf(&[page], &[image]));

    let output = binary()
        .arg(&input)
        .arg(&output_dir)
        .output()
        .expect("conversion should execute");

    assert!(output.status.success());
    assert_eq!(
        read_to_string(&output_dir.join("main.typ")),
        "Best-effort mode should keep readable text.\n"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: degraded rich element on page 1"));
    assert!(stderr.contains("image ImBad could not be extracted"));
    assert!(stderr.contains("warning: unsupported content on page 1: XObject invocation"));
}

#[test]
fn strict_mode_fails_when_conversion_reports_multiple_diagnostics() {
    let output_root = test_path("strict-multi-diagnostic");
    let input = output_root.join("input.pdf");
    let output_dir = output_root.join("out");
    let page = RichPageSpec {
        lines: &[TextLine {
            font: "F1",
            size: 12.0,
            x: 72.0,
            y: 720.0,
            text: "Strict mode should reject incomplete conversion.",
        }],
        extra_commands: &[
            "q",
            "160 0 0 90 72 548 cm",
            "/ImBad Do",
            "Q",
            "q",
            "160 0 0 90 260 548 cm",
            "/ImMissing Do",
            "Q",
        ],
        xobjects: &["ImBad"],
    };
    let image = ImageObjectSpec {
        name: "ImBad",
        width: 2,
        height: 2,
        color_space: "DeviceCMYK",
        bits_per_component: 8,
        filter: ImageObjectFilter::Flate,
        bytes: &[
            0, 255, 255, 0, 255, 0, 255, 0, //
            255, 255, 0, 0, 0, 0, 0, 255,
        ],
    };

    create_dir(&output_root);
    write_file(&input, &build_rich_pdf(&[page], &[image]));

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
    assert!(stderr.contains("error: degraded rich element on page 1"));
    assert!(stderr.contains("image ImBad could not be extracted"));
    assert!(stderr.contains("error: unsupported content on page 1: XObject invocation"));
}
