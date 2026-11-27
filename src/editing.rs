//! Side-pane editing for text, tables and basic Office files.
//!
//! Spindle writes Word and Excel files with a minimal writer that keeps text and cell values
//! only. Saving over a `.docx` or `.xlsx` is therefore lossy for files with styles, formulas,
//! images or charts, and the desktop asks before doing it.

use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{Cursor, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_CELLS: usize = 200_000;
const MAX_TEXT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Sheets(Vec<Sheet>),
}

pub struct Editable {
    pub path: PathBuf,
    pub content: Content,
    /// Saving over the original drops formatting Spindle cannot reproduce.
    pub lossy: bool,
    modified: Option<SystemTime>,
}

fn ext(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

/// File types the side pane can edit and save.
pub fn editable(path: &Path) -> bool {
    matches!(
        ext(path).as_str(),
        "md" | "markdown" | "txt" | "csv" | "tsv" | "docx" | "xlsx" | "rtf"
    )
}

/// File types Spindle can write back in their own format.
pub fn writable(path: &Path) -> bool {
    editable(path) && ext(path) != "rtf"
}

/// Extensions a document can be saved as without changing its kind of content.
pub fn save_as_extensions(content: &Content) -> &'static [&'static str] {
    match content {
        Content::Text(_) => &["md", "txt", "docx"],
        Content::Sheets(_) => &["xlsx", "csv", "tsv"],
    }
}

fn parse_table(text: &str, delimiter: u8) -> Result<Vec<Vec<String>>> {
    let mut rows = vec![];
    for record in csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes())
        .records()
    {
        rows.push(record?.iter().map(str::to_owned).collect());
    }
    Ok(rows)
}

fn cells(sheets: &[Sheet]) -> usize {
    sheets.iter().flat_map(|s| &s.rows).map(Vec::len).sum()
}

/// Load a file for editing from its bytes. `path` decides the format.
pub fn parse(path: &Path, bytes: &[u8]) -> Result<Content> {
    let content = match ext(path).as_str() {
        "md" | "markdown" | "txt" => Content::Text(crate::documents::decode(bytes)?),
        "csv" => Content::Sheets(vec![Sheet {
            name: "Table".into(),
            rows: parse_table(&crate::documents::decode(bytes)?, b',')?,
        }]),
        "tsv" => Content::Sheets(vec![Sheet {
            name: "Table".into(),
            rows: parse_table(&crate::documents::decode(bytes)?, b'\t')?,
        }]),
        "docx" => Content::Text(crate::documents::docx(bytes)?),
        "rtf" => Content::Text(crate::documents::rtf_text(&String::from_utf8_lossy(bytes))?),
        "xlsx" => {
            use calamine::Reader;
            let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))?;
            let mut sheets = vec![];
            let mut total = 0usize;
            for name in book.sheet_names().to_vec() {
                let range = book.worksheet_range(&name)?;
                let (top, left) = range
                    .start()
                    .map(|(r, c)| (r as usize, c as usize))
                    .unwrap_or_default();
                total = total.saturating_add((top + range.height()) * (left + range.width()));
                ensure!(total <= MAX_CELLS, "Workbook is too large to edit here");
                let mut rows = vec![vec![]; top];
                for row in range.rows() {
                    let mut cells = vec![String::new(); left];
                    cells.extend(row.iter().map(ToString::to_string));
                    rows.push(cells);
                }
                sheets.push(Sheet { name, rows });
            }
            ensure!(!sheets.is_empty(), "Workbook has no sheets");
            Content::Sheets(sheets)
        }
        other => bail!("Spindle cannot edit .{other} files"),
    };
    match &content {
        Content::Text(t) => ensure!(t.len() <= MAX_TEXT, "Text exceeds 8 MiB"),
        Content::Sheets(s) => ensure!(cells(s) <= MAX_CELLS, "Table is too large to edit here"),
    }
    Ok(content)
}

