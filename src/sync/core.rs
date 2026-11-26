//! The sync engine's core: keys, clocks, and signed, encrypted, content-addressed records.
//!
//! A *space* is one person's synced Spindle. Its key (32 random bytes, replaced when a device
//! is removed) never leaves the devices; servers, relays and folders see only ciphertext, the
//! space ID, a keyed tag per item and sizes. Each *record* is one revision of one item:
//!
//! ```text
//! Envelope { v, space, kind, item (keyed tag), parents, clock (HLC), device, epoch, body, sig }
//! ```
//!
//! The body is AES-256-GCM under a key derived per kind and epoch (HKDF), with the header as
//! associated data, then the whole envelope is signed with the device's Ed25519 key. A
//! record's ID is the SHA-256 of its serialized envelope, so records are immutable and a
//! transport only moves the ones it lacks.

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine as _;
use ring::{digest, hkdf, hmac, signature};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u8 = 1;
/// The most a single record may hold.
pub const MAX_RECORD: usize = 16 << 20;

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(data)
}

pub fn unb64(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(text.trim_end_matches('='))
        .context("Invalid base64")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Result<Vec<u8>> {
    ensure!(text.len() % 2 == 0, "Invalid hex");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| anyhow!("Invalid hex")))
        .collect()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = digest::digest(&digest::SHA256, data);
    d.as_ref().try_into().unwrap()
}

/// What an item is; decides its key and how revisions merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The signed list of devices in the space.
    Devices,
    /// Keys for a new epoch, wrapped to each device (not encrypted with the space key).
    Keys,
    Chat,
    Project,
    Settings,
    Skill,
    Memory,
    Schedule,
    Provider,
    /// API keys, encrypted again with the secrets key (OAuth sign-ins never sync).
    Secret,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Devices => "devices",
            Kind::Keys => "keys",
            Kind::Chat => "chat",
            Kind::Project => "project",
            Kind::Settings => "settings",
            Kind::Skill => "skill",
            Kind::Memory => "memory",
            Kind::Schedule => "schedule",
            Kind::Provider => "provider",
            Kind::Secret => "secret",
        }
    }
}

struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

pub fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf::Salt::new(hkdf::HKDF_SHA256, salt)
        .extract(ikm)
        .expand(&[info], Len(32))
        .expect("32-byte HKDF output")
        .fill(&mut out)
        .expect("32-byte HKDF output");
    out
}

/// The space key for each epoch; the newest encrypts new records.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SpaceKeys {
    pub space: String,
    pub epochs: BTreeMap<u32, [u8; 32]>,
    /// The secrets key per epoch, held only by devices allowed to have API keys.
    #[serde(default)]
    pub secrets: BTreeMap<u32, [u8; 32]>,
}

impl std::fmt::Debug for SpaceKeys {
    // Never print keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SpaceKeys({}, epoch {})", self.space, self.epoch())
    }
}

impl SpaceKeys {
    pub fn generate() -> Result<Self> {
        let key = crate::vault::random::<32>()?;
        let mut epochs = BTreeMap::new();
        epochs.insert(1, key);
        let mut secrets = BTreeMap::new();
        secrets.insert(1, crate::vault::random::<32>()?);
        Ok(Self {
            space: space_id(&key),
            epochs,
            secrets,
        })
    }
    pub fn epoch(&self) -> u32 {
        *self.epochs.keys().next_back().unwrap_or(&0)
    }
    fn key(&self, epoch: u32, kind: Kind) -> Result<[u8; 32]> {
        let k = self
            .epochs
            .get(&epoch)
            .with_context(|| format!("No space key for epoch {epoch}"))?;
        Ok(hkdf32(k, b"spindle-sync-v1", kind.name().as_bytes()))
    }
    /// A tag for an item that hides its kind-specific ID from servers but is the same on
    /// every device. Stable across epochs (derived from the first key).
    pub fn tag(&self, kind: Kind, id: &str) -> Result<String> {
        let first = self.epochs.values().next().context("No space key")?;
        let k = hmac::Key::new(
            hmac::HMAC_SHA256,
            &hkdf32(first, b"spindle-sync-v1", b"tag"),
        );
        let t = hmac::sign(&k, format!("{}\0{id}", kind.name()).as_bytes());
        Ok(hex(&t.as_ref()[..16]))
    }
}

/// A space's public ID: a hash of its first key.
pub fn space_id(key: &[u8; 32]) -> String {
    let mut data = b"spindle space id\0".to_vec();
    data.extend_from_slice(key);
    hex(&sha256(&data)[..16])
}

