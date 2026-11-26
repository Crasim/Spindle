use super::*;
use crate::pastes::Submission;
use base64::Engine as _;

pub(super) type ImageDraft = (String, Vec<u8>, Option<egui::TextureHandle>);

pub(super) enum Dispatch {
    Handled,
    Sent,
    Rejected,
}

enum Destination {
    Chat(u64),
    Project(PathBuf),
}

struct Draft {
    destination: Destination,
    text: String,
    files: Vec<paste::Draft>,
    images: Vec<ImageDraft>,
}

#[derive(Default)]
pub(super) struct Submissions {
    next: u64,
    pending: Option<(u64, Draft)>,
}

fn restore_text(target: &mut String, mut original: String) {
    if !target.is_empty() {
        if !original.is_empty() {
            original.push('\n');
        }
        original.push_str(target);
    }
    *target = original;
}

impl Desktop {
    pub(super) fn awaiting_submission(&self) -> bool {
        self.submissions.pending.is_some()
    }

    pub(super) fn submit_chat(&mut self, steer: bool) {
        if self.awaiting_submission() {
            return;
        }
        let thread = self.view.state.chat_id;
        let has_files = self.paste_drafts.files.contains_key(&thread);
        if steer && (has_files || !self.image_drafts.is_empty()) {
            self.notice = "Send attachments as a new message.".into();
            return;
        }
        let text = std::mem::take(&mut self.draft);
        let images = std::mem::take(&mut self.image_drafts);
        let files = if images.is_empty() {
            self.paste_drafts.files.remove(&thread).unwrap_or_default()
        } else {
            vec![]
        };
        let command = if !images.is_empty() {
            let encoded: Vec<_> = images
                .iter()
                .map(|(_, bytes, _)| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect();
            format!(
                "/send-images {}",
                serde_json::json!({"prompt": text, "images": encoded})
            )
        } else if !files.is_empty() {
            let submission = Submission {
                thread,
                model: self.view.state.model.clone(),
                prompt: text.clone(),
                names: files.iter().map(|file| file.name.clone()).collect(),
                pastes: files.iter().map(|file| file.body.clone()).collect(),
            };
            format!(
                "/send-pastes {}",
                serde_json::to_string(&submission).expect("Serializable draft")
            )
        } else if steer {
            format!("/steer {text}")
        } else {
            text.clone()
        };
        self.start_submission(
            command,
            Draft {
                destination: Destination::Chat(thread),
                text,
                files,
                images,
            },
        );
        self.focus = true;
    }

    pub(super) fn submit_project_prompt(&mut self, path: PathBuf, text: String) {
        let command = format!("/project-prompt {}", serde_json::json!([path, text]));
        self.start_submission(
            command,
            Draft {
                destination: Destination::Project(path),
                text,
                files: vec![],
                images: vec![],
            },
        );
    }

    fn start_submission(&mut self, command: String, draft: Draft) {
        self.submissions.next += 1;
        let id = self.submissions.next;
        self.submissions.pending = Some((id, draft));
        match self.dispatch(command, Some(id)) {
            Dispatch::Handled => {
                self.submissions.pending = None;
            }
            Dispatch::Rejected => self.reject_submission(),
            Dispatch::Sent => {}
        }
    }

    pub(super) fn reject_submission(&mut self) {
        let Some((_, mut draft)) = self.submissions.pending.take() else {
            return;
        };
        match draft.destination {
            Destination::Chat(thread) => {
                if thread == self.view.state.chat_id {
                    restore_text(&mut self.draft, draft.text);
                    draft.images.append(&mut self.image_drafts);
                    self.image_drafts = draft.images;
                } else {
                    restore_text(self.drafts_by_thread.entry(thread).or_default(), draft.text);
                    let images = self.images_by_thread.entry(thread).or_default();
                    draft.images.append(images);
                    *images = draft.images;
                }
                if !draft.files.is_empty() {
                    let files = self.paste_drafts.files.entry(thread).or_default();
                    draft.files.append(files);
                    *files = draft.files;
                }
            }
            Destination::Project(path) => {
                restore_text(self.projects.composers.entry(path).or_default(), draft.text);
            }
        }
    }

    pub(super) fn observe_submission(&mut self, view: &View) {
        let Some((id, accepted)) = view.composer_receipt else {
            return;
        };
        if self
            .submissions
            .pending
            .as_ref()
            .is_none_or(|(pending, _)| *pending != id)
        {
            return;
        }
        if accepted {
            let (_, draft) = self.submissions.pending.take().unwrap();
            if let Destination::Project(path) = draft.destination
                && self.page == Page::Project
                && self.projects.home.as_ref().is_some_and(|p| p.path == path)
            {
                self.page = Page::Chat;
                self.focus = true;
            }
        } else {
            self.reject_submission();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_restores_the_exact_draft_and_attachments_to_their_chat() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let root = tempfile::tempdir().unwrap();
        let mut app = Desktop::new(&cc, root.path().into(), tx, rx, preview, Accent::Blue);
        app.draft = "  First line\nsecond line  ".into();
        app.image_drafts
            .push(("sample.png".into(), vec![10, 20], None));
        app.submit_chat(false);
        assert!(
            commands
                .try_recv()
                .unwrap()
                .starts_with("/ui-submit 1 0 /send-images ")
        );
        app.observe_submission(&View {
            composer_receipt: Some((44, false)),
            ..View::default()
        });
        assert!(app.awaiting_submission());
        app.drafts_by_thread.insert(0, "Typed meanwhile".into());
        app.view.state.chat_id = 1;
        app.draft = "Unrelated draft".into();
        app.observe_submission(&View {
            composer_receipt: Some((1, false)),
            ..View::default()
        });
        assert_eq!(
            app.drafts_by_thread[&0],
            "  First line\nsecond line  \nTyped meanwhile"
        );
        assert_eq!(app.images_by_thread[&0][0].1, [10, 20]);
        assert_eq!(app.draft, "Unrelated draft");
        app.submit_chat(false);
        assert_eq!(
            commands.try_recv().unwrap(),
            "/ui-submit 2 1 Unrelated draft"
        );
        app.draft = "Next draft".into();
        app.observe_submission(&View {
            composer_receipt: Some((2, true)),
            ..View::default()
        });
        assert_eq!(app.draft, "Next draft");
        assert!(!app.awaiting_submission());
    }

    #[test]
    fn local_and_channel_errors_keep_the_draft() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let (tx, commands) = tokio::sync::mpsc::unbounded_channel();
        let (_views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let root = tempfile::tempdir().unwrap();
        let mut app = Desktop::new(&cc, root.path().into(), tx, rx, preview, Accent::Blue);
        for draft in ["/preview missing.html", "Please keep this draft"] {
            app.draft = draft.into();
            if draft.starts_with('/') {
                app.submit_chat(false);
                assert_eq!(app.draft, draft);
                assert!(!app.awaiting_submission());
                continue;
            }
            drop(commands);
            app.submit_chat(false);
            assert_eq!(app.draft, draft);
            assert!(!app.awaiting_submission());
            break;
        }
    }
}
