//! Between Spindle's state and the synced items: what this device publishes, and how merged
//! items from other devices are applied.
//!
//! Each round imports first (so this device builds on what others did), then publishes what
//! changed here. What is shared follows the switches: "sync conversations" and "sync files"
//! globally, overridden per project (the project wins), and "Don't show on other devices".

use super::chat::{self, Turn};
use super::core::Kind;
use super::project_sync::{self, Policies, make_project, project_id};
use super::space::{Identity, Space};
use super::transport::{Progress, Transport};
use crate::ollama::Message;
use crate::state::State;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Global switches (Settings › Sync).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Switches {
    pub conversations: bool,
    pub files: bool,
    #[serde(default = "yes")]
    pub skills: bool,
    #[serde(default)]
    pub local_providers: bool,
    /// Sync every few minutes and when something changes.
    pub background: bool,
    /// Fetch a chat's latest turns when it opens.
    pub on_open: bool,
    /// A quick sync before sending in a synced chat.
    pub before_send: bool,
    /// Find this space's other devices on the local network and sync with them directly.
    #[serde(default = "yes")]
    pub peers: bool,
}

fn yes() -> bool {
    true
}

impl Default for Switches {
    fn default() -> Self {
        Self {
            conversations: true,
            files: false,
            skills: true,
            local_providers: false,
            background: true,
            on_open: true,
            before_send: true,
            peers: true,
        }
    }
}

/// A project's own choices; `None` uses the global switch.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectSync {
    pub id: String,
    #[serde(default)]
    pub conversations: Option<bool>,
    #[serde(default)]
    pub files: Option<bool>,
    /// Don't show this project on other devices at all.
    #[serde(default)]
    pub hidden: bool,
    /// Where this device runs the project's work (phase 2): `None` is this device.
    #[serde(default)]
    pub run_on: Option<String>,
    /// For projects with file sync on: download files ("download"), or only when opened.
    #[serde(default)]
    pub download: Option<bool>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub pending_pause: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Place {
    Folder {
        path: PathBuf,
    },
    Hosted {
        url: String,
    },
    /// Another device on the local network: found by its announcements, or typed by hand
    /// (then the device isn't known until it answers).
    Peer {
        device: Option<String>,
        addr: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub place: Place,
    #[serde(default)]
    pub progress: Progress,
}

/// The sync state shown in the window.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    /// Last successful sync (Unix seconds).
    pub last_ok: Option<i64>,
    /// Why the last attempt failed, if it did.
    pub error: Option<String>,
    pub syncing: bool,
    /// Other devices that were off or closed (a phone with Spindle shut) when nothing else
    /// could be reached: not an error, they catch up when they're back.
    #[serde(default)]
    pub waiting: Vec<String>,
    /// Addresses advertised during the most recent local sync round.
    #[serde(skip)]
    pub local_endpoints: String,
}

/// Another branch of a chat, for the switcher.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BranchInfo {
    /// The branch's first turn after the fork.
    pub id: String,
    /// The device that started it.
    pub device: String,
    pub device_name: String,
    /// How many messages it shares with the main branch.
    pub fork: usize,
    pub messages: Vec<Message>,
    /// Messages before this position were already merged into the active history.
    #[serde(default)]
    pub new_from: usize,
    /// Syncable tool notes, results and attachments, keyed by position in this history.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub info: BTreeMap<usize, Value>,
}

impl BranchInfo {
    pub fn merge_from(&self) -> usize {
        self.fork.max(self.new_from)
    }
}

/// Everything this device keeps about sync (inside Spindle's encrypted state).
#[derive(Clone, Serialize, Deserialize)]
pub struct SyncState {
    #[serde(default)]
    pub remote: super::remote::Remote,
    pub identity: Identity,
    #[serde(default)]
    pub switches: Switches,
    #[serde(default)]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub device_rules: BTreeMap<String, DeviceRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admitted_records: Option<BTreeSet<String>>,
    /// Local chat ID → synced chat ID.
    #[serde(default)]
    pub chats: BTreeMap<u64, String>,
    /// Local project folder → its sync settings.
    #[serde(default)]
    pub projects: BTreeMap<PathBuf, ProjectSync>,
    #[serde(default)]
    pub project_aliases: BTreeMap<String, String>,
    /// Projects other devices share that have no folder here.
    #[serde(default)]
    pub remote_projects: BTreeMap<String, Value>,
    #[serde(default)]
    pub project_deletions: BTreeMap<String, bool>,
    #[serde(default)]
    pub provider_host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_dns: String,
    #[serde(default)]
    pub imported_providers: BTreeMap<String, (String, String)>,
    #[serde(default)]
    pub imported_ollama: Option<(String, String)>,
    #[serde(default)]
    pub pending_removals: BTreeMap<String, BTreeSet<String>>,
    /// Local memory ID → synced ID.
    #[serde(default)]
    pub memories: BTreeMap<u64, String>,
    /// The fingerprint last published or applied, per item, to skip unchanged ones.
    #[serde(default)]
    pub seen: BTreeMap<String, String>,
    /// The value this device last published or applied, per item: what local changes are
    /// measured against, so a stale local copy never overwrites a newer edit elsewhere.
    #[serde(default)]
    pub base: BTreeMap<String, Value>,
    /// Other branches per local chat.
    #[serde(default)]
    pub branches: BTreeMap<u64, Vec<BranchInfo>>,
    /// A branch the user made main, per synced chat (published with the chat).
    #[serde(default)]
    pub main: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pending_main: BTreeSet<String>,
    /// Turns hidden with Delete branch, per synced chat.
    #[serde(default)]
    pub dropped: BTreeMap<String, BTreeSet<String>>,
    /// Merge receipts waiting to be published together with the copied messages.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pending_merges: BTreeMap<String, BTreeSet<chat::MergeCopy>>,
    #[serde(default)]
    pub status: Status,
    /// Sync and remote work paused from the tray or Settings.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub activity: Vec<String>,
    /// A sync was asked for (Sync now, a chat opened, a message about to be sent).
    #[serde(default, skip)]
    pub wanted: Option<Want>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceRules {
    pub allow_host_jobs: bool,
    pub allow_browser: bool,
    pub paused: bool,
    pub blocked: BTreeSet<String>,
    pub blocked_projects: BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub blocked_jobs: BTreeSet<super::placement::Kind>,
}

impl DeviceRules {
    fn allows(&self, kind: Kind, body: &super::core::Body, project: Option<&str>, aliases: &BTreeMap<String, String>) -> bool {
        let category = match kind {
            Kind::Project => "projects",
            Kind::Chat => "conversations",
            Kind::Settings if project.is_some() || body.id.starts_with("project-alias:") => "projects",
            Kind::Settings => "settings",
            Kind::Skill => "skills",
            Kind::Memory => "memories",
            Kind::Provider | Kind::Secret => "providers",
            Kind::Schedule => "settings",
            Kind::Devices | Kind::Keys => return true,
        };
        let other = (kind == Kind::Settings && body.id.starts_with("project-alias:"))
            .then(|| body.value["target"].as_str()).flatten();
        !self.paused && !self.blocked.contains(category)
            && [project, other].into_iter().flatten().all(|id|
                !self.blocked_projects.contains(project_sync::resolved(aliases, id)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Now,
    Open,
    Send,
}

/// Local sync bindings survive leaving a space. Membership, keys and transports are
/// established afresh when the device is paired again.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedSpace {
    #[serde(default)]
    project_aliases: BTreeMap<String, String>,
    switches: Switches,
    projects: BTreeMap<PathBuf, ProjectSync>,
    chats: BTreeMap<u64, String>,
    project_base: BTreeMap<String, Value>,
    chat_fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    main: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pending_main: BTreeSet<String>,
    #[serde(default)]
    dropped: BTreeMap<String, BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pending_merges: BTreeMap<String, BTreeSet<chat::MergeCopy>>,
    #[serde(default)]
    provider_host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    peer_dns: String,
    #[serde(default)]
    imported_providers: BTreeMap<String, (String, String)>,
    #[serde(default)]
    imported_ollama: Option<(String, String)>,
    #[serde(default)]
    device_rules: BTreeMap<String, DeviceRules>,
    #[serde(default)]
    admitted_records: Option<BTreeSet<String>>,
}

pub fn disconnect(state: &mut State) {
    if let Some(sync) = state.space.take() {
        state.detached_spaces.insert(
            sync.identity.keys.space,
            DetachedSpace {
                project_aliases: sync.project_aliases,
                switches: sync.switches,
                projects: sync.projects,
                chats: sync.chats,
                project_base: sync
                    .base
                    .into_iter()
                    .filter(|(key, _)| key.starts_with("project:"))
                    .collect(),
                chat_fingerprints: sync
                    .seen
                    .into_iter()
                    .filter(|(key, _)| key.starts_with("chat:"))
                    .collect(),
                main: sync.main,
                pending_main: sync.pending_main,
                dropped: sync.dropped,
                pending_merges: sync.pending_merges,
                provider_host: sync.provider_host,
                peer_dns: sync.peer_dns,
                imported_providers: sync.imported_providers,
                imported_ollama: sync.imported_ollama,
                device_rules: sync.device_rules,
                admitted_records: sync.admitted_records,
            },
        );
    }
}

pub fn reconnect(state: &mut State, identity: Identity) -> SyncState {
    disconnect(state);
    let detached = state.detached_spaces.remove(&identity.keys.space);
    let mut sync = SyncState::new(identity);
    if let Some(previous) = detached {
        sync.project_aliases = previous.project_aliases;
        sync.switches = previous.switches;
        sync.projects = previous.projects;
        sync.chats = previous.chats;
        sync.base = previous.project_base;
        sync.seen = previous.chat_fingerprints;
        sync.main = previous.main;
        sync.pending_main = previous.pending_main;
        sync.dropped = previous.dropped;
        sync.pending_merges = previous.pending_merges;
        sync.provider_host = previous.provider_host;
        sync.peer_dns = previous.peer_dns;
        sync.imported_providers = previous.imported_providers;
        sync.imported_ollama = previous.imported_ollama;
        sync.device_rules = previous.device_rules;
        sync.admitted_records = previous.admitted_records;
    }
    sync
}

impl SyncState {
    pub fn new(identity: Identity) -> Self {
        SyncState {
            remote: Default::default(),
            project_aliases: Default::default(),
            identity,
            switches: Switches::default(),
            links: vec![],
            device_rules: Default::default(),
            admitted_records: None,
            chats: BTreeMap::new(),
            projects: BTreeMap::new(),
            remote_projects: BTreeMap::new(),
            project_deletions: BTreeMap::new(),
            provider_host: String::new(),
            peer_dns: String::new(),
            imported_providers: BTreeMap::new(),
            imported_ollama: None,
            pending_removals: BTreeMap::new(),
            memories: BTreeMap::new(),
            seen: BTreeMap::new(),
            base: BTreeMap::new(),
            branches: BTreeMap::new(),
            main: BTreeMap::new(),
            pending_main: BTreeSet::new(),
            dropped: BTreeMap::new(),
            pending_merges: BTreeMap::new(),
            status: Status::default(),
            paused: false,
            activity: vec![],
            wanted: None,
        }
    }

    pub fn log(&mut self, line: String) {
        self.activity.push(line);
        if self.activity.len() > 100 {
            self.activity.remove(0);
        }
    }

    pub(crate) fn set_device_rule(&mut self, device: &str, key: &str, on: bool, baseline: BTreeSet<String>) -> Result<()> {
        let mut rules = self.device_rules.get(device).cloned().unwrap_or_default();
        if key == "sync" {
            rules.paused = !on;
        } else if key == "host-jobs" {
            rules.allow_host_jobs = on;
        } else if key == "host-browser" {
            rules.allow_browser = on;
        } else if let Some(key) = key.strip_prefix("job:") {
            let job = super::placement::Kind::OPERATIONS.into_iter()
                .find(|job| job.key() == key)
                .context("Unknown remote job category")?;
            if on { rules.blocked_jobs.remove(&job); } else { rules.blocked_jobs.insert(job); }
        } else {
            let (items, label) = if let Some(id) = key.strip_prefix("project:") {
                anyhow::ensure!(crate::projects::valid_sync_id(id), "Use a valid project ID");
                (&mut rules.blocked_projects, project_sync::resolved(&self.project_aliases, id))
            } else {
                anyhow::ensure!(matches!(key, "projects" | "conversations" | "settings" | "skills" | "memories" | "providers"), "Unknown sync category");
                (&mut rules.blocked, key)
            };
            if on { items.remove(label); } else { items.insert(label.to_owned()); }
        }
        if rules == DeviceRules::default() {
            self.device_rules.remove(device);
        } else {
            self.admitted_records.get_or_insert(baseline);
            self.device_rules.insert(device.to_owned(), rules);
        }
        self.wanted = Some(Want::Now);
        Ok(())
    }

    pub(crate) fn source_paused(&self, device: &str) -> bool {
        self.device_rules.get(device).is_some_and(|r| r.paused)
    }

    fn incoming(&mut self, space: &Space) -> Option<Space> {
        if self.device_rules.is_empty() {
            self.admitted_records = None;
            return None;
        }
        let admitted = self.admitted_records.get_or_insert_with(BTreeSet::new);
        let mut projects = BTreeMap::new();
        // A chat or memory tombstone has no value. Look up its project from its previous records.
        for (id, envelope) in &space.store.records {
            if matches!(envelope.kind, Kind::Chat | Kind::Memory)
                && let Some(body) = space.store.bodies.get(id)
                && !body.deleted
                && projects.get(&envelope.item).is_none_or(|(clock, _)| *clock < &envelope.clock)
            {
                projects.insert(&envelope.item, (&envelope.clock, body.value["project"].as_str()));
            }
        }
        for (id, envelope) in &space.store.records {
            let Some(body) = space.store.bodies.get(id) else { continue };
            if admitted.contains(id) { continue; }
            let project = match envelope.kind {
                Kind::Project => Some(body.id.as_str()),
                Kind::Chat | Kind::Memory if body.deleted => projects.get(&envelope.item).and_then(|(_, p)| *p),
                Kind::Chat | Kind::Memory => body.value["project"].as_str(),
                Kind::Settings => ["project-sharing:", "project-deleted:", "project-alias:"]
                    .into_iter().find_map(|prefix| body.id.strip_prefix(prefix)),
                _ => None,
            };
            if self.device_rules.get(&envelope.device).is_none_or(|rules|
                rules.allows(envelope.kind, body, project, &self.project_aliases)) {
                admitted.insert(id.clone());
            }
        }
        if space.store.bodies.keys().all(|id| admitted.contains(id)) {
            return None;
        }
        Some(Space {
            id: space.id.clone(),
            store: super::core::Store {
                records: space.store.records.iter().filter(|(id, _)| admitted.contains(*id))
                    .map(|(id, record)| (id.clone(), record.clone())).collect(),
                bodies: space.store.bodies.iter().filter(|(id, _)| admitted.contains(*id))
                    .map(|(id, body)| (id.clone(), body.clone())).collect(),
                last: space.store.last.clone(),
                waiting: Default::default(),
            },
            dir: None,
        })
    }

    /// Whether a project's conversations sync (the project wins over the global switch).
    pub fn project_conversations(&self, project: Option<&Path>) -> bool {
        match project.and_then(|p| self.projects.get(p)) {
            Some(p) if p.hidden || p.paused => false,
            Some(p) => p.conversations.unwrap_or(self.switches.conversations),
            None => self.switches.conversations,
        }
    }

    pub fn project_files(&self, project: &Path) -> bool {
        match self.projects.get(project) {
            Some(p) if p.hidden || p.paused => false,
            Some(p) => p.files.unwrap_or(self.switches.files),
            None => self.switches.files,
        }
    }
}

/// Where a space's records live on this device.
pub fn store_dir(data: &Path, space: &str) -> PathBuf {
    data.join("sync").join(space)
}

fn fingerprint(v: &Value) -> String {
    super::core::hex(&super::core::sha256(v.to_string().as_bytes())[..12])
}

/// Preference fields that stay with one device.
const LOCAL_PREFS: [&str; 16] = [
    "browser_path",
    "browser_mode",
    "disabled_browsers",
    "web_country",
    "use_os_location",
    "sidebar_width",
    "zoom",
    "compact_sidebar",
    "text_size",
    "nerd_stats",
    "over_18",
    "background",
    "start_on_sign_in",
    // Addresses and folders: "localhost" or C:\ on one device means something else on another.
    "ollama",
    "ollama_on",
    "projects_dir",
];

fn pref_value(state: &State) -> Value {
    let mut prefs = serde_json::to_value(&state.preferences).unwrap_or(Value::Null);
    if let Some(o) = prefs.as_object_mut() {
        for k in LOCAL_PREFS {
            o.remove(k);
        }
    }
    prefs
}

fn models_value(state: &State) -> Value {
    json!({
        "favorites": state.favorites,
        "hidden": state.hidden_models,
        "council": state.council,
    })
}

pub(super) fn provider_value(p: &crate::providers::Provider) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        for k in [
            "credential",
            "registration",
            "claude_login",
            "catalog_warning",
        ] {
            o.remove(k);
        }
    }
    v
}

pub(super) fn provider_from_wire(id: &str, value: &Value) -> Option<crate::providers::Provider> {
    if !id.starts_with('p')
        || id.len() > 96
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    let mut value = value.as_object()?.clone();
    for field in ["credential", "registration", "claude_login"] {
        value.remove(field);
    }
    value.insert("id".into(), json!(id));
    serde_json::from_value(Value::Object(value)).ok()
}

// Connection metadata and its credential binding are complete snapshots. Applying them
// field by field could combine an older key or catalog with a different destination.
fn latest_snapshot(space: &Space, kind: Kind, id: &str) -> Option<Value> {
    let records = space.item(kind, id);
    let body = records.last()?.2;
    (!body.deleted).then(|| body.value.clone())
}

fn provider_fingerprint(p: &crate::providers::Provider) -> String {
    fingerprint(&json!([p.name, p.base, p.protocol]))
}

fn legacy_provider_id(id: &str) -> Option<&str> {
    let original = id
        .split_once("-legacy-")
        .map_or(id, |(original, _)| original);
    original
        .strip_prefix('p')
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))?;
    Some(original)
}

