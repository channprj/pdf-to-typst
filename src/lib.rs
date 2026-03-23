use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

const HELP_TEXT: &str = "\
pdf-to-typst

Convert a single PDF into a deterministic Typst output directory.

Usage: pdf-to-typst <INPUT_PDF> <OUTPUT_DIR> [OPTIONS]

Required arguments:
  <INPUT_PDF>   Path to the source PDF file.
  <OUTPUT_DIR>  Directory where main.typ and assets/ are written.

Options:
  --strict      Treat warnings as fatal errors.
  -v, --version Print the release version.
  -h, --help    Print this help text.
";

const DEFAULT_OCR_LANGUAGES: &str = "kor+eng";
const DEFAULT_OCR_MIN_CONFIDENCE: f32 = 65.0;
const RASTER_FALLBACK_DPI: u32 = 144;
const PDFKIT_RENDER_SCALE: f32 = 2.0;

fn tools_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(dir) = env::var("PDF_TO_TYPST_TOOLS_DIR") {
            return PathBuf::from(dir);
        }
        if let Ok(exe) = env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let homebrew = parent.join("../lib/pdf-to-typst/tools");
            if homebrew.is_dir() {
                return homebrew;
            }
            let sibling = parent.join("tools");
            if sibling.is_dir() {
                return sibling;
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools")
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct CliOptions {
    pub input_pdf: PathBuf,
    pub output_dir: PathBuf,
    pub strict: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Warning {
    message: String,
}

impl Warning {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConversionSuccess {
    pub main_typ: PathBuf,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CliFailure {
    pub exit_code: i32,
    pub message: String,
    pub print_help: bool,
}

impl CliFailure {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            exit_code: 1,
            message: message.into(),
            print_help: true,
        }
    }

    fn fatal(message: impl Into<String>) -> Self {
        Self {
            exit_code: 2,
            message: message.into(),
            print_help: false,
        }
    }
}

impl fmt::Display for CliFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

pub enum ParseResult {
    Help,
    Version,
    Run(CliOptions),
}

pub fn help_text() -> &'static str {
    HELP_TEXT
}

pub fn version_text() -> &'static str {
    include_str!("../VERSION").trim()
}

pub fn parse_args<I>(args: I) -> Result<ParseResult, CliFailure>
where
    I: IntoIterator<Item = OsString>,
{
    let mut strict = false;
    let mut positional = Vec::new();

    for arg in args.into_iter().skip(1) {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => return Ok(ParseResult::Help),
            "-v" | "--version" => return Ok(ParseResult::Version),
            "--strict" => strict = true,
            flag if flag.starts_with('-') => {
                return Err(CliFailure::usage(format!("unknown option: {flag}")));
            }
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    match positional.as_slice() {
        [input_pdf, output_dir] => Ok(ParseResult::Run(CliOptions {
            input_pdf: input_pdf.clone(),
            output_dir: output_dir.clone(),
            strict,
        })),
        _ => Err(CliFailure::usage(
            "expected required arguments: <INPUT_PDF> <OUTPUT_DIR>",
        )),
    }
}

pub fn run(options: &CliOptions) -> Result<ConversionSuccess, CliFailure> {
    validate_input(&options.input_pdf)?;

    let mut warnings = collect_output_warnings(&options.output_dir)?;
    if options.strict && !warnings.is_empty() {
        return Err(strict_failure_from_warnings(&warnings));
    }

    let document = convert_pdf(&options.input_pdf)?;
    warnings.extend(document.warnings);
    let warnings = dedupe_warnings(warnings);

    if options.strict && !warnings.is_empty() {
        return Err(strict_failure_from_warnings(&warnings));
    }

    fs::create_dir_all(&options.output_dir)
        .map_err(|error| CliFailure::fatal(format_output_error(&options.output_dir, &error)))?;

    let assets_dir = options.output_dir.join("assets");
    fs::create_dir_all(&assets_dir)
        .map_err(|error| CliFailure::fatal(format_output_error(&assets_dir, &error)))?;

    for asset in document.assets {
        let asset_path = assets_dir.join(&asset.filename);
        fs::write(&asset_path, asset.bytes)
            .map_err(|error| CliFailure::fatal(format_output_error(&asset_path, &error)))?;
    }

    let main_typ = options.output_dir.join("main.typ");
    fs::write(&main_typ, document.typst)
        .map_err(|error| CliFailure::fatal(format_output_error(&main_typ, &error)))?;

    Ok(ConversionSuccess { main_typ, warnings })
}

fn validate_input(input_pdf: &Path) -> Result<(), CliFailure> {
    let metadata = fs::metadata(input_pdf).map_err(|_| {
        CliFailure::fatal(format!(
            "error: input PDF does not exist: {}",
            input_pdf.display()
        ))
    })?;

    if !metadata.is_file() {
        return Err(CliFailure::fatal(format!(
            "error: input PDF is not a file: {}",
            input_pdf.display()
        )));
    }

    Ok(())
}

fn collect_output_warnings(output_dir: &Path) -> Result<Vec<Warning>, CliFailure> {
    if !output_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(output_dir).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to inspect output directory {}: {error}",
            output_dir.display()
        ))
    })?;

    if entries.next().is_some() {
        return Ok(vec![Warning::new(format!(
            "output directory is not empty: {}",
            output_dir.display()
        ))]);
    }

    Ok(Vec::new())
}

fn format_output_error(path: &Path, error: &std::io::Error) -> String {
    format!("error: failed to write {}: {error}", path.display())
}

fn dedupe_warnings(warnings: Vec<Warning>) -> Vec<Warning> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for warning in warnings {
        if seen.insert(warning.message.clone()) {
            deduped.push(warning);
        }
    }

    deduped
}

fn strict_failure_from_warnings(warnings: &[Warning]) -> CliFailure {
    let mut message = String::new();

    for (index, warning) in warnings.iter().enumerate() {
        if index > 0 {
            message.push('\n');
        }
        message.push_str("error: ");
        message.push_str(warning.message());
    }

    CliFailure::fatal(message)
}

fn convert_pdf(input_pdf: &Path) -> Result<ConvertedDocument, CliFailure> {
    let bytes = fs::read(input_pdf).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to read input PDF {}: {error}",
            input_pdf.display()
        ))
    })?;

    let pdf = ParsedPdf::parse(&bytes)
        .map_err(|message| CliFailure::fatal(format!("error: failed to parse PDF: {message}")))?;
    let page_refs = pdf
        .page_refs()
        .map_err(|message| CliFailure::fatal(format!("error: failed to parse PDF: {message}")))?;

    if page_refs.is_empty() {
        return Err(CliFailure::fatal(
            "error: failed to parse PDF: input PDF does not contain any pages",
        ));
    }

    let mut warnings = Vec::new();
    let mut pages = Vec::with_capacity(page_refs.len());
    let mut ocr_engine = OcrEngine::from_env();
    let mut pdfkit_render_pages = HashSet::new();
    let mut raster_fallback_pages = BTreeSet::new();

    for (page_index, page_ref) in page_refs.iter().enumerate() {
        let page_number = page_index + 1;
        let content_refs = pdf.page_content_refs(*page_ref).map_err(|message| {
            CliFailure::fatal(format!(
                "error: failed to parse PDF page {page_number}: {message}"
            ))
        })?;
        let page_images = pdf.page_image_resources(*page_ref).map_err(|message| {
            CliFailure::fatal(format!(
                "error: failed to parse PDF page {page_number}: {message}"
            ))
        })?;

        let mut page_lines = Vec::new();
        let mut page_warnings = Vec::new();
        let mut page_xobjects = Vec::new();
        let mut ocr_attempted = false;
        let mut rendered_ocr_page = None;

        for content_ref in content_refs {
            match pdf.decode_content_stream(content_ref) {
                Ok(Some(stream)) => match parse_content_stream(&stream, page_number) {
                    Ok(parsed) => {
                        page_lines.extend(parsed.lines);
                        page_warnings.extend(parsed.warnings);
                        page_xobjects.extend(parsed.xobject_invocations);
                    }
                    Err(message) => page_warnings.push(Warning::new(format!(
                        "unsupported content on page {page_number}: {message}"
                    ))),
                },
                Ok(None) => page_warnings.push(Warning::new(format!(
                    "unsupported content on page {page_number}: unsupported stream filter"
                ))),
                Err(message) => page_warnings.push(Warning::new(format!(
                    "unsupported content on page {page_number}: {message}"
                ))),
            }
        }

        if page_lines.is_empty() && !page_xobjects.is_empty() && page_images.image_count > 0 {
            ocr_attempted = true;

            if page_images.candidates.is_empty() {
                page_warnings.push(Warning::new(format!(
                    "OCR unavailable on page {page_number}: embedded image encoding is unsupported; scanned page content could not be extracted"
                )));
            } else {
                rendered_ocr_page = render_page_png_for_ocr(input_pdf, page_number);
                let rendered_candidates;
                let ocr_candidates = if let Some(rendered) = rendered_ocr_page.as_ref() {
                    rendered_candidates = vec![rendered.candidate.clone()];
                    rendered_candidates.as_slice()
                } else {
                    page_images.candidates.as_slice()
                };
                let ocr_result = ocr_engine.ocr_page(page_number, ocr_candidates);
                page_lines.extend(ocr_result.lines);
                page_warnings.extend(ocr_result.warnings);
            }
        }

        assign_line_ids(&mut page_lines);
        let mut page_fragment = None;

        if ocr_attempted {
            if page_lines.is_empty() {
                if let Some(rendered) = rendered_ocr_page.as_ref() {
                    if rendered.is_blank {
                        page_fragment = Some(render_blank_page_fragment(
                            rendered.width_pt,
                            rendered.height_pt,
                        ));
                    } else {
                        page_warnings.push(Warning::new(format!(
                            "unsupported content on page {page_number}: no digital text extracted"
                        )));
                    }
                } else {
                    page_warnings.push(Warning::new(format!(
                        "unsupported content on page {page_number}: no digital text extracted"
                    )));
                }
            } else {
                page_fragment = Some(PageFragment {
                    blocks: render_text_blocks(page_lines),
                    assets: Vec::new(),
                    layout: PageLayoutMode::Flow,
                });
            }
        } else {
            let rendered = render_page(page_number, page_lines, page_xobjects, page_images);
            page_warnings.extend(rendered.warnings);
            if rendered.blocks.is_empty() {
                page_warnings.push(Warning::new(format!(
                    "unsupported content on page {page_number}: no digital text extracted"
                )));
            } else {
                page_fragment = Some(PageFragment {
                    blocks: rendered.blocks,
                    assets: rendered.assets,
                    layout: PageLayoutMode::Flow,
                });
            }
        }

        match page_recovery_for_warnings(&page_warnings) {
            Some(PageRecovery::Pdfkit) => {
                pdfkit_render_pages.insert(page_number);
                pages.push(PageAssembly {
                    number: page_number,
                    fragment: None,
                });
            }
            Some(PageRecovery::Raster) => {
                raster_fallback_pages.insert(page_number);
                pages.push(PageAssembly {
                    number: page_number,
                    fragment: None,
                });
            }
            None => {
                warnings.extend(dedupe_warnings(page_warnings));
                pages.push(PageAssembly {
                    number: page_number,
                    fragment: page_fragment,
                });
            }
        }
    }

    if !pdfkit_render_pages.is_empty() {
        let recovered_pages = convert_pdf_pages_with_pdfkit(input_pdf, &pdfkit_render_pages)?;
        for page in &mut pages {
            if page.fragment.is_some() || !pdfkit_render_pages.contains(&page.number) {
                continue;
            }

            if let Some(fragment) = recovered_pages
                .as_ref()
                .and_then(|recovered| recovered.get(&page.number))
                .cloned()
            {
                page.fragment = Some(fragment);
            } else {
                raster_fallback_pages.insert(page.number);
            }
        }
    }

    if !raster_fallback_pages.is_empty() {
        let mut rasterized_pages = rasterize_pdf_pages(input_pdf, &raster_fallback_pages)?;
        for page in &mut pages {
            if page.fragment.is_some() || !raster_fallback_pages.contains(&page.number) {
                continue;
            }

            page.fragment = Some(rasterized_pages.remove(&page.number).ok_or_else(|| {
                CliFailure::fatal(format!(
                    "error: failed to rasterize PDF page {} during fallback",
                    page.number
                ))
            })?);
        }
    }

    let mut blocks = Vec::new();
    let mut assets = Vec::new();
    let mut previous_layout = None;

    for page in pages {
        let fragment = page.fragment.ok_or_else(|| {
            CliFailure::fatal(format!(
                "error: failed to convert PDF page {} into Typst output",
                page.number
            ))
        })?;

        if should_insert_page_break(previous_layout, fragment.layout) {
            blocks.push("#pagebreak()".to_string());
        }
        assets.extend(fragment.assets);
        blocks.extend(fragment.blocks);
        previous_layout = Some(fragment.layout);
    }

    Ok(ConvertedDocument {
        typst: render_document_blocks(blocks),
        assets,
        warnings: dedupe_warnings(warnings),
    })
}

