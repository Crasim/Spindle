//! Ways to move records between devices, and the protocol that uses them.
//!
//! Every transport offers the same three things: list the record IDs it has (after a cursor),
//! get records by ID, and put records. Sync then fetches what this device lacks and sends
//! what the transport lacks. Records are already encrypted and signed, so a transport never
//! needs to read them.

use super::core::{Envelope, MAX_RECORD};
use super::space::Space;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::{Arc, Mutex};

/// IDs a transport has after a cursor, and the cursor to use next time.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Listing {
    pub ids: Vec<String>,
    pub cursor: String,
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Records shared in memory for transport tests.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct Hub(Arc<Mutex<Vec<(String, Vec<u8>)>>>);

#[derive(Clone)]
pub enum Transport {
    /// In-memory (tests).
    #[cfg(test)]
    Memory(Hub),
    /// A shared folder: OneDrive, Syncthing, a network share…
    Folder(PathBuf),
    Hosted(super::hosted::Client),
    /// Another device on the local network.
    Peer(super::peer::Client),
}

impl Transport {
    pub fn label(&self) -> String {
        match self {
            #[cfg(test)]
            Transport::Memory(_) => "memory".into(),
            Transport::Folder(p) => format!("folder {}", p.display()),
            Transport::Hosted(c) => format!("server {}", c.base),
            Transport::Peer(c) => format!("device at {}", c.addr),
        }
    }

    fn folder(root: &std::path::Path, space: &str) -> PathBuf {
        root.join("spindle-sync").join(space).join("records")
    }

    pub async fn list(&self, space: &str, since: &str) -> Result<Listing> {
        match self {
            #[cfg(test)]
            Transport::Memory(h) => {
                let all = h.0.lock().unwrap();
                let from: usize = since.parse().unwrap_or(0);
                Ok(Listing {
                    ids: all.iter().skip(from).map(|(id, _)| id.clone()).collect(),
                    cursor: all.len().to_string(),
                })
            }
            Transport::Folder(root) => {
                let dir = Self::folder(root, space);
                let mut ids = vec![];
                if let Ok(shards) = std::fs::read_dir(&dir) {
                    for shard in shards.flatten() {
                        for f in std::fs::read_dir(shard.path())?.flatten() {
                            let name = f.file_name().to_string_lossy().into_owned();
                            if let Some(id) = name.strip_suffix(".json")
                                && valid_id(id)
                            {
                                ids.push(id.to_owned());
                            }
                        }
                    }
                }
                // A folder has no order to resume from: list everything, cheaply.
                Ok(Listing {
                    ids,
                    cursor: String::new(),
                })
            }
            Transport::Hosted(c) => c.list(since).await,
            Transport::Peer(c) => c.list().await,
        }
    }

    pub async fn get(&self, space: &str, ids: &[String]) -> Result<Vec<Vec<u8>>> {
        match self {
            #[cfg(test)]
            Transport::Memory(h) => {
                let all = h.0.lock().unwrap();
                Ok(all
                    .iter()
                    .filter(|(id, _)| ids.contains(id))
                    .map(|(_, b)| b.clone())
                    .collect())
            }
            Transport::Folder(root) => {
                let dir = Self::folder(root, space);
                let mut out = vec![];
                for id in ids.iter().filter(|i| valid_id(i)) {
                    let path = dir.join(&id[..2]).join(format!("{id}.json"));
                    if let Ok(meta) = std::fs::metadata(&path)
                        && meta.len() as usize <= MAX_RECORD
                    {
                        out.push(std::fs::read(&path)?);
                    }
                }
                Ok(out)
            }
            Transport::Hosted(c) => c.get(ids).await,
            Transport::Peer(c) => c.get(ids).await,
        }
    }

    pub async fn put(&self, space: &str, records: &[(String, Vec<u8>)]) -> Result<()> {
        match self {
            #[cfg(test)]
            Transport::Memory(h) => {
                let mut all = h.0.lock().unwrap();
                for (id, b) in records {
                    if !all.iter().any(|(x, _)| x == id) {
                        all.push((id.clone(), b.clone()));
                    }
                }
                Ok(())
            }
            Transport::Folder(root) => {
                let dir = Self::folder(root, space);
                for (id, b) in records {
                    ensure!(valid_id(id), "Invalid record ID");
                    let shard = dir.join(&id[..2]);
                    std::fs::create_dir_all(&shard)?;
                    let path = shard.join(format!("{id}.json"));
                    if path.exists() {
                        continue;
                    }
                    let mut tmp = tempfile::NamedTempFile::new_in(&shard)?;
                    std::io::Write::write_all(&mut tmp, b)?;
                    tmp.persist_noclobber(&path)
                        .or_else(|e| {
                            if path.exists() {
                                Ok(std::fs::File::open(&path)?)
                            } else {
                                Err(e.error)
                            }
                        })
                        .context("Writing a record to the sync folder")?;
                }
                Ok(())
            }
            Transport::Hosted(c) => c.put(records).await,
            Transport::Peer(c) => c.put(records).await,
        }
    }
}

