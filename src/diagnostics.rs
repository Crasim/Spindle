use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

pub fn log(
    state: &mut crate::state::State,
    chat: u64,
    model: &str,
    operation: &str,
    reason: Reason,
    browser_link_offered: bool,
) {
    const LIMIT: usize = 250;
    state.failures.push(Failure {
        time: crate::automation::now(),
        chat,
        model: model.into(),
        operation: operation.into(),
        reason,
        browser_link_offered,
    });
    let excess = state.failures.len().saturating_sub(LIMIT);
    state.failures.drain(..excess);
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

#[cfg(test)]
mod tests {
    use super::*;

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