struct ConvertedDocument {
    typst: String,
    assets: Vec<OutputAsset>,
    warnings: Vec<Warning>,
}

#[derive(Clone)]
struct OutputAsset {
    filename: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PageLayoutMode {
    Flow,
    Fixed,
}

#[derive(Clone)]
struct PageFragment {
    blocks: Vec<String>,
    assets: Vec<OutputAsset>,
    layout: PageLayoutMode,
}

struct PageAssembly {
    number: usize,
    fragment: Option<PageFragment>,
}

struct RasterizedPage {
    filename: String,
    width_pt: f32,
    height_pt: f32,
}

struct RenderedOcrPage {
    candidate: OcrImageCandidate,
    width_pt: f32,
    height_pt: f32,
    is_blank: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageRecovery {
    Pdfkit,
    Raster,
}

struct PdfkitPage {
    number: usize,
    width_pt: f32,
    height_pt: f32,
    render_path: PathBuf,
    lines: Vec<PdfkitLine>,
}

struct PdfkitLine {
    x_pt: f32,
    y_pt: f32,
    width_pt: f32,
    height_pt: f32,
    font_size_pt: f32,
    font_name: String,
    text: String,
}

struct PositionedAsset {
    filename: String,
    x_pt: f32,
    y_pt: f32,
    width_pt: f32,
    height_pt: f32,
}

fn page_recovery_for_warning(message: &str) -> Option<PageRecovery> {
    if message.contains("vector drawing commands")
        || message.contains("XObject invocation")
        || message.contains("inline image data")
        || message.contains("unsupported stream filter")
    {
        Some(PageRecovery::Pdfkit)
    } else if message.contains("no digital text extracted") {
        Some(PageRecovery::Raster)
    } else {
        None
    }
}

fn page_recovery_for_warnings(warnings: &[Warning]) -> Option<PageRecovery> {
    if warnings
        .iter()
        .any(|warning| page_recovery_for_warning(warning.message()) == Some(PageRecovery::Pdfkit))
    {
        Some(PageRecovery::Pdfkit)
    } else if warnings
        .iter()
        .any(|warning| page_recovery_for_warning(warning.message()) == Some(PageRecovery::Raster))
    {
        Some(PageRecovery::Raster)
    } else {
        None
    }
}

fn should_insert_page_break(
    previous_layout: Option<PageLayoutMode>,
    current_layout: PageLayoutMode,
) -> bool {
    previous_layout.is_some_and(|layout| layout == PageLayoutMode::Fixed)
        || current_layout == PageLayoutMode::Fixed
}

fn pdfkit_helper_paths(workspace: &Path) -> (PathBuf, PathBuf) {
    (
        workspace.join("pdfkit-scene"),
        workspace.join("swift-module-cache"),
    )
}

fn convert_pdf_pages_with_pdfkit(
    input_pdf: &Path,
    render_pages: &HashSet<usize>,
) -> Result<Option<HashMap<usize, PageFragment>>, CliFailure> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let workspace = env::temp_dir().join(format!(
        "pdf-to-typst-pdfkit-{}-{timestamp}",
        std::process::id()
    ));
    fs::create_dir_all(&workspace).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to prepare PDFKit workspace {}: {error}",
            workspace.display()
        ))
    })?;

    let pages = match run_pdfkit_scene(input_pdf, &workspace, render_pages)? {
        Some(pages)
            if pages
                .iter()
                .any(|page| render_pages.contains(&page.number) && !page.lines.is_empty()) =>
        {
            pages
        }
        _ => {
            let _ = fs::remove_dir_all(&workspace);
            return Ok(None);
        }
    };

    let mut recovered_pages = HashMap::new();

    for page in pages
        .iter()
        .filter(|page| render_pages.contains(&page.number))
    {
        let positioned_assets = match detect_non_text_regions(page, &workspace)? {
            Some(assets_for_page) => assets_for_page,
            None => {
                let _ = fs::remove_dir_all(&workspace);
                return Ok(None);
            }
        };

        let mut assets = Vec::new();
        let mut blocks = Vec::new();
        blocks.push(format!(
            "#set page(width: {}, height: {}, margin: 0pt)",
            format_pt(page.width_pt),
            format_pt(page.height_pt)
        ));

        for positioned in positioned_assets {
            let bytes = fs::read(workspace.join(&positioned.filename)).map_err(|error| {
                CliFailure::fatal(format!(
                    "error: failed to read extracted region {}: {error}",
                    positioned.filename
                ))
            })?;
            assets.push(OutputAsset {
                filename: positioned.filename.clone(),
                bytes,
            });
            blocks.push(render_positioned_asset(&positioned));
        }

        for line in &page.lines {
            blocks.push(render_positioned_line(page, line));
        }

        recovered_pages.insert(
            page.number,
            PageFragment {
                blocks,
                assets,
                layout: PageLayoutMode::Fixed,
            },
        );
    }

    let _ = fs::remove_dir_all(&workspace);

    Ok(Some(recovered_pages))
}

fn run_pdfkit_scene(
    input_pdf: &Path,
    workspace: &Path,
    render_pages: &HashSet<usize>,
) -> Result<Option<Vec<PdfkitPage>>, CliFailure> {
    let helper_source = tools_dir().join("pdfkit_scene.swift");
    let (helper_binary, module_cache) = pdfkit_helper_paths(workspace);
    let render_dir = workspace.join("renders");
    fs::create_dir_all(&render_dir).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to prepare PDFKit render directory {}: {error}",
            render_dir.display()
        ))
    })?;
    fs::create_dir_all(&module_cache).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to prepare Swift module cache {}: {error}",
            module_cache.display()
        ))
    })?;

    let helper_needs_build = match (fs::metadata(&helper_binary), fs::metadata(&helper_source)) {
        (Ok(binary_meta), Ok(source_meta)) => {
            match (binary_meta.modified(), source_meta.modified()) {
                (Ok(binary_mtime), Ok(source_mtime)) => binary_mtime < source_mtime,
                _ => true,
            }
        }
        _ => true,
    };

    if helper_needs_build {
        let sdk_output = match Command::new("xcrun").arg("--show-sdk-path").output() {
            Ok(output) if output.status.success() => output,
            _ => return Ok(None),
        };
        let sdk_path = String::from_utf8_lossy(&sdk_output.stdout)
            .trim()
            .to_string();
        if sdk_path.is_empty() {
            return Ok(None);
        }

        let compile = match Command::new("xcrun")
            .arg("swiftc")
            .arg("-sdk")
            .arg(&sdk_path)
            .arg("-module-cache-path")
            .arg(&module_cache)
            .arg(&helper_source)
            .arg("-o")
            .arg(&helper_binary)
            .output()
        {
            Ok(output) => output,
            Err(_) => return Ok(None),
        };

        if !compile.status.success() {
            return Ok(None);
        }
    }

    let output = match Command::new(&helper_binary)
        .arg(input_pdf)
        .arg(&render_dir)
        .arg(PDFKIT_RENDER_SCALE.to_string())
        .arg(
            render_pages
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .map(|page| page.to_string())
                .collect::<Vec<_>>()
                .join(","),
        )
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    if !output.status.success() {
        return Ok(None);
    }

    let mut pages = Vec::<PdfkitPage>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split('\t');
        match parts.next() {
            Some("PAGE") => {
                let number = parts.next().and_then(|value| value.parse::<usize>().ok());
                let width_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let height_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let render_path = parts.next().map(unescape_helper_field);
                let (Some(number), Some(width_pt), Some(height_pt), Some(render_path)) =
                    (number, width_pt, height_pt, render_path)
                else {
                    continue;
                };
                pages.push(PdfkitPage {
                    number,
                    width_pt,
                    height_pt,
                    render_path: PathBuf::from(render_path),
                    lines: Vec::new(),
                });
            }
            Some("LINE") => {
                let number = parts.next().and_then(|value| value.parse::<usize>().ok());
                let x_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let y_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let width_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let height_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let font_size_pt = parts.next().and_then(|value| value.parse::<f32>().ok());
                let font_name = parts.next().map(unescape_helper_field);
                let text = parts.next().map(unescape_helper_field);
                let (
                    Some(number),
                    Some(x_pt),
                    Some(y_pt),
                    Some(width_pt),
                    Some(height_pt),
                    Some(font_size_pt),
                    Some(font_name),
                    Some(text),
                ) = (
                    number,
                    x_pt,
                    y_pt,
                    width_pt,
                    height_pt,
                    font_size_pt,
                    font_name,
                    text,
                )
                else {
                    continue;
                };

                if let Some(page) = pages.iter_mut().find(|page| page.number == number) {
                    page.lines.push(PdfkitLine {
                        x_pt,
                        y_pt,
                        width_pt,
                        height_pt,
                        font_size_pt,
                        font_name,
                        text,
                    });
                }
            }
            _ => {}
        }
    }

    Ok((!pages.is_empty()).then_some(pages))
}

fn detect_non_text_regions(
    page: &PdfkitPage,
    workspace: &Path,
) -> Result<Option<Vec<PositionedAsset>>, CliFailure> {
    if page.render_path.as_os_str().is_empty() {
        return Ok(Some(Vec::new()));
    }

    let boxes_path = workspace.join(format!("page-{:04}-boxes.tsv", page.number));
    let regions_dir = workspace.join(format!("regions-page-{:04}", page.number));
    fs::create_dir_all(&regions_dir).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to prepare region directory {}: {error}",
            regions_dir.display()
        ))
    })?;

    let mut box_lines = String::new();
    for line in &page.lines {
        let left = (line.x_pt * PDFKIT_RENDER_SCALE).round().max(0.0) as i32;
        let top = ((page.height_pt - line.y_pt - line.height_pt) * PDFKIT_RENDER_SCALE)
            .round()
            .max(0.0) as i32;
        let width = (line.width_pt * PDFKIT_RENDER_SCALE).round().max(1.0) as i32;
        let height = (line.height_pt * PDFKIT_RENDER_SCALE).round().max(1.0) as i32;
        box_lines.push_str(&format!("{left}\t{top}\t{width}\t{height}\n"));
    }
    fs::write(&boxes_path, box_lines).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to write text box mask {}: {error}",
            boxes_path.display()
        ))
    })?;

    let helper = tools_dir().join("extract_non_text_regions.py");
    let output = match Command::new("python3")
        .arg(&helper)
        .arg(&page.render_path)
        .arg(&boxes_path)
        .arg(&regions_dir)
        .arg(format!("page-{:04}", page.number))
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    if !output.status.success() {
        return Ok(None);
    }

    let mut assets = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split('\t');
        if parts.next() != Some("REGION") {
            continue;
        }

        let left_px = parts.next().and_then(|value| value.parse::<f32>().ok());
        let top_px = parts.next().and_then(|value| value.parse::<f32>().ok());
        let width_px = parts.next().and_then(|value| value.parse::<f32>().ok());
        let height_px = parts.next().and_then(|value| value.parse::<f32>().ok());
        let path = parts.next();
        let (Some(left_px), Some(top_px), Some(width_px), Some(height_px), Some(path)) =
            (left_px, top_px, width_px, height_px, path)
        else {
            continue;
        };

        let filename = Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("region.png")
            .to_string();
        assets.push(PositionedAsset {
            filename,
            x_pt: left_px / PDFKIT_RENDER_SCALE,
            y_pt: top_px / PDFKIT_RENDER_SCALE,
            width_pt: width_px / PDFKIT_RENDER_SCALE,
            height_pt: height_px / PDFKIT_RENDER_SCALE,
        });
    }

    Ok(Some(assets))
}

fn unescape_helper_field(field: &str) -> String {
    let mut output = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }

        match chars.next() {
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('\\') => output.push('\\'),
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => output.push('\\'),
        }
    }
    output
}

