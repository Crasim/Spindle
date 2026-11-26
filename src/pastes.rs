use crate::{
    artifacts,
    engine::Engine,
    generation,
    ollama::Message,
    providers::route,
    state::Attachment,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FILES: usize = 8;
pub fn large(text: &str) -> bool {
    text.len() >= 4096 || text.lines().take(40).count() >= 40
}

#[derive(Serialize, Deserialize)]
pub struct Submission {
    pub thread: u64,
    pub model: String,
    pub prompt: String,
    pub pastes: Vec<String>,
    /// File names for attached files, "" for pasted text. Missing for older drafts.
    #[serde(default)]
    pub names: Vec<String>,
}

/// The text of a file the user attached: documents (Word, PDF, spreadsheets) as their text,
/// anything else as UTF-8. Binary files are refused.
pub fn file_text(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "doc" | "docx" | "pdf" | "xls" | "xlsx" | "xlsm" | "xlsb"
    ) {
        let doc = crate::documents::read(path)?;
        let text = doc
            .sections
            .iter()
            .map(|s| {
                if s.title.is_empty() {
                    s.text()
                } else {
                    format!("## {}\n{}", s.title, s.text())
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        ensure!(
            !text.trim().is_empty(),
            "{} has no text Spindle can read",
            path.display()
        );
        return Ok(text);
    }
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "{} is larger than {} MiB",
        path.display(),
        MAX_BYTES >> 20
    );
    let text = String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!(
            "{} isn't a text file. Attach text, code, logs, HAR/JSON, CSV, Word, PDF or Excel files.",
            path.display()
        )
    })?;
    ensure!(!text.contains('\0'), "{} isn't a text file", path.display());
    Ok(text)
}

/// A safe file name for an attachment: the last path part, with anything odd replaced.
fn attachment_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next()?.trim();
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim_start_matches(['.', '-', ' ']).trim().to_owned();
    (!clean.is_empty() && clean.len() <= 120).then_some(clean)
}

fn save(root: &Path, thread: u64, text: &str, wanted: Option<String>) -> Result<String> {
    // A retry can reuse a file committed before an interruption.
    let hash = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    let suffix: String = hash.as_ref()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let dir = artifacts::folder(root, thread)?;
    let name = match wanted {
        // The same name with other content gets a short tag, so nothing is overwritten.
        Some(n) => match fs::read(dir.join(&n)) {
            Err(_) => n,
            Ok(old) if old == text.as_bytes() => n,
            Ok(_) => match n.rsplit_once('.') {
                Some((stem, ext)) => format!("{stem}-{}.{ext}", &suffix[..6]),
                None => format!("{n}-{}", &suffix[..6]),
            },
        },
        None => format!("paste-{suffix}.txt"),
    };
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    if let Err(e) = temp.persist_noclobber(dir.join(&name)) {
        if e.error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(e.into());
        }
        let existing = artifacts::existing_file(root, thread, &name)?;
        ensure!(
            fs::metadata(&existing)?.len() == text.len() as u64
                && fs::read(existing)? == text.as_bytes(),
            "A different file already uses this paste name"
        );
    }
    Ok(name)
}

impl Engine {
    pub fn submit_pastes(&mut self, input: Submission) -> Result<String> {
        ensure!(
            self.state.chat_id == input.thread && self.state.model == input.model,
            "Thread or model changed. Review your draft and send again"
        );
        ensure!(
            self.models.contains(&input.model),
            "Select a connected model"
        );
        ensure!(
            (1..=MAX_FILES).contains(&input.pastes.len())
                && input.pastes.iter().map(String::len).sum::<usize>() <= MAX_BYTES,
            "A message supports up to {MAX_FILES} attached files totaling {} MiB",
            MAX_BYTES >> 20
        );
        let mut text = if input.prompt.trim().is_empty() {
            "Review the attached text.".into()
        } else {
            input.prompt
        };
        let mut files = vec![];
        // In a project chat with the sandbox on, the model's tools work in the project's copy,
        // so attachments also go there (in a folder that is never applied to the project).
        let copy = self
            .state
            .thread_data(input.thread)
            .and_then(|(t, _)| t.project.clone())
            .filter(|_| crate::execution::ready(&self.state))
            .and_then(|p| {
                crate::workcopy::ensure(&crate::runtime::data_dir(), input.thread, &p).ok()
            });
        for (i, body) in input.pastes.into_iter().enumerate() {
            let wanted = input.names.get(i).and_then(|n| attachment_name(n));
            let name = save(
                &self.state.files_root(&self.root, input.thread),
                input.thread,
                &body,
                wanted,
            )?;
            let place = match &copy {
                Some(copy) => {
                    let dir = copy.join(crate::workcopy::ATTACHMENTS);
                    fs::create_dir_all(&dir)?;
                    fs::write(dir.join(&name), &body)?;
                    format!(
                        " (your tools can read it as {}/{name})",
                        crate::workcopy::ATTACHMENTS
                    )
                }
                None => String::new(),
            };
            text.push_str(&format!("\nAttached file: {name}{place}"));
            files.push(Attachment {
                path: name,
                text: body,
            });
        }
        self.enqueue(&text, self.state.thread.message_mode(), String::new())?;
        let task = self
            .state
            .queue
            .back_mut()
            .context("Missing submitted task")?;
        task.chain.retain(|m| route(m).0 == route(&input.model).0);
        task.pastes = files;
        Ok(format!(
            "Pasted files queued for thread #{} · task #{}",
            input.thread, task.id
        ))
    }
}

pub fn add_evidence(files: &[Attachment], window: u64, messages: &mut [Message]) {
    if files.is_empty() {
        return;
    }
    let room = window.saturating_mul(2) / 3;
    let budget = (room.saturating_sub(generation::message_tokens(messages) + 128) * 3)
        .min(96 * 1024) as usize;
    let per_file = budget / files.len();
    let current = &mut messages[1].content;
    for file in files {
        current.push_str(&format!(
            "\n\nUser-attached file {} ({} bytes):\n",
            file.path,
            file.text.len()
        ));
        if file.text.len() <= per_file {
            current.push_str(&file.text);
            continue;
        }
        let mut head = per_file / 2;
        let mut tail = file.text.len() - per_file / 2;
        while !file.text.is_char_boundary(head) {
            head -= 1;
        }
        while !file.text.is_char_boundary(tail) {
            tail += 1;
        }
        current.push_str(&file.text[..head]);
        current.push_str("\n[… middle omitted to fit this request; full text is in the file …]\n");
        current.push_str(&file.text[tail..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_text_is_saved_exactly_and_excerpted_on_unicode_boundaries() {
        assert!(!large("short string"));
        assert!(large(&"x".repeat(4096)));
        assert!(large(&"x\n".repeat(40)));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        crate::projects::create(&path, "Paste test").unwrap();
        let body = format!("START{}END", "文🦀".repeat(3000));
        let name = save(&path, 3, &body, None).unwrap();
        assert_eq!(save(&path, 3, &body, None).unwrap(), name);
        assert_eq!(
            fs::read_to_string(path.join("artifacts").join(&name)).unwrap(),
            body
        );
        let files = [Attachment {
            path: name,
            text: body,
        }];
        let mut messages = [
            Message::new("system", "rules"),
            Message::new("user", "Review it"),
        ];
        add_evidence(&files, 2048, &mut messages);
        assert!(messages[1].content.contains("START") && messages[1].content.ends_with("END"));
        assert!(messages[1].content.contains("middle omitted"));
        assert!(messages[1].content.len() < files[0].text.len());
    }
}
