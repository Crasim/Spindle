use crate::ollama::Message;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// What the project is for, shown on its page.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Added to the system prompt of every chat in the project.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u8,
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    instructions: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub title: String,
    pub messages: Vec<Message>,
}

impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        validate_title(&self.title)?;
        ensure!(
            !self.messages.is_empty() && self.messages.len() % 2 == 0,
            "Share a completed conversation"
        );
        for (i, m) in self.messages.iter().enumerate() {
            ensure!(
                m.role == if i % 2 == 0 { "user" } else { "assistant" },
                "Unexpected message role"
            );
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 2 * 1024 * 1024,
            "Conversation exceeds the 2 MB sharing limit"
        );
        Ok(())
    }
}

fn validate_title(s: &str) -> Result<()> {
    ensure!(
        !s.trim().is_empty() && s.chars().count() <= 100 && !s.chars().any(char::is_control),
        "Enter a name between 1 and 100 characters"
    );
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path, max: u64) -> Result<T> {
    let mut buf = vec![];
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "Expected a regular project file"
    );
    File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    ensure!(
        buf.len() as u64 <= max,
        "Project file exceeds the size limit"
    );
    Ok(serde_json::from_slice(&buf)?)
}

pub(crate) fn shared_dir(root: &Path, create: bool) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let expected = root.join(".spindle");
    if create && !expected.exists() {
        match fs::create_dir(&expected) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
    }
    ensure!(
        expected.canonicalize()? == expected,
        "Shared project metadata must reside directly inside the project folder"
    );
    Ok(expected)
}

fn write_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut f =
        tempfile::NamedTempFile::new_in(path.parent().context("Missing project directory")?)?;
    serde_json::to_writer(&mut f, value)?;
    f.flush()?;
    f.as_file().sync_all()?;
    f.persist_noclobber(path)?;
    Ok(())
}

pub fn existing(root: &Path) -> Result<Option<Project>> {
    match fs::symlink_metadata(root.join(".spindle/project.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => (),
    }
    let dir = shared_dir(root, false)?;
    let manifest: Manifest = read_json(&dir.join("project.json"), 16384)?;
    ensure!(manifest.version == 1, "Unsupported project format");
    validate_title(&manifest.name)?;
    Ok(Some(Project {
        name: manifest.name,
        path: root.canonicalize()?,
        description: manifest.description,
        instructions: manifest.instructions,
    }))
}

const MAX_NOTE: usize = 16 * 1024;

/// Create a project in a new or empty-of-Spindle folder with its description.
pub fn create_described(root: &Path, name: &str, description: &str) -> Result<Project> {
    fs::create_dir_all(root).context("Create the project folder")?;
    create(root, name)?;
    update(root, name, description, "")
}

pub(crate) fn create_synced(
    root: &Path,
    name: &str,
    description: &str,
    id: &str,
) -> Result<Project> {
    fs::create_dir(root).context("Create the synced project folder")?;
    create_with_identity(root, name, Some(id))?;
    update(root, name, description, "")
}

/// Rewrite the project's name, description and instructions.
pub fn update(root: &Path, name: &str, description: &str, instructions: &str) -> Result<Project> {
    validate_title(name)?;
    ensure!(
        description.len() <= MAX_NOTE && instructions.len() <= MAX_NOTE,
        "Keep the description and instructions under 16 KB each"
    );
    existing(root)?.context("This folder is not a Spindle project")?;
    let dir = shared_dir(root, false)?;
    let mut f = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(
        &mut f,
        &Manifest {
            version: 1,
            name: name.trim().into(),
            description: description.trim().into(),
            instructions: instructions.trim().into(),
        },
    )?;
    f.flush()?;
    f.as_file().sync_all()?;
    f.persist(dir.join("project.json"))?;
    existing(root)?.context("Project update failed")
}

/// Remove a project from Spindle: drop it from the library and delete its manifest.
/// Every other file in the folder, including generated artifacts, is left in place.
pub fn forget(root: &Path) -> Result<()> {
    let dir = library_dir()?;
    fs::create_dir_all(&dir)?;
    let lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("projects.lock"))?;
    crate::state::lock_file(&lock)?;
    let path = dir.join("projects.json");
    if path.try_exists()? {
        let mut entries: Vec<Project> = read_json(&path, 2 * 1024 * 1024)?;
        let canonical = root.canonicalize().unwrap_or_else(|_| root.into());
        entries.retain(|p| p.path != canonical && p.path != root);
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer(&mut tmp, &entries)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        tmp.persist(path)?;
    }
    drop(lock);
    if existing(root)?.is_some() {
        let metadata = shared_dir(root, false)?;
        fs::remove_file(metadata.join("project.json"))?;
        let identity = metadata.join("sync-id.json");
        if let Err(error) = fs::remove_file(identity)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
    }
    Ok(())
}