fn rasterize_pdf_pages(
    input_pdf: &Path,
    page_numbers: &BTreeSet<usize>,
) -> Result<HashMap<usize, PageFragment>, CliFailure> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = env::temp_dir().join(format!(
        "pdf-to-typst-page-render-{}-{timestamp}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).map_err(|error| {
        CliFailure::fatal(format!(
            "error: failed to prepare page raster fallback workspace {}: {error}",
            temp_dir.display()
        ))
    })?;

    let ghostscript = env::var_os("PDF_TO_TYPST_GS_BIN").unwrap_or_else(|| OsString::from("gs"));
    let mut recovered_pages = HashMap::new();

    for page_number in page_numbers {
        let output_path = temp_dir.join(format!("page-{page_number:04}.png"));
        let output = Command::new(&ghostscript)
            .arg("-q")
            .arg("-dSAFER")
            .arg("-dBATCH")
            .arg("-dNOPAUSE")
            .arg("-sDEVICE=png16m")
            .arg(format!("-r{RASTER_FALLBACK_DPI}"))
            .arg(format!("-dFirstPage={page_number}"))
            .arg(format!("-dLastPage={page_number}"))
            .arg("-o")
            .arg(&output_path)
            .arg(input_pdf)
            .output()
            .map_err(|error| {
                CliFailure::fatal(format!(
                    "error: failed to launch Ghostscript raster fallback: {error}"
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(CliFailure::fatal(if detail.is_empty() {
                format!("error: Ghostscript raster fallback failed for page {page_number}")
            } else {
                format!(
                    "error: Ghostscript raster fallback failed for page {page_number}: {detail}"
                )
            }));
        }

        let bytes = fs::read(&output_path).map_err(|error| {
            CliFailure::fatal(format!(
                "error: failed to read rasterized page {}: {error}",
                output_path.display()
            ))
        })?;
        let (width_px, height_px) = parse_png_dimensions(&bytes).map_err(|message| {
            CliFailure::fatal(format!(
                "error: failed to inspect rasterized page {}: {message}",
                output_path.display()
            ))
        })?;
        let filename = format!("page-{page_number:04}.png");
        let rasterized = RasterizedPage {
            filename: filename.clone(),
            width_pt: width_px as f32 * 72.0 / RASTER_FALLBACK_DPI as f32,
            height_pt: height_px as f32 * 72.0 / RASTER_FALLBACK_DPI as f32,
        };

        recovered_pages.insert(
            *page_number,
            PageFragment {
                blocks: render_raster_page_blocks(&rasterized),
                assets: vec![OutputAsset { filename, bytes }],
                layout: PageLayoutMode::Fixed,
            },
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);

    Ok(recovered_pages)
}

fn render_page_png_for_ocr(input_pdf: &Path, page_number: usize) -> Option<RenderedOcrPage> {
    let workspace = ocr_temp_workspace().ok()?;
    let path = workspace.join(format!(
        "pdf-to-typst-ocr-page-{}-{page_number}.png",
        std::process::id()
    ));
    let ghostscript = env::var_os("PDF_TO_TYPST_GS_BIN").unwrap_or_else(|| OsString::from("gs"));
    let output = Command::new(&ghostscript)
        .arg("-q")
        .arg("-dSAFER")
        .arg("-dBATCH")
        .arg("-dNOPAUSE")
        .arg("-sDEVICE=pnggray")
        .arg("-r300")
        .arg(format!("-dFirstPage={page_number}"))
        .arg(format!("-dLastPage={page_number}"))
        .arg("-o")
        .arg(&path)
        .arg(input_pdf)
        .output()
        .ok()?;

    if !output.status.success() {
        cleanup_temp_ocr_image(&path);
        return None;
    }

    let bytes = fs::read(&path).ok()?;
    let (width, height) = parse_png_dimensions(&bytes).ok()?;
    let is_blank = detect_blank_rendered_page(&path).unwrap_or(false);
    cleanup_temp_ocr_image(&path);

    Some(RenderedOcrPage {
        candidate: OcrImageCandidate {
            width: width as usize,
            height: height as usize,
            extension: "png",
            bytes,
        },
        width_pt: width as f32 * 72.0 / 300.0,
        height_pt: height as f32 * 72.0 / 300.0,
        is_blank,
    })
}

fn detect_blank_rendered_page(path: &Path) -> Option<bool> {
    let output = Command::new("magick")
        .arg(path)
        .arg("-colorspace")
        .arg("gray")
        .arg("-threshold")
        .arg("95%")
        .arg("-negate")
        .arg("-format")
        .arg("%[fx:mean]")
        .arg("info:")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let mean = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f32>()
        .ok()?;
    Some(mean <= 0.01)
}

fn render_blank_page_fragment(width_pt: f32, height_pt: f32) -> PageFragment {
    PageFragment {
        blocks: vec![format!(
            "#set page(width: {}, height: {}, margin: 0pt)",
            format_pt(width_pt),
            format_pt(height_pt)
        )],
        assets: Vec::new(),
        layout: PageLayoutMode::Fixed,
    }
}

fn parse_png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE {
        return Err("not a PNG image".to_string());
    }

    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 {
        return Err("PNG image has invalid dimensions".to_string());
    }

    Ok((width, height))
}

fn render_raster_page_blocks(page: &RasterizedPage) -> Vec<String> {
    let width = format_pt(page.width_pt);
    let height = format_pt(page.height_pt);
    vec![
        format!("#set page(width: {width}, height: {height}, margin: 0pt)"),
        format!(
            "#image(\"assets/{}\", width: {width}, height: {height})",
            page.filename
        ),
    ]
}

fn format_pt(value: f32) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let mut text = format!("{rounded:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text.push_str("pt");
    text
}

fn render_positioned_asset(asset: &PositionedAsset) -> String {
    format!(
        "#place(left + top, dx: {}, dy: {})[#image(\"assets/{}\", width: {}, height: {})]",
        format_pt(asset.x_pt),
        format_pt(asset.y_pt),
        asset.filename,
        format_pt(asset.width_pt),
        format_pt(asset.height_pt)
    )
}

fn render_positioned_line(page: &PdfkitPage, line: &PdfkitLine) -> String {
    let top = page.height_pt - line.y_pt - line.height_pt;
    let font_size = if line.font_size_pt.is_finite() && line.font_size_pt > 0.0 {
        line.font_size_pt
    } else {
        line.height_pt.max(8.0)
    };
    let body = typst_string_literal(&line.text);
    let x = format_pt(line.x_pt);
    let y = format_pt(top.max(0.0));
    let size = format_pt(font_size);

    if let Some(font) = map_typst_font_name(&line.font_name) {
        format!(
            "#place(left + top, dx: {x}, dy: {y})[#text(size: {size}, font: \"{font}\", \"{body}\")]"
        )
    } else {
        format!("#place(left + top, dx: {x}, dy: {y})[#text(size: {size}, \"{body}\")]")
    }
}

fn map_typst_font_name(font_name: &str) -> Option<&'static str> {
    let lower = font_name.to_ascii_lowercase();
    if lower.contains("times") {
        Some("Times New Roman")
    } else if lower.contains("helvetica") {
        Some("Helvetica")
    } else if lower.contains("courier") {
        Some("Courier New")
    } else if lower.contains("arial") {
        Some("Arial")
    } else {
        None
    }
}

fn typst_string_literal(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

struct ParsedPdf {
    objects: HashMap<u32, PdfObject>,
}

struct PdfObject {
    dictionary: String,
    stream: Option<Vec<u8>>,
}

struct PageImageResources {
    image_count: usize,
    resources: HashMap<String, PageImageResource>,
    candidates: Vec<OcrImageCandidate>,
}

struct PageImageResource {
    name: String,
    asset: Option<ImageAssetCandidate>,
    asset_issue: Option<String>,
    ocr_candidate: Option<OcrImageCandidate>,
}

#[derive(Clone)]
struct OcrImageCandidate {
    width: usize,
    height: usize,
    extension: &'static str,
    bytes: Vec<u8>,
}

struct ImageAssetCandidate {
    extension: &'static str,
    bytes: Vec<u8>,
}

impl ParsedPdf {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut objects = HashMap::new();
        let mut cursor = 0;

        while cursor < bytes.len() {
            if !is_line_start(bytes, cursor) {
                cursor += 1;
                continue;
            }

            let Some((object_id, body_start)) = parse_object_header(bytes, cursor) else {
                cursor += 1;
                continue;
            };

            let endobj = find_line_token(bytes, body_start, b"endobj")
                .ok_or_else(|| format!("unterminated object {object_id}"))?;
            let body = &bytes[body_start..endobj];
            let (dictionary, stream) = split_stream(body)?;

            objects.insert(
                object_id,
                PdfObject {
                    dictionary: String::from_utf8_lossy(dictionary).into_owned(),
                    stream,
                },
            );

            cursor = endobj + "endobj".len();
        }

        if objects.is_empty() {
            return Err("no PDF objects found".to_string());
        }

        Ok(Self { objects })
    }

    fn object(&self, object_id: u32) -> Result<&PdfObject, String> {
        self.objects
            .get(&object_id)
            .ok_or_else(|| format!("missing object {object_id}"))
    }

    fn page_refs(&self) -> Result<Vec<u32>, String> {
        let catalog_id = self
            .objects
            .iter()
            .find_map(|(object_id, object)| {
                dictionary_has_name_value(&object.dictionary, "/Type", "Catalog")
                    .then_some(*object_id)
            })
            .ok_or_else(|| "catalog object not found".to_string())?;
        let catalog = self.object(catalog_id)?;
        let pages_ref = extract_reference_value(&catalog.dictionary, "/Pages")
            .ok_or_else(|| "catalog is missing /Pages".to_string())?;

        let mut page_refs = Vec::new();
        self.collect_page_refs(pages_ref, &mut page_refs)?;
        Ok(page_refs)
    }

    fn collect_page_refs(&self, object_id: u32, page_refs: &mut Vec<u32>) -> Result<(), String> {
        let object = self.object(object_id)?;

        if dictionary_has_name_value(&object.dictionary, "/Type", "Pages") {
            let kids = extract_reference_list_value(&object.dictionary, "/Kids")
                .ok_or_else(|| format!("page tree node {object_id} is missing /Kids"))?;

            for kid in kids {
                self.collect_page_refs(kid, page_refs)?;
            }

            return Ok(());
        }

        if dictionary_has_name_value(&object.dictionary, "/Type", "Page") {
            page_refs.push(object_id);
            return Ok(());
        }

        Err(format!("object {object_id} is not a page node"))
    }

    fn page_content_refs(&self, object_id: u32) -> Result<Vec<u32>, String> {
        let object = self.object(object_id)?;

        extract_reference_list_or_single(&object.dictionary, "/Contents")
            .ok_or_else(|| format!("page {object_id} is missing /Contents"))
    }

    fn decode_content_stream(&self, object_id: u32) -> Result<Option<Vec<u8>>, String> {
        let object = self.object(object_id)?;
        let Some(stream) = &object.stream else {
            return Err(format!("object {object_id} does not contain a stream"));
        };

        if !object.dictionary.contains("/Filter") {
            return Ok(Some(stream.clone()));
        }

        if !dictionary_has_name_value(&object.dictionary, "/Filter", "FlateDecode") {
            return Ok(None);
        }

        let mut decoder = ZlibDecoder::new(stream.as_slice());
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .map_err(|error| format!("failed to decompress stream {object_id}: {error}"))?;

        Ok(Some(decoded))
    }

    fn page_image_resources(&self, object_id: u32) -> Result<PageImageResources, String> {
        let page = self.object(object_id)?;
        let Some(resources_value) =
            extract_dictionary_or_reference_value(&page.dictionary, "/Resources")
        else {
            return Ok(PageImageResources {
                image_count: 0,
                resources: HashMap::new(),
                candidates: Vec::new(),
            });
        };
        let resources = self.resolve_dictionary_value(resources_value)?;
        let Some(xobject_value) = extract_dictionary_or_reference_value(resources, "/XObject")
        else {
            return Ok(PageImageResources {
                image_count: 0,
                resources: HashMap::new(),
                candidates: Vec::new(),
            });
        };
        let xobjects = self.resolve_dictionary_value(xobject_value)?;
        let refs = parse_named_reference_map(xobjects);
        let mut image_count = 0usize;
        let mut resources = HashMap::new();
        let mut candidates = Vec::new();

        for (name, image_ref) in refs {
            let object = self.object(image_ref)?;
            if !dictionary_has_name_value(&object.dictionary, "/Subtype", "Image") {
                continue;
            }

            image_count += 1;

            let resource = self.image_resource(name.clone(), image_ref)?;
            if let Some(candidate) = resource.ocr_candidate.as_ref() {
                candidates.push(candidate.clone());
            }
            resources.insert(name, resource);
        }

        Ok(PageImageResources {
            image_count,
            resources,
            candidates,
        })
    }

    fn resolve_dictionary_value<'a>(
        &'a self,
        value: DictionaryValue<'a>,
    ) -> Result<&'a str, String> {
        match value {
            DictionaryValue::Inline(dictionary) => Ok(dictionary),
            DictionaryValue::Reference(object_id) => {
                Ok(self.object(object_id)?.dictionary.as_str())
            }
        }
    }

    fn image_resource(&self, name: String, object_id: u32) -> Result<PageImageResource, String> {
        let object = self.object(object_id)?;
        let Some(stream) = &object.stream else {
            return Ok(PageImageResource {
                name,
                asset: None,
                asset_issue: Some("image stream is missing".to_string()),
                ocr_candidate: None,
            });
        };

        let width = extract_usize_value(&object.dictionary, "/Width")
            .ok_or_else(|| format!("image object {object_id} is missing /Width"))?;
        let height = extract_usize_value(&object.dictionary, "/Height")
            .ok_or_else(|| format!("image object {object_id} is missing /Height"))?;

        if dictionary_has_name_value(&object.dictionary, "/Filter", "CCITTFaxDecode") {
            let bits = extract_usize_value(&object.dictionary, "/BitsPerComponent").unwrap_or(1);
            if bits != 1 {
                return Ok(PageImageResource {
                    name,
                    asset: None,
                    asset_issue: Some(format!(
                        "unsupported image bit depth {bits}; only 1-bit CCITT scans are supported for OCR"
                    )),
                    ocr_candidate: None,
                });
            }

            let Some(color_space) = extract_name_value(&object.dictionary, "/ColorSpace") else {
                return Ok(PageImageResource {
                    name,
                    asset: None,
                    asset_issue: Some("image color space is missing".to_string()),
                    ocr_candidate: None,
                });
            };

            if color_space != "DeviceGray" {
                return Ok(PageImageResource {
                    name,
                    asset: None,
                    asset_issue: Some(format!(
                        "unsupported image color space {color_space}; CCITT OCR only supports DeviceGray"
                    )),
                    ocr_candidate: None,
                });
            }

            let Some(decode_params_value) =
                extract_dictionary_or_reference_value(&object.dictionary, "/DecodeParms")
            else {
                return Ok(PageImageResource {
                    name,
                    asset: None,
                    asset_issue: Some("CCITT image is missing decode parameters".to_string()),
                    ocr_candidate: None,
                });
            };
            let decode_params = self.resolve_dictionary_value(decode_params_value)?;
            let compression = match extract_i32_value(decode_params, "/K").unwrap_or(0) {
                value if value < 0 => 4u16,
                _ => {
                    return Ok(PageImageResource {
                        name,
                        asset: None,
                        asset_issue: Some(
                            "unsupported CCITT compression; only Group 4 images are supported for OCR"
                                .to_string(),
                        ),
                        ocr_candidate: None,
                    });
                }
            };
            let black_is_1 = extract_bool_value(decode_params, "/BlackIs1").unwrap_or(false);

            return Ok(PageImageResource {
                name,
                asset: None,
                asset_issue: Some(
                    "CCITT image can be OCRed but cannot be emitted as a Typst asset".to_string(),
                ),
                ocr_candidate: Some(OcrImageCandidate {
                    width,
                    height,
                    extension: "tiff",
                    bytes: encode_ccitt_tiff(width, height, compression, black_is_1, stream),
                }),
            });
        }

        if object.dictionary.contains("/DecodeParms")
            || dictionary_value_starts_with(&object.dictionary, "/Filter", "[")
        {
            return Ok(PageImageResource {
                name,
                asset: None,
                asset_issue: Some("unsupported image decode parameters".to_string()),
                ocr_candidate: None,
            });
        }

        if dictionary_has_name_value(&object.dictionary, "/Filter", "DCTDecode") {
            let bytes = stream.clone();
            return Ok(PageImageResource {
                name,
                asset: Some(ImageAssetCandidate {
                    extension: "jpg",
                    bytes: bytes.clone(),
                }),
                asset_issue: None,
                ocr_candidate: Some(OcrImageCandidate {
                    width,
                    height,
                    extension: "jpg",
                    bytes,
                }),
            });
        }

        let decoded = if object.dictionary.contains("/Filter") {
            if !dictionary_has_name_value(&object.dictionary, "/Filter", "FlateDecode") {
                return Ok(PageImageResource {
                    name,
                    asset: None,
                    asset_issue: Some("unsupported image filter".to_string()),
                    ocr_candidate: None,
                });
            }

            decode_flate_bytes(stream, object_id)?
        } else {
            stream.clone()
        };

        let bits = extract_usize_value(&object.dictionary, "/BitsPerComponent").unwrap_or(8);
        if bits != 8 {
            return Ok(PageImageResource {
                name,
                asset: None,
                asset_issue: Some(format!(
                    "unsupported image bit depth {bits}; only 8-bit images are supported"
                )),
                ocr_candidate: None,
            });
        }

        let Some(color_space) = extract_name_value(&object.dictionary, "/ColorSpace") else {
            return Ok(PageImageResource {
                name,
                asset: None,
                asset_issue: Some("image color space is missing".to_string()),
                ocr_candidate: None,
            });
        };

        let asset = build_image_asset_candidate(color_space, width, height, &decoded);
        let ocr_candidate = build_pnm_candidate(color_space, width, height, decoded);
        let asset_issue = if asset.is_none() {
            Some(format!(
                "unsupported image color space {color_space}; only DeviceGray and DeviceRGB are supported"
            ))
        } else {
            None
        };

        Ok(PageImageResource {
            name,
            asset,
            asset_issue,
            ocr_candidate,
        })
    }
}

