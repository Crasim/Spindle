//! Git, read-only except for a commit the user confirms.
//!
//! Spindle reads status, diffs and history for a project folder. It never commits on its own:
//! `commit` is called only from the desktop after the user reviews the message and confirms.
//! Every call disables the repository settings that would run programs from the repository
//! (fsmonitor, external diff and textconv drivers, hooks), so a cloned repository cannot run
//! code through Spindle.

use anyhow::{Context, Result, bail, ensure};
use std::path::Path;
use std::process::Command;

const SAFE: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=",
    "-c",
    "diff.external=",
    "-c",
    "core.pager=cat",
    "-c",
    "color.ui=false",
];

const MAX_DIFF: usize = 60_000;

fn git(folder: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(folder)
        .args(SAFE)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd
        .output()
        .context("Git is not installed or not on PATH")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{}", err.lines().last().unwrap_or("git failed").trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    /// Two-letter porcelain code, such as " M", "A " or "??".
    pub code: String,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub repository: bool,
    pub branch: String,
    pub ahead_behind: String,
    pub changes: Vec<Change>,
    /// Recent commits: (short hash, subject, relative date).
    pub log: Vec<(String, String, String)>,
}

impl Change {
    pub fn describe(&self) -> &'static str {
        match self.code.as_str() {
            "??" => "new",
            c if c.contains('D') => "deleted",
            c if c.contains('A') => "added",
            c if c.contains('R') => "renamed",
            c if c.contains('U') => "conflict",
            _ => "modified",
        }
    }
}

pub fn parse_status(text: &str) -> (String, String, Vec<Change>) {
    let mut branch = String::new();
    let mut ahead = String::new();
    let mut changes = vec![];
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            if let Some(name) = head.strip_prefix("No commits yet on ") {
                branch = name.to_owned();
                ahead = "no commits yet".into();
            } else {
                let (name, rest) = head.split_once(' ').unwrap_or((head, ""));
                branch = name.split("...").next().unwrap_or(name).to_owned();
                ahead = rest.trim_matches(['[', ']']).to_owned();
            }
        } else if line.len() > 3 {
            let path = &line[3..];
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            changes.push(Change {
                code: line[..2].to_owned(),
                path: path.trim_matches('"').to_owned(),
            });
        }
    }
    (branch, ahead, changes)
}

pub fn status(folder: &Path) -> Result<Status> {
    let inside = git(folder, &["rev-parse", "--is-inside-work-tree"]);
    if !inside.is_ok_and(|s| s.trim() == "true") {
        return Ok(Status::default());
    }
    let (branch, ahead_behind, changes) = parse_status(&git(
        folder,
        &["status", "--porcelain=v1", "-b", "--untracked-files=all"],
    )?);
    let log = git(folder, &["log", "-n", "15", "--format=%h%x1f%s%x1f%cr"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut p = l.split('\x1f');
            Some((p.next()?.into(), p.next()?.into(), p.next()?.into()))
        })
        .collect();
    Ok(Status {
        repository: true,
        branch,
        ahead_behind,
        changes,
        log,
    })
}

/// Uncommitted changes as a unified diff, including new files, cut to a size a model can read.
pub fn diff(folder: &Path) -> Result<String> {
    let base = ["--no-ext-diff", "--no-textconv", "--stat", "--patch"];
    let has_head = git(folder, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let mut text = if has_head {
        git(folder, &[&["diff", "HEAD"], &base[..]].concat())?
    } else {
        git(folder, &[&["diff", "--cached"], &base[..]].concat())?
    };
    let untracked = git(folder, &["ls-files", "--others", "--exclude-standard"])?;
    for name in untracked.lines().take(20) {
        let path = folder.join(name);
        let body = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= 64 * 1024)
            .and_then(|_| std::fs::read_to_string(&path).ok());
        match body {
            Some(body) => text.push_str(&format!(
                "\nNew file {name}\n{}\n",
                body.lines()
                    .map(|l| format!("+{l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
            None => text.push_str(&format!("\nNew file {name} (binary or large; not shown)\n")),
        }
    }
    ensure!(!text.trim().is_empty(), "No uncommitted changes");
    if text.len() > MAX_DIFF {
        let mut end = MAX_DIFF;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… diff shortened");
    }
    Ok(text)
}

/// Stage everything and commit. Only call this after the user confirmed the message.
pub fn commit(folder: &Path, message: &str) -> Result<String> {
    let message = message.trim();
    ensure!(!message.is_empty(), "Write a commit message");
    git(folder, &["add", "--all"])?;
    git(folder, &["commit", "--no-verify", "--quiet", "-m", message])?;
    Ok(git(folder, &["log", "-n", "1", "--format=%h %s"])?
        .trim()
        .to_owned())
}

/// Create a repository in a project folder that has none. Only on the user's click.
pub fn init(folder: &Path) -> Result<()> {
    git(folder, &["init", "--quiet"]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_status_is_read() {
        let (branch, ahead, changes) = parse_status(
            "## main...origin/main [ahead 2]\n M src/a.rs\n?? notes.md\nR  old.txt -> new.txt\n",
        );
        assert_eq!(branch, "main");
        assert_eq!(ahead, "ahead 2");
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[1].describe(), "new");
        assert_eq!(changes[2].path, "new.txt");
        let (branch, ahead, _) = parse_status("## No commits yet on main\n");
        assert_eq!(
            (branch.as_str(), ahead.as_str()),
            ("main", "no commits yet")
        );
    }

    #[test]
    fn commits_only_through_commit() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(!status(dir).unwrap().repository);
        init(dir).unwrap();
        git(dir, &["config", "user.email", "t@example.com"]).unwrap();
        git(dir, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let s = status(dir).unwrap();
        assert!(s.repository && s.changes.len() == 1);
        assert!(diff(dir).unwrap().contains("+hello"));
        assert!(commit(dir, " ").is_err());
        assert!(commit(dir, "First").unwrap().ends_with("First"));
        let s = status(dir).unwrap();
        assert!(s.changes.is_empty() && s.log.len() == 1);
        assert!(diff(dir).is_err());
    }
}
