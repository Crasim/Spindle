use crate::{
    generation,
    ollama::Message,
    state::{State, ThreadInfo},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const UNKNOWN_WINDOW: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub percent: u8,
    pub token_cap: Option<u32>,
    /// Compact generously while usage is low and harder as the model nears a usage limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dynamic: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            percent: 50,
            token_cap: None,
            dynamic: false,
        }
    }
}

impl Policy {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.percent),
            "Choose a compaction percentage from 1 to 100"
        );
        ensure!(
            self.token_cap.is_none_or(|n| n > 0),
            "Choose a positive token ceiling"
        );
        Ok(())
    }

    pub fn threshold(self, window: u64) -> u64 {
        window
            .saturating_mul(u64::from(self.percent))
            .div_ceil(100)
            .min(self.token_cap.map(u64::from).unwrap_or(u64::MAX))
            .max(1)
    }

    /// The policy in force given how much of the model's usage allowance is spent
    /// (0.0 to 1.0, `None` when no limit is configured).
    pub fn effective(self, pressure: Option<f64>) -> Self {
        if !self.dynamic {
            return self;
        }
        // 75% (generous) with plenty of allowance, down to 20% at the limit.
        let percent = match pressure {
            Some(p) => (75.0 - 55.0 * p.clamp(0.0, 1.0)).round() as u8,
            None => 50,
        };
        Self { percent, ..self }
    }

    pub fn label(self) -> String {
        if self.dynamic {
            return "Dynamic · tightens as usage limits near".into();
        }
        let preset = match self.percent {
            25 => "Aggressive",
            50 => "Normal",
            75 => "Generous",
            _ => "Custom",
        };
        let mut label = format!("{preset} · {}%", self.percent);
        if let Some(cap) = self.token_cap {
            label.push_str(&format!(" · {cap} token ceiling"));
        }
        label
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub covered: usize,
    pub text: String,
}

impl Summary {
    fn messages(&self) -> [Message; 2] {
        [
            Message::new("user", "Earlier conversation excerpts for reference:"),
            Message::new("assistant", &self.text),
        ]
    }
}

fn clipped(s: &str, bytes: usize) -> String {
    let s = s.trim();
    if s.len() <= bytes {
        return s.into();
    }
    let mut end = bytes.saturating_sub('…'.len_utf8());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", s[..end].trim_end())
}

// Extract goals, decisions, results and artifact names locally, retaining their speaker
// and turn number. Build from the transcript so repeated summarization never compounds errors.
fn digest(thread: &ThreadInfo, prefix: &[Message], budget: usize) -> String {
    let mut candidates = Vec::new();
    for (index, m) in prefix.iter().enumerate() {
        if thread.tool_results.contains(&index) {
            continue;
        }
        let mut in_code = false;
        let mut first = true;
        for line in m.content.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line.starts_with("```") || line.starts_with("~~~") {
                in_code = !in_code;
                continue;
            }
            if in_code || line.starts_with("<!-- spindle-title:") {
                continue;
            }
            let lower = line.to_lowercase();
            let decision = [
                "must ",
                "use ",
                "prefer",
                "require",
                "decid",
                "constraint",
                "agreed",
                "remaining",
                "failed",
                "passed",
                "todo",
                "next step",
                "do not",
            ]
            .iter()
            .any(|s| lower.contains(s));
            if first || decision {
                let score = u8::from(decision) * 3
                    + u8::from(m.role == "user")
                    + u8::from(index == 0 && first) * 3;
                candidates.push((score, index, candidates.len(), clipped(line, 220)));
            }
            first = false;
        }
        if let Some(turn) = thread.turns.get(&index) {
            if !turn.files.is_empty() {
                candidates.push((
                    6,
                    index,
                    candidates.len(),
                    clipped(&format!("Saved {}", turn.files.join(", ")), 220),
                ));
            }
        }
    }
    candidates.sort_by_key(|(priority, index, order, _)| {
        (
            std::cmp::Reverse(*priority),
            std::cmp::Reverse(*index),
            *order,
        )
    });
    let mut used = 0;
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for (_, index, order, line) in candidates {
        let role = if prefix[index].role == "user" {
            "User"
        } else {
            "Assistant"
        };
        if !seen.insert((role, line.clone())) {
            continue;
        }
        let entry = format!("{} · {role}: {line}", index / 2 + 1);
        let room = budget.saturating_sub(used);
        if room < 32 {
            break;
        }
        if entry.len() > room && !selected.is_empty() {
            continue;
        }
        let entry = clipped(&entry, room);
        used += entry.len() + 1;
        selected.push((index, order, entry));
    }
    selected.sort_by_key(|(index, order, _)| (*index, *order));
    selected
        .into_iter()
        .map(|(_, _, s)| s)
        .collect::<Vec<_>>()
        .join("\n")
}

