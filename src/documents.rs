use anyhow::{Context, Result, bail, ensure};
use calamine::Reader;
use std::{
    fs::File,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const EXTENSIONS: &[&str] = &[
    "md", "markdown", "txt", "doc", "docx", "csv", "tsv", "xls", "xlsx", "xlsm", "xlsb", "pdf",
    "html", "htm", "rtf",
];
const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_TEXT: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod tests {
    #[test]
    fn excel_number_formats_are_shown() {
        assert_eq!(number(0.255, "0.0%").as_deref(), Some("25.5%"));
        assert_eq!(number(12000.0, "#,##0").as_deref(), Some("12,000"));
        assert_eq!(number(1234.5, "$#,##0.00").as_deref(), Some("$1,234.50"));
        assert_eq!(number(-3.0, "[$€-407]#,##0.00").as_deref(), Some("-€3.00"));
        assert_eq!(number(1.5, "General"), None);
        assert_eq!(number(45000.0, "yyyy-mm-dd"), None);
    }

    /// Documents made by spindle_docs read back with their formatting, when Python is here.
    #[test]
    fn office_files_keep_their_formatting_in_the_reader() {
        let Some(python) = ["/usr/local/bin/python3", "/usr/bin/python3"]
            .into_iter()
            .find(|p| Path::new(p).is_file())
        else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pylib");
        let script = "from spindle_docs import write_docx, write_xlsx\n\
            write_docx('d.docx', '# Sky\\n\\nIntro with **bold** text.\\n\\n## Why\\n\\n- one\\n- two\\n\\n| A | B |\\n|---|---|\\n| x | y |')\n\
            write_xlsx('b.xlsx', [['Route','Share'],['1',0.255]], formats={'Share':'percent1'})\n";
        let ok = std::process::Command::new(python)
            .args(["-c", script])
            .current_dir(tmp.path())
            .env("PYTHONPATH", &lib)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok);
        let md = docx_markdown(&std::fs::read(tmp.path().join("d.docx")).unwrap()).unwrap();
        assert!(md.starts_with("# Sky"), "{md}");
        assert!(
            md.contains("Intro with **bold** text.") && md.contains("## Why"),
            "{md}"
        );
        assert!(md.contains("- one\n- two"), "{md}");
        assert!(
            md.contains("| **A** | **B** |") && md.contains("| x | y |"),
            "{md}"
        );
        let doc = parse("xlsx", std::fs::read(tmp.path().join("b.xlsx")).unwrap()).unwrap();
        let Body::Table { rows, styles, .. } = &doc.sections[0].body else {
            panic!()
        };
        assert_eq!(rows[1][1], "25.5%");
        assert!(
            styles[0][0].bold && styles[0][0].fill.is_some(),
            "{styles:?}"
        );
        assert!(!styles[1][1].bold);
    }

    use super::*;
    #[test]
    fn rtf_text_keeps_paragraphs_and_escapes_and_drops_tables_of_fonts() {
        let rtf = r"{\rtf1\ansi{\fonttbl{\f0 Calibri;}}{\*\generator Riched20;}\f0 Model metrics\par
Accuracy\tab 0.91\par Caf\'e9 \u8364?5 \{x\}\par}";
        assert_eq!(
            rtf_text(rtf).unwrap(),
            "Model metrics\nAccuracy\t0.91\nCafé €5 {x}"
        );
        assert!(rtf_text("plain").is_err());
        assert!(readable("letter.rtf"));
    }

    #[test]
    fn snapshot_documents_parse_by_extension() {
        assert!(readable("notes.MD") && readable("q3.xlsx") && readable("data.tsv"));
        assert!(!readable("main.rs") && !readable("README"));
        let md = parse("md", b"# Title\n\nBody".to_vec()).unwrap();
        assert_eq!(md.kind, "Markdown");
        let csv = parse("CSV", b"a,b\n1,2".to_vec()).unwrap();
        let Body::Table { rows, .. } = &csv.sections[0].body else {
            panic!()
        };
        assert_eq!(rows[1], vec!["1", "2"]);
        assert!(parse("exe", vec![]).is_err());
    }
    #[test]
    fn quoted_csv_and_offline_html() {
        let tmp = tempfile::tempdir().unwrap();
        let csv = tmp.path().join("fixture.csv");
        std::fs::write(&csv, "Name,Note\nA,\"first, second\nthird\"").unwrap();
        let doc = read(&csv).unwrap();
        let Body::Table { rows, .. } = &doc.sections[0].body else {
            panic!()
        };
        assert_eq!(rows[1][1], "first, second\nthird");
        let html = tmp.path().join("test.html");
        std::fs::write(&html, "<p>Document text</p><script>private_code()</script>").unwrap();
        let text = read(&html).unwrap().sections[0].text();
        assert!(text.contains("Document text"));
        assert!(!text.contains("private_code"));
    }
    #[cfg(windows)]
    #[test]
    fn pdf_page_renders_pixels() {
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (i,obj) in ["<< /Type /Catalog /Pages 2 0 R >>","<< /Type /Pages /Count 1 /Kids [3 0 R] >>","<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 60] /Resources << >> /Contents 4 0 R >>","<< /Length 15 >>\nstream\n1 1 20 20 re f\nendstream"].iter().enumerate(){
            offsets.push(bytes.len());writeln!(&mut bytes,"{} 0 obj\n{obj}\nendobj",i+1).unwrap();
        }
        let xref = bytes.len();
        write!(&mut bytes, "xref\n0 5\n0000000000 65535 f \n").unwrap();
        for off in &offsets[1..] {
            writeln!(&mut bytes, "{off:010} 00000 n ").unwrap();
        }
        write!(
            &mut bytes,
            "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
        )
        .unwrap();
        let doc = pdf(&bytes).unwrap();
        let bitmap = render_pdf(doc.pdf.as_ref().unwrap(), 0).unwrap();
        assert_eq!(bitmap.size, [1000, 1500]);
        assert!(bitmap.pixels.iter().any(|p| p.r() < 25));
        assert!(bitmap.pixels.iter().any(|p| p.r() > 230));
    }
}