enum DictionaryValue<'a> {
    Inline(&'a str),
    Reference(u32),
}

fn decode_flate_bytes(stream: &[u8], object_id: u32) -> Result<Vec<u8>, String> {
    let mut decoder = ZlibDecoder::new(stream);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .map_err(|error| format!("failed to decompress stream {object_id}: {error}"))?;
    Ok(decoded)
}

fn build_pnm_candidate(
    color_space: &str,
    width: usize,
    height: usize,
    decoded: Vec<u8>,
) -> Option<OcrImageCandidate> {
    let (magic, expected_len, extension) = match color_space {
        "DeviceGray" => ("P5", width.saturating_mul(height), "pgm"),
        "DeviceRGB" => ("P6", width.saturating_mul(height).saturating_mul(3), "ppm"),
        _ => return None,
    };

    if decoded.len() != expected_len {
        return None;
    }

    let mut bytes = format!("{magic}\n{width} {height}\n255\n").into_bytes();
    bytes.extend_from_slice(&decoded);

    Some(OcrImageCandidate {
        width,
        height,
        extension,
        bytes,
    })
}

fn build_image_asset_candidate(
    color_space: &str,
    width: usize,
    height: usize,
    decoded: &[u8],
) -> Option<ImageAssetCandidate> {
    let color_type = match color_space {
        "DeviceGray" => 0,
        "DeviceRGB" => 2,
        _ => return None,
    };

    let channels = if color_type == 0 { 1 } else { 3 };
    let expected_len = width.saturating_mul(height).saturating_mul(channels);
    if decoded.len() != expected_len {
        return None;
    }

    Some(ImageAssetCandidate {
        extension: "png",
        bytes: encode_png(width, height, color_type, decoded),
    })
}

fn encode_png(width: usize, height: usize, color_type: u8, pixels: &[u8]) -> Vec<u8> {
    let channels = if color_type == 0 { 1usize } else { 3usize };
    let row_len = width.saturating_mul(channels);
    let mut filtered = Vec::with_capacity(height.saturating_mul(row_len + 1));

    for row in pixels.chunks(row_len) {
        filtered.push(0);
        filtered.extend_from_slice(row);
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(&filtered);
    let compressed = encoder.finish().unwrap_or_default();

    let mut png = Vec::new();
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.push(8);
    ihdr.push(color_type);
    ihdr.push(0);
    ihdr.push(0);
    ihdr.push(0);
    write_png_chunk(&mut png, b"IHDR", &ihdr);
    write_png_chunk(&mut png, b"IDAT", &compressed);
    write_png_chunk(&mut png, b"IEND", &[]);

    png
}

fn write_png_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(chunk_type);
    png.extend_from_slice(data);

    let mut crc_input = Vec::with_capacity(chunk_type.len() + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;

    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }

    !crc
}

fn encode_ccitt_tiff(
    width: usize,
    height: usize,
    compression: u16,
    black_is_1: bool,
    compressed_bytes: &[u8],
) -> Vec<u8> {
    const TYPE_SHORT: u16 = 3;
    const TYPE_LONG: u16 = 4;

    let entry_count = 9u16;
    let ifd_offset = 8u32;
    let strip_offset = ifd_offset + 2 + u32::from(entry_count) * 12 + 4;
    let photometric = if black_is_1 { 1u16 } else { 0u16 };
    let mut tiff = Vec::with_capacity(strip_offset as usize + compressed_bytes.len());

    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42u16.to_le_bytes());
    tiff.extend_from_slice(&ifd_offset.to_le_bytes());
    tiff.extend_from_slice(&entry_count.to_le_bytes());

    write_tiff_ifd_entry(&mut tiff, 256, TYPE_LONG, 1, width as u32);
    write_tiff_ifd_entry(&mut tiff, 257, TYPE_LONG, 1, height as u32);
    write_tiff_ifd_entry(&mut tiff, 258, TYPE_SHORT, 1, 1);
    write_tiff_ifd_entry(&mut tiff, 259, TYPE_SHORT, 1, u32::from(compression));
    write_tiff_ifd_entry(&mut tiff, 262, TYPE_SHORT, 1, u32::from(photometric));
    write_tiff_ifd_entry(&mut tiff, 266, TYPE_SHORT, 1, 1);
    write_tiff_ifd_entry(&mut tiff, 273, TYPE_LONG, 1, strip_offset);
    write_tiff_ifd_entry(&mut tiff, 278, TYPE_LONG, 1, height as u32);
    write_tiff_ifd_entry(&mut tiff, 279, TYPE_LONG, 1, compressed_bytes.len() as u32);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(compressed_bytes);

    tiff
}

fn write_tiff_ifd_entry(tiff: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32) {
    tiff.extend_from_slice(&tag.to_le_bytes());
    tiff.extend_from_slice(&field_type.to_le_bytes());
    tiff.extend_from_slice(&count.to_le_bytes());
    tiff.extend_from_slice(&value.to_le_bytes());
}

fn is_line_start(bytes: &[u8], index: usize) -> bool {
    index == 0 || bytes[index - 1] == b'\n' || bytes[index - 1] == b'\r'
}

fn parse_object_header(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    let (object_id, mut cursor) = parse_unsigned_integer(bytes, start)?;
    cursor = skip_inline_whitespace(bytes, cursor);
    let (_, next_cursor) = parse_unsigned_integer(bytes, cursor)?;
    cursor = skip_inline_whitespace(bytes, next_cursor);

    if !bytes.get(cursor..)?.starts_with(b"obj") {
        return None;
    }

    cursor += 3;
    cursor = skip_inline_whitespace(bytes, cursor);

    if bytes.get(cursor) == Some(&b'\r') {
        cursor += 1;
        if bytes.get(cursor) == Some(&b'\n') {
            cursor += 1;
        }
    } else if bytes.get(cursor) == Some(&b'\n') {
        cursor += 1;
    }

    Some((object_id, cursor))
}

fn parse_unsigned_integer(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    let mut cursor = start;

    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }

    let number_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }

    if cursor == number_start {
        return None;
    }

    let value = std::str::from_utf8(&bytes[number_start..cursor])
        .ok()?
        .parse()
        .ok()?;

    Some((value, cursor))
}

fn skip_inline_whitespace(bytes: &[u8], mut cursor: usize) -> usize {
    while let Some(byte) = bytes.get(cursor) {
        if !matches!(byte, b' ' | b'\t') {
            break;
        }

        cursor += 1;
    }

    cursor
}

fn find_line_token(bytes: &[u8], start: usize, token: &[u8]) -> Option<usize> {
    let mut cursor = start;

    while cursor + token.len() <= bytes.len() {
        if bytes[cursor..].starts_with(token) && is_line_start(bytes, cursor) {
            return Some(cursor);
        }

        cursor += 1;
    }

    None
}

fn split_stream(body: &[u8]) -> Result<(&[u8], Option<Vec<u8>>), String> {
    let Some(stream_start) = find_bytes(body, b"stream") else {
        return Ok((trim_ascii(body), None));
    };

    let dictionary = trim_ascii(&body[..stream_start]);
    let mut data_start = stream_start + "stream".len();

    if body.get(data_start) == Some(&b'\r') {
        data_start += 1;
        if body.get(data_start) == Some(&b'\n') {
            data_start += 1;
        }
    } else if body.get(data_start) == Some(&b'\n') {
        data_start += 1;
    }

    let endstream = find_bytes(&body[data_start..], b"endstream")
        .map(|offset| data_start + offset)
        .ok_or_else(|| "unterminated stream".to_string())?;

    let mut stream = body[data_start..endstream].to_vec();
    while stream
        .last()
        .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
    {
        stream.pop();
    }

    Ok((dictionary, Some(stream)))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();

    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }

    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }

    &bytes[start..end]
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn dictionary_has_name_value(dictionary: &str, key: &str, value: &str) -> bool {
    dictionary
        .split_once(key)
        .map(|(_, remainder)| remainder.trim_start())
        .and_then(|remainder| remainder.strip_prefix('/'))
        .is_some_and(|remainder| remainder.starts_with(value))
}