/// This device's identity: an Ed25519 key for signing and an X25519 key for receiving keys.
#[derive(Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pkcs8: String,
    x25519: String,
}

impl Device {
    pub fn generate(name: &str) -> Result<Self> {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| anyhow!("Couldn't make a device key"))?;
        let pair = signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| anyhow!("Couldn't read the device key"))?;
        use signature::KeyPair as _;
        Ok(Self {
            id: hex(pair.public_key().as_ref()),
            name: name.chars().take(64).collect(),
            pkcs8: b64(pkcs8.as_ref()),
            x25519: b64(&crate::vault::random::<32>()?),
        })
    }
    fn pair(&self) -> Result<signature::Ed25519KeyPair> {
        signature::Ed25519KeyPair::from_pkcs8(&unb64(&self.pkcs8)?)
            .map_err(|_| anyhow!("Damaged device key"))
    }
    pub fn sign(&self, msg: &[u8]) -> Result<Vec<u8>> {
        Ok(self.pair()?.sign(msg).as_ref().to_vec())
    }
    fn x_secret(&self) -> Result<x25519_dalek::StaticSecret> {
        let bytes: [u8; 32] = unb64(&self.x25519)?
            .try_into()
            .map_err(|_| anyhow!("Damaged device key"))?;
        Ok(x25519_dalek::StaticSecret::from(bytes))
    }
    pub fn x_public(&self) -> Result<String> {
        Ok(b64(
            x25519_dalek::PublicKey::from(&self.x_secret()?).as_bytes()
        ))
    }
    /// Open a key wrapped to this device with [`wrap`].
    pub fn unwrap(&self, wrapped: &str) -> Result<[u8; 32]> {
        let data = unb64(wrapped)?;
        ensure!(data.len() > 32, "Damaged wrapped key");
        let eph: [u8; 32] = data[..32].try_into()?;
        let shared = self
            .x_secret()?
            .diffie_hellman(&x25519_dalek::PublicKey::from(eph));
        let k = hkdf32(shared.as_bytes(), &eph, b"spindle-wrap");
        let key = crate::vault::open(&k, b"spindle-wrap", &data[32..])?;
        key.try_into().map_err(|_| anyhow!("Damaged wrapped key"))
    }
}

/// Encrypt a key to a device's X25519 public key (ephemeral-static Diffie-Hellman).
pub fn wrap(x_public: &str, key: &[u8; 32]) -> Result<String> {
    let to: [u8; 32] = unb64(x_public)?
        .try_into()
        .map_err(|_| anyhow!("Invalid device key"))?;
    let eph = x25519_dalek::StaticSecret::from(crate::vault::random::<32>()?);
    let eph_pub = x25519_dalek::PublicKey::from(&eph);
    let shared = eph.diffie_hellman(&x25519_dalek::PublicKey::from(to));
    let k = hkdf32(shared.as_bytes(), eph_pub.as_bytes(), b"spindle-wrap");
    let mut out = eph_pub.as_bytes().to_vec();
    out.extend(crate::vault::seal(&k, b"spindle-wrap", key.to_vec())?);
    Ok(b64(&out))
}

pub fn verify(device: &str, msg: &[u8], sig: &[u8]) -> bool {
    let Ok(key) = unhex(device) else { return false };
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(msg, sig)
        .is_ok()
}

/// A hybrid logical clock: wall time in milliseconds, a counter for ties, and the device as
/// the last tie-breaker, so every device orders revisions the same way.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc {
    pub wall: u64,
    pub count: u32,
    pub device: String,
}

impl Hlc {
    pub fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    /// The next clock value after `last` (the newest this device has seen).
    pub fn tick(last: &Hlc, device: &str) -> Hlc {
        let now = Self::now_ms();
        if now > last.wall {
            Hlc {
                wall: now,
                count: 0,
                device: device.into(),
            }
        } else {
            Hlc {
                wall: last.wall,
                count: last.count + 1,
                device: device.into(),
            }
        }
    }
}

/// The signed, encrypted form of a record, as stored and sent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub v: u8,
    pub space: String,
    pub kind: Kind,
    pub item: String,
    pub parents: Vec<String>,
    pub clock: Hlc,
    pub device: String,
    pub epoch: u32,
    pub body: String,
    #[serde(default)]
    pub sig: String,
}

/// What a record says, once decrypted.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Body {
    pub id: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
    #[serde(default)]
    pub value: serde_json::Value,
    /// The top-level fields this revision changed (empty: all of them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
}

