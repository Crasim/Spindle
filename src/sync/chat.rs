//! Conversations as a tree of turns, so edits on two devices become branches of one chat.
//!
//! Each synced turn is identified by a hash of its parent, role and text, so two devices that
//! add the same turn don't fork. The union of every revision's turns forms a tree. The *main*
//! branch is the line of the device where the conversation started (or the branch the user
//! made main); every other line is a branch named for the device that started it.

use super::core::{Body, Envelope, Hlc, hex, sha256};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One turn as it travels in a chat record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// This turn's hash.
    pub h: String,
    /// The previous turn's hash ("" for the first turn).
    #[serde(default)]
    pub p: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Spindle's notes about the turn (model, files, tool notes), as stored locally.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub info: serde_json::Value,
}

pub fn turn_hash(parent: &str, role: &str, content: &str, images: &[String]) -> String {
    let mut data = format!("{parent}\0{role}\0{content}").into_bytes();
    for i in images {
        data.extend_from_slice(&sha256(i.as_bytes()));
    }
    hex(&sha256(&data)[..16])
}

/// Turns for a line of messages, chained by hash.
pub fn chain(messages: &[(String, String, Vec<String>, serde_json::Value)]) -> Vec<Turn> {
    let mut parent = String::new();
    messages
        .iter()
        .map(|(role, content, images, info)| {
            let h = turn_hash(&parent, role, content, images);
            let t = Turn {
                h: h.clone(),
                p: std::mem::replace(&mut parent, h),
                role: role.clone(),
                content: content.clone(),
                images: images.clone(),
                info: info.clone(),
            };
            t
        })
        .collect()
}

/// A turn with where it came from: the first device and time it appeared.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub turn: Turn,
    pub device: String,
    pub clock: Hlc,
}

/// A turn copied onto another line by a merge. It counts only while the copy is on main.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MergeCopy {
    pub source: String,
    pub copy: String,
}

/// A branch other than the main one.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    /// The device that started it.
    pub device: String,
    /// How many turns it shares with the main branch.
    pub fork: usize,
    /// The first message that isn't already on main through a merge.
    pub new_from: usize,
    /// The whole line, from the first turn.
    pub turns: Vec<Placed>,
}

