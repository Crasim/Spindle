//! The sandbox works on a copy of the project, one per conversation. The model edits and tests
//! the copy freely; the real project changes only when the user approves applying the copy's
//! changes, and a file the user changed in the meantime is never overwritten.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const MANIFEST: &str = ".spindle-base.json";
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
    let builds = root.join(crate::native::PROJECT_OUTPUTS);
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip(&name) {
                continue;
            }
            let kind = entry.file_type()?;
            let path = entry.path();
            if path == builds { continue; }
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

pub(crate) fn check_project(copy: &Path, project: &Path) -> Result<()> {
    ensure!(base(copy)?.project.canonicalize()? == project.canonicalize()?, "The sandbox belongs to a different project");
    Ok(())
}

/// Merge current project files into the sandbox wherever it still matches its starting point.
pub(crate) fn refresh(copy: &Path, project: &Path) -> Result<()> {
    let project = project.canonicalize()?;
    let mut record = base(copy)?;
    ensure!(record.project.canonicalize()? == project, "The sandbox belongs to a different project");
    let copy = copy.canonicalize()?;
    let mut incoming = BTreeMap::new();
    let mut bytes = 0u64;
    walk(&project, |rel, path| {
        if path.canonicalize()? != path { return Ok(()); }
        bytes += std::fs::metadata(path)?.len();
        ensure!(incoming.len() < MAX_FILES && bytes <= MAX_BYTES, "The project exceeds the sandbox copy limits");
        incoming.insert(rel.to_owned(), file_hash(path)?);
        Ok(())
    })?;
    let changed: std::collections::BTreeSet<_> = incoming.keys().chain(record.files.keys())
        .filter(|rel| incoming.get(*rel) != record.files.get(*rel)).cloned().collect();
    let mut updated = false;
    let result = (|| -> Result<()> {
        for rel in changed {
            let (target, inside) = crate::tools::resolve(&copy, &rel)?;
            ensure!(inside && target == copy.join(&rel), "Sandbox file path has changed: {rel}");
            let current = target.is_file().then(|| file_hash(&target)).transpose()?;
            if current.as_ref() != record.files.get(&rel) { continue; }
            if incoming.contains_key(&rel) {
                let path = project.join(&rel);
                ensure!(path.canonicalize()? == path, "Project file path has changed: {rel}");
                let contents = std::fs::read(path)?;
                std::fs::create_dir_all(target.parent().context("Missing sandbox folder")?)?;
                crate::tools::write_generated(&target, &contents, current.is_some())?;
                record.files.insert(rel, crate::tools::hash(&contents));
            } else if target.is_file() {
                std::fs::remove_file(target)?;
                record.files.remove(&rel);
            }
            updated = true;
        }
        Ok(())
    })();
    if updated { crate::tools::write_generated(&copy.join(MANIFEST), &serde_json::to_vec(&record)?, true)?; }
    result
}

/// Bring a restored project file into this chat's sandbox when its copy is clean.
pub(crate) fn refresh_restored_file(data: &Path, chat: u64, project: &Path, file: &Path) -> Result<()> {
    let project_root = project.canonicalize()?;
    let Ok(relative) = file.strip_prefix(&project_root) else { return Ok(()); };
    let rel = relative.to_str().context("Invalid project file name")?.replace('\\', "/");
    let copy = folder(data, chat, project);
    if !copy.join(MANIFEST).is_file() {
        return Ok(());
    }
    let mut record = base(&copy)?;
    ensure!(record.project.canonicalize()? == project_root, "Sandbox project has changed");
    let (target, inside) = crate::tools::resolve(&copy, &rel)?;
    ensure!(inside && target == copy.canonicalize()?.join(relative), "Sandbox file path has changed");
    let baseline = record.files.get(&rel);
    let current = target.is_file().then(|| file_hash(&target)).transpose()?;
    if current.as_ref() != baseline {
        return Ok(());
    }
    let bytes = std::fs::read(file)?;
    std::fs::create_dir_all(target.parent().context("Missing sandbox folder")?)?;
    crate::file_history::write(&target, &bytes, target.is_file())?;
    record.files.insert(rel, crate::tools::hash(&bytes));
    crate::tools::write_generated(&copy.join(MANIFEST), &serde_json::to_vec(&record)?, true)?;
    Ok(())
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
            text.truncate(text.floor_char_boundary(limit));
            text.push_str("\n…");
            break;
        }
    }
    text
}

/// Copy the changes into the project. A project file that changed since the copy was made is
/// left alone and reported as a conflict. Returns a summary.
#[cfg(test)]
fn apply(copy: &Path) -> Result<String> {
    Ok(apply_changes(copy, changes(copy)?, None)?.summary)
}

pub(crate) fn deletion_preview(changes: &[Change]) -> Option<String> {
    let paths: Vec<_> = changes.iter().filter_map(|change| match change {
        Change::Deleted(path) => Some(format!("• {path}")),
        _ => None,
    }).collect();
    (!paths.is_empty()).then(|| format!("Apply the sandbox changes and delete these project files?\n\n{}", paths.join("\n")))
}

