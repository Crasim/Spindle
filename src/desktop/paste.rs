use super::*;
use crate::pastes;
use std::collections::BTreeMap;

/// A file waiting to go with the next message: pasted text (no name) or an attached file.
#[derive(Clone, Default)]
pub(super) struct Draft {
    pub name: String,
    pub body: String,
}

#[derive(Default)]
pub(super) struct PasteDrafts {
    pub files: BTreeMap<u64, Vec<Draft>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn post_reply_typing_recovers_the_first_key_and_respects_other_editors() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let root = tempfile::tempdir().unwrap();
        let (tx, mut input) = tokio::sync::mpsc::unbounded_channel();
        let (_out, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, root.path().into(), tx, rx, preview, Accent::Blue);
        app.view.state.history = vec![
            crate::ollama::Message::new("user", "First"),
            crate::ollama::Message::new("assistant", "Reply"),
        ];
        let other_id = egui::Id::new("other-editor");
        let mut other_text = String::new();
        let mut frame = |app: &mut Desktop, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                app.composer(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut other_text).id(other_id));
                });
            });
        };
        frame(&mut app, vec![]);
        let composer = ctx.memory(|m| m.focused()).unwrap();
        ctx.memory_mut(|m| m.surrender_focus(composer));
        for c in "Next".chars() {
            frame(&mut app, vec![egui::Event::Text(c.to_string())]);
        }
        assert_eq!(app.draft, "Next");
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(input.try_recv().unwrap(), "/ui-submit 1 0 Next");
        frame(
            &mut app,
            vec![egui::Event::Text("First key after send".into())],
        );
        assert_eq!(app.draft, "First key after send");
        ctx.memory_mut(|m| m.request_focus(other_id));
        frame(
            &mut app,
            vec![egui::Event::Text("editing another field".into())],
        );
        assert_eq!(other_text, "editing another field");
        assert_eq!(app.draft, "First key after send");
    }

    #[test]
    fn composer_keeps_large_paste_out_of_the_text_and_restores_a_failed_submission() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let root = tempfile::tempdir().unwrap();
        let (tx, _input) = tokio::sync::mpsc::unbounded_channel();
        let (_out, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, root.path().into(), tx, rx, preview, Accent::Blue);
        let frame = |app: &mut Desktop, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 650.),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.composer(ctx));
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Paste("short".into())]);
        assert_eq!(app.draft, "short");
        frame(
            &mut app,
            vec![egui::Event::Paste("large paste ".repeat(500))],
        );
        assert_eq!(app.draft, "short");
        assert_eq!(app.paste_drafts.files[&0].len(), 1);
        assert!(!root.path().join("artifacts").exists());
        app.submit_chat(false);
        assert!(app.draft.is_empty());
        app.reject_submission();
        assert_eq!(app.draft, "short");
        assert_eq!(app.paste_drafts.files[&0][0].body.len(), 6000);
    }

    #[test]
    fn user_rows_start_to_the_right_and_fit_next_to_the_panel() {
        for width in [350., 950.] {
            let ctx = egui::Context::default();
            let mut rows = vec![];
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 550.),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for user in [true, false] {
                        message_row(ui, user, |ui| {
                            rows.push(ui.max_rect());
                            ui.label("A short reply");
                        });
                    }
                });
            });
            let user = rows[rows.len() - 2];
            let assistant = rows[rows.len() - 1];
            assert!(user.left() > assistant.left() + 12.);
            assert!(user.right() <= width && assistant.right() < user.right());
        }
    }
}

impl Desktop {
    pub(super) fn paste_chips(&mut self, ui: &mut egui::Ui, id: egui::Id) {
        let thread = self.view.state.chat_id;
        if ui.memory(|m| m.has_focus(id)) {
            ui.input_mut(|input| input.events.retain(|event|{
                let egui::Event::Paste(body)=event else{return true};
                if !pastes::large(body){return true;}
                let files=self.paste_drafts.files.entry(thread).or_default();
                if files.len()>=pastes::MAX_FILES || files.iter().map(|f|f.body.len()).sum::<usize>()+body.len()>pastes::MAX_BYTES {
                    self.notice=format!("A message supports {} files totaling {} MiB. The text is still on your clipboard.",pastes::MAX_FILES,pastes::MAX_BYTES>>20);
                } else { files.push(Draft{name:String::new(),body:body.clone()}); }
                false
            }));
        }
        let Some(files) = self.paste_drafts.files.get_mut(&thread) else {
            return;
        };
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(Text::new("Attached:").size(13.).color(muted()));
            for (index, file) in files.iter().enumerate() {
                let title = if file.name.is_empty() {
                    format!("Pasted text {}", index + 1)
                } else {
                    file.name.clone()
                };
                if ui
                    .small_button(format!(
                        "📎 {title} · {:.1} KB",
                        file.body.len() as f64 / 1024.
                    ))
                    .on_hover_text("Goes to the model with your next message. Click to review.")
                    .clicked()
                {
                    let name = if file.name.is_empty() {
                        format!("{title}.txt")
                    } else {
                        title.clone()
                    };
                    self.reader.open_output(name, file.body.clone());
                }
                if ui
                    .small_button("×")
                    .on_hover_text(format!("Remove {title}"))
                    .clicked()
                {
                    remove = Some(index);
                }
            }
        });
        if let Some(index) = remove {
            files.remove(index);
        }
        if files.is_empty() {
            self.paste_drafts.files.remove(&thread);
        }
    }

    /// Attach a file from disk to the next message, as a chip above the message box.
    pub(super) fn attach_file(&mut self, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if crate::speech::is_audio(&name) {
            let settings = self.speech_settings();
            self.notice = match self.voice.attach(settings, self.view.state.chat_id, path) {
                Ok(n) => format!("Transcribing {n}…"),
                Err(e) => format!("Couldn't attach {name}: {e:#}"),
            };
            return;
        }
        let body = match pastes::file_text(path) {
            Ok(body) => body,
            Err(e) => {
                self.notice = format!("Couldn't attach {name}: {e:#}");
                return;
            }
        };
        let files = self
            .paste_drafts
            .files
            .entry(self.view.state.chat_id)
            .or_default();
        if files.len() >= pastes::MAX_FILES
            || files.iter().map(|f| f.body.len()).sum::<usize>() + body.len() > pastes::MAX_BYTES
        {
            self.notice = format!(
                "A message supports {} attached files totaling {} MiB",
                pastes::MAX_FILES,
                pastes::MAX_BYTES >> 20
            );
            return;
        }
        files.retain(|f| f.name != name);
        files.push(Draft {
            name: name.clone(),
            body,
        });
        self.notice = format!("Attached {name}. It goes with your next message.");
        self.focus = true;
    }

}