pub enum Body {
    Text(String),
    Table {
        rows: Vec<Vec<String>>,
        first_row: usize,
        first_col: usize,
        /// Cell formatting by position in `rows`, when the file has any (Excel).
        styles: Vec<Vec<CellStyle>>,
    },
}

/// How a spreadsheet cell looks: what the reader can show of Excel's formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub color: Option<[u8; 3]>,
    pub fill: Option<[u8; 3]>,
    /// 'l', 'c' or 'r'.
    pub align: Option<char>,
}
pub struct Section {
    pub title: String,
    pub body: Body,
}

pub fn read_request(args: &str) -> Result<(usize, &str)> {
    let args = args.trim();
    if let Some(args) = args.strip_prefix("--section ") {
        let (number, file) = args
            .split_once(char::is_whitespace)
            .context("Use /read --section N <file>")?;
        return Ok((
            number
                .parse::<usize>()?
                .checked_sub(1)
                .context("Sections start at 1")?,
            file.trim(),
        ));
    }
    Ok((0, args))
}

pub fn read_section(root: &Path, args: &str) -> Result<String> {
    let (index, file) = read_request(args)?;
    let doc = read(&crate::state::project_path(root, file)?)?;
    let section = doc.sections.get(index).context("Section does not exist")?;
    let text = section.text();
    Ok(format!(
        "{} · {} of {} · {}\n{}",
        doc.kind,
        index + 1,
        doc.sections.len(),
        section.title,
        if text.trim().is_empty() {
            "No extractable text in this section."
        } else {
            &text
        }
    ))
}
pub struct Document {
    pub kind: &'static str,
    pub sections: Vec<Section>,
    pub note: String,
    #[cfg(windows)]
    pub pdf: Option<windows::Data::Pdf::PdfDocument>,
}
impl Section {
    pub fn text(&self) -> String {
        match &self.body {
            Body::Text(s) => s.clone(),
            Body::Table { rows, .. } => rows
                .iter()
                .map(|r| r.join("\t"))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}
impl Document {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            sections: vec![],
            note: String::new(),
            #[cfg(windows)]
            pdf: None,
        }
    }
    fn text(kind: &'static str, text: String) -> Result<Self> {
        ensure!(text.len() <= MAX_TEXT, "Extracted text exceeds 8 MiB");
        let mut doc = Self::new(kind);
        // Keep layout work bounded even when the source contains a single very long line.
        let mut rest = text.as_str();
        loop {
            let mut end = rest.len().min(12000);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            if end < rest.len() {
                if let Some(i) = rest[..end].rfind('\n').filter(|i| *i > 6000) {
                    end = i + 1;
                }
            }
            doc.sections.push(Section {
                title: format!("Section {}", doc.sections.len() + 1),
                body: Body::Text(rest[..end].into()),
            });
            rest = &rest[end..];
            if rest.is_empty() {
                break;
            }
        }
        Ok(doc)
    }
}

fn cp1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9f => HIGH[(byte - 0x80) as usize],
        b => b as char,
    }
}