impl Editable {
    pub fn open(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)?;
        ensure!(
            meta.is_file() && meta.len() <= 32 * 1024 * 1024,
            "Choose a regular file up to 32 MiB"
        );
        let bytes = std::fs::read(path)?;
        Ok(Self {
            content: parse(path, &bytes)?,
            lossy: matches!(ext(path).as_str(), "docx" | "xlsx"),
            modified: meta.modified().ok(),
            path: path.into(),
        })
    }

    /// Edit a generated file's thread snapshot. It can only be saved as a new file.
    pub fn snapshot(name: &str, text: &str) -> Result<Self> {
        let path = PathBuf::from(name);
        Ok(Self {
            content: parse(&path, text.as_bytes())?,
            lossy: false,
            modified: None,
            path,
        })
    }

    /// Replace the original. Refuses if the file changed on disk since it was opened.
    pub fn save(&mut self) -> Result<()> {
        let current = std::fs::metadata(&self.path)
            .context("The original file is unavailable. Use Save as.")?
            .modified()
            .ok();
        ensure!(
            self.modified.is_some() && current == self.modified,
            "This file changed on disk after you opened it. Use Save as, or reload and edit again."
        );
        let bytes = encode(&ext(&self.path), &self.content)?;
        crate::file_history::write(&self.path, &bytes, true)?;
        self.modified = std::fs::metadata(&self.path)?.modified().ok();
        Ok(())
    }
}

/// Default name for an edited copy beside the original.
pub fn copy_name(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = ext(path);
    let dir = path.parent().unwrap_or(Path::new(""));
    (1..1000)
        .map(|n| {
            dir.join(if n == 1 {
                format!("{stem}-edited.{ext}")
            } else {
                format!("{stem}-edited-{n}.{ext}")
            })
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem}-edited.{ext}")))
}

/// Write content to `path` atomically in the format its extension names.
pub fn write(path: &Path, content: &Content) -> Result<()> {
    let bytes = encode(&ext(path), content)?;
    crate::file_history::write(path, &bytes, path.is_file())?;
    Ok(())
}

pub fn encode(ext: &str, content: &Content) -> Result<Vec<u8>> {
    match (ext, content) {
        ("md" | "markdown" | "txt", Content::Text(t)) => Ok(t.clone().into_bytes()),
        ("docx", Content::Text(t)) => docx(t),
        ("csv" | "tsv", Content::Sheets(sheets)) => {
            ensure!(
                sheets.len() == 1,
                "Save a workbook with several sheets as .xlsx"
            );
            let mut out = csv::WriterBuilder::new()
                .flexible(true)
                .delimiter(if ext == "tsv" { b'\t' } else { b',' })
                .from_writer(vec![]);
            for row in &sheets[0].rows {
                if row.is_empty() {
                    out.write_record([""])?;
                } else {
                    out.write_record(row)?;
                }
            }
            Ok(out.into_inner()?)
        }
        ("xlsx", Content::Sheets(sheets)) => xlsx(sheets),
        _ => bail!(
            "Save this content as one of: {}",
            save_as_extensions(content).join(", ")
        ),
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || matches!(c, '\u{fffe}' | '\u{ffff}') => {}
            c => out.push(c),
        }
    }
    out
}