pub(crate) fn apply_approved(copy: &Path, confirmed: Option<&str>, keep: &Path) -> Result<crate::state::Applied> {
    let found = changes(copy)?;
    ensure!(deletion_preview(&found).as_deref() == confirmed, "Project deletions require confirmation for the current list of files");
    apply_checkpointed_changes(copy, keep, found)
}

fn apply_changes(copy: &Path, changes: Vec<Change>, keep: Option<&Path>) -> Result<crate::state::Applied> {
    let mut record = base(copy)?;
    let project = record.project.clone();
    let mut applied = vec![];
    let mut edits = vec![];
    let mut checkpoint = Checkpoint::default();
    let mut patch_bytes = 0usize;
    let mut conflicts = vec![];
    let canonical = project.canonicalize()?;
    let result = (|| -> Result<()> {
        for change in changes {
            let rel = change.path().to_owned();
            let (target, inside) = crate::tools::resolve(&project, &rel)?;
            ensure!(
                inside && target == canonical.join(&rel) && !crate::policy::protected(&target),
                "{rel} is in a protected location"
            );
            let current = target.is_file().then(|| file_hash(&target)).transpose()?;
            let expected = record.files.get(&rel).cloned();
            if current != expected {
                conflicts.push(rel);
                continue;
            }
            let (revision, edit, wrote) = match &change {
                Change::Deleted(_) => {
                    let (revision, edit) = crate::file_history::remove_recorded(&target, keep.map(|_| rel.as_str()))?;
                    (revision, edit, None)
                }
                Change::Added(_) | Change::Modified(_) => {
                    let bytes = std::fs::read(copy.join(&rel))?;
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let (revision, edit) = crate::file_history::write_recorded(&target, &bytes, current.is_some(), keep.map(|_| rel.as_str()))?;
                    (revision, edit, Some(crate::tools::hash(&bytes)))
                }
            };
            if let Some(mut edit) = edit {
                if let Some(patch) = &edit.patch {
                    patch_bytes = patch_bytes.saturating_add(patch.len());
                    if patch_bytes > 96_000 { edit.patch = None; }
                }
                edits.push(edit);
            }
            if current.is_none() || revision.is_some() {
                checkpoint.files.push((rel.clone(), revision.map(|id| format!("history:{id}")), wrote.clone()));
            }
            if let Some(wrote) = wrote {
                record.files.insert(rel, wrote);
            } else {
                record.files.remove(&rel);
            }
            applied.push(change.label());
        }
        Ok(())
    })();
    if applied.is_empty() && let Err(error) = &result { anyhow::bail!("{error:#}"); }
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
    if let Err(error) = result { text.push_str(&format!("\nStopped applying: {error:#}")); }
    let mut saved = PathBuf::new();
    if !applied.is_empty() {
        let save = (|| -> Result<()> {
            crate::tools::write_generated(&copy.join(MANIFEST), &serde_json::to_vec(&record)?, true)?;
            if let Some(keep) = keep {
                if checkpoint.files.len() == applied.len() {
                    std::fs::create_dir_all(keep)?;
                    crate::tools::write_generated(&keep.join("checkpoint.json"), &serde_json::to_vec(&checkpoint)?, false)?;
                    saved = keep.to_owned();
                } else {
                    text.push_str("\nUndo is unavailable because a previous version could not be saved.");
                }
            }
            Ok(())
        })();
        if let Err(error) = save { text.push_str(&format!("\nCould not save the change checkpoint: {error:#}")); }
    }
    Ok(crate::state::Applied { summary: text, edits, keep: saved, ..Default::default() })
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
pub(crate) fn apply_checkpointed_changes(copy: &Path, keep: &Path, found: Vec<Change>) -> Result<crate::state::Applied> {
    apply_changes(copy, found, Some(keep))
}

/// Put back the project files an automatic apply changed, and the copy with them, so the
/// model sees the project as it is again. A file changed since the apply is left alone.
pub fn undo(copy: &Path, keep: &Path) -> Result<String> {
    let checkpoint: Checkpoint = serde_json::from_slice(
        &std::fs::read(keep.join("checkpoint.json")).context("The saved versions are gone")?,
    )?;
    let mut record = base(copy)?;
    let project = record.project.clone();
    let canonical = project.canonicalize()?;
    let (mut restored, mut skipped) = (vec![], vec![]);
    for (rel, before, wrote) in &checkpoint.files {
        let (target, inside) = crate::tools::resolve(&project, rel)?;
        ensure!(inside && target == canonical.join(rel) && !crate::policy::protected(&target), "{rel} is in a protected location");
        let now = target.is_file().then(|| file_hash(&target)).transpose()?;
        let (in_copy, inside_copy) = crate::tools::resolve(copy, rel)?;
        ensure!(inside_copy && in_copy == copy.canonicalize()?.join(rel), "The sandbox path changed: {rel}");
        let now_copy = in_copy.is_file().then(|| file_hash(&in_copy)).transpose()?;
        if &now != wrote || &now_copy != wrote || record.files.get(rel) != wrote.as_ref() {
            skipped.push(rel.clone());
            continue;
        }
        match before {
            Some(name) => {
                let bytes = if let Some(id) = name.strip_prefix("history:") {
                    crate::file_history::History::for_path(&target)?.saved(id)?
                } else {
                    ensure!(name == &crate::tools::hash(rel.as_bytes()), "Invalid saved version for {rel}");
                    std::fs::read(keep.join(name))?
                };
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                crate::file_history::write(&target, &bytes, now.is_some())?;
                if let Some(parent) = in_copy.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&in_copy, &bytes)?;
                record.files.insert(rel.clone(), crate::tools::hash(&bytes));
            }
            None => {
                if target.is_file() {
                    crate::file_history::remove(&target)?;
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
    fn refresh_brings_in_project_updates_and_preserves_unapplied_edits() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("project");
        std::fs::create_dir(&project)?;
        for file in ["clean.rs", "editing.rs", "removed.rs"] { std::fs::write(project.join(file), "initial")?; }
        let copy = ensure(&tmp.path().join("data"), 1, &project)?;
        std::fs::write(copy.join("editing.rs"), "unapplied model edit")?;
        std::fs::write(project.join("editing.rs"), "conflicting project edit")?;
        std::fs::write(project.join("clean.rs"), "latest project version")?;
        std::fs::write(project.join("new.rs"), "new project file")?;
        std::fs::remove_file(project.join("removed.rs"))?;
        let builds = project.join(crate::native::PROJECT_OUTPUTS);
        std::fs::create_dir_all(&builds)?;
        std::fs::write(builds.join("game.exe"), b"fixture")?;
        refresh(&copy, &project)?;
        assert_eq!(std::fs::read_to_string(copy.join("clean.rs"))?, "latest project version");
        assert_eq!(std::fs::read_to_string(copy.join("editing.rs"))?, "unapplied model edit");
        assert!(copy.join("new.rs").is_file());
        assert!(!copy.join("removed.rs").exists());
        assert!(!copy.join(crate::native::PROJECT_OUTPUTS).exists());
        assert_eq!(changes(&copy)?, vec![Change::Modified("editing.rs".into())]);
        Ok(())
    }

    #[test]
    fn applying_project_deletions_requires_the_current_list() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("project");
        std::fs::create_dir(&project)?;
        for name in ["keep.txt", "remove.txt"] { std::fs::write(project.join(name), name)?; }
        let copy = ensure(&tmp.path().join("data"), 1, &project)?;
        let checkpoint = tmp.path().join("checkpoint");
        std::fs::remove_file(copy.join("remove.txt"))?;
        assert!(apply_approved(&copy, None, &checkpoint).is_err());
        let old = deletion_preview(&changes(&copy)?).unwrap();
        std::fs::remove_file(copy.join("keep.txt"))?;
        assert!(apply_approved(&copy, Some(&old), &checkpoint).is_err());
        assert!(project.join("keep.txt").exists());
        std::fs::write(copy.join("keep.txt"), "keep.txt")?;
        let note = apply_approved(&copy, Some(&old), &checkpoint)?;
        assert_eq!(note.edits[0].lines, Some((0, 1)));
        assert!(!project.join("remove.txt").exists());
        Ok(())
    }

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
        let note = apply_checkpointed_changes(&copy, &keep, changes(&copy).unwrap()).unwrap();
        assert!(note.summary.contains("Applied 2"), "{}", note.summary);
        assert!(note.edits.iter().any(|edit| edit.patch.as_deref().unwrap().contains("-old\n+new")));
        assert_eq!(note.edits.len(), 2);
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
    fn only_applied_files_are_listed_and_undo_keeps_newer_sandbox_edits() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("project");
        std::fs::create_dir(&project)?;
        std::fs::write(project.join("changed.txt"), "one\n")?;
        std::fs::write(project.join("conflict.txt"), "initial\n")?;
        let copy = ensure(&tmp.path().join("data"), 1, &project)?;
        std::fs::write(project.join("conflict.txt"), "user edit\n")?;
        std::fs::write(copy.join("conflict.txt"), "sandbox edit\n")?;
        std::fs::write(copy.join("changed.txt"), "two\nthree\n")?;
        let keep = tmp.path().join("checkpoint");
        let applied = apply_approved(&copy, None, &keep)?;
        assert_eq!(applied.edits.len(), 1);
        assert_eq!(applied.edits[0].path, "changed.txt");
        assert_eq!(applied.edits[0].lines, Some((2, 1)));
        assert!(applied.summary.contains("conflict.txt"));
        std::fs::write(copy.join("changed.txt"), "next edit\n")?;
        assert!(undo(&copy, &keep)?.contains("Left alone"));
        assert_eq!(std::fs::read_to_string(copy.join("changed.txt"))?, "next edit\n");
        assert_eq!(std::fs::read_to_string(project.join("changed.txt"))?, "two\nthree\n");
        Ok(())
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
