//! One-time approvals for model access to a named local desktop window.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(any(windows, target_os = "linux"))]
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};

#[cfg(any(windows, target_os = "linux"))]
fn epoch() -> &'static AtomicU64 {
    static EPOCH: OnceLock<AtomicU64> = OnceLock::new();
    EPOCH.get_or_init(|| AtomicU64::new(u64::from_le_bytes(
        crate::vault::random().expect("desktop approval nonce"),
    )))
}

pub fn approval_is_current(target: &Path) -> bool {
    #[cfg(any(windows, target_os = "linux"))]
    return target.to_str().and_then(|ticket| ticket.split('-').next())
        .and_then(|epoch| epoch.parse::<u64>().ok()) == Some(epoch().load(Ordering::Acquire));
    #[cfg(not(any(windows, target_os = "linux")))]
    { let _ = target; false }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Access {
    pub enabled: bool,
    pub list: bool,
    pub inspect: bool,
    pub click: bool,
    pub type_text: bool,
}

impl Default for Access {
    fn default() -> Self {
        Self { enabled: cfg!(windows), list: true, inspect: true, click: true, type_text: true }
    }
}

impl Access {
    pub fn check(self, action: &Action) -> Result<()> {
        if matches!(action, Action::Release) { return Ok(()); }
        ensure!(SUPPORTED, "App control is unavailable on this device. See Settings › Computer use › App control.");
        ensure!(self.enabled, "App control is off. Turn it on in Settings › Computer use › App control.");
        ensure!(match action {
            Action::Windows => self.list,
            Action::Inspect { .. } => self.inspect,
            Action::Click { .. } => self.click,
            Action::Type { .. } => self.type_text,
            Action::Release => true,
        }, "This action is disabled. Change its permission in Settings › Computer use › App control › Manage.");
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Windows,
    Inspect { window: String },
    Click { window: String, x: i32, y: i32 },
    Type { window: String, text: String },
    Release,
}

impl Action {
    pub fn summary(&self) -> String {
        match self {
            Self::Windows => "List open desktop windows".into(),
            Self::Inspect { window } => format!("Inspect desktop window {window}"),
            Self::Click { window, .. } => format!("Click in desktop window {window}"),
            Self::Type { window, .. } => format!("Type in desktop window {window}"),
            Self::Release => "Release desktop control".into(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Windows | Self::Release => return Ok(()),
            Self::Type { text, .. } => ensure!(
                !text.is_empty() && text.len() <= 500 && !text.chars().any(char::is_control),
                "Send 1–500 bytes of literal text without control characters."
            ),
            _ => {}
        }
        let name = self.window().unwrap();
        ensure!(
            !name.trim().is_empty() && name.len() <= 180 && !name.chars().any(char::is_control),
            "Specify the window by title."
        );
        Ok(())
    }

    fn window(&self) -> Option<&str> {
        match self {
            Self::Inspect { window } | Self::Click { window, .. } | Self::Type { window, .. } => {
                Some(window)
            }
            Self::Windows | Self::Release => None,
        }
    }
}

pub const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));

#[cfg(windows)]
#[path = "computer_use_windows.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "computer_use_linux.rs"]
mod platform;

pub fn setup_status() -> &'static str {
    #[cfg(windows)]
    { "Built into Spindle for Windows. No extension or installation required." }
    #[cfg(target_os = "linux")]
    { platform::setup_status() }
    #[cfg(target_os = "android")]
    { "Android app control requires an Accessibility service and Android permission. This build does not include that service." }
    #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
    { "App control is unavailable on this platform." }
}

#[cfg(target_os = "linux")]
pub fn check_setup() -> String {
    match platform::check_setup() {
        Ok(details) => details.into(),
        Err(error) => format!("{error:#}"),
    }
}

pub fn preview(action: &Action) -> Result<(PathBuf, String)> {
    action.validate()?;
    #[cfg(any(windows, target_os = "linux"))]
    return platform::preview(action);
    #[cfg(not(any(windows, target_os = "linux")))]
    anyhow::bail!(setup_status());
}

pub fn run(
    action: &Action,
    target: &Path,
    chat: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String> {
    #[cfg(any(windows, target_os = "linux"))]
    return platform::run(action, target, chat, cancel);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (action, target, chat, cancel);
        anyhow::bail!(setup_status());
    }
}

pub fn stop(chat: Option<u64>) {
    #[cfg(any(windows, target_os = "linux"))]
    platform::stop(chat);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = chat;
}

pub fn current() -> Option<String> {
    #[cfg(any(windows, target_os = "linux"))]
    return platform::current();
    #[cfg(not(any(windows, target_os = "linux")))]
    None
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    #[test]
    fn access_rules_are_independent_and_release_remains_available() {
        let mut permissions = Access { enabled: true, click: false, ..Default::default() };
        assert!(permissions.check(&Action::Windows).is_ok());
        assert!(permissions.check(&Action::Click { window: "App".into(), x: 1, y: 1 }).is_err());
        permissions.enabled = false;
        assert!(permissions.check(&Action::Windows).is_err());
        assert!(permissions.check(&Action::Release).is_ok());
    }
}