fn package(parts: &[(&str, String)]) -> Result<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in parts {
        zip.start_file(*name, options)?;
        zip.write_all(body.as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn docx(text: &str) -> Result<Vec<u8>> {
    let mut body = String::new();
    for line in text.lines() {
        if line.is_empty() {
            body.push_str("<w:p/>");
        } else {
            body.push_str(&format!(
                r#"<w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
                escape(line)
            ));
        }
    }
    package(&[
        (
            "[Content_Types].xml",
            format!(
                r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#
            ),
        ),
        (
            "_rels/.rels",
            format!(
                r#"{XML}<Relationships xmlns="{RELS}"><Relationship Id="rId1" Type="{OFFICE}/officeDocument" Target="word/document.xml"/></Relationships>"#
            ),
        ),
        (
            "word/document.xml",
            format!(
                r#"{XML}<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr/></w:body></w:document>"#
            ),
        ),
    ])
}

pub fn column(mut n: usize) -> String {
    let mut bytes = Vec::new();
    loop {
        bytes.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    bytes.reverse();
    String::from_utf8(bytes).unwrap()
}

fn numeric(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && digits.bytes().filter(|b| *b == b'.').count() <= 1
        && !digits.starts_with('.')
        && !digits.ends_with('.')
        && !(digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0."))
        && s.parse::<f64>().is_ok_and(f64::is_finite)
}

fn sheet_name(name: &str, index: usize, used: &mut Vec<String>) -> String {
    let mut clean: String = name
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
        .take(31)
        .collect();
    clean = clean.trim_matches('\'').trim().to_owned();
    if clean.is_empty() || used.iter().any(|u| u.eq_ignore_ascii_case(&clean)) {
        clean = format!("Sheet{}", index + 1);
    }
    used.push(clean.clone());
    clean
}

fn xlsx(sheets: &[Sheet]) -> Result<Vec<u8>> {
    ensure!(!sheets.is_empty(), "A workbook needs at least one sheet");
    let mut types = String::new();
    let mut entries = String::new();
    let mut rels = String::new();
    let mut parts = vec![];
    let mut used = vec![];
    for (i, sheet) in sheets.iter().enumerate() {
        let n = i + 1;
        types.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        ));
        entries.push_str(&format!(
            r#"<sheet name="{}" sheetId="{n}" r:id="rId{n}"/>"#,
            escape(&sheet_name(&sheet.name, i, &mut used))
        ));
        rels.push_str(&format!(
            r#"<Relationship Id="rId{n}" Type="{OFFICE}/worksheet" Target="worksheets/sheet{n}.xml"/>"#
        ));
        let mut data = String::new();
        for (r, row) in sheet.rows.iter().enumerate() {
            if row.iter().all(String::is_empty) {
                continue;
            }
            data.push_str(&format!(r#"<row r="{}">"#, r + 1));
            for (c, value) in row.iter().enumerate().filter(|(_, v)| !v.is_empty()) {
                let cell = format!("{}{}", column(c), r + 1);
                if numeric(value) {
                    data.push_str(&format!(r#"<c r="{cell}"><v>{value}</v></c>"#));
                } else {
                    data.push_str(&format!(
                        r#"<c r="{cell}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                        escape(value)
                    ));
                }
            }
            data.push_str("</row>");
        }
        parts.push((
            format!("xl/worksheets/sheet{n}.xml"),
            format!(
                r#"{XML}<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#
            ),
        ));
    }
    let mut all = vec![
        (
            "[Content_Types].xml".to_owned(),
            format!(
                r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>{types}</Types>"#
            ),
        ),
        (
            "_rels/.rels".to_owned(),
            format!(
                r#"{XML}<Relationships xmlns="{RELS}"><Relationship Id="rId1" Type="{OFFICE}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
            ),
        ),
        (
            "xl/workbook.xml".to_owned(),
            format!(
                r#"{XML}<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{OFFICE}"><sheets>{entries}</sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_owned(),
            format!(r#"{XML}<Relationships xmlns="{RELS}">{rels}</Relationships>"#),
        ),
    ];
    all.extend(parts);
    package(
        &all.iter()
            .map(|(n, b)| (n.as_str(), b.clone()))
            .collect::<Vec<_>>(),
    )
}

/// Markdown formatting applied from the editor toolbar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    Bold,
    Italic,
    Code,
    Heading,
    Bullet,
    Numbered,
    Link,
    Table,
}

fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// Apply `format` to the selection given in character offsets. Returns the new selection.
pub fn apply(text: &mut String, selection: (usize, usize), format: Format) -> (usize, usize) {
    let (a, b) = (selection.0.min(selection.1), selection.0.max(selection.1));
    let (start, end) = (byte_index(text, a), byte_index(text, b));
    let chars = |s: &str| s.chars().count();
    let wrap = |text: &mut String, marker: &str, placeholder: &str| {
        let inner = if start == end {
            placeholder.to_owned()
        } else {
            text[start..end].to_owned()
        };
        text.replace_range(start..end, &format!("{marker}{inner}{marker}"));
        let m = chars(marker);
        (a + m, a + m + chars(&inner))
    };
    match format {
        Format::Bold => wrap(text, "**", "bold text"),
        Format::Italic => wrap(text, "*", "italic text"),
        Format::Code => wrap(text, "`", "code"),
        Format::Link => {
            let label = if start == end {
                "link text".to_owned()
            } else {
                text[start..end].to_owned()
            };
            text.replace_range(start..end, &format!("[{label}](https://)"));
            let url = a + chars(&label) + 3;
            (url, url + 8)
        }
        Format::Table => {
            let table = "\n| Column 1 | Column 2 |\n| --- | --- |\n|  |  |\n";
            text.insert_str(end, table);
            (b + chars(table), b + chars(table))
        }
        Format::Heading | Format::Bullet | Format::Numbered => {
            let prefix = match format {
                Format::Heading => "## ",
                Format::Bullet => "- ",
                _ => "1. ",
            };
            let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
            let lines: Vec<&str> = text[line_start..line_end].split('\n').collect();
            let remove = lines.iter().all(|l| l.starts_with(prefix));
            let changed: Vec<String> = lines
                .iter()
                .map(|l| {
                    if remove {
                        l[prefix.len()..].to_owned()
                    } else {
                        format!("{prefix}{l}")
                    }
                })
                .collect();
            let joined = changed.join("\n");
            text.replace_range(line_start..line_end, &joined);
            let first = chars(&text[..line_start]);
            (first, first + chars(&joined))
        }
    }
}

/// Changes to a sheet's shape from the row and column header menus.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    InsertRow(usize),
    DeleteRow(usize),
    InsertColumn(usize),
    DeleteColumn(usize),
}

pub fn reshape(rows: &mut Vec<Vec<String>>, change: Shape) {
    match change {
        Shape::InsertRow(i) => {
            let width = rows.iter().map(Vec::len).max().unwrap_or(1);
            rows.insert(i.min(rows.len()), vec![String::new(); width]);
        }
        Shape::DeleteRow(i) if i < rows.len() => {
            rows.remove(i);
        }
        Shape::InsertColumn(c) => {
            for row in rows.iter_mut() {
                if c <= row.len() {
                    row.insert(c, String::new());
                }
            }
        }
        Shape::DeleteColumn(c) => {
            for row in rows.iter_mut() {
                if c < row.len() {
                    row.remove(c);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_formats_selections_and_lines() {
        let mut t = "Héllo world".to_owned();
        assert_eq!(apply(&mut t, (0, 5), Format::Bold), (2, 7));
        assert_eq!(t, "**Héllo** world");
        let mut t = "one\ntwo".to_owned();
        apply(&mut t, (0, 7), Format::Bullet);
        assert_eq!(t, "- one\n- two");
        apply(&mut t, (0, 1), Format::Bullet);
        assert_eq!(t, "one\n- two");
        let mut t = String::new();
        assert_eq!(apply(&mut t, (0, 0), Format::Italic), (1, 12));
        assert_eq!(t, "*italic text*");
        let mut rows = vec![vec!["a".to_owned(), "b".to_owned()]];
        reshape(&mut rows, Shape::InsertColumn(1));
        reshape(&mut rows, Shape::InsertRow(0));
        assert_eq!(rows, vec![vec!["", "", ""], vec!["a", "", "b"]]);
        reshape(&mut rows, Shape::DeleteRow(0));
        reshape(&mut rows, Shape::DeleteColumn(0));
        assert_eq!(rows, vec![vec!["", "b"]]);
    }

    fn sheet(rows: &[&[&str]]) -> Vec<Sheet> {
        vec![Sheet {
            name: "Budget & Plan".into(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
        }]
    }

    #[test]
    fn workbook_round_trips_values_through_the_reader() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("budget.xlsx");
        let sheets = sheet(&[&["Item", "Cost"], &["Pens <blue>", "4.5"], &["", "007"]]);
        write(&path, &Content::Sheets(sheets.clone())).unwrap();
        let Content::Sheets(back) = parse(&path, &std::fs::read(&path).unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(back[0].name, "Budget & Plan");
        assert_eq!(back[0].rows[1], vec!["Pens <blue>", "4.5"]);
        // Leading zeros stay text, so the value is not changed to 7.
        assert_eq!(back[0].rows[2], vec!["", "007"]);
        let doc = crate::documents::read(&path).unwrap();
        assert_eq!(doc.kind, "Excel");
    }

    #[test]
    fn word_round_trips_paragraphs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.docx");
        write(&path, &Content::Text("Title\n\nFirst & second".into())).unwrap();
        let Content::Text(back) = parse(&path, &std::fs::read(&path).unwrap()).unwrap() else {
            panic!()
        };
        assert!(back.contains("Title") && back.contains("First & second"));
    }

    #[test]
    fn save_over_refuses_a_file_changed_on_disk_and_save_as_keeps_the_original() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("data.csv");
        std::fs::write(&path, "a,b\n1,2\n").unwrap();
        let mut edit = Editable::open(&path).unwrap();
        assert!(!edit.lossy);
        if let Content::Sheets(s) = &mut edit.content {
            s[0].rows[1][1] = "3, quoted".into();
        }
        edit.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a,b\n1,\"3, quoted\"\n"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "changed elsewhere\n").unwrap();
        let mut stale = Editable {
            modified: Some(SystemTime::UNIX_EPOCH),
            ..Editable::open(&path).unwrap()
        };
        assert!(stale.save().is_err());
        let copy = copy_name(&path);
        assert!(copy.ends_with("data-edited.csv"));
        write(&copy, &stale.content).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "changed elsewhere\n"
        );
    }

    #[test]
    fn formats_and_kinds_must_match() {
        assert!(editable(Path::new("a.XLSX")) && !editable(Path::new("a.xlsm")));
        assert!(encode("xlsx", &Content::Text("x".into())).is_err());
        let two = [sheet(&[&["a"]]), sheet(&[&["b"]])].concat();
        assert!(encode("csv", &Content::Sheets(two)).is_err());
        assert!(numeric("-12.5") && !numeric("1e5") && !numeric("012") && !numeric("NaN"));
    }
}
