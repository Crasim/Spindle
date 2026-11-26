//! The sandbox works on a copy of the project, one per conversation. The model edits and tests
//! the copy freely; the real project changes only when the user approves applying the copy's
//! changes, and a file the user changed in the meantime is never overwritten.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MANIFEST: &str = ".spindle-base.json";
/// Where files the user attaches in a project chat go inside the copy. Never applied.
pub const ATTACHMENTS: &str = "_attachments";
const MAX_FILES: usize = 20_000;
const MAX_BYTES: u64 = 500 * 1024 * 1024;

fn skip(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".spindle"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".pytest_cache"
            | ".tmp"
            | ".spindle-access"
    ) || name == MANIFEST
        || name == ATTACHMENTS
}

/// The copy for a conversation.
pub fn folder(data: &Path, chat: u64, project: &Path) -> PathBuf {
    let tag = crate::tools::hash(project.to_string_lossy().as_bytes());
    data.join("work").join(format!("{tag}-{chat}"))
}

/// Hashes of the project's files when the copy was made, by path relative to the project.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Base {
    pub project: PathBuf,
    pub files: BTreeMap<String, String>,
}

fn walk(root: &Path, mut each: impl FnMut(&str, &Path) -> Result<()>) -> Result<()> {
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip(&name) {
                continue;
            }
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let rel = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                each(&rel, &path)?;
            }
        }
    }
    Ok(())
}

fn file_hash(path: &Path) -> Result<String> {
    Ok(crate::tools::hash(&std::fs::read(path)?))
}

/// Make the copy if it doesn't exist yet. Returns its folder.
pub fn ensure(data: &Path, chat: u64, project: &Path) -> Result<PathBuf> {
    let copy = folder(data, chat, project);
    if copy.join(MANIFEST).is_file() {
        return Ok(copy);
    }
    std::fs::create_dir_all(&copy)?;
    let mut base = Base {
        project: project.to_owned(),
        files: BTreeMap::new(),
    };
    let (mut count, mut bytes) = (0usize, 0u64);
    walk(project, |rel, path| {
        count += 1;
        bytes += std::fs::metadata(path)?.len();
        ensure!(
            count <= MAX_FILES,
            "The project has more than {MAX_FILES} files to copy into the sandbox"
        );
        ensure!(
            bytes <= MAX_BYTES,
            "The project is larger than 500 MB, too large to copy into the sandbox"
        );
        let dest = copy.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, &dest)?;
        base.files.insert(rel.to_owned(), file_hash(path)?);
        Ok(())
    })?;
    std::fs::write(copy.join(MANIFEST), serde_json::to_vec(&base)?)?;
    Ok(copy)
}

fn base(copy: &Path) -> Result<Base> {
    let bytes = std::fs::read(copy.join(MANIFEST))
        .context("The sandbox copy has no record of its starting point")?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Added(String),
    Modified(String),
    Deleted(String),
}

impl Change {
    pub fn path(&self) -> &str {
        match self {
            Change::Added(p) | Change::Modified(p) | Change::Deleted(p) => p,
        }
    }
    pub fn label(&self) -> String {
        match self {
            Change::Added(p) => format!("new {p}"),
            Change::Modified(p) => format!("changed {p}"),
            Change::Deleted(p) => format!("deleted {p}"),
        }
    }
}

/// What differs between the copy and the project as it was copied.
pub fn changes(copy: &Path) -> Result<Vec<Change>> {
    let base = base(copy)?;
    let mut seen = BTreeMap::new();
    walk(copy, |rel, path| {
        seen.insert(rel.to_owned(), file_hash(path)?);
        Ok(())
    })?;
    let mut out = vec![];
    for (rel, hash) in &seen {
        match base.files.get(rel) {
            None => out.push(Change::Added(rel.clone())),
            Some(old) if old != hash => out.push(Change::Modified(rel.clone())),
            _ => {}
        }
    }
    for rel in base.files.keys() {
        if !seen.contains_key(rel) {
            out.push(Change::Deleted(rel.clone()));
        }
    }
    Ok(out)
}

/// A unified-style preview of the changes, for the approval card.
pub fn preview(copy: &Path, changes: &[Change], limit: usize) -> String {
    let Ok(base) = base(copy) else {
        return String::new();
    };
    let mut text = String::new();
    for change in changes {
        text.push_str(&format!("── {}\n", change.label()));
        let old = std::fs::read_to_string(base.project.join(change.path())).unwrap_or_default();
        let new = std::fs::read_to_string(copy.join(change.path())).unwrap_or_default();
        match change {
            Change::Deleted(_) => {}
            Change::Added(_) => {
                for l in new.lines().take(20) {
                    text.push_str(&format!("+ {l}\n"));
                }
            }
            Change::Modified(_) => text.push_str(&crate::diff::unified(&old, &new, 3)),
        }
        if text.len() > limit {
            text.truncate(limit);
            text.push_str("\n…");
            break;
        }
    }
    text
}

