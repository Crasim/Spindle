use anyhow::{Context, Result, bail, ensure};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
};

pub fn safe_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let device = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    !name.starts_with(['.', '-'])
        && !stem.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && !matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.as_bytes()[3].is_ascii_digit())
        && matches!(
            ext.to_ascii_lowercase().as_str(),
            "html"
                | "htm"
                | "css"
                | "js"
                | "json"
                | "csv"
                | "tsv"
                | "md"
                | "txt"
                | "svg"
                | "rs"
                | "py"
                | "xml"
                | "ts"
                | "yaml"
                | "toml"
                | "tsx"
                | "jsx"
                | "go"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "java"
                | "cs"
                | "sql"
                | "sh"
                | "rb"
                | "yml"
                | "rtf"
        )
}

pub fn folder(root: &Path, thread: u64) -> Result<PathBuf> {
    let (base, parts) = if let Some(p) = crate::projects::existing(root)? {
        (p.path, vec!["artifacts".into()])
    } else {
        let mut hash = DefaultHasher::new();
        root.hash(&mut hash);
        (
            std::env::temp_dir().canonicalize()?,
            vec![
                "Spindle".into(),
                format!("{:016x}", hash.finish()),
                format!("thread-{thread}"),
            ],
        )
    };
    let mut path = base;
    for part in parts {
        path.push(part);
        if let Err(e) = fs::create_dir(&path) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(e.into());
            }
        }
        ensure!(
            path.is_dir() && path.canonicalize()? == path,
            "Artifact folder must stay inside the workspace"
        );
    }
    Ok(path)
}

pub fn save(root: &Path, thread: u64, name: &str, body: &str) -> Result<PathBuf> {
    ensure!(
        safe_name(name),
        "Choose a simple file name and a supported output extension"
    );
    ensure!(
        body.len() <= 2 * 1024 * 1024,
        "File exceeds the 2 MiB artifact limit"
    );
    let dir = folder(root, thread)?;
    let (stem, ext) = name.rsplit_once('.').unwrap();
    let mut file = tempfile::NamedTempFile::new_in(&dir)?;
    file.write_all(body.as_bytes())?;
    file.as_file().sync_all()?;
    for n in 1..=1000 {
        let path = dir.join(if n == 1 {
            name.to_owned()
        } else {
            format!("{stem}-{n}.{ext}")
        });
        match file.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => file = e.file,
            Err(e) => return Err(e.into()),
        }
    }
    bail!("Choose another artifact name")
}

pub fn html(info: &str) -> bool {
    matches!(
        info.split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "html" | "htm"
    )
}
pub fn name(info: &str, prompt: &str) -> Option<String> {
    if let Some(n) = info
        .split_whitespace()
        .find_map(|s| s.strip_prefix("filename="))
    {
        return safe_name(n).then(|| n.to_owned());
    }
    if !html(info) {
        return None;
    }
    Some(
        prompt
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .find(|s| safe_name(s) && s.to_ascii_lowercase().ends_with(".html"))
            .unwrap_or("index.html")
            .into(),
    )
}

fn inline_request(brief: &str) -> bool {
    [
        "show inline",
        "show it inline",
        "inline only",
        "inline example",
        "no file",
        "don't save",
        "do not save",
    ]
    .iter()
    .any(|v| brief.contains(v))
}

pub fn file_request(prompt: &str) -> bool {
    let brief = prompt.to_lowercase();
    if inline_request(&brief) {
        return false;
    }
    // "as a .docx", "save it to notes.md", "in rtf"
    if brief.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
        let ext = w.rsplit_once('.').map_or(w, |(_, e)| e);
        (w.contains('.')
            && (2..=5).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            && !ext.chars().all(|c| c.is_ascii_digit())
            || matches!(ext, "docx" | "xlsx" | "pptx" | "pdf" | "rtf" | "csv")
    }) {
        return true;
    }
    if [
        "single-file",
        "single file",
        "a script for",
        "a program that",
        "i need ",
        "i want ",
        "i'd like ",
        "i would like ",
        "give me ",
        "can i get ",
    ]
    .iter()
    .any(|x| brief.contains(x))
    {
        return true;
    }
    brief.split(|c: char| !c.is_alphabetic()).any(|w| {
        matches!(
            w,
            "build"
                | "create"
                | "generate"
                | "make"
                | "write"
                | "save"
                | "implement"
                | "refactor"
                | "fix"
                | "update"
                | "scaffold"
                | "modify"
                | "port"
        )
    })
}

pub struct SavedReply {
    pub text: String,
    pub files: Vec<crate::state::Attachment>,
}