/// How far this device has got with one transport.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Progress {
    pub cursor: String,
    /// Records the transport is known to have.
    pub pushed: BTreeSet<String>,
}

/// What one sync did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub received: usize,
    pub sent: usize,
}

/// One round with a transport: fetch what's missing here, send what's missing there.
pub async fn sync(space: &mut Space, t: &Transport, p: &mut Progress) -> Result<Outcome> {
    let sid = space.id.keys.space.clone();
    let listing = t.list(&sid, &p.cursor).await?;
    let missing: Vec<String> = listing
        .ids
        .iter()
        .filter(|id| valid_id(id) && !space.store.records.contains_key(*id))
        .cloned()
        .collect();
    p.pushed.extend(listing.ids.iter().cloned());
    let mut received = 0;
    for batch in missing.chunks(200) {
        let got = t.get(&sid, batch).await?;
        // Keep only records whose content matches the ID asked for.
        let good: Vec<Vec<u8>> = got
            .into_iter()
            .filter(|b| {
                Envelope::parse(b)
                    .and_then(|e| e.id())
                    .is_ok_and(|id| batch.contains(&id))
            })
            .collect();
        received += space.receive(good)?;
    }
    let to_send: Vec<(String, Vec<u8>)> = space
        .store
        .records
        .iter()
        .filter(|(id, _)| !p.pushed.contains(*id))
        .map(|(id, e)| Ok((id.clone(), e.bytes()?)))
        .collect::<Result<_>>()?;
    for batch in to_send.chunks(100) {
        t.put(&sid, batch).await?;
        p.pushed.extend(batch.iter().map(|(id, _)| id.clone()));
    }
    p.cursor = listing.cursor;
    Ok(Outcome {
        received,
        sent: to_send.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::core::Kind;
    use super::super::space::*;
    use super::*;
    use serde_json::json;

    fn joined(founder: &mut Space, name: &str) -> Space {
        let mut s = Space::open(
            Identity {
                keys: founder.keys_for(true),
                device: super::super::core::Device::generate(name).unwrap(),
                secrets: true,
            },
            None,
        )
        .unwrap();
        let m = s.member_entry().unwrap();
        founder.add_member(m).unwrap();
        s
    }

    async fn converge(t: &Transport, spaces: &mut [(&mut Space, &mut Progress)]) {
        for _ in 0..2 {
            for (s, p) in spaces.iter_mut() {
                sync(s, t, p).await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn three_devices_converge_through_memory_and_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        for t in [
            Transport::Memory(Hub::default()),
            Transport::Folder(dir.path().into()),
        ] {
            let mut a = Space::create("A", None).unwrap();
            let mut b = joined(&mut a, "B");
            let mut c = joined(&mut a, "C");
            let (mut pa, mut pb, mut pc) = Default::default();
            a.put(Kind::Settings, "prefs", json!({"theme":"dark","zoom":100}))
                .unwrap();
            converge(
                &t,
                &mut [(&mut a, &mut pa), (&mut b, &mut pb), (&mut c, &mut pc)],
            )
            .await;
            // Concurrent edits to different fields both survive.
            b.put(Kind::Settings, "prefs", json!({"theme":"light","zoom":100}))
                .unwrap();
            c.put(Kind::Settings, "prefs", json!({"theme":"dark","zoom":125}))
                .unwrap();
            converge(
                &t,
                &mut [(&mut a, &mut pa), (&mut b, &mut pb), (&mut c, &mut pc)],
            )
            .await;
            for s in [&a, &b, &c] {
                assert_eq!(
                    s.value(Kind::Settings, "prefs").unwrap(),
                    json!({"theme":"light","zoom":125}),
                    "{}",
                    t.label()
                );
            }
            assert_eq!(a.store.records.len(), c.store.records.len());
            // Nothing new: nothing moves.
            let o = sync(&mut a, &t, &mut pa).await.unwrap();
            assert_eq!(o, Outcome::default());
        }
    }
}
