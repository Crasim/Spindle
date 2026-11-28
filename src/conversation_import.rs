use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{ollama::Message, state::Conversation};

const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
pub enum Source {
    Codex,
    ClaudeCode,
}

pub struct LocalSession {
    pub path: PathBuf,
    pub title: String,
    pub origin: String,
    pub modified: SystemTime,
    pub archived: bool,
    pub workspace: Option<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Selection {
    pub path: PathBuf,
    pub title: String,
    pub origin: String,
    pub archived: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProjectSelection {
    pub workspace: PathBuf,
    pub sessions: Vec<Selection>,
}

#[derive(Default)]
struct Header {
    id: Option<String>,
    prompt: Option<String>,
    workspace: Option<PathBuf>,
}

fn guardian_session(payload: &Value) -> bool {
    payload["thread_source"] == "guardian_review"
        || payload["source"]["subagent"]["other"] == "guardian_review"
}

impl From<&LocalSession> for Selection {
    fn from(session: &LocalSession) -> Self {
        Self {
            path: session.path.clone(),
            title: session.title.clone(),
            origin: session.origin.clone(),
            archived: session.archived,
        }
    }
}

impl Source {
    fn prefix(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    fn home(self) -> Option<PathBuf> {
        let (variable, directory) = match self {
            Self::Codex => ("CODEX_HOME", ".codex"),
            Self::ClaudeCode => ("CLAUDE_CONFIG_DIR", ".claude"),
        };
        std::env::var_os(variable)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                Some(
                    PathBuf::from(std::env::var_os(if cfg!(windows) {
                        "USERPROFILE"
                    } else {
                        "HOME"
                    })?)
                    .join(directory),
                )
            })
    }

    pub fn discover(self) -> Vec<LocalSession> {
        self.home()
            .map_or_else(Vec::new, |home| self.discover_in(&home))
    }

    fn discover_in(self, home: &Path) -> Vec<LocalSession> {
        let mut paths = Vec::new();
        let names = match self {
            Self::Codex => {
                session_paths(&home.join("sessions"), 4, false, &mut paths);
                session_paths(&home.join("archived_sessions"), 1, true, &mut paths);
                json_lines(&home.join("session_index.jsonl"), 4 * 1024 * 1024)
                    .filter_map(|row| {
                        Some((
                            row["id"].as_str()?.to_owned(),
                            row["thread_name"].as_str()?.to_owned(),
                        ))
                    })
                    .collect::<HashMap<_, _>>()
            }
            Self::ClaudeCode => {
                session_paths(&home.join("projects"), 1, false, &mut paths);
                HashMap::new()
            }
        };
        paths.sort_unstable_by_key(|(_, modified, _)| std::cmp::Reverse(*modified));
        let mut seen = std::collections::HashSet::new();
        paths
            .into_iter()
            .filter_map(|(path, modified, archived)| {
                let header = self.header(&path)?;
                let id = header.id?;
                let origin = format!("{}:{id}", self.prefix());
                if !seen.insert(origin.clone()) {
                    return None;
                }
                Some(LocalSession {
                    title: conversation_title(names.get(&id).map(String::as_str), header.prompt.as_deref()),
                    path,
                    origin,
                    modified,
                    archived,
                    workspace: header.workspace,
                })
            })
            .collect()
    }

    fn header(self, path: &Path) -> Option<Header> {
        let mut header = Header::default();
        for row in json_lines(path, 512 * 1024).take(128) {
            let payload = &row["payload"];
            let prompt = match self {
                Self::Codex => {
                    if row["type"] == "session_meta" && header.id.is_none() {
                        if guardian_session(payload) { return None; }
                        header.id = payload["id"].as_str().map(str::to_owned);
                        header.workspace = workspace_path(&payload["cwd"]);
                    }
                    if row["type"] == "event_msg" && payload["type"] == "user_message" {
                        message("user", conversation_text(&payload["message"]))
                    } else if row["type"] == "response_item" && payload["type"] == "message" && payload["role"] == "user" {
                        message("user", conversation_text(&payload["content"]))
                    } else {
                        None
                    }
                }
                Self::ClaudeCode => {
                    if row["isSidechain"] == true { continue; }
                    if header.id.is_none() { header.id = row["sessionId"].as_str().map(str::to_owned); }
                    if header.workspace.is_none() { header.workspace = workspace_path(&row["cwd"]); }
                    if row["type"] == "user" && row["isSidechain"] != true && row["isMeta"] != true
                    {
                        message("user", conversation_text(&row["message"]["content"]))
                    } else {
                        None
                    }
                }
            };
            if header.prompt.is_none() {
                header.prompt = prompt.and_then(|m| prompt_title(&m.content).map(str::to_owned));
            }
            if header.id.is_some() && header.prompt.is_some() && (header.workspace.is_some() || matches!(self, Self::Codex)) {
                break;
            }
        }
        header.id.is_some().then_some(header)
    }
}

fn workspace_path(value: &Value) -> Option<PathBuf> {
    let path = PathBuf::from(value.as_str()?);
    (path.is_absolute() && path.parent().is_some()).then_some(path)
}

fn session_paths(
    root: &Path,
    depth: usize,
    archived: bool,
    paths: &mut Vec<(PathBuf, SystemTime, bool)>,
) {
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth > 0 {
            session_paths(&entry.path(), depth - 1, archived, paths);
        } else if kind.is_file()
            && entry.path().extension().is_some_and(|e| e == "jsonl")
            && let Ok(modified) = entry.metadata().and_then(|m| m.modified())
        {
            paths.push((entry.path(), modified, archived));
        }
    }
}

