//! The permission broker: what a model may do, decided outside the model.
//!
//! Every tool call is classified into a risk class and checked immediately before it runs.
//! The answer is allow, ask or deny. Grants are kept by Spindle, never by the model, and are
//! qualified by the risk class, a scope (a folder) and the trust level of the model that asked:
//! a less-trusted model never inherits a grant made for a more-trusted one.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How much a model is trusted to act on the computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Trust {
    /// Small or old models: standing grants only for low-risk work.
    Restricted,
    #[default]
    Standard,
    /// Large, recent models the user relies on for risky work, with Spindle's summary.
    Trusted,
}

impl Trust {
    pub const ALL: [Trust; 3] = [Trust::Restricted, Trust::Standard, Trust::Trusted];

    pub fn label(self) -> &'static str {
        match self {
            Trust::Restricted => "Restricted",
            Trust::Standard => "Standard",
            Trust::Trusted => "Trusted",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Trust::Restricted => {
                "Asks every time for anything outside the project; can't be given risky work"
            }
            Trust::Standard => {
                "Can be allowed for a conversation or project inside the project folder"
            }
            Trust::Trusted => {
                "Can be given standing permission for risky work, with a summary first"
            }
        }
    }
}

/// Parameter count in billions from a model name, such as `qwen3.5:9b` or `mixtral:8x7b`.
pub fn parameters(model: &str) -> Option<f64> {
    let name = model.to_ascii_lowercase();
    let bytes = name.as_bytes();
    let mut best = None;
    for (i, _) in name.match_indices('b') {
        // The `b` must end a token: followed by nothing, or a separator.
        if bytes.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let start = name[..i]
            .rfind(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'x'))
            .map(|p| p + 1)
            .unwrap_or(0);
        let number = &name[start..i];
        let value = match number.split_once('x') {
            Some((n, each)) => n
                .parse::<f64>()
                .ok()
                .zip(each.parse::<f64>().ok())
                .map(|(n, e)| n * e),
            None => number.parse::<f64>().ok(),
        };
        if let Some(v) = value.filter(|v| *v > 0.) {
            best = Some(v);
        }
    }
    best
}

const SIX_MONTHS: i64 = 183 * 24 * 3600;

/// The trust a model starts with: models of 16B parameters or fewer, or released more than six
/// months ago, are Restricted; very large ones are Trusted; unknown sizes are Standard.
pub fn default_trust(model: &str, released: Option<i64>, now: i64) -> Trust {
    if released.is_some_and(|r| now - r > SIX_MONTHS) {
        return Trust::Restricted;
    }
    match parameters(model).or_else(|| crate::ollama::known_size(model)) {
        Some(b) if b <= 16. => Trust::Restricted,
        Some(b) if b >= 100. => Trust::Trusted,
        _ => Trust::Standard,
    }
}

/// The trust in effect for a model: the user's choice, else its size and age.
/// Small models (about 9B parameters or fewer) get short, plain instructions.
pub fn small(model: &str) -> bool {
    // SPINDLE_COMPACT=off turns small-model mode off, for comparing.
    match std::env::var("SPINDLE_COMPACT").as_deref() {
        Ok("off") => return false,
        // SPINDLE_COMPACT=on gives every model the short directions, for comparing.
        Ok("on") => return true,
        _ => {}
    }
    parameters(model)
        .or_else(|| crate::ollama::known_size(model))
        .is_some_and(|b| b <= 9.5)
}

pub fn trust(state: &crate::state::State, model: &str) -> Trust {
    state
        .tuning
        .get(model)
        .and_then(|t| t.trust)
        .unwrap_or_else(|| default_trust(model, None, crate::automation::now()))
}

/// What kind of effect an action has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Risk {
    /// Read files in the project.
    Read,
    /// Read files outside the project.
    ReadOutside,
    /// Create or change files in the project.
    Write,
    /// Create or change files outside the project.
    WriteOutside,
    /// Create or change files in the conversation's sandbox copy.
    Sandboxed,
    /// Run code in the sandbox.
    Run,
    /// Compile staged source or test a stored build in a disposable sandbox on this device.
    SandboxBuild,
    Native,
    Browser,
    Connector,
    /// Download and install software for Spindle (its Python, or packages for the sandbox).
    Install,
    Destructive,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Read => "Read project files",
            Risk::ReadOutside => "Read files outside the project",
            Risk::Write => "Change project files",
            Risk::WriteOutside => "Change files outside the project",
            Risk::Sandboxed => "Change files in the sandbox copy",
            Risk::Run => "Run code in the sandbox",
            Risk::SandboxBuild => "Build or test in the sandbox",
            Risk::Native => "Load, build or run executable tools",
            Risk::Browser => "Use the connected browser tab",
            Risk::Connector => "Read from a connected account",
            Risk::Install => "Download and install software for Spindle",
            Risk::Destructive => "Confirm this change",
        }
    }
    pub fn outside(self) -> bool {
        matches!(self, Risk::ReadOutside | Risk::WriteOutside)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    Once,
    Conversation,
    Project,
    Always,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Once => "Allow once",
            Level::Conversation => "Allow for this conversation",
            Level::Project => "Allow for this project",
            Level::Always => "Allow always",
        }
    }
}

