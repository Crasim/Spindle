//! Running sync inside the engine: when to sync, the space kept in memory between rounds,
//! and work that needs the network (registering with a server, saving an account, joining,
//! removing a device), done between rounds so the engine never waits on it in a command.

use super::adapter::{Link, Place, SyncState, Want, round, store_dir};
use super::core::Device;
use super::hosted::Client;
use super::space::{Identity, Space};
use super::transport::Transport;
use crate::state::State;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Network work waiting for the next round.
#[derive(Clone)]
pub enum Job {
    /// Register the space on a server with its admin token.
    Register { url: String, admin: String },
    /// Save an account for joining by sign-in.
    Account { user: String, password: String },
    /// Join a space by signing in to a server.
    Join {
        url: String,
        user: String,
        password: String,
        name: String,
    },
    /// Register a paired device with the servers.
    AddDevice { device: String },
}

struct Live {
    root: PathBuf,
    space: Option<Space>,
    jobs: Vec<Job>,
    next: Option<Instant>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
/// Set by a server's live channel when another device published something.
static WAKE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// One live-channel watcher per server.
static WATCHERS: Mutex<Vec<(String, tokio::task::JoinHandle<()>)>> = Mutex::new(Vec::new());

/// Keep a live connection to each server, so changes arrive within seconds.
fn watch(client: &Client) {
    let mut w = WATCHERS.lock().unwrap();
    w.retain(|(_, h)| !h.is_finished());
    if w.iter().any(|(u, _)| *u == client.base) {
        return;
    }
    let c = client.clone();
    let h = tokio::spawn(async move {
        use futures_util::StreamExt;
        loop {
            if let Ok(mut ws) = c.live().await {
                while let Some(Ok(msg)) = ws.next().await {
                    if msg
                        .to_text()
                        .is_ok_and(|t| t.contains("\"records\"") || t.contains("\"record\""))
                    {
                        WAKE.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    w.push((client.base.clone(), h));
}

/// The last thing that went wrong before sync was set up here (a failed join).
static LAST: Mutex<String> = Mutex::new(String::new());

pub fn last_error() -> String {
    LAST.lock().unwrap().clone()
}

/// Queue network work for the next round.
pub fn queue(job: Job) {
    let mut g = LIVE.lock().unwrap();
    let live = g.get_or_insert_with(|| Live {
        root: crate::runtime::data_dir(),
        space: None,
        jobs: vec![],
        next: None,
    });
    live.jobs.push(job);
    live.next = None;
}

/// Ask for a round soon.
/// A quick fingerprint of what syncs: chats (length, last message, title and flags),
/// preferences, skills and memories. Cheap enough to take on every pass of the engine.
fn fingerprint(state: &State) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let chat = |h: &mut std::collections::hash_map::DefaultHasher,
                id: u64,
                t: &crate::state::ThreadInfo,
                m: &[crate::ollama::Message]| {
        id.hash(h);
        m.len().hash(h);
        if let Some(last) = m.last() {
            last.content.hash(h);
        }
        t.title.hash(h);
        t.pinned.hash(h);
        t.archived.hash(h);
        t.section.hash(h);
        t.project.hash(h);
    };
    // The active chat is stored separately. Hash it in ID order so viewing a chat
    // does not look like an edit and start another background sync.
    for (id, c) in state.chats.range(..state.chat_id) {
        chat(&mut h, *id, &c.thread, &c.history);
    }
    chat(&mut h, state.chat_id, &state.thread, &state.history);
    for (id, c) in state.chats.range(state.chat_id..) {
        chat(&mut h, *id, &c.thread, &c.history);
    }
    serde_json::to_string(&state.preferences)
        .unwrap_or_default()
        .hash(&mut h);
    for (k, v) in &state.skills {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    for p in &state.providers {
        p.id.hash(&mut h);
        p.base.hash(&mut h);
        p.name.hash(&mut h);
        p.models.hash(&mut h);
        p.has_saved_key().hash(&mut h);
    }
    state.memories.len().hash(&mut h);
    serde_json::to_string(&state.memories)
        .map(|s| s.len())
        .unwrap_or_default()
        .hash(&mut h);
    h.finish()
}

/// Seen state, and when a change was noticed (synced a few seconds later, so a burst of
/// edits goes out together).
static CHANGES: Mutex<(u64, Option<Instant>)> = Mutex::new((0, None));

/// Called on every pass of the engine: something that syncs changed here (a reply finished,
/// a chat renamed or deleted, a setting changed): sync a few seconds later, rather than at
/// the next background check.
pub fn notice_changes(state: &mut State, busy: bool) {
    let Some(s) = state.space.as_ref() else {
        return;
    };
    if s.paused || !s.switches.background {
        return;
    }
    let fp = fingerprint(state);
    let mut g = CHANGES.lock().unwrap();
    if g.0 == 0 {
        // First look: nothing to compare with.
        g.0 = fp;
        return;
    }
    if fp != g.0 {
        g.0 = fp;
        g.1 = Some(Instant::now() + Duration::from_secs(3));
    }
    // Not while a reply is still being written: it goes out once it's finished.
    if !busy && g.1.is_some_and(|t| Instant::now() >= t) {
        g.1 = None;
        drop(g);
        want(state, Want::Now);
    }
}

/// After a round: what it brought in isn't a change of ours to send back.
fn changes_seen(state: &State) {
    CHANGES.lock().unwrap().0 = fingerprint(state);
}

#[cfg(test)]
#[test]
fn viewing_existing_chats_is_not_a_sync_edit() {
    let mut state = State::default();
    let first = state.chat_id;
    state.history.push(crate::ollama::Message::new("user", "First chat"));
    state.switch_chat(None).unwrap();
    state.history.push(crate::ollama::Message::new("user", "Second chat"));
    let before = fingerprint(&state);
    state.switch_chat(Some(first)).unwrap();
    assert_eq!(fingerprint(&state), before);
    state.history.push(crate::ollama::Message::new("user", "New message"));
    assert_ne!(fingerprint(&state), before);
}

pub fn want(state: &mut State, why: Want) {
    if let Some(s) = state.space.as_mut() {
        let allowed = match why {
            Want::Now => true,
            Want::Open => s.switches.on_open,
            Want::Send => s.switches.before_send,
        };
        if allowed && !s.links.is_empty() {
            s.wanted = Some(why);
        }
    }
}

fn transports(sync: &SyncState, members: &[String]) -> Result<Vec<(usize, Transport)>> {
    let me = super::peer::Me {
        identity: sync.identity.clone(),
        members: members.iter().filter(|id| !sync.source_paused(id)).cloned().collect(),
        port: super::peer::port().unwrap_or(0),
    };
    sync.links
        .iter()
        .enumerate()
        .filter(|(_, link)| match &link.place {
            Place::Peer { device, .. } => sync.switches.peers && device.as_ref().is_none_or(|id| !sync.source_paused(id)),
            _ => true,
        })
        .map(|(i, l)| {
            Ok((
                i,
                match &l.place {
                    Place::Folder { path } => Transport::Folder(path.clone()),
                    Place::Hosted { url } => Transport::Hosted(Client::new(
                        url,
                        &sync.identity.keys.space,
                        &sync.identity.device,
                    )?),
                    Place::Peer { device, addr } => Transport::Peer(super::peer::Client::new(
                        me.clone(),
                        addr,
                        device.as_deref(),
                    )),
                },
            ))
        })
        .collect()
}

/// Read current routes from the signed sync records and include any fresh LAN discovery.
fn merge_peers(sync: &mut SyncState, space: &Space, discovered: &[(String, String)]) -> bool {
    let mut changed = false;
    for member in space.members() {
        let device = member.id;
        if member.removed.is_some() || device == sync.identity.device.id || sync.source_paused(&device) {
            continue;
        }
        let observed = discovered.iter().find(|(id, _)| *id == device)
            .map_or("", |(_, addr)| addr.as_str());
        let fresh = super::routes::merge(observed, &super::routes::advertised(space, &device));
        if fresh.is_empty() {
            continue;
        }
        if let Some(addr) = sync.links.iter_mut().find_map(|link| match &mut link.place {
            Place::Peer { device: Some(id), addr } if *id == device => Some(addr),
            _ => None,
        }) {
            let merged = super::routes::merge(&fresh, addr);
            changed |= merged != *addr;
            *addr = merged;
        } else if !sync.links.iter().any(|link| matches!(&link.place,
            Place::Peer { device: None, addr } if addr.split(',').any(|a| fresh.split(',').any(|b| b == a.trim()))
        )) {
            sync.links.push(Link {
                place: Place::Peer { device: Some(device), addr: fresh },
                progress: Default::default(),
            });
            changed = true;
        }
    }
    changed
}

/// Pin a manually entered name (or an old invite) only after a successful handshake.
async fn bind_peer_links(sync: &mut SyncState, transports: &[(usize, Transport)]) {
    for (i, transport) in transports {
        if let Transport::Peer(peer) = transport
            && let Some(Link { place: Place::Peer { device, .. }, .. }) = sync.links.get_mut(*i)
            && device.is_none()
        {
            *device = peer.connected_device().await;
        }
    }
    let mut links: Vec<Link> = Vec::with_capacity(sync.links.len());
    for link in std::mem::take(&mut sync.links) {
        if let Place::Peer { device: Some(id), addr } = &link.place
            && let Some(saved) = links.iter_mut().find_map(|known| match &mut known.place {
                Place::Peer { device: Some(known), addr } if known == id => Some(addr),
                _ => None,
            })
        {
            *saved = super::routes::merge(saved, addr);
            continue;
        }
        links.push(link);
    }
    sync.links = links;
}

/// Addresses of devices no longer on the list go; with no other devices left, so do
/// addresses not yet tied to a device (from a pairing that came to nothing). True when any
/// went.
fn prune_peers(s: &mut SyncState, members: &[String]) -> bool {
    let me = s.identity.device.id.clone();
    let others: Vec<&String> = members.iter().filter(|m| **m != me).collect();
    let before = s.links.len();
    s.links.retain(|l| match &l.place {
        Place::Peer {
            device: Some(d), ..
        } => others.contains(&d),
        Place::Peer { device: None, .. } => !others.is_empty(),
        _ => true,
    });
    s.links.len() != before
}

/// How a device on the network is shown: its name when known.
pub fn peer_label(state: &State, device: Option<&str>, addr: &str) -> String {
    let name = device.and_then(|d| {
        devices(state)
            .into_iter()
            .find(|(id, ..)| id == d)
            .map(|(_, n, ..)| n)
    });
    match name {
        Some(n) => format!("{n} on your network ({addr})"),
        None => format!("Device at {addr}"),
    }
}

/// When listening for peers last failed to start, so it isn't retried every moment.
static PEER_TRIED: Mutex<Option<Instant>> = Mutex::new(None);

/// Add a device's network address by hand (when announcements don't get through).
pub fn add_peer(state: &mut State, addr: &str) -> Result<String> {
    let addr = super::routes::endpoint(addr, super::peer::PORT)
        .context("Enter a DNS name or IP, with an optional :port")?;
    let sync = state.space.as_mut().context("Set up sync first")?;
    if !sync
        .links
        .iter()
        .any(|l| matches!(&l.place, Place::Peer { addr: a, .. } if a.split(',').any(|saved| saved.trim() == addr)))
    {
        sync.links.push(Link {
            place: Place::Peer {
                device: None,
                addr: addr.clone(),
            },
            progress: Default::default(),
        });
    }
    sync.wanted = Some(Want::Now);
    Ok(format!("Added the device at {addr}"))
}

pub fn peer_dns(state: &mut State, input: &str) -> Result<String> {
    let name = input.trim().trim_end_matches('.');
    anyhow::ensure!(name.is_empty() || super::provider_sync::valid_hostname(name),
        "Enter a DNS name without a port, or leave it blank for automatic discovery");
    let sync = state.space.as_mut().context("Set up sync first")?;
    sync.peer_dns = name.to_ascii_lowercase();
    sync.wanted = Some(Want::Now);
    Ok("Sync DNS name saved".into())
}

pub fn local_peer_endpoints(port: u16, override_name: &str) -> String {
    super::routes::local_endpoints(port, override_name)
}

/// This computer's name, the default name for the device.
pub fn hostname() -> String {
    #[cfg(target_os = "android")]
    if let Some(model) = crate::android::device_name() {
        return model.chars().take(64).collect();
    }
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        "This device".into()
    } else {
        name.chars().take(64).collect()
    }
}

pub fn provider_hostname(state: &mut State, input: &str) -> Result<String> {
    let name = input.trim().trim_end_matches('.');
    anyhow::ensure!(name.is_empty() || super::provider_sync::valid_hostname(name),
        "Enter a network hostname, for example model-host");
    let sync = state.space.as_mut().context("Set up sync first")?;
    sync.provider_host = name.to_ascii_lowercase();
    sync.wanted = Some(Want::Now);
    Ok("Provider hostname saved".into())
}

/// Rename this device in the space.
pub fn rename_device(state: &mut State, name: &str) -> Result<String> {
    let sync = state.space.as_ref().context("Sync isn't set up")?;
    let mut g = LIVE.lock().unwrap();
    let live = g.get_or_insert_with(|| Live {
        root: crate::runtime::data_dir(),
        space: None,
        jobs: vec![],
        next: None,
    });
    if live.space.is_none() {
        live.space = Some(Space::open(
            sync.identity.clone(),
            Some(store_dir(&live.root, &sync.identity.keys.space)),
        )?);
    }
    let space = live.space.as_mut().unwrap();
    space.rename(name)?;
    space.save()?;
    let id = space.id.clone();
    drop(g);
    let s = state.space.as_mut().unwrap();
    let shown = id.device.name.clone();
    s.identity = id;
    s.wanted = Some(Want::Now);
    Ok(format!("This device is now {shown}"))
}

/// Start syncing: a new space with this device in it.
pub fn create(state: &mut State, name: &str) -> Result<String> {
    let name = if name.trim().is_empty() {
        hostname()
    } else {
        name.trim().to_owned()
    };
    let name = name.as_str();
    anyhow::ensure!(
        state.space.is_none(),
        "Sync is already set up on this device"
    );
    let dir = crate::runtime::data_dir();
    let tmp = Space::create(name, None)?;
    let space = Space {
        dir: Some(store_dir(&dir, &tmp.id.keys.space)),
        ..tmp
    };
    space.save()?;
    state.space = Some(SyncState::new(space.id.clone()));
    let mut g = LIVE.lock().unwrap();
    *g = Some(Live {
        root: dir,
        space: Some(space),
        jobs: vec![],
        next: None,
    });
    Ok("Sync is set up. Add your other devices with /sync pair (Settings › Sync › Add a device): on this network they sync directly, with no server.".into())
}

/// Use keys received by pairing or sign-in.
pub fn adopt(state: &mut State, identity: Identity, links: Vec<Place>) -> Result<()> {
    let dir = crate::runtime::data_dir();
    let space = Space::open(
        identity.clone(),
        Some(store_dir(&dir, &identity.keys.space)),
    )?;
    let mut sync = super::adapter::reconnect(state, identity);
    for place in links {
        sync.links.push(Link {
            place,
            progress: Default::default(),
        });
    }
    sync.wanted = Some(Want::Now);
    state.space = Some(sync);
    *LIVE.lock().unwrap() = Some(Live {
        root: dir,
        space: Some(space),
        jobs: vec![],
        next: None,
    });
    Ok(())
}

/// Whether a round is due now: asked for, or the background interval passed.
pub fn due(state: &State) -> Option<Want> {
    let g = LIVE.lock().unwrap();
    let pending_join = g
        .as_ref()
        .is_some_and(|l| l.jobs.iter().any(|j| matches!(j, Job::Join { .. })));
    if pending_join {
        return Some(Want::Now);
    }
    let sync = state.space.as_ref()?;
    if sync.paused && sync.wanted != Some(Want::Now) {
        return None;
    }
    if let Some(w) = sync.wanted {
        return Some(w);
    }
    if sync.switches.background && WAKE.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return Some(Want::Now);
    }
    // Peer to peer: start listening (the round does it), and sync when another device sent
    // something or showed up.
    if sync.switches.peers {
        if super::peer::port().is_none() {
            let mut tried = PEER_TRIED.lock().unwrap();
            if tried.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                *tried = Some(Instant::now());
                return Some(Want::Now);
            }
        } else if super::peer::WAKE.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return Some(Want::Now);
        }
    }
    if g.as_ref().is_some_and(|l| !l.jobs.is_empty()) {
        return Some(Want::Now);
    }
    if !sync.switches.background || sync.links.is_empty() {
        return None;
    }
    match g.as_ref().and_then(|l| l.next) {
        Some(t) if Instant::now() < t => None,
        _ => Some(Want::Now),
    }
}

/// Run one round. Returns a notice for the window, if any.
pub async fn run(state: &mut State, why: Want) -> Option<String> {
    let mut live = {
        let mut g = LIVE.lock().unwrap();
        g.take().unwrap_or(Live {
            root: crate::runtime::data_dir(),
            space: None,
            jobs: vec![],
            next: None,
        })
    };
    let notice = run_with(state, &mut live, why).await;
    if std::env::var_os("SPINDLE_DEBUG_SYNC").is_some() {
        eprintln!(
            "sync round ({why:?}): {} · links {}",
            notice.as_deref().unwrap_or("-"),
            state.space.as_ref().map(|s| s.links.len()).unwrap_or(0)
        );
    }
    changes_seen(state);
    // A device that left (or was removed) during that round: drop its address now rather
    // than failing to reach it until the next round, and the error with it.
    if let (Some(space), Some(s)) = (live.space.as_ref(), state.space.as_mut()) {
        let members: Vec<String> = space
            .members()
            .into_iter()
            .filter(|m| m.removed.is_none())
            .map(|m| m.id)
            .collect();
        if prune_peers(s, &members) && s.links.is_empty() {
            s.status.error = None;
        }
    }
    // Another device off or closed (a phone): look again soon, so a phone opened for a
    // minute catches up even when its announcements don't get through (a firewall).
    let soon = state.space.as_ref().is_some_and(|s| {
        !s.status.waiting.is_empty()
            && s.links.iter().any(|l| matches!(l.place, Place::Peer { .. }))
    });
    live.next = Some(Instant::now() + Duration::from_secs(if soon { 20 } else { 120 }));
    if let Some(s) = state.space.as_mut() {
        s.wanted = None;
    }
    {
        let mut g = LIVE.lock().unwrap();
        // Changed while this round ran (a device paired in, one removed, renamed, a job
        // queued): those went to a copy of the space opened meanwhile and saved to disk.
        // Keep its jobs, and read the space from disk again so the change isn't lost: else
        // a newly paired device stays off this device's list (and is turned away) until
        // Spindle restarts.
        if let Some(meanwhile) = g.take() {
            live.jobs.extend(meanwhile.jobs);
            if meanwhile.space.is_some() {
                live.space = None;
                live.next = None;
            }
        }
        *g = Some(live);
    }
    // Leaving: the goodbye went out with that round (or couldn't): stop now.
    if LEAVING.lock().unwrap().is_some() && state.space.is_some() {
        let told = state
            .space
            .as_ref()
            .is_some_and(|s| s.status.error.is_none());
        finish_leaving(state);
        return Some(if told {
            "This device no longer syncs; your other devices have been told. Its chats and settings stay here.".into()
        } else {
            "This device no longer syncs. Your other devices couldn't be reached: remove it there (Settings › Sync › Devices). Its chats and settings stay here.".into()
        });
    }
    notice
}

async fn run_with(state: &mut State, live: &mut Live, why: Want) -> Option<String> {
    let mut notice = None;
    // Joining comes first: there's no space here yet.
    if let Some(i) = live.jobs.iter().position(|j| matches!(j, Job::Join { .. })) {
        let Job::Join {
            url,
            user,
            password,
            name,
        } = live.jobs.remove(i)
        else {
            unreachable!()
        };
        let result = async {
            let device = Device::generate(&name)?;
            let keys = Client::join_with_account(&url, &user, &password, &device).await?;
            let identity = Identity {
                secrets: !keys.secrets.is_empty(),
                keys,
                device,
            };
            let mut space = Space::open(
                identity.clone(),
                Some(store_dir(&live.root, &identity.keys.space)),
            )?;
            // The device adds itself (it holds the key; the server registered it at sign-in).
            let me = space.member_entry()?;
            space.publish_self(me)?;
            Ok::<_, anyhow::Error>((identity, space))
        }
        .await;
        match result {
            Ok((identity, space)) => {
                let mut sync = super::adapter::reconnect(state, identity);
                sync.links.push(Link {
                    place: Place::Hosted { url },
                    progress: Default::default(),
                });
                state.space = Some(sync);
                live.space = Some(space);
                notice = Some("Joined. Syncing…".into());
            }
            Err(e) => {
                let msg = format!("Couldn't join: {e:#}");
                *LAST.lock().unwrap() = msg.clone();
                return Some(msg);
            }
        }
    }
    let sync = state.space.as_ref()?;
    if live.space.is_none() {
        match Space::open(
            sync.identity.clone(),
            Some(store_dir(&live.root, &sync.identity.keys.space)),
        ) {
            Ok(s) => live.space = Some(s),
            Err(e) => return Some(format!("Couldn't open sync: {e:#}")),
        }
    }
    let space = live.space.as_mut().unwrap();
    // Keep the identity in step with the space (new keys after a device was removed).
    if let Some(s) = state.space.as_mut() {
        s.identity = space.id.clone();
    }
    // Peer to peer: listen with the current device list, take in what peers sent, and
    // add the devices heard on the network.
    let members: Vec<String> = space
        .members()
        .into_iter()
        .filter(|m| m.removed.is_none())
        .map(|m| m.id)
        .collect();
    // Removed by another device (Remove pressed there): the others turn it away, and what
    // it adds (a device it pairs with) doesn't count. Say so instead of failing to reach.
    let me_id = space.id.device.id.clone();
    if !members.contains(&me_id) && !LEAVING.lock().unwrap().is_some() {
        let msg = "Another of your devices removed this one, so the others turn it away. Use Stop syncing here, then pair it again.".to_string();
        if let Some(s) = state.space.as_mut() {
            if s.status.error.as_deref() != Some(msg.as_str()) {
                s.log(msg.clone());
            }
            s.status.error = Some(msg.clone());
            s.status.syncing = false;
        }
        return Some(msg);
    }
    let peers = state
        .space
        .as_ref()
        .is_some_and(|s| s.switches.peers && !s.paused);
    if peers {
        if let Some(dir) = space.dir.clone()
            && let Err(e) = super::peer::serve(&space.id, &dir, Some(members.clone()))
            && let Some(s) = state.space.as_mut()
        {
            s.log(format!("Couldn't listen for your other devices: {e:#}"));
        }
        if let Some(s) = state.space.as_mut() {
            super::routes::refresh_dns().await;
            merge_peers(s, space, &super::peer::discovered());
        }
    } else {
        super::peer::stop();
    }
    if let Some(s) = state.space.as_mut() {
        prune_peers(s, &members);
        let addresses = super::peer::port()
            .filter(|_| peers)
            .map_or_else(String::new, |p| local_peer_endpoints(p, &s.peer_dns));
        if let Err(err) = super::routes::publish(space, &addresses) {
            s.log(format!("Could not publish sync addresses: {err:#}"));
        }
        s.status.local_endpoints = addresses;
    }
    if let Some(dir) = space.dir.clone() {
        let arrived = super::peer::take_inbox(&dir);
        if !arrived.is_empty() {
            let _ = space.receive(arrived);
        }
    }
    let ts = match transports(state.space.as_ref().unwrap(), &members) {
        Ok(t) => t,
        Err(e) => return Some(format!("Couldn't sync: {e:#}")),
    };
    if state
        .space
        .as_ref()
        .is_some_and(|s| s.switches.background && !s.paused)
    {
        for (_, t) in &ts {
            if let Transport::Hosted(c) = t {
                watch(c);
            }
        }
    }
    // Network work between rounds.
    for job in std::mem::take(&mut live.jobs) {
        let r = async {
            match &job {
                Job::Register { url, admin } => {
                    Client::new(url, &space.id.keys.space, &space.id.device)?
                        .create_space(admin)
                        .await
                }
                Job::Account { user, password } => {
                    let url = hosted_url(state).context("Add a server first")?;
                    Client::new(&url, &space.id.keys.space, &space.id.device)?
                        .set_account(user, password, &space.keys_for(true))
                        .await
                }
                Job::AddDevice { device } => {
                    for (_, t) in &ts {
                        if let Transport::Hosted(c) = t {
                            c.add_device(device).await?;
                        }
                    }
                    Ok(())
                }
                Job::Join { .. } => Ok(()),
            }
        }
        .await;
        let label = match &job {
            Job::Register { .. } => "Registering with the server",
            Job::Account { .. } => "Saving the account",
            Job::Join { .. } => "Joining",
            Job::AddDevice { .. } => "Registering the new device with the server",
        };
        let line = match r {
            Ok(()) => format!("{label}: done"),
            Err(e) => format!("{label}: {e:#}"),
        };
        if let Some(s) = state.space.as_mut() {
            s.log(line.clone());
        }
        notice = Some(line);
    }
    if let Some(sync) = state.space.as_mut() {
        for (id, urls) in sync.pending_removals.clone() {
            for url in urls {
                let result = async {
                    Client::new(&url, &space.id.keys.space, &space.id.device)?
                        .remove_device(&id).await
                }.await;
                match result {
                    Ok(()) => {
                        if let Some(pending) = sync.pending_removals.get_mut(&id) {
                            pending.remove(&url);
                        }
                    }
                    Err(e) => sync.log(format!("Device removal pending on {url}: {e:#}")),
                }
            }
        }
        sync.pending_removals.retain(|_, urls| !urls.is_empty());
    }
    if let Some(s) = state.space.as_mut() {
        s.status.syncing = true;
    }
    match round(state, space, &ts, why == Want::Send).await {
        Ok((r, _)) if r > 0 && notice.is_none() => {
            notice = Some(format!(
                "Synced {r} change{}",
                if r == 1 { "" } else { "s" }
            ))
        }
        Ok(_) => {}
        Err(e) => {
            if let Some(s) = state.space.as_mut() {
                s.status.syncing = false;
                s.status.error = Some(format!("{e:#}"));
            }
        }
    }
    if let Some(s) = state.space.as_mut() {
        s.identity = space.id.clone();
        if peers {
            bind_peer_links(s, &ts).await;
            if merge_peers(s, space, &super::peer::discovered()) {
                super::peer::WAKE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    notice
}

fn hosted_url(state: &State) -> Option<String> {
    state
        .space
        .as_ref()?
        .links
        .iter()
        .find_map(|l| match &l.place {
            Place::Hosted { url } => Some(url.clone()),
            _ => None,
        })
}

/// Where projects from other devices go on this one. Checked by making the folder and
/// writing in it; chats waiting for their project are brought in next round.
pub fn projects_folder(state: &mut State, path: &str) -> Result<String> {
    let path = path.trim().trim_matches('"');
    anyhow::ensure!(!path.is_empty(), "Give a folder");
    let dir = std::path::PathBuf::from(path);
    anyhow::ensure!(dir.is_absolute(), "Give the folder's full path");
    std::fs::create_dir_all(&dir).with_context(|| format!("Couldn't make {}", dir.display()))?;
    let probe = dir.join(".spindle-write-test");
    std::fs::write(&probe, b"ok")
        .with_context(|| format!("Spindle can't write in {}", dir.display()))?;
    let _ = std::fs::remove_file(probe);
    state.preferences.projects_dir = dir.display().to_string();
    if let Some(s) = state.space.as_mut() {
        s.seen.retain(|k, _| !k.starts_with("chatview:"));
        s.wanted = Some(Want::Now);
    }
    Ok(format!(
        "Projects from your other devices go in {}",
        dir.display()
    ))
}

/// A suggested projects folder, somewhere the person can find it.
pub fn suggested_projects_folder() -> String {
    #[cfg(target_os = "android")]
    {
        "/storage/emulated/0/Documents/Spindle".into()
    }
    #[cfg(not(target_os = "android"))]
    {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        std::path::Path::new(&home)
            .join("Documents")
            .join("Spindle Projects")
            .display()
            .to_string()
    }
}

/// Set while this device says goodbye: one more round tells the others it left.
static LEAVING: Mutex<Option<Instant>> = Mutex::new(None);

/// Stop syncing on this device. Its chats, projects and settings stay here; it can start
/// again, or join another device, at any time. First, where other devices are on the list,
/// it marks itself removed and syncs once, so they stop syncing with it and drop its
/// addresses; if none can be reached it stops anyway (they can remove it themselves).
pub fn leave(state: &mut State) -> Result<String> {
    anyhow::ensure!(state.space.is_some(), "This device doesn't sync");
    let others = devices(state)
        .into_iter()
        .filter(|(_, _, me, removed)| !me && !removed)
        .count();
    if others > 0 && !state.space.as_ref().is_some_and(|s| s.links.is_empty()) {
        let sync = state.space.as_ref().unwrap();
        let mut g = LIVE.lock().unwrap();
        let live = g.get_or_insert_with(|| Live {
            root: crate::runtime::data_dir(),
            space: None,
            jobs: vec![],
            next: None,
        });
        if live.space.is_none() {
            live.space = Some(Space::open(
                sync.identity.clone(),
                Some(store_dir(&live.root, &sync.identity.keys.space)),
            )?);
        }
        if live.space.as_mut().unwrap().leave().is_ok() {
            drop(g);
            *LEAVING.lock().unwrap() = Some(Instant::now());
            let s = state.space.as_mut().unwrap();
            s.paused = false;
            s.wanted = Some(Want::Now);
            return Ok("Telling your other devices this one is leaving…".into());
        }
    }
    finish_leaving(state);
    Ok("This device no longer syncs. Its chats and settings stay here.".into())
}

/// The last step of leaving: forget the space here.
fn finish_leaving(state: &mut State) {
    *LEAVING.lock().unwrap() = None;
    super::peer::stop();
    if let Some(h) = HOST.lock().unwrap().take() {
        h.cancel();
    }
    if let Some(j) = JOIN.lock().unwrap().take() {
        j.task.abort();
    }
    *LIVE.lock().unwrap() = None;
    super::adapter::disconnect(state);
}

/// Remove a device: a new device list and new keys now, the server told next round.
pub fn remove_device(state: &mut State, device: &str) -> Result<String> {
    let sync = state.space.as_ref().context("Sync isn't set up")?;
    let mut g = LIVE.lock().unwrap();
    let live = g.get_or_insert_with(|| Live {
        root: crate::runtime::data_dir(),
        space: None,
        jobs: vec![],
        next: None,
    });
    if live.space.is_none() {
        live.space = Some(Space::open(
            sync.identity.clone(),
            Some(store_dir(&live.root, &sync.identity.keys.space)),
        )?);
    }
    let space = live.space.as_mut().unwrap();
    let name = space
        .members()
        .into_iter()
        .find(|m| m.id == device && m.removed.is_none())
        .map(|m| m.name)
        .context("No such device")?;
    space.remove_member(device)?;
    let id = space.id.clone();
    let members: Vec<String> = space.members().into_iter()
        .filter(|m| m.removed.is_none()).map(|m| m.id).collect();
    drop(g);
    let s = state.space.as_mut().unwrap();
    s.identity = id;
    let urls = s.links.iter().filter_map(|link| match &link.place {
        Place::Hosted { url } => Some(url.clone()),
        _ => None,
    }).collect();
    s.pending_removals.insert(device.to_owned(), urls);
    prune_peers(s, &members);
    s.wanted = Some(Want::Now);
    Ok(format!(
        "Removed {name}. It keeps what it already had, but gets nothing new."
    ))
}

/// The devices in the space: (id, name, is this device, removed).
pub fn devices(state: &State) -> Vec<(String, String, bool, bool)> {
    let Some(sync) = state.space.as_ref() else {
        return vec![];
    };
    let g = LIVE.lock().unwrap();
    let list = match g.as_ref().and_then(|l| l.space.as_ref()) {
        Some(space) => space.members(),
        None => Space::open(
            sync.identity.clone(),
            Some(store_dir(
                &crate::runtime::data_dir(),
                &sync.identity.keys.space,
            )),
        )
        .map(|s| s.members())
        .unwrap_or_default(),
    };
    list.into_iter()
        .map(|m| {
            let me = m.id == sync.identity.device.id;
            (m.id, m.name, me, m.removed.is_some())
        })
        .collect()
}

pub fn set_device_rule(state: &mut State, id: &str, option: &str, on: bool) -> Result<String> {
    anyhow::ensure!(devices(state).iter().any(|(device, _, me, removed)| device == id && !me && !removed), "Choose a paired device");
    let sync = state.space.as_mut().context("Set up sync first")?;
    let mut baseline = std::collections::BTreeSet::new();
    if sync.device_rules.is_empty() && !on {
        let guard = LIVE.lock().unwrap();
        if let Some(space) = guard.as_ref().and_then(|live| live.space.as_ref()) {
            baseline.extend(space.store.bodies.keys().cloned());
        } else {
            let data = crate::runtime::data_dir();
            let space = Space::open(sync.identity.clone(), Some(store_dir(&data, &sync.identity.keys.space)))?;
            baseline.extend(space.store.bodies.keys().cloned());
        }
    }
    sync.set_device_rule(id, option, on, baseline)?;
    Ok("Device permissions saved".into())
}

/// Add a device after pairing (the new device's entry, with its keys already handed over).
pub fn add_device(state: &mut State, member: super::core::Member) -> Result<()> {
    let sync = state.space.as_ref().context("Sync isn't set up")?;
    let mut g = LIVE.lock().unwrap();
    let live = g.get_or_insert_with(|| Live {
        root: crate::runtime::data_dir(),
        space: None,
        jobs: vec![],
        next: None,
    });
    if live.space.is_none() {
        live.space = Some(Space::open(
            sync.identity.clone(),
            Some(store_dir(&live.root, &sync.identity.keys.space)),
        )?);
    }
    live.space.as_mut().unwrap().add_member(member)?;
    live.next = None;
    Ok(())
}

/// The records this device has, for handing to a new device during pairing.
pub fn records(state: &State) -> Vec<Vec<u8>> {
    let Some(sync) = state.space.as_ref() else {
        return vec![];
    };
    let g = LIVE.lock().unwrap();
    let from = |s: &Space| {
        s.store
            .records
            .values()
            .filter_map(|e| e.bytes().ok())
            .collect()
    };
    match g.as_ref().and_then(|l| l.space.as_ref()) {
        Some(s) => from(s),
        None => Space::open(
            sync.identity.clone(),
            Some(store_dir(
                &crate::runtime::data_dir(),
                &sync.identity.keys.space,
            )),
        )
        .map(|s| from(&s))
        .unwrap_or_default(),
    }
}

/// Read a sync-on/off word.
pub fn on_off(word: &str) -> Result<Option<bool>> {
    Ok(match word {
        "on" | "yes" | "true" => Some(true),
        "off" | "no" | "false" => Some(false),
        "global" | "default" | "inherit" => None,
        _ => anyhow::bail!("Use on, off or global"),
    })
}

pub fn folder_link(path: &Path) -> Result<Place> {
    let path = path
        .canonicalize()
        .with_context(|| format!("{} doesn't exist", path.display()))?;
    anyhow::ensure!(path.is_dir(), "Choose a folder");
    Ok(Place::Folder { path })
}

// ------------------------------------------------------------------ pairing

struct Joining {
    number: Arc<Mutex<Option<u32>>>,
    task: tokio::task::JoinHandle<Result<(super::pair::Joined, Device, Option<String>)>>,
}

static HOST: Mutex<Option<super::pair::Host>> = Mutex::new(None);
static JOIN: Mutex<Option<Joining>> = Mutex::new(None);

use std::sync::Arc;

/// Start adding a device: returns the QR code, the link and the typed code.
pub fn pair_start(state: &State, short: bool) -> Result<String> {
    let sync = state.space.as_ref().context("Set up sync first")?;
    let relay = hosted_url(state);
    // Only servers travel with the keys: folders are paths on this device.
    let links: Vec<String> = sync
        .links
        .iter()
        .filter_map(|l| match &l.place {
            Place::Hosted { url } => Some(url.clone()),
            _ => None,
        })
        .chain(
            super::peer::port()
                .map(|port| local_peer_endpoints(port, &sync.peer_dns))
                .filter(|list| !list.is_empty())
                .map(|list| format!("peer:{list}")),
        )
        .collect();
    let host = super::pair::start(
        sync.identity.keys.clone(),
        serde_json::json!(links),
        records(state),
        relay,
        true,
        short,
        &sync.peer_dns,
    )?;
    let v = host.view.lock().unwrap().clone();
    if let Some(old) = HOST.lock().unwrap().replace(host) {
        old.cancel();
    }
    Ok(format!(
        "On the new device, scan this code with Spindle, or run `spindle sync join` with the link or the typed code.\n\n{}\nLink: {}\nCode: {}\n\nThe new device then shows a number from 1 to 100: type it here with /sync pair NUMBER.",
        super::pair::qr_text(&v.link).unwrap_or_default(),
        v.link,
        v.typed
    ))
}

/// What the existing device's pairing shows now.
pub fn pair_view() -> Option<super::pair::HostView> {
    HOST.lock()
        .unwrap()
        .as_ref()
        .map(|h| h.view.lock().unwrap().clone())
}

/// The number typed from the new device's screen, or `None` to cancel.
pub fn pair_answer(number: Option<u32>) -> Result<String> {
    let mut g = HOST.lock().unwrap();
    let h = g.as_mut().context("No pairing in progress")?;
    let Some(n) = number else {
        h.answer(None);
        g.take().map(|h| h.cancel());
        return Ok("Pairing cancelled".into());
    };
    anyhow::ensure!(
        h.view.lock().unwrap().asking,
        "The new device hasn't connected yet; type the number once it shows one"
    );
    h.answer(Some(n));
    Ok(format!("Checking {n}…"))
}

/// Join from the new device with a link, or a typed code and where to connect.
pub fn join_start(state: &State, input: &str, place: Option<&str>, name: &str) -> Result<String> {
    // A device that syncs only with itself so far may join another device's space instead;
    // its own chats and settings go along.
    anyhow::ensure!(
        state.space.is_none() || devices(state).iter().filter(|(_, _, _, removed)| !removed).count() <= 1,
        "This device already syncs with other devices"
    );
    let invite = super::pair::Invite::parse(input, place)?;
    let device = Device::generate(name)?;
    let number = Arc::new(Mutex::new(None));
    let d2 = number.clone();
    let relay = invite.relay.clone();
    let task = tokio::spawn(async move {
        let joined = super::pair::join(&invite, &device, true, move |n| {
            *d2.lock().unwrap() = Some(n)
        })
        .await?;
        Ok((joined, device, relay))
    });
    *JOIN.lock().unwrap() = Some(Joining { number, task });
    Ok(
        "Connecting… this device will show a number (/sync status); type it on the other device."
            .into(),
    )
}

/// The number the new device shows while joining.
pub fn join_number() -> Option<u32> {
    JOIN.lock()
        .unwrap()
        .as_ref()
        .and_then(|j| *j.number.lock().unwrap())
}

/// Finish pairing on either side when it's done. Returns a notice.
pub async fn pair_poll(state: &mut State) -> Option<String> {
    // The existing device: add the new one.
    let done = HOST
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|h| h.view.lock().unwrap().done.clone());
    if let Some(result) = done {
        HOST.lock().unwrap().take();
        return Some(match result {
            Ok(member) => {
                let id = member.id.clone();
                let name = member.name.clone();
                match add_device(state, member) {
                    Ok(()) => {
                        queue(Job::AddDevice { device: id });
                        want(state, Want::Now);
                        format!("Added {name}")
                    }
                    Err(e) => format!("Couldn't add the device: {e:#}"),
                }
            }
            Err(e) => format!("Pairing: {e}"),
        });
    }
    // The new device: take the keys and records.
    let finished = JOIN
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|j| j.task.is_finished());
    if finished {
        let j = JOIN.lock().unwrap().take()?;
        let result = j
            .task
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .and_then(|r| r);
        return Some(match result {
            Ok((joined, device, relay)) => {
                let identity = Identity {
                    secrets: !joined.keys.secrets.is_empty(),
                    keys: joined.keys,
                    device,
                };
                let given: Vec<&str> = joined
                    .links
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|u| u.as_str())
                    .collect();
                let mut links: Vec<Place> = given
                    .iter()
                    .find(|u| !u.starts_with("peer:"))
                    .map(|u| Place::Hosted {
                        url: (*u).to_owned(),
                    })
                    .or(relay.map(|url| Place::Hosted { url }))
                    .into_iter()
                    .collect();
                // The device it paired with, directly on the network.
                links.extend(given.iter().filter_map(|u| {
                    u.strip_prefix("peer:").map(|a| Place::Peer {
                        device: None,
                        addr: a.to_owned(),
                    })
                }));
                match adopt(state, identity, links) {
                    Ok(()) => {
                        let mut g = LIVE.lock().unwrap();
                        if let Some(space) = g.as_mut().and_then(|l| l.space.as_mut()) {
                            let _ = space.receive(joined.records);
                            if let Ok(me) = space.member_entry() {
                                let _ = space.publish_self(me);
                            }
                        }
                        "Joined. Syncing…".into()
                    }
                    Err(e) => format!("Couldn't join: {e:#}"),
                }
            }
            Err(e) => {
                let msg = format!("Couldn't join: {e:#}");
                *LAST.lock().unwrap() = msg.clone();
                msg
            }
        });
    }
    None
}

/// `/sync check`: what this device knows about the others and whether each address answers,
/// in words someone can paste back for help.
pub fn check(state: &State) -> String {
    let Some(sync) = state.space.as_ref() else {
        return "Sync isn't set up on this device.".into();
    };
    let mut out = vec![format!(
        "Spindle {} · this device: {} ({})",
        crate::version(),
        sync.identity.device.name,
        super::peer::short(&sync.identity.device.id)
    )];
    out.push(format!(
        "Peer to peer: {}{} · listening on {} · hears announcements: {}",
        if sync.switches.peers { "on" } else { "off" },
        if sync.paused { " (paused)" } else { "" },
        super::peer::port()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "nothing (not started)".into()),
        if super::peer::HEARS.load(std::sync::atomic::Ordering::Relaxed) {
            "yes"
        } else {
            "no"
        }
    ));
    out.push(format!(
        "This device's IP and DNS candidates: {}",
        super::peer::port().map(|port| local_peer_endpoints(port, &sync.peer_dns).replace(',', ", "))
            .unwrap_or_else(|| "listener not started".into())
    ));
    let members = devices(state);
    out.push(format!("Devices in this space ({}):", members.len()));
    for (id, name, me, removed) in &members {
        out.push(format!(
            "  {name} ({}){}{}",
            super::peer::short(id),
            if *me { " · this one" } else { "" },
            if *removed { " · removed" } else { "" }
        ));
    }
    let heard = super::peer::discovered();
    out.push(format!(
        "Heard on the network just now: {}",
        if heard.is_empty() {
            "none".to_string()
        } else {
            heard
                .iter()
                .map(|(d, a)| format!("{} at {a}", super::peer::short(d)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    let current: Vec<String> = members
        .iter()
        .filter(|m| !m.3)
        .map(|m| m.0.clone())
        .collect();
    let me = super::peer::Me {
        identity: sync.identity.clone(),
        members: current,
        port: super::peer::port().unwrap_or(0),
    };
    let peers: Vec<(Option<String>, String)> = sync
        .links
        .iter()
        .filter_map(|l| match &l.place {
            Place::Peer { device, addr } => Some((device.clone(), addr.clone())),
            _ => None,
        })
        .collect();
    out.push(format!("Links ({}):", sync.links.len()));
    for l in &sync.links {
        out.push(format!(
            "  {}",
            match &l.place {
                Place::Folder { path } => format!("folder {}", path.display()),
                Place::Hosted { url } => format!("server {url}"),
                Place::Peer { device, addr } => format!(
                    "peer {} at {addr}{}",
                    device
                        .as_deref()
                        .map(super::peer::short)
                        .unwrap_or_else(|| "(not known yet)".into()),
                    match device {
                        Some(d) if !members.iter().any(|m| &m.0 == d && !m.3) =>
                            " · NOT in the device list",
                        _ => "",
                    }
                ),
            }
        ));
    }
    // Try every address now, each on its own, off the window's thread.
    let tries = std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return vec!["Couldn't start the check".to_string()];
        };
        rt.block_on(async {
            let mut lines = vec![];
            for (device, addr) in peers {
                for a in addr.split(',').map(str::trim).filter(|a| !a.is_empty()) {
                    let one = super::peer::connect(&me, a, device.as_deref()).await;
                    lines.push(match one {
                        Ok(c) => format!("  {a}: OK, {} answered", super::peer::short(&c.device)),
                        Err(e) if super::peer::offline(&e) => {
                            format!("  {a}: nothing answered (see below)")
                        }
                        Err(e) => format!("  {a}: {e:#}"),
                    });
                }
            }
            lines
        })
    })
    .join()
    .unwrap_or_default();
    if !tries.is_empty() {
        out.push("Trying each address now:".into());
        out.extend(tries);
    }
    out.push(match (&sync.status.error, sync.status.last_ok) {
        (Some(e), _) => format!("Last round: {e}"),
        (None, Some(t)) => format!("Last synced {}", crate::automation::local_time(t)),
        (None, None) => "Never synced yet".into(),
    });
    if !sync.status.waiting.is_empty() {
        out.push(format!("Waiting for: {}", sync.status.waiting.join(", ")));
    }
    #[cfg(windows)]
    out.push(firewall_report());
    out.push("Recent connections:".into());
    let ev = super::peer::events();
    if ev.is_empty() {
        out.push("  none since Spindle started".into());
    }
    out.extend(ev.into_iter().map(|e| format!("  {e}")));
    out.push("Activity:".into());
    out.extend(
        sync.activity
            .iter()
            .rev()
            .take(8)
            .map(|a| format!("  {a}")),
    );
    out.join("\n")
}

/// Windows: what the firewall does with Spindle, and which kind of network this is (a
/// "Public" network blocks other devices from reaching it unless a rule allows it).
#[cfg(windows)]
pub fn firewall_report() -> String {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue';\
         Get-NetConnectionProfile | ForEach-Object {{ 'Network: ' + $_.Name + ' (' + $_.InterfaceAlias + ') is ' + $_.NetworkCategory }};\
         $rules = Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -ieq '{}' }} | Get-NetFirewallRule;\
         if (-not $rules) {{ 'Firewall: no rule for this Spindle program' }};\
         $rules | ForEach-Object {{ 'Firewall rule: ' + $_.DisplayName + ' · ' + $_.Direction + ' ' + $_.Action + ' · profiles ' + $_.Profile + ' · enabled ' + $_.Enabled }}",
        exe.replace('\'', "''")
    );
    match std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000)
        .output()
    {
        Ok(o) => format!(
            "Program: {exe}\n{}",
            String::from_utf8_lossy(&o.stdout).trim_end()
        ),
        Err(e) => format!("Couldn't ask Windows about the firewall: {e}"),
    }
}

/// Windows: allow this Spindle through the firewall for your other devices (asks Windows
/// for permission). The connection is still only accepted from your own devices.
#[cfg(windows)]
pub fn allow_firewall() -> Result<String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe()?.display().to_string().replace('\'', "''");
    let inner = format!(
        "Remove-NetFirewallRule -DisplayName 'Spindle sync' -ErrorAction SilentlyContinue;\
         New-NetFirewallRule -DisplayName 'Spindle sync' -Direction Inbound -Action Allow -Program '{exe}' -Protocol TCP -Profile Any | Out-Null;\
         New-NetFirewallRule -DisplayName 'Spindle sync' -Direction Inbound -Action Allow -Program '{exe}' -Protocol UDP -LocalPort {} -Profile Any | Out-Null;\
         Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -ieq '{exe}' }} | Get-NetFirewallRule | Where-Object {{ $_.Action -eq 'Block' }} | Disable-NetFirewallRule",
        super::peer::BEACON_PORT
    );
    // Encoded, so nothing in it is read twice by the outer PowerShell.
    let utf16: Vec<u8> = inner.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let outer = format!(
        "Start-Process powershell.exe -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-EncodedCommand','{}'",
        {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(utf16)
        }
    );
    let o = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .creation_flags(0x0800_0000)
        .output()?;
    anyhow::ensure!(
        o.status.success(),
        "Windows didn't allow it: {}",
        String::from_utf8_lossy(&o.stderr).trim()
    );
    Ok(format!("Done.\n{}", firewall_report()))
}
