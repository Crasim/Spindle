pub mod adapter;
pub mod remote;
pub mod placement;
pub mod chat;
pub mod core;
pub mod hosted;
pub mod live;
mod provider_sync;
pub(crate) mod project_sync;
pub mod pair;
pub mod peer;
pub(super) mod routes;
pub mod space;
pub mod transport;

use crate::{
    projects,
    state::{Conversation, State},
    vault,
};
use anyhow::{Context, Result, ensure};
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::Path,
};

const LIMIT: u64 = 16 * 1024 * 1024;
const CHECK: &[u8] = b"Spindle project sync v1";

#[derive(Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub enabled: bool,
    device: [u8; 16],
    key: [u8; 32],
    salt: [u8; 16],
    heads: BTreeMap<u64, String>,
    fingerprints: BTreeMap<u64, String>,
    seen: BTreeSet<String>,
    #[serde(default)]
    pending_deletes: BTreeSet<String>,
    pub remote_work: BTreeMap<u64, String>,
}

impl Pairing {
    pub fn delete(&mut self, id: u64) {
        if let Some(head) = self.heads.remove(&id) {
            self.pending_deletes.insert(head);
        }
        self.fingerprints.remove(&id);
        self.remote_work.remove(&id);
    }

    pub fn tracks(&self, id: u64) -> bool {
        self.heads.contains_key(&id)
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u8,
    salt: [u8; 16],
    check: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    parent: Option<String>,
    conversation: Conversation,
    pending: Option<String>,
    provider_base: Option<String>,
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.is_file() && path.canonicalize()? == path,
        "Invalid project sync file"
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Project sync record exceeds its size limit"
    );
    Ok(bytes)
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("Missing sync folder")?)?;
    f.write_all(data)?;
    f.as_file().sync_all()?;
    f.persist_noclobber(path)?;
    Ok(())
}

pub fn pair(root: &Path, pass: &str) -> Result<Pairing> {
    ensure!(
        projects::existing(root)?.is_some(),
        "Open or create a project first"
    );
    ensure!(
        pass.chars().count() >= 12,
        "Use a project passphrase of at least 12 characters"
    );
    let dir = projects::shared_dir(root, false)?;
    let path = dir.join("sync-v1.json");
    if !path.try_exists()? {
        let salt = vault::random()?;
        let header = Header {
            version: 1,
            salt,
            check: vault::seal(&vault::derive(pass, &salt), CHECK, CHECK.to_vec())?,
        };
        if let Err(e) = write(&path, &serde_json::to_vec(&header)?) {
            if !path.try_exists()? {
                return Err(e);
            }
        }
    }
    let header: Header = serde_json::from_slice(&read(&path, 4096)?)?;
    ensure!(header.version == 1, "Unsupported sync format");
    let key = vault::derive(pass, &header.salt);
    ensure!(
        vault::open(&key, CHECK, &header.check)? == CHECK,
        "Invalid project key"
    );
    Ok(Pairing {
        enabled: true,
        device: vault::random()?,
        key,
        salt: header.salt,
        heads: Default::default(),
        fingerprints: Default::default(),
        seen: Default::default(),
        pending_deletes: Default::default(),
        remote_work: Default::default(),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn revision_id(key: &[u8; 32], bytes: &[u8]) -> String {
    let domain = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        b"Spindle revision identifiers v1",
    );
    hex(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, domain.as_ref()), bytes).as_ref())
}

fn valid_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn fingerprint(c: &Conversation, pending: &Option<String>) -> Result<String> {
    Ok(hex(digest::digest(
        &digest::SHA256,
        &serde_json::to_vec(&(c, pending))?,
    )
    .as_ref()))
}

pub fn refresh(root: &Path, state: &mut State) -> Result<usize> {
    let mut pair = state.sync.take().context("Unlock project sync first")?;
    let result = reconcile(root, state, &mut pair);
    state.sync = Some(pair);
    result
}

fn publish_record(dir: &Path, pair: &Pairing, record: &Record) -> Result<String> {
    let bytes = serde_json::to_vec(&record)?;
    ensure!(
        bytes.len() as u64 + 28 <= LIMIT,
        "Thread exceeds the 16 MiB sync limit"
    );
    let revision = revision_id(&pair.key, &bytes);
    let path = dir.join(format!("thread-{revision}.spindle"));
    if !path.try_exists()? {
        let sealed = vault::seal(&pair.key, revision.as_bytes(), bytes.clone())?;
        if let Err(e) = write(&path, &sealed) {
            if !path.try_exists()? {
                return Err(e);
            }
        }
    }
    ensure!(
        vault::open(&pair.key, revision.as_bytes(), &read(&path, LIMIT)?)? == bytes,
        "Conflicting revision on disk"
    );
    Ok(revision)
}

