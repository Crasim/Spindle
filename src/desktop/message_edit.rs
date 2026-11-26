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
    pub fn observe(&mut self, view: &View) {
        self.0.retain(|&thread, e| {
            let Some(message) = view
                .state
                .thread_data(thread)
                .and_then(|(_, h)| h.get(e.index))
            else {
                return false;
            };
            if e.pending {
                if message.content == e.draft {
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
        if ui
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
        // For a prompt the user wrote: how many messages follow it. `None` for replies.
        later: Option<usize>,
        tx: &UnboundedSender<String>,
    ) -> bool {
        let Some(e) = self.0.get_mut(&thread).filter(|e| e.index == index) else {
            return false;
        };
        let id = ui.make_persistent_id(("edit-message", thread, index));
        if std::mem::take(&mut e.focus) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        egui::ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .max_height(260.)
            .show(ui, |ui| {
                ui.add_enabled(
                    !e.pending,
                    egui::TextEdit::multiline(&mut e.draft)
                        .id(id)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3),
                );
            });
        if let Some(later) = later {
            ui.weak(if later > 0 {
                format!("Send answers it again with only the conversation before it, replacing the {later} message{} after it. Save edit keeps them.", if later == 1 { "" } else { "s" })
            } else {
                "Send answers it again. Save edit only changes the text.".to_owned()
            });
        } else {
            ui.weak("Future prompts will use the edited text.");
        }
        let conflict = current != e.original;
        if conflict {
            ui.label("This message changed elsewhere. Copy your draft or reload the latest text.");
            ui.horizontal(|ui| {
                if ui.small_button("Copy my draft").clicked() {
                    ui.ctx().copy_text(e.draft.clone());
                }
                if ui.small_button("Reload latest").clicked() {
                    current.clone_into(&mut e.original);
                    current.clone_into(&mut e.draft);
                    e.error.clear();
                }
            });
        }
        let cancel = ui
            .horizontal(|ui| {
                let changed = !e.draft.trim().is_empty() && e.draft != e.original;
                if later.is_some()
                    && ui
                        .add_enabled(
                            !e.draft.trim().is_empty() && !conflict && !e.pending,
                            egui::Button::new(Text::new("Send").strong()),
                        )
                        .on_hover_text(
                            "Send the edited prompt with only the conversation before it",
                        )
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
                if ui
                    .add_enabled(
                        changed && !conflict && !e.pending,
                        egui::Button::new("Save edit"),
                    )
                    .clicked()
                {
                    let action = Action::EditMessage {
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
