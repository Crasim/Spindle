//! Memory: durable facts carried between conversations.
//!
//! There is one personal memory and one per project, stored with the encrypted session. The
//! model saves facts by ending a reply with a fenced `spindle-memory` block, one fact per
//! line, and each save can be undone. Relevant memories are added to every chat's system
//! prompt.

use crate::state::State;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: u64,
    pub text: String,
    /// The project this belongs to; `None` is personal memory used in every chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    pub saved: i64,
}

pub fn enabled() -> bool {
    true
}

const MAX_FACT: usize = 300;
const MAX_PROMPT: usize = 2_500;

/// How the model saves a memory.
pub const INSTRUCTION: &str = " When the user shares a durable fact, preference or decision that would help in future conversations, end your reply with a fenced block labeled spindle-memory containing one short fact per line. Prefix a line with personal: when it is about the user rather than this project. Save rarely, never repeat what is already remembered, and never save passwords, keys, account numbers or sensitive personal details.";

/// Which memories a chat may use and save: (personal, its project's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub personal: bool,
    pub project: bool,
}

impl Scope {
    pub fn any(self) -> bool {
        self.personal || self.project
    }
}

/// The memory a chat in `project` (or outside any project) may use, from the settings.
pub fn scope(state: &State, project: Option<&Path>) -> Scope {
    let prefs = &state.preferences;
    Scope {
        personal: prefs.memory && prefs.personal_memory,
        project: prefs.memory
            && prefs.project_memory
            && project.is_some_and(|p| !state.memory_off.contains(p)),
    }
}

/// Where a fact the model saved goes, if anywhere.
pub fn destination(
    scope: Scope,
    personal: bool,
    project: Option<&Path>,
) -> Option<Option<PathBuf>> {
    match (personal, project) {
        (false, Some(p)) if scope.project => Some(Some(p.to_owned())),
        (false, Some(_)) => None,
        _ if scope.personal => Some(None),
        _ => None,
    }
}

/// How the model saves a memory, for the scopes that are on.
pub fn instruction(scope: Scope) -> String {
    let mut text = INSTRUCTION.to_owned();
    match (scope.personal, scope.project) {
        (true, false) => text.push_str(" Only personal memory is on, so save only facts about the user, each prefixed personal:."),
        (false, true) => text.push_str(" Only project memory is on, so save only facts about this project and never about the user personally."),
        _ => {}
    }
    text
}

/// Memories for a chat: personal ones plus the chat's project, newest first, within budget.
pub fn prompt(memories: &[Memory], project: Option<&Path>, scope: Scope) -> Option<String> {
    let mut lines = vec![];
    let mut used = 0;
    for m in memories.iter().rev().filter(|m| match &m.project {
        None => scope.personal,
        Some(p) => scope.project && Some(p.as_path()) == project,
    }) {
        used += m.text.len() + 3;
        if used > MAX_PROMPT {
            break;
        }
        lines.push(format!("- {}", m.text));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "\n\nRemembered from earlier conversations (may be out of date; the user can edit these):\n{}",
            lines.join("\n")
        )
    })
}

fn sensitive(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "password",
        "passcode",
        "api key",
        "secret",
        "sk-",
        "ssn",
        "social security",
        "credit card",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// Remove `spindle-memory` blocks from a reply. Returns the cleaned reply and the facts,
/// each marked personal or not.
pub fn extract(answer: &str) -> (String, Vec<(bool, String)>) {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut cut = vec![];
    let mut facts = vec![];
    let mut events = Parser::new(answer).into_offset_iter();
    while let Some((event, range)) = events.next() {
        let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event else {
            continue;
        };
        if info.trim() != "spindle-memory" {
            continue;
        }
        let mut end = range.end;
        let mut body = String::new();
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
        cut.push(range.start..end);
        for line in body.lines() {
            let line = line.trim().trim_start_matches(['-', '*', '•']).trim();
            let (personal, fact) = match line.strip_prefix("personal:") {
                Some(rest) => (true, rest.trim()),
                None => (false, line),
            };
            if !fact.is_empty() && fact.len() <= MAX_FACT && !sensitive(fact) && facts.len() < 5 {
                facts.push((personal, fact.to_owned()));
            }
        }
    }
    let mut cleaned = answer.to_owned();
    for range in cut.into_iter().rev() {
        cleaned.replace_range(range, "");
    }
    (cleaned.trim_end().to_owned(), facts)
}

/// Save a fact, or return the id of an identical one already saved in the same place.
pub fn save(state: &mut State, text: &str, project: Option<PathBuf>, now: i64) -> u64 {
    let text = text.trim();
    if let Some(m) = state
        .memories
        .iter()
        .find(|m| m.project == project && m.text.eq_ignore_ascii_case(text))
    {
        return m.id;
    }
    let id = state.memories.iter().map(|m| m.id).max().unwrap_or(0) + 1;
    state.memories.push(Memory {
        id,
        text: text.chars().take(MAX_FACT).collect(),
        project,
        saved: now,
    });
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_removed_and_facts_scoped() {
        let reply = "Done.\n\n```spindle-memory\n- Prefers British spelling\npersonal: Works in transit planning\nMy password is hunter2\n```\n";
        let (clean, facts) = extract(reply);
        assert_eq!(clean, "Done.");
        assert_eq!(
            facts,
            vec![
                (false, "Prefers British spelling".to_owned()),
                (true, "Works in transit planning".to_owned())
            ]
        );
        let mut state = State::default();
        let plans = PathBuf::from("/p");
        let a = save(&mut state, "Uses Rust", Some(plans.clone()), 1);
        assert_eq!(save(&mut state, "uses rust", Some(plans.clone()), 2), a);
        save(&mut state, "Lives in Boise", None, 3);
        save(&mut state, "Other project fact", Some("/q".into()), 4);
        let all = scope(&state, Some(&plans));
        let text = prompt(&state.memories, Some(&plans), all).unwrap();
        assert!(text.contains("Uses Rust") && text.contains("Lives in Boise"));
        assert!(!text.contains("Other project fact"));
        assert!(prompt(&[], None, all).is_none());
        // Turning a project off keeps personal memory; turning personal off keeps the project's.
        state.memory_off.insert(plans.clone());
        let off = scope(&state, Some(&plans));
        assert_eq!(
            off,
            Scope {
                personal: true,
                project: false
            }
        );
        let text = prompt(&state.memories, Some(&plans), off).unwrap();
        assert!(!text.contains("Uses Rust") && text.contains("Lives in Boise"));
        assert_eq!(destination(off, false, Some(&plans)), None);
        assert_eq!(destination(off, true, Some(&plans)), Some(None));
        state.memory_off.clear();
        state.preferences.personal_memory = false;
        let projects_only = scope(&state, Some(&plans));
        let text = prompt(&state.memories, Some(&plans), projects_only).unwrap();
        assert!(text.contains("Uses Rust") && !text.contains("Lives in Boise"));
        assert_eq!(destination(projects_only, true, Some(&plans)), None);
        assert!(!scope(&state, None).any());
    }
}
