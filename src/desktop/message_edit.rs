use super::*;
use crate::{ollama::Message, threads::Action};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct MessageEditors(BTreeMap<u64, Edit>);

struct Edit {
    index: usize,
    original: String,
    draft: String,
    focus: bool,
    pending: bool,
    error: String,
}

impl MessageEditors {
    pub(super) fn editing_row(&self, thread: u64) -> Option<usize> {
        self.0.get(&thread).map(|edit| edit.index)
    }

    pub fn observe(&mut self, view: &View) {
        self.0.retain(|&thread, e| {
            let Some(_) = view
                .state
                .thread_data(thread)
                .and_then(|(_, h)| h.get(e.index))
            else {
                return false;
            };
            if e.pending {
                if thread != view.state.chat_id
                    && view.notice
                        == format!("Thread #{thread} · Edited prompt sent as a new version")
                {
                    return false;
                }
                if view
                    .notice
                    .starts_with(&format!("Error: Thread #{thread}:"))
                {
                    e.error.clone_from(&view.notice);
                    e.pending = false;
                }
            }
            true
        });
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui, thread: u64, index: usize, msg: &Message) -> bool {
        let copying = small(ui, Icon::Copy, "Copy message text");
        if copying {
            ui.ctx().copy_text(msg.content.clone());
        }
        if msg.role == "user"
            && ui
                .add_enabled_ui(!self.0.contains_key(&thread), |ui| {
                    small(ui, Icon::Rename, "Edit message")
                })
                .inner
        {
            self.0.insert(
                thread,
                Edit {
                    index,
                    original: msg.content.clone(),
                    draft: msg.content.clone(),
                    focus: true,
                    pending: false,
                    error: String::new(),
                },
            );
        }
        copying
    }

    pub fn field(
        &mut self,
        ui: &mut egui::Ui,
        thread: u64,
        index: usize,
        current: &str,
        tx: &UnboundedSender<String>,
        ime: &mut android_ime::ComposerIme,
    ) -> bool {
        let Some(e) = self.0.get_mut(&thread).filter(|e| e.index == index) else {
            return false;
        };
        let id = ui.make_persistent_id(("edit-message", thread, index));
        egui::ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .max_height(260.)
            .show(ui, |ui| {
                let response = ime.multiline(ui, id, &mut e.draft, |field| {
                    field
                        .interactive(!e.pending)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                });
                if std::mem::take(&mut e.focus) {
                    response.request_focus();
                }
            });
        ui.weak("Send keeps both versions in this conversation.");
        let conflict = current != e.original;
        if conflict {
            ui.label(
                "This message changed elsewhere. Cancel and reopen the editor to use the new text.",
            );
        }
        let cancel = ui
            .horizontal(|ui| {
                if ui
                    .add_enabled(
                        !e.draft.trim().is_empty() && !conflict && !e.pending,
                        egui::Button::new(Text::new("Send").strong()),
                    )
                    .on_hover_text("Send the edited prompt with only the conversation before it")
                    .clicked()
                {
                    let action = Action::Resend {
                        index,
                        original: e.original.clone(),
                        replacement: e.draft.clone(),
                    };
                    e.pending = tx
                        .send(format!(
                            "/thread {thread} {}",
                            serde_json::to_string(&action).expect("Message edit serializes")
                        ))
                        .is_ok();
                    if !e.pending {
                        e.error =
                            "Session engine is unavailable. Copy your draft before closing.".into();
                    }
                }
                ui.add_enabled(!e.pending, egui::Button::new("Cancel"))
                    .clicked()
            })
            .inner;
        if !e.error.is_empty() {
            ui.colored_label(accent(), &e.error);
        }
        if cancel {
            self.0.remove(&thread);
        }
        true
    }
}

pub(super) fn versions(
    ui: &mut egui::Ui,
    state: &crate::state::State,
    index: usize,
    tx: &UnboundedSender<String>,
) {
    let Some(version) = state.thread.prompt_versions.get(&index) else {
        return;
    };
    let ids = state.prompt_versions(&version.group);
    let Some(current) = ids.iter().position(|id| *id == state.chat_id) else {
        return;
    };
    if ids.len() < 2 {
        return;
    }
    ui.horizontal(|ui| {
        let previous = current.checked_sub(1).and_then(|i| ids.get(i));
        let next = ids.get(current + 1);
        for (label, hint, target) in [
            ("‹", "Previous version", previous),
            ("›", "Next version", next),
        ] {
            if label == "›" {
                ui.weak(format!("{}/{}", current + 1, ids.len()));
            }
            if ui
                .add_enabled(target.is_some(), egui::Button::new(label).small())
                .on_hover_text(hint)
                .clicked()
                && let Some(id) = target
            {
                let _ = tx.send(format!("/open {id}"));
            }
        }
    });
}
