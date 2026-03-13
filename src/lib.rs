use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::ZlibDecoder;

const HELP_TEXT: &str = "\
pdf-to-typst

Convert a single PDF into a deterministic Typst output directory.

Usage: pdf-to-typst <INPUT_PDF> <OUTPUT_DIR> [OPTIONS]

Required arguments:
  <INPUT_PDF>   Path to the source PDF file.
  <OUTPUT_DIR>  Directory where main.typ and assets/ are written.

Options:
  --strict      Treat warnings as fatal errors.
  -h, --help    Print this help text.
";

const DEFAULT_OCR_LANGUAGES: &str = "kor+eng";
const DEFAULT_OCR_MIN_CONFIDENCE: f32 = 65.0;

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

pub fn help_text() -> &'static str {
    HELP_TEXT
}

pub fn parse_args<I>(args: I) -> Result<Option<CliOptions>, CliFailure>
where
    I: IntoIterator<Item = OsString>,
{
    let mut strict = false;
    let mut positional = Vec::new();

    for arg in args.into_iter().skip(1) {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => return Ok(None),
            "--strict" => strict = true,
            flag if flag.starts_with('-') => {
                return Err(CliFailure::usage(format!("unknown option: {flag}")));
            }
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    match positional.as_slice() {
        [input_pdf, output_dir] => Ok(Some(CliOptions {
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
        return Err(CliFailure::fatal(format!(
            "error: {}",
            warnings[0].message()
        )));
    }

    let document = convert_pdf(&options.input_pdf)?;
    warnings.extend(document.warnings);
    let warnings = dedupe_warnings(warnings);

    if options.strict && !warnings.is_empty() {
        return Err(CliFailure::fatal(format!(
            "error: {}",
            warnings[0].message()
        )));
    }

    fs::create_dir_all(&options.output_dir)
        .map_err(|error| CliFailure::fatal(format_output_error(&options.output_dir, &error)))?;

    let assets_dir = options.output_dir.join("assets");
    fs::create_dir_all(&assets_dir)
        .map_err(|error| CliFailure::fatal(format_output_error(&assets_dir, &error)))?;

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
    let mut lines = Vec::new();
    let mut ocr_engine = OcrEngine::from_env();

    for (page_index, page_ref) in page_refs.iter().enumerate() {
        let page_number = page_index + 1;
        let content_refs = pdf.page_content_refs(*page_ref).map_err(|message| {
            CliFailure::fatal(format!(
                "error: failed to parse PDF page {page_number}: {message}"
            ))
        })?;
        let page_images = pdf.page_ocr_images(*page_ref).map_err(|message| {
            CliFailure::fatal(format!(
                "error: failed to parse PDF page {page_number}: {message}"
            ))
        })?;

        let mut page_lines = Vec::new();
        let mut page_warnings = Vec::new();
        let mut page_had_xobject = false;
        let mut ocr_attempted = false;

        for content_ref in content_refs {
            match pdf.decode_content_stream(content_ref) {
                Ok(Some(stream)) => match parse_content_stream(&stream, page_number) {
                    Ok(parsed) => {
                        page_lines.extend(parsed.lines);
                        page_warnings.extend(parsed.warnings);
                        page_had_xobject |= parsed.invoked_xobjects > 0;
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

        if page_lines.is_empty() && page_had_xobject && page_images.image_count > 0 {
            ocr_attempted = true;

            if page_images.candidates.is_empty() {
                page_warnings.push(Warning::new(format!(
                    "OCR unavailable on page {page_number}: embedded image encoding is unsupported; scanned page content could not be extracted"
                )));
            } else {
                let ocr_result = ocr_engine.ocr_page(page_number, &page_images.candidates);
                page_lines.extend(ocr_result.lines);
                page_warnings.extend(ocr_result.warnings);
            }
        } else if page_had_xobject {
            page_warnings.push(Warning::new(format!(
                "unsupported content on page {page_number}: XObject invocation"
            )));
        }

        if page_lines.is_empty() {
            if !ocr_attempted {
                page_warnings.push(Warning::new(format!(
                    "unsupported content on page {page_number}: no digital text extracted"
                )));
            }
        }

        lines.extend(page_lines);
        warnings.extend(dedupe_warnings(page_warnings));
    }

    Ok(ConvertedDocument {
        typst: render_typst(lines),
        warnings: dedupe_warnings(warnings),
    })
}

struct ConvertedDocument {
    typst: String,
    warnings: Vec<Warning>,
}

struct ParsedPdf {
    objects: HashMap<u32, PdfObject>,
}

struct PdfObject {
    dictionary: String,
    stream: Option<Vec<u8>>,
}

struct PageOcrImages {
    image_count: usize,
    candidates: Vec<OcrImageCandidate>,
}

struct OcrImageCandidate {
    width: usize,
    height: usize,
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
                object
                    .dictionary
                    .contains("/Type /Catalog")
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

        if object.dictionary.contains("/Type /Pages") {
            let kids = extract_reference_list_value(&object.dictionary, "/Kids")
                .ok_or_else(|| format!("page tree node {object_id} is missing /Kids"))?;

            for kid in kids {
                self.collect_page_refs(kid, page_refs)?;
            }

            return Ok(());
        }

        if object.dictionary.contains("/Type /Page") {
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

        if !object.dictionary.contains("/FlateDecode") {
            return Ok(None);
        }

        let mut decoder = ZlibDecoder::new(stream.as_slice());
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .map_err(|error| format!("failed to decompress stream {object_id}: {error}"))?;

        Ok(Some(decoded))
    }

    fn page_ocr_images(&self, object_id: u32) -> Result<PageOcrImages, String> {
        let page = self.object(object_id)?;
        let Some(resources_value) =
            extract_dictionary_or_reference_value(&page.dictionary, "/Resources")
        else {
            return Ok(PageOcrImages {
                image_count: 0,
                candidates: Vec::new(),
            });
        };
        let resources = self.resolve_dictionary_value(resources_value)?;
        let Some(xobject_value) = extract_dictionary_or_reference_value(resources, "/XObject")
        else {
            return Ok(PageOcrImages {
                image_count: 0,
                candidates: Vec::new(),
            });
        };
        let xobjects = self.resolve_dictionary_value(xobject_value)?;
        let refs = parse_named_reference_map(xobjects);
        let mut image_count = 0usize;
        let mut candidates = Vec::new();

        for image_ref in refs.into_values() {
            let object = self.object(image_ref)?;
            if !object.dictionary.contains("/Subtype /Image") {
                continue;
            }

            image_count += 1;

            if let Some(candidate) = self.ocr_image_candidate(image_ref)? {
                candidates.push(candidate);
            }
        }

        Ok(PageOcrImages {
            image_count,
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

    fn ocr_image_candidate(&self, object_id: u32) -> Result<Option<OcrImageCandidate>, String> {
        let object = self.object(object_id)?;
        let Some(stream) = &object.stream else {
            return Ok(None);
        };

        let width = extract_usize_value(&object.dictionary, "/Width")
            .ok_or_else(|| format!("image object {object_id} is missing /Width"))?;
        let height = extract_usize_value(&object.dictionary, "/Height")
            .ok_or_else(|| format!("image object {object_id} is missing /Height"))?;

        if object.dictionary.contains("/DecodeParms") || object.dictionary.contains("/Filter [") {
            return Ok(None);
        }

        if object.dictionary.contains("/DCTDecode") {
            return Ok(Some(OcrImageCandidate {
                width,
                height,
                extension: "jpg",
                bytes: stream.clone(),
            }));
        }

        let decoded = if object.dictionary.contains("/Filter") {
            if !object.dictionary.contains("/FlateDecode") {
                return Ok(None);
            }

            decode_flate_bytes(stream, object_id)?
        } else {
            stream.clone()
        };

        let bits = extract_usize_value(&object.dictionary, "/BitsPerComponent").unwrap_or(8);
        if bits != 8 {
            return Ok(None);
        }

        build_pnm_candidate(&object.dictionary, width, height, decoded)
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
    dictionary: &str,
    width: usize,
    height: usize,
    decoded: Vec<u8>,
) -> Result<Option<OcrImageCandidate>, String> {
    let Some(color_space) = extract_name_value(dictionary, "/ColorSpace") else {
        return Ok(None);
    };

    let (magic, expected_len, extension) = match color_space {
        "DeviceGray" => ("P5", width.saturating_mul(height), "pgm"),
        "DeviceRGB" => ("P6", width.saturating_mul(height).saturating_mul(3), "ppm"),
        _ => return Ok(None),
    };

    if decoded.len() != expected_len {
        return Ok(None);
    }

    let mut bytes = format!("{magic}\n{width} {height}\n255\n").into_bytes();
    bytes.extend_from_slice(&decoded);

    Ok(Some(OcrImageCandidate {
        width,
        height,
        extension,
        bytes,
    }))
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

fn extract_usize_value(dictionary: &str, key: &str) -> Option<usize> {
    let remainder = dictionary.split_once(key)?.1.trim_start();
    let (value, _) = parse_unsigned_integer(remainder.as_bytes(), 0)?;
    Some(value as usize)
}

struct ParsedContent {
    lines: Vec<ExtractedLine>,
    warnings: Vec<Warning>,
    invoked_xobjects: usize,
}

#[derive(Clone)]
struct ExtractedLine {
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

fn parse_content_stream(stream: &[u8], page_number: usize) -> Result<ParsedContent, String> {
    let mut lexer = ContentLexer::new(stream);
    let mut operands = Vec::new();
    let mut lines = Vec::new();
    let mut warnings = HashSet::new();
    let mut state = TextState {
        font_size: 12.0,
        ..TextState::default()
    };
    let mut sequence = 0usize;
    let mut invoked_xobjects = 0usize;

    while let Some(token) = lexer.next_token()? {
        match token {
            ContentToken::Operand(operand) => operands.push(operand),
            ContentToken::Operator(operator) => {
                match operator.as_str() {
                    "Tf" => {
                        if let [.., Operand::Name, Operand::Number(size)] = operands.as_slice() {
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
                                sequence,
                                normalize_extracted_text(text),
                            );
                            sequence += 1;
                        }
                    }
                    "TJ" => {
                        if let Some(text) = take_array_text(operands.last()) {
                            push_text_line(
                                &mut lines,
                                page_number,
                                &state,
                                sequence,
                                normalize_extracted_text(&text),
                            );
                            sequence += 1;
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
                                sequence,
                                normalize_extracted_text(text),
                            );
                            sequence += 1;
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
                                sequence,
                                normalize_extracted_text(text),
                            );
                            sequence += 1;
                        }
                    }
                    "Do" => {
                        invoked_xobjects += 1;
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
        invoked_xobjects,
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
        let _ = fs::remove_file(&temp_path);

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
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = env::temp_dir().join(format!(
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

    let mut groups = groups
        .into_iter()
        .map(|(_, group)| group)
        .collect::<Vec<_>>();
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
    Name,
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
            Some(b'/') => {
                let _ = self.read_name();
                Ok(Operand::Name)
            }
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
    if zero_bytes * 4 >= bytes.len().max(1) && bytes.len() % 2 == 0 {
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
        if ch == '\n' || ch == '\r' {
            normalized.push(' ');
        } else if ch.is_control() && ch != '\t' {
            normalized.push(' ');
        } else {
            normalized.push(ch);
        }
    }

    normalized
}

fn render_typst(lines: Vec<ExtractedLine>) -> String {
    let lines = collapse_lines(lines);
    if lines.is_empty() {
        return "// No digital text could be extracted from the PDF.\n".to_string();
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

    let mut output = blocks.join("\n\n");
    output.push('\n');
    output
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