fn provider_alias(id: &str, provider: &crate::providers::Provider) -> String {
    format!("{id}-legacy-{}", provider_fingerprint(provider))
}

fn provider_record_id(space: &Space, p: &crate::providers::Provider) -> String {
    if legacy_provider_id(&p.id) == Some(p.id.as_str()) {
        let alias = provider_alias(&p.id, p);
        if space.value(Kind::Provider, &alias).is_some()
            || latest_snapshot(space, Kind::Provider, &p.id)
                .and_then(|v| provider_from_wire(&p.id, &v))
                .is_some_and(|remote| !p.same_connection(&remote))
        {
            return alias;
        }
    }
    p.id.clone()
}

fn local_provider_index(state: &State, remote: &crate::providers::Provider) -> Option<usize> {
    let original = legacy_provider_id(&remote.id).unwrap_or(&remote.id);
    let alias = provider_alias(original, remote);
    state.providers.iter().position(|p| {
        (p.id == remote.id || p.id == original || p.id == alias) && p.same_connection(remote)
    })
}

fn local_thread_model(state: &State, remote: &str, binding: &Value) -> String {
    let (scope, tag) = crate::providers::route(remote);
    let Some(original) = legacy_provider_id(scope) else {
        return remote.to_owned();
    };
    let Some(binding) = binding["connection"]
        .as_str()
        .filter(|_| binding["model"].as_str() == Some(remote))
        .filter(|id| id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return remote.to_owned();
    };
    let alias = format!("{original}-legacy-{binding}");
    if let Some(p) = state
        .providers
        .iter()
        .find(|p| (p.id == original || p.id == alias) && provider_fingerprint(p) == binding)
    {
        return format!("@{}/{tag}", p.id);
    }
    // Keep the intended route unavailable until its provider arrives or the user selects one.
    format!("@{alias}/{tag}")
}

fn me(sync: &SyncState) -> String {
    sync.identity.device.id.clone()
}

/// Find a chat's messages and notes for writing.
fn chat_mut(
    state: &mut State,
    id: u64,
) -> Option<(&mut Vec<Message>, &mut crate::state::ThreadInfo)> {
    if id == state.chat_id {
        Some((&mut state.history, &mut state.thread))
    } else {
        state
            .chats
            .get_mut(&id)
            .map(|c| (&mut c.history, &mut c.thread))
    }
}

/// A chat as turns: the role, text, images and Spindle's notes per message.
fn chat_turns(state: &State, id: u64) -> Vec<Turn> {
    let Some((thread, history)) = state.thread_data(id) else {
        return vec![];
    };
    let lines: Vec<_> = history
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut info = thread
                .turns
                .get(&i)
                .and_then(|t| serde_json::to_value(t).ok())
                .unwrap_or(Value::Null);
            if let Some(o) = info.as_object_mut() {
                // Undo data points at folders on this device.
                o.remove("applied");
            }
            if thread.tool_results.contains(&i) {
                info = json!({ "tool_result": true });
            }
            (m.role.clone(), m.content.clone(), m.images.clone(), info)
        })
        .collect();
    chat::chain(&lines)
}

fn project_value(project: &crate::projects::Project, settings: &ProjectSync, home: &str) -> Value {
    json!({
        "name": project.name,
        "description": project.description,
        "instructions": project.instructions,
        "home": home,
        "conversations": settings.conversations,
        "files": settings.files,
    })
}