fn output_name(info: &str, prompt: &str) -> Option<String> {
    if let Some(n) = name(info, prompt) {
        return Some(n);
    }
    if info.contains("filename=") {
        return None;
    }
    let ext = match info
        .split_whitespace()
        .next()?
        .to_ascii_lowercase()
        .as_str()
    {
        "python" | "py" => "py",
        "rust" | "rs" => "rs",
        "javascript" | "js" => "js",
        "typescript" | "ts" => "ts",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "go" => "go",
        "c" => "c",
        "cpp" | "c++" => "cpp",
        "java" => "java",
        "cs" | "csharp" => "cs",
        "sql" => "sql",
        "sh" | "bash" => "sh",
        "ruby" | "rb" => "rb",
        "css" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "csv" => "csv",
        "tsv" => "tsv",
        "xml" => "xml",
        "svg" => "svg",
        "markdown" | "md" => "md",
        "rtf" => "rtf",
        _ => return None,
    };
    Some(
        prompt
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .find(|s| {
                safe_name(s)
                    && s.rsplit_once('.')
                        .is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
            })
            .map(str::to_owned)
            .unwrap_or_else(|| format!("output.{ext}")),
    )
}

pub fn streaming_text<'a>(prompt: &str, answer: &'a str) -> std::borrow::Cow<'a, str> {
    let requested = file_request(prompt);
    let context = format!("{prompt}\n{answer}");
    let mut edits = vec![];
    for (event, range) in Parser::new(answer).into_offset_iter() {
        if let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event {
            // Memory and plan updates are shown as notes once the reply finishes.
            match info.trim() {
                "spindle-memory" => {
                    edits.push((range, String::new()));
                    continue;
                }
                "spindle-plan" => {
                    edits.push((range, "Updating the plan…\n".into()));
                    continue;
                }
                _ => {}
            }
            if !requested {
                continue;
            }
            if let Some(n) = output_name(&info, &context) {
                edits.push((range, format!("File in progress · {n}\n")));
            }
        }
    }
    if edits.is_empty() {
        return answer.into();
    }
    let mut result = answer.to_owned();
    for (range, brief) in edits.into_iter().rev() {
        result.replace_range(range, &brief);
    }
    result.into()
}

fn file_reply(
    prompt: &str,
    answer: &str,
    mut store: impl FnMut(&str, &str) -> Result<Option<String>>,
) -> SavedReply {
    let mut result = SavedReply {
        text: answer.into(),
        files: vec![],
    };
    let requested = file_request(prompt);
    let context = format!("{prompt}\n{answer}");
    let mut edits = vec![];
    let mut errors = vec![];
    let mut events = Parser::new(answer).into_offset_iter();
    while let Some((event, range)) = events.next() {
        let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event else {
            continue;
        };
        let mut body = String::new();
        let mut end = range.end;
        for (inner, r) in events.by_ref() {
            match inner {
                Event::End(TagEnd::CodeBlock) => {
                    end = r.end;
                    break;
                }
                Event::Text(t) => body.push_str(&t),
                _ => {}
            }
        }
        if !requested {
            continue;
        }
        let Some(n) = output_name(&info, &context) else {
            if let Some(n) = info
                .split_whitespace()
                .find_map(|s| s.strip_prefix("filename="))
            {
                errors.push(format!(
                    "Spindle did not save {n}: it writes plain-text files only. \
                     Ask for a plain-text format or a Python script that builds this file."
                ));
            }
            continue;
        };
        match store(&n, &body) {
            Ok(Some(path)) => {
                edits.push((range.start..end, format!("Saved file: {path}\n")));
                result
                    .files
                    .push(crate::state::Attachment { path, text: body });
            }
            Ok(None) => {}
            Err(e) => errors.push(format!("Could not save {n}: {e}")),
        }
    }
    for (range, label) in edits.into_iter().rev() {
        result.text.replace_range(range, &label);
    }
    for e in errors {
        result.text.push_str(&format!("\n\n{e}"));
    }
    result
}

pub fn outputs(root: &Path, id: u64, prompt: &str, answer: &str) -> SavedReply {
    file_reply(prompt, answer, |n, body| {
        Ok(Some(
            save(root, id, n, body)?
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into(),
        ))
    })
}

// Existing encrypted sessions kept file bodies in the message. Use only recorded saves;
// unmatched examples and failed writes stay in the original reply.
pub fn upgrade_history(
    thread: &mut crate::state::ThreadInfo,
    history: &mut [crate::ollama::Message],
) {
    for (&index, turn) in &mut thread.turns {
        if index == 0
            || index >= history.len()
            || turn.files.is_empty()
            || !turn.artifacts.is_empty()
        {
            continue;
        }
        let mut names = turn.files.iter().peekable();
        let saved = file_reply(
            &history[index - 1].content,
            &history[index].content,
            |expected, _| {
                let Some(n) = names.peek() else {
                    return Ok(None);
                };
                let (stem, ext) = expected.rsplit_once('.').unwrap();
                let numbered = n
                    .strip_prefix(&format!("{stem}-"))
                    .and_then(|s| s.strip_suffix(&format!(".{ext}")))
                    .is_some_and(|s| s.parse::<usize>().is_ok());
                if n.as_str() == expected || numbered {
                    Ok(names.next().cloned())
                } else {
                    Ok(None)
                }
            },
        );
        if !saved.files.is_empty() {
            history[index].content = saved.text;
            turn.artifacts = saved.files;
        }
    }
}