impl Branch {
    /// The branch's identity: its first turn after the fork.
    pub fn id(&self) -> &str {
        self.turns
            .get(self.new_from)
            .map(|t| t.turn.h.as_str())
            .unwrap_or("")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub main: Vec<Placed>,
    pub branches: Vec<Branch>,
}

/// Build the branch view from every revision of a chat.
///
/// `origin` is the device the conversation started on. `main` is the tip of a branch the
/// user made main, if any; `dropped` lists turns hidden with Delete branch (or left behind by
/// an edit on the device that wrote them).
pub fn view(
    records: &[(&String, &Envelope, &Body)],
    origin: &str,
    main: Option<&str>,
    dropped: &BTreeSet<String>,
) -> View {
    // Revisions after the newest delete only.
    let since = records
        .iter()
        .filter(|r| r.2.deleted)
        .map(|r| r.1.clock.clone())
        .max();
    let mut turns: BTreeMap<String, Placed> = BTreeMap::new();
    let mut copies = BTreeSet::<MergeCopy>::new();
    for (_, e, b) in records {
        if since.as_ref().is_some_and(|s| e.clock <= *s) {
            continue;
        }
        if let Some(entries) = b.value["merges"].as_array() {
            copies.extend(entries.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()));
        }
        let Some(list) = b.value.get("turns").and_then(|t| t.as_array()) else {
            continue;
        };
        for t in list {
            let Ok(t) = serde_json::from_value::<Turn>(t.clone()) else {
                continue;
            };
            let place = Placed {
                device: e.device.clone(),
                clock: e.clock.clone(),
                turn: t.clone(),
            };
            match turns.get_mut(&t.h) {
                Some(old) if old.clock > e.clock => *old = place,
                Some(old) => {
                    // Keep the first place, but take newer notes (a reply's info can grow).
                    if !t.info.is_null() {
                        old.turn.info = t.info;
                    }
                }
                None => {
                    turns.insert(t.h.clone(), place);
                }
            }
        }
    }
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in turns.values() {
        if dropped.contains(&t.turn.h) {
            continue;
        }
        // A turn whose parent is missing (or dropped) has no line to the root.
        if !t.turn.p.is_empty() && !turns.contains_key(&t.turn.p) {
            continue;
        }
        children
            .entry(t.turn.p.clone())
            .or_default()
            .push(t.turn.h.clone());
    }
    // Earliest first; the origin device's turns first at a fork.
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            let (a, b) = (&turns[a], &turns[b]);
            (a.device != origin, &a.clock).cmp(&(b.device != origin, &b.clock))
        });
    }
    let ancestors_of = |tip: &str| -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        let mut at = tip.to_owned();
        while let Some(t) = turns.get(&at) {
            set.insert(at.clone());
            at = t.turn.p.clone();
        }
        set
    };
    let chosen = main.map(ancestors_of).unwrap_or_default();
    let follow = |start: &str, prefer: &str| -> Vec<Placed> {
        let mut out = vec![];
        let mut at = start.to_owned();
        loop {
            out.push(turns[&at].clone());
            let Some(kids) = children.get(&at) else { break };
            let next = kids
                .iter()
                .find(|k| chosen.contains(*k))
                .or_else(|| kids.iter().find(|k| turns[*k].device == prefer))
                .or(kids.first());
            match next {
                Some(n) => at = n.clone(),
                None => break,
            }
        }
        out
    };
    let Some(roots) = children.get("") else {
        return View::default();
    };
    let first = roots
        .iter()
        .find(|k| chosen.contains(*k))
        .unwrap_or(&roots[0])
        .clone();
    let main_line = follow(&first, origin);
    let on_main: BTreeSet<&str> = main_line.iter().map(|t| t.turn.h.as_str()).collect();
    let mut sources: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for entry in &copies {
        if let (Some(original), Some(copy)) = (turns.get(&entry.source), turns.get(&entry.copy))
            && original.turn.role == copy.turn.role
            && original.turn.content == copy.turn.content
            && original.turn.images == copy.turn.images
        {
            sources.entry(&entry.copy).or_default().push(&entry.source);
        }
    }
    let mut included = BTreeSet::new();
    let mut represented: Vec<_> = sources.keys().copied().filter(|hash| on_main.contains(hash)).collect();
    while let Some(copy) = represented.pop() {
        for &source in sources.get(copy).into_iter().flatten() {
            if included.insert(source) {
                represented.push(source);
            }
        }
    }
    let mut branches = vec![];
    // Forks at the root and after each main turn.
    let mut parents: Vec<(usize, String)> = vec![(0, String::new())];
    parents.extend(
        main_line
            .iter()
            .enumerate()
            .map(|(i, t)| (i + 1, t.turn.h.clone())),
    );
    for (fork, parent) in parents {
        for kid in children.get(&parent).into_iter().flatten() {
            if on_main.contains(kid.as_str()) {
                continue;
            }
            // A device may extend an already merged branch while offline. Find the first
            // unmerged turn on each continuation, without hiding the new work or copying
            // the merged prefix again.
            let mut pending = vec![kid.as_str()];
            while let Some(start) = pending.pop() {
                if included.contains(start) {
                    pending.extend(children.get(start).into_iter().flatten().rev().map(String::as_str));
                    continue;
                }
                let device = turns[start].device.clone();
                let mut prefix = Vec::new();
                let mut at = turns[start].turn.p.as_str();
                while at != parent {
                    let earlier = &turns[at];
                    prefix.push(earlier.clone());
                    at = earlier.turn.p.as_str();
                }
                let new_from = fork + prefix.len();
                let mut line = main_line[..fork].to_vec();
                line.extend(prefix.into_iter().rev());
                line.extend(follow(start, &device));
                branches.push(Branch { device, fork, new_from, turns: line });
            }
        }
    }
    View {
        main: main_line,
        branches,
    }
}