fn json_lines(path: &Path, limit: u64) -> impl Iterator<Item = Value> {
    File::open(path)
        .ok()
        .into_iter()
        .flat_map(move |file| BufReader::new(file.take(limit)).lines())
        .filter_map(|line| serde_json::from_str(line.ok()?.trim_start_matches('\u{feff}')).ok())
}

pub fn load(path: &Path) -> Result<Vec<Conversation>> {
    let mut file = File::open(path).context("Could not open the selected conversation file")?;
    let mut bytes = Vec::new();
    let mut signature = [0; 4];
    let is_zip = file.read_exact(&mut signature).is_ok() && &signature == b"PK\x03\x04";
    file.rewind()?;
    if is_zip
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        let mut zip = zip::ZipArchive::new(file).context("Could not read the export ZIP")?;
        let mut matches = zip
            .file_names()
            .filter(|name| name.rsplit('/').next() == Some("conversations.json"));
        let name = matches
            .next()
            .context("No conversations.json in the export ZIP")?
            .to_owned();
        ensure!(
            matches.next().is_none(),
            "Choose an export with one conversations.json file"
        );
        drop(matches);
        zip.by_name(&name)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
    } else {
        let mut reader = BufReader::new(file);
        if reader.fill_buf()?.starts_with(b"\xef\xbb\xbf") {
            reader.consume(3);
        }
        loop {
            let available = reader.fill_buf()?;
            let skip = available
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            if skip < available.len() {
                if available[skip] == b'[' {
                    reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                    break;
                }
                return Ok(vec![load_session(reader)?]);
            }
            if skip == 0 {
                bail!("The selected conversation file is empty");
            }
            reader.consume(skip);
        }
    }
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Claude export exceeds 64 MB"
    );
    let text = std::str::from_utf8(&bytes).context("Expected a UTF-8 conversation file")?;
    let text = text.trim_start_matches('\u{feff}').trim();
    let mut chats = Vec::new();
    if text.starts_with('[') {
        let export: Vec<Value> =
            serde_json::from_str(text).context("Could not read the Claude conversations export")?;
        for chat in export {
            let messages = chat["chat_messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    let role = if row["sender"] == "human" {
                        "user"
                    } else {
                        row["sender"].as_str()?
                    };
                    let body = conversation_text(&row["content"]);
                    message(
                        role,
                        if body.is_empty() {
                            conversation_text(&row["text"])
                        } else {
                            body
                        },
                    )
                })
                .collect();
            if let Some(mut chat_info) = conversation(chat["name"].as_str(), messages) {
                chat_info.thread.imported_from =
                    chat["uuid"].as_str().map(|id| format!("claude:{id}"));
                chats.push(chat_info);
            }
        }
    }
    if chats.is_empty() {
        bail!(
            "No user messages found. Choose a Codex session, Claude Code session, or Claude export."
        );
    }
    Ok(chats)
}