pub fn existing_file(root: &Path, thread: u64, name: &str) -> Result<PathBuf> {
    // Any plain file name in the chat's folder: tools and scripts make files (such as .docx)
    // that the text-file writer itself would not.
    ensure!(
        !name.is_empty()
            && !name.starts_with(['.', '-'])
            && !name.contains(['/', '\\', ':'])
            && name != "..",
        "Invalid generated filename"
    );
    let path = folder(root, thread)?.join(name);
    ensure!(
        path.is_file() && path.canonicalize()? == path,
        "File is unavailable in this workspace"
    );
    Ok(path)
}

/// Copy the files already introduced in the fork's saved transcript. Project
/// chats continue to use the same project files.
pub(crate) fn inherit_files(root: &Path, parent: u64, branch: u64, thread: &crate::state::ThreadInfo) -> Result<()> {
    if crate::projects::existing(root)?.is_some() {
        return Ok(());
    }
    let names: std::collections::BTreeSet<_> = thread.turns.values()
        .flat_map(|turn| turn.files.iter()).collect();
    if names.is_empty() {
        return Ok(());
    }
    let source = folder(root, parent)?;
    let dest = folder(root, branch)?;
    for name in names {
        let relative = Path::new(name);
        if !relative.components().all(|part| matches!(part, std::path::Component::Normal(_)))
            || crate::policy::protected(relative)
        {
            continue;
        }
        let from = source.join(relative);
        if from.canonicalize().ok().as_ref() != Some(&from) || !from.is_file() {
            continue;
        }
        let (to, inside) = crate::tools::resolve(&dest, name)?;
        ensure!(inside && to == dest.join(relative), "A branch file path left its scratch folder");
        let directory = to.parent().context("Missing branch folder")?;
        fs::create_dir_all(directory)?;
        ensure!(directory.canonicalize()? == directory, "A branch file path left its scratch folder");
        let mut tmp = tempfile::NamedTempFile::new_in(directory)?;
        std::io::copy(&mut fs::File::open(&from)?, &mut tmp)?;
        tmp.persist_noclobber(to)?;
    }
    Ok(())
}

pub fn read_html(path: &Path) -> Result<String> {
    ensure!(
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm")),
        "Choose an HTML file"
    );
    let f = fs::File::open(path)?;
    ensure!(f.metadata()?.is_file(), "Expected a regular HTML file");
    use std::io::Read;
    let mut body = String::new();
    f.take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut body)
        .context("Expected UTF-8 HTML")?;
    ensure!(body.len() <= 2 * 1024 * 1024, "Preview limit is 2 MiB");
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_blocks_and_extension_requests_become_files() {
        // No create verb, but the user named a format and the reply named the script.
        assert!(file_request("can you do it as a .docx?"));
        assert!(!file_request("what is 2.5 times 3?"));
        let reply = file_reply(
            "and as docx please",
            "Run make_story_docx.py to build it.\n~~~python\nprint(1)\n~~~",
            |n, _| Ok(Some(n.to_owned())),
        );
        assert_eq!(reply.files[0].path, "make_story_docx.py");
    }

    #[test]
    fn unsupported_binary_names_are_reported_not_dropped() {
        let reply = file_reply(
            "Create report.docx",
            "~~~xml filename=report.docx\n<w:document/>\n~~~\n~~~python filename=make_report.py\nprint(1)\n~~~",
            |n, _| Ok(Some(n.to_owned())),
        );
        assert_eq!(reply.files.len(), 1);
        assert_eq!(reply.files[0].path, "make_report.py");
        assert!(reply.text.contains("<w:document/>"));
        assert!(reply.text.contains("did not save report.docx"));
    }
    #[test]
    fn project_artifacts_are_scoped_and_never_replace_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(
            folder(&root, 17)
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        crate::projects::create(&root, "Snake").unwrap();
        let game = "~~~html filename=snake.html\n<button>Play</button>\n~~~";
        let first = outputs(&root, 17, "Build Snake", game);
        assert!(!first.text.contains("<button>"));
        assert_eq!(first.files[0].path, "snake.html");
        assert_eq!(
            outputs(&root, 17, "Build Snake", game).files[0].path,
            "snake-2.html"
        );
        assert_eq!(
            read_html(&root.join("artifacts/snake.html")).unwrap(),
            "<button>Play</button>\n"
        );
        assert!(
            outputs(&root, 17, "Explain the example", game)
                .files
                .is_empty()
        );
        for name in [
            "../snake.html",
            "NUL.html",
            "secret.txt:stream",
            "folder/game.html",
            ".git",
            "CON.foo.html",
        ] {
            assert!(!safe_name(name));
        }
    }
}
