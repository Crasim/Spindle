//! Project identity, pool-wide sharing control and deletion.

use super::adapter::{SyncState, Want};
use super::core::Kind;
use super::space::Space;
use crate::{projects, state::State};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SHARING: &str = "project-sharing:";
const DELETED: &str = "project-deleted:";

pub(super) fn resolved<'a>(links: &'a BTreeMap<String, String>, mut id: &'a str) -> &'a str {
    let mut visited = BTreeSet::new();
    while let Some(next) = links.get(id) {
        if !visited.insert(id) {
            return visited.into_iter().min().unwrap_or(id);
        }
        id = next;
    }
    id
}

pub(super) fn read_links(state: &mut State, space: &Space) {
    let Some(sync) = state.space.as_mut() else { return };
    for (key, value) in space.items(Kind::Settings) {
        if let Some(from) = key.strip_prefix("project-alias:")
            && let Some(to) = value.as_ref().and_then(|v| v["target"].as_str())
            && projects::valid_sync_id(from) && projects::valid_sync_id(to) && from != to
        {
            sync.project_aliases.insert(from.to_owned(), to.to_owned());
        }
    }
}

pub fn link(state: &mut State, from: &str, to: &str) -> Result<String> {
    let sync = state.space.as_mut().context("Set up sync first")?;
    ensure!(projects::valid_sync_id(from) && projects::valid_sync_id(to), "Use /sync link-projects <duplicate ID> <original ID>");
    let target = resolved(&sync.project_aliases, to).to_owned();
    ensure!(from != target && !sync.project_aliases.contains_key(from), "These projects are already linked");
    for id in [from, target.as_str()] {
        ensure!(sync.projects.values().any(|p| p.id == id), "Unknown local project {id}");
    }
    sync.project_aliases.insert(from.to_owned(), target);
    if !sync.paused { sync.wanted = Some(Want::Now); }
    normalize_bindings(state);
    Ok("Project IDs linked. Chats use the original project folder.".into())
}

/// Adopt saved IDs before minting any new ones. Older Windows imports stored both
/// extended-length and ordinary spellings of a single folder.
pub(crate) fn normalize_bindings(state: &mut State) {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_owned());
    let mut homes = BTreeMap::<String, PathBuf>::new();
    let mut paths = BTreeMap::new();
    if let Some(sync) = &mut state.space {
        for (old, settings) in std::mem::take(&mut sync.projects) {
            let path = canonical(&old);
            let entry = sync.projects.entry(path).or_insert(settings.clone());
            if entry.id.is_empty() { *entry = settings; }
        }
        for (path, settings) in &mut sync.projects {
            if projects::existing(path).ok().flatten().is_none() { continue; }
            let preferred = projects::valid_sync_id(&settings.id).then_some(settings.id.as_str());
            if let Ok(id) = projects::ensure_sync_identity(path, preferred) {
                let target = resolved(&sync.project_aliases, &id);
                if target != id && projects::replace_sync_identity(path, &id, target).is_err() {
                    continue;
                }
                settings.id = target.to_owned();
                let home = homes.entry(settings.id.clone()).or_insert_with(|| path.clone());
                if projects::prefer_folder(path, home) { *home = path.clone(); }
            }
        }
        for (path, settings) in &sync.projects {
            if let Some(home) = homes.get(&settings.id) {
                paths.insert(path.clone(), home.clone());
            }
        }
    }
    let resolve = |p: &mut PathBuf| {
        let path = canonical(p);
        *p = paths.get(&path).cloned().unwrap_or(path);
    };
    for path in state.thread.project.iter_mut()
        .chain(state.chats.values_mut().filter_map(|c| c.thread.project.as_mut()))
        .chain(state.memories.iter_mut().filter_map(|m| m.project.as_mut()))
    {
        resolve(path);
    }
}

pub(crate) fn project_id(sync: &mut SyncState, path: &Path) -> Result<String> {
    let path = path.canonicalize()?;
    let preferred = sync.projects.get(&path).map(|p| p.id.as_str());
    let id = projects::ensure_sync_identity(&path, preferred)?;
    sync.projects.entry(path).or_default().id = id.clone();
    Ok(id)
}

pub(super) fn find_copy(root: &Path, id: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let path = entry.path();
            let project = projects::existing(&path).ok()??;
            (projects::sync_identity(&project.path)
                .ok()
                .flatten()
                .as_deref()
                == Some(id))
            .then_some(project.path)
        })
}