fn reconcile(root: &Path, state: &mut State, pair: &mut Pairing) -> Result<usize> {
    let dir = projects::shared_dir(root, false)?;
    let header: Header = serde_json::from_slice(&read(&dir.join("sync-v1.json"), 4096)?)?;
    ensure!(
        header.salt == pair.salt
            && header.version == 1
            && vault::open(&pair.key, CHECK, &header.check)? == CHECK,
        "Project sync identity changed"
    );
    let ids: Vec<_> = state.chats.keys().copied().chain([state.chat_id]).collect();
    let mut changes = 0;
    for head in pair.pending_deletes.clone() {
        let tombstone = Record {
            deleted: true,
            origin: None,
            parent: Some(head.clone()),
            conversation: Conversation::default(),
            pending: None,
            provider_base: None,
        };
        let revision = publish_record(&dir, pair, &tombstone)?;
        pair.seen.insert(revision);
        pair.pending_deletes.remove(&head);
        changes += 1;
    }
    for id in ids {
        let conversation = state.conversation(id)?;
        let pending = state
            .queue
            .iter()
            .find(|t| t.origin.unwrap_or(state.chat_id) == id)
            .map(|t| {
                format!(
                    "{} · {}",
                    if state.held { "Held" } else { "Pending" },
                    t.text
                )
            })
            .or_else(|| pair.remote_work.get(&id).cloned());
        if conversation.history.is_empty()
            && !conversation.thread.started
            && conversation.thread.title.is_empty()
            && !conversation.thread.pinned
            && !conversation.thread.archived
            && conversation.thread.section.is_empty()
            && conversation.goal.is_empty()
            && pending.is_none()
            && !pair.heads.contains_key(&id)
        {
            continue;
        }
        let hash = fingerprint(&conversation, &pending)?;
        if pair.fingerprints.get(&id) == Some(&hash) {
            continue;
        }
        let scope = crate::providers::route(&conversation.thread.model).0;
        let provider_base = state
            .providers
            .iter()
            .find(|p| p.id == scope)
            .map(|p| p.base.clone());
        let record = Record {
            deleted: false,
            origin: (!pair.heads.contains_key(&id)).then(|| format!("{}-{id}", hex(&pair.device))),
            parent: pair.heads.get(&id).cloned(),
            conversation,
            pending,
            provider_base,
        };
        let revision = publish_record(&dir, pair, &record)?;
        pair.seen.insert(revision.clone());
        pair.heads.insert(id, revision);
        pair.fingerprints.insert(id, hash);
        changes += 1;
    }
    let mut waiting = BTreeMap::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(rev) = name
            .to_str()
            .and_then(|s| s.strip_prefix("thread-"))
            .and_then(|s| s.strip_suffix(".spindle"))
        else {
            continue;
        };
        if !valid_id(rev) || pair.seen.contains(rev) {
            continue;
        }
        let data = vault::open(&pair.key, rev.as_bytes(), &read(&entry.path(), LIMIT)?)?;
        let tag = revision_id(&pair.key, &data);
        ensure!(
            tag == rev,
            "Thread revision identifier does not match its contents"
        );
        let record: Record = serde_json::from_slice(&data)?;
        ensure!(
            record.conversation.history.len() % 2 == 0
                && record
                    .conversation
                    .history
                    .iter()
                    .enumerate()
                    .all(|(i, m)| m.role == if i % 2 == 0 { "user" } else { "assistant" }),
            "Invalid synced thread roles"
        );
        waiting.insert(rev.to_owned(), record);
    }
    loop {
        let Some(rev) = waiting
            .iter()
            .find(|(_, r)| r.parent.as_ref().is_none_or(|p| pair.seen.contains(p)))
            .map(|(rev, _)| rev.clone())
        else {
            break;
        };
        let r = waiting.remove(&rev).unwrap();
        if r.deleted {
            let parent = r
                .parent
                .as_ref()
                .context("Deleted revision must identify a parent")?;
            if let Some(id) = pair
                .heads
                .iter()
                .find(|(_, h)| *h == parent)
                .map(|(&id, _)| id)
            {
                state.forget_thread(id)?;
                pair.heads.remove(&id);
                pair.remote_work.remove(&id);
                pair.fingerprints.remove(&id);
            }
            pair.seen.insert(rev);
            changes += 1;
            continue;
        }
        let id = r
            .parent
            .as_ref()
            .and_then(|p| {
                pair.heads
                    .iter()
                    .find(|(_, head)| *head == p)
                    .map(|(id, _)| *id)
            })
            .filter(|id| {
                !state
                    .queue
                    .iter()
                    .any(|t| t.origin.unwrap_or(state.chat_id) == *id)
            })
            .unwrap_or_else(|| {
                state.serial += 1;
                state.serial
            });
        let mut c = r.conversation;
        crate::artifacts::upgrade_history(&mut c.thread, &mut c.history);
        if let Some(base) = r.provider_base {
            let tag = crate::providers::route(&c.thread.model).1;
            c.thread.model = state
                .providers
                .iter()
                .find(|p| p.base == base && p.models.iter().any(|m| m == tag))
                .map(|p| format!("@{}/{tag}", p.id))
                .unwrap_or_else(|| format!("@unavailable/{tag}"));
        }
        pair.fingerprints.insert(id, fingerprint(&c, &r.pending)?);
        if let Some(work) = r.pending {
            pair.remote_work.insert(id, work);
        } else {
            pair.remote_work.remove(&id);
        }
        if id == state.chat_id {
            if state.model != c.thread.model {
                state.attachment = None;
                state.pending = None;
            }
            state.model.clone_from(&c.thread.model);
            state.history = c.history;
            state.thread = c.thread;
            state.goal = c.goal;
            state.plan = c.plan;
        } else {
            state.chats.insert(id, c);
        }
        state.note_thread_update(id);
        pair.heads.insert(id, rev.clone());
        pair.seen.insert(rev);
        changes += 1;
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::Message;
    fn reply(state: &mut State, q: &str, a: &str) {
        state.history.push(Message::new("user", q));
        state.history.push(Message::new("assistant", a));
    }
    #[test]
    fn identical_first_turns_belong_to_distinct_threads() {
        let dir = tempfile::tempdir().unwrap();
        projects::create(dir.path(), "Project").unwrap();
        let pass = "two equal conversations";
        let mut a = State {
            sync: Some(pair(dir.path(), pass).unwrap()),
            ..Default::default()
        };
        reply(&mut a, "hello", "hi");
        a.switch_chat(None).unwrap();
        reply(&mut a, "hello", "hi");
        refresh(dir.path(), &mut a).unwrap();
        let mut b = State {
            sync: Some(pair(dir.path(), pass).unwrap()),
            ..Default::default()
        };
        refresh(dir.path(), &mut b).unwrap();
        assert_eq!(b.chats.len(), 2);
    }

    #[test]
    fn paused_deletion_removes_thread_on_existing_and_fresh_devices() {
        let root = tempfile::tempdir().unwrap();
        projects::create(root.path(), "Project").unwrap();
        let pass = "encrypted thread deletion";
        let mut a = State {
            sync: Some(pair(root.path(), pass).unwrap()),
            ..Default::default()
        };
        let mut b = State {
            sync: Some(pair(root.path(), pass).unwrap()),
            ..Default::default()
        };
        reply(&mut a, "private message", "response");
        refresh(root.path(), &mut a).unwrap();
        refresh(root.path(), &mut b).unwrap();
        let id = *b.chats.keys().next().unwrap();
        b.switch_chat(Some(id)).unwrap();
        a.sync.as_mut().unwrap().delete(0);
        a.forget_thread(0).unwrap();
        let mut a: State = serde_json::from_slice(&serde_json::to_vec(&a).unwrap()).unwrap();
        refresh(root.path(), &mut a).unwrap();
        refresh(root.path(), &mut b).unwrap();
        assert!(b.thread_data(id).is_none() && b.history.is_empty() && b.chats.is_empty());
        let mut fresh = State {
            sync: Some(pair(root.path(), pass).unwrap()),
            ..Default::default()
        };
        refresh(root.path(), &mut fresh).unwrap();
        assert!(fresh.chats.is_empty() && fresh.history.is_empty());
        assert_eq!(refresh(root.path(), &mut fresh).unwrap(), 0);
    }
    #[test]
    fn two_devices_continue_and_preserve_offline_forks() {
        let root = tempfile::tempdir().unwrap();
        projects::create(root.path(), "Shared").unwrap();
        let pass = "a project secret for testing";
        let mut a = State {
            sync: Some(pair(root.path(), pass).unwrap()),
            model: "test".into(),
            ..Default::default()
        };
        let mut b = State {
            sync: Some(pair(root.path(), pass).unwrap()),
            ..Default::default()
        };
        assert!(pair(root.path(), "wrong password entered").is_err());
        reply(&mut a, "sensitive-message", "one");
        refresh(root.path(), &mut a).unwrap();
        refresh(root.path(), &mut b).unwrap();
        assert_eq!(b.chats.len(), 1);
        b.switch_chat(b.chats.keys().next().copied()).unwrap();
        assert_eq!(b.history[0].content, "sensitive-message");
        reply(&mut b, "from device B", "two");
        refresh(root.path(), &mut b).unwrap();
        refresh(root.path(), &mut a).unwrap();
        assert_eq!(a.history.len(), 4);
        reply(&mut a, "change offline A", "three");
        reply(&mut b, "change offline B", "other");
        refresh(root.path(), &mut a).unwrap();
        refresh(root.path(), &mut b).unwrap();
        refresh(root.path(), &mut a).unwrap();
        assert_eq!(a.chats.len(), 1);
        assert_eq!(b.chats.len(), 1);
        assert!(a.queue.is_empty() && b.queue.is_empty());
        assert_eq!(refresh(root.path(), &mut a).unwrap(), 0);
        let dir = projects::shared_dir(root.path(), false).unwrap();
        let file = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "spindle"))
            .unwrap();
        let mut bytes = fs::read(&file).unwrap();
        assert!(!bytes.windows(17).any(|s| s == b"sensitive-message"));
        bytes[14] ^= 1;
        fs::write(file, bytes).unwrap();
        let mut c = State {
            sync: Some(pair(root.path(), pass).unwrap()),
            ..Default::default()
        };
        assert!(refresh(root.path(), &mut c).is_err());
        assert!(c.queue.is_empty());
    }
}