fn load_session(mut reader: impl BufRead) -> Result<Conversation> {
    let mut event_messages = Vec::new();
    let mut codex_messages = Vec::new();
    let mut claude_messages = Vec::new();
    let mut title = None;
    let mut imported_from = None;
    let mut raw = Vec::new();
    let mut index = 0;
    loop {
        raw.clear();
        index += 1;
        let bytes = reader
            .by_ref()
            .take(MAX_BYTES + 1)
            .read_until(b'\n', &mut raw)?;
        if bytes == 0 {
            break;
        }
        if bytes as u64 > MAX_BYTES {
            if !raw.ends_with(b"\n") {
                reader.skip_until(b'\n')?;
            }
            continue;
        }
        let line = std::str::from_utf8(&raw).context("Expected a UTF-8 session transcript")?;
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        // Read the record kind without building Values for command output or images.
        #[derive(serde::Deserialize)]
        struct Header<'a> {
            #[serde(rename = "type", borrow)]
            kind: Option<&'a str>,
        }
        let kind = match serde_json::from_str::<Header>(line) {
            Ok(header) => header.kind,
            Err(_) if !raw.ends_with(b"\n") && index > 1 => break,
            Err(error) => {
                return Err(error).with_context(|| format!("Invalid session JSON at line {index}"));
            }
        };
        if !matches!(
            kind,
            Some(
                "session_meta"
                    | "event_msg"
                    | "response_item"
                    | "user"
                    | "assistant"
                    | "custom-title"
            )
        ) {
            continue;
        }
        // Codex stores large tool blobs as response items. Only message items
        // can contribute to imported conversation history.
        if matches!(kind, Some("response_item" | "event_msg")) {
            #[derive(serde::Deserialize)]
            struct Item<'a> {
                #[serde(rename = "type", borrow)]
                kind: Option<&'a str>,
            }
            #[derive(serde::Deserialize)]
            struct Response<'a> {
                #[serde(borrow)]
                payload: Item<'a>,
            }
            let payload = serde_json::from_str::<Response>(line)?.payload.kind;
            if !matches!((kind, payload),
                (Some("response_item"), Some("message"))
                | (Some("event_msg"), Some("user_message" | "agent_message"))) {
                continue;
            }
        }
        let row: Value = serde_json::from_str(line)?;
        let payload = &row["payload"];
        let entry = match row["type"].as_str() {
            Some("session_meta") => {
                ensure!(!guardian_session(payload), "This file is an internal Guardian Review. Choose the original Codex conversation.");
                imported_from = payload["id"].as_str().map(|id| format!("codex:{id}"));
                continue;
            }
            Some("event_msg") => {
                let role = match payload["type"].as_str() {
                    Some("user_message") => "user",
                    Some("agent_message") => "assistant",
                    _ => continue,
                };
                (&mut event_messages, role, &payload["message"])
            }
            Some("response_item") if payload["type"] == "message" => (
                &mut codex_messages,
                payload["role"].as_str().unwrap_or_default(),
                &payload["content"],
            ),
            Some("user" | "assistant") if row["isSidechain"] != true && row["isMeta"] != true => {
                if imported_from.is_none() {
                    imported_from = row["sessionId"]
                        .as_str()
                        .map(|id| format!("claude-code:{id}"));
                }
                let msg = &row["message"];
                (
                    &mut claude_messages,
                    msg["role"].as_str().unwrap_or_default(),
                    &msg["content"],
                )
            }
            Some("custom-title") => {
                title = row["customTitle"].as_str().map(str::to_owned);
                continue;
            }
            _ => continue,
        };
        if let Some(msg) = message(entry.1, conversation_text(entry.2)) {
            entry.0.push(msg);
        }
    }
    let messages = if !claude_messages.is_empty() {
        claude_messages
    } else if event_messages.iter().any(|m| m.role == "user")
        && (event_messages.iter().any(|m| m.role == "assistant")
            || !codex_messages.iter().any(|m| m.role == "assistant"))
    {
        event_messages
    } else {
        codex_messages
    };
    let mut chat = conversation(title.as_deref(), messages)
        .context("No user messages found in this session")?;
    chat.thread.imported_from = imported_from;
    Ok(chat)
}