/// Extract readable text from an RTF document: paragraphs, tabs, table cells, Unicode and
/// code-page escapes. Formatting, fonts, pictures and metadata groups are dropped.
pub(crate) fn rtf_text(src: &str) -> Result<String> {
    ensure!(
        src.trim_start().starts_with("{\\rtf"),
        "This is not an RTF document"
    );
    const SKIPPED: &[&str] = &[
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "pict",
        "header",
        "headerl",
        "headerr",
        "headerf",
        "footer",
        "footerl",
        "footerr",
        "footerf",
        "object",
        "themedata",
        "colorschememapping",
        "latentstyles",
        "datastore",
        "xmlnstbl",
        "listtable",
        "listoverridetable",
        "rsidtbl",
        "generator",
        "filetbl",
        "revtbl",
        "pgdsctbl",
        "fldinst",
        "listtext",
        "pntext",
    ];
    let b = src.as_bytes();
    let mut out = String::new();
    let mut stack: Vec<(bool, usize)> = vec![];
    let (mut skip, mut uc, mut fallback) = (false, 1usize, 0usize);
    let mut i = 0;
    let emit = |out: &mut String, c: char, skip: bool, fallback: &mut usize| {
        if *fallback > 0 {
            *fallback -= 1;
        } else if !skip {
            out.push(c);
        }
    };
    while i < b.len() {
        match b[i] {
            b'{' => {
                stack.push((skip, uc));
                i += 1;
            }
            b'}' => {
                if let Some((s, u)) = stack.pop() {
                    skip = s;
                    uc = u;
                }
                i += 1;
            }
            b'\\' if i + 1 < b.len() => {
                i += 1;
                let n = b[i];
                if n.is_ascii_alphabetic() {
                    let start = i;
                    while i < b.len() && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &src[start..i];
                    let p = i;
                    if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                        i += 1;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let param: Option<i64> = src[p..i].parse().ok();
                    if i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                    let symbol = match word {
                        w if SKIPPED.contains(&w) => {
                            skip = true;
                            None
                        }
                        "par" | "line" | "sect" | "page" | "row" => Some('\n'),
                        "cell" | "tab" => Some('\t'),
                        "emdash" => Some('—'),
                        "endash" => Some('–'),
                        "bullet" => Some('•'),
                        "lquote" => Some('‘'),
                        "rquote" => Some('’'),
                        "ldblquote" => Some('“'),
                        "rdblquote" => Some('”'),
                        "uc" => {
                            uc = param.unwrap_or(1).clamp(0, 8) as usize;
                            None
                        }
                        "u" => {
                            let v = param.unwrap_or(0);
                            let v = if v < 0 { v + 65536 } else { v } as u32;
                            if !skip {
                                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                            }
                            fallback = uc;
                            None
                        }
                        _ => None,
                    };
                    if let Some(c) = symbol {
                        if !skip {
                            out.push(c);
                        }
                    }
                } else {
                    i += 1;
                    match n {
                        b'*' => skip = true,
                        b'\'' if i + 2 <= b.len() => {
                            if let Ok(v) = u8::from_str_radix(&src[i..i + 2], 16) {
                                emit(&mut out, cp1252(v), skip, &mut fallback);
                            }
                            i += 2;
                        }
                        b'\\' | b'{' | b'}' => emit(&mut out, n as char, skip, &mut fallback),
                        b'~' => emit(&mut out, '\u{a0}', skip, &mut fallback),
                        b'_' => emit(&mut out, '-', skip, &mut fallback),
                        b'\n' | b'\r' if !skip => out.push('\n'),
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                let c = src[i..].chars().next().unwrap();
                emit(&mut out, c, skip, &mut fallback);
                i += c.len_utf8();
            }
        }
    }
    ensure!(out.len() <= MAX_TEXT, "Extracted text exceeds 8 MiB");
    Ok(out.trim_end().to_owned())
}

pub(crate) fn decode(bytes: &[u8]) -> Result<String> {
    for (bom, little) in [(&[0xff, 0xfe][..], true), (&[0xfe, 0xff][..], false)] {
        if let Some(data) = bytes.strip_prefix(bom) {
            ensure!(data.len() % 2 == 0, "Incomplete UTF-16 text");
            let words = data
                .chunks_exact(2)
                .map(|b| {
                    if little {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                })
                .collect::<Vec<_>>();
            return Ok(String::from_utf16(&words)?);
        }
    }
    Ok(
        std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
            .context("Expected UTF-8 or UTF-16 text")?
            .into(),
    )
}

fn archive(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(zip.len() <= 16000, "Archive contains too many entries");
    let mut size = 0u64;
    for i in 0..zip.len() {
        size = size.saturating_add(zip.by_index_raw(i)?.size());
        ensure!(
            size <= 128 * 1024 * 1024,
            "Expanded Office file exceeds 128 MiB"
        );
    }
    Ok(zip)
}

pub(crate) fn docx(bytes: &[u8]) -> Result<String> {
    let mut zip = archive(bytes)?;
    let xml = zip
        .by_name("word/document.xml")
        .context("Missing Word document body")?;
    let mut data = String::new();
    xml.take(MAX_TEXT as u64 + 1).read_to_string(&mut data)?;
    ensure!(data.len() <= MAX_TEXT, "Word document body exceeds 8 MiB");
    let doc = roxmltree::Document::parse(&data)?;
    let mut result = String::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "p" if !result.is_empty() => result.push('\n'),
            "t" => result.push_str(n.text().unwrap_or("")),
            "tab" => result.push('\t'),
            "br" | "cr" => result.push('\n'),
            _ => (),
        }
    }
    Ok(result)
}

/// A Word document as Markdown, keeping its structure: Title and Heading styles become
/// headings, list paragraphs become list items, bold and italic runs keep their emphasis, and
/// tables become pipe tables. Used to show .docx files with their formatting.
fn wchild<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn wval(n: roxmltree::Node<'_, '_>) -> Option<String> {
    n.attributes()
        .find(|a| a.name() == "val")
        .map(|a| a.value().to_owned())
}

fn won(props: Option<roxmltree::Node<'_, '_>>, name: &str) -> bool {
    props
        .and_then(|p| wchild(p, name))
        .is_some_and(|b| !matches!(wval(b).as_deref(), Some("0" | "false" | "none")))
}

pub(crate) fn docx_markdown(bytes: &[u8]) -> Result<String> {
    let mut zip = archive(bytes)?;
    let xml = zip
        .by_name("word/document.xml")
        .context("Missing Word document body")?;
    let mut data = String::new();
    xml.take(MAX_TEXT as u64 + 1).read_to_string(&mut data)?;
    ensure!(data.len() <= MAX_TEXT, "Word document body exceeds 8 MiB");
    let doc = roxmltree::Document::parse(&data)?;
    // Inline text of a paragraph, with bold and italic.
    let inline = |p: roxmltree::Node<'_, '_>| -> String {
        let mut out = String::new();
        for run in p
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "r")
        {
            let mut text = String::new();
            for n in run.children().filter(|n| n.is_element()) {
                match n.tag_name().name() {
                    "t" => text.push_str(n.text().unwrap_or("")),
                    "tab" => text.push('\t'),
                    "br" | "cr" => text.push(' '),
                    _ => {}
                }
            }
            if text.trim().is_empty() {
                out.push_str(&text);
                continue;
            }
            let props = wchild(run, "rPr");
            let mark = match (won(props, "b"), won(props, "i")) {
                (true, true) => "***",
                (true, false) => "**",
                (false, true) => "*",
                _ => "",
            };
            // Keep spaces outside the markers so the emphasis still parses.
            let lead = &text[..text.len() - text.trim_start().len()];
            let trail = &text[text.trim_end().len()..];
            out.push_str(&format!("{lead}{mark}{}{mark}{trail}", text.trim()));
        }
        out.replace(&"****".to_owned(), "")
    };
    let paragraph = |p: roxmltree::Node<'_, '_>| -> String {
        let props = wchild(p, "pPr");
        let style = props
            .and_then(|pp| wchild(pp, "pStyle"))
            .and_then(wval)
            .unwrap_or_default();
        let listed = props.and_then(|pp| wchild(pp, "numPr")).is_some();
        let text = inline(p);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let lower = style.to_ascii_lowercase();
        let level = if lower == "title" {
            Some(1)
        } else if let Some(n) = lower
            .strip_prefix("heading")
            .and_then(|n| n.trim().parse::<usize>().ok())
        {
            Some(n.clamp(1, 6))
        } else {
            None
        };
        if let Some(level) = level {
            return format!("{} {}", "#".repeat(level), trimmed.trim_matches('*'));
        }
        // Bullets written as text ("•\t…") and numbered items ("1.\t…").
        if let Some(rest) = trimmed
            .strip_prefix('•')
            .or_else(|| trimmed.strip_prefix('·'))
        {
            return format!("- {}", rest.trim_start());
        }
        let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0
            && trimmed[digits..].starts_with(['.', ')'])
            && trimmed[digits + 1..].starts_with(['\t', ' '])
        {
            return format!(
                "{}. {}",
                &trimmed[..digits],
                trimmed[digits + 1..].trim_start()
            );
        }
        if listed || lower.contains("list") {
            return format!("- {trimmed}");
        }
        trimmed.to_owned()
    };
    let mut blocks: Vec<String> = vec![];
    let Some(body) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "body")
    else {
        return Ok(String::new());
    };
    let mut list_run = false;
    for n in body.children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "p" => {
                let text = paragraph(n);
                if text.is_empty() {
                    continue;
                }
                let item = text.starts_with("- ")
                    || text.split_once(". ").is_some_and(|(d, _)| {
                        !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())
                    });
                // List items sit together; everything else is its own paragraph.
                if item && list_run {
                    if let Some(last) = blocks.last_mut() {
                        last.push('\n');
                        last.push_str(&text);
                        continue;
                    }
                }
                list_run = item;
                blocks.push(text);
            }
            "tbl" => {
                list_run = false;
                let rows: Vec<Vec<String>> = n
                    .children()
                    .filter(|r| r.is_element() && r.tag_name().name() == "tr")
                    .map(|r| {
                        r.children()
                            .filter(|c| c.is_element() && c.tag_name().name() == "tc")
                            .map(|c| {
                                c.children()
                                    .filter(|p| p.is_element() && p.tag_name().name() == "p")
                                    .map(|p| inline(p).trim().replace('|', "\\|"))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .collect()
                    })
                    .collect();
                let width = rows.iter().map(Vec::len).max().unwrap_or(0);
                if width == 0 {
                    continue;
                }
                let line = |r: &Vec<String>| {
                    let mut cells = r.clone();
                    cells.resize(width, String::new());
                    format!("| {} |", cells.join(" | "))
                };
                let mut table = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
                table.extend(rows.iter().skip(1).map(line));
                blocks.push(table.join("\n"));
            }
            _ => {}
        }
    }
    Ok(blocks.join("\n\n"))
}

