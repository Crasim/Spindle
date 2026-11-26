use crate::{
    engine::Engine,
    ollama::Message,
    projects,
    providers::route,
    state::{Conversation, State, Store, ThreadInfo},
    vault,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    EditMessage {
        index: usize,
        original: String,
        replacement: String,
    },
    /// Replace a prompt and send it again: everything after it is discarded and the model
    /// answers with only the conversation before it.
    Resend {
        index: usize,
        original: String,
        replacement: String,
    },
    Edit {
        title: Option<String>,
        section: Option<String>,
        model: Option<String>,
    },
    Compaction(Option<crate::compaction::Policy>),
    CompactNow,
    Pin(bool),
    Unread(bool),
    Archive(bool),
    Branch,
    Move(PathBuf),
    BranchTo(PathBuf),
    SetProject(Option<PathBuf>),
    Delete,
}

impl State {
    /// Compaction policy a thread uses when it has no override of its own: the thread's
    /// model setting if one exists, otherwise the workspace default.
    /// The model a thread will use next.
    pub fn thread_model(&self, id: u64) -> &str {
        if id == self.chat_id {
            &self.model
        } else {
            self.chats
                .get(&id)
                .map(|c| &c.thread.model)
                .filter(|m| !m.is_empty())
                .unwrap_or(&self.model)
        }
    }

    pub fn default_compaction(&self, id: u64) -> crate::compaction::Policy {
        let model = self.thread_model(id);
        self.tuning
            .get(model)
            .and_then(|t| t.compaction)
            .unwrap_or(self.preferences.compaction)
    }

    /// Folder whose artifacts directory receives this thread's generated files and pastes.
    /// A project that is no longer available falls back to the workspace.
    pub fn files_root(&self, root: &Path, id: u64) -> PathBuf {
        self.thread_data(id)
            .and_then(|(t, _)| t.project.clone())
            .filter(|p| crate::projects::existing(p).ok().flatten().is_some())
            .unwrap_or_else(|| root.to_path_buf())
    }

    pub fn thread_data(&self, id: u64) -> Option<(&ThreadInfo, &[Message])> {
        if id == self.chat_id {
            Some((&self.thread, &self.history))
        } else {
            self.chats
                .get(&id)
                .map(|c| (&c.thread, c.history.as_slice()))
        }
    }
    fn thread_mut(&mut self, id: u64) -> Result<&mut ThreadInfo> {
        if id == self.chat_id {
            Ok(&mut self.thread)
        } else {
            Ok(&mut self
                .chats
                .get_mut(&id)
                .context("Thread no longer exists")?
                .thread)
        }
    }
    pub fn conversation(&self, id: u64) -> Result<Conversation> {
        if id != self.chat_id {
            return self
                .chats
                .get(&id)
                .cloned()
                .context("Thread no longer exists");
        }
        let mut thread = self.thread.clone();
        thread.model.clone_from(&self.model);
        Ok(Conversation {
            thread,
            history: self.history.clone(),
            goal: self.goal.clone(),
            plan: self.plan.clone(),
        })
    }
    pub fn thread_work(&self, id: u64) -> (usize, usize) {
        (
            self.queue
                .iter()
                .filter(|t| t.origin.unwrap_or(self.chat_id) == id)
                .count(),
            self.automation
                .schedules
                .iter()
                .filter(|s| s.chat == id)
                .count(),
        )
    }
    pub(crate) fn note_thread_update(&mut self, id: u64) {
        if self.viewed_chat != Some(id) {
            self.unread.insert(id);
        }
    }

    pub fn forget_thread(&mut self, id: u64) -> Result<()> {
        ensure!(self.thread_data(id).is_some(), "Thread no longer exists");
        let removes_front = self
            .queue
            .front()
            .is_some_and(|t| t.origin.unwrap_or(self.chat_id) == id);
        self.queue
            .retain(|t| t.origin.unwrap_or(self.chat_id) != id);
        self.automation.schedules.retain(|s| s.chat != id);
        if removes_front {
            self.automation.wake = None;
            self.held = !self.queue.is_empty();
        }
        self.unread.remove(&id);
        if id == self.chat_id {
            let next = self
                .chats
                .iter()
                .rev()
                .find(|(_, c)| !c.thread.archived)
                .map(|(&n, _)| n);
            self.switch_chat(next)?;
        }
        self.chats.remove(&id);
        Ok(())
    }
}

