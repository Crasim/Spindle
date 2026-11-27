//! Previous file contents, kept in Spindle's encrypted local storage.

use anyhow::{Context, Result, ensure};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_VERSION: u64 = 32 * 1024 * 1024;
const MAX_DIFF: usize = 1024 * 1024;
static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) struct History {
    pub path: PathBuf,
    folder: PathBuf,
}

impl History {
    pub(crate) fn for_path(path: &Path) -> Result<Self> {
        Self::in_store(path, &crate::runtime::data_dir().join("file-history"))
    }

    fn in_store(path: &Path, store: &Path) -> Result<Self> {
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                parent
                    .canonicalize()?
                    .join(path.file_name().context("Choose a file")?)
            }
            Err(e) => return Err(e.into()),
        };
        let key = crate::sync::core::hex(digest(path.as_os_str().as_encoded_bytes()).as_ref());
        Ok(Self {
            path,
            folder: store.join(key),
        })
    }

    pub(crate) fn versions(&self) -> Result<Vec<String>> {
        let entries = match std::fs::read_dir(&self.folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut versions = vec![];
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".rev"))
                && valid_id(id)
                && entry.file_type()?.is_file()
            {
                versions.push(id.to_owned());
            }
        }
        versions.sort_unstable_by(|a, b| b.cmp(a));
        Ok(versions)
    }

    fn saved(&self, id: &str) -> Result<Vec<u8>> {
        ensure!(
            valid_id(id),
            "Choose a revision ID from this file's history"
        );
        let bytes = std::fs::read(self.folder.join(format!("{id}.rev")))
            .context("This saved version is unavailable")?;
        let saved = crate::vault::local(&bytes, false)?;
        let saved = saved
            .strip_prefix(self.binding(id).as_slice())
            .context("This saved version belongs to a different file or revision")?;
        Ok(saved.to_vec())
    }

    fn binding(&self, id: &str) -> Vec<u8> {
        format!(
            "Spindle file history 1\0{}\0{id}\0",
            self.folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        )
        .into_bytes()
    }

    fn snapshot(&self, before: &[u8]) -> Result<()> {
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let suffix = crate::sync::core::hex(&crate::vault::random::<8>()?);
        let id = format!("{time:016}-{suffix}");
        let mut record = self.binding(&id);
        record.extend_from_slice(before);
        let sealed = crate::vault::local(&record, true)?;
        std::fs::create_dir_all(&self.folder)?;
        crate::tools::write_generated(&self.folder.join(format!("{id}.rev")), &sealed, false)
    }

    pub(crate) fn compare(&self, revision: &str) -> Result<Comparison> {
        let saved = self.saved(revision)?;
        let current = read_current(&self.path)?;
        let diff = text_comparison(&self.path, &saved, current.as_deref());
        Ok(Comparison {
            diff,
            matches_current: current.as_deref() == Some(saved.as_slice()),
            current: current.as_deref().map(digest),
        })
    }

    pub(crate) fn restore(&self, revision: &str, view: &Comparison) -> Result<()> {
        let _guard = WRITES
            .lock()
            .map_err(|_| anyhow::anyhow!("File history is unavailable"))?;
        ensure!(
            !crate::policy::protected(&self.path),
            "This location is protected"
        );
        let saved = self.saved(revision)?;
        let current_path = Self::for_path(&self.path)?.path;
        ensure!(
            current_path == self.path,
            "The file path changed. Reopen its history."
        );
        let before = read_current(&self.path)?;
        ensure!(
            before.as_deref().map(digest).as_ref().map(|d| d.as_ref())
                == view.current.as_ref().map(|d| d.as_ref()),
            "The file changed since this comparison. Refresh it before restoring."
        );
        self.write(&saved, before.as_deref(), before.is_some())?;
        Ok(())
    }

    fn write(&self, bytes: &[u8], before: Option<&[u8]>, replace: bool) -> Result<bool> {
        if before == Some(bytes) {
            return Ok(false);
        }
        if let Some(before) = before {
            self.snapshot(before)?;
        }
        crate::tools::write_generated(&self.path, bytes, replace)?;
        Ok(before.is_some())
    }
}