impl Envelope {
    fn header(&self) -> Result<Vec<u8>> {
        let mut h = self.clone();
        h.body.clear();
        h.sig.clear();
        Ok(serde_json::to_vec(&h)?)
    }
    fn unsigned(&self) -> Result<Vec<u8>> {
        let mut h = self.clone();
        h.sig.clear();
        Ok(serde_json::to_vec(&h)?)
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
    pub fn id(&self) -> Result<String> {
        Ok(hex(&sha256(&self.bytes()?)))
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_RECORD, "Record too large");
        let e: Envelope = serde_json::from_slice(bytes).context("Damaged record")?;
        ensure!(e.v == VERSION, "Record from a newer Spindle");
        Ok(e)
    }
    /// Check the signature against the device named in the record.
    pub fn verified(&self) -> Result<()> {
        ensure!(
            verify(&self.device, &self.unsigned()?, &unb64(&self.sig)?),
            "Record signature doesn't match its device"
        );
        Ok(())
    }
}

/// Seal a record: encrypt the body for its kind, then sign the envelope.
pub fn seal(
    keys: &SpaceKeys,
    device: &Device,
    kind: Kind,
    body: &Body,
    parents: Vec<String>,
    clock: Hlc,
) -> Result<Envelope> {
    let mut e = Envelope {
        v: VERSION,
        space: keys.space.clone(),
        kind,
        item: keys.tag(kind, &body.id)?,
        parents,
        clock,
        device: device.id.clone(),
        epoch: keys.epoch(),
        body: String::new(),
        sig: String::new(),
    };
    let mut plain = serde_json::to_vec(body)?;
    if kind == Kind::Secret {
        let s = keys
            .secrets
            .get(&keys.epoch())
            .context("This device can't hold API keys")?;
        plain = crate::vault::seal(s, b"spindle-secret", plain)?;
    }
    let aad = e.header()?;
    let sealed = if kind == Kind::Keys {
        // Key records carry only keys wrapped to devices; the space key can't protect the
        // key that replaces it.
        plain
    } else {
        crate::vault::seal(&keys.key(e.epoch, kind)?, &aad, plain)?
    };
    e.body = b64(&sealed);
    e.sig = b64(&device.sign(&e.unsigned()?)?);
    Ok(e)
}

/// Check and decrypt a record.
pub fn open(keys: &SpaceKeys, e: &Envelope) -> Result<Body> {
    ensure!(e.space == keys.space, "Record from another space");
    e.verified()?;
    let data = unb64(&e.body)?;
    let mut plain = if e.kind == Kind::Keys {
        data
    } else {
        crate::vault::open(&keys.key(e.epoch, e.kind)?, &e.header()?, &data)?
    };
    if e.kind == Kind::Secret {
        let s = keys
            .secrets
            .get(&e.epoch)
            .context("This device doesn't hold API keys")?;
        plain = crate::vault::open(s, b"spindle-secret", &plain)?;
    }
    let body: Body = serde_json::from_slice(&plain).context("Damaged record body")?;
    ensure!(
        keys.tag(e.kind, &body.id)? == e.item,
        "Record body doesn't match its item"
    );
    Ok(body)
}

/// A device in the space's device list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub x25519: String,
    /// Allowed to hold API keys (desktops by default).
    #[serde(default)]
    pub secrets: bool,
    #[serde(default)]
    pub added: u64,
    /// When it was removed; its records after this are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
}

/// Every record this device has, by ID, with what it decodes to.
#[derive(Default)]
pub struct Store {
    pub records: BTreeMap<String, Envelope>,
    pub bodies: BTreeMap<String, Body>,
    /// Records that couldn't be opened yet (a newer epoch, or an unknown device).
    pub waiting: BTreeSet<String>,
    pub last: Hlc,
}

impl Store {
    pub fn insert(&mut self, keys: &SpaceKeys, e: Envelope) -> Result<String> {
        let id = e.id()?;
        if self.records.contains_key(&id) {
            return Ok(id);
        }
        e.verified()?;
        if e.clock > self.last {
            self.last = Hlc {
                device: String::new(),
                ..e.clock.clone()
            };
        }
        match open(keys, &e) {
            Ok(b) => {
                self.bodies.insert(id.clone(), b);
            }
            Err(_) => {
                self.waiting.insert(id.clone());
            }
        }
        self.records.insert(id.clone(), e);
        Ok(id)
    }

    /// Try records that couldn't be opened before (after new keys arrived).
    pub fn retry(&mut self, keys: &SpaceKeys) {
        for id in std::mem::take(&mut self.waiting) {
            match self.records.get(&id).map(|e| open(keys, e)) {
                Some(Ok(b)) => {
                    self.bodies.insert(id, b);
                }
                _ => {
                    self.waiting.insert(id);
                }
            }
        }
    }