/// The approval levels offered for a risk to a model of this trust. Empty means denied.
pub fn levels(risk: Risk, trust: Trust) -> &'static [Level] {
    use Level::*;
    match (risk, trust) {
        (Risk::Read | Risk::Sandboxed | Risk::Run, _) => &[Once, Conversation, Project],
        (Risk::Write, _) => &[Once, Conversation, Project],
        (Risk::ReadOutside, Trust::Restricted) => &[Once, Conversation],
        (Risk::ReadOutside, _) => &[Once, Conversation, Project],
        (Risk::WriteOutside, Trust::Restricted) => &[Once],
        (Risk::WriteOutside, Trust::Standard) => &[Once, Conversation],
        (Risk::WriteOutside, Trust::Trusted) => &[Once, Conversation, Project],
        (Risk::Install, Trust::Trusted) => &[Once, Conversation],
        (Risk::Install, _) => &[Once],
        (Risk::SandboxBuild, _) => &[Once, Conversation],
        (Risk::Native | Risk::Connector | Risk::Destructive, _) => &[Once],
        (Risk::Browser, _) => &[Once, Conversation, Project, Always],
    }
}

/// A warning shown with a broad approval.
pub fn warning(risk: Risk, level: Level) -> Option<&'static str> {
    match (risk, level) {
        (Risk::WriteOutside, Level::Project) => Some(
            "Every chat in this project, with a model this trusted or more, could change files in this folder without asking.",
        ),
        (Risk::WriteOutside, Level::Conversation) | (Risk::Write, Level::Project) => {
            Some("Changes will be made without asking again. Each one is still logged.")
        }
        (Risk::SandboxBuild, Level::Conversation) => {
            Some("Covers local sandboxed builds and tests in this chat and its current workspace.")
        }
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Holder {
    Conversation(u64),
    Project(PathBuf),
    Always,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub id: u64,
    pub risk: Risk,
    /// The folder the grant covers, including everything inside it.
    pub scope: PathBuf,
    pub holder: Holder,
    /// The least-trusted model this grant applies to.
    pub trust: Trust,
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
}

impl Grant {
    pub fn covers(
        &self,
        risk: Risk,
        target: &Path,
        chat: u64,
        project: Option<&Path>,
        trust: Trust,
    ) -> bool {
        let in_scope = if risk == Risk::SandboxBuild {
            self.scope == target && matches!(self.holder, Holder::Conversation(_))
        } else {
            target.starts_with(&self.scope)
        };
        self.risk == risk
            && trust >= self.trust
            && in_scope
            && match &self.holder {
                Holder::Conversation(id) => *id == chat,
                Holder::Project(p) => project == Some(p.as_path()),
                Holder::Always => true,
            }
    }

    pub fn describe(&self) -> String {
        let place = match &self.holder {
            Holder::Conversation(_) => "this conversation",
            Holder::Project(_) => "this project",
            Holder::Always => "all chats on this workspace",
        };
        if let Some(access) = &self.access {
            return format!("{access} · {place}");
        }
        format!(
            "{} in {} · {} · {}+ models",
            self.risk.label(),
            crate::projects::display_path(&self.scope),
            place,
            self.trust.label()
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow(Option<u64>),
    Ask,
    Deny(String),
}

/// Paths a model may never write, whatever is granted: repository internals and Spindle's own
/// state.
pub fn protected(target: &Path) -> bool {
    target.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        s == ".git" || s == ".spindle" || s == ".ssh"
    })
}

pub struct Request<'a> {
    pub risk: Risk,
    pub target: &'a Path,
    pub chat: u64,
    pub project: Option<&'a Path>,
    pub trust: Trust,
}