fn dictionary_value_starts_with(dictionary: &str, key: &str, prefix: &str) -> bool {
    dictionary
        .split_once(key)
        .map(|(_, remainder)| remainder.trim_start())
        .is_some_and(|remainder| remainder.starts_with(prefix))
}

fn extract_reference_value(dictionary: &str, key: &str) -> Option<u32> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    parse_reference(remainder).map(|(reference, _)| reference)
}

fn extract_dictionary_or_reference_value<'a>(
    dictionary: &'a str,
    key: &str,
) -> Option<DictionaryValue<'a>> {
    let remainder = dictionary.split_once(key)?.1.trim_start();

    if remainder.starts_with("<<") {
        return extract_dictionary(remainder).map(DictionaryValue::Inline);
    }

    parse_reference(remainder).map(|(reference, _)| DictionaryValue::Reference(reference))
}

fn extract_reference_list_value(dictionary: &str, key: &str) -> Option<Vec<u32>> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    let array = extract_array(remainder)?;
    Some(parse_references(array))
}

fn extract_reference_list_or_single(dictionary: &str, key: &str) -> Option<Vec<u32>> {
    let remainder = dictionary.split_once(key)?.1.trim_start();

    if remainder.starts_with('[') {
        return Some(parse_references(extract_array(remainder)?));
    }

    Some(vec![parse_reference(remainder)?.0])
}

fn extract_array(input: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut start = None;

    for (index, ch) in input.char_indices() {
        if ch == '[' {
            if depth == 0 {
                start = Some(index + 1);
            }
            depth += 1;
        } else if ch == ']' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                let array_start = start?;
                return Some(&input[array_start..index]);
            }
        }
    }

    None
}

fn extract_dictionary(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    let mut start = None;

    while cursor + 1 < bytes.len() {
        let pair = &bytes[cursor..cursor + 2];

        if pair == b"<<" {
            if depth == 0 {
                start = Some(cursor + 2);
            }
            depth += 1;
            cursor += 2;
            continue;
        }

        if pair == b">>" {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                let dictionary_start = start?;
                return Some(&input[dictionary_start..cursor]);
            }
            cursor += 2;
            continue;
        }

        cursor += 1;
    }

    None
}

fn parse_references(input: &str) -> Vec<u32> {
    let mut references = Vec::new();
    let bytes = input.as_bytes();
    let mut cursor = 0;

    while cursor < bytes.len() {
        if let Some((reference, next_cursor)) = parse_reference(&input[cursor..]) {
            references.push(reference);
            cursor += next_cursor;
        } else {
            cursor += 1;
        }
    }

    references
}

fn parse_reference(input: &str) -> Option<(u32, usize)> {
    let bytes = input.as_bytes();
    let (object_id, mut cursor) = parse_unsigned_integer(bytes, 0)?;

    if cursor == 0 {
        return None;
    }

    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }

    let (_, next_cursor) = parse_unsigned_integer(bytes, cursor)?;
    cursor = next_cursor;

    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }

    if bytes.get(cursor) != Some(&b'R') {
        return None;
    }

    Some((object_id, cursor + 1))
}

fn parse_named_reference_map(input: &str) -> HashMap<String, u32> {
    let mut mapping = HashMap::new();
    let bytes = input.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }

        if bytes.get(cursor) != Some(&b'/') {
            cursor += 1;
            continue;
        }

        cursor += 1;
        let name_start = cursor;
        while let Some(byte) = bytes.get(cursor).copied() {
            if byte.is_ascii_whitespace() || is_delimiter(byte) {
                break;
            }
            cursor += 1;
        }

        let name = &input[name_start..cursor];
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }

        if let Some((reference, consumed)) = parse_reference(&input[cursor..]) {
            mapping.insert(name.to_string(), reference);
            cursor += consumed;
        }
    }

    mapping
}

fn extract_name_value<'a>(dictionary: &'a str, key: &str) -> Option<&'a str> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    let stripped = remainder.strip_prefix('/')?;
    let end = stripped
        .find(|ch: char| ch.is_whitespace() || matches!(ch, '/' | '[' | ']' | '<' | '>'))
        .unwrap_or(stripped.len());
    Some(&stripped[..end])
}

fn extract_bool_value(dictionary: &str, key: &str) -> Option<bool> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    if remainder.starts_with("true") {
        Some(true)
    } else if remainder.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_i32_value(dictionary: &str, key: &str) -> Option<i32> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    let end = remainder
        .find(|ch: char| ch.is_whitespace() || matches!(ch, '/' | '[' | ']' | '<' | '>'))
        .unwrap_or(remainder.len());
    remainder[..end].parse().ok()
}

fn extract_usize_value(dictionary: &str, key: &str) -> Option<usize> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    let (value, _) = parse_unsigned_integer(remainder.as_bytes(), 0)?;
    Some(value as usize)
}

struct ParsedContent {
    lines: Vec<ExtractedLine>,
    warnings: Vec<Warning>,
    xobject_invocations: Vec<XObjectInvocation>,
}

#[derive(Clone)]
struct ExtractedLine {
    id: usize,
    page_number: usize,
    x: f32,
    y: f32,
    font_size: f32,
    sequence: usize,
    text: String,
}

#[derive(Default)]
struct TextState {
    font_size: f32,
    x: f32,
    y: f32,
    leading: Option<f32>,
}

#[derive(Clone, Copy)]
struct GraphicsState {
    ctm: [f32; 6],
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Copy)]
struct BoundingBox {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
}

struct XObjectInvocation {
    name: String,
    bounds: BoundingBox,
    sequence: usize,
}

fn concat_matrices(left: [f32; 6], right: [f32; 6]) -> [f32; 6] {
    [
        left[0] * right[0] + left[2] * right[1],
        left[1] * right[0] + left[3] * right[1],
        left[0] * right[2] + left[2] * right[3],
        left[1] * right[2] + left[3] * right[3],
        left[0] * right[4] + left[2] * right[5] + left[4],
        left[1] * right[4] + left[3] * right[5] + left[5],
    ]
}

fn transform_point(matrix: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (
        matrix[0] * x + matrix[2] * y + matrix[4],
        matrix[1] * x + matrix[3] * y + matrix[5],
    )
}

fn matrix_bounds(matrix: [f32; 6]) -> BoundingBox {
    let corners = [
        transform_point(matrix, 0.0, 0.0),
        transform_point(matrix, 1.0, 0.0),
        transform_point(matrix, 0.0, 1.0),
        transform_point(matrix, 1.0, 1.0),
    ];
    let mut left = f32::INFINITY;
    let mut bottom = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::NEG_INFINITY;

    for (x, y) in corners {
        left = left.min(x);
        bottom = bottom.min(y);
        right = right.max(x);
        top = top.max(y);
    }

    BoundingBox {
        left,
        bottom,
        right,
        top,
    }
}

fn parse_content_stream(stream: &[u8], page_number: usize) -> Result<ParsedContent, String> {
    let mut lexer = ContentLexer::new(stream);
    let mut operands = Vec::new();
    let mut lines = Vec::new();
    let mut warnings = HashSet::new();
    let mut state = TextState {
        font_size: 12.0,
        ..TextState::default()
    };
    let mut text_sequence = 0usize;
    let mut xobject_sequence = 0usize;
    let mut graphics_state = GraphicsState::default();
    let mut graphics_stack = Vec::new();
    let mut xobject_invocations = Vec::new();

    while let Some(token) = lexer.next_token()? {
        match token {
            ContentToken::Operand(operand) => operands.push(operand),
            ContentToken::Operator(operator) => {
                match operator.as_str() {
                    "q" => graphics_stack.push(graphics_state),
                    "Q" => {
                        if let Some(saved) = graphics_stack.pop() {
                            graphics_state = saved;
                        }
                    }
                    "cm" => {
                        if let Some(values) = take_numbers(&operands, 6) {
                            graphics_state.ctm = concat_matrices(
                                graphics_state.ctm,
                                [
                                    values[0], values[1], values[2], values[3], values[4],
                                    values[5],
                                ],
                            );
                        }
                    }
                    "Tf" => {
                        if let [.., Operand::Name(_), Operand::Number(size)] = operands.as_slice() {
                            state.font_size = size.abs();
                        }
                    }
                    "TL" => {
                        if let Some(Operand::Number(leading)) = operands.last() {
                            state.leading = Some(leading.abs());
                        }
                    }
                    "Tm" => {
                        if let Some(values) = take_numbers(&operands, 6) {
                            state.x = values[4];
                            state.y = values[5];
                        }
                    }
                    "Td" => {
                        if let Some(values) = take_numbers(&operands, 2) {
                            state.x += values[0];
                            state.y += values[1];
                        }
                    }
                    "TD" => {
                        if let Some(values) = take_numbers(&operands, 2) {
                            state.x += values[0];
                            state.y += values[1];
                            state.leading = Some(values[1].abs());
                        }
                    }
                    "T*" => {
                        let leading = state.leading.unwrap_or(state.font_size * 1.2);
                        state.y -= leading;
                    }
                    "Tj" => {
                        if let Some(text) = take_string(operands.last()) {
                            push_text_line(
                                &mut lines,
                                page_number,
                                &state,
                                text_sequence,
                                normalize_extracted_text(text),
                            );
                            text_sequence += 1;
                        }
                    }
                    "TJ" => {
                        if let Some(text) = take_array_text(operands.last()) {
                            push_text_line(
                                &mut lines,
                                page_number,
                                &state,
                                text_sequence,
                                normalize_extracted_text(&text),
                            );
                            text_sequence += 1;
                        }
                    }
                    "'" => {
                        let leading = state.leading.unwrap_or(state.font_size * 1.2);
                        state.y -= leading;

                        if let Some(text) = take_string(operands.last()) {
                            push_text_line(
                                &mut lines,
                                page_number,
                                &state,
                                text_sequence,
                                normalize_extracted_text(text),
                            );
                            text_sequence += 1;
                        }
                    }
                    "\"" => {
                        let leading = state.leading.unwrap_or(state.font_size * 1.2);
                        state.y -= leading;

                        if let Some(text) = take_string(operands.last()) {
                            push_text_line(
                                &mut lines,
                                page_number,
                                &state,
                                text_sequence,
                                normalize_extracted_text(text),
                            );
                            text_sequence += 1;
                        }
                    }
                    "Do" => {
                        if let Some(Operand::Name(name)) = operands.last() {
                            xobject_invocations.push(XObjectInvocation {
                                name: name.clone(),
                                bounds: matrix_bounds(graphics_state.ctm),
                                sequence: xobject_sequence,
                            });
                            xobject_sequence += 1;
                        }
                    }
                    "BI" | "ID" | "EI" => {
                        warnings.insert(format!(
                            "unsupported content on page {page_number}: inline image data"
                        ));
                    }
                    "m" | "l" | "c" | "v" | "y" | "h" | "re" | "S" | "s" | "f" | "F" | "f*"
                    | "B" | "B*" | "b" | "b*" | "n" => {
                        warnings.insert(format!(
                            "unsupported content on page {page_number}: vector drawing commands"
                        ));
                    }
                    _ => {}
                }

                operands.clear();
            }
        }
    }

    Ok(ParsedContent {
        lines,
        warnings: warnings.into_iter().map(Warning::new).collect(),
        xobject_invocations,
    })
}

struct OcrPageResult {
    lines: Vec<ExtractedLine>,
    warnings: Vec<Warning>,
}

struct ParsedOcrPage {
    lines: Vec<ExtractedLine>,
    average_confidence: Option<f32>,
}

struct OcrLineGroup {
    top: f32,
    left: f32,
    height: f32,
    words: Vec<(f32, String)>,
}

enum OcrAvailability {
    Unknown,
    Available,
    Unavailable(String),
}

struct OcrEngine {
    command: PathBuf,
    language_profile: String,
    availability: OcrAvailability,
}

impl OcrEngine {
    fn from_env() -> Self {
        let command = env::var_os("PDF_TO_TYPST_TESSERACT_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tesseract"));
        let language_profile = env::var("PDF_TO_TYPST_OCR_LANGS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_OCR_LANGUAGES.to_string());

        Self {
            command,
            language_profile,
            availability: OcrAvailability::Unknown,
        }
    }