fn checkpoint(
    thread: &mut ThreadInfo,
    history: &[Message],
    fixed: u64,
    gate: u64,
    force: bool,
) -> bool {
    if thread
        .summary
        .as_ref()
        .is_some_and(|s| s.covered > history.len() || s.covered % 2 != 0)
    {
        thread.summary = None;
    }
    let covered = thread.summary.as_ref().map_or(0, |s| s.covered);
    let remaining = &history[covered..];
    let old_summary = thread
        .summary
        .as_ref()
        .map_or(0, |s| generation::message_tokens(&s.messages()));
    let before = old_summary + generation::message_tokens(remaining);
    if history.is_empty() || (!force && fixed + before < gate) {
        return false;
    }

    let target = gate.saturating_mul(3) / 5;
    let summary_tokens = (target / 4).clamp(32, 768);
    if remaining.len() < 2 && old_summary <= summary_tokens + 48 {
        return false;
    }
    let recent_budget = target.saturating_sub(fixed + summary_tokens);
    let mut end = history.len();
    let mut recent = 0;
    for pair in remaining.rchunks_exact(2) {
        let cost = generation::message_tokens(pair);
        if recent + cost > recent_budget || end <= covered + 2 || (force && recent > 0) {
            break;
        }
        recent += cost;
        end -= 2;
    }
    let replaced = before.saturating_sub(recent);
    let budget = (summary_tokens.min(replaced / 2) * 3) as usize;
    if budget < 32 {
        return false;
    }
    let mut text = digest(thread, &history[..end], budget);
    if text.is_empty() {
        text = format!(
            "Earlier code and text are available in exchanges 1–{} of the transcript.",
            end / 2
        );
    }
    let summary = Summary { covered: end, text };
    if generation::message_tokens(&summary.messages()) >= replaced {
        return false;
    }
    thread.summary = Some(summary);
    true
}

impl State {
    fn context_parts(&mut self, id: u64) -> Result<(&mut ThreadInfo, &[Message])> {
        if id == self.chat_id {
            Ok((&mut self.thread, &self.history))
        } else {
            let chat = self.chats.get_mut(&id).context("Thread no longer exists")?;
            Ok((&mut chat.thread, &chat.history))
        }
    }

    pub fn compact_now(&mut self, id: u64, window: u64) -> Result<String> {
        let defaults = self.default_compaction(id);
        let pressure = self.automation.pressure(self.thread_model(id));
        let (thread, history) = self.context_parts(id)?;
        let gate = thread
            .compaction
            .unwrap_or(defaults)
            .effective(pressure)
            .threshold(window);
        let changed = checkpoint(thread, history, 0, gate, true);
        if changed {
            let covered = thread.summary.as_ref().map_or(0, |s| s.covered / 2);
            thread.compactions.insert(history.len(), covered);
        }
        Ok(if changed {
            format!(
                "Compacted {} earlier exchanges for future prompts",
                thread.summary.as_ref().unwrap().covered / 2
            )
        } else {
            "No additional history to compact usefully yet".into()
        })
    }

