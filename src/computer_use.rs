//! One-time approvals for model access to a named local desktop window.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

pub const SUPPORTED: bool = cfg!(windows);

#[cfg(windows)]
#[path = "computer_use_windows.rs"]
mod windows;

pub fn preview(action: &Action) -> Result<(PathBuf, String)> {
    action.validate()?;
    #[cfg(windows)]
    return windows::preview(action);
    #[cfg(not(windows))]
    anyhow::bail!("Desktop app control is available in Spindle for Windows.");
}

pub fn run(
    action: &Action,
    target: &Path,
    chat: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String> {
    #[cfg(windows)]
    return windows::run(action, target, chat, cancel);
    #[cfg(not(windows))]
    {
        let _ = (action, target, chat, cancel);
        anyhow::bail!("Desktop app control is available in Spindle for Windows.");
    }
}

pub fn stop(chat: Option<u64>) {
    #[cfg(windows)]
    windows::stop(chat);
    #[cfg(not(windows))]
    let _ = chat;
}

pub fn current() -> Option<String> {
    #[cfg(windows)]
    return windows::current();
    #[cfg(not(windows))]
    None
}