fn checked_label(input: &str, max: usize, empty: bool) -> Result<String> {
    let title = input.trim();
    ensure!(
        (empty || !title.is_empty())
            && title.chars().count() <= max
            && !title.chars().any(char::is_control),
        "Enter a single line of up to {max} characters"
    );
    Ok(title.into())
}

impl Engine {
    pub fn thread_action(&mut self, id: u64, action: Action) -> Result<String> {
        ensure!(
            self.state.thread_data(id).is_some(),
            "Thread no longer exists"
        );
        let result = match action {
            Action::EditMessage {
                index,
                original,
                replacement,
            } => {
                ensure!(!replacement.trim().is_empty(), "Enter some message text");
                let (thread, history) = if id == self.state.chat_id {
                    (&mut self.state.thread, &mut self.state.history)
                } else {
                    let chat = self
                        .state
                        .chats
                        .get_mut(&id)
                        .context("Thread no longer exists")?;
                    (&mut chat.thread, &mut chat.history)
                };
                let message = history.get_mut(index).context("Message no longer exists")?;
                ensure!(
                    matches!(message.role.as_str(), "user" | "assistant"),
                    "Select a completed chat message"
                );
                ensure!(
                    message.content == original || message.content == replacement,
                    "This message changed while you were editing it. Review the latest text."
                );
                if message.content != replacement {
                    message.content = replacement;
                    thread.edited_messages.insert(index);
                    if thread.summary.as_ref().is_some_and(|s| index < s.covered) {
                        thread.summary = None;
                    }
                }
                format!("Message {} saved", index + 1)
            }
            Action::Resend {
                index,
                original,
                replacement,
            } => {
                ensure!(!replacement.trim().is_empty(), "Enter some message text");
                ensure!(
                    id == self.state.chat_id,
                    "Open the conversation to send an edited prompt"
                );
                ensure!(
                    !self
                        .state
                        .queue
                        .iter()
                        .any(|t| t.origin.unwrap_or(self.state.chat_id) == id),
                    "Stop the reply in progress before sending an edited prompt"
                );
                let message = self
                    .state
                    .history
                    .get(index)
                    .context("Message no longer exists")?;
                ensure!(
                    message.role == "user",
                    "Only your own prompts can be sent again"
                );
                ensure!(
                    message.content == original,
                    "This message changed while you were editing it. Review the latest text."
                );
                let images = message.images.clone();
                let dropped = self.state.history.len() - index;
                self.state.history.truncate(index);
                let thread = &mut self.state.thread;
                // Forget everything recorded about the discarded messages.
                thread.turns.retain(|k, _| *k < index);
                thread.edited_messages.retain(|k| *k < index);
                thread.tool_results.retain(|k| *k < index);
                thread.compactions.retain(|k, _| *k <= index);
                thread.plan_notes.retain(|k, _| *k <= index);
                if thread.summary.as_ref().is_some_and(|s| s.covered > index) {
                    thread.summary = None;
                }
                self.state.requests.retain(|r| r.chat != id);
                self.enqueue(&replacement, self.state.thread.message_mode(), String::new())?;
                if let Some(task) = self.state.queue.back_mut() {
                    task.images = images;
                }
                format!(
                    "Sent the edited prompt; {} later message{} discarded",
                    dropped - 1,
                    if dropped == 2 { " was" } else { "s were" }
                )
            }
            Action::Edit {
                title,
                section,
                model,
            } => {
                let title = title
                    .as_deref()
                    .map(|s| checked_label(s, 100, false))
                    .transpose()?;
                let section = section
                    .as_deref()
                    .map(|s| checked_label(s, 48, true))
                    .transpose()?;
                if let Some(m) = &model {
                    ensure!(self.models.contains(m), "Select an available model");
                }
                let info = self.state.thread_mut(id)?;
                if let Some(t) = title {
                    info.title = t;
                }
                if let Some(s) = section {
                    info.section = s;
                }
                if let Some(m) = model {
                    info.model = m.clone();
                    if id == self.state.chat_id {
                        if route(&self.state.model).0 != route(&m).0
                            && self.state.attachment.is_some()
                        {
                            self.state.pending = self.state.attachment.take();
                        }
                        self.state.model = m;
                    }
                }
                "Settings saved".into()
            }
            Action::Compaction(policy) => {
                if let Some(policy) = policy {
                    policy.validate()?;
                }
                self.state.thread_mut(id)?.compaction = policy;
                format!(
                    "Context compaction · {}",
                    policy.unwrap_or(self.state.default_compaction(id)).label()
                )
            }
            Action::CompactNow => {
                let info = self.state.thread_data(id).unwrap().0;
                let model = if id == self.state.chat_id {
                    &self.state.model
                } else {
                    &info.model
                };
                let window = info
                    .turns
                    .values()
                    .rev()
                    .filter(|t| &t.model == model)
                    .find_map(|t| t.window)
                    .unwrap_or(crate::compaction::UNKNOWN_WINDOW);
                self.state.compact_now(id, window)?
            }
            Action::SetProject(path) => {
                let project = path
                    .map(|p| {
                        crate::projects::existing(&p)?.context("Choose an existing Spindle project")
                    })
                    .transpose()?;
                let label = project
                    .as_ref()
                    .map(|p| format!("Moved to project {}", p.name))
                    .unwrap_or_else(|| "Removed from project".into());
                self.state.thread_mut(id)?.project = project.map(|p| p.path);
                label
            }
            Action::Pin(value) => {
                self.state.thread_mut(id)?.pinned = value;
                "Pin updated".into()
            }
            Action::Unread(value) => {
                if value {
                    self.state.unread.insert(id);
                } else {
                    self.state.unread.remove(&id);
                }
                "Read status updated".into()
            }
            Action::Archive(value) => {
                self.state.thread_mut(id)?.archived = value;
                if value && id == self.state.chat_id {
                    self.state.switch_chat(None)?;
                }
                if value { "Archived" } else { "Restored" }.into()
            }
            Action::Branch => {
                let mut c = self.state.conversation(id)?;
                make_branch(&mut c);
                self.state.serial += 1;
                let new_id = self.state.serial;
                self.state.chats.insert(new_id, c);
                self.state.switch_chat(Some(new_id))?;
                "Branch opened".into()
            }
            Action::Move(ref path) | Action::BranchTo(ref path) => {
                let moving = matches!(action, Action::Move(_));
                if moving {
                    ensure!(
                        self.state.thread_work(id) == (0, 0),
                        "Finish or remove this thread’s queued work and schedules, or branch to the project now"
                    );
                }
                let dest = projects::existing(path)?
                    .context("Choose a folder containing a Spindle project")?;
                ensure!(
                    dest.path != self.root,
                    "Thread already belongs to this workspace"
                );
                let mut conversation = self.state.conversation(id)?;
                if !moving {
                    make_branch(&mut conversation);
                }
                for turn in conversation.thread.turns.values_mut() {
                    turn.files.clear();
                }
                conversation.thread.started = true;
                let scope = route(&conversation.thread.model).0;
                let provider = self
                    .state
                    .providers
                    .iter()
                    .find(|p| p.id == scope)
                    .map(|p| (p.base.clone(), p.protocol));
                let transfer = Transfer {
                    conversation,
                    provider,
                };
                if self.state.transfer_namespace.is_none() {
                    self.state.transfer_namespace = Some(vault::random()?);
                    if let Some(store) = &self.store {
                        store.save(&self.state)?;
                    }
                }
                let operation = if moving {
                    None
                } else {
                    Some(vault::random::<16>()?)
                };
                let key = ring::digest::digest(
                    &ring::digest::SHA256,
                    &serde_json::to_vec(&(
                        self.state.transfer_namespace,
                        id,
                        operation,
                        &transfer,
                    ))?,
                );
                let receipt: String = key.as_ref().iter().map(|b| format!("{b:02x}")).collect();
                deliver(&Store::directory(&dest.path)?, &receipt, &transfer)?;
                if moving {
                    self.delete_managed(id)?;
                }
                format!(
                    "{} to {}",
                    if moving { "Moved" } else { "Branched" },
                    dest.name
                )
            }
            Action::Delete => {
                self.delete_managed(id)?;
                "Deleted".into()
            }
        };
        Ok(format!("Thread #{id} · {result}"))
    }

