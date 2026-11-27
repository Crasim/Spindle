use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reason {
    InvalidCall,
    ToolLimit,
    Http(u16),
    VerificationPage,
    Timeout,
    Disconnected,
    Unavailable,
    BlockedBySettings,
    EmptyReply,
    Other,
}

impl Reason {
    pub fn label(self) -> String {
        match self {
            Self::Http(status) => format!("HTTP {status}"),
            reason => match reason {
                Self::InvalidCall => "Malformed tool call",
                Self::ToolLimit => "Tool step limit reached",
                Self::VerificationPage => "Site verification page",
                Self::Timeout => "Timed out",
                Self::Disconnected => "Connection failed",
                Self::Unavailable => "Tool or provider unavailable",
                Self::BlockedBySettings => "Blocked by a setting",
                Self::EmptyReply => "Empty model response",
                Self::Other => "Operation failed",
                Self::Http(_) => unreachable!(),
            }
            .into(),
        }
    }

    pub fn classify(error: &str) -> Self {
        let error = error.to_ascii_lowercase();
        for marker in ["http ", "status code ", "status "] {
            if let Some((_, number)) = error.split_once(marker)
                && let Some(code) = number.get(..3).and_then(|number| number.parse().ok())
                && (400..600).contains(&code)
            {
                return Self::Http(code);
            }
        }
        if ["timed out", "timeout", "deadline"]
            .iter()
            .any(|s| error.contains(s))
        {
            Self::Timeout
        } else if [
            "connection",
            "disconnect",
            "socket",
            "error sending request",
        ]
        .iter()
        .any(|s| error.contains(s))
        {
            Self::Disconnected
        } else if error.contains("empty answer") || error.contains("empty response") {
            Self::EmptyReply
        } else if error.contains("disabled") || error.contains("blocked by") {
            Self::BlockedBySettings
        } else if [
            "unavailable",
            "not available",
            "not installed",
            "could not start",
            "couldn't start",
            "could not find",
            "no supported browser",
        ]
        .iter()
        .any(|s| error.contains(s))
        {
            Self::Unavailable
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub time: i64,
    pub chat: u64,
    pub model: String,
    pub operation: String,
    pub reason: Reason,
    pub browser_link_offered: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncDecision {
    BranchesChanged {
        before: usize,
        after: usize,
        added: usize,
    },
    PreservedSeparateChat {
        active_local_work: bool,
    },
    SameTitle {
        visible: usize,
        related: usize,
        added: bool,
    },
    BranchMerged {
        messages: usize,
    },
}

impl SyncDecision {
    pub fn label(&self) -> String {
        match self {
            Self::BranchesChanged { before, after, added } => format!("Sync updated branches inside one chat ({before} → {after}, {added} new)"),
            Self::PreservedSeparateChat { active_local_work: true } => "Legacy sync preserved a separate chat while local work was active".into(),
            Self::PreservedSeparateChat { active_local_work: false } => "Legacy sync created a separate chat because its parent was no longer the local head".into(),
            Self::SameTitle { visible, related, added } => {
                let action = if *added { "Sync added a separate chat" } else { "Sync updated a chat" };
                format!("{action}. The title now matches {visible} visible chats ({related} other chats share history or import identity)")
            }
            Self::BranchMerged { messages } => format!("Merged a sync branch into the chat ({messages} messages)"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncEvent {
    pub time: i64,
    pub chat: u64,
    pub decision: SyncDecision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Entry {
    Failure(Failure),
    Sync(SyncEvent),
    BrowserLink {
        time: i64,
        chat: u64,
        completed: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capture {
    pub start: i64,
    pub end: i64,
    pub version: String,
    entries: VecDeque<Entry>,
    pub omitted: usize,
}

impl Capture {
    pub fn start(now: i64, days: i64) -> anyhow::Result<Self> {
        anyhow::ensure!(matches!(days, 2 | 3), "Choose a two- or three-day capture");
        Ok(Self {
            start: now,
            end: now + days * 86_400,
            version: env!("CARGO_PKG_VERSION").into(),
            entries: VecDeque::new(),
            omitted: 0,
        })
    }

    pub fn active(&self, now: i64) -> bool {
        now >= self.start && now < self.end
    }
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn finish(&mut self, now: i64) {
        self.end = self.end.min(now.max(self.start));
    }

    fn push(&mut self, time: i64, entry: Entry) {
        const LIMIT: usize = 5000;
        if self.active(time) {
            if self.entries.len() >= LIMIT {
                self.entries.pop_front();
                self.omitted = self.omitted.saturating_add(1);
            }
            self.entries.push_back(entry);
        }
    }
}

#[derive(Deserialize)]
pub struct SavedDiagnostics {
    #[serde(default)]
    pub failures: Vec<Failure>,
    #[serde(default)]
    pub sync_diagnostics: Vec<SyncEvent>,
    #[serde(default)]
    pub diagnostic_capture: Option<Capture>,
}

pub fn log(
    state: &mut crate::state::State,
    chat: u64,
    model: &str,
    operation: &str,
    reason: Reason,
    browser_link_offered: bool,
) {
    const LIMIT: usize = 250;
    let event = Failure {
        time: crate::automation::now(),
        chat,
        model: model.into(),
        operation: operation.into(),
        reason,
        browser_link_offered,
    };
    if let Some(capture) = state
        .diagnostic_capture
        .as_mut()
        .filter(|capture| capture.active(event.time))
    {
        capture.push(event.time, Entry::Failure(event.clone()));
    }
    state.failures.push(event);
    let excess = state.failures.len().saturating_sub(LIMIT);
    state.failures.drain(..excess);
}

pub fn sync(state: &mut crate::state::State, chat: u64, decision: SyncDecision, peers: Vec<u64>) {
    const LIMIT: usize = 100;
    let event = SyncEvent {
        time: crate::automation::now(),
        chat,
        decision,
        peers,
    };
    if let Some(capture) = state
        .diagnostic_capture
        .as_mut()
        .filter(|capture| capture.active(event.time))
    {
        capture.push(event.time, Entry::Sync(event.clone()));
    }
    state.sync_diagnostics.push(event);
    let excess = state.sync_diagnostics.len().saturating_sub(LIMIT);
    state.sync_diagnostics.drain(..excess);
}

/// A new or renamed synced chat can sit beside unrelated chats with the same title.
/// Count these separately from clones which carry the same imported origin or message prefix.
pub fn synced_title(state: &mut crate::state::State, chat: u64, added: bool) {
    let Some((incoming, history)) = state.thread_data(chat) else {
        return;
    };
    let title = incoming.label(history).trim().to_lowercase();
    if incoming.archived || matches!(title.as_str(), "" | "new chat") {
        return;
    }
    let mut peers = vec![];
    let mut visible = 1;
    let mut related = 0;
    for id in std::iter::once(state.chat_id)
        .chain(state.chats.keys().copied())
        .filter(|id| *id != chat)
    {
        let Some((thread, other)) = state.thread_data(id) else {
            continue;
        };
        if thread.archived
            || thread.project != incoming.project
            || thread.label(other).trim().to_lowercase() != title
        {
            continue;
        }
        visible += 1;
        if (incoming.imported_from.is_some() && incoming.imported_from == thread.imported_from)
            || history.get(..2).is_some_and(|first| {
                other.get(..2) == Some(first)
                    && first
                        .iter()
                        .any(|message| !message.content.trim().is_empty())
            })
        {
            related += 1;
        }
        if peers.len() < 12 {
            peers.push(id);
        }
    }
    if visible > 1 {
        sync(
            state,
            chat,
            SyncDecision::SameTitle {
                visible,
                related,
                added,
            },
            peers,
        );
    }
}

pub fn browser_link(state: &mut crate::state::State, chat: u64, completed: bool) {
    if let Some(capture) = &mut state.diagnostic_capture {
        let time = crate::automation::now();
        capture.push(
            time,
            Entry::BrowserLink {
                time,
                chat,
                completed,
            },
        );
    }
}

pub fn report(failures: &[Failure], include_models: bool) -> String {
    let mut counts = BTreeMap::new();
    for event in failures {
        let model = if include_models {
            event.model.as_str()
        } else {
            ""
        };
        *counts
            .entry((
                &event.operation,
                event.reason,
                model,
                event.browser_link_offered,
            ))
            .or_insert(0) += 1;
    }
    let mut text = format!(
        "Spindle {} troubleshooting report\nPlatform: {} {}\nRecorded failures: {}\n\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        failures.len()
    );
    for ((operation, reason, model, fallback), count) in counts {
        let _ = write!(text, "{count} × {operation} · {}", reason.label());
        if !model.is_empty() {
            let name: String = model
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect();
            let _ = write!(text, " · {name}");
        }
        if fallback {
            text.push_str(" · Browser Link offered");
        }
        text.push('\n');
    }
    text
}

pub struct ReportOptions {
    pub now: i64,
    pub include_models: bool,
    pub chat_references: bool,
}

pub fn review_report(
    failures: &[Failure],
    sync_events: &[SyncEvent],
    capture: Option<&Capture>,
    options: ReportOptions,
) -> String {
    fn utc(time: i64) -> String {
        chrono::DateTime::from_timestamp(time, 0)
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Unknown time".into())
    }
    fn safe_model(name: &str) -> String {
        name.chars()
            .filter(|ch| !ch.is_control())
            .take(120)
            .collect()
    }
    let entries = if let Some(capture) = capture {
        capture.entries.iter().collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let window_failures;
    let mut text = if let Some(capture) = capture {
        window_failures = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Failure(event) => Some(event.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut text = report(&window_failures, options.include_models);
        let status = if capture.active(options.now) {
            "in progress"
        } else {
            "finished"
        };
        let _ = writeln!(
            text,
            "Capture {status} · {} to {} · started on Spindle {}",
            utc(capture.start),
            utc(capture.end),
            capture.version
        );
        if capture.omitted > 0 {
            let _ = writeln!(
                text,
                "{} oldest events omitted at the capture limit",
                capture.omitted
            );
        }
        text
    } else {
        report(failures, options.include_models)
    };
    text.push_str("\nTimeline\n");
    let mut lines = Vec::new();
    let mut add = |time, chat, peers: &[u64], description: String| {
        let mut line = format!("{} · {description}", utc(time));
        if options.chat_references {
            let _ = write!(line, " · chat #{chat}");
            for peer in peers {
                let _ = write!(line, " #{peer}");
            }
        }
        lines.push((time, line));
    };
    let add_failure = |event: &Failure| {
        let mut description = format!("{} · {}", event.operation, event.reason.label());
        if options.include_models && !event.model.is_empty() {
            let _ = write!(description, " · {}", safe_model(&event.model));
        }
        if event.browser_link_offered {
            description.push_str(" · Browser Link offered");
        }
        (event.time, event.chat, description)
    };
    if capture.is_some() {
        for entry in entries {
            match entry {
                Entry::Failure(event) => {
                    let (time, chat, text) = add_failure(event);
                    add(time, chat, &[], text);
                }
                Entry::Sync(event) => {
                    add(event.time, event.chat, &event.peers, event.decision.label())
                }
                Entry::BrowserLink {
                    time,
                    chat,
                    completed,
                } => add(
                    *time,
                    *chat,
                    &[],
                    if *completed {
                        "Browser Link completed a browser action"
                    } else {
                        "Browser Link selected as a fallback"
                    }
                    .into(),
                ),
            }
        }
    } else {
        for event in failures {
            let (time, chat, text) = add_failure(event);
            add(time, chat, &[], text);
        }
        for event in sync_events {
            add(event.time, event.chat, &event.peers, event.decision.label());
        }
        lines.sort_by_key(|(time, _)| *time);
    }
    if lines.is_empty() {
        text.push_str("No events recorded.\n");
    }
    for (_, line) in lines {
        let _ = writeln!(text, "{line}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_restores_its_window_and_filters_late_events() {
        let mut capture = Capture::start(20, 3).unwrap();
        let end = capture.end;
        let event = |time| {
            Entry::Sync(SyncEvent {
                time,
                chat: 871,
                decision: SyncDecision::BranchesChanged {
                    before: 0,
                    after: 1,
                    added: 1,
                },
                peers: vec![872],
            })
        };
        capture.push(end - 1, event(end - 1));
        let mut restored: Capture =
            serde_json::from_slice(&serde_json::to_vec(&capture).unwrap()).unwrap();
        restored.push(end, event(end));
        assert_eq!(restored.count(), 1);
        let text = review_report(
            &[],
            &[],
            Some(&restored),
            ReportOptions {
                now: end,
                include_models: false,
                chat_references: false,
            },
        );
        assert!(text.contains("Capture finished") && text.contains("0 → 1, 1 new"));
        assert!(!text.contains("chat #871") && !text.contains("#872"));
        restored.finish(50);
        assert!(!restored.active(50));
    }

    #[test]
    fn same_title_distinguishes_five_visible_chats_from_related_history() {
        use crate::ollama::Message;
        use crate::state::{Conversation, State, ThreadInfo};
        let history = vec![
            Message::new("user", "private question"),
            Message::new("assistant", "private answer"),
        ];
        let mut state = State {
            chat_id: 7,
            history: history.clone(),
            thread: ThreadInfo {
                title: "Internal project name".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        for id in 1..=5 {
            let thread = ThreadInfo {
                title: "internal project name".into(),
                archived: id == 5,
                ..Default::default()
            };
            state.chats.insert(
                id,
                Conversation {
                    thread,
                    history: if id <= 2 { history.clone() } else { vec![] },
                    ..Default::default()
                },
            );
        }
        synced_title(&mut state, 7, true);
        let event = &state.sync_diagnostics[0];
        assert!(matches!(
            event.decision,
            SyncDecision::SameTitle {
                visible: 5,
                related: 2,
                added: true
            }
        ));
        assert_eq!(event.peers, vec![1, 2, 3, 4]);
        let report = review_report(
            &[],
            &state.sync_diagnostics,
            None,
            ReportOptions {
                now: 20,
                include_models: true,
                chat_references: true,
            },
        );
        assert!(
            report.contains("5 visible chats")
                && report.contains("2 other chats")
                && report.contains("chat #7 #1 #2 #3 #4")
        );
        assert!(!report.contains("Internal project name") && !report.contains("private answer"));
    }

    #[test]
    fn report_groups_failures_without_disclosing_their_source() {
        let event = Failure {
            time: 10,
            chat: 876543,
            model: "private-model".into(),
            operation: "web_open".into(),
            reason: Reason::classify("https://example.com/?token=secret returned HTTP 403"),
            browser_link_offered: true,
        };
        let text = report(&[event.clone(), event.clone()], false);
        assert!(text.contains("2 × web_open · HTTP 403 · Browser Link offered"));
        for private in ["secret", "876543", "private-model", "example.com"] {
            assert!(!text.contains(private));
        }
        assert!(report(&[event], true).contains("private-model"));
    }
}
