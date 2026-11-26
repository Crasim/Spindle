
//! A synced space on this device: its keys, this device's identity, every record it has, and
//! the rules for who may write (the device list) and how keys change when a device leaves.

use super::core::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The ID of the one devices item.
const DEVICES: &str = "devices";

/// What a device keeps about the space (stored inside Spindle's encrypted state).
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub keys: SpaceKeys,
    pub device: Device,
    /// Allowed to hold API keys (desktops by default).
    pub secrets: bool,
}

pub struct Space {
    pub id: Identity,
    pub store: Store,
    /// Where records are kept on this device, if anywhere.
    pub dir: Option<PathBuf>,
}

impl Space {
    /// Start a new space with this device as its first member.
    pub fn create(name: &str, dir: Option<PathBuf>) -> Result<Self> {
        let mut s = Space {
            id: Identity {
                keys: SpaceKeys::generate()?,
                device: Device::generate(name)?,
                secrets: true,
            },
            store: Store::default(),
            dir,
        };
        let me = s.member_entry()?;
        s.publish_raw(
            Kind::Devices,
            DEVICES,
            json!({ me.id.clone(): me }),
            vec![],
            false,
        )?;
        Ok(s)
    }

    /// Open a space this device already belongs to (or has just been given keys for).
    pub fn open(id: Identity, dir: Option<PathBuf>) -> Result<Self> {
        let mut s = Space {
            id,
            store: Store::default(),
            dir,
        };
        if let Some(d) = s.dir.clone() {
            s.store.load_from(&s.id.keys, &d)?;
            s.absorb_keys();
        }
        Ok(s)
    }

    pub fn member_entry(&self) -> Result<Member> {
        Ok(Member {
            id: self.id.device.id.clone(),
            name: self.id.device.name.clone(),
            x25519: self.id.device.x_public()?,
            secrets: self.id.secrets,
            added: Hlc::now_ms(),
            removed: None,
        })
    }

    /// The device list, following the rules for who may change it: the space's first device;
    /// then any member; and a device that isn't a member yet may add only itself (it holds
    /// the key, so it was let in by pairing or an account sign-in).
    pub fn members(&self) -> Vec<Member> {
        let Ok(tag) = self.id.keys.tag(Kind::Devices, DEVICES) else {
            return vec![];
        };
        let records = self.store.item(Kind::Devices, &tag);
        let Some(founder) = records.first().map(|r| r.1.device.clone()) else {
            return vec![];
        };
        let mut list: BTreeMap<String, Member> = BTreeMap::new();
        let mut when: BTreeMap<String, Hlc> = BTreeMap::new();
        for (_, e, b) in &records {
            let Some(obj) = b.value.as_object() else {
                continue;
            };
            let member = list
                .get(&e.device)
                .is_some_and(|m| m.removed.is_none_or(|r| e.clock.wall <= r));
            let allowed = e.device == founder
                || member
                || (obj.len() == 1 && obj.contains_key(&e.device) && !list.contains_key(&e.device));
            if !allowed {
                continue;
            }
            for (k, v) in obj {
                let changed = b.changed.is_empty() || b.changed.iter().any(|c| c == k);
                if !changed {
                    continue;
                }
                let Ok(m) = serde_json::from_value::<Member>(v.clone()) else {
                    continue;
                };
                if when.get(k).is_none_or(|w| e.clock > *w) {
                    when.insert(k.clone(), e.clock.clone());
                    list.insert(k.clone(), m);
                }
            }
        }
        list.into_values().collect()
    }

    pub fn is_member(&self, device: &str) -> bool {
        self.members()
            .iter()
            .any(|m| m.id == device && m.removed.is_none())
    }

    /// The records of an item from current members (and from removed ones before removal).
    pub fn item(&self, kind: Kind, id: &str) -> Vec<(&String, &Envelope, &Body)> {
        let members = self.members();
        let Ok(tag) = self.id.keys.tag(kind, id) else {
            return vec![];
        };
        self.store
            .item(kind, &tag)
            .into_iter()
            .filter(|r| ensure_member(&members, r.1).is_ok())
            .collect()
    }

    /// The merged value of a last-write-wins item.
    pub fn value(&self, kind: Kind, id: &str) -> Option<Value> {
        lww(&self.item(kind, id))
    }