/// Publish what changed on this device.
pub fn export(state: &mut State, space: &mut Space) -> Result<usize> {
    let filtered = state.space.as_mut().and_then(|sync| sync.incoming(space));
    project_sync::read_links(state, filtered.as_ref().unwrap_or(space));
    project_sync::normalize_bindings(state);
    let Some(mut sync) = state.space.take() else {
        return Ok(0);
    };
    let r = export_with(state, &mut sync, space);
    state.space = Some(sync);
    r
}

/// Publish only what changed here since this device last published or applied the item.
pub(super) fn put_item(
    sync: &mut SyncState,
    space: &mut Space,
    kind: Kind,
    id: &str,
    value: Value,
) -> Result<usize> {
    let key = format!("{}:{id}", kind.name());
    let base = sync.base.get(&key);
    if base == Some(&value) {
        return Ok(0);
    }
    // A device that has never published or applied an item that already exists in the space
    // (a new device's defaults) takes the space's value first, on import.
    if base.is_none() && space.value(kind, id).is_some() {
        return Ok(0);
    }
    let changed = super::core::changed_fields(base, &value);
    if base.is_some() && changed.is_empty() && value.is_object() {
        sync.base.insert(key, value);
        return Ok(0);
    }
    space.put_changes(kind, id, value.clone(), changed)?;
    sync.base.insert(key, value);
    Ok(1)
}

fn export_with(state: &State, sync: &mut SyncState, space: &mut Space) -> Result<usize> {
    let mut n = super::remote::export(sync,space)?;
    for (from, to) in &sync.project_aliases {
        let key = format!("project-alias:{from}");
        if space.value(Kind::Settings, &key).is_none() {
            n += usize::from(space.put(Kind::Settings, &key, json!({"target":to}))?.is_some());
        }
    }
    let device = me(sync);
    n += project_sync::publish(sync, space)?;
    let policies = if let Some(filtered) = sync.incoming(space) {
        Policies::read(&filtered)
    } else {
        Policies::read(space)
    };
    for p in sync.projects.values_mut() {
        p.paused = policies.paused(&p.id);
    }
    // Settings: everything but what belongs to this screen.
    n += put_item(
        sync,
        space,
        Kind::Settings,
        "preferences",
        pref_value(state),
    )?;
    n += put_item(
        sync,
        space,
        Kind::Settings,
        "tuning",
        serde_json::to_value(&state.tuning)?,
    )?;
    n += put_item(sync, space, Kind::Settings, "models", models_value(state))?;
    if sync.switches.skills {
        for (name, text) in &state.skills {
            n += put_item(sync, space, Kind::Skill, name, json!({ "text": text }))?;
        }
        for (name, v) in space.items(Kind::Skill) {
            if v.is_some()
                && !state.skills.contains_key(&name)
                && sync.base.contains_key(&format!("skill:{name}"))
            {
                space.delete(Kind::Skill, &name)?;
                sync.base.remove(&format!("skill:{name}"));
                n += 1;
            }
        }
    }
    // Memories (project memories only for shared projects).
    for m in &state.memories {
        if let Some(p) = &m.project
            && !sync.project_conversations(Some(p))
        {
            continue;
        }
        let gid = sync
            .memories
            .entry(m.id)
            .or_insert_with(|| format!("{}-{}", &device[..12], m.id))
            .clone();
        let project = match m.project.as_deref() {
            Some(p) => match project_id(sync, p) {
                Ok(id) if !policies.blocked(&id) => Some(id),
                _ => continue,
            },
            None => None,
        };
        n += put_item(
            sync,
            space,
            Kind::Memory,
            &gid,
            json!({ "text": m.text, "project": project, "saved": m.saved }),
        )?;
    }
    let live: BTreeSet<u64> = state.memories.iter().map(|m| m.id).collect();
    for (local, gid) in sync.memories.clone() {
        if !live.contains(&local) {
            space.delete(Kind::Memory, &gid)?;
            sync.memories.remove(&local);
            sync.base.remove(&format!("memory:{gid}"));
            n += 1;
        }
    }
    // Providers, without credentials or sign-ins; API keys as secrets.
    for p in &state.providers {
        if !p.protocol.oauth() && !p.needs_auth && !p.has_saved_key() {
            continue;
        }
        let id = provider_record_id(space, p);
        let mut value = provider_value(p);
        value["id"] = json!(id);
        value["needs_auth"] = json!(true);
        n += put_item(sync, space, Kind::Provider, &id, value)?;
        let oauth = p.protocol.oauth();
        if !oauth
            && sync.identity.secrets
            && let Ok(key) = p.key()
            && !key.is_empty()
        {
            n += put_item(
                sync,
                space,
                Kind::Secret,
                &id,
                json!({ "key": key, "connection": provider_fingerprint(p) }),
            )?;
        }
    }
    n += super::provider_sync::export(state, sync, space)?;
    // Schedules run on the device that made them unless moved.
    for s in &state.automation.schedules {
        let mut v = serde_json::to_value(s)?;
        v["runs_on"] = json!(device);
        n += put_item(
            sync,
            space,
            Kind::Schedule,
            &format!("{}-{}", &device[..12], s.id),
            v,
        )?;
    }
    // Projects this device has.
    let paths: BTreeSet<PathBuf> = state
        .chats
        .values()
        .filter_map(|c| c.thread.project.clone())
        .chain(state.thread.project.clone())
        .chain(sync.projects.keys().cloned())
        .collect();
    let mut local_homes = BTreeMap::new();
    for (path, settings) in &sync.projects {
        let home = local_homes.entry(settings.id.clone()).or_insert_with(|| path.clone());
        if crate::projects::prefer_folder(path, home) { *home = path.clone(); }
    }
    for path in paths {
        let Ok(Some(p)) = crate::projects::existing(&path) else {
            continue;
        };
        if sync.projects.get(&p.path).is_some_and(|s| policies.deleted(&s.id)) {
            continue;
        }
        let pid = project_id(sync, &p.path)?;
        let ps = sync.projects.get(&p.path).cloned().unwrap_or_default();
        if policies.blocked(&pid) || local_homes.get(&pid).is_some_and(|home| home != &p.path) {
            continue;
        }
        if ps.hidden {
            // Don't show on other devices: take its listing down.
            if space.value(Kind::Project, &pid).is_some() {
                space.delete(Kind::Project, &pid)?;
                sync.base.remove(&format!("project:{pid}"));
                n += 1;
            }
            continue;
        }
        // The device that made it stays its home, whoever republishes it.
        let saved = space.value(Kind::Project, &pid);
        let home = saved
            .as_ref()
            .and_then(|v| v["home"].as_str())
            .unwrap_or(&device);
        n += put_item(sync, space, Kind::Project, &pid, project_value(&p, &ps, home))?;
    }
    // Conversations, as branch trees.
    let ids: Vec<u64> = state.chats.keys().copied().chain([state.chat_id]).collect();
    for id in ids {
        let Some((thread, history)) = state.thread_data(id) else {
            continue;
        };
        if history.is_empty() && thread.title.is_empty() {
            continue;
        }
        if !sync.project_conversations(thread.project.as_deref()) {
            continue;
        }
        // Work still running stays local until the reply is in.
        if state
            .queue
            .iter()
            .any(|t| t.origin.unwrap_or(state.chat_id) == id)
        {
            continue;
        }
        let project = match thread.project.as_deref() {
            Some(p) => match project_id(sync, p) {
                Ok(id) if !policies.blocked(&id) => Some(id),
                _ => continue,
            },
            None => None,
        };
        let gid = sync
            .chats
            .entry(id)
            .or_insert_with(|| format!("{}-{id}", &device[..12]))
            .clone();
        let turns = chat_turns(state, id);
        let local: BTreeSet<&str> = turns.iter().map(|t| t.h.as_str()).collect();
        // Turns this device wrote that are no longer in its line (an edit) are dropped, so
        // they don't come back as a branch.
        let origin = space
            .item(Kind::Chat, &gid)
            .first()
            .map(|r| r.1.device.clone())
            .unwrap_or_else(|| device.clone());
        let before = space.value(Kind::Chat, &gid);
        let saved_main = || sync.main.get(&gid).cloned();
        let main = if sync.pending_main.contains(&gid) || sync.pending_merges.contains_key(&gid) {
            saved_main()
        } else {
            before.as_ref().and_then(|v| v["main"].as_str()).map(str::to_owned).or_else(saved_main)
        };
        let pending_merge = sync.pending_merges.get(&gid);
        let dropped_now = sync.dropped.get(&gid).cloned().unwrap_or_default();
        let merged = chat::view(
            &space.item(Kind::Chat, &gid),
            &origin,
            main.as_deref(),
            &dropped_now,
        );
        let mut dropped = dropped_now;
        if pending_merge.is_none() && !sync.pending_main.contains(&gid) {
            for t in merged.main.iter().filter(|t| t.device == device) {
                if !local.contains(t.turn.h.as_str()) && turns.len() <= merged.main.len() {
                    dropped.insert(t.turn.h.clone());
                }
            }
        }
        sync.dropped.insert(gid.clone(), dropped.clone());
        let (scope, _) = crate::providers::route(&thread.model);
        let model_provider = legacy_provider_id(scope)
            .and_then(|_| state.providers.iter().find(|p| p.id == scope))
            .map(|p| json!({"model": thread.model, "connection": provider_fingerprint(p)}));
        let mut value = json!({
            "title": thread.title,
            "model": thread.model,
            "model_provider": model_provider,
            "pinned": thread.pinned,
            "archived": thread.archived,
            "section": thread.section,
            "project": project,
            "main": main,
            "dropped": dropped,
            "turns": turns,
        });
        let key = format!("chat:{gid}");
        let f = fingerprint(&value);
        if pending_merge.is_none() && sync.seen.get(&key) == Some(&f) {
            continue;
        }
        if let Some(copies) = pending_merge {
            value["merges"] = serde_json::to_value(copies)?;
        }
        let changed = super::core::changed_fields(before.as_ref(), &value);
        space.put_chat(&gid, value, changed)?;
        sync.pending_main.remove(&gid);
        sync.pending_merges.remove(&gid);
        if let Some(main) = main { sync.main.insert(gid, main); }
        sync.seen.insert(key, f);
        n += 1;
    }
    Ok(n)
}

/// Apply what other devices published.
pub fn import(state: &mut State, space: &Space) -> Result<usize> {
    let Some(sync) = state.space.as_mut() else { return Ok(0) };
    let n = super::remote::import(sync, space)?;
    let incoming = sync.incoming(space);
    let space = incoming.as_ref().unwrap_or(space);
    project_sync::read_links(state, space);
    project_sync::normalize_bindings(state);
    let Some(mut sync) = state.space.take() else {
        return Ok(0);
    };
    super::remote::deliver(state,&mut sync);
    let r = import_with(state, &mut sync, space).map(|count| n + count);
    state.space = Some(sync);
    r
}

/// Whether a merged value differs from what this device last published or applied.
fn applied(sync: &mut SyncState, key: String, v: &Value) -> bool {
    sync.base.get(&key) != Some(v)
}