#[cfg(test)]
mod tests {
    use super::super::core::*;
    use super::*;
    use serde_json::json;

    fn msgs(list: &[(&str, &str)]) -> Vec<(String, String, Vec<String>, serde_json::Value)> {
        list.iter()
            .map(|(r, c)| {
                (
                    r.to_string(),
                    c.to_string(),
                    vec![],
                    serde_json::Value::Null,
                )
            })
            .collect()
    }

    fn rec(keys: &SpaceKeys, dev: &Device, wall: u64, turns: Vec<Turn>) -> Envelope {
        seal(
            keys,
            dev,
            Kind::Chat,
            &Body {
                id: "c1".into(),
                value: json!({ "turns": turns }),
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
    }

    #[test]
    fn divergent_turns_become_branches_with_the_origin_as_main() {
        let keys = SpaceKeys::generate().unwrap();
        let desk = Device::generate("Desktop").unwrap();
        let lap = Device::generate("Laptop").unwrap();
        let mut s = Store::default();
        let shared = msgs(&[("user", "hi"), ("assistant", "hello")]);
        s.insert(&keys, rec(&keys, &desk, 1, chain(&shared)))
            .unwrap();
        // The laptop adds a turn later in time than the desktop's own next turn, but the
        // desktop started the chat, so its line stays main.
        let mut d = shared.clone();
        d.extend(msgs(&[("user", "from desk")]));
        let mut l = shared.clone();
        l.extend(msgs(&[("user", "from laptop"), ("assistant", "sure")]));
        s.insert(&keys, rec(&keys, &lap, 2, chain(&l))).unwrap();
        s.insert(&keys, rec(&keys, &desk, 3, chain(&d))).unwrap();
        let tag = keys.tag(Kind::Chat, "c1").unwrap();
        let v = view(&s.item(Kind::Chat, &tag), &desk.id, None, &BTreeSet::new());
        assert_eq!(
            v.main
                .iter()
                .map(|t| t.turn.content.as_str())
                .collect::<Vec<_>>(),
            ["hi", "hello", "from desk"]
        );
        assert_eq!(v.branches.len(), 1);
        let b = &v.branches[0];
        assert_eq!(b.device, lap.id);
        assert_eq!(b.fork, 2);
        assert_eq!(b.turns.len(), 4);
        // Make the laptop's branch main.
        let tip = b.turns.last().unwrap().turn.h.clone();
        let v2 = view(
            &s.item(Kind::Chat, &tag),
            &desk.id,
            Some(&tip),
            &BTreeSet::new(),
        );
        assert_eq!(v2.main.last().unwrap().turn.content, "sure");
        assert_eq!(
            v2.branches[0].turns.last().unwrap().turn.content,
            "from desk"
        );
        // Delete the branch.
        let gone: BTreeSet<String> = [b.id().to_owned()].into();
        let v3 = view(&s.item(Kind::Chat, &tag), &desk.id, None, &gone);
        assert!(v3.branches.is_empty());
    }

    #[test]
    fn identical_turns_do_not_branch() {
        let keys = SpaceKeys::generate().unwrap();
        let a = Device::generate("A").unwrap();
        let b = Device::generate("B").unwrap();
        let mut s = Store::default();
        let line = msgs(&[("user", "q"), ("assistant", "a")]);
        s.insert(&keys, rec(&keys, &a, 1, chain(&line))).unwrap();
        s.insert(&keys, rec(&keys, &b, 2, chain(&line))).unwrap();
        let tag = keys.tag(Kind::Chat, "c1").unwrap();
        let v = view(&s.item(Kind::Chat, &tag), &a.id, None, &BTreeSet::new());
        assert_eq!(v.main.len(), 2);
        assert!(v.branches.is_empty());
    }
}