    fn delete_managed(&mut self, id: u64) -> Result<()> {
        if let Some(pair) = &mut self.state.sync {
            pair.delete(id);
        }
        self.state.forget_thread(id)
    }
}

fn make_branch(c: &mut Conversation) {
    let label = c
        .thread
        .label(&c.history)
        .chars()
        .take(84)
        .collect::<String>();
    c.thread.title = format!("{label} (branch)");
    c.thread.started = true;
    c.thread.pinned = false;
    c.thread.archived = false;
    // A scratch branch may get a different artifact directory.
    for turn in c.thread.turns.values_mut() {
        turn.files.clear();
    }
}

#[derive(Serialize, Deserialize)]
struct Transfer {
    conversation: Conversation,
    provider: Option<(String, crate::providers::Protocol)>,
}

fn deliver(dir: &Path, receipt: &str, transfer: &Transfer) -> Result<()> {
    let dir = dir.join("incoming-threads");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{receipt}.thread"));
    let plain = serde_json::to_vec(transfer)?;
    let blob = vault::local(&plain, true)?;
    let mut file = tempfile::NamedTempFile::new_in(&dir)?;
    file.write_all(&blob)?;
    file.as_file().sync_all()?;
    if let Err(err) = file.persist_noclobber(&path) {
        if err.error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(err.into());
        }
        match fs::read(&path) {
            Ok(bytes) => ensure!(
                vault::local(&bytes, false)? == plain,
                "Conflicting transfer in destination inbox"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // The recipient already committed and acknowledged this receipt.
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn receive(store: &Store, state: &mut State) -> Result<usize> {
    let dir = store.session_dir().join("incoming-threads");
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut received = 0;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "thread") || !entry.file_type()?.is_file() {
            continue;
        }
        let receipt = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !state.received_threads.contains(&receipt) {
            let t: Transfer = serde_json::from_slice(&vault::local(&fs::read(&path)?, false)?)?;
            let mut c = t.conversation;
            if let Some((base, protocol)) = t.provider {
                let (_, tag) = route(&c.thread.model);
                c.thread.model = state
                    .providers
                    .iter()
                    .find(|p| {
                        p.base == base
                            && p.protocol == protocol
                            && p.models.iter().any(|m| m == tag)
                    })
                    .map(|p| format!("@{}/{tag}", p.id))
                    .unwrap_or_else(|| format!("@unavailable/{tag}"));
            }
            state.serial += 1;
            state.chats.insert(state.serial, c);
            state.unread.insert(state.serial);
            state.received_threads.insert(receipt);
            received += 1;
        }
        // Commit the imported conversation and receipt before acknowledging the delivery.
        store.save(state)?;
        fs::remove_file(path)?;
    }
    Ok(received)
}

pub fn transcript(title: &str, messages: &[Message]) -> String {
    let mut text = format!("# {title}\n");
    for m in messages {
        let who = match m.role.as_str() {
            "user"
                if m.content.starts_with("Tool result ")
                    || m.content.starts_with("Tool call not understood") =>
            {
                "Tool result"
            }
            "user" => "You",
            _ => "Assistant",
        };
        text.push_str(&format!("\n## {who}\n\n{}\n", m.content));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_into_an_open_workspace_is_encrypted_and_retryable() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, mut state) = Store::open_directory(tmp.path()).unwrap();
        let mut c = Conversation::default();
        c.thread.title = "Private sample".into();
        c.history = vec![
            Message::new("user", "question"),
            Message::new("assistant", "answer"),
        ];
        let t = Transfer {
            conversation: c,
            provider: None,
        };
        deliver(tmp.path(), "repeat", &t).unwrap();
        deliver(tmp.path(), "repeat", &t).unwrap();
        let bytes = fs::read(tmp.path().join("incoming-threads/repeat.thread")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("Private sample"));
        assert_eq!(receive(&store, &mut state).unwrap(), 1);
        deliver(tmp.path(), "repeat", &t).unwrap();
        assert_eq!(receive(&store, &mut state).unwrap(), 0);
        drop(store);
        let (_, state) = Store::open_directory(tmp.path()).unwrap();
        assert_eq!(state.chats.len(), 1);
        assert_eq!(
            state.chats.values().next().unwrap().thread.title,
            "Private sample"
        );
    }
}