fn import_with(state: &mut State, sync: &mut SyncState, space: &Space) -> Result<usize> {
    let mut n = 0;
    let device = me(sync);
    let names: BTreeMap<String, String> = space
        .members()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();
    // Settings.
    if let Some(v) = space.value(Kind::Settings, "preferences")
        && applied(sync, "settings:preferences".into(), &v)
    {
        let mut local = serde_json::to_value(&state.preferences)?;
        if let (Some(l), Some(r)) = (local.as_object_mut(), v.as_object()) {
            for (k, val) in r {
                if !LOCAL_PREFS.contains(&k.as_str()) {
                    l.insert(k.clone(), val.clone());
                }
            }
        }
        if let Ok(p) = serde_json::from_value(local) {
            state.preferences = p;
            n += 1;
        }
        sync.base
            .insert("settings:preferences".into(), pref_value(state));
    }
    if let Some(v) = space.value(Kind::Settings, "tuning")
        && applied(sync, "settings:tuning".into(), &v)
        && let Ok(t) = serde_json::from_value(v)
    {
        state.tuning = t;
        sync.base.insert(
            "settings:tuning".into(),
            serde_json::to_value(&state.tuning)?,
        );
        n += 1;
    }
    if let Some(v) = space.value(Kind::Settings, "models")
        && applied(sync, "settings:models".into(), &v)
    {
        if let Ok(f) = serde_json::from_value(v["favorites"].clone()) {
            state.favorites = f;
        }
        if let Ok(h) = serde_json::from_value(v["hidden"].clone()) {
            state.hidden_models = h;
        }
        if let Ok(c) = serde_json::from_value(v["council"].clone()) {
            state.council = c;
        }
        sync.base
            .insert("settings:models".into(), models_value(state));
        n += 1;
    }
    // Skills: the newest wins; a concurrent loser is kept under another name.
    let skills = if sync.switches.skills { space.items(Kind::Skill) } else { vec![] };
    for (name, v) in skills {
        let key = format!("skill:{name}");
        match v {
            Some(v) => {
                if !applied(sync, key.clone(), &v) {
                    continue;
                }
                if let Some(t) = v["text"].as_str() {
                    state.skills.insert(name.clone(), t.to_owned());
                    sync.base.insert(key, json!({ "text": t }));
                    n += 1;
                }
                if let Ok(tag) = space.id.keys.tag(Kind::Skill, &name) {
                    for (e, b) in super::core::conflict(&space.store, Kind::Skill, &tag)
                        .iter()
                        .skip(1)
                    {
                        if let Some(t) = b.value["text"].as_str() {
                            let from = names
                                .get(&e.device)
                                .cloned()
                                .unwrap_or_else(|| "another device".into());
                            state
                                .skills
                                .entry(format!("{name} (from {from})"))
                                .or_insert_with(|| t.to_owned());
                        }
                    }
                }
            }
            None => {
                if sync.base.remove(&key).is_some() && state.skills.remove(&name).is_some() {
                    n += 1;
                }
            }
        }
    }
    // Memories.
    let projects_dir = Some(state.preferences.projects_dir.trim())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    let policies = Policies::read(space);
    n += project_sync::apply(state, sync, &policies, projects_dir.as_deref());
    let mut project_paths = BTreeMap::new();
    for (path, settings) in &sync.projects {
        let home = project_paths.entry(settings.id.clone()).or_insert_with(|| path.clone());
        if crate::projects::prefer_folder(path, home) { *home = path.clone(); }
    }
    for (gid, v) in space.items(Kind::Memory) {
        let local = sync
            .memories
            .iter()
            .find(|(_, g)| **g == gid)
            .map(|(l, _)| *l);
        if v.as_ref().and_then(|v| v["project"].as_str())
            .map(|p| project_sync::resolved(&sync.project_aliases, p))
            .is_some_and(|p| policies.blocked(p) || !project_paths.contains_key(p))
        {
            continue;
        }
        let key = format!("memory:{gid}");
        match (v, local) {
            (Some(v), _) if !applied(sync, key.clone(), &v) => {}
            (Some(v), local) => {
                let text = v["text"].as_str().unwrap_or("").to_owned();
                let project = v["project"]
                    .as_str()
                    .and_then(|p| project_paths.get(project_sync::resolved(&sync.project_aliases, p)).cloned());
                sync.base.insert(key.clone(), v.clone());
                match local.and_then(|l| state.memories.iter_mut().find(|m| m.id == l)) {
                    Some(m) => {
                        m.text = text;
                        m.project = project;
                    }
                    None => {
                        let id = state.memories.iter().map(|m| m.id).max().unwrap_or(0) + 1;
                        state.memories.push(crate::memory::Memory {
                            id,
                            text,
                            project,
                            saved: v["saved"].as_i64().unwrap_or(0),
                        });
                        sync.memories.insert(id, gid.clone());
                    }
                }
                n += 1;
            }
            (None, Some(l)) => {
                state.memories.retain(|m| m.id != l);
                sync.memories.remove(&l);
                sync.base.remove(&key);
                n += 1;
            }
            (None, None) => {}
        }
    }
    // A synced catalog can update a matching connection. Credentials and endpoints stay local.
    for (pid, _) in space.items(Kind::Provider) {
        let Some(v) = latest_snapshot(space, Kind::Provider, &pid) else {
            continue;
        };
        let base = format!("provider:{pid}");
        if !applied(sync, base.clone(), &v) {
            continue;
        }
        let Some(mut profile) = provider_from_wire(&pid, &v) else {
            continue;
        };
        if !profile.protocol.oauth()
            && !profile.needs_auth
            && latest_snapshot(space, Kind::Secret, &pid).is_none()
            && !local_provider_index(state, &profile).is_some_and(|i| state.providers[i].has_saved_key())
        {
            continue;
        }
        profile.needs_auth = true;
        sync.base.insert(base, v);
        if let Some(index) = local_provider_index(state, &profile) {
            state.providers[index].merge_catalog(profile);
        } else {
            if state.providers.iter().any(|p| p.id == pid) {
                // Old versions assigned p1, p2, … independently on each device.
                let Some(original) = legacy_provider_id(&pid) else {
                    continue;
                };
                profile.id = provider_alias(original, &profile);
                if state.providers.iter().any(|p| p.id == profile.id) {
                    continue;
                }
            }
            state.providers.push(profile);
        }
        n += 1;
    }
    n += super::provider_sync::import(state, sync, space);
    // API keys: stored the way this device stores keys (Windows Credential Manager).
    if sync.identity.secrets {
        for (pid, _) in space.items(Kind::Secret) {
            let Some(v) = latest_snapshot(space, Kind::Secret, &pid) else {
                continue;
            };
            if !applied(sync, format!("secret:{pid}"), &v) {
                continue;
            }
            let Some(key) = v["key"].as_str().filter(|k| !k.is_empty()) else {
                continue;
            };
            let Some(profile) = latest_snapshot(space, Kind::Provider, &pid)
                .and_then(|value| provider_from_wire(&pid, &value))
            else {
                continue;
            };
            let valid = if let Some(connection) = v["connection"].as_str() {
                connection == provider_fingerprint(&profile)
            } else {
                // Legacy keys have no binding. A slot with more than one connection cannot
                // assign its key safely.
                space.item(Kind::Provider, &pid).iter().all(|(_, _, body)| {
                    provider_from_wire(&pid, &body.value)
                        .is_some_and(|previous| profile.same_connection(&previous))
                })
            };
            if !valid {
                continue;
            }
            let Some(index) = local_provider_index(state, &profile) else {
                continue;
            };
            let p = &mut state.providers[index];
            if p.has_saved_key() || p.protocol.oauth() {
                sync.base.insert(format!("secret:{pid}"), v);
                continue;
            }
            match p.store_key(key) {
                Ok(()) => {
                    sync.base.insert(format!("secret:{pid}"), v);
                    n += 1;
                }
                Err(e) => sync.log(format!("Couldn't store the API key for {}: {e:#}", p.name)),
            }
        }
    }
    // Schedules made elsewhere are listed but run only on their device.
    let mut remote_schedules = vec![];
    for (sid, v) in space.items(Kind::Schedule) {
        if let Some(v) = v
            && v["runs_on"].as_str() != Some(device.as_str())
        {
            remote_schedules.push((sid, v));
        }
    }
    // Projects made on another device. Where its conversations sync, this device gets a
    // project of its own to hold them (name, description and instructions; the files stay on
    // the other device until file sync), so its chats appear in it rather than loose.
    let mut made = false;
    for (pid, v) in space.items(Kind::Project) {
        if project_sync::resolved(&sync.project_aliases, &pid) != pid {
            sync.remote_projects.remove(&pid);
            continue;
        }
        if policies.deleted(&pid) {
            continue;
        }
        if policies.paused(&pid) {
            if !project_paths.contains_key(&pid) && let Some(mut value) = v {
                value["paused"] = json!(true);
                sync.remote_projects.insert(pid, value);
            }
            continue;
        }
        if let Some(path) = project_paths.get(&pid) {
            // Keep the local copy's name and notes current.
            if let Some(v) = &v
                && let Ok(Some(p)) = crate::projects::existing(path)
            {
                let (name, description, instructions) = (
                    v["name"].as_str().unwrap_or(&p.name),
                    v["description"].as_str().unwrap_or(&p.description),
                    v["instructions"].as_str().unwrap_or(&p.instructions),
                );
                let p = if (name, description, instructions)
                    != (
                        p.name.as_str(),
                        p.description.as_str(),
                        p.instructions.as_str(),
                    ) {
                    let Ok(p) = crate::projects::update(path, name, description, instructions)
                    else {
                        continue;
                    };
                    let _ = crate::projects::library(Some(&p));
                    n += 1;
                    p
                } else {
                    p
                };
                let home = v["home"].as_str().unwrap_or(&device);
                sync.base.insert(
                    format!("project:{pid}"),
                    project_value(&p, &sync.projects[path], home),
                );
            }
            continue;
        }
        let Some(v) = v else {
            sync.remote_projects.remove(&pid);
            continue;
        };
        let conversations = v["conversations"]
            .as_bool()
            .unwrap_or(sync.switches.conversations);
        // Only into the folder the person chose (asked on the first sync that brings one).
        if conversations
            && let Some(root) = projects_dir.as_deref()
            && let Some(p) = make_project(root, &pid, &v)
        {
            let settings = ProjectSync {
                id: pid.clone(),
                ..Default::default()
            };
            let home = v["home"].as_str().unwrap_or(&device);
            sync.base
                .insert(format!("project:{pid}"), project_value(&p, &settings, home));
            sync.projects.insert(p.path.clone(), settings);
            project_paths.insert(pid.clone(), p.path);
            sync.remote_projects.remove(&pid);
            made = true;
            n += 1;
            continue;
        }
        sync.remote_projects.insert(pid, v);
    }
    // Chats already brought in loose look again, now that their project exists.
    if made {
        sync.seen.retain(|k, _| !k.starts_with("chatview:"));
    }
    let _ = remote_schedules;
    // Conversations.
    for (gid, v) in space.items(Kind::Chat) {
        let local = sync.chats.iter().find(|(_, g)| **g == gid).map(|(l, _)| *l);
        if local.and_then(|id| state.thread_data(id))
            .and_then(|(t, _)| t.project.as_ref())
            .and_then(|path| sync.projects.get(path))
            .is_some_and(|p| p.paused)
        {
            continue;
        }
        let Some(v) = v else {
            // Deleted elsewhere: remove here unless work is running in it.
            if let Some(l) = local
                && !state
                    .queue
                    .iter()
                    .any(|t| t.origin.unwrap_or(state.chat_id) == l)
                && l != state.chat_id
            {
                state.chats.remove(&l);
                sync.chats.remove(&l);
                sync.branches.remove(&l);
                n += 1;
            }
            continue;
        };
        if let Some(l) = local
            && state
                .queue
                .iter()
                .any(|t| t.origin.unwrap_or(state.chat_id) == l)
        {
            continue;
        }
        if sync.pending_merges.contains_key(&gid) || sync.pending_main.contains(&gid) {
            continue;
        }
        // In a project waiting for its folder here: wait with it (not loose in Chats).
        if v["project"].as_str().is_some_and(|p| policies.blocked(project_sync::resolved(&sync.project_aliases, p))) {
            continue;
        }
        if local.is_none()
            && v["project"]
                .as_str()
                .is_some_and(|p| sync.remote_projects.contains_key(project_sync::resolved(&sync.project_aliases, p)))
        {
            continue;
        }
        let records = space.item(Kind::Chat, &gid);
        let origin = records
            .first()
            .map(|r| r.1.device.clone())
            .unwrap_or_default();
        let main = v["main"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| sync.main.get(&gid).cloned());
        if let Some(main) = &main { sync.main.insert(gid.clone(), main.clone()); }
        let mut dropped: BTreeSet<String> = sync.dropped.get(&gid).cloned().unwrap_or_default();
        for r in &records {
            if let Some(list) = r.2.value["dropped"].as_array() {
                dropped.extend(list.iter().filter_map(|x| x.as_str().map(str::to_owned)));
            }
        }
        let view = chat::view(&records, &origin, main.as_deref(), &dropped);
        let fp = fingerprint(&json!({
            "main": view.main.iter().map(|t| &t.turn.h).collect::<Vec<_>>(),
            "branches": view.branches.iter().map(|b| (b.new_from, b.turns.iter()
                .map(|t| &t.turn.h).collect::<Vec<_>>())).collect::<Vec<_>>(),
            "meta": [&v["title"], &v["pinned"], &v["archived"], &v["section"], &v["project"]],
        }));
        if sync.seen.get(&format!("chatview:{gid}")) == Some(&fp) {
            continue;
        }
        sync.seen.insert(format!("chatview:{gid}"), fp);
        let to_messages = |turns: &[chat::Placed]| -> Vec<Message> {
            turns
                .iter()
                .map(|t| Message {
                    images: t.turn.images.clone(),
                    ..Message::new(&t.turn.role, t.turn.content.clone())
                })
                .collect()
        };
        let id = match local {
            Some(l) => l,
            None => {
                state.serial += 1;
                let id = state.serial;
                state.chats.insert(id, Default::default());
                sync.chats.insert(id, gid.clone());
                id
            }
        };
        let project = v["project"]
            .as_str()
            .and_then(|p| project_paths.get(project_sync::resolved(&sync.project_aliases, p)).cloned());
        let imported_model = v["model"]
            .as_str()
            .map(|m| local_thread_model(state, m, &v["model_provider"]));
        let Some((history, thread)) = chat_mut(state, id) else {
            continue;
        };
        let messages = to_messages(&view.main);
        let messages_changed = *history != messages;
        if messages_changed {
            let mut turns = BTreeMap::new();
            let mut results = BTreeSet::new();
            for (i, t) in view.main.iter().enumerate() {
                restore_turn(i, &t.turn.info, &mut turns, &mut results);
            }
            *history = messages;
            thread.turns = turns;
            thread.tool_results = results;
        }
        if let Some(t) = v["title"].as_str() {
            thread.title = t.to_owned();
        }
        thread.pinned = v["pinned"].as_bool().unwrap_or(thread.pinned);
        thread.archived = v["archived"].as_bool().unwrap_or(thread.archived);
        if let Some(s) = v["section"].as_str() {
            thread.section = s.to_owned();
        }
        if project.is_some() || v.get("project").is_some_and(Value::is_null) {
            thread.project = project;
        }
        if thread.model.is_empty()
            && let Some(m) = imported_model
        {
            thread.model = m;
        }
        thread.started = true;
        let branches: Vec<BranchInfo> = view
            .branches
            .iter()
            .map(|b| BranchInfo {
                id: b.id().to_owned(),
                device: b.device.clone(),
                device_name: names
                    .get(&b.device)
                    .cloned()
                    .unwrap_or_else(|| "another device".into()),
                fork: b.fork,
                messages: to_messages(&b.turns),
                new_from: b.new_from,
                info: b.turns.iter().enumerate().skip(b.fork)
                    .filter(|(_, t)| !t.turn.info.is_null())
                    .map(|(index, t)| (index, t.turn.info.clone())).collect(),
            })
            .collect();
        let branches_changed = sync.branches.get(&id).map_or(!branches.is_empty(), |previous| {
            previous.len() != branches.len() || previous.iter().zip(&branches)
                .any(|(a, b)| a.id != b.id || a.fork != b.fork || a.messages != b.messages)
        });
        if branches.is_empty() {
            sync.branches.remove(&id);
        } else {
            sync.branches.insert(id, branches);
        }
        if messages_changed || branches_changed || local.is_none() {
            state.note_thread_update(id);
        }
        // What's shown now matches what's published: don't publish it back unchanged.
        let turns = chat_turns(state, id);
        let _ = turns;
        n += 1;
    }
    Ok(n)
}