    fn ocr_page(&mut self, page_number: usize, candidates: &[OcrImageCandidate]) -> OcrPageResult {
        let Some(candidate) = candidates
            .iter()
            .max_by_key(|candidate| candidate.width.saturating_mul(candidate.height))
        else {
            return OcrPageResult {
                lines: Vec::new(),
                warnings: vec![Warning::new(format!(
                    "OCR unavailable on page {page_number}: embedded image encoding is unsupported; scanned page content could not be extracted"
                ))],
            };
        };

        if let Err(reason) = self.ensure_available() {
            return OcrPageResult {
                lines: Vec::new(),
                warnings: vec![Warning::new(format!(
                    "OCR unavailable on page {page_number}: {reason}; scanned page content could not be extracted"
                ))],
            };
        }

        let temp_path = match write_temp_ocr_image(candidate) {
            Ok(path) => path,
            Err(reason) => {
                return OcrPageResult {
                    lines: Vec::new(),
                    warnings: vec![Warning::new(format!(
                        "OCR unavailable on page {page_number}: {reason}; scanned page content could not be extracted"
                    ))],
                };
            }
        };

        let output = Command::new(&self.command)
            .arg(&temp_path)
            .arg("stdout")
            .arg("-l")
            .arg(&self.language_profile)
            .arg("--psm")
            .arg("6")
            .arg("tsv")
            .output();
        cleanup_temp_ocr_image(&temp_path);

        let output = match output {
            Ok(output) => output,
            Err(error) => {
                return OcrPageResult {
                    lines: Vec::new(),
                    warnings: vec![Warning::new(format!(
                        "OCR unavailable on page {page_number}: failed to launch {}: {error}; scanned page content could not be extracted",
                        self.command.display()
                    ))],
                };
            }
        };

        if !output.status.success() {
            let detail = best_command_output(&output.stderr, &output.stdout).unwrap_or_else(|| {
                format!(
                    "{} exited with status {}",
                    self.command.display(),
                    output.status
                )
            });
            return OcrPageResult {
                lines: Vec::new(),
                warnings: vec![Warning::new(format!(
                    "OCR unavailable on page {page_number}: {detail}; scanned page content could not be extracted"
                ))],
            };
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = match parse_ocr_tsv(&stdout, page_number) {
            Ok(parsed) => parsed,
            Err(reason) => {
                return OcrPageResult {
                    lines: Vec::new(),
                    warnings: vec![Warning::new(format!(
                        "OCR unavailable on page {page_number}: invalid OCR output ({reason}); scanned page content could not be extracted"
                    ))],
                };
            }
        };

        if parsed.lines.is_empty() {
            return OcrPageResult {
                lines: Vec::new(),
                warnings: vec![Warning::new(format!(
                    "OCR produced no text on page {page_number} with language profile {}; scanned page content could not be extracted",
                    self.language_profile
                ))],
            };
        }

        let mut warnings = Vec::new();
        if parsed
            .average_confidence
            .is_some_and(|confidence| confidence < DEFAULT_OCR_MIN_CONFIDENCE)
        {
            warnings.push(Warning::new(format!(
                "low-confidence OCR on page {page_number} (avg {:.1} with language profile {}); generated Typst may contain recognition errors",
                parsed.average_confidence.unwrap_or(0.0),
                self.language_profile
            )));
        }

        OcrPageResult {
            lines: parsed.lines,
            warnings,
        }
    }

    fn ensure_available(&mut self) -> Result<(), String> {
        match &self.availability {
            OcrAvailability::Available => return Ok(()),
            OcrAvailability::Unavailable(reason) => return Err(reason.clone()),
            OcrAvailability::Unknown => {}
        }

        let output = Command::new(&self.command).arg("--list-langs").output();
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                let reason = format!(
                    "tesseract executable not found at {}: {error}",
                    self.command.display()
                );
                self.availability = OcrAvailability::Unavailable(reason.clone());
                return Err(reason);
            }
        };

        if !output.status.success() {
            let detail = best_command_output(&output.stderr, &output.stdout).unwrap_or_else(|| {
                format!(
                    "{} exited with status {}",
                    self.command.display(),
                    output.status
                )
            });
            self.availability = OcrAvailability::Unavailable(detail.clone());
            return Err(detail);
        }

        let installed_languages = parse_tesseract_languages(&output.stdout);
        let missing = self
            .language_profile
            .split('+')
            .filter(|language| !language.is_empty())
            .filter(|language| !installed_languages.contains(*language))
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            let reason = format!(
                "missing language data for {} in OCR profile {}",
                missing.join(", "),
                self.language_profile
            );
            self.availability = OcrAvailability::Unavailable(reason.clone());
            return Err(reason);
        }

        self.availability = OcrAvailability::Available;
        Ok(())
    }
}

fn write_temp_ocr_image(candidate: &OcrImageCandidate) -> Result<PathBuf, String> {
    let workspace = ocr_temp_workspace()
        .map_err(|error| format!("failed to prepare OCR workspace: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = workspace.join(format!(
        "pdf-to-typst-ocr-{}-{timestamp}.{}",
        std::process::id(),
        candidate.extension
    ));

    fs::write(&path, &candidate.bytes).map_err(|error| {
        format!(
            "failed to write temporary OCR image {}: {error}",
            path.display()
        )
    })?;

    Ok(path)
}

fn ocr_temp_workspace() -> std::io::Result<PathBuf> {
    let preferred = env::current_dir()?.join(".pdf-to-typst-ocr-tmp");
    fs::create_dir_all(&preferred)?;
    Ok(preferred)
}

fn cleanup_temp_ocr_image(path: &Path) {
    let _ = fs::remove_file(path);
    if let Some(parent) = path.parent()
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == ".pdf-to-typst-ocr-tmp")
    {
        let _ = fs::remove_dir(parent);
    }
}

fn parse_tesseract_languages(stdout: &[u8]) -> HashSet<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn best_command_output(primary: &[u8], fallback: &[u8]) -> Option<String> {
    let primary = String::from_utf8_lossy(primary).trim().to_string();
    if !primary.is_empty() {
        return Some(primary);
    }

    let fallback = String::from_utf8_lossy(fallback).trim().to_string();
    (!fallback.is_empty()).then_some(fallback)
}

fn parse_ocr_tsv(tsv: &str, page_number: usize) -> Result<ParsedOcrPage, String> {
    let mut rows = tsv.lines().filter(|line| !line.trim().is_empty());
    let header = rows
        .next()
        .ok_or_else(|| "missing TSV header".to_string())?;
    let columns = header.split('\t').collect::<Vec<_>>();

    let level_index = find_column_index(&columns, "level")?;
    let block_index = find_column_index(&columns, "block_num")?;
    let paragraph_index = find_column_index(&columns, "par_num")?;
    let line_index = find_column_index(&columns, "line_num")?;
    let left_index = find_column_index(&columns, "left")?;
    let top_index = find_column_index(&columns, "top")?;
    let height_index = find_column_index(&columns, "height")?;
    let confidence_index = find_column_index(&columns, "conf")?;
    let text_index = find_column_index(&columns, "text")?;

    let mut page_height = 0f32;
    let mut confidences = Vec::new();
    let mut groups: HashMap<(u32, u32, u32), OcrLineGroup> = HashMap::new();

    for row in rows {
        let fields = row.split('\t').collect::<Vec<_>>();
        if fields.len() <= text_index {
            continue;
        }

        let level = parse_tsv_u32(fields[level_index], "level")?;
        if level == 1 {
            page_height = page_height.max(parse_tsv_f32(fields[height_index], "height")?);
            continue;
        }

        if level != 5 {
            continue;
        }

        let text = fields[text_index].trim();
        if text.is_empty() {
            continue;
        }

        let block = parse_tsv_u32(fields[block_index], "block_num")?;
        let paragraph = parse_tsv_u32(fields[paragraph_index], "par_num")?;
        let line = parse_tsv_u32(fields[line_index], "line_num")?;
        let left = parse_tsv_f32(fields[left_index], "left")?;
        let top = parse_tsv_f32(fields[top_index], "top")?;
        let height = parse_tsv_f32(fields[height_index], "height")?;
        let confidence = parse_tsv_f32(fields[confidence_index], "conf")?;

        if confidence >= 0.0 {
            confidences.push(confidence);
        }

        let entry = groups
            .entry((block, paragraph, line))
            .or_insert_with(|| OcrLineGroup {
                top,
                left,
                height,
                words: Vec::new(),
            });
        entry.top = entry.top.min(top);
        entry.left = entry.left.min(left);
        entry.height = entry.height.max(height);
        entry.words.push((left, text.to_string()));
    }

    if page_height <= 0.0 {
        page_height = groups
            .values()
            .map(|group| group.top + group.height)
            .fold(0.0, f32::max);
    }

    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        left.top
            .partial_cmp(&right.top)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                left.left
                    .partial_cmp(&right.left)
                    .unwrap_or(Ordering::Equal)
            })
    });

    let lines = groups
        .into_iter()
        .enumerate()
        .map(|(sequence, mut group)| {
            group
                .words
                .sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
            ExtractedLine {
                id: 0,
                page_number,
                x: group.left,
                y: page_height - group.top,
                font_size: group.height.max(1.0),
                sequence,
                text: join_ocr_words(&group.words),
            }
        })
        .collect();

    let average_confidence = (!confidences.is_empty())
        .then(|| confidences.iter().sum::<f32>() / confidences.len() as f32);

    Ok(ParsedOcrPage {
        lines,
        average_confidence,
    })
}

fn find_column_index(columns: &[&str], name: &str) -> Result<usize, String> {
    columns
        .iter()
        .position(|column| *column == name)
        .ok_or_else(|| format!("missing {name} column"))
}

fn parse_tsv_u32(value: &str, label: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|error| format!("invalid {label}: {error}"))
}

fn parse_tsv_f32(value: &str, label: &str) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .map_err(|error| format!("invalid {label}: {error}"))
}