pub fn decide(grants: &[Grant], r: &Request) -> Decision {
    if matches!(r.risk, Risk::Write | Risk::WriteOutside | Risk::Destructive) && protected(r.target) {
        return Decision::Deny(
            "That location is protected; Spindle never lets a model change it".into(),
        );
    }
    if matches!(r.risk, Risk::Native | Risk::Connector | Risk::Destructive) {
        return Decision::Ask;
    }
    // Reading the project, and anything contained by the sandbox, needs no approval.
    if matches!(r.risk, Risk::Read | Risk::Sandboxed | Risk::Run) {
        return Decision::Allow(None);
    }
    if levels(r.risk, r.trust).is_empty() {
        return Decision::Deny(format!(
            "{} isn't available to {} models",
            r.risk.label(),
            r.trust.label()
        ));
    }
    match grants
        .iter()
        .find(|g| g.covers(r.risk, r.target, r.chat, r.project, r.trust))
    {
        Some(g) => Decision::Allow(Some(g.id)),
        None => Decision::Ask,
    }
}

/// The folder a standing grant for this request would cover.
pub fn scope_for(risk: Risk, target: &Path, project: Option<&Path>) -> PathBuf {
    if risk == Risk::Browser {
        return target.parent().unwrap_or(target).to_owned();
    }
    if matches!(risk, Risk::Install | Risk::SandboxBuild) {
        return target.to_owned();
    }
    match (risk.outside(), project) {
        (false, Some(p)) => p.to_owned(),
        _ => target
            .parent()
            .filter(|_| target.extension().is_some() || !target.is_dir())
            .unwrap_or(target)
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_size_and_age_set_default_trust() {
        assert_eq!(parameters("qwen3.5:9b"), Some(9.));
        assert_eq!(parameters("@ollama-cloud/gpt-oss:120b"), Some(120.));
        assert_eq!(parameters("mixtral:8x7b"), Some(56.));
        assert_eq!(parameters("deepseek-r1:1.5b"), Some(1.5));
        assert_eq!(parameters("Maternion/spark-x2.6:4b"), Some(4.));
        assert_eq!(parameters("@openai/gpt-5"), None);
        assert_eq!(parameters("bge-base"), None);
        let now = 1_800_000_000;
        assert_eq!(default_trust("qwen3:16b", None, now), Trust::Restricted);
        assert_eq!(default_trust("qwen3:32b", None, now), Trust::Standard);
        assert_eq!(default_trust("gpt-oss:120b", None, now), Trust::Trusted);
        assert_eq!(
            default_trust("gpt-oss:120b", Some(now - 200 * 86400), now),
            Trust::Restricted
        );
        assert_eq!(default_trust("@openai/gpt-5", None, now), Trust::Standard);
    }

    #[test]
    fn grants_follow_scope_holder_and_trust() {
        let project = PathBuf::from("/p");
        let file = project.join("src/a.py");
        let ask = |grants: &[Grant], trust, chat| {
            decide(
                grants,
                &Request {
                    risk: Risk::Write,
                    target: &file,
                    chat,
                    project: Some(&project),
                    trust,
                },
            )
        };
        assert_eq!(ask(&[], Trust::Standard, 1), Decision::Ask);
        assert_eq!(
            decide(
                &[],
                &Request {
                    risk: Risk::Read,
                    target: &file,
                    chat: 1,
                    project: Some(&project),
                    trust: Trust::Restricted
                }
            ),
            Decision::Allow(None)
        );
        let grant = Grant {
            access: None,
            id: 7,
            risk: Risk::Write,
            scope: project.clone(),
            holder: Holder::Conversation(1),
            trust: Trust::Standard,
            created: 0,
        };
        let grants = [grant];
        assert_eq!(ask(&grants, Trust::Standard, 1), Decision::Allow(Some(7)));
        assert_eq!(ask(&grants, Trust::Trusted, 1), Decision::Allow(Some(7)));
        assert_eq!(
            ask(&grants, Trust::Restricted, 1),
            Decision::Ask,
            "a smaller model doesn't inherit it"
        );
        assert_eq!(
            ask(&grants, Trust::Standard, 2),
            Decision::Ask,
            "another conversation doesn't inherit it"
        );
        let git = project.join(".git/config");
        assert!(matches!(
            decide(
                &grants,
                &Request {
                    risk: Risk::Write,
                    target: &git,
                    chat: 1,
                    project: Some(&project),
                    trust: Trust::Trusted
                }
            ),
            Decision::Deny(_)
        ));
        assert_eq!(
            levels(Risk::WriteOutside, Trust::Restricted),
            &[Level::Once]
        );
        assert_eq!(levels(Risk::WriteOutside, Trust::Trusted).len(), 3);
        assert_eq!(
            scope_for(Risk::WriteOutside, Path::new("/r/out.txt"), Some(&project)),
            PathBuf::from("/r")
        );
        assert_eq!(scope_for(Risk::Write, &file, Some(&project)), project);
    }
}