/// Copy the changes into the project. A project file that changed since the copy was made is
/// left alone and reported as a conflict. Returns a summary.
pub fn apply(copy: &Path) -> Result<String> {
    let mut record = base(copy)?;
    let project = record.project.clone();
    let mut applied = vec![];
    let mut conflicts = vec![];
    for change in changes(copy)? {
        let rel = change.path().to_owned();
        let target = project.join(&rel);
        ensure!(
            !crate::policy::protected(&target),
            "{rel} is in a protected location"
        );
        let current = target.is_file().then(|| file_hash(&target)).transpose()?;
        let expected = record.files.get(&rel).cloned();
        if current != expected {
            conflicts.push(rel);
            continue;
        }
        match &change {
            Change::Deleted(_) => {
                std::fs::remove_file(&target)?;
                record.files.remove(&rel);
            }
            Change::Added(_) | Change::Modified(_) => {
                let bytes = std::fs::read(copy.join(&rel))?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut tmp =
                    tempfile::NamedTempFile::new_in(target.parent().context("No folder")?)?;
                std::io::Write::write_all(&mut tmp, &bytes)?;
                tmp.persist(&target).map_err(|e| e.error)?;
                record.files.insert(rel.clone(), crate::tools::hash(&bytes));
            }
        }
        applied.push(change.label());
    }
    std::fs::write(copy.join(MANIFEST), serde_json::to_vec(&record)?)?;
    let mut text = if applied.is_empty() {
        "No changes were applied.".to_owned()
    } else {
        format!(
            "Applied {} change{} to the project: {}",
            applied.len(),
            if applied.len() == 1 { "" } else { "s" },
            applied.join(", ")
        )
    };
    if !conflicts.is_empty() {
        text.push_str(&format!(
            "\nNot applied, because the project file changed since the sandbox copy was made: {}",
            conflicts.join(", ")
        ));
    }
    Ok(text)
}

/// What an automatic apply did, kept so it can be undone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Each applied path, with the project file before (None: it didn't exist) and the hash
    /// of what Spindle wrote (None: Spindle deleted it).
    pub files: Vec<(String, Option<String>, Option<String>)>,
}

/// Apply the copy's changes, first saving the project's versions of those files in `keep`
/// so the apply can be undone. Returns the summary, a diff and what to keep.
pub fn apply_checkpointed(copy: &Path, keep: &Path) -> Result<(String, String, Checkpoint)> {
    let found = changes(copy)?;
    let diff = preview(copy, &found, 20_000);
    let project = base(copy)?.project;
    std::fs::create_dir_all(keep)?;
    let mut saved = BTreeMap::new();
    for change in &found {
        let rel = change.path();
        let target = project.join(rel);
        let before = if target.is_file() {
            let bytes = std::fs::read(&target)?;
            let name = crate::tools::hash(rel.as_bytes());
            std::fs::write(keep.join(&name), &bytes)?;
            Some(name)
        } else {
            None
        };
        saved.insert(rel.to_owned(), before);
    }
    let summary = apply(copy)?;
    let record = base(copy)?;
    let mut checkpoint = Checkpoint::default();
    for (rel, before) in saved {
        let target = project.join(&rel);
        let now = target.is_file().then(|| file_hash(&target)).transpose()?;
        // Only files the apply actually changed (not conflicts) can be undone.
        if now.as_ref() == record.files.get(&rel) {
            checkpoint.files.push((rel, before, now));
        }
    }
    std::fs::write(
        keep.join("checkpoint.json"),
        serde_json::to_vec(&checkpoint)?,
    )?;
    Ok((summary, diff, checkpoint))
}