fn conversation_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| {
            matches!(
                part["type"].as_str(),
                Some("text" | "input_text" | "output_text")
            )
        })
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn message(role: &str, text: String) -> Option<Message> {
    let body = text.trim();
    if !matches!(role, "user" | "assistant") || body.is_empty() {
        return None;
    }
    if role == "user"
        && [
            "# AGENTS.md instructions",
            "<environment_context>",
            "<user_action>",
            "<local-command-stdout>",
            "<local-command-caveat>",
            "<command-name>",
        ]
        .iter()
        .any(|marker| body.starts_with(marker))
    {
        return None;
    }
    Some(Message::new(role, text))
}

fn conversation(title: Option<&str>, history: Vec<Message>) -> Option<Conversation> {
    history.iter().find(|m| m.role == "user")?;
    let title = conversation_title(title, title_from_history(&history));
    let mut chat = Conversation {
        history,
        ..Default::default()
    };
    chat.thread.title = title;
    chat.thread.started = true;
    Some(chat)
}

fn generic_title(title: &str) -> bool {
    matches!(title.trim().to_ascii_lowercase().as_str(),
        "" | "guardian review" | "untitled" | "untitled conversation" | "new chat" | "imported conversation")
}

fn prompt_title(prompt: &str) -> Option<&str> {
    let lines = if let Some((_, request)) = prompt.split_once("## My request") {
        request.trim_start_matches(" for Codex").trim_start_matches(':').trim()
    } else { prompt };
    lines.lines().map(str::trim).find(|line| !generic_title(line)
        && !matches!(line.trim_end_matches(['.', '!', '?']).to_ascii_lowercase().as_str(),
            "continue" | "resume" | "retry" | "try again")
        && !line.starts_with("<") && !line.starts_with("# Files mentioned"))
}

fn title_from_history(history: &[Message]) -> Option<&str> {
    history.iter().filter(|m| m.role == "user").find_map(|m| prompt_title(&m.content))
}

fn conversation_title(title: Option<&str>, prompt: Option<&str>) -> String {
    title.filter(|s| !generic_title(s)).or(prompt).unwrap_or("Untitled conversation")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(96)
        .collect()
}

impl crate::engine::Engine {
    pub(crate) fn improve_imported_titles(&mut self) -> String {
        let mut count = 0;
        for (thread, history) in std::iter::once((&mut self.state.thread, self.state.history.as_slice()))
            .chain(self.state.chats.values_mut().map(|chat| (&mut chat.thread, chat.history.as_slice())))
        {
            if thread.imported_from.is_some() && generic_title(&thread.title) && let Some(prompt) = title_from_history(history) {
                thread.title = conversation_title(None, Some(prompt));
                count += 1;
            }
        }
        format!("Updated {count} imported conversation title{}", if count == 1 { "" } else { "s" })
    }

    pub(crate) fn complete_import(&mut self, status: &str, imported: bool) {
        if let Some(pending) = self.jobs.import_request.take() {
            crate::engine::record_outcome(&mut self.state, pending.id, status, imported);
            crate::engine::set_tool_note(&mut self.state, pending.chat, "Import conversations".into(),
                if imported { status } else { "Denied by you" });
            crate::engine::continue_tool(&mut self.state, &pending, format!("Tool result {} · {status}", pending.call.id), false);
        }
    }