/// A cell's look and number format code, read from an .xlsx file.
#[derive(Clone, Debug, Default)]
pub struct Format {
    pub style: CellStyle,
    pub code: String,
}

fn rgb(n: roxmltree::Node<'_, '_>) -> Option<[u8; 3]> {
    let v = n.attribute("rgb")?;
    let v = if v.len() == 8 { &v[2..] } else { v };
    let p = |i: usize| u8::from_str_radix(v.get(i..i + 2)?, 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

fn kids<'a, 'i>(
    n: roxmltree::Node<'a, 'i>,
    name: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn zip_text(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Result<String> {
    let mut data = String::new();
    zip.by_name(name)?
        .take(MAX_TEXT as u64 + 1)
        .read_to_string(&mut data)?;
    ensure!(data.len() <= MAX_TEXT, "{name} is too large");
    Ok(data)
}

/// Each formatted cell of one sheet in an .xlsx file: bold, italic, font colour, fill,
/// alignment and number format. Theme colours are not resolved.
pub(crate) fn xlsx_formats(
    bytes: &[u8],
    sheet: &str,
) -> Result<std::collections::HashMap<(usize, usize), Format>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))?;
    let workbook = zip_text(&mut zip, "xl/workbook.xml")?;
    let rels = zip_text(&mut zip, "xl/_rels/workbook.xml.rels")?;
    let wb = roxmltree::Document::parse(&workbook)?;
    let id = wb
        .descendants()
        .find(|n| n.tag_name().name() == "sheet" && n.attribute("name") == Some(sheet))
        .and_then(|n| {
            n.attributes()
                .find(|a| a.name() == "id")
                .map(|a| a.value().to_owned())
        })
        .context("Sheet not found")?;
    let rels = roxmltree::Document::parse(&rels)?;
    let target = rels
        .descendants()
        .find(|n| n.attribute("Id") == Some(id.as_str()))
        .and_then(|n| n.attribute("Target"))
        .context("Sheet part not found")?;
    let target = target.trim_start_matches('/');
    let path = if target.starts_with("xl/") {
        target.to_owned()
    } else {
        format!("xl/{target}")
    };
    // Styles: fonts, fills, number formats, then the cell formats that combine them.
    let mut fonts = vec![];
    let mut fills = vec![];
    let mut codes: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    let mut xfs: Vec<Format> = vec![];
    if let Ok(styles) = zip_text(&mut zip, "xl/styles.xml") {
        let st = roxmltree::Document::parse(&styles)?;
        let root = st.root_element();
        if let Some(n) = kids(root, "numFmts").next() {
            for f in kids(n, "numFmt") {
                if let (Some(id), Some(code)) = (
                    f.attribute("numFmtId").and_then(|v| v.parse().ok()),
                    f.attribute("formatCode"),
                ) {
                    codes.insert(id, code.to_owned());
                }
            }
        }
        if let Some(n) = kids(root, "fonts").next() {
            for f in kids(n, "font") {
                let on = |name| {
                    kids(f, name)
                        .next()
                        .is_some_and(|b| b.attribute("val") != Some("0"))
                };
                fonts.push((on("b"), on("i"), kids(f, "color").next().and_then(rgb)));
            }
        }
        if let Some(n) = kids(root, "fills").next() {
            for f in kids(n, "fill") {
                fills.push(
                    kids(f, "patternFill")
                        .next()
                        .filter(|p| p.attribute("patternType") == Some("solid"))
                        .and_then(|p| kids(p, "fgColor").next())
                        .and_then(rgb),
                );
            }
        }
        if let Some(n) = kids(root, "cellXfs").next() {
            for x in kids(n, "xf") {
                let num = |k: &str| {
                    x.attribute(k)
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0)
                };
                let (bold, italic, color) = fonts.get(num("fontId")).cloned().unwrap_or_default();
                let fmt = num("numFmtId") as u32;
                let code = codes
                    .get(&fmt)
                    .cloned()
                    .unwrap_or_else(|| builtin_format(fmt).to_owned());
                let align = kids(x, "alignment")
                    .next()
                    .and_then(|a| a.attribute("horizontal"))
                    .and_then(|h| match h {
                        "left" => Some('l'),
                        "center" | "centerContinuous" => Some('c'),
                        "right" => Some('r'),
                        _ => None,
                    });
                xfs.push(Format {
                    style: CellStyle {
                        bold,
                        italic,
                        color,
                        fill: fills.get(num("fillId")).copied().flatten(),
                        align,
                    },
                    code,
                });
            }
        }
    }
    let sheet_xml = zip_text(&mut zip, &path)?;
    let doc = roxmltree::Document::parse(&sheet_xml)?;
    let mut out = std::collections::HashMap::new();
    for c in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "c")
    {
        let (Some(r), Some(s)) = (
            c.attribute("r"),
            c.attribute("s").and_then(|v| v.parse::<usize>().ok()),
        ) else {
            continue;
        };
        let Some(f) = xfs.get(s) else { continue };
        if f.style == CellStyle::default() && f.code.is_empty() {
            continue;
        }
        let letters: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let Ok(row) = r[letters.len()..].parse::<usize>() else {
            continue;
        };
        let col = letters.bytes().fold(0usize, |n, b| {
            n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize
        });
        if row == 0 || col == 0 {
            continue;
        }
        out.insert((row - 1, col - 1), f.clone());
        if out.len() > 200_000 {
            break;
        }
    }
    Ok(out)
}

