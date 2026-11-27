use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
};

use crate::{ollama::Message, state::Conversation};

const MAX_BYTES: u64 = 64 * 1024 * 1024;

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
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    }
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Conversation file exceeds 64 MB"
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
            chats.extend(conversation(chat["name"].as_str(), messages));
        }
    } else {
        let mut event_messages = Vec::new();
        let mut codex_messages = Vec::new();
        let mut claude_messages = Vec::new();
        let mut title = None;
        for (index, line) in text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
        {
            let row: Value = serde_json::from_str(line)
                .with_context(|| format!("Invalid session JSON at line {}", index + 1))?;
            let payload = &row["payload"];
            let entry = match row["type"].as_str() {
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
                Some("user" | "assistant") if row["isSidechain"] != true => {
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
        chats.extend(conversation(title.as_deref(), messages));
    }
    if chats.is_empty() {
        bail!(
            "No user messages found. Choose a Codex session, Claude Code session, or Claude export."
        );
    }
    Ok(chats)
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
        ]
        .iter()
        .any(|marker| body.starts_with(marker))
    {
        return None;
    }
    Some(Message::new(role, text))
}

fn conversation(title: Option<&str>, history: Vec<Message>) -> Option<Conversation> {
    let prompt = history.iter().find(|m| m.role == "user")?;
    let title: String = title
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            prompt
                .content
                .trim()
                .lines()
                .next()
                .unwrap_or("Imported conversation")
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(96)
        .collect();
    let mut chat = Conversation {
        history,
        ..Default::default()
    };
    chat.thread.title = title;
    chat.thread.started = true;
    Some(chat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