pub(super) fn make_project(root: &Path, id: &str, value: &Value) -> Option<projects::Project> {
    if !projects::valid_sync_id(id) {
        return None;
    }
    std::fs::create_dir_all(root).ok()?;
    if let Some(path) = find_copy(root, id) {
        let p = projects::existing(&path).ok()??;
        projects::library(Some(&p)).ok()?;
        return Some(p);
    }
    let name = value["name"].as_str().unwrap_or("Project").trim();
    let safe = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches(['.', ' ']);
    let safe = if safe.is_empty() { "Project" } else { safe };
    let exact = root.join(safe);
    let prior_copy = root.join(format!("{safe} ({})", &id[id.len() - 6..]));
    if prior_copy.exists() {
        // Old imports used a six-character suffix before they stored a portable ID.
        let p = projects::existing(&prior_copy).ok()??;
        if p.name != name || projects::sync_identity(&prior_copy).ok()?.is_some() {
            return None;
        }
        if projects::ensure_sync_identity(&prior_copy, Some(id))
            .ok()?
            .as_str()
            != id
        {
            return None;
        }
        projects::library(Some(&p)).ok()?;
        return Some(p);
    }
    let path = if exact.exists() { prior_copy } else { exact };
    let p = projects::create_synced(&path, name, value["description"].as_str().unwrap_or(""), id)
        .ok()?;
    let p = match value["instructions"].as_str().filter(|s| !s.is_empty()) {
        Some(s) => projects::update(&p.path, &p.name, &p.description, s).unwrap_or(p),
        None => p,
    };
    projects::library(Some(&p)).ok()?;
    Some(p)
}

pub fn set_paused(state: &mut State, path: &Path, paused: bool) -> Result<()> {
    let project = projects::existing(path)?.context("Choose an existing Spindle project")?;
    let sync = state.space.as_mut().context("Set up sync first")?;
    project_id(sync, &project.path)?;
    let p = sync.projects.get_mut(&project.path).unwrap();
    p.paused = paused;
    p.pending_pause = Some(paused);
    sync.wanted = Some(Want::Now);
    Ok(())
}

pub fn request_delete(state: &mut State, path: &Path, files: bool) -> Result<()> {
    let project = projects::existing(path)?.context("Choose an existing Spindle project")?;
    let sync = state.space.as_mut().context("Set up sync first")?;
    let id = project_id(sync, &project.path)?;
    let pending = sync.project_deletions.entry(id).or_default();
    *pending |= files;
    sync.wanted = Some(Want::Now);
    Ok(())
}

#[derive(Default)]
pub(super) struct Policies {
    deleted: BTreeMap<String, bool>,
    paused: BTreeSet<String>,
}

impl Policies {
    pub(super) fn read(space: &Space) -> Self {
        let mut policies = Self::default();
        for (id, value) in space.items(Kind::Settings) {
            if let Some(project) = id
                .strip_prefix(DELETED)
                .filter(|p| projects::valid_sync_id(p))
            {
                if value.is_some() {
                    // An explicit file deletion is sticky across concurrent delete requests.
                    let files = space
                        .item(Kind::Settings, &id)
                        .iter()
                        .any(|(_, _, b)| !b.deleted && b.value["files"] == true);
                    policies.deleted.insert(project.to_owned(), files);
                }
            } else if let Some(project) = id.strip_prefix(SHARING)
                && value.is_some_and(|v| v["paused"] == true)
            {
                policies.paused.insert(project.to_owned());
            }
        }
        policies
    }

    pub(super) fn deleted(&self, id: &str) -> bool {
        self.deleted.contains_key(id)
    }

    pub(super) fn paused(&self, id: &str) -> bool {
        self.paused.contains(id)
    }

    pub(super) fn blocked(&self, id: &str) -> bool {
        self.deleted(id) || self.paused(id)
    }
}