fn restore_turn(index: usize, info: &Value, turns: &mut BTreeMap<usize, crate::state::Turn>,
    results: &mut BTreeSet<usize>) {
    if info["tool_result"] == true {
        results.insert(index);
    } else if !info.is_null() && let Ok(mut turn) = serde_json::from_value::<crate::state::Turn>(info.clone()) {
        // A remote Undo entry points at files on a different device.
        turn.applied = None;
        turns.insert(index, turn);
    }
}

/// Append the branch's unmerged messages and publish which source turns they came from.
pub fn merge_branch(state: &mut State, chat: u64, branch: &str) -> Result<usize> {
    anyhow::ensure!(!state.queue.iter().any(|task| task.origin.unwrap_or(state.chat_id) == chat),
        "Finish or remove this chat's queued work before merging a branch");
    let sync = state.space.as_ref().context("Sync isn't set up")?;
    let gid = sync.chats.get(&chat).cloned().context("This chat isn't synced")?;
    let b = sync.branches.get(&chat).and_then(|list| list.iter().find(|b| b.id == branch))
        .cloned().context("This branch has already changed or been merged")?;
    let from = b.merge_from();
    let (history, thread) = chat_mut(state, chat).context("Chat no longer exists")?;
    anyhow::ensure!(from < b.messages.len() && b.messages.get(..b.fork).is_some()
        && history.get(..b.fork) == b.messages.get(..b.fork),
        "The chat history has changed. Sync and review the branch again before merging");

    let hash = |parent: &str, m: &Message| chat::turn_hash(parent, &m.role, &m.content, &m.images);
    let mut copy = history.iter().fold(String::new(), |parent, m| hash(&parent, m));
    let mut source = b.messages[..from].iter().fold(String::new(), |parent, m| hash(&parent, m));
    let mut receipts = BTreeSet::new();
    for (offset, message) in b.messages.into_iter().skip(from).enumerate() {
        source = hash(&source, &message);
        copy = hash(&copy, &message);
        receipts.insert(chat::MergeCopy { source: source.clone(), copy: copy.clone() });
        if let Some(info) = b.info.get(&(from + offset)) {
            restore_turn(history.len(), info, &mut thread.turns, &mut thread.tool_results);
        }
        history.push(message);
    }
    let count = receipts.len();
    let sync = state.space.as_mut().unwrap();
    sync.main.insert(gid.clone(), copy);
    sync.pending_merges.entry(gid.clone()).or_default().extend(receipts);
    if let Some(list) = sync.branches.get_mut(&chat) {
        list.retain(|b| b.id != branch);
    }
    sync.seen.remove(&format!("chatview:{gid}"));
    sync.wanted = Some(Want::Now);
    state.note_thread_update(chat);
    Ok(count)
}

/// Make a branch the main one (the old main becomes a branch).
pub fn make_main(state: &mut State, chat: u64, branch: &str) -> Result<()> {
    let sync = state.space.as_mut().context("Sync isn't set up")?;
    let gid = sync
        .chats
        .get(&chat)
        .cloned()
        .context("This chat isn't synced")?;
    let b = sync
        .branches
        .get(&chat)
        .and_then(|list| list.iter().find(|b| b.id == branch))
        .context("No such branch")?
        .clone();
    // The branch's tip: its last message's hash.
    let lines: Vec<_> = b
        .messages
        .iter()
        .map(|m| {
            (
                m.role.clone(),
                m.content.clone(),
                m.images.clone(),
                Value::Null,
            )
        })
        .collect();
    let tip = chat::chain(&lines)
        .last()
        .map(|t| t.h.clone())
        .context("Empty branch")?;
    sync.main.insert(gid.clone(), tip);
    sync.pending_main.insert(gid.clone());
    sync.seen.remove(&format!("chatview:{gid}"));
    sync.seen.remove(&format!("chat:{gid}"));
    // Show it now; the next sync publishes the choice.
    if let Some((history, thread)) = chat_mut(state, chat) {
        *history = b.messages.clone();
        thread.turns.retain(|i, _| *i < b.fork);
        thread.tool_results.retain(|i| *i < b.fork);
        for (index, info) in &b.info {
            restore_turn(*index, info, &mut thread.turns, &mut thread.tool_results);
        }
    }
    state.space.as_mut().unwrap().wanted = Some(Want::Now);
    Ok(())
}

/// Hide a branch everywhere.
pub fn delete_branch(state: &mut State, chat: u64, branch: &str) -> Result<()> {
    let sync = state.space.as_mut().context("Sync isn't set up")?;
    let gid = sync
        .chats
        .get(&chat)
        .cloned()
        .context("This chat isn't synced")?;
    sync.dropped
        .entry(gid.clone())
        .or_default()
        .insert(branch.to_owned());
    if let Some(list) = sync.branches.get_mut(&chat) {
        list.retain(|b| b.id != branch);
    }
    sync.seen.remove(&format!("chat:{gid}"));
    sync.seen.remove(&format!("chatview:{gid}"));
    sync.wanted = Some(Want::Now);
    Ok(())
}