/// Put back the project files an automatic apply changed, and the copy with them, so the
/// model sees the project as it is again. A file changed since the apply is left alone.
pub fn undo(copy: &Path, keep: &Path) -> Result<String> {
    let checkpoint: Checkpoint = serde_json::from_slice(
        &std::fs::read(keep.join("checkpoint.json")).context("The saved versions are gone")?,
    )?;
    let mut record = base(copy)?;
    let project = record.project.clone();
    let (mut restored, mut skipped) = (vec![], vec![]);
    for (rel, before, wrote) in &checkpoint.files {
        let target = project.join(rel);
        let now = target.is_file().then(|| file_hash(&target)).transpose()?;
        if &now != wrote {
            skipped.push(rel.clone());
            continue;
        }
        let in_copy = copy.join(rel);
        match before {
            Some(name) => {
                let bytes = std::fs::read(keep.join(name))?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, &bytes)?;
                if let Some(parent) = in_copy.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&in_copy, &bytes)?;
                record.files.insert(rel.clone(), crate::tools::hash(&bytes));
            }
            None => {
                if target.is_file() {
                    std::fs::remove_file(&target)?;
                }
                if in_copy.is_file() {
                    std::fs::remove_file(&in_copy)?;
                }
                record.files.remove(rel);
            }
        }
        restored.push(rel.clone());
    }
    std::fs::write(copy.join(MANIFEST), serde_json::to_vec(&record)?)?;
    let mut text = format!(
        "Undid {} file{}: {}",
        restored.len(),
        if restored.len() == 1 { "" } else { "s" },
        restored.join(", ")
    );
    if !skipped.is_empty() {
        text.push_str(&format!(
            ". Left alone because they changed since: {}",
            skipped.join(", ")
        ));
    }
    Ok(text)
}

/// Remove a conversation's copy.
pub fn discard(data: &Path, chat: u64, project: &Path) -> Result<()> {
    let copy = folder(data, chat, project);
    if copy.exists() {
        std::fs::remove_dir_all(copy)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_apply_can_be_undone() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("p");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("snake.html"), "old\n").unwrap();
        let copy = ensure(&tmp.path().join("data"), 1, &project).unwrap();
        std::fs::write(copy.join("snake.html"), "new\n").unwrap();
        std::fs::write(copy.join("added.js"), "x\n").unwrap();
        let keep = tmp.path().join("keep");
        let (summary, diff, cp) = apply_checkpointed(&copy, &keep).unwrap();
        assert!(summary.contains("Applied 2"), "{summary}");
        assert!(diff.contains("- old\n+ new"), "{diff}");
        assert_eq!(cp.files.len(), 2);
        assert_eq!(
            std::fs::read_to_string(project.join("snake.html")).unwrap(),
            "new\n"
        );
        let text = undo(&copy, &keep).unwrap();
        assert!(text.starts_with("Undid 2"), "{text}");
        assert_eq!(
            std::fs::read_to_string(project.join("snake.html")).unwrap(),
            "old\n"
        );
        assert!(!project.join("added.js").exists());
        assert!(
            changes(&copy).unwrap().is_empty(),
            "the copy matches the project again"
        );
    }

    #[test]
    fn copies_apply_back_without_overwriting_user_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let project = tmp.path().join("p");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::write(project.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(project.join("src/a.py"), "x = 1\n").unwrap();
        std::fs::write(project.join("notes.md"), "keep\n").unwrap();
        std::fs::write(project.join("old.txt"), "bye\n").unwrap();
        let copy = ensure(&data, 3, &project).unwrap();
        assert!(
            !copy.join(".git").exists(),
            "repository internals are not copied"
        );
        std::fs::write(copy.join("src/a.py"), "x = 2\n").unwrap();
        std::fs::write(copy.join("src/b.py"), "y = 1\n").unwrap();
        std::fs::write(copy.join("notes.md"), "model edit\n").unwrap();
        std::fs::remove_file(copy.join("old.txt")).unwrap();
        let found = changes(&copy).unwrap();
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(preview(&copy, &found, 4000).contains("- x = 1\n+ x = 2"));
        // The user edits notes.md in the project meanwhile.
        std::fs::write(project.join("notes.md"), "user edit\n").unwrap();
        let summary = apply(&copy).unwrap();
        assert!(summary.contains("Applied 3 changes"), "{summary}");
        assert!(summary.contains("notes.md"));
        assert_eq!(
            std::fs::read_to_string(project.join("src/a.py")).unwrap(),
            "x = 2\n"
        );
        assert!(project.join("src/b.py").exists() && !project.join("old.txt").exists());
        assert_eq!(
            std::fs::read_to_string(project.join("notes.md")).unwrap(),
            "user edit\n"
        );
        // Only the conflict remains.
        let left = changes(&copy).unwrap();
        assert_eq!(left, vec![Change::Modified("notes.md".into())]);
        discard(&data, 3, &project).unwrap();
        assert!(!copy.exists());
    }
}