fn join_ocr_words(words: &[(f32, String)]) -> String {
    words
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn take_numbers(operands: &[Operand], count: usize) -> Option<Vec<f32>> {
    if operands.len() < count {
        return None;
    }

    operands[operands.len() - count..]
        .iter()
        .map(|operand| match operand {
            Operand::Number(value) => Some(*value),
            _ => None,
        })
        .collect()
}

fn take_string(operand: Option<&Operand>) -> Option<&str> {
    match operand? {
        Operand::Str(text) => Some(text),
        _ => None,
    }
}

fn take_array_text(operand: Option<&Operand>) -> Option<String> {
    let Operand::Array(items) = operand? else {
        return None;
    };

    let mut text = String::new();

    for item in items {
        match item {
            Operand::Str(fragment) => text.push_str(fragment),
            Operand::Number(adjustment) if *adjustment <= -120.0 => {
                if !text.ends_with(' ') {
                    text.push(' ');
                }
            }
            _ => {}
        }
    }

    Some(text)
}

fn push_text_line(
    lines: &mut Vec<ExtractedLine>,
    page_number: usize,
    state: &TextState,
    sequence: usize,
    text: String,
) {
    if text.trim().is_empty() {
        return;
    }

    lines.push(ExtractedLine {
        id: 0,
        page_number,
        x: state.x,
        y: state.y,
        font_size: state.font_size.max(1.0),
        sequence,
        text,
    });
}

enum ContentToken {
    Operand(Operand),
    Operator(String),
}

enum Operand {
    Number(f32),
    Name(String),
    Str(String),
    Array(Vec<Operand>),
    Other,
}

struct ContentLexer<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> ContentLexer<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn next_token(&mut self) -> Result<Option<ContentToken>, String> {
        self.skip_whitespace_and_comments();

        let Some(byte) = self.bytes.get(self.cursor).copied() else {
            return Ok(None);
        };

        let token = match byte {
            b'[' | b'(' | b'<' | b'/' | b'+' | b'-' | b'.' | b'0'..=b'9' => {
                ContentToken::Operand(self.read_operand()?)
            }
            _ => ContentToken::Operator(self.read_word()),
        };

        Ok(Some(token))
    }

    fn skip_whitespace_and_comments(&mut self) {
        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            if byte.is_ascii_whitespace() {
                self.cursor += 1;
                continue;
            }

            if byte == b'%' {
                while let Some(current) = self.bytes.get(self.cursor).copied() {
                    self.cursor += 1;
                    if current == b'\n' || current == b'\r' {
                        break;
                    }
                }
                continue;
            }

            break;
        }
    }

    fn read_operand(&mut self) -> Result<Operand, String> {
        self.skip_whitespace_and_comments();

        match self.bytes.get(self.cursor).copied() {
            Some(b'[') => {
                self.cursor += 1;
                self.read_array()
            }
            Some(b'(') => self.read_literal_string().map(Operand::Str),
            Some(b'<') if self.bytes.get(self.cursor + 1) != Some(&b'<') => {
                self.read_hex_string().map(Operand::Str)
            }
            Some(b'/') => Ok(Operand::Name(self.read_name())),
            Some(b'+' | b'-' | b'.' | b'0'..=b'9') => self.read_number().map(Operand::Number),
            Some(_) => {
                let _ = self.read_word();
                Ok(Operand::Other)
            }
            None => Err("unexpected end of content stream".to_string()),
        }
    }

    fn read_array(&mut self) -> Result<Operand, String> {
        let mut items = Vec::new();

        loop {
            self.skip_whitespace_and_comments();

            match self.bytes.get(self.cursor).copied() {
                Some(b']') => {
                    self.cursor += 1;
                    return Ok(Operand::Array(items));
                }
                Some(_) => items.push(self.read_operand()?),
                None => return Err("unterminated array".to_string()),
            }
        }
    }

    fn read_literal_string(&mut self) -> Result<String, String> {
        self.cursor += 1;
        let mut buffer = Vec::new();
        let mut depth = 1usize;

        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            self.cursor += 1;

            match byte {
                b'\\' => {
                    let Some(escaped) = self.bytes.get(self.cursor).copied() else {
                        break;
                    };
                    self.cursor += 1;

                    match escaped {
                        b'n' => buffer.push(b'\n'),
                        b'r' => buffer.push(b'\r'),
                        b't' => buffer.push(b'\t'),
                        b'b' => buffer.push(8),
                        b'f' => buffer.push(12),
                        b'(' | b')' | b'\\' => buffer.push(escaped),
                        b'\n' => {}
                        b'\r' => {
                            if self.bytes.get(self.cursor) == Some(&b'\n') {
                                self.cursor += 1;
                            }
                        }
                        b'0'..=b'7' => {
                            let mut octal = vec![escaped];

                            for _ in 0..2 {
                                if let Some(next) = self.bytes.get(self.cursor).copied() {
                                    if matches!(next, b'0'..=b'7') {
                                        octal.push(next);
                                        self.cursor += 1;
                                    } else {
                                        break;
                                    }
                                }
                            }

                            let value = u8::from_str_radix(
                                std::str::from_utf8(&octal).map_err(|error| error.to_string())?,
                                8,
                            )
                            .map_err(|error| error.to_string())?;
                            buffer.push(value);
                        }
                        _ => buffer.push(escaped),
                    }
                }
                b'(' => {
                    depth += 1;
                    buffer.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(decode_text_bytes(&buffer));
                    }
                    buffer.push(byte);
                }
                _ => buffer.push(byte),
            }
        }

        Err("unterminated literal string".to_string())
    }

    fn read_hex_string(&mut self) -> Result<String, String> {
        self.cursor += 1;
        let start = self.cursor;

        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            if byte == b'>' {
                let slice = &self.bytes[start..self.cursor];
                self.cursor += 1;
                return decode_hex_string(slice);
            }

            self.cursor += 1;
        }

        Err("unterminated hex string".to_string())
    }

    fn read_name(&mut self) -> String {
        self.cursor += 1;
        let start = self.cursor;

        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            if byte.is_ascii_whitespace() || is_delimiter(byte) {
                break;
            }
            self.cursor += 1;
        }

        String::from_utf8_lossy(&self.bytes[start..self.cursor]).into_owned()
    }

    fn read_number(&mut self) -> Result<f32, String> {
        let start = self.cursor;

        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            if !matches!(byte, b'+' | b'-' | b'.' | b'0'..=b'9') {
                break;
            }
            self.cursor += 1;
        }

        std::str::from_utf8(&self.bytes[start..self.cursor])
            .map_err(|error| error.to_string())?
            .parse::<f32>()
            .map_err(|error| error.to_string())
    }

    fn read_word(&mut self) -> String {
        let start = self.cursor;

        while let Some(byte) = self.bytes.get(self.cursor).copied() {
            if byte.is_ascii_whitespace() || is_delimiter(byte) {
                break;
            }
            self.cursor += 1;
        }

        String::from_utf8_lossy(&self.bytes[start..self.cursor]).into_owned()
    }
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'[' | b']' | b'(' | b')' | b'<' | b'>' | b'/' | b'%')
}

fn decode_hex_string(bytes: &[u8]) -> Result<String, String> {
    let mut hex = bytes
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();

    if hex.len() % 2 != 0 {
        hex.push(b'0');
    }

    let mut decoded = Vec::with_capacity(hex.len() / 2);
    let mut cursor = 0;

    while cursor < hex.len() {
        let value = u8::from_str_radix(
            std::str::from_utf8(&hex[cursor..cursor + 2]).map_err(|error| error.to_string())?,
            16,
        )
        .map_err(|error| error.to_string())?;
        decoded.push(value);
        cursor += 2;
    }

    Ok(decode_text_bytes(&decoded))
}

fn decode_text_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16(&bytes[2..], true);
    }

    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16(&bytes[2..], false);
    }

    let zero_bytes = bytes.iter().filter(|byte| **byte == 0).count();
    if zero_bytes * 4 >= bytes.len().max(1) && bytes.len().is_multiple_of(2) {
        return decode_utf16(bytes, true);
    }

    bytes
        .iter()
        .map(|byte| match byte {
            b'\n' | b'\r' | b'\t' => char::from(*byte),
            0x20..=0x7E => char::from(*byte),
            _ => char::from(*byte),
        })
        .collect()
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    let mut cursor = 0;

    while cursor + 1 < bytes.len() {
        let unit = if big_endian {
            u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]])
        } else {
            u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]])
        };
        units.push(unit);
        cursor += 2;
    }

    String::from_utf16_lossy(&units)
}

fn normalize_extracted_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());

    for ch in text.chars() {
        if ch == '\n' || ch == '\r' || (ch.is_control() && ch != '\t') {
            normalized.push(' ');
        } else {
            normalized.push(ch);
        }
    }

    normalized
}

struct RenderedPage {
    blocks: Vec<String>,
    warnings: Vec<Warning>,
    assets: Vec<OutputAsset>,
}

struct PageRichElement {
    sort_y: f32,
    sort_x: f32,
    block: String,
}

#[derive(Clone)]
struct TableRowGroup {
    y: f32,
    font_size: f32,
    cells: Vec<ExtractedLine>,
}

struct CaptionCandidate {
    id: usize,
    text: String,
    y: f32,
}

enum CaptionKind {
    Image,
    Table,
}

enum PageEvent {
    Text(ExtractedLine),
    Rich(PageRichElement),
}

fn assign_line_ids(lines: &mut [ExtractedLine]) {
    for (index, line) in lines.iter_mut().enumerate() {
        line.id = index;
    }
}

fn render_page(
    page_number: usize,
    lines: Vec<ExtractedLine>,
    mut xobjects: Vec<XObjectInvocation>,
    image_resources: PageImageResources,
) -> RenderedPage {
    let (mut rich_elements, mut consumed_ids) = detect_tables(&lines);
    let mut warnings = Vec::new();
    let mut assets = Vec::new();
    let mut image_count = 0usize;

    xobjects.sort_by(|left, right| {
        right
            .bounds
            .top
            .partial_cmp(&left.bounds.top)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                left.bounds
                    .left
                    .partial_cmp(&right.bounds.left)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.sequence.cmp(&right.sequence))
    });

    for invocation in xobjects {
        let Some(resource) = image_resources.resources.get(&invocation.name) else {
            warnings.push(Warning::new(format!(
                "unsupported content on page {page_number}: XObject invocation"
            )));
            continue;
        };

        let Some(asset) = resource.asset.as_ref() else {
            let detail = resource
                .asset_issue
                .as_deref()
                .unwrap_or("unsupported image encoding");
            warnings.push(Warning::new(format!(
                "degraded rich element on page {page_number}: image {} could not be extracted ({detail})",
                resource.name
            )));
            continue;
        };

        image_count += 1;
        let filename = format!("page-{page_number}-image-{image_count}.{}", asset.extension);
        let caption = find_caption_line(
            lines.as_slice(),
            &consumed_ids,
            invocation.bounds,
            CaptionKind::Image,
        );
        if let Some(caption) = &caption {
            consumed_ids.insert(caption.id);
        }

        assets.push(OutputAsset {
            filename: filename.clone(),
            bytes: asset.bytes.clone(),
        });
        rich_elements.push(PageRichElement {
            sort_y: caption.as_ref().map_or(invocation.bounds.top, |caption| {
                caption.y.max(invocation.bounds.top)
            }),
            sort_x: invocation.bounds.left,
            block: render_image_block(
                &filename,
                caption.as_ref().map(|caption| caption.text.as_str()),
            ),
        });
    }

    let mut events = lines
        .into_iter()
        .filter(|line| !consumed_ids.contains(&line.id))
        .map(PageEvent::Text)
        .collect::<Vec<_>>();
    events.extend(rich_elements.into_iter().map(PageEvent::Rich));
    events.sort_by(page_event_sort_key);

    let mut blocks = Vec::new();
    let mut text_chunk = Vec::new();

    for event in events {
        match event {
            PageEvent::Text(line) => text_chunk.push(line),
            PageEvent::Rich(element) => {
                blocks.extend(render_text_blocks(std::mem::take(&mut text_chunk)));
                blocks.push(element.block);
            }
        }
    }

    blocks.extend(render_text_blocks(text_chunk));

    RenderedPage {
        blocks,
        warnings,
        assets,
    }
}

fn page_event_sort_key(left: &PageEvent, right: &PageEvent) -> Ordering {
    let (left_y, left_x, left_kind) = match left {
        PageEvent::Text(line) => (line.y, line.x, 0usize),
        PageEvent::Rich(element) => (element.sort_y, element.sort_x, 1usize),
    };
    let (right_y, right_x, right_kind) = match right {
        PageEvent::Text(line) => (line.y, line.x, 0usize),
        PageEvent::Rich(element) => (element.sort_y, element.sort_x, 1usize),
    };

    right_y
        .partial_cmp(&left_y)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left_x.partial_cmp(&right_x).unwrap_or(Ordering::Equal))
        .then_with(|| left_kind.cmp(&right_kind))
}

fn detect_tables(lines: &[ExtractedLine]) -> (Vec<PageRichElement>, HashSet<usize>) {
    if lines.is_empty() {
        return (Vec::new(), HashSet::new());
    }

    let rows = group_table_rows(lines);
    let body_font_size = infer_body_font_size(lines);
    let mut rich_elements = Vec::new();
    let mut consumed_ids = HashSet::new();
    let mut row_index = 0usize;

    while let Some(row) = rows.get(row_index) {
        if row.cells.len() < 2 || !row_has_distinct_columns(row, body_font_size) {
            row_index += 1;
            continue;
        }

        let column_positions = row.cells.iter().map(|cell| cell.x).collect::<Vec<_>>();
        let mut matched_rows = vec![row.clone()];
        let mut previous_y = row.y;
        let mut next_index = row_index + 1;

        while let Some(next_row) = rows.get(next_index) {
            let gap = previous_y - next_row.y;
            if gap > body_font_size * 2.0 + 14.0 {
                break;
            }

            if next_row.cells.len() != column_positions.len()
                || !columns_align(&column_positions, next_row, body_font_size)
            {
                break;
            }

            matched_rows.push(next_row.clone());
            previous_y = next_row.y;
            next_index += 1;
        }

        if matched_rows.len() < 2 {
            row_index += 1;
            continue;
        }

        let bounds = table_bounds(&matched_rows, body_font_size);
        let mut blocked = consumed_ids.clone();
        for row in &matched_rows {
            for cell in &row.cells {
                blocked.insert(cell.id);
            }
        }

        let caption = find_caption_line(lines, &blocked, bounds, CaptionKind::Table);
        let mut table_consumed = blocked
            .difference(&consumed_ids)
            .copied()
            .collect::<HashSet<_>>();
        if let Some(caption) = &caption {
            table_consumed.insert(caption.id);
        }

        rich_elements.push(PageRichElement {
            sort_y: caption
                .as_ref()
                .map_or(bounds.top, |caption| caption.y.max(bounds.top)),
            sort_x: bounds.left,
            block: render_table_block(
                &matched_rows
                    .iter()
                    .map(|row| {
                        row.cells
                            .iter()
                            .map(|cell| normalize_plain_line(&cell.text))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
                caption.as_ref().map(|caption| caption.text.as_str()),
            ),
        });
        consumed_ids.extend(table_consumed);
        row_index = next_index;
    }

    (rich_elements, consumed_ids)
}

fn group_table_rows(lines: &[ExtractedLine]) -> Vec<TableRowGroup> {
    let mut sorted = lines.to_vec();
    sorted.sort_by(|left, right| {
        right
            .y
            .partial_cmp(&left.y)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.x.partial_cmp(&right.x).unwrap_or(Ordering::Equal))
            .then_with(|| left.sequence.cmp(&right.sequence))
    });

    let mut rows: Vec<TableRowGroup> = Vec::new();

    for line in sorted {
        let plain = normalize_plain_line(&line.text);
        if plain.is_empty() {
            continue;
        }

        if let Some(previous) = rows.last_mut() {
            let same_row = (previous.y - line.y).abs() <= 1.0;
            let similar_size = (previous.font_size - line.font_size).abs() <= 0.75;
            if same_row && similar_size {
                previous.cells.push(line);
                continue;
            }
        }

        rows.push(TableRowGroup {
            y: line.y,
            font_size: line.font_size,
            cells: vec![line],
        });
    }

    for row in &mut rows {
        row.cells.sort_by(|left, right| {
            left.x
                .partial_cmp(&right.x)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
    }

    rows
}

fn row_has_distinct_columns(row: &TableRowGroup, body_font_size: f32) -> bool {
    row.cells
        .windows(2)
        .all(|cells| (cells[1].x - cells[0].x) >= body_font_size * 4.0)
}

fn columns_align(column_positions: &[f32], row: &TableRowGroup, body_font_size: f32) -> bool {
    let tolerance = body_font_size.max(10.0) * 1.5;
    row.cells
        .iter()
        .zip(column_positions.iter())
        .all(|(cell, expected)| (cell.x - *expected).abs() <= tolerance)
}

fn table_bounds(rows: &[TableRowGroup], body_font_size: f32) -> BoundingBox {
    let left = rows
        .iter()
        .flat_map(|row| row.cells.iter().map(|cell| cell.x))
        .fold(f32::INFINITY, f32::min);
    let right = rows
        .iter()
        .flat_map(|row| row.cells.iter().map(|cell| cell.x))
        .fold(f32::NEG_INFINITY, f32::max)
        + body_font_size * 8.0;
    let top = rows.first().map(|row| row.y + row.font_size).unwrap_or(0.0);
    let bottom = rows.last().map(|row| row.y - row.font_size).unwrap_or(0.0);

    BoundingBox {
        left,
        bottom,
        right,
        top,
    }
}

fn find_caption_line(
    lines: &[ExtractedLine],
    blocked_ids: &HashSet<usize>,
    bounds: BoundingBox,
    kind: CaptionKind,
) -> Option<CaptionCandidate> {
    lines
        .iter()
        .filter(|line| !blocked_ids.contains(&line.id))
        .filter_map(|line| {
            let text = normalize_plain_line(&line.text);
            if !matches_caption(&text, &kind) {
                return None;
            }

            let vertical_distance = if line.y > bounds.top {
                line.y - bounds.top
            } else if line.y < bounds.bottom {
                bounds.bottom - line.y
            } else {
                0.0
            };
            if vertical_distance > 72.0 {
                return None;
            }

            let horizontal_close = line.x <= bounds.right + 48.0 && line.x >= bounds.left - 72.0;
            if !horizontal_close {
                return None;
            }

            let rank = if line.y >= bounds.top { 0usize } else { 1usize };
            Some((vertical_distance, rank, line.id, line.y, text))
        })
        .min_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| right.3.partial_cmp(&left.3).unwrap_or(Ordering::Equal))
        })
        .map(|(_, _, id, y, text)| CaptionCandidate { id, text, y })
}