/// Exchange records with every transport.
async fn pass(
    state: &mut State,
    space: &mut Space,
    transports: &[(usize, Transport)],
    limit: std::time::Duration,
) -> (usize, usize, Vec<String>, Vec<String>) {
    let mut got = 0;
    let mut out = 0;
    let mut errors = vec![];
    let mut away = vec![];
    let names: BTreeMap<String, String> = space
        .members()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();
    for (i, t) in transports {
        let mut progress = state
            .space
            .as_ref()
            .and_then(|s| s.links.get(*i))
            .map(|l| l.progress.clone())
            .unwrap_or_default();
        match tokio::time::timeout(limit, super::transport::sync(space, t, &mut progress)).await {
            Ok(Ok(o)) => {
                got += o.received;
                out += o.sent;
                if let Some(l) = state.space.as_mut().and_then(|s| s.links.get_mut(*i)) {
                    l.progress = progress;
                }
            }
            // Another device on the network that's off or asleep isn't an error by itself.
            Ok(Err(e)) if super::peer::offline(&e) => away.push(match t {
                Transport::Peer(c) => c
                    .device
                    .as_ref()
                    .and_then(|d| names.get(d).cloned())
                    .unwrap_or_else(|| c.addr.clone()),
                _ => t.label(),
            }),
            Ok(Err(e)) => errors.push(format!("{}: {e:#}", t.label())),
            Err(_) => errors.push(format!("{}: no answer in {} s", t.label(), limit.as_secs())),
        }
    }
    // Only worth saying when nothing at all was reached.
    if away.len() < transports.len() {
        away.clear();
    }
    (got, out, errors, away)
}

/// One full round. Fetch first (so this device knows what exists), publish what changed here
/// (measured against what it last published or applied), send that, then apply everything
/// merged. `quick` limits each transport to a short wait (before sending a message).
pub async fn round(
    state: &mut State,
    space: &mut Space,
    transports: &[(usize, Transport)],
    quick: bool,
) -> Result<(usize, usize)> {
    let limit = std::time::Duration::from_secs(if quick { 3 } else { 30 });
    let mut received = 0;
    let mut sent = 0;
    let (r1, s1, mut errors, mut away) = pass(state, space, transports, limit).await;
    received += r1;
    sent += s1;
    if export(state, space)? > 0 {
        let (r2, s2, e2, a2) = pass(state, space, transports, limit).await;
        received += r2;
        sent += s2;
        errors = e2;
        away = a2;
    }
    import(state, space)?;
    let sync = state.space.as_mut().unwrap();
    sync.status.syncing = false;
    if away.is_empty() && (errors.is_empty() || errors.len() < transports.len()) {
        sync.status.last_ok = Some(crate::automation::now());
    }
    sync.status.error = (!errors.is_empty()).then(|| errors.join("; "));
    sync.status.waiting = away;
    if received + sent > 0 {
        sync.log(format!(
            "{}: received {received}, sent {sent}",
            crate::automation::local_time(crate::automation::now())
        ));
    }
    if let Some(e) = &sync.status.error {
        let e = e.clone();
        sync.log(format!("Couldn't sync: {e}"));
    }
    Ok((received, sent))
}

#[cfg(test)]
mod tests {
    use super::super::core::Device;
    use super::super::transport::Hub;
    use super::*;

    fn device_state(space: &Space) -> State {
        let mut s = State::default();
        s.space = Some(SyncState::new(space.id.clone()));
        s
    }

    fn joined(founder: &mut Space, name: &str) -> Space {
        let s = Space::open(
            Identity {
                keys: founder.keys_for(true),
                device: Device::generate(name).unwrap(),
                secrets: true,
            },
            None,
        )
        .unwrap();
        founder.add_member(s.member_entry().unwrap()).unwrap();
        s
    }

    fn say(state: &mut State, role: &str, text: &str) {
        state.history.push(Message::new(role, text));
        state.thread.title = "Chat".into();
    }

    #[tokio::test]
    async fn pausing_a_device_keeps_prior_changes_and_holds_new_ones() -> Result<()> {
        let transports = [(0, Transport::Memory(Hub::default()))];
        let mut desktop = Space::create("Desktop", None)?;
        let mut phone = joined(&mut desktop, "Phone");
        let mut sender = device_state(&desktop);
        let mut receiver = device_state(&phone);
        let device = desktop.id.device.id.clone();
        sender.skills.insert("tidy".into(), "Initial".into());
        round(&mut sender, &mut desktop, &transports, true).await?;
        round(&mut receiver, &mut phone, &transports, true).await?;
        assert_eq!(receiver.skills.get("tidy").map(String::as_str), Some("Initial"));
        let baseline = phone.store.bodies.keys().cloned().collect();
        receiver.space.as_mut().unwrap().set_device_rule(&device, "sync", false, baseline)?;
        sender.skills.insert("tidy".into(), "Updated".into());
        sender.skills.insert("new-skill".into(), "New".into());
        round(&mut sender, &mut desktop, &transports, true).await?;
        round(&mut receiver, &mut phone, &transports, true).await?;
        assert_eq!(receiver.skills.get("tidy").map(String::as_str), Some("Initial"));
        assert!(!receiver.skills.contains_key("new-skill"));
        receiver.space.as_mut().unwrap().set_device_rule(&device, "skills", false, Default::default())?;
        receiver.space.as_mut().unwrap().set_device_rule(&device, "sync", true, Default::default())?;
        round(&mut receiver, &mut phone, &transports, true).await?;
        assert!(!receiver.skills.contains_key("new-skill"));
        receiver.space.as_mut().unwrap().set_device_rule(&device, "skills", true, Default::default())?;
        round(&mut receiver, &mut phone, &transports, true).await?;
        assert_eq!(receiver.skills.get("tidy").map(String::as_str), Some("Updated"));
        assert!(receiver.skills.contains_key("new-skill"));
        Ok(())
    }

    #[tokio::test]
    async fn colliding_legacy_providers_keep_both_connections_and_local_credentials() {
        let hub = Transport::Memory(Hub::default());
        let transports = [(0, hub)];
        let mut desktop = Space::create("Desktop", None).unwrap();
        let mut phone = joined(&mut desktop, "Phone");
        let mut a = device_state(&desktop);
        let mut b = device_state(&phone);
        for (state, name, url, model) in [
            (
                &mut a,
                "Desktop Maya",
                "http://127.0.0.1:18081/v1",
                "inkling",
            ),
            (&mut b, "Phone OpenAI", "https://api.openai.com/v1", "gpt"),
        ] {
            state.space.as_mut().unwrap().identity.secrets = false;
            state.model = format!("@p1/{model}");
            state.thread.model = state.model.clone();
            say(state, "user", name);
            state.providers.push(
                serde_json::from_value(json!({
                    "id": "p1", "name": name, "base": url, "protocol": "ChatCompletions",
                    "models": [model], "credential": format!("local-{model}")
                }))
                .unwrap(),
            );
        }
        // Both old clients published p1 before learning about the other device's p1.
        export(&mut a, &mut desktop).unwrap();
        export(&mut b, &mut phone).unwrap();
        for _ in 0..4 {
            round(&mut a, &mut desktop, &transports, false)
                .await
                .unwrap();
            round(&mut b, &mut phone, &transports, false).await.unwrap();
        }
        for (state, local, model) in [(&a, "Desktop Maya", "inkling"), (&b, "Phone OpenAI", "gpt")]
        {
            assert_eq!(state.providers.len(), 2);
            assert_eq!(state.model, format!("@p1/{model}"));
            let original = state.providers.iter().find(|p| p.id == "p1").unwrap();
            assert_eq!(original.name, local);
            assert!(original.has_saved_key());
            let other = state.providers.iter().find(|p| p.id != "p1").unwrap();
            assert!(other.id.starts_with("p1-legacy-"));
            assert!(!other.has_saved_key());
            let synced = state
                .chats
                .values()
                .find(|chat| {
                    chat.history
                        .first()
                        .is_some_and(|m| m.content == other.name)
                })
                .unwrap();
            assert_eq!(crate::providers::route(&synced.thread.model).0, other.id);
        }

        // A peer that can see the same connection but has no catalog cannot empty it.
        let profile = &b.providers[0];
        let slot = provider_record_id(&phone, profile);
        let mut empty = provider_value(profile);
        empty["id"] = json!(slot);
        empty["models"] = json!([]);
        phone.put(Kind::Provider, &slot, empty).unwrap();
        import(&mut b, &phone).unwrap();
        assert_eq!(b.providers[0].models, vec!["gpt"]);
        assert!(b.providers[0].has_saved_key());
    }