fn builtin_format(id: u32) -> &'static str {
    match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 | 37 => "$#,##0",
        6 | 7 | 8 | 38 | 39 | 40 => "$#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        _ => "",
    }
}

/// A number shown with an Excel number format: percent, currency, thousands separators and a
/// fixed number of decimals. Formats this doesn't understand (dates, fractions) give `None`.
pub(crate) fn number(v: f64, code: &str) -> Option<String> {
    let section = code.split(';').next()?.trim();
    if section.is_empty() || section.eq_ignore_ascii_case("general") {
        return None;
    }
    // Drop colours, conditions and quoted text; keep a currency symbol from [$€-…].
    let mut symbol = String::new();
    let mut plain = String::new();
    let mut chars = section.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {
                let inner: String = chars.by_ref().take_while(|&c| c != ']').collect();
                if let Some(s) = inner.strip_prefix('$') {
                    symbol = s.split('-').next().unwrap_or("").to_owned();
                }
            }
            '"' => {
                let lit: String = chars.by_ref().take_while(|&c| c != '"').collect();
                if lit.trim().len() <= 3 && symbol.is_empty() && !lit.trim().is_empty() {
                    symbol = lit.trim().to_owned();
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            _ => plain.push(c),
        }
    }
    let lower = plain.to_ascii_lowercase();
    if lower.contains(['y', 'd', 'h', 's'])
        || lower.contains("m")
        || plain.contains('/')
        || plain.contains('E')
    {
        return None;
    }
    if !plain.contains(['0', '#']) {
        return None;
    }
    for sym in ['$', '€', '£', '¥'] {
        if plain.contains(sym) && symbol.is_empty() {
            symbol = sym.to_string();
        }
    }
    let percent = plain.contains('%');
    let decimals = plain
        .split_once('.')
        .map(|(_, d)| d.chars().filter(|c| *c == '0' || *c == '#').count())
        .unwrap_or(0);
    let grouped = plain.contains(',');
    let value = if percent { v * 100. } else { v };
    let fixed = format!("{:.*}", decimals, value.abs());
    let (int, frac) = fixed
        .split_once('.')
        .map(|(a, b)| (a.to_owned(), Some(b.to_owned())))
        .unwrap_or((fixed.clone(), None));
    let int = if grouped {
        let digits: Vec<char> = int.chars().collect();
        let mut out = String::new();
        for (i, d) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(*d);
        }
        out
    } else {
        int
    };
    let body = match frac {
        Some(f) => format!("{int}.{f}"),
        None => int,
    };
    let sign = if value < 0. && fixed.chars().any(|c| c != '0' && c != '.') {
        "-"
    } else {
        ""
    };
    Some(format!(
        "{sign}{symbol}{body}{}",
        if percent { "%" } else { "" }
    ))
}