pub fn create(root: &Path, name: &str) -> Result<Project> {
    create_with_identity(root, name, None)
}

fn create_with_identity(root: &Path, name: &str, id: Option<&str>) -> Result<Project> {
    validate_title(name)?;
    let dir = shared_dir(root, true)?;
    write_new(
        &dir.join("project.json"),
        &Manifest {
            version: 1,
            name: name.trim().into(),
            description: String::new(),
            instructions: String::new(),
        },
    )
    .context("Create project metadata in this folder")?;
    ensure_sync_identity(root, id)?;
    existing(root)?.context("Project creation failed")
}

pub(crate) fn valid_sync_id(id: &str) -> bool {
    id.strip_prefix("p-")
        .is_some_and(|s| (16..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The identity travels with the folder, including copies made before pairing another device.
pub(crate) fn sync_identity(root: &Path) -> Result<Option<String>> {
    let path = shared_dir(root, false)?.join("sync-id.json");
    let id: String = match read_json(&path, 128) {
        Ok(id) => id,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    ensure!(valid_sync_id(&id), "Invalid project sync identity");
    Ok(Some(id))
}

pub(crate) fn ensure_sync_identity(root: &Path, preferred: Option<&str>) -> Result<String> {
    if let Some(id) = sync_identity(root)? {
        return Ok(id);
    }
    let id = match preferred {
        Some(id) => id.to_owned(),
        None => {
            let mut uuid = crate::vault::random::<16>()?;
            uuid[6] = (uuid[6] & 0x0f) | 0x40;
            uuid[8] = (uuid[8] & 0x3f) | 0x80;
            format!("p-{}", crate::sync::core::hex(&uuid))
        }
    };
    ensure!(valid_sync_id(&id), "Invalid project sync identity");
    let path = shared_dir(root, false)?.join("sync-id.json");
    match write_new(&path, &id) {
        Ok(()) => Ok(id),
        Err(error) => sync_identity(root)?.ok_or(error),
    }
}

pub(crate) fn replace_sync_identity(root: &Path, previous: &str, target: &str) -> Result<()> {
    ensure!(valid_sync_id(target) && sync_identity(root)?.as_deref() == Some(previous), "Project identity changed");
    let dir = shared_dir(root, false)?;
    let marker = dir.join("linked-from.json");
    if !marker.try_exists()? {
        write_new(&marker, &previous)?;
    }
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(&mut tmp, target)?;
    tmp.flush()?;
    tmp.as_file().sync_all()?;
    tmp.persist(dir.join("sync-id.json"))?;
    Ok(())
}

pub(crate) fn prefer_folder(candidate: &Path, current: &Path) -> bool {
    let linked = |path: &Path| path.join(".spindle/linked-from.json").is_file();
    (linked(candidate), candidate) < (linked(current), current)
}

pub fn list_snapshots(root: &Path) -> Result<Vec<String>> {
    let mut names = vec![];
    for entry in fs::read_dir(shared_dir(root, false)?)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("chat-") && name.ends_with(".json") && entry.file_type()?.is_file() {
            names.push(name);
        }
    }
    names.sort_by(|a, b| b.cmp(a));
    Ok(names)
}

pub fn snapshot(root: &Path, name: &str) -> Result<Snapshot> {
    ensure!(
        Path::new(name).file_name().is_some_and(|f| f == name)
            && name.starts_with("chat-")
            && name.ends_with(".json")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
        "Invalid shared conversation ID"
    );
    let dir = shared_dir(root, false)?;
    let path = dir.join(name);
    ensure!(
        path.canonicalize()? == path,
        "Shared conversation points outside its project"
    );
    let s: Snapshot = read_json(&path, 2 * 1024 * 1024)?;
    s.validate()?;
    Ok(s)
}

#[cfg(not(test))]
fn library_dir() -> Result<PathBuf> {
    crate::state::workspace_storage()
}

#[cfg(test)]
fn library_dir() -> Result<PathBuf> {
    static LIBRARY: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    Ok(LIBRARY
        .get_or_init(|| tempfile::tempdir().expect("Create an isolated test project registry"))
        .path()
        .to_owned())
}

pub fn library(add: Option<&Project>) -> Result<Vec<Project>> {
    let dir = library_dir()?;
    fs::create_dir_all(&dir)?;
    let lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("projects.lock"))?;
    crate::state::lock_file(&lock)?;
    let path = dir.join("projects.json");
    let mut entries: Vec<Project> = if path.try_exists()? {
        read_json(&path, 2 * 1024 * 1024)?
    } else {
        vec![]
    };
    let before = entries.len();
    // An unavailable LAN share or removable drive can come back. Stale paths under the
    // system temp folder (including old test projects) are safe to remove from the registry.
    if let Ok(temp) = std::env::temp_dir().canonicalize() {
        entries.retain(|p| !p.path.starts_with(&temp) || !matches!(p.path.try_exists(), Ok(false)));
    }
    let pruned = entries.len() != before;
    if let Some(project) = add {
        entries.retain(|p| p.path != project.path);
        entries.insert(0, project.clone());
    }
    if add.is_some() || pruned {
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer(&mut tmp, &entries)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        tmp.persist(path)?;
    }
    // Linked legacy folders can both contain files. List their common identity once,
    // using the same preferred folder as the chat bindings.
    let mut homes = std::collections::BTreeMap::new();
    let identities: Vec<_> = entries.iter().map(|p| sync_identity(&p.path).ok().flatten()).collect();
    for (p, id) in entries.iter().zip(&identities) {
        if let Some(id) = id {
            let home = homes.entry(id).or_insert(&p.path);
            if prefer_folder(&p.path, home) { *home = &p.path; }
        }
    }
    let keep: Vec<_> = entries.iter().zip(&identities)
        .map(|(p,id)| id.as_ref().is_none_or(|id| homes[id] == &p.path)).collect();
    Ok(entries.into_iter().zip(keep).filter_map(|(p,keep)| keep.then_some(p)).collect())
}

pub fn display_path(path: &Path) -> String {
    let s = path.display().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        s.trim_start_matches(r"\\?\").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_snapshots_still_validate_roles_and_paths() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "Team review").unwrap();
        let snapshot = Snapshot {
            title: "Review draft".into(),
            messages: vec![
                Message::new("user", "check"),
                Message::new("assistant", "feedback"),
            ],
        };
        write_new(&dir.path().join(".spindle/chat-legacy.json"), &snapshot).unwrap();
        assert_eq!(list_snapshots(dir.path()).unwrap().len(), 1);
        assert_eq!(
            super::snapshot(dir.path(), "chat-legacy.json")
                .unwrap()
                .title,
            "Review draft"
        );
        assert!(super::snapshot(dir.path(), "../session.json").is_err());
    }
}