fn matches_caption(text: &str, kind: &CaptionKind) -> bool {
    let lower = text.to_ascii_lowercase();

    match kind {
        CaptionKind::Image => {
            lower.starts_with("figure ")
                || lower.starts_with("figure:")
                || lower.starts_with("fig. ")
                || lower.starts_with("fig ")
                || lower.starts_with("image ")
                || lower.starts_with("image:")
        }
        CaptionKind::Table => lower.starts_with("table ") || lower.starts_with("table:"),
    }
}

fn render_image_block(filename: &str, caption: Option<&str>) -> String {
    match caption {
        Some(caption) => format!(
            "#figure(\n  image(\"assets/{filename}\"),\n  caption: [{}],\n)",
            typst_bracket_text(caption)
        ),
        None => format!("#image(\"assets/{filename}\")"),
    }
}

fn render_table_block(rows: &[Vec<String>], caption: Option<&str>) -> String {
    let table = render_table(rows);

    match caption {
        Some(caption) => format!(
            "#figure(\n  kind: table,\n  {table},\n  caption: [{}],\n)",
            typst_bracket_text(caption)
        ),
        None => format!("#{table}"),
    }
}

fn render_table(rows: &[Vec<String>]) -> String {
    let columns = rows.first().map_or(0usize, |row| row.len());
    let mut lines = vec![format!("table(\n    columns: {columns},")];

    for row in rows {
        for cell in row {
            lines.push(format!("    [{}],", typst_bracket_text(cell)));
        }
    }

    lines.push("  )".to_string());
    lines.join("\n")
}

fn typst_bracket_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('#', "\\#")
}

fn render_document_blocks(blocks: Vec<String>) -> String {
    if blocks.is_empty() {
        return "// No digital text could be extracted from the PDF.\n".to_string();
    }

    let mut output = blocks.join("\n\n");
    output.push('\n');
    output
}

fn render_text_blocks(lines: Vec<ExtractedLine>) -> Vec<String> {
    let lines = collapse_lines(lines);
    if lines.is_empty() {
        return Vec::new();
    }

    let body_font_size = infer_body_font_size(&lines);
    let mut blocks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = &lines[index];
        let plain = normalize_plain_line(&line.text);

        if plain.is_empty() {
            index += 1;
            continue;
        }

        if is_heading_line(line, &plain, body_font_size) {
            blocks.push(format!("= {}", plain));
            index += 1;
            continue;
        }

        if let Some(item) = normalize_list_item(&plain) {
            let mut items = vec![format!("- {}", item)];
            index += 1;

            while let Some(next_line) = lines.get(index) {
                let next_plain = normalize_plain_line(&next_line.text);
                let Some(next_item) = normalize_list_item(&next_plain) else {
                    break;
                };

                items.push(format!("- {}", next_item));
                index += 1;
            }

            blocks.push(items.join("\n"));
            continue;
        }

        if looks_code_like(&plain) {
            let mut code_lines = vec![line.text.trim_end().to_string()];
            let start_indent = line.x;
            let start_page = line.page_number;
            let mut previous_y = line.y;
            index += 1;

            while let Some(next_line) = lines.get(index) {
                if next_line.page_number != start_page {
                    break;
                }

                let gap = previous_y - next_line.y;
                let next_plain = normalize_plain_line(&next_line.text);
                if gap > body_font_size * 2.0 + 8.0 {
                    break;
                }

                if !looks_code_like(&next_plain) && next_line.x + 1.0 < start_indent {
                    break;
                }

                code_lines.push(next_line.text.trim_end().to_string());
                previous_y = next_line.y;
                index += 1;
            }

            blocks.push(render_raw_block(&code_lines));
            continue;
        }

        let mut paragraph = vec![plain];
        let paragraph_page = line.page_number;
        let mut previous_y = line.y;
        index += 1;

        while let Some(next_line) = lines.get(index) {
            if next_line.page_number != paragraph_page {
                break;
            }

            let next_plain = normalize_plain_line(&next_line.text);
            if next_plain.is_empty()
                || is_heading_line(next_line, &next_plain, body_font_size)
                || normalize_list_item(&next_plain).is_some()
                || looks_code_like(&next_plain)
            {
                break;
            }

            let gap = previous_y - next_line.y;
            if gap > body_font_size * 2.0 + 8.0 {
                break;
            }

            paragraph.push(next_plain);
            previous_y = next_line.y;
            index += 1;
        }

        blocks.push(paragraph.join(" "));
    }

    blocks
}

fn collapse_lines(mut lines: Vec<ExtractedLine>) -> Vec<ExtractedLine> {
    lines.sort_by(|left, right| {
        left.page_number
            .cmp(&right.page_number)
            .then_with(|| right.y.partial_cmp(&left.y).unwrap_or(Ordering::Equal))
            .then_with(|| left.x.partial_cmp(&right.x).unwrap_or(Ordering::Equal))
            .then_with(|| left.sequence.cmp(&right.sequence))
    });

    let mut collapsed: Vec<ExtractedLine> = Vec::new();

    for line in lines {
        let mut merged = false;

        if let Some(previous) = collapsed.last_mut() {
            let same_page = previous.page_number == line.page_number;
            let same_row = (previous.y - line.y).abs() <= 1.0;
            let similar_size = (previous.font_size - line.font_size).abs() <= 0.75;

            if same_page && same_row && similar_size {
                append_fragment(&mut previous.text, &line.text);
                previous.x = previous.x.min(line.x);
                merged = true;
            }
        }

        if !merged {
            collapsed.push(line);
        }
    }

    collapsed
}

fn append_fragment(existing: &mut String, fragment: &str) {
    let trimmed = fragment.trim();
    if trimmed.is_empty() {
        return;
    }

    if existing.ends_with(char::is_alphanumeric)
        && trimmed.starts_with(char::is_alphanumeric)
        && !existing.ends_with(' ')
    {
        existing.push(' ');
    }

    existing.push_str(trimmed);
}

fn infer_body_font_size(lines: &[ExtractedLine]) -> f32 {
    let mut weights = HashMap::new();

    for line in lines {
        let bucket = (line.font_size * 10.0).round() as i32;
        let weight = normalize_plain_line(&line.text).len().max(1);
        *weights.entry(bucket).or_insert(0usize) += weight;
    }

    weights
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(bucket, _)| bucket as f32 / 10.0)
        .unwrap_or(12.0)
}

fn normalize_plain_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_heading_line(line: &ExtractedLine, plain: &str, body_font_size: f32) -> bool {
    !plain.is_empty()
        && !looks_code_like(plain)
        && normalize_list_item(plain).is_none()
        && plain.len() <= 120
        && (line.font_size >= body_font_size + 2.0 || line.font_size >= body_font_size * 1.2)
}

fn normalize_list_item(text: &str) -> Option<String> {
    for prefix in ["- ", "* ", "• ", "– ", "— "] {
        if let Some(item) = text.strip_prefix(prefix) {
            return Some(item.trim().to_string());
        }
    }

    None
}

fn looks_code_like(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    if matches!(trimmed, "{" | "}" | "[" | "]" | "(" | ")") {
        return true;
    }

    for prefix in [
        "fn ", "let ", "if ", "for ", "while ", "match ", "return ", "class ", "def ", "import ",
        "from ", "SELECT ", "INSERT ", "UPDATE ", "DELETE ",
    ] {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }

    [
        "println(", "::", "=>", "();", "{", "}", ";", "</", "/>", "#include", "```",
    ]
    .iter()
    .any(|needle| trimmed.contains(needle))
}

fn render_raw_block(lines: &[String]) -> String {
    let fence_width = lines
        .iter()
        .map(|line| max_backtick_run(line) + 3)
        .max()
        .unwrap_or(3);
    let fence = "`".repeat(fence_width);

    format!("{fence}\n{}\n{fence}", lines.join("\n"))
}

fn max_backtick_run(input: &str) -> usize {
    let mut max_run = 0usize;
    let mut current_run = 0usize;

    for ch in input.chars() {
        if ch == '`' {
            current_run += 1;
            max_run = max_run.max(current_run);
        } else {
            current_run = 0;
        }
    }

    max_run
}

#[cfg(test)]
mod tests {
    use super::{PageRecovery, page_recovery_for_warning, pdfkit_helper_paths};
    use std::path::Path;

    #[test]
    fn xobject_warnings_trigger_pdfkit_recovery() {
        assert_eq!(
            page_recovery_for_warning("unsupported content on page 1: XObject invocation"),
            Some(PageRecovery::Pdfkit)
        );
    }

    #[test]
    fn warning_classification_distinguishes_pdfkit_and_raster_recovery() {
        assert_eq!(
            page_recovery_for_warning("unsupported content on page 1: vector drawing commands"),
            Some(PageRecovery::Pdfkit)
        );
        assert_eq!(
            page_recovery_for_warning("unsupported content on page 1: inline image data"),
            Some(PageRecovery::Pdfkit)
        );
        assert_eq!(
            page_recovery_for_warning("unsupported content on page 1: unsupported stream filter"),
            Some(PageRecovery::Pdfkit)
        );
        assert_eq!(
            page_recovery_for_warning("unsupported content on page 1: no digital text extracted"),
            Some(PageRecovery::Raster)
        );
    }

    #[test]
    fn pdfkit_helper_paths_are_scoped_to_workspace() {
        let workspace_a = Path::new("/tmp/pdf-to-typst-a");
        let workspace_b = Path::new("/tmp/pdf-to-typst-b");

        let (binary_a, cache_a) = pdfkit_helper_paths(workspace_a);
        let (binary_b, cache_b) = pdfkit_helper_paths(workspace_b);

        assert!(binary_a.starts_with(workspace_a));
        assert!(cache_a.starts_with(workspace_a));
        assert!(binary_b.starts_with(workspace_b));
        assert!(cache_b.starts_with(workspace_b));
        assert_ne!(binary_a, binary_b);
        assert_ne!(cache_a, cache_b);
    }
}