pub(crate) struct Comparison {
    pub diff: Option<String>,
    pub matches_current: bool,
    current: Option<ring::digest::Digest>,
}

impl Comparison {
    pub(crate) fn description(&self) -> &str {
        match self.diff.as_deref() {
            Some("") if self.matches_current => {
                "The saved version and current file have the same contents."
            }
            Some("") => "The current file is missing. The saved version is empty.",
            Some(diff) => diff,
            None => {
                "These versions can be restored, but a text diff is unavailable for binary or large files."
            }
        }
    }
}

fn digest(bytes: &[u8]) -> ring::digest::Digest {
    ring::digest::digest(&ring::digest::SHA256, bytes)
}

fn valid_id(id: &str) -> bool {
    id.split_once('-').is_some_and(|(time, suffix)| {
        time.len() == 16
            && time.bytes().all(|b| b.is_ascii_digit())
            && suffix.len() == 16
            && suffix.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

pub(crate) fn label(id: &str) -> String {
    id.split_once('-')
        .and_then(|(time, _)| time.parse().ok())
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || id.to_owned(),
            |date| {
                let suffix = id.split_once('-').unwrap().1;
                format!(
                    "{} · {}",
                    date.with_timezone(&chrono::Local)
                        .format("%b %d, %Y %H:%M:%S"),
                    &suffix[..4]
                )
            },
        )
}

fn read_current(path: &Path) -> Result<Option<Vec<u8>>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        meta.is_file() && meta.len() <= MAX_VERSION,
        "File history supports regular files up to 32 MiB"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_VERSION + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_VERSION,
        "File exceeds the 32 MiB history limit"
    );
    Ok(Some(bytes))
}

pub(crate) fn write(path: &Path, bytes: &[u8], replace: bool) -> Result<bool> {
    let _guard = WRITES
        .lock()
        .map_err(|_| anyhow::anyhow!("File history is unavailable"))?;
    if !replace {
        crate::tools::write_generated(path, bytes, false)?;
        return Ok(false);
    }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() > MAX_VERSION) {
        crate::tools::write_generated(path, bytes, true)?;
        return Ok(false);
    }
    let before = read_current(path)?;
    let history = History::for_path(path)?;
    history.write(bytes, before.as_deref(), before.is_some())
}

pub(crate) fn remove(path: &Path) -> Result<()> {
    let _guard = WRITES
        .lock()
        .map_err(|_| anyhow::anyhow!("File history is unavailable"))?;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_VERSION)
        && let Some(before) = read_current(path)?
    {
        History::for_path(path)?.snapshot(&before)?;
    }
    std::fs::remove_file(path)?;
    Ok(())
}