    #[tokio::test]
    async fn chats_settings_and_skills_reach_another_device_and_edits_branch() {
        let hub = Transport::Memory(Hub::default());
        let mut sa = Space::create("Desktop", None).unwrap();
        let mut sb = joined(&mut sa, "Laptop");
        let mut a = device_state(&sa);
        let mut b = device_state(&sb);
        a.preferences.enter_sends = false;
        a.preferences.zoom = 150;
        a.preferences.web_country = "US".into();
        b.preferences.web_country = "CA".into();
        a.preferences.use_os_location = Some(true);
        b.preferences.use_os_location = Some(false);
        a.preferences.browser_mode = crate::browser_link::Mode::AllowAll;
        a.preferences.browser_allowlist.push("https://docs.example.com".into());
        a.preferences.browser_blocklist.push("https://bank.example.com".into());
        a.skills.insert("tidy".into(), "Tidy the notes".into());
        say(&mut a, "user", "hello");
        say(&mut a, "assistant", "hi there");
        let ts = [(0usize, hub.clone())];
        a.space.as_mut().unwrap().links.push(Link {
            place: Place::Folder { path: "/x".into() },
            progress: Default::default(),
        });
        b.space.as_mut().unwrap().links.push(Link {
            place: Place::Folder { path: "/x".into() },
            progress: Default::default(),
        });
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        assert!(!b.preferences.enter_sends);
        assert_eq!(b.preferences.zoom, 100, "zoom stays per device");
        assert_eq!(b.preferences.web_country, "CA", "country consent stays per device");
        assert_eq!(b.preferences.use_os_location, Some(false), "OS permission stays per device");
        assert_eq!(b.preferences.browser_mode, crate::browser_link::Mode::Allowlist);
        assert_eq!(b.preferences.browser_allowlist, ["https://docs.example.com"]);
        assert_eq!(b.preferences.browser_blocklist, ["https://bank.example.com"]);
        assert_eq!(b.skills["tidy"], "Tidy the notes");
        let (bid, chat) = b.chats.iter().next().map(|(i, c)| (*i, c.clone())).unwrap();
        assert_eq!(chat.history.len(), 2);
        assert_eq!(chat.thread.title, "Chat");

        // Both devices continue the chat at the same time: the desktop (where it started)
        // keeps the main branch; the laptop's turns become a branch.
        say(&mut a, "user", "desk question");
        let c = b.chats.get_mut(&bid).unwrap();
        c.history.push(Message::new("user", "laptop question"));
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        assert_eq!(a.history.last().unwrap().content, "desk question");
        let branches = &a.space.as_ref().unwrap().branches[&a.chat_id];
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].device_name, "Laptop");
        assert_eq!(
            branches[0].messages.last().unwrap().content,
            "laptop question"
        );
        // The laptop shows the main line too, with its own turns as a branch.
        let bc = &b.chats[&bid];
        assert_eq!(bc.history.last().unwrap().content, "desk question");
        assert_eq!(b.space.as_ref().unwrap().branches[&bid].len(), 1);
    }

    #[tokio::test]
    async fn merged_branches_sync_once_and_keep_late_offline_messages() {
        let transport = [(0, Transport::Memory(Hub::default()))];
        let mut desktop = Space::create("Desktop", None).unwrap();
        let mut phone = joined(&mut desktop, "Phone");
        let mut a = device_state(&desktop);
        let mut b = device_state(&phone);
        say(&mut a, "user", "shared question");
        say(&mut a, "assistant", "shared answer");
        round(&mut a, &mut desktop, &transport, false).await.unwrap();
        round(&mut b, &mut phone, &transport, false).await.unwrap();
        let aid = a.chat_id;
        let bid = *b.chats.keys().next().unwrap();

        say(&mut a, "user", "desktop question");
        let alt = b.chats.get_mut(&bid).unwrap();
        let mut question = Message::new("user", "phone question");
        question.images.push("example-image".into());
        alt.history.push(question);
        alt.history.push(Message::new("assistant", "phone answer"));
        alt.thread.turns.insert(3, crate::state::Turn {
            model: "example-model".into(),
            artifacts: vec![crate::state::Attachment { path: "answer.txt".into(), text: "answer".into() }],
            ..Default::default()
        });
        let offline = alt.clone();
        round(&mut a, &mut desktop, &transport, false).await.unwrap();
        round(&mut b, &mut phone, &transport, false).await.unwrap();
        round(&mut a, &mut desktop, &transport, false).await.unwrap();

        let branch = a.space.as_ref().unwrap().branches[&aid][0].id.clone();
        assert_eq!(merge_branch(&mut a, aid, &branch).unwrap(), 2);
        assert!(merge_branch(&mut a, aid, &branch).is_err());
        assert_eq!(a.history.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            ["shared question", "shared answer", "desktop question", "phone question", "phone answer"]);
        assert_eq!(a.history[3].images, ["example-image"]);
        assert_eq!(a.thread.turns[&4].artifacts[0].path, "answer.txt");

        // Pending merges also survive restarting before the first sync.
        a = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        round(&mut a, &mut desktop, &transport, false).await.unwrap();
        round(&mut b, &mut phone, &transport, false).await.unwrap();
        assert_eq!(b.chats[&bid].history, a.history);
        assert_eq!(b.chats[&bid].thread.turns[&4].model, "example-model");
        assert!(a.space.as_ref().unwrap().branches.get(&aid).is_none());
        assert!(b.space.as_ref().unwrap().branches.get(&bid).is_none());

        // The phone already had another message in progress on the original branch.
        let mut delayed = offline;
        delayed.history.push(Message::new("user", "late phone message"));
        b.chats.insert(bid, delayed);
        round(&mut b, &mut phone, &transport, false).await.unwrap();
        round(&mut a, &mut desktop, &transport, false).await.unwrap();
        let late = &a.space.as_ref().unwrap().branches[&aid][0];
        assert_eq!(late.messages.len() - late.merge_from(), 1);
        assert_eq!(late.messages[late.merge_from()].content, "late phone message");
        let branch = late.id.clone();
        assert_eq!(merge_branch(&mut a, aid, &branch).unwrap(), 1);
        round(&mut a, &mut desktop, &transport, false).await.unwrap();
        round(&mut b, &mut phone, &transport, false).await.unwrap();
        assert_eq!(b.chats[&bid].history, a.history);
        assert_eq!(a.history.len(), 6);
        assert_eq!(a.history[5].content, "late phone message");
        assert!(a.space.as_ref().unwrap().branches.get(&aid).is_none());
        assert!(b.space.as_ref().unwrap().branches.get(&bid).is_none());
    }

    /// A project made on one device shows up on another as a project of its own (a folder
    /// under the data folder), with its chats inside, not loose.
    #[tokio::test]
    async fn a_project_from_another_device_holds_its_chats() {
        let hub = Transport::Memory(Hub::default());
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("Phone projects");
        let proj = tmp.path().canonicalize().unwrap().join("VRT reports");
        std::fs::create_dir_all(&proj).unwrap();
        crate::projects::create_described(&proj, "VRT reports", "Ridership").unwrap();
        let mut sa = Space::create("Desktop", None).unwrap();
        let mut sb = joined(&mut sa, "Phone");
        let mut a = device_state(&sa);
        let mut b = device_state(&sb);
        say(&mut a, "user", "summarize route 9");
        say(&mut a, "assistant", "The route 9 pilot starts on April 17.");
        a.thread.project = Some(proj.clone());
        let ts = [(0usize, hub.clone())];
        a.preferences.projects_dir = "/desktop/only".into();
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        // No folder chosen on the phone yet: the project waits, and its chat with it (not
        // loose in Chats). The desktop's folder setting doesn't come across.
        assert!(b.chats.is_empty());
        assert!(b.preferences.projects_dir.is_empty());
        assert!(!b.space.as_ref().unwrap().remote_projects.is_empty());
        // Chosen: the project is made there and the chat goes in it.
        b.preferences.projects_dir = data.display().to_string();
        b.space
            .as_mut()
            .unwrap()
            .seen
            .retain(|k, _| !k.starts_with("chatview:"));
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        let chat = b.chats.values().next().expect("the chat arrived");
        let here = chat
            .thread
            .project
            .clone()
            .expect("in a project, not loose");
        assert!(here.starts_with(data.canonicalize().unwrap()), "{here:?}");
        let p = crate::projects::existing(&here).unwrap().unwrap();
        assert_eq!(
            (p.name.as_str(), p.description.as_str()),
            ("VRT reports", "Ridership")
        );
        assert!(b.space.as_ref().unwrap().remote_projects.is_empty());
        b.thread.project = Some(here.clone());
        let evidence = crate::project_history::related(&b, b.chat_id, "When does the route 9 pilot start?").unwrap();
        assert!(evidence.contains("April 17"));
        // Renamed on the desktop: the phone's copy follows, and its home stays the desktop.
        crate::projects::update(&proj, "VRT reports 2026", "Ridership", "").unwrap();
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        assert_eq!(
            crate::projects::existing(&here).unwrap().unwrap().name,
            "VRT reports 2026"
        );
        let pid = &a.space.as_ref().unwrap().projects[&proj].id;
        assert_eq!(
            sb.value(Kind::Project, pid).unwrap()["home"],
            json!(sa.id.device.id)
        );
    }

    #[tokio::test]
    async fn rejoining_with_a_new_device_id_reuses_project_and_chat_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project = root.join("Reports");
        let peer_projects = root.join("Peer projects");
        crate::projects::create_described(&project, "Reports", "Original description").unwrap();
        let mut peer = Space::create("Remaining peer", None).unwrap();
        let mut old = joined(&mut peer, "Old desktop");
        let mut a = device_state(&old);
        let mut b = device_state(&peer);
        b.preferences.projects_dir = peer_projects.display().to_string();
        a.thread.project = Some(project.clone());
        let mapping = a.space.as_mut().unwrap();
        mapping.switches.conversations = false;
        project_id(mapping, &project).unwrap();
        mapping.projects.get_mut(&project).unwrap().conversations = Some(true);
        say(&mut a, "user", "Summarize the reports");
        let transport = [(0, Transport::Memory(Hub::default()))];
        round(&mut b, &mut peer, &transport, false).await.unwrap();
        round(&mut a, &mut old, &transport, false).await.unwrap();
        round(&mut b, &mut peer, &transport, false).await.unwrap();

        let previous = a.space.as_ref().unwrap();
        let project_id = previous.projects[&project].id.clone();
        let chat_id = previous.chats[&a.chat_id].clone();
        let peer_path = b
            .chats
            .values()
            .next()
            .unwrap()
            .thread
            .project
            .clone()
            .unwrap();
        std::fs::write(peer_path.join("keep.txt"), "peer file").unwrap();
        disconnect(&mut a);
        let snapshot = serde_json::to_vec(&a).unwrap();
        let mut a: State = serde_json::from_slice(&snapshot).unwrap();
        assert!(a.space.is_none());
        let other = Space::create("Unrelated sync space", None).unwrap();
        let unrelated = reconnect(&mut a, other.id.clone());
        assert!(unrelated.projects.is_empty() && unrelated.chats.is_empty());
        assert!(unrelated.switches.conversations);
        peer.remove_member(&old.id.device.id).unwrap();
        crate::projects::update(&peer_path, "Reports", "Edited while away", "").unwrap();
        round(&mut b, &mut peer, &transport, false).await.unwrap();

        let mut again = joined(&mut peer, "Same desktop, new identity");
        assert_ne!(again.id.device.id, old.id.device.id);
        a.space = Some(reconnect(&mut a, again.id.clone()));
        round(&mut b, &mut peer, &transport, false).await.unwrap();
        round(&mut a, &mut again, &transport, false).await.unwrap();
        round(&mut b, &mut peer, &transport, false).await.unwrap();

        let rejoined = a.space.as_ref().unwrap();
        assert_eq!(rejoined.projects[&project].id, project_id);
        assert!(!rejoined.switches.conversations);
        assert_eq!(rejoined.projects[&project].conversations, Some(true));
        assert_eq!(rejoined.chats[&a.chat_id], chat_id);
        assert!(a.chats.is_empty(), "existing chat was imported again");
        assert_eq!(b.chats.len(), 1, "peer received a duplicate chat");
        assert_eq!(peer.items(Kind::Project).len(), 1);
        assert_eq!(std::fs::read_dir(&peer_projects).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_to_string(peer_path.join("keep.txt")).unwrap(),
            "peer file"
        );
        assert_eq!(
            crate::projects::existing(&project)
                .unwrap()
                .unwrap()
                .description,
            "Edited while away"
        );
    }

    #[tokio::test]
    async fn synced_updates_make_a_chat_unread_only_when_it_is_not_viewed() {
        let mut a_space = Space::create("Desktop", None).unwrap();
        let mut b_space = joined(&mut a_space, "Phone");
        let mut a = device_state(&a_space);
        let mut b = device_state(&b_space);
        let ts = [(0, Transport::Memory(Hub::default()))];
        say(&mut a, "user", "A synced question");
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        let id = *b.chats.keys().next().unwrap();
        assert!(b.unread.contains(&id));
        b.switch_chat(Some(id)).unwrap();
        assert!(!b.unread.contains(&id));
        b.viewed_chat = Some(id);

        say(&mut a, "assistant", "Visible update");
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert!(!b.unread.contains(&id));

        // A chat can remain selected while Settings covers it.
        b.viewed_chat = None;
        say(&mut a, "user", "Another update");
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert!(b.unread.contains(&id));
        b.unread.remove(&id);
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert!(!b.unread.contains(&id), "replay does not mark it unread again");
    }

    #[tokio::test]
    async fn skills_and_unauthenticated_endpoints_follow_device_switches() {
        let mut a_space = Space::create("Desktop", None).unwrap();
        let mut b_space = joined(&mut a_space, "Phone");
        let mut a = device_state(&a_space);
        let mut b = device_state(&b_space);
        let ts = [(0, Transport::Memory(Hub::default()))];
        a.preferences.ollama_on = true;
        a.preferences.ollama = "http://localhost:11434".into();
        a.space.as_mut().unwrap().provider_host = "model-host".into();
        b.preferences.ollama_on = false;
        b.preferences.ollama.clear();
        a.skills.insert("notes".into(), "My installed skill".into());
        a.providers.push(serde_json::from_value(json!({
            "id": "p-0123456789abcdef0123456789abcdef",
            "name": "Local Maya",
            "base": "http://127.0.0.1:9000/v1",
            "protocol": "ChatCompletions",
            "models": ["mini"]
        })).unwrap());
        b.space.as_mut().unwrap().switches.skills = false;
        assert!(!a.space.as_ref().unwrap().switches.local_providers);
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert!(b.skills.is_empty());
        assert!(b.providers.is_empty());
        assert!(!b.preferences.ollama_on);
        a.space.as_mut().unwrap().switches.local_providers = true;
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert!(b.providers.is_empty(), "receiver must opt in too");
        let switches = &mut b.space.as_mut().unwrap().switches;
        switches.skills = true;
        switches.local_providers = true;
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert_eq!(b.skills["notes"], "My installed skill");
        assert_eq!(b.preferences.ollama, "http://model-host:11434");
        assert_eq!(b.providers[0].base, "http://model-host:9000/v1");
        assert!(!b.providers[0].has_saved_key());
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        assert_eq!(b.providers.len(), 1);
        assert_eq!(a.providers.len(), 1);
        assert_eq!(a.providers[0].base, "http://127.0.0.1:9000/v1");
        assert_eq!(a.preferences.ollama, "http://localhost:11434");
    }

    #[tokio::test]
    async fn linked_legacy_projects_converge_without_cloning_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let first = crate::projects::create_described(&root.join("SnakeTest"), "SnakeTest", "").unwrap();
        let copy = crate::projects::create_described(&root.join("SnakeTest (legacy)"), "SnakeTest", "").unwrap();
        let original = crate::projects::sync_identity(&first.path).unwrap().unwrap();
        let duplicate = crate::projects::sync_identity(&copy.path).unwrap().unwrap();
        let peer_dir = root.join("phone");
        std::fs::create_dir(&peer_dir).unwrap();
        let phone = crate::projects::create_synced(&peer_dir.join("SnakeTest"), "SnakeTest", "", &duplicate).unwrap();
        std::fs::write(copy.path.join("keep.txt"), "local artifact").unwrap();
        let mut sa = Space::create("Desktop", None).unwrap();
        let mut sb = joined(&mut sa, "Phone");
        let mut a = device_state(&sa);
        let mut b = device_state(&sb);
        project_id(a.space.as_mut().unwrap(), &first.path).unwrap();
        project_id(a.space.as_mut().unwrap(), &copy.path).unwrap();
        a.thread.project = Some(copy.path.clone());
        say(&mut a, "user", "Original chat");
        b.preferences.projects_dir = peer_dir.display().to_string();
        b.thread.project = Some(phone.path.clone());
        say(&mut b, "user", "Phone chat");
        let ts = [(0, Transport::Memory(Hub::default()))];
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        project_sync::link(&mut a, &duplicate, &original).unwrap();
        for _ in 0..2 {
            round(&mut a, &mut sa, &ts, false).await.unwrap();
            round(&mut b, &mut sb, &ts, false).await.unwrap();
        }
        assert_eq!(a.thread.project.as_ref(), Some(&first.path));
        assert_eq!(b.thread.project.as_ref(), Some(&phone.path));
        assert_eq!(crate::projects::sync_identity(&phone.path).unwrap(), Some(original));
        assert_eq!(std::fs::read_dir(peer_dir).unwrap().count(), 1);
        assert!(copy.path.join("keep.txt").is_file());
    }

    #[tokio::test]
    async fn project_identity_reuses_a_copy_in_the_import_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let source = root.join("source");
        let imports = root.join("imports");
        std::fs::create_dir(&imports).unwrap();
        let project = crate::projects::create_described(&source, "Snake Test", "").unwrap();
        let id = crate::projects::sync_identity(&source).unwrap().unwrap();
        let copy = imports.join("A renamed copy");
        crate::projects::create_synced(&copy, "Snake Test", "", &id).unwrap();
        std::fs::write(copy.join("keep.txt"), "Local files").unwrap();

        let mut a_space = Space::create("Desktop", None).unwrap();
        let mut b_space = joined(&mut a_space, "Phone");
        let mut a = device_state(&a_space);
        let mut b = device_state(&b_space);
        b.preferences.projects_dir = imports.display().to_string();
        a.thread.project = Some(project.path);
        say(&mut a, "user", "This project has already been copied");
        let ts = [(0, Transport::Memory(Hub::default()))];
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();

        let copy = copy.canonicalize().unwrap();
        assert_eq!(b.space.as_ref().unwrap().projects[&copy].id, id);
        assert_eq!(b.chats.values().next().unwrap().thread.project.as_ref(), Some(&copy));
        assert_eq!(std::fs::read_dir(imports).unwrap().count(), 1);
        assert!(copy.join("keep.txt").is_file());
    }

    #[tokio::test]
    async fn paused_project_keeps_changes_local_until_resumed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let a_path = crate::projects::create_described(&root.join("source"), "Snake Test", "").unwrap().path;
        let imports = root.join("imports");
        std::fs::create_dir(&imports).unwrap();
        let mut a_space = Space::create("Desktop", None).unwrap();
        let mut b_space = joined(&mut a_space, "Phone");
        let mut a = device_state(&a_space);
        let mut b = device_state(&b_space);
        b.preferences.projects_dir = imports.display().to_string();
        a.thread.project = Some(a_path.clone());
        say(&mut a, "user", "Before the pause");
        let ts = [(0, Transport::Memory(Hub::default()))];
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        let b_path = b.chats.values().next().unwrap().thread.project.clone().unwrap();
        project_sync::set_paused(&mut a, &a_path, true).unwrap();
        say(&mut a, "assistant", "Keep this here for now");
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert!(b.space.as_ref().unwrap().projects[&b_path].paused);
        assert_eq!(b.chats.values().next().unwrap().history.len(), 1);
        project_sync::set_paused(&mut a, &a_path, false).unwrap();
        round(&mut a, &mut a_space, &ts, false).await.unwrap();
        round(&mut b, &mut b_space, &ts, false).await.unwrap();
        assert_eq!(b.chats.values().next().unwrap().history.len(), 2);
        assert!(!b.space.as_ref().unwrap().projects[&b_path].paused);
    }

    #[tokio::test]
    async fn deleting_a_project_reaches_an_offline_device_and_can_keep_its_files() {
        for files in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().canonicalize().unwrap();
            let a_path = crate::projects::create_described(&root.join("source"), "Snake Test", "").unwrap().path;
            let b_root = root.join("phone");
            std::fs::create_dir(&b_root).unwrap();
            let mut a_space = Space::create("Desktop", None).unwrap();
            let mut b_space = joined(&mut a_space, "Phone");
            let mut a = device_state(&a_space);
            let mut b = device_state(&b_space);
            b.preferences.projects_dir = b_root.display().to_string();
            a.thread.project = Some(a_path.clone());
            say(&mut a, "user", "Project chat");
            let ts = [(0, Transport::Memory(Hub::default()))];
            round(&mut a, &mut a_space, &ts, false).await.unwrap();
            round(&mut b, &mut b_space, &ts, false).await.unwrap();
            let b_chat = *b.chats.keys().next().unwrap();
            let b_path = b.chats[&b_chat].thread.project.clone().unwrap();
            std::fs::write(b_path.join("keep.txt"), "File on the phone").unwrap();
            std::fs::write(a_path.join("keep.txt"), "File on the desktop").unwrap();
            let id = a.space.as_ref().unwrap().projects[&a_path].id.clone();

            b.switch_chat(Some(b_chat)).unwrap();
            say(&mut b, "assistant", "Made this change offline");
            project_sync::request_delete(&mut a, &a_path, files).unwrap();
            round(&mut a, &mut a_space, &ts, false).await.unwrap();
            round(&mut b, &mut b_space, &ts, false).await.unwrap();
            round(&mut a, &mut a_space, &ts, false).await.unwrap();

            assert!(a.thread.project.is_none() && b.thread.project.is_none());
            assert!(a.chats.is_empty() && b.chats.is_empty());
            assert!(b_space.value(Kind::Project, &id).is_none());
            assert!(crate::projects::existing(&a_path).unwrap().is_none());
            assert!(crate::projects::existing(&b_path).unwrap().is_none());
            assert_eq!(a_path.join("keep.txt").exists(), !files);
            assert_eq!(b_path.join("keep.txt").exists(), !files);
        }
    }

    #[tokio::test]
    async fn a_project_can_keep_its_conversations_on_this_device() {
        let hub = Transport::Memory(Hub::default());
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap().join("Secret");
        std::fs::create_dir_all(&proj).unwrap();
        crate::projects::create(&proj, "Secret").unwrap();
        let mut sa = Space::create("Desktop", None).unwrap();
        let mut sb = joined(&mut sa, "Laptop");
        let mut a = device_state(&sa);
        let mut b = device_state(&sb);
        say(&mut a, "user", "private stuff");
        a.thread.project = Some(proj.clone());
        // Global says share; the project says no, and the project wins.
        let pid = project_id(a.space.as_mut().unwrap(), &proj).unwrap();
        a.space
            .as_mut()
            .unwrap()
            .projects
            .get_mut(&proj)
            .unwrap()
            .conversations = Some(false);
        let ts = [(0usize, hub.clone())];
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        round(&mut b, &mut sb, &ts, false).await.unwrap();
        assert!(b.chats.is_empty());
        // The project itself is listed on the other device.
        assert_eq!(
            b.space.as_ref().unwrap().remote_projects[&pid]["name"],
            "Secret"
        );
        // Hidden: not even listed.
        a.space
            .as_mut()
            .unwrap()
            .projects
            .get_mut(&proj)
            .unwrap()
            .hidden = true;
        let mut sc = joined(&mut sa, "Phone");
        let mut c = device_state(&sc);
        round(&mut a, &mut sa, &ts, false).await.unwrap();
        round(&mut c, &mut sc, &ts, false).await.unwrap();
        assert!(c.chats.is_empty());
    }
}