pub(super) fn publish(sync: &mut SyncState, space: &mut Space) -> Result<usize> {
    let mut count = 0;
    for (id, files) in sync.project_deletions.clone() {
        let key = format!("{DELETED}{id}");
        let files = files
            || space
                .value(Kind::Settings, &key)
                .is_some_and(|v| v["files"] == true);
        count += usize::from(
            space
                .put(Kind::Settings, &key, json!({"files": files}))?
                .is_some(),
        );
        count += usize::from(
            space.value(Kind::Project, &id).is_some()
                && space.delete(Kind::Project, &id)?.is_some(),
        );
        for kind in [Kind::Chat, Kind::Memory] {
            for (gid, v) in space.items(kind) {
                if v.as_ref().and_then(|v| v["project"].as_str()) == Some(id.as_str()) {
                    count += usize::from(space.delete(kind, &gid)?.is_some());
                }
            }
        }
        sync.project_deletions.remove(&id);
    }
    for p in sync.projects.values_mut() {
        if let Some(paused) = p.pending_pause {
            count += usize::from(
                space
                    .put(
                        Kind::Settings,
                        &format!("{SHARING}{}", p.id),
                        json!({"paused":paused}),
                    )?
                    .is_some(),
            );
            p.pending_pause = None;
        }
    }
    Ok(count)
}

/// Deletion records carry identities only. File removal is scoped to a matching local project.
pub(super) fn apply(
    state: &mut State,
    sync: &mut SyncState,
    policies: &Policies,
    projects_dir: Option<&Path>,
) -> usize {
    let mut count = 0;
    for (id, &files) in &policies.deleted {
        let marker = format!("{DELETED}{id}");
        let applied = sync
            .seen
            .get(&marker)
            .is_some_and(|v| v == if files { "files" } else { "metadata" });
        let mut paths: BTreeSet<PathBuf> = sync
            .projects
            .iter()
            .filter(|(_, p)| &p.id == id)
            .map(|(p, _)| p.clone())
            .collect();
        if !applied && let Some(path) = projects_dir.and_then(|root| find_copy(root, id)) {
            paths.insert(path);
        }
        sync.remote_projects.remove(id);
        let mut complete = true;
        for path in paths {
            match remove_local_project(state, sync, &path, id, files) {
                Ok(removed) => count += usize::from(removed),
                Err(error) => {
                    complete = false;
                    let message = format!("Project deletion pending for {}: {error:#}", path.display());
                    if !sync.activity.contains(&message) {
                        sync.log(message);
                    }
                }
            }
        }
        if complete {
            sync.seen.insert(marker, if files { "files" } else { "metadata" }.into());
        }
    }
    for p in sync.projects.values_mut() {
        p.paused = p.pending_pause.unwrap_or_else(|| policies.paused(&p.id));
    }
    count
}

fn remove_local_project(state: &mut State, sync: &mut SyncState, path: &Path, id: &str, files: bool) -> Result<bool> {
    let exists = path.try_exists()?;
    if exists && projects::existing(path)?.is_some() {
        let identity = projects::sync_identity(path)?;
        if identity.as_deref().is_some_and(|local| local != id) {
            // A project now occupying the old path has its own identity.
            sync.projects.remove(path);
            return Ok(false);
        }
        if files {
            ensure!(
                identity.as_deref() == Some(id),
                "Cannot verify the identity of the project folder {}",
                path.display()
            );
            let canonical = path.canonicalize()?;
            ensure!(
                canonical == path && canonical.parent().is_some(),
                "Project folder changed before deletion"
            );
            std::fs::remove_dir_all(&canonical).with_context(|| {
                format!("Couldn't delete project files in {}", canonical.display())
            })?;
        }
        projects::forget(path)?;
    } else {
        ensure!(
            !exists || !files,
            "Cannot verify the identity of the project folder {}",
            path.display()
        );
        projects::forget(path)?;
    }
    let ids: Vec<u64> = std::iter::once(state.chat_id)
        .chain(state.chats.keys().copied())
        .filter(|chat| {
            state
                .thread_data(*chat)
                .is_some_and(|(t, _)| t.project.as_deref() == Some(path))
        })
        .collect();
    for chat in ids {
        if let Some(pair) = state.sync.as_mut() {
            pair.delete(chat);
        }
        state.forget_thread(chat)?;
        state.requests.retain(|r| r.chat != chat);
        state
            .grants
            .retain(|g| g.holder != crate::policy::Holder::Conversation(chat));
        sync.chats.remove(&chat);
        sync.branches.remove(&chat);
    }
    state
        .grants
        .retain(|g| g.holder != crate::policy::Holder::Project(path.to_owned()));
    state
        .memories
        .retain(|m| m.project.as_deref() != Some(path));
    sync.projects.remove(path);
    sync.base.remove(&format!("project:{id}"));
    Ok(true)
}