    /// Every item of a kind: (id, merged value or None when deleted).
    pub fn items(&self, kind: Kind) -> Vec<(String, Option<Value>)> {
        let members = self.members();
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for (rid, e) in &self.store.records {
            if e.kind == kind
                && ensure_member(&members, e).is_ok()
                && let Some(b) = self.store.bodies.get(rid)
            {
                ids.insert(b.id.clone());
            }
        }
        ids.into_iter()
            .map(|id| {
                let v = self.value(kind, &id);
                (id, v)
            })
            .collect()
    }

    fn publish_raw(
        &mut self,
        kind: Kind,
        id: &str,
        value: Value,
        changed: Vec<String>,
        deleted: bool,
    ) -> Result<String> {
        let rid = self.store.publish(
            &self.id.keys,
            &self.id.device,
            kind,
            Body {
                id: id.into(),
                deleted,
                value,
                changed,
            },
        )?;
        self.save()?;
        Ok(rid)
    }

    /// Publish an item's new value, unless it is already the merged value. Returns the new
    /// record's ID when something was published.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn put(&mut self, kind: Kind, id: &str, value: Value) -> Result<Option<String>> {
        let before = self.value(kind, id);
        if before.as_ref() == Some(&value) {
            return Ok(None);
        }
        let changed = changed_fields(before.as_ref(), &value);
        // Fields this device would publish unchanged aren't claimed as changed, so an
        // older edit elsewhere isn't overwritten by a stale copy.
        if before.is_some() && changed.is_empty() && value.is_object() {
            return Ok(None);
        }
        Ok(Some(self.publish_raw(kind, id, value, changed, false)?))
    }

    /// Publish a value with the fields that changed, as measured by the caller.
    pub fn put_changes(
        &mut self,
        kind: Kind,
        id: &str,
        value: Value,
        changed: Vec<String>,
    ) -> Result<String> {
        self.publish_raw(kind, id, value, changed, false)
    }

    /// Publish a chat revision (turns are merged by the chat tree, not field by field).
    pub fn put_chat(&mut self, id: &str, value: Value, changed: Vec<String>) -> Result<String> {
        self.publish_raw(Kind::Chat, id, value, changed, false)
    }

    pub fn delete(&mut self, kind: Kind, id: &str) -> Result<Option<String>> {
        if self.item(kind, id).is_empty() {
            return Ok(None);
        }
        Ok(Some(self.publish_raw(
            kind,
            id,
            Value::Null,
            vec![],
            true,
        )?))
    }

    /// Add a device that has been let in (pairing does this for the new device).
    pub fn add_member(&mut self, m: Member) -> Result<()> {
        ensure!(
            self.is_member(&self.id.device.id),
            "This device isn't in the space"
        );
        let id = m.id.clone();
        self.publish_raw(
            Kind::Devices,
            DEVICES,
            json!({ id.clone(): m }),
            vec![id],
            false,
        )?;
        Ok(())
    }

    /// Add this device to the list itself (it was let in by pairing or a sign-in and holds
    /// the key).
    pub fn publish_self(&mut self, me: Member) -> Result<()> {
        let id = me.id.clone();
        self.publish_raw(
            Kind::Devices,
            DEVICES,
            json!({ id.clone(): me }),
            vec![id],
            false,
        )?;
        Ok(())
    }

    /// Leave the space: mark this device removed on the list, so the others stop syncing
    /// with it (and drop its addresses) once they hear.
    pub fn leave(&mut self) -> Result<()> {
        let mut me = self
            .members()
            .into_iter()
            .find(|m| m.id == self.id.device.id)
            .context("This device isn't in the space")?;
        me.removed = Some(Hlc::now_ms());
        self.publish_self(me)
    }

    /// Give this device a new name; other devices see it on their next sync.
    pub fn rename(&mut self, name: &str) -> Result<()> {
        let name: String = name.trim().chars().take(64).collect();
        ensure!(!name.is_empty(), "Give the device a name");
        let mut me = self
            .members()
            .into_iter()
            .find(|m| m.id == self.id.device.id)
            .context("This device isn't in the space")?;
        me.name = name.clone();
        self.id.device.name = name;
        self.publish_self(me)
    }

    /// Remove a device and replace the keys, so it can't read anything new.
    pub fn remove_member(&mut self, device: &str) -> Result<()> {
        ensure!(
            device != self.id.device.id,
            "Remove this device from another one"
        );
        let mut m = self
            .members()
            .into_iter()
            .find(|m| m.id == device)
            .context("No such device")?;
        m.removed = Some(Hlc::now_ms());
        self.publish_raw(
            Kind::Devices,
            DEVICES,
            json!({ device: m }),
            vec![device.to_owned()],
            false,
        )?;
        self.rotate()
    }

    /// A new epoch: fresh space and secrets keys, wrapped to each remaining device.
    pub fn rotate(&mut self) -> Result<()> {
        let epoch = self.id.keys.epoch() + 1;
        let key = crate::vault::random::<32>()?;
        let secret = crate::vault::random::<32>()?;
        let mut wraps = serde_json::Map::new();
        for m in self.members().into_iter().filter(|m| m.removed.is_none()) {
            let mut w = json!({ "key": wrap(&m.x25519, &key)? });
            if m.secrets {
                w["secrets"] = json!(wrap(&m.x25519, &secret)?);
            }
            wraps.insert(m.id.clone(), w);
        }
        // Signed with the old epoch's numbering so every member can find it; the body is the
        // wraps, readable only by their devices.
        let body = Body {
            id: format!("epoch-{epoch}"),
            value: json!({ "epoch": epoch, "wraps": wraps }),
            ..Default::default()
        };
        let parents = vec![];
        let clock = Hlc::tick(&self.store.last, &self.id.device.id);
        let e = seal(
            &self.id.keys,
            &self.id.device,
            Kind::Keys,
            &body,
            parents,
            clock,
        )?;
        self.store.insert(&self.id.keys, e)?;
        self.absorb_keys();
        self.save()
    }

    /// Take any new keys wrapped to this device, then open records that were waiting.
    pub fn absorb_keys(&mut self) {
        let members = self.members();
        let mut grew = false;
        for (rid, e) in &self.store.records {
            if e.kind != Kind::Keys || ensure_member(&members, e).is_err() {
                continue;
            }
            let Some(b) = self.store.bodies.get(rid) else {
                continue;
            };
            let Some(epoch) = b.value["epoch"].as_u64().map(|n| n as u32) else {
                continue;
            };
            if self.id.keys.epochs.contains_key(&epoch) {
                continue;
            }
            let w = &b.value["wraps"][&self.id.device.id];
            if let Some(k) = w["key"]
                .as_str()
                .and_then(|k| self.id.device.unwrap(k).ok())
            {
                self.id.keys.epochs.insert(epoch, k);
                grew = true;
            }
            if let Some(k) = w["secrets"]
                .as_str()
                .and_then(|k| self.id.device.unwrap(k).ok())
            {
                self.id.keys.secrets.insert(epoch, k);
            }
        }
        if grew {
            self.store.retry(&self.id.keys);
        }
    }

    /// Take records from elsewhere. Returns how many were new.
    pub fn receive(&mut self, records: Vec<Vec<u8>>) -> Result<usize> {
        let mut n = 0;
        for bytes in records {
            let Ok(e) = Envelope::parse(&bytes) else {
                continue;
            };
            if e.space != self.id.keys.space {
                continue;
            }
            let id = e.id()?;
            if self.store.records.contains_key(&id) {
                continue;
            }
            if self.store.insert(&self.id.keys, e).is_ok() {
                n += 1;
            }
        }
        if n > 0 {
            self.absorb_keys();
            self.save()?;
        }
        Ok(n)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(d) = &self.dir {
            self.store.save_to(d)?;
        }
        Ok(())
    }

    /// The keys to give a new device: every epoch, and the secrets keys if it may hold them.
    pub fn keys_for(&self, secrets: bool) -> SpaceKeys {
        let mut k = self.id.keys.clone();
        if !secrets {
            k.secrets.clear();
        }
        k
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn join(founder: &Space, name: &str, secrets: bool) -> Space {
        let mut s = Space {
            id: Identity {
                keys: founder.keys_for(secrets),
                device: Device::generate(name).unwrap(),
                secrets,
            },
            store: Store::default(),
            dir: None,
        };
        // Pairing hands over the records it has.
        s.receive(
            founder
                .store
                .records
                .values()
                .map(|e| e.bytes().unwrap())
                .collect(),
        )
        .unwrap();
        s
    }

    fn exchange(a: &mut Space, b: &mut Space) {
        let from_a: Vec<_> = a
            .store
            .records
            .values()
            .map(|e| e.bytes().unwrap())
            .collect();
        let from_b: Vec<_> = b
            .store
            .records
            .values()
            .map(|e| e.bytes().unwrap())
            .collect();
        a.receive(from_b).unwrap();
        b.receive(from_a).unwrap();
    }


    /// Pairing as it really goes: the host hands over its records, the new device adds
    /// itself, the host adds it too; then devices leave, are removed, and new ones pair.
    fn pair(host: &mut Space, name: &str, secrets: bool) -> Space {
        let mut d = join(host, name, secrets);
        let me = d.member_entry().unwrap();
        d.publish_self(me.clone()).unwrap();
        host.add_member(me).unwrap();
        d
    }

    fn knows(a: &Space, b: &Space) -> bool {
        a.is_member(&b.id.device.id)
    }

    #[test]
    fn every_device_keeps_the_others_through_leaving_removing_and_pairing_again() {
        let mut pc = Space::create("PC", None).unwrap();
        let mut old = pair(&mut pc, "Old phone", false);
        exchange(&mut pc, &mut old);
        assert!(knows(&old, &pc) && knows(&pc, &old));
        // The old phone stops syncing; the PC removes it too.
        old.leave().unwrap();
        exchange(&mut pc, &mut old);
        let _ = pc.remove_member(&old.id.device.id.clone());
        pc.rename("Desktop-PC").unwrap();
        // A wiped phone pairs again as a new device.
        let mut phone = pair(&mut pc, "Phone", false);
        exchange(&mut pc, &mut phone);
        assert!(knows(&phone, &pc), "phone lost the PC: {:?}", phone.members().iter().map(|m| (&m.name, m.removed)).collect::<Vec<_>>());
        assert!(knows(&pc, &phone));
        // A second PC pairs from the phone's side of things later.
        let mut pc2 = pair(&mut pc, "PC2", true);
        exchange(&mut pc, &mut pc2);
        exchange(&mut pc2, &mut phone);
        exchange(&mut pc, &mut phone);
        for (a, b) in [(&pc, &phone), (&phone, &pc), (&pc2, &phone), (&phone, &pc2), (&pc, &pc2), (&pc2, &pc)] {
            assert!(knows(a, b), "{} doesn't know {}: {:?}", a.id.device.name, b.id.device.name,
                a.members().iter().map(|m| (&m.name, m.removed)).collect::<Vec<_>>());
        }
        // Remove the second PC from the phone... no: from the PC, and pair a third.
        pc.remove_member(&pc2.id.device.id.clone()).unwrap();
        exchange(&mut pc, &mut phone);
        let mut pc3 = pair(&mut phone, "PC3", true);
        exchange(&mut phone, &mut pc3);
        exchange(&mut pc, &mut phone);
        exchange(&mut pc, &mut pc3);
        for (a, b) in [(&pc, &phone), (&phone, &pc), (&pc3, &phone), (&phone, &pc3), (&pc, &pc3), (&pc3, &pc)] {
            assert!(knows(a, b), "{} doesn't know {}: {:?}", a.id.device.name, b.id.device.name,
                a.members().iter().map(|m| (&m.name, m.removed)).collect::<Vec<_>>());
        }
    }


    #[test]
    fn a_device_removed_elsewhere_is_no_longer_a_member() {
        let mut pc = Space::create("PC", None).unwrap();
        let mut old = pair(&mut pc, "Old phone", false);
        exchange(&mut pc, &mut old);
        // "Remove" pressed for the PC on the old phone.
        old.remove_member(&pc.id.device.id.clone()).unwrap();
        exchange(&mut pc, &mut old);
        assert!(!knows(&pc, &pc));
        assert!(pc.add_member(old.member_entry().unwrap()).is_err());
    }

    #[test]
    fn a_device_renames_itself_and_others_see_it() {
        let mut desk = Space::create("Desktop", None).unwrap();
        let mut lap = join(&desk, "Laptop", true);
        let entry = lap.member_entry().unwrap();
        desk.add_member(entry).unwrap();
        exchange(&mut desk, &mut lap);
        lap.rename("  EXAMPLE-LAPTOP ").unwrap();
        assert_eq!(lap.id.device.name, "EXAMPLE-LAPTOP");
        assert!(lap.rename(" ").is_err());
        exchange(&mut desk, &mut lap);
        let names: Vec<_> = desk.members().into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"EXAMPLE-LAPTOP".to_owned()), "{names:?}");
        assert!(
            desk.is_member(&lap.id.device.id),
            "renaming keeps it in the space"
        );
    }

    #[test]
    fn devices_join_and_a_removed_device_loses_new_keys() {
        let mut desk = Space::create("Desktop", None).unwrap();
        let mut lap = join(&desk, "Laptop", true);
        // Pairing on the existing device adds the new one.
        let entry = lap.member_entry().unwrap();
        desk.add_member(entry).unwrap();
        let mut phone = join(&desk, "Phone", false);
        let entry = phone.member_entry().unwrap();
        desk.add_member(entry).unwrap();
        exchange(&mut desk, &mut lap);
        exchange(&mut desk, &mut phone);
        assert_eq!(desk.members().len(), 3);
        assert!(lap.is_member(&phone.id.device.id));

        desk.put(Kind::Settings, "prefs", json!({"theme": "dark"}))
            .unwrap();
        exchange(&mut desk, &mut lap);
        assert_eq!(lap.value(Kind::Settings, "prefs").unwrap()["theme"], "dark");

        // Remove the phone: new records use a key it never gets.
        let phone_id = phone.id.device.id.clone();
        desk.remove_member(&phone_id).unwrap();
        desk.put(Kind::Settings, "prefs", json!({"theme": "light"}))
            .unwrap();
        exchange(&mut desk, &mut lap);
        exchange(&mut desk, &mut phone);
        assert_eq!(lap.id.keys.epoch(), 2);
        assert_eq!(
            lap.value(Kind::Settings, "prefs").unwrap()["theme"],
            "light"
        );
        assert_eq!(phone.id.keys.epoch(), 1);
        assert_eq!(
            phone.value(Kind::Settings, "prefs").unwrap()["theme"],
            "dark"
        );
        // Its new records are ignored by the others.
        phone
            .put(Kind::Settings, "prefs", json!({"theme": "pink"}))
            .unwrap();
        exchange(&mut phone, &mut desk);
        assert_eq!(
            desk.value(Kind::Settings, "prefs").unwrap()["theme"],
            "light"
        );
    }

    #[test]
    fn a_stranger_cannot_add_other_devices() {
        let desk = Space::create("Desktop", None).unwrap();
        let mut lap = join(&desk, "Laptop", true);
        // The laptop adds itself (allowed: it holds the key) but not a third device.
        let me = lap.member_entry().unwrap();
        let id = me.id.clone();
        lap.publish_raw(
            Kind::Devices,
            DEVICES,
            json!({ id.clone(): me }),
            vec![id],
            false,
        )
        .unwrap();
        assert!(lap.is_member(&lap.id.device.id));
        let other = Device::generate("Evil").unwrap();
        let fake = Member {
            id: other.id.clone(),
            name: "Evil".into(),
            x25519: other.x_public().unwrap(),
            ..Default::default()
        };
        let fid = fake.id.clone();
        // As a member now, the laptop may add devices: that's pairing's job. A record from a
        // non-member that adds someone else is what must be refused:
        let stranger = join(&desk, "Stranger", false);
        let mut st = stranger;
        st.publish_raw(
            Kind::Devices,
            DEVICES,
            json!({ fid.clone(): fake }),
            vec![fid.clone()],
            false,
        )
        .unwrap();
        assert!(!st.is_member(&fid));
    }

    #[test]
    fn deletes_win_until_the_item_is_added_again() {
        let mut a = Space::create("A", None).unwrap();
        a.put(Kind::Memory, "m1", json!({"text": "likes tea"}))
            .unwrap();
        a.delete(Kind::Memory, "m1").unwrap();
        assert!(a.value(Kind::Memory, "m1").is_none());
        a.put(Kind::Memory, "m1", json!({"text": "likes coffee"}))
            .unwrap();
        assert_eq!(a.value(Kind::Memory, "m1").unwrap()["text"], "likes coffee");
    }
}
