//! Retrieve relevant passages from other chats in the current project.

use crate::{
    ollama::Message,
    state::{State, ThreadInfo},
};

const EXCERPTS: usize = 3;
const EXCERPT_BYTES: usize = 550;

fn separator(c: char) -> bool {
    !c.is_alphanumeric() && c != '_'
}

fn term(word: &str) -> String {
    let word = word.to_lowercase();
    if word.len() > 4
        && !word.ends_with("ss")
        && let Some(singular) = word.strip_suffix('s')
    {
        return singular.to_owned();
    }
    word
}

fn query_terms(query: &str) -> Vec<String> {
    const COMMON: &str = "the and are but can could did does for from had has have how into its may our out some than that them then there these they thing this those use used want was were what when where which who will with would you your please get tell find help earlier previous last prior other another chat thread conversation project history agree agreed decide decided decision discuss discussed said say talk talked before remember recall picked chose choose mention mentioned continue left off";
    let mut terms = Vec::new();
    for word in query.split(separator).map(term) {
        if word.chars().count() < 3 || COMMON.split_whitespace().any(|s| s == word) {
            continue;
        }
        if !terms.contains(&word) {
            terms.push(word);
            if terms.len() == 24 {
                break;
            }
        }
    }
    terms
}

fn matches(text: &str, terms: &[String]) -> (u32, usize) {
    let mut mask = 0;
    let mut first_match = None;
    let mut offset = 0;
    if terms.is_empty() {
        return (0, 0);
    }
    for part in text.split_inclusive(separator) {
        let word = term(part.trim_end_matches(separator));
        if let Some(i) = terms.iter().position(|t| *t == word) {
            mask |= 1 << i;
            first_match.get_or_insert(offset);
        }
        offset += part.len();
    }
    (mask, first_match.unwrap_or(0))
}

fn excerpt(text: &str, center: usize) -> String {
    let mut start = center.saturating_sub(EXCERPT_BYTES / 4);
    let mut end = start.saturating_add(EXCERPT_BYTES).min(text.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let passage = text[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}{passage}{}",
        if start > 0 { "…" } else { "" },
        if end < text.len() { "…" } else { "" }
    )
}

/// Use transcripts already available on this device, including ones imported through sync.
/// The project binding is normalized by the engine and sync importer.
pub fn related(state: &State, chat: u64, query: &str) -> Option<String> {
    let project = state.thread_data(chat)?.0.project.as_ref()?;
    let terms = query_terms(query);
    let lower = query.to_lowercase();
    let explicit = [
        "other chat",
        "other thread",
        "previous conversation",
        "earlier conversation",
        "previous chat",
        "earlier chat",
        "previous thread",
        "earlier thread",
        "another chat",
        "another thread",
        "project history",
        "what did we decide",
        "what did we agree",
        "where we left off",
        "we discussed",
        "we agreed",
        "we decided",
        "last time",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase));
    if !explicit && terms.len() < 2 {
        return None;
    }
    let siblings = state
        .chats
        .iter()
        .map(|(&id, c)| (id, &c.thread, c.history.as_slice()))
        .chain(std::iter::once((
            state.chat_id,
            &state.thread,
            state.history.as_slice(),
        )));
    let mut candidates: Vec<(u32, u64, usize, &ThreadInfo, &[Message], usize)> = Vec::new();
    for (id, thread, history) in
        siblings.filter(|(id, t, _)| *id != chat && t.project.as_ref() == Some(project))
    {
        let (title_mask, _) = matches(&thread.title, &terms);
        for (index, message) in history.iter().enumerate() {
            if !matches!(message.role.as_str(), "assistant" | "user")
                || message.content.trim().is_empty()
                || thread.tool_results.contains(&index)
                || thread.turns.get(&index).is_some_and(|turn| turn.failed)
            {
                continue;
            }
            let (body_mask, offset) = matches(&message.content, &terms);
            let overlap = (body_mask | title_mask).count_ones();
            let relevant = if explicit {
                overlap > 0 || terms.is_empty()
            } else {
                overlap >= 2 && body_mask != 0
            };
            if relevant {
                let score = overlap * 4 + title_mask.count_ones() * 2 + body_mask.count_ones();
                candidates.push((score, id, index, thread, history, offset));
            }
        }
    }
    candidates
        .sort_unstable_by_key(|&(score, id, index, ..)| std::cmp::Reverse((score, id, index)));
    let excerpts = candidates
        .into_iter()
        .take(EXCERPTS)
        .map(|(_, id, index, thread, history, offset)| {
            let message = &history[index];
            let title = excerpt(thread.label(history), 0)
                .chars()
                .take(100)
                .collect::<String>();
            let speaker = if message.role == "user" {
                "User"
            } else {
                "Assistant"
            };
            format!(
                "{} (chat #{id}, message {}, {speaker}): {}",
                title,
                index + 1,
                excerpt(&message.content, offset)
            )
        })
        .collect::<Vec<_>>();
    (!excerpts.is_empty()).then(|| excerpts.join("\n\n"))
}