fn trusted_helper(name: &str) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        for env in ["ProgramW6432", "ProgramFiles"] {
            if let Some(base) = std::env::var_os(env) {
                let path = PathBuf::from(base)
                    .join("Git/mingw64/bin")
                    .join(format!("{name}.exe"));
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let p = Path::new("/usr/bin").join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    bail!("Install {name} (included with Git for Windows) to extract this document's text")
}

fn convert(name: &str, args: &[&str], bytes: &[u8], suffix: &str) -> Result<String> {
    let exe = trusted_helper(name)?;
    let mut input = tempfile::Builder::new().suffix(suffix).tempfile()?;
    input.write_all(bytes)?;
    input.flush()?;
    let input = input.into_temp_path();
    let mut output = tempfile::tempfile()?;
    let mut diagnostics = tempfile::tempfile()?;
    let mut cmd = Command::new(&exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    if name == "antiword" {
        if let Some(base) = exe.parent().and_then(Path::parent) {
            cmd.env("ANTIWORDHOME", base.join("share/antiword"));
        }
    }
    cmd.current_dir(exe.parent().context("Missing helper directory")?)
        .args(args)
        .arg(crate::projects::display_path(&input));
    if name == "pdftotext" {
        cmd.arg("-");
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(diagnostics.try_clone()?))
        .spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(20) || output.metadata()?.len() > MAX_TEXT as u64 {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Document conversion exceeded the preview limit");
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    use std::io::Seek;
    if !status.success() {
        diagnostics.rewind()?;
        let mut reason = Vec::new();
        diagnostics.take(800).read_to_end(&mut reason)?;
        bail!(
            "{name} failed ({status}): {}",
            String::from_utf8_lossy(&reason).trim()
        );
    }
    output.rewind()?;
    let mut data = Vec::new();
    output.take(MAX_TEXT as u64 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= MAX_TEXT, "Extracted text exceeds 8 MiB");
    decode(&data)
}

fn table(
    doc: &mut Document,
    name: &str,
    rows: impl Iterator<Item = Result<Vec<String>>>,
    start: (usize, usize),
    budget: &mut usize,
    formats: Option<&std::collections::HashMap<(usize, usize), Format>>,
) -> Result<()> {
    let before = doc.sections.len();
    let mut page = vec![];
    let mut row = start.0;
    let append = |doc: &mut Document, page: Vec<Vec<String>>, row: usize| {
        let n = page.len();
        let styles = match formats {
            Some(f) if !f.is_empty() => page
                .iter()
                .enumerate()
                .map(|(i, cells)| {
                    (0..cells.len())
                        .map(|c| {
                            f.get(&(row + i, start.1 + c))
                                .map(|x| x.style.clone())
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect(),
            _ => vec![],
        };
        doc.sections.push(Section {
            title: format!("{name} · rows {}–{}", row + 1, row + n.max(1)),
            body: Body::Table {
                rows: page,
                first_row: row,
                first_col: start.1,
                styles,
            },
        });
    };
    for cells in rows {
        let cells = cells?;
        let cost = cells.iter().map(|s| s.len() + 32).sum::<usize>() + 32;
        ensure!(
            cost <= *budget,
            "Workbook or CSV is too large for the preview"
        );
        *budget -= cost;
        page.push(cells);
        if page.len() == 30 {
            append(doc, std::mem::take(&mut page), row);
            row += 30;
        }
    }
    if !page.is_empty() || before == doc.sections.len() {
        append(doc, page, row);
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<Document> {
    let f = File::open(path)?;
    let meta = f.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= MAX_FILE,
        "Choose a regular file up to 32 MiB"
    );
    let mut bytes = vec![];
    f.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_FILE, "File exceeds 32 MiB");
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    parse(&ext, bytes)
}

/// Lowercase extension of a path, or an empty string.
pub fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

/// True when the reader can open a file with this name's extension.
pub fn readable(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// Parse document bytes by extension, such as a generated file's thread snapshot.
pub fn parse(ext: &str, bytes: Vec<u8>) -> Result<Document> {
    ensure!(bytes.len() as u64 <= MAX_FILE, "File exceeds 32 MiB");
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => Document::text("Markdown", decode(&bytes)?),
        "txt" => Document::text("Text", decode(&bytes)?),
        "rtf" => Document::text("Rich Text", rtf_text(&String::from_utf8_lossy(&bytes))?),
        "html" | "htm" => Document::text(
            "HTML",
            html2text::from_read(decode(&bytes)?.as_bytes(), 100)?,
        ),
        "docx" => Document::text("Word", docx_markdown(&bytes)?),
        "doc" => Document::text(
            "Word",
            convert("antiword", &["-m", "UTF-8.txt", "-w", "0"], &bytes, ".doc")?,
        ),
        "csv" | "tsv" => {
            let text = decode(&bytes)?;
            let mut csv = csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .delimiter(if ext == "tsv" { b'\t' } else { b',' })
                .from_reader(text.as_bytes());
            let mut doc = Document::new(if ext == "tsv" { "TSV" } else { "CSV" });
            let mut budget = MAX_TEXT;
            table(
                &mut doc,
                "Table",
                csv.records()
                    .map(|r| Ok(r?.iter().map(str::to_owned).collect())),
                (0, 0),
                &mut budget,
                None,
            )?;
            Ok(doc)
        }
        "xls" | "xlsx" | "xlsm" | "xlsb" => {
            if ext != "xls" {
                archive(&bytes)?;
            }
            let bytes_copy = bytes.clone();
            let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))?;
            let mut doc = Document::new("Excel");
            let mut budget = MAX_TEXT;
            for name in book.sheet_names().to_vec() {
                let dimensions = match &mut book {
                    calamine::Sheets::Xlsx(w) => {
                        Some(w.worksheet_cells_reader(&name)?.dimensions())
                    }
                    calamine::Sheets::Xlsb(w) => {
                        Some(w.worksheet_cells_reader(&name)?.dimensions())
                    }
                    _ => None,
                };
                ensure!(
                    dimensions.is_none_or(|d| d.len() <= 1_000_000),
                    "Sheet {name} has too large a used range to preview"
                );
                let range = book.worksheet_range(&name)?;
                let start = range.start().unwrap_or((0, 0));
                let formats = (ext == "xlsx" || ext == "xlsm")
                    .then(|| xlsx_formats(&bytes_copy, &name).ok())
                    .flatten()
                    .unwrap_or_default();
                let (r0, c0) = (start.0 as usize, start.1 as usize);
                table(
                    &mut doc,
                    &name,
                    range.rows().enumerate().map(|(r, row)| {
                        Ok(row
                            .iter()
                            .enumerate()
                            .map(|(c, cell)| {
                                let shown = match (cell, formats.get(&(r0 + r, c0 + c))) {
                                    (calamine::Data::Float(v), Some(f)) => number(*v, &f.code),
                                    (calamine::Data::Int(v), Some(f)) => number(*v as f64, &f.code),
                                    _ => None,
                                };
                                shown.unwrap_or_else(|| cell.to_string())
                            })
                            .collect())
                    }),
                    (r0, c0),
                    &mut budget,
                    Some(&formats),
                )?;
            }
            ensure!(!doc.sections.is_empty(), "Workbook has no sheets");
            Ok(doc)
        }
        "pdf" => pdf(&bytes),
        _ => bail!("Unsupported document extension: {ext}"),
    }
}

fn pdf(bytes: &[u8]) -> Result<Document> {
    let mut doc = Document::new("PDF");
    #[cfg(windows)]
    let count = {
        use windows::{
            Data::Pdf::PdfDocument,
            Storage::Streams::{DataWriter, InMemoryRandomAccessStream},
        };
        let stream = InMemoryRandomAccessStream::new()?;
        let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0)?)?;
        writer.WriteBytes(bytes)?;
        writer.StoreAsync()?.get()?;
        writer.DetachStream()?;
        stream.Seek(0)?;
        let pdf = PdfDocument::LoadFromStreamAsync(&stream)?.get()?;
        let count = pdf.PageCount()? as usize;
        doc.pdf = Some(pdf);
        count
    };
    let text = convert("pdftotext", &["-enc", "UTF-8", "-layout"], bytes, ".pdf");
    #[cfg(not(windows))]
    let count = text
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .trim_end_matches('\x0c')
        .split('\x0c')
        .count();
    ensure!(
        (1..=5000).contains(&count),
        "PDF page count exceeds the preview limit"
    );
    if let Err(e) = &text {
        doc.note = format!("Page images available. Text extraction: {e}");
    }
    let text = text.unwrap_or_default();
    let mut pages = text.split('\x0c');
    for n in 1..=count {
        doc.sections.push(Section {
            title: format!("Page {n}"),
            body: Body::Text(pages.next().unwrap_or("").trim().into()),
        });
    }
    Ok(doc)
}

#[cfg(windows)]
pub fn render_pdf(
    pdf: &windows::Data::Pdf::PdfDocument,
    index: usize,
) -> Result<eframe::egui::ColorImage> {
    use windows::{
        Data::Pdf::PdfPageRenderOptions,
        Storage::Streams::{DataReader, InMemoryRandomAccessStream},
    };
    let page = pdf.GetPage(index as u32)?;
    let size = page.Size()?;
    let options = PdfPageRenderOptions::new()?;
    let scale = 1500. / size.Width.max(size.Height).max(1.);
    options.SetDestinationWidth((size.Width * scale).max(1.) as u32)?;
    options.SetDestinationHeight((size.Height * scale).max(1.) as u32)?;
    let stream = InMemoryRandomAccessStream::new()?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)?
        .get()?;
    page.Close()?;
    let count = u32::try_from(stream.Size()?)?;
    ensure!(
        count <= 16 * 1024 * 1024,
        "Rendered page exceeds preview limit"
    );
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    ensure!(
        reader.LoadAsync(count)?.get()? == count,
        "Incomplete rendered page"
    );
    let mut data = vec![0; count as usize];
    reader.ReadBytes(&mut data)?;
    let image = image::load_from_memory(&data)?.to_rgba8();
    Ok(eframe::egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}