fn text_comparison(path: &Path, saved: &[u8], current: Option<&[u8]>) -> Option<String> {
    let current = current.unwrap_or_default();
    if saved == current {
        return Some(String::new());
    }
    if saved.len().max(current.len()) > MAX_DIFF || saved.contains(&0) || current.contains(&0) {
        return None;
    }
    let (Ok(old), Ok(new)) = (std::str::from_utf8(saved), std::str::from_utf8(current)) else {
        return None;
    };
    Some(crate::diff::patch(
        &path.file_name().unwrap_or_default().to_string_lossy(),
        old,
        new,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Call, classify, hash, run};

    #[test]
    fn model_edits_record_local_revisions_and_restore_preserves_the_current_file() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let root = folder.path().canonicalize()?;
        let create = Call::Create {
            path: "game.html".into(),
            content: "<p>first</p>\n".into(),
        };
        let (_, path) = classify(&root, &create, true)?;
        run(&root, &create, &path, false)?;
        let edit = Call::Patch {
            path: "game.html".into(),
            find: "first".into(),
            replace: "second".into(),
            hash: hash(b"<p>first</p>\n"),
        };
        assert!(run(&root, &edit, &path, false)?.contains("previous version saved"));
        let history = History::for_path(&path)?;
        let versions = history.versions()?;
        assert_eq!(versions.len(), 1);
        let first = &versions[0];
        let sealed = std::fs::read(history.folder.join(format!("{first}.rev")))?;
        assert!(
            !sealed
                .windows(b"<p>first</p>".len())
                .any(|part| part == b"<p>first</p>")
        );
        let response = format!(
            r#"{{"name":"file_history","args":{{"path":"game.html","revision":"{first}","applied":true,"reason":"verify edit"}}}}"#
        );
        let (_, parsed) = crate::tools::extract(&response);
        let lookup = parsed
            .expect("model tool call")
            .expect("valid arguments")
            .call;
        let (risk, target) = classify(&root, &lookup, true)?;
        assert_eq!(risk, crate::policy::Risk::Read);
        assert!(run(&root, &lookup, &target, false)?.contains("-<p>first</p>\n+<p>second</p>"));
        let compared = history.compare(first)?;
        std::fs::write(&path, "<p>outside edit</p>\n")?;
        assert!(history.restore(first, &compared).is_err());
        history.restore(first, &history.compare(first)?)?;
        assert_eq!(std::fs::read_to_string(&path)?, "<p>first</p>\n");
        let second = history
            .versions()?
            .into_iter()
            .find(|id| id != first)
            .unwrap();
        history.restore(&second, &history.compare(&second)?)?;
        assert_eq!(std::fs::read_to_string(&path)?, "<p>outside edit</p>\n");
        let outside = Call::FileHistory {
            path: "../outside.html".into(),
            revision: None,
            applied: false,
        };
        assert!(classify(&root, &outside, true).is_err());
        Ok(())
    }

    #[test]
    fn binary_and_empty_revisions_restore_exact_bytes() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("file.bin");
        let history = History::in_store(&path, &folder.path().join("history"))?;
        std::fs::write(&path, [0, 255])?;
        history.write(&[], Some(&[0, 255]), true)?;
        let version = history.versions()?.pop().unwrap();
        let comparison = history.compare(&version)?;
        assert!(comparison.diff.is_none());
        history.restore(&version, &comparison)?;
        assert_eq!(std::fs::read(&path)?, [0, 255]);
        let empty = history
            .versions()?
            .into_iter()
            .find(|id| id != &version)
            .unwrap();
        std::fs::remove_file(&path)?;
        let comparison = history.compare(&empty)?;
        assert!(!comparison.matches_current);
        history.restore(&empty, &comparison)?;
        assert!(std::fs::read(&path)?.is_empty());
        Ok(())
    }

    #[test]
    fn project_restore_updates_only_a_clean_sandbox_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let data = dir.path().join("data");
        let project = dir.path().join("project");
        std::fs::create_dir(&project)?;
        let path = project.join("file.txt");
        std::fs::write(&path, "base")?;
        let clean = crate::workcopy::ensure(&data, 1, &project)?;
        let dirty = crate::workcopy::ensure(&data, 2, &project)?;
        std::fs::write(dirty.join("file.txt"), "unapplied edit")?;
        std::fs::write(&path, "restored")?;
        for chat in [1, 2] {
            crate::workcopy::refresh_restored_file(&data, chat, &project, &path.canonicalize()?)?;
        }
        assert_eq!(std::fs::read_to_string(clean.join("file.txt"))?, "restored");
        assert!(crate::workcopy::changes(&clean)?.is_empty());
        assert_eq!(
            std::fs::read_to_string(dirty.join("file.txt"))?,
            "unapplied edit"
        );
        assert_eq!(crate::workcopy::changes(&dirty)?.len(), 1);
        Ok(())
    }
}