    /// The records of one item, oldest first.
    pub fn item(&self, kind: Kind, tag: &str) -> Vec<(&String, &Envelope, &Body)> {
        let mut out: Vec<_> = self
            .records
            .iter()
            .filter(|(_, e)| e.kind == kind && e.item == tag)
            .filter_map(|(id, e)| self.bodies.get(id).map(|b| (id, e, b)))
            .collect();
        out.sort_by(|a, b| a.1.clock.cmp(&b.1.clock));
        out
    }

    /// The newest records of an item: the ones no other record names as a parent.
    pub fn heads(&self, kind: Kind, tag: &str) -> Vec<String> {
        let all = self.item(kind, tag);
        let parents: BTreeSet<&String> =
            all.iter().flat_map(|(_, e, _)| e.parents.iter()).collect();
        all.iter()
            .filter(|(id, _, _)| !parents.contains(id))
            .map(|(id, _, _)| (*id).clone())
            .collect()
    }

    /// Make, store and return a new record for an item, with the current heads as parents.
    pub fn publish(
        &mut self,
        keys: &SpaceKeys,
        device: &Device,
        kind: Kind,
        body: Body,
    ) -> Result<String> {
        let tag = keys.tag(kind, &body.id)?;
        let parents = self.heads(kind, &tag);
        let clock = Hlc::tick(&self.last, &device.id);
        let e = seal(keys, device, kind, &body, parents, clock)?;
        self.insert(keys, e)
    }

    /// Save every record into a folder (`records/ab/abcdef….json`).
    pub fn save_to(&self, dir: &std::path::Path) -> Result<()> {
        for (id, e) in &self.records {
            let path = dir
                .join("records")
                .join(&id[..2])
                .join(format!("{id}.json"));
            if !path.exists() {
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(&path, e.bytes()?)?;
            }
        }
        Ok(())
    }

    pub fn load_from(&mut self, keys: &SpaceKeys, dir: &std::path::Path) -> Result<usize> {
        let mut n = 0;
        let Ok(shards) = std::fs::read_dir(dir.join("records")) else {
            return Ok(0);
        };
        for shard in shards.flatten() {
            for f in std::fs::read_dir(shard.path())?.flatten() {
                let bytes = std::fs::read(f.path())?;
                if let Ok(e) = Envelope::parse(&bytes)
                    && self.insert(keys, e).is_ok()
                {
                    n += 1;
                }
            }
        }
        Ok(n)
    }
}

/// The value of a last-write-wins item: per top-level field, the newest revision that
/// changed it. `None` when the newest revision deletes the item.
pub fn lww(records: &[(&String, &Envelope, &Body)]) -> Option<serde_json::Value> {
    let newest = records.iter().max_by(|a, b| a.1.clock.cmp(&b.1.clock))?;
    if newest.2.deleted {
        return None;
    }
    // Only revisions after the newest delete count (a delete and then a re-add starts over).
    let since = records
        .iter()
        .filter(|r| r.2.deleted)
        .map(|r| r.1.clock.clone())
        .max();
    let live: Vec<_> = records
        .iter()
        .filter(|r| since.as_ref().is_none_or(|s| r.1.clock > *s))
        .collect();
    if !newest.2.value.is_object() {
        return Some(newest.2.value.clone());
    }
    let mut out = serde_json::Map::new();
    let mut when: BTreeMap<String, Hlc> = BTreeMap::new();
    for r in live {
        let Some(obj) = r.2.value.as_object() else {
            continue;
        };
        for (k, v) in obj {
            let changed = r.2.changed.is_empty() || r.2.changed.iter().any(|c| c == k);
            if !changed {
                continue;
            }
            if when.get(k).is_none_or(|w| r.1.clock > *w) {
                when.insert(k.clone(), r.1.clock.clone());
                out.insert(k.clone(), v.clone());
            }
        }
    }
    // A field removed by the newest full revision is gone.
    if newest.2.changed.is_empty()
        && let Some(obj) = newest.2.value.as_object()
    {
        out.retain(|k, _| obj.contains_key(k) || when.get(k).is_some_and(|w| *w > newest.1.clock));
    }
    Some(serde_json::Value::Object(out))
}

/// The top-level fields that differ between two values (for `Body::changed`).
pub fn changed_fields(
    before: Option<&serde_json::Value>,
    after: &serde_json::Value,
) -> Vec<String> {
    let (Some(b), Some(a)) = (before.and_then(|b| b.as_object()), after.as_object()) else {
        return vec![];
    };
    let mut out: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(v))
        .map(|(k, _)| k.clone())
        .collect();
    out.extend(b.keys().filter(|k| !a.contains_key(*k)).cloned());
    out
}