    pub fn add_chat_history(
        &mut self,
        id: u64,
        window: u64,
        messages: &mut Vec<Message>,
        tool_chain: bool,
    ) -> Result<()> {
        let defaults = self.default_compaction(id);
        let pressure = self.automation.pressure(self.thread_model(id));
        let (thread, history) = self.context_parts(id)?;
        let gate = thread
            .compaction
            .unwrap_or(defaults)
            .effective(pressure)
            .threshold(window);
        let fixed = generation::message_tokens(messages) + 64;
        // Reserve a little output space even when the chosen trigger is 100%.
        let input_limit = window.saturating_sub((window / 10).min(1024));
        if !tool_chain && checkpoint(thread, history, fixed, gate.min(input_limit), false) {
            let covered = thread.summary.as_ref().map_or(0, |s| s.covered / 2);
            thread.compactions.insert(history.len(), covered);
        }
        let mut recalled = Vec::new();
        let mut room = input_limit.saturating_sub(fixed);
        // A tool result is a continuation of the last real request. Reserve its
        // goal before the summary and the working history compete for space.
        let anchor = if tool_chain && room >= 48 {
            history
                .iter()
                .enumerate()
                .rev()
                .find(|(index, m)| m.role == "user" && !thread.tool_results.contains(index))
                .map(|(index, m)| {
                    let max_bytes =
                        usize::try_from((room - 12).saturating_mul(3)).unwrap_or(usize::MAX);
                    let original = generation::message_tokens(std::slice::from_ref(m));
                    let mut m = m.clone();
                    if m.content.len() > max_bytes {
                        m.content = clipped(&m.content, max_bytes);
                    }
                    room -= generation::message_tokens(std::slice::from_ref(&m));
                    (index, m, original)
                })
        } else {
            None
        };
        let mut start = 0;
        if let Some(summary) = &thread.summary
            && summary.covered <= history.len()
        {
            start = summary.covered;
            let excerpt = summary.messages();
            let cost = generation::message_tokens(&excerpt);
            if cost <= room {
                room -= cost;
                recalled.extend(excerpt);
            }
        }
        let mut first_recent = history.len();
        for pair in history[start..].rchunks_exact(2) {
            let at = first_recent - pair.len();
            let mut cost = generation::message_tokens(pair);
            if let Some((index, _, original)) = &anchor
                && (at..first_recent).contains(index)
            {
                cost -= original;
            }
            for index in at..first_recent {
                if let Some(call) = thread.turns.get(&index).and_then(|turn| turn.call.as_ref()) {
                    cost += (call.len() + 2).div_ceil(3) as u64;
                }
            }
            if cost > room {
                break;
            }
            room -= cost;
            first_recent -= 2;
        }
        if let Some((index, request, _)) = &anchor
            && *index < first_recent
        {
            recalled.push(request.clone());
        }
        // A reply that called a tool is sent back with its call: without it the model sees
        // itself announcing steps and stopping, and small models copy that.
        for (i, m) in history.iter().enumerate().skip(first_recent) {
            if let Some((index, request, _)) = &anchor
                && i == *index
            {
                recalled.push(request.clone());
                continue;
            }
            match thread.turns.get(&i).and_then(|t| t.call.as_deref()) {
                Some(call) if m.role == "assistant" => {
                    let mut m = m.clone();
                    m.content = format!("{}\n\n{call}", m.content.trim_end());
                    recalled.push(m);
                }
                _ => recalled.push(m.clone()),
            }
        }
        messages.splice(1..1, recalled);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> Vec<Message> {
        vec![
            Message::new("system", "Answer the user"),
            Message::new("user", "What did we decide?"),
        ]
    }

    #[test]
    fn crossing_the_exact_threshold_summarizes_once_and_preserves_the_transcript() {
        let mut state = State::default();
        for n in 0..8 {
            let goal = if n == 0 {
                "We must use ORCHARD for the project name."
            } else {
                "Add a progress note."
            };
            state.history.push(Message::new("user", goal));
            state.history.push(Message::new(
                "assistant",
                format!("Round {n}\n{}", "Routine telemetry. ".repeat(55)),
            ));
        }
        let size =
            generation::message_tokens(&state.history) + generation::message_tokens(&prompt()) + 64;
        let original = serde_json::to_vec(&state.history).unwrap();
        let mut before = prompt();
        state
            .add_chat_history(0, size * 2 + 2, &mut before, false)
            .unwrap();
        assert!(state.thread.summary.is_none());
        assert_eq!(before.len(), 18);
        let mut after = prompt();
        state
            .add_chat_history(0, size * 2, &mut after, false)
            .unwrap();
        let summary = state.thread.summary.as_ref().unwrap();
        assert!(summary.text.contains("ORCHARD"));
        assert!(summary.covered < state.history.len());
        assert!(generation::message_tokens(&after) < generation::message_tokens(&before));
        assert_eq!(
            after[after.len() - 2].content,
            state.history.last().unwrap().content
        );
        assert_eq!(serde_json::to_vec(&state.history).unwrap(), original);
        let mut again = prompt();
        state
            .add_chat_history(0, size * 2, &mut again, false)
            .unwrap();
        assert_eq!(
            serde_json::to_vec(&again).unwrap(),
            serde_json::to_vec(&after).unwrap()
        );
    }

    #[test]
    fn reading_through_many_pages_keeps_the_original_request_until_the_tool_run_finishes() {
        let mut state = State::default();
        state.preferences.compaction.percent = 25;
        let request = "Find the one line containing LANTERN in the file and report its value";
        state.history.push(Message::new("user", request));
        state
            .history
            .push(Message::new("assistant", "I'll read the first page."));
        for n in 1..=14 {
            let row = state.history.len();
            state.thread.tool_results.insert(row);
            state.history.push(Message::new(
                "user",
                format!(
                    "Tool result from page {n}\n{}",
                    "unrelated file text ".repeat(148)
                ),
            ));
            state.history.push(Message::new(
                "assistant",
                "I'll follow the next read cursor.",
            ));
            let page = format!("Next page: {}\n{}", n + 1, "more evidence ".repeat(180));
            let mut msgs = vec![
                Message::new(
                    "system",
                    "Follow the user's request and use read with the supplied cursor.",
                ),
                Message::new("user", page.clone()),
            ];
            state.add_chat_history(0, 4096, &mut msgs, true).unwrap();
            assert_eq!(msgs.iter().filter(|m| m.content == request).count(), 1);
            assert_eq!(msgs.last().unwrap().content, page);
            assert!(generation::message_tokens(&msgs) < 4096);
            assert!(state.thread.summary.is_none() && state.thread.compactions.is_empty());
        }
        // A queued read from an older build may already have been compacted.
        state.compact_now(0, 4096).unwrap();
        let before = state.thread.compactions.clone();
        let mut resumed = prompt();
        state.add_chat_history(0, 4096, &mut resumed, true).unwrap();
        assert!(resumed.iter().any(|m| m.content == request));
        assert_eq!(state.thread.compactions, before);
        // At the next user message the normal trigger is enabled again.
        state.thread.summary = None;
        state
            .add_chat_history(0, 4096, &mut prompt(), false)
            .unwrap();
        assert!(state.thread.summary.is_some());
    }

    #[test]
    fn presets_custom_ceiling_and_thread_overrides_round_trip() {
        for (percent, trigger) in [(25, 1024), (50, 2048), (75, 3072), (37, 1516)] {
            let p = Policy {
                percent,
                token_cap: None,
                dynamic: false,
            };
            assert_eq!(p.threshold(4096), trigger);
        }
        let mut s = State::default();
        s.thread.compaction = Some(Policy {
            percent: 25,
            token_cap: Some(512),
            dynamic: false,
        });
        let restored: State = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(restored.thread.compaction.unwrap().threshold(65536), 512);
        assert_eq!(restored.preferences.compaction.percent, 50);
        // Dynamic: generous with no spend, tight near the limit, normal with no budget set.
        let dynamic = Policy {
            dynamic: true,
            ..Policy::default()
        };
        assert_eq!(dynamic.effective(Some(0.0)).percent, 75);
        assert_eq!(dynamic.effective(Some(1.0)).percent, 20);
        assert_eq!(dynamic.effective(None).percent, 50);
        let mut s = State {
            model: "@ollama-cloud/big".into(),
            ..State::default()
        };
        s.preferences.compaction = dynamic;
        s.automation.rules.push(crate::automation::Rule {
            scope: "ollama-cloud".into(),
            period: 3600,
            cap: 100,
            percent: 90,
            used: 80,
            reset: 0,
            pause: false,
        });
        let id = s.chat_id;
        assert_eq!(
            s.default_compaction(id)
                .effective(s.automation.pressure(s.thread_model(id)))
                .percent,
            31
        );
    }
}