    pub(crate) fn import_conversations(
        &mut self,
        paths: Vec<Selection>,
        discovered: bool,
        workspace: Option<&Path>,
    ) -> Result<String> {
        ensure!(!paths.is_empty(), "Select at least one conversation");
        let mut known: HashMap<String, u64> = std::iter::once((self.state.chat_id, &self.state.thread))
            .chain(self.state.chats.iter().map(|(&id, chat)| (id, &chat.thread)))
            .filter_map(|(id, thread)| Some((thread.imported_from.clone()?, id)))
            .collect();
        let workspace = workspace.map(Path::canonicalize).transpose()?;
        let mut project = None;
        let mut count = 0;
        let mut attached = 0;
        let mut skipped = 0;
        let mut failed = 0;
        for source in paths {
            if let Some(workspace) = &workspace {
                let kind = if source.origin.starts_with("codex:") { Source::Codex } else { Source::ClaudeCode };
                let reported = kind.header(&source.path).and_then(|header| header.workspace)
                    .and_then(|path| path.canonicalize().ok());
                if reported.as_ref() != Some(workspace) {
                    failed += 1;
                    continue;
                }
            }
            let mut chats = match load(&source.path) {
                Ok(chats) => chats,
                Err(_) if discovered => {
                    failed += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if discovered {
                if chats.len() != 1
                    || chats[0].thread.imported_from.as_deref() != Some(&source.origin)
                {
                    failed += 1;
                    continue;
                }
                if source.origin.starts_with("codex:") && !generic_title(&source.title) {
                    chats[0].thread.title = conversation_title(Some(&source.title), None);
                }
                chats[0].thread.archived = source.archived;
            }
            if project.is_none() && let Some(folder) = &workspace {
                let project_info = match crate::projects::existing(folder)? {
                    Some(p) => p,
                    None => {
                        let name: String = folder.file_name().context("Choose a project folder")?
                            .to_string_lossy().chars().filter(|ch| !ch.is_control()).take(100).collect();
                        crate::projects::create(folder, &name)?
                    }
                };
                crate::projects::library(Some(&project_info))?;
                project = Some(project_info);
            }
            for mut chat in chats {
                if let Some(existing) = chat.thread.imported_from.as_ref().and_then(|id| known.get(id)).copied()
                {
                    let busy = self.state.queue.iter().any(|t| t.origin.unwrap_or(self.state.chat_id) == existing)
                        || self.state.requests.iter().any(|p| p.chat == existing);
                    let thread = if existing == self.state.chat_id { &mut self.state.thread }
                        else { &mut self.state.chats.get_mut(&existing).context("Missing previously imported chat")?.thread };
                    // Leave any existing workspace or generated files with the conversation.
                    if let Some(project) = &project && !busy && thread.project.is_none() && thread.turns.is_empty() {
                        thread.project = Some(project.path.clone());
                        attached += 1;
                    } else {
                        skipped += 1;
                    }
                    continue;
                }
                chat.thread.model.clone_from(&self.state.model);
                chat.thread.project = project.as_ref().map(|p| p.path.clone());
                self.state.serial += 1;
                if let Some(origin) = &chat.thread.imported_from { known.insert(origin.clone(), self.state.serial); }
                self.state.chats.insert(self.state.serial, chat);
                count += 1;
            }
        }
        let mut status = format!(
            "Imported {count} conversation{}",
            if count == 1 { "" } else { "s" }
        );
        if let Some(project) = project { status.push_str(&format!(" into {}", project.name)); }
        if attached != 0 { status.push_str(&format!(" · {attached} existing chats added to the project")); }
        if skipped != 0 {
            status.push_str(&format!(" · {skipped} already imported"));
        }
        if failed != 0 {
            status.push_str(&format!(" · {failed} unavailable or empty"));
        }
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn discovers_native_codex_and_claude_code_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("sessions/2026/11/26");
        let claude = dir.path().join("projects/example-project");
        fs::create_dir_all(claude.join("subagents")).unwrap();
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            dir.path().join("session_index.jsonl"),
            r#"{"id":"abc","thread_name":"The saved Codex name"}"#,
        )
        .unwrap();
        fs::write(codex.join("rollout-abc.jsonl"), concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"abc\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Prompt\"}}\n"
        )).unwrap();
        let transcript = r#"{"type":"user","sessionId":"def","message":{"role":"user","content":"Claude prompt"}}"#;
        fs::write(claude.join("def.jsonl"), transcript).unwrap();
        fs::write(claude.join("subagents/agent-copy.jsonl"), transcript).unwrap();
        let found = Source::Codex.discover_in(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "The saved Codex name");
        assert_eq!(
            load(&found[0].path).unwrap()[0]
                .thread
                .imported_from
                .as_deref(),
            Some("codex:abc")
        );
        let found = Source::ClaudeCode.discover_in(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Claude prompt");
        assert_eq!(
            load(&found[0].path).unwrap()[0]
                .thread
                .imported_from
                .as_deref(),
            Some("claude-code:def")
        );
    }

    #[test]
    fn discovery_excludes_guardian_sessions_and_recovers_descriptive_titles() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let sessions = dir.path().join("sessions");
        fs::create_dir(&sessions)?;
        let cwd = dir.path().canonicalize()?;
        let rows = [
            serde_json::json!({"type":"session_meta","payload":{"id":"original","cwd":cwd}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Update the checkout form validation"}]}}),
        ];
        fs::write(dir.path().join("session_index.jsonl"), r#"{"id":"original","thread_name":"Guardian Review"}"#)?;
        fs::write(sessions.join("original.jsonl"), rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n"))?;
        let review = sessions.join("review.jsonl");
        fs::write(&review, r#"{"type":"session_meta","payload":{"id":"review","thread_source":"guardian_review"}}
{"type":"event_msg","payload":{"type":"user_message","message":"Tool assessment"}}"#)?;
        let found = Source::Codex.discover_in(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Update the checkout form validation");
        assert_eq!(found[0].workspace.as_ref(), Some(&cwd));
        assert!(load(&review).err().unwrap().to_string().contains("Guardian Review"));
        Ok(())
    }

    #[test]
    fn imports_visible_codex_and_claude_code_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        for log in [
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"internal"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello"}]}}
{"type":"event_msg","payload":{"type":"user_message","message":"Hello"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"Hi there"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"Hello"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"internal"},{"type":"text","text":"Hi there"}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"internal"}]}}"#,
        ] {
            std::fs::write(&path, log).unwrap();
            let chats = load(&path).unwrap();
            assert_eq!(chats.len(), 1);
            assert_eq!(chats[0].thread.title, "Hello");
            assert_eq!(
                chats[0].history,
                [
                    Message::new("user", "Hello"),
                    Message::new("assistant", "Hi there"),
                ]
            );
        }
    }

    #[test]
    fn imports_claude_zip_and_ignores_unrelated_exports() {
        let dir = tempfile::tempdir().unwrap();
        // Android's system picker supplies a private temporary file.
        let path = dir.path().join("chat-import-123.tmp");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("data/conversations.json", options).unwrap();
        zip.write_all(br#"[{"name":"One","chat_messages":[{"sender":"human","text":"Hello"},{"sender":"assistant","content":[{"type":"text","text":"Hi there"}]}]},{"name":"Two","chat_messages":[{"sender":"human","text":"Another question"}]}]"#).unwrap();
        zip.start_file("account.json", options).unwrap();
        zip.write_all(b"unrelated").unwrap();
        zip.finish().unwrap();
        let chats = load(&path).unwrap();
        assert_eq!(chats.len(), 2);
        assert_eq!(chats[0].thread.title, "One");
        assert_eq!(chats[0].history[1].content, "Hi there");
        assert_eq!(chats[1].thread.title, "Two");
    }
}