/// Concurrent revisions of an item: more than one head with different values. Returns the
/// heads ordered newest first.
pub fn conflict<'a>(store: &'a Store, kind: Kind, tag: &str) -> Vec<(&'a Envelope, &'a Body)> {
    let mut heads: Vec<_> = store
        .heads(kind, tag)
        .into_iter()
        .filter_map(|id| Some((store.records.get(&id)?, store.bodies.get(&id)?)))
        .collect();
    heads.sort_by(|a, b| b.0.clock.cmp(&a.0.clock));
    heads.dedup_by(|a, b| a.1.value == b.1.value && a.1.deleted == b.1.deleted);
    if heads.len() > 1 { heads } else { vec![] }
}

pub fn ensure_member(members: &[Member], e: &Envelope) -> Result<()> {
    let m = members
        .iter()
        .find(|m| m.id == e.device)
        .ok_or_else(|| anyhow!("Record from a device that isn't in this space"))?;
    if let Some(r) = m.removed
        && e.clock.wall > r
    {
        bail!("Record from a removed device");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_are_sealed_signed_and_addressed_by_content() {
        let keys = SpaceKeys::generate().unwrap();
        let dev = Device::generate("Desktop").unwrap();
        let body = Body {
            id: "prefs".into(),
            value: json!({"theme": "dark"}),
            ..Default::default()
        };
        let e = seal(&keys, &dev, Kind::Settings, &body, vec![], Hlc::default()).unwrap();
        assert!(!e.body.contains("dark"));
        assert_eq!(open(&keys, &e).unwrap(), body);
        let id = e.id().unwrap();
        let again = Envelope::parse(&e.bytes().unwrap()).unwrap();
        assert_eq!(again.id().unwrap(), id);
        // Tampering breaks the signature.
        let mut bad = e.clone();
        bad.parents.push("x".into());
        assert!(open(&keys, &bad).is_err());
        // Another space can't read it.
        let other = SpaceKeys::generate().unwrap();
        assert!(open(&other, &e).is_err());
    }

    #[test]
    fn keys_wrap_to_a_device() {
        let dev = Device::generate("Laptop").unwrap();
        let key = crate::vault::random::<32>().unwrap();
        let wrapped = wrap(&dev.x_public().unwrap(), &key).unwrap();
        assert_eq!(dev.unwrap(&wrapped).unwrap(), key);
        let other = Device::generate("Phone").unwrap();
        assert!(other.unwrap(&wrapped).is_err());
    }

    #[test]
    fn secrets_need_the_secrets_key() {
        let keys = SpaceKeys::generate().unwrap();
        let dev = Device::generate("Desktop").unwrap();
        let body = Body {
            id: "openai".into(),
            value: json!({"key": "sk-123"}),
            ..Default::default()
        };
        let e = seal(&keys, &dev, Kind::Secret, &body, vec![], Hlc::default()).unwrap();
        assert_eq!(open(&keys, &e).unwrap().value["key"], "sk-123");
        let mut phone = keys.clone();
        phone.secrets.clear();
        assert!(open(&phone, &e).is_err());
    }

    #[test]
    fn last_write_wins_per_field() {
        let keys = SpaceKeys::generate().unwrap();
        let a = Device::generate("A").unwrap();
        let b = Device::generate("B").unwrap();
        let mut s = Store::default();
        let mk = |dev: &Device, wall, value: serde_json::Value, changed: Vec<&str>| {
            seal(
                &keys,
                dev,
                Kind::Project,
                &Body {
                    id: "p".into(),
                    value,
                    changed: changed.into_iter().map(String::from).collect(),
                    ..Default::default()
                },
                vec![],
                Hlc {
                    wall,
                    count: 0,
                    device: dev.id.clone(),
                },
            )
            .unwrap()
        };
        s.insert(&keys, mk(&a, 1, json!({"name":"X","notes":"a"}), vec![]))
            .unwrap();
        s.insert(
            &keys,
            mk(&b, 2, json!({"name":"Y","notes":"a"}), vec!["name"]),
        )
        .unwrap();
        s.insert(
            &keys,
            mk(&a, 3, json!({"name":"X","notes":"b"}), vec!["notes"]),
        )
        .unwrap();
        let tag = keys.tag(Kind::Project, "p").unwrap();
        assert_eq!(
            lww(&s.item(Kind::Project, &tag)).unwrap(),
            json!({"name":"Y","notes":"b"})
        );
    }
}
