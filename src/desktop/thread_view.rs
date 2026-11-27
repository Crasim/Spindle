use super::*;
use crate::{
    projects::{self, Project},
    threads::{Action, transcript},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Threads {
    editors: BTreeMap<u64, Editor>,
    reader: Option<u64>,
    pub(super) archived: bool,
    collapsed: BTreeSet<std::path::PathBuf>,
    projects_hidden: bool,
    pub(super) importing: bool,
    asked_to_import: bool,
    import_path: String,
    #[cfg(not(target_os = "android"))]
    local_import: LocalImport,
}

#[cfg(not(target_os = "android"))]
#[derive(Default)]
struct LocalImport {
    pending: Option<std::sync::mpsc::Receiver<Vec<crate::conversation_import::LocalSession>>>,
    sessions: Vec<crate::conversation_import::LocalSession>,
    source: usize,
    scanned: bool,
    filter: String,
    selected: BTreeSet<String>,
}

#[cfg(not(target_os = "android"))]
impl LocalImport {
    fn start(&mut self) {
        self.selected.clear();
        self.sessions.clear();
        self.scanned = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending = Some(rx);
        let source = if self.source == 0 {
            crate::conversation_import::Source::Codex
        } else {
            crate::conversation_import::Source::ClaudeCode
        };
        std::thread::spawn(move || { let _ = tx.send(source.discover()); });
    }

    fn show(&mut self, ui: &mut egui::Ui, state: &crate::state::State) -> Option<String> {
        let mut refresh = !self.scanned;
        ui.horizontal_wrapped(|ui| {
            refresh |= ui.selectable_value(&mut self.source, 0, "Codex").changed();
            refresh |= ui.selectable_value(&mut self.source, 1, "Claude Code").changed();
            refresh |= ui.button("Rescan").clicked();
        });
        if refresh {
            self.start();
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(sessions) => {
                    self.sessions = sessions;
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    return None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self.sessions.is_empty() {
            ui.weak("No local sessions found. For a Claude account export, use Choose file.");
            return None;
        }
        let imported: BTreeSet<&str> = std::iter::once(&state.thread)
            .chain(state.chats.values().map(|chat| &chat.thread))
            .filter_map(|thread| thread.imported_from.as_deref()).collect();
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Find a local conversation"));
        let filter = self.filter.to_lowercase();
        let matches = self.sessions.iter().filter(|s| s.title.to_lowercase().contains(filter.trim()));
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Select matches").clicked() {
                self.selected.extend(matches.clone().filter(|s| !imported.contains(s.origin.as_str()))
                    .map(|s| s.origin.clone()));
            }
            if ui.small_button("Clear selection").clicked() {
                self.selected.clear();
            }
        });
        egui::ScrollArea::vertical().id_salt("local-import-results").max_height(290.).show(ui, |ui| {
            for session in matches {
                ui.push_id(&session.origin, |ui| {
                    let already = imported.contains(session.origin.as_str());
                    let mut checked = already || self.selected.contains(&session.origin);
                    let day: chrono::DateTime<chrono::Local> = session.modified.into();
                    let text = format!("{} · {}{}", session.title, day.format("%b %d, %Y"),
                        if already { " · Imported" } else if session.archived { " · Archived" } else { "" });
                    if ui.add_enabled(!already, egui::Checkbox::new(&mut checked, text)).changed() {
                        if checked { self.selected.insert(session.origin.clone()); }
                        else { self.selected.remove(&session.origin); }
                    }
                });
            }
        });
        let count = self.selected.len();
        if ui.add_enabled(count > 0, egui::Button::new(format!("Import selected ({count})"))).clicked() {
            let selected = self.sessions.iter().filter(|s| self.selected.contains(&s.origin))
                .map(crate::conversation_import::Selection::from).collect::<Vec<_>>();
            return Some(format!("/import-chat-local {}", serde_json::json!(selected)));
        }
        None
    }
}

enum Entry<R> {
    ProjectsHeader(usize, bool),
    /// "Chats" with a + for a new chat outside any project.
    ChatsHeader,
    Heading(String, bool),
    Project(std::path::PathBuf, String, usize, bool),
    Row(R),
}
struct Editor {
    title: String,
    saved_title: String,
    focus_name: bool,
    model: String,
    saved_model: String,
    section: String,
    saved_section: String,
    projects: Vec<Project>,
    project: Option<PathBuf>,
    saved_project: Option<PathBuf>,
    confirm_delete: bool,
    feedback: String,
}
fn action(tx: &UnboundedSender<String>, id: u64, a: Action) {
    let _ = tx.send(format!(
        "/thread {id} {}",
        serde_json::to_string(&a).expect("Serializable thread action")
    ));
}
/// A clickable, left-aligned sidebar label that leaves room for one trailing button.
fn left_label(ui: &mut egui::Ui, text: Text) -> egui::Response {
    left_label_with(ui, text, 1)
}

/// A left-aligned sidebar label that leaves room for `buttons` trailing icon buttons.
fn left_label_with(ui: &mut egui::Ui, text: Text, buttons: usize) -> egui::Response {
    let width =
        (ui.available_width() - buttons as f32 * (32. + ui.spacing().item_spacing.x)).max(30.);
    ui.allocate_ui_with_layout(
        egui::vec2(width, 27.),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, 27.));
            ui.add(
                egui::Label::new(text)
                    .truncate()
                    .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
        },
    )
    .inner
}

pub(super) fn thread_button(
    ui: &mut egui::Ui,
    title: Text,
    size: egui::Vec2,
    selected: bool,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Min),
        |ui| {
            ui.spacing_mut().button_padding = egui::vec2(9., 5.);
            let inactive = &mut ui.visuals_mut().widgets.inactive;
            inactive.weak_bg_fill = Color::TRANSPARENT;
            inactive.bg_stroke = egui::Stroke::NONE;
            ui.add(
                egui::Button::new(title)
                    .truncate()
                    .frame(true)
                    .corner_radius(8)
                    .selected(selected)
                    .min_size(size),
            )
        },
    )
    .inner
}

pub(super) fn unread_dot(ui: &mut egui::Ui, unread: bool) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(7., 24.), egui::Sense::hover());
    if unread {
        ui.painter().circle_filled(rect.center(), 3.5, accent());
        response.on_hover_text("Unread");
    }
}

fn menu_item(ui: &mut egui::Ui, icon: Icon, label: &str) -> bool {
    control(ui, icon, label, egui::vec2(ui.available_width().min(188.), 30.), false).clicked()
}

fn row_options(ui: &mut egui::Ui, toggle: bool, title: &str, items: impl FnOnce(&mut egui::Ui)) {
    let id = ui.id();
    let slot = egui::Id::new("sidebar-row-options");
    let selected = ui.data(|data| data.get_temp::<egui::Id>(slot));
    if toggle {
        ui.data_mut(|data| {
            if selected == Some(id) {
                data.remove::<egui::Id>(slot);
            } else {
                data.insert_temp(slot, id);
            }
        });
    }
    if ui.data(|data| data.get_temp::<egui::Id>(slot)) == Some(id) {
        ui.group(|ui| {
            if super::panel_header(ui, title, "Close") {
                ui.data_mut(|data| data.remove::<egui::Id>(slot));
            } else {
                items(ui);
            }
        });
    }
}

fn close_row_options(ui: &egui::Ui) {
    ui.data_mut(|data| data.remove::<egui::Id>(egui::Id::new("sidebar-row-options")));
}

#[cfg(windows)]
fn export_transcript(
    id: u64,
    title: &str,
    history: &[crate::ollama::Message],
) -> anyhow::Result<Option<PathBuf>> {
    let stem: String = title
        .chars()
        .take(60)
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let Some(path) = rfd::FileDialog::new()
        .set_title("Export a plaintext Markdown transcript")
        .set_file_name(format!("Thread-{id}-{stem}.md"))
        .add_filter("Markdown", &["md"])
        .save_file()
    else {
        return Ok(None);
    };
    super::export_skill(&path, &transcript(title, history))?;
    Ok(Some(path))
}
impl Editor {
    fn refresh_project(&mut self, current: Option<&std::path::Path>) {
        if self.project == self.saved_project {
            self.project = current.map(Into::into);
        }
        self.saved_project = current.map(Into::into);
    }

    fn refresh_fields(&mut self, title: &str, section: &str, model: &str) {
        // Keep the user's edits when the first reply supplies a title or sync updates the thread.
        for (field, saved, current) in [
            (&mut self.title, &mut self.saved_title, title),
            (&mut self.section, &mut self.saved_section, section),
            (&mut self.model, &mut self.saved_model, model),
        ] {
            if field == saved {
                current.clone_into(field);
            }
            current.clone_into(saved);
        }
    }

    fn delete_confirmation(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        label: &str,
        work: (usize, usize),
        tx: &UnboundedSender<String>,
    ) {
        let danger = |label| Text::new(label).color(Color::LIGHT_RED);
        let (tasks, schedules) = work;
        ui.strong(format!(
            "Delete “{}”?",
            label.chars().take(60).collect::<String>()
        ));
        ui.label(format!("Permanently removes this thread from the list, including {tasks} queued or running tasks and {schedules} schedules. Paired devices apply the deletion when they sync. Earlier encrypted revisions and saved files stay on disk."));
        ui.horizontal(|ui| {
            if ui.button("Keep conversation").clicked() {
                self.confirm_delete = false;
            }
            if ui.button(danger("Delete conversation")).clicked() {
                action(tx, id, Action::Delete);
                self.confirm_delete = false;
            }
        });
    }
}
impl Threads {
    pub(super) fn open_import_request(&mut self, source: &str) {
        self.importing = true;
        self.asked_to_import = true;
        #[cfg(not(target_os = "android"))]
        {
            self.local_import.source = usize::from(source == "claude");
            self.local_import.scanned = false;
        }
        #[cfg(target_os = "android")]
        let _ = source;
    }

    pub(super) fn has_editor(&self) -> bool {
        self.reader.is_some() || !self.editors.is_empty()
    }


    pub fn observe(&mut self, v: &View) {
        if v.notice.starts_with("Imported ") && v.notice.contains("conversation") {
            self.importing = false;
            self.asked_to_import = false;
            self.import_path.clear();
            #[cfg(not(target_os = "android"))]
            self.local_import.selected.clear();
        }
        self.editors.retain(|id, e| {
            if v.state.thread_data(*id).is_none() {
                return false;
            }
            if v.notice.starts_with(&format!("Thread #{id} · "))
                || v.notice.starts_with(&format!("Error: Thread #{id}:"))
            {
                e.feedback = v.notice.clone();
            }
            true
        });
        self.reader = self.reader.filter(|id| v.state.thread_data(*id).is_some());
    }
}
impl Desktop {
    pub(super) fn edit_thread(&mut self, id: u64) {
        if let Some(editor) = self.threads.editors.get_mut(&id) {
            editor.focus_name = !cfg!(target_os = "android");
            return;
        }
        let Some((thread, history)) = self.view.state.thread_data(id) else {
            return;
        };
        let title = thread.label(history).to_owned();
        let model = if id == self.view.state.chat_id {
            &self.view.state.model
        } else {
            &thread.model
        };
        let current = projects::existing(&self.root).ok().flatten();
        let library = projects::library(current.as_ref());
        let mut feedback = String::new();
        let projects = library
            .unwrap_or_else(|e| {
                feedback = e.to_string();
                vec![]
            })
            .into_iter()
            .filter(|p| p.path != self.root)
            .collect();
        self.threads.editors.insert(
            id,
            Editor {
                saved_title: title.clone(),
                focus_name: !cfg!(target_os = "android"),
                title,
                saved_model: model.clone(),
                model: model.clone(),
                saved_section: thread.section.clone(),
                section: thread.section.clone(),
                projects,
                project: thread.project.clone(),
                saved_project: thread.project.clone(),
                confirm_delete: false,
                feedback,
            },
        );
    }

    pub(super) fn thread_list(&mut self, ui: &mut egui::Ui) {
        let mut edit = None;
        let mut delete_dialog = None;
        let state = &self.view.state;
        let mut rows: Vec<_> = std::iter::once(state.chat_id)
            .chain(state.chats.keys().copied())
            .filter_map(|id| {
                let (t, h) = state.thread_data(id)?;
                let visible = id != state.chat_id
                    || t.started
                    || !h.is_empty()
                    || !t.title.is_empty()
                    || t.pinned
                    || !t.section.is_empty()
                    || t.project.is_some()
                    || !state.goal.is_empty()
                    || state.thread_work(id) != (0, 0);
                visible.then(|| {
                    (
                        id,
                        t.pinned,
                        t.archived,
                        t.section.clone(),
                        t.label(h).to_owned(),
                        state.unread.contains(&id),
                        t.project.clone(),
                    )
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.3.cmp(&b.3))
                .then_with(|| b.0.cmp(&a.0))
        });
        ui.add_space(6.);
        ui.horizontal(|ui| {
            ui.strong(if self.threads.archived {
                "Archive"
            } else {
                "Threads"
            });
            if self.threads.archived || rows.iter().any(|r| r.2) {
                let label = if self.threads.archived {
                    "Show threads"
                } else {
                    "Archive"
                };
                if ui.small_button(label).clicked() {
                    self.threads.archived = !self.threads.archived;
                }
            }
        });
        let archived = self.threads.archived;
        let rows: Vec<_> = rows.into_iter().filter(|r| r.2 == archived).collect();
        let mut projects: Vec<(std::path::PathBuf, String)> = self
            .projects
            .recent
            .iter()
            .filter(|p| p.path != self.root)
            .map(|p| (p.path.clone(), p.name.clone()))
            .collect();
        for r in &rows {
            if let Some(p) = &r.6 {
                if !projects.iter().any(|(q, _)| q == p) {
                    let name = p.file_name().unwrap_or_default().to_string_lossy().into();
                    projects.push((p.clone(), name));
                }
            }
        }
        let mut entries = vec![];
        let any_project = !archived || !projects.is_empty();
        let hidden = self.threads.projects_hidden;
        if any_project {
            entries.push(Entry::ProjectsHeader(projects.len(), hidden));
        }
        for (path, name) in projects.into_iter().filter(|_| !hidden) {
            let mine: Vec<_> = rows
                .iter()
                .filter(|r| r.6.as_ref() == Some(&path))
                .cloned()
                .collect();
            if archived && mine.is_empty() {
                continue;
            }
            let collapsed = self.threads.collapsed.contains(&path);
            entries.push(Entry::Project(path, name, mine.len(), collapsed));
            if !collapsed {
                entries.extend(mine.into_iter().map(Entry::Row));
            }
        }
        let mut group: Option<String> = None;
        let _ = any_project;
        entries.push(Entry::ChatsHeader);
        for r in rows.into_iter().filter(|r| r.6.is_none()) {
            let heading = if r.1 {
                "Pinned".to_owned()
            } else if r.3.is_empty() {
                "Recent".to_owned()
            } else {
                r.3.clone()
            };
            if group.as_deref() != Some(heading.as_str()) {
                group = Some(heading.clone());
                entries.push(Entry::Heading(heading, false));
            }
            entries.push(Entry::Row(r));
        }
        let library: Vec<(std::path::PathBuf, String)> = self
            .projects
            .recent
            .iter()
            .filter(|p| p.path != self.root)
            .map(|p| (p.path.clone(), p.name.clone()))
            .collect();
        let mut toggle = None;
        let mut start_in = None;
        let mut toggle_projects = false;
        let mut manage = false;
        let mut new_chat = false;
        let mut open_home = None;
        let mut project_menu = None;
        // Chats with work in progress show a spinner instead of their options icon; chats
        // waiting for an approval show a dot.
        let working: std::collections::BTreeSet<u64> = state
            .queue
            .front()
            .filter(|_| self.view.busy)
            .map(|t| t.origin.unwrap_or(state.chat_id))
            .into_iter()
            .chain(state.requests.iter().filter(|r| r.approved).map(|r| r.chat))
            .collect();
        let waiting: std::collections::BTreeSet<u64> = state
            .requests
            .iter()
            .filter(|r| !r.approved)
            .map(|r| r.chat)
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("thread-list")
            .max_height((ui.available_height() - 94.).max(45.))
            .show(ui, |ui| {
                for entry in entries {
                    let (id, pinned, _, _, title, unread, in_project) = match entry {
                        Entry::ProjectsHeader(count, hidden) => {
                            ui.add_space(4.);
                            ui.horizontal(|ui| {
                                let label = Text::new(if hidden {
                                    format!("Projects  ·  {count}")
                                } else {
                                    "Projects".to_owned()
                                })
                                .size(14.)
                                .strong();
                                if left_label(ui, label)
                                    .on_hover_text(if hidden {
                                        "Show projects"
                                    } else {
                                        "Hide projects"
                                    })
                                    .clicked()
                                {
                                    toggle_projects = true;
                                }
                                if small(ui, Icon::New, "Create a project") {
                                    manage = true;
                                }
                            });
                            continue;
                        }
                        Entry::ChatsHeader => {
                            ui.add_space(4.);
                            ui.horizontal(|ui| {
                                left_label_with(ui, Text::new("Chats").size(14.).strong(), 2);
                                if small(ui, Icon::Import, "Import conversations") {
                                    self.threads.importing = true;
                                    self.notice.clear();
                                    self.drawer = false;
                                }
                                if small(ui, Icon::New, "New chat outside any project (Ctrl+N)") {
                                    new_chat = true;
                                }
                            });
                            continue;
                        }
                        Entry::Heading(text, major) => {
                            let text = Text::new(text).color(muted());
                            ui.label(if major {
                                text.size(12.).strong()
                            } else {
                                text.size(11.)
                            });
                            continue;
                        }
                        Entry::Project(path, name, count, collapsed) => {
                            ui.push_id(("project", &path), |ui| {
                                let options = ui
                                    .horizontal(|ui| {
                                        let (rect, icon) = ui.allocate_exact_size(
                                            egui::vec2(18., 18.),
                                            egui::Sense::click(),
                                        );
                                        draw(
                                            ui,
                                            Icon::Folder,
                                            rect,
                                            if collapsed { muted() } else { accent() },
                                        );
                                        if icon
                                            .on_hover_text(if collapsed {
                                                "Show chats"
                                            } else {
                                                "Hide chats"
                                            })
                                            .clicked()
                                        {
                                            toggle = Some(path.clone());
                                        }
                                        let label = if count == 0 {
                                            name.clone()
                                        } else {
                                            format!("{name}  ·  {count}")
                                        };
                                        let row = left_label_with(
                                            ui,
                                            Text::new(label).size(14.).strong(),
                                            2,
                                        );
                                        let secondary = row.secondary_clicked();
                                        if row
                                            .on_hover_text(projects::display_path(&path))
                                            .clicked()
                                        {
                                            open_home = Some(path.clone());
                                        }
                                        if small(ui, Icon::New, "New chat in this project") {
                                            start_in = Some(path.clone());
                                        }
                                        small(ui, Icon::More, "Project options") || secondary
                                    })
                                    .inner;
                                row_options(ui, options, "Project options", |ui| {
                                    use super::project_view::ProjectMenu as M;
                                    let mut pick = |m| project_menu = Some((path.clone(), m));
                                    if menu_item(ui, Icon::New, "New chat in project") {
                                        pick(M::NewChat);
                                    }
                                    if menu_item(ui, Icon::Folder, "Open project page") {
                                        pick(M::Open);
                                    }
                                    if menu_item(
                                        ui,
                                        Icon::Chevron,
                                        if collapsed {
                                            "Show chats"
                                        } else {
                                            "Hide chats"
                                        },
                                    ) {
                                        toggle = Some(path.clone());
                                    }
                                    ui.separator();
                                    if menu_item(ui, Icon::Rename, "Rename or edit goal…") {
                                        pick(M::Rename);
                                    }
                                    if menu_item(ui, Icon::Reader, "Instructions…") {
                                        pick(M::Instructions);
                                    }
                                    if menu_item(ui, Icon::Memory, "Memories…") {
                                        pick(M::Memories);
                                    }
                                    if menu_item(ui, Icon::Folder, "Show in folder") {
                                        pick(M::Folder);
                                    }
                                    if menu_item(ui, Icon::Copy, "Copy folder path") {
                                        ui.ctx().copy_text(projects::display_path(&path));
                                    }
                                    ui.separator();
                                    if menu_item(ui, Icon::Trash, "Delete project…") {
                                        pick(M::Delete);
                                    }
                                });
                                if project_menu.is_some() {
                                    close_row_options(ui);
                                }
                            });
                            continue;
                        }
                        Entry::Row(r) => r,
                    };
                    ui.push_id(id, |ui| {
                        let options = ui
                            .horizontal(|ui| {
                                if in_project.is_some() {
                                    ui.add_space(18.);
                                }
                                unread_dot(ui, unread);
                                let label = Text::new(&title).size(14.);
                                let label = if unread {
                                    label.strong().color(accent())
                                } else {
                                    label
                                };
                                let width =
                                    (ui.available_width() - 32. - ui.spacing().item_spacing.x)
                                        .max(30.);
                                let r = thread_button(
                                    ui,
                                    label,
                                    egui::vec2(width, 32.),
                                    id == state.chat_id && self.page == Page::Chat,
                                )
                                .on_hover_text(&title);
                                if r.clicked() {
                                    let _ = self.tx.send(format!("/open {id}"));
                                    self.page = Page::Chat;
                                }
                                let dots = ui
                                    .add(
                                        egui::Button::new("")
                                            .frame(false)
                                            .min_size(egui::vec2(32., 30.)),
                                    )
                                    .on_hover_text(if working.contains(&id) {
                                        "Working · more options"
                                    } else if waiting.contains(&id) {
                                        "Waiting for your approval · more options"
                                    } else {
                                        "More options"
                                    });
                                if working.contains(&id) {
                                    ui.put(
                                        egui::Rect::from_center_size(
                                            dots.rect.center(),
                                            egui::vec2(10., 10.),
                                        ),
                                        egui::Spinner::new().size(9.).color(accent()),
                                    );
                                } else if waiting.contains(&id) {
                                    ui.painter().circle_filled(dots.rect.center(), 4., accent());
                                } else {
                                    draw(ui, Icon::More, dots.rect.shrink(6.), muted());
                                }
                                dots.clicked() || r.secondary_clicked()
                            })
                            .inner;
                        row_options(ui, options, "Conversation options", |ui| {
                            if menu_item(ui, Icon::Rename, "Rename…") {
                                edit = Some(id);
                            }
                            if menu_item(ui, Icon::Settings, "Conversation settings") {
                                edit = Some(id);
                            }
                            if menu_item(ui, Icon::Star, if pinned { "Unpin" } else { "Pin" }) {
                                action(&self.tx, id, Action::Pin(!pinned));
                            }
                            if menu_item(
                                ui,
                                Icon::Chat,
                                if unread {
                                    "Mark as read"
                                } else {
                                    "Mark as unread"
                                },
                            ) {
                                action(&self.tx, id, Action::Unread(!unread));
                            }
                            egui::CollapsingHeader::new("Move to project").show(ui, |ui| {
                                for (path, name) in &library {
                                    if in_project.as_ref() == Some(path) {
                                        continue;
                                    }
                                    if ui.button(name).clicked() {
                                        action(
                                            &self.tx,
                                            id,
                                            Action::SetProject(Some(path.clone())),
                                        );
                                    }
                                }
                                if library.is_empty() {
                                    ui.weak("No projects yet");
                                }
                                if in_project.is_some() {
                                    ui.separator();
                                    if ui.button("Remove from project").clicked() {
                                        action(&self.tx, id, Action::SetProject(None));
                                    }
                                }
                            });
                            ui.separator();
                            if menu_item(ui, Icon::Swarm, "Branch conversation") {
                                action(&self.tx, id, Action::Branch);
                                self.page = Page::Chat;
                            }
                            if menu_item(ui, Icon::Reader, "Read conversation") {
                                self.threads.reader = Some(id);
                                action(&self.tx, id, Action::Unread(false));
                            }
                            if menu_item(ui, Icon::Queue, "Copy conversation") {
                                if let Some((_, h)) = state.thread_data(id) {
                                    ui.ctx().copy_text(transcript(&title, h));
                                }
                            }
                            if menu_item(
                                ui,
                                Icon::Archive,
                                if self.threads.archived {
                                    "Restore"
                                } else {
                                    "Archive"
                                },
                            ) {
                                action(&self.tx, id, Action::Archive(!self.threads.archived));
                            }
                            if menu_item(ui, Icon::Trash, "Delete…") {
                                edit = Some(id);
                                delete_dialog = Some(id);
                            }
                        });
                        if edit == Some(id) || self.threads.reader == Some(id) {
                            close_row_options(ui);
                        }
                    });
                }
            });
        if toggle_projects {
            self.threads.projects_hidden = !self.threads.projects_hidden;
        }
        if manage {
            self.projects.start_create();
        }
        if new_chat {
            self.send("/new");
            self.page = Page::Chat;
        }
        if let Some((path, choice)) = project_menu {
            self.project_menu(&path, choice);
        }
        if let Some(path) = open_home {
            self.threads.collapsed.remove(&path);
            self.open_project(&path);
        }
        if let Some(path) = toggle {
            if !self.threads.collapsed.remove(&path) {
                self.threads.collapsed.insert(path);
            }
        }
        if let Some(path) = start_in {
            self.new_chat_in(&path);
        }
        if let Some(id) = edit {
            self.edit_thread(id);
        }
        if let Some(id) = delete_dialog {
            if let Some(e) = self.threads.editors.get_mut(&id) {
                e.confirm_delete = true;
            }
        }
    }

    pub(super) fn thread_windows(&mut self, ui: &mut egui::Ui) {
        let state = &self.view.state;
        let ime = &mut self.android_ime;
        let active = self.threads.editors.keys().next_back().copied();
        self.threads.editors.retain(|&id, editor| {
            if Some(id) != active {
                return true;
            }
            let Some((info, history)) = state.thread_data(id) else {
                return false;
            };
            let label = info.label(history);
            let model = if id == state.chat_id {
                &state.model
            } else {
                &info.model
            };
            editor.refresh_fields(label, &info.section, model);
            editor.refresh_project(info.project.as_deref());
            if editor.confirm_delete {
                let mut open = true;
                super::detail_panel(ui, "Delete conversation?", "Back", &mut open, |ui| {
                    ui.push_id(("delete-conversation", id), |ui| {
                        editor.delete_confirmation(ui, id, label, state.thread_work(id), &self.tx);
                    });
                });
                if !open {
                    editor.confirm_delete = false;
                }
                return true;
            }
            let (mut save, mut cancel) = (false, false);
            let mut open = true;
            super::detail_panel(ui, "Conversation settings", "Back", &mut open, |ui| {
                    let place = info
                        .project
                        .as_deref()
                        .and_then(|p| editor.projects.iter().find(|q| q.path == p))
                        .map(|p| format!(" · in {}", p.name))
                        .unwrap_or_default();
                    ui.weak(format!("{} messages{place}", history.len()));
                    ui.add_space(10.);
                    egui::ScrollArea::vertical()
                        .max_height((ui.available_height() - 78.).max(90.))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let heading = |ui: &mut egui::Ui, text: &str| {
                                ui.label(Text::new(text).strong().size(14.));
                            };
                            heading(ui, "Name");
                            let field = ime.field(
                                ui, "thread-name", &mut editor.title,
                                super::android_ime::Field { limit: 100, ..Default::default() },
                                |field| field
                                    .background_color(raised())
                                    .margin(egui::Margin::symmetric(10, 8))
                                    .desired_width(f32::INFINITY),
                            );
                            if std::mem::take(&mut editor.focus_name) {
                                field.request_focus();
                            }
                            ui.add_space(8.);
                            let field_width = (ui.available_width() - 86.).clamp(90., 280.);
                            egui::Grid::new(("thread-settings-grid", id))
                                .num_columns(2)
                                .spacing([14., 10.])
                                .show(ui, |ui| {
                                    heading(ui, "Model");
                                    let selected = if editor.model.is_empty() {
                                        "Select model".to_owned()
                                    } else {
                                        route(&editor.model).1.to_owned()
                                    };
                                    crate::widgets::InlineChoice::from_id_salt(("thread-model", id))
                                        .selected_text(selected)
                                        .width(field_width)
                                        .show_ui(ui, |ui| {
                                            for model in &self.view.models {
                                                if !state.hidden_models.contains(model)
                                                    || *model == editor.model
                                                {
                                                    ui.selectable_value(
                                                        &mut editor.model,
                                                        model.clone(),
                                                        model,
                                                    );
                                                }
                                            }
                                        })
                                        .response
                                        .on_hover_text("Model for the next message in this conversation");
                                    ui.end_row();
                                    heading(ui, "Project");
                                    let shown = editor
                                        .project
                                        .as_deref()
                                        .map(|p| {
                                            editor
                                                .projects
                                                .iter()
                                                .find(|q| q.path == p)
                                                .map(|q| q.name.clone())
                                                .unwrap_or_else(|| {
                                                    p.file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy()
                                                        .into_owned()
                                                })
                                        })
                                        .unwrap_or_else(|| "No project".into());
                                    crate::widgets::InlineChoice::from_id_salt(("thread-project", id))
                                        .selected_text(shown)
                                        .width(field_width)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut editor.project, None, "No project");
                                            for p in &editor.projects {
                                                ui.selectable_value(
                                                    &mut editor.project,
                                                    Some(p.path.clone()),
                                                    &p.name,
                                                )
                                                .on_hover_text(projects::display_path(&p.path));
                                            }
                                        })
                                        .response
                                        .on_hover_text("New files from this conversation go to the project's artifacts folder");
                                    ui.end_row();
                                    heading(ui, "Section");
                                    ime.field(
                                        ui, "thread-section", &mut editor.section,
                                        super::android_ime::Field { limit: 48, ..Default::default() },
                                        |field| field
                                            .hint_text("Groups this chat in the sidebar")
                                            .background_color(raised())
                                            .desired_width(field_width),
                                    );
                                    ui.end_row();
                                });
                            ui.add_space(12.);
                            heading(ui, "Context compaction");
                            let default = state.default_compaction(id);
                            let mut inherit = info.compaction.is_none();
                            if ui
                                .checkbox(&mut inherit, format!("Use the default ({})", default.label()))
                                .changed()
                            {
                                action(&self.tx, id, Action::Compaction((!inherit).then_some(default)));
                            }
                            if !inherit {
                                let mut policy = info.compaction.unwrap_or(default);
                                if settings::compaction_controls(ui, &mut policy) {
                                    action(&self.tx, id, Action::Compaction(Some(policy)));
                                }
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .small_button("Compact now")
                                    .on_hover_text("Summarize older messages before the next request")
                                    .clicked()
                                {
                                    action(&self.tx, id, Action::CompactNow);
                                }
                                if let Some(summary) = &info.summary {
                                    ui.weak(format!("{} exchanges summarized", summary.covered / 2));
                                }
                            });
                            if let Some(summary) = &info.summary {
                                egui::CollapsingHeader::new(Text::new("View summary").size(13.).color(muted()))
                                    .id_salt(("thread-summary", id))
                                    .show(ui, |ui| {
                                        ui.label(Text::new(&summary.text).size(13.));
                                    });
                            }
                            if !editor.feedback.is_empty() {
                                ui.add_space(6.);
                                ui.weak(&editor.feedback);
                            }
                        });
                    ui.add_space(16.);
                    ui.horizontal(|ui| {
                        if ui
                            .link(Text::new("Delete…").size(13.).color(Color::LIGHT_RED))
                            .clicked()
                        {
                            editor.confirm_delete = true;
                        }
                        #[cfg(windows)]
                        if ui
                            .link(Text::new("Export Markdown…").size(13.).color(muted()))
                            .on_hover_text("Save a plaintext copy for sharing")
                            .clicked()
                        {
                            match export_transcript(id, label, history) {
                                Ok(Some(path)) => {
                                    editor.feedback =
                                        format!("Exported to {}", projects::display_path(&path))
                                }
                                Err(err) => editor.feedback = err.to_string(),
                                Ok(None) => {}
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let changed = editor.title != editor.saved_title
                                || editor.section != editor.saved_section
                                || editor.model != editor.saved_model
                                || editor.project != editor.saved_project;
                            let ready = changed && !editor.title.trim().is_empty();
                            save = ui
                                .add_enabled(
                                    ready,
                                    egui::Button::new(Text::new("Save").strong().color(
                                        if ready { Color::from_gray(20) } else { muted() },
                                    ))
                                    .fill(if ready { accent() } else { raised() })
                                    .corner_radius(8)
                                    .min_size(egui::vec2(84., 32.)),
                                )
                                .clicked();
                            cancel |= ui
                                .add(
                                    egui::Button::new("Cancel")
                                        .corner_radius(8)
                                        .min_size(egui::vec2(74., 32.)),
                                )
                                .clicked();
                        });
                    });
                });
            if save {
                let (title, section, model) = (
                    editor.title != editor.saved_title,
                    editor.section != editor.saved_section,
                    editor.model != editor.saved_model,
                );
                if title || section || model {
                    action(
                        &self.tx,
                        id,
                        Action::Edit {
                            title: title.then(|| editor.title.clone()),
                            section: section.then(|| editor.section.clone()),
                            model: model.then(|| editor.model.clone()),
                        },
                    );
                }
                if editor.project != editor.saved_project {
                    action(&self.tx, id, Action::SetProject(editor.project.clone()));
                }
            }
            open && !save && !cancel
        });
        if active.is_none()
            && let Some(id) = self.threads.reader
        {
            if let Some((thread, history)) = state.thread_data(id) {
                let mut open = true;
                super::detail_panel(ui, thread.label(history), "Close", &mut open, |ui| {
                    for (i, message) in history.iter().enumerate() {
                        message_row(ui, message.role == "user", |ui| {
                            ui.strong(if message.role == "user" {
                                "You"
                            } else {
                                thread
                                    .turns
                                    .get(&i)
                                    .map(|t| route(&t.model).1)
                                    .unwrap_or("Assistant")
                            });
                            ui.push_id(i, |ui| reader::markdown(ui, &message.content, false));
                            if small(ui, Icon::Copy, "Copy message text") {
                                ui.ctx().copy_text(message.content.clone());
                            }
                        });
                    }
                });
                if !open {
                    self.threads.reader = None;
                }
            } else {
                self.threads.reader = None;
            }
        }
    }

    pub(super) fn conversation_import_panel(&mut self, ui: &mut egui::Ui) {
        let mut open = self.threads.importing;
        #[cfg(not(target_os = "android"))]
        let mut command = None;
        #[cfg(not(target_os = "android"))]
        let mut selected: Option<std::path::PathBuf> = None;
        super::detail_panel(ui, "Import conversations", "Back", &mut open, |ui| {
            #[cfg(not(target_os = "android"))]
            {
                ui.weak("Select local chats to copy into Spindle.");
                command = self.threads.local_import.show(ui, &self.view.state);
                ui.separator();
            }
            ui.weak("Import from a session file or a Claude export (.jsonl, .json, .zip).");
            ui.add_space(8.);
            #[cfg(windows)]
            if ui.button("Choose file…").clicked() {
                selected = rfd::FileDialog::new()
                    .add_filter("Conversation files", &["json", "jsonl", "zip"])
                    .pick_file();
            }
            #[cfg(target_os = "android")]
            if ui.button("Choose file…").clicked() {
                if let Err(error) = crate::android::choose_conversation_file() {
                    self.notice = format!("Could not open the file picker: {error}");
                }
            }
            #[cfg(not(any(windows, target_os = "android")))]
            {
                ui.label("File path");
                ui.text_edit_singleline(&mut self.threads.import_path);
                let path = self.threads.import_path.trim();
                if ui.add_enabled(!path.is_empty(), egui::Button::new("Import")).clicked() {
                    selected = Some(std::path::PathBuf::from(path));
                }
            }
            if !self.notice.is_empty() {
                ui.weak(&self.notice);
            }
        });
        self.threads.importing = open;
        if !open && std::mem::take(&mut self.threads.asked_to_import) {
            self.send("/cancel-import");
        }
        #[cfg(not(target_os = "android"))]
        if let Some(command) = command {
            self.notice = "Importing selected conversations…".into();
            self.send(command);
        } else if let Some(path) = selected {
            self.send(format!("/import-chat {}", serde_json::json!(path)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2};

    // Drive the same sidebar and settings windows as the desktop, without creating a native window.
    fn frame(app: &mut Desktop, ctx: &egui::Context, events: Vec<Event>) -> BTreeMap<String, Pos2> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(1280., 900.),
            )),
            events,
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            app.sidebar(ctx);
            egui::CentralPanel::default().show(ctx, |ui| app.thread_windows(ui));
        });
        fn collect(shape: &egui::Shape, labels: &mut BTreeMap<String, Pos2>) {
            match shape {
                egui::Shape::Text(t) => {
                    labels.insert(t.galley.text().into(), t.pos + t.galley.size() * 0.5);
                }
                egui::Shape::Vec(children) => {
                    for child in children {
                        collect(child, labels);
                    }
                }
                _ => {}
            }
        }
        let mut labels = BTreeMap::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut labels);
        }
        labels
    }

    fn click(
        app: &mut Desktop,
        ctx: &egui::Context,
        pos: Pos2,
        button: PointerButton,
    ) -> BTreeMap<String, Pos2> {
        for pressed in [true, false] {
            frame(
                app,
                ctx,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        frame(app, ctx, vec![])
    }

    #[test]
    fn pending_thread_opens_its_settings_and_confirms_deletion() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        ctx.style_mut(|style| style.animation_time = 0.);
        let id = app.view.state.chat_id;
        app.view.state.thread.started = true;
        app.view.state.model = "test model".into();

        frame(&mut app, &ctx, vec![]);
        let labels = frame(&mut app, &ctx, vec![]);
        let menu = click(&mut app, &ctx, labels["New chat"], PointerButton::Secondary);
        let window = click(
            &mut app,
            &ctx,
            menu["Conversation settings"],
            PointerButton::Primary,
        );
        for control in [
            "Name",
            "Model",
            "+  No project",
            "Section",
            "Context compaction",
            "Save",
            "Delete…",
        ] {
            assert!(
                window.contains_key(control),
                "Missing {control} among {:?}",
                window.keys()
            );
        }

        // Menu actions such as Archive live in the thread menu, not in settings.
        assert!(!window.contains_key("Archive"));
        app.threads.editors.get_mut(&id).unwrap().title = "My own name".into();
        app.view.state.thread.title = "Model title".into();
        let window = frame(&mut app, &ctx, vec![]);
        assert_eq!(app.threads.editors[&id].title, "My own name");
        let confirm = click(&mut app, &ctx, window["Delete…"], PointerButton::Primary);
        let sent = commands.try_recv();
        assert!(sent.is_err(), "Opening confirmation sent {sent:?}");
        assert!(
            app.threads.editors[&id].confirm_delete,
            "Delete did not open confirmation"
        );
        assert!(
            confirm.contains_key("Keep conversation"),
            "Visible controls: {:?}",
            confirm.keys()
        );
        click(
            &mut app,
            &ctx,
            confirm["Keep conversation"],
            PointerButton::Primary,
        );
        assert!(!app.threads.editors[&id].confirm_delete);
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn ctrl_scroll_zooms_and_the_level_is_saved() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let run = |app: &mut Desktop, events: Vec<Event>, modifiers| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1280., 900.),
                )),
                events,
                modifiers,
                ..Default::default()
            };
            let mut frame = eframe::Frame::_new_kittest();
            ctx.run(input, |ctx| eframe::App::update(app, ctx, &mut frame));
        };
        run(&mut app, vec![], Default::default());
        while commands.try_recv().is_ok() {}
        let wheel = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0., 3.),
            modifiers: ctrl,
        };
        run(
            &mut app,
            vec![Event::PointerMoved(Pos2::new(700., 400.)), wheel],
            ctrl,
        );
        // egui spreads a wheel notch over a few frames.
        for _ in 0..20 {
            run(&mut app, vec![], ctrl);
        }
        let zoomed = ctx.zoom_factor();
        assert!(zoomed > 1.01, "Ctrl+Scroll up zooms in, got {zoomed}");
        let early: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(
            early.iter().all(|c| !c.starts_with("/preferences")),
            "not saved mid-scroll: {early:?}"
        );
        app.zoom_changed = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        run(&mut app, vec![], Default::default());
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let prefs: crate::state::Preferences =
            serde_json::from_str(sent[0].strip_prefix("/preferences ").unwrap()).unwrap();
        assert_eq!(prefs.zoom, (zoomed * 100.).round() as u16);
        run(
            &mut app,
            vec![Event::Key {
                key: egui::Key::Num0,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: ctrl,
            }],
            ctrl,
        );
        run(&mut app, vec![], Default::default());
        assert!((ctx.zoom_factor() - 1.0).abs() < 0.001, "Ctrl+0 resets");
    }

    #[test]
    fn new_chat_is_the_plus_beside_chats() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        frame(&mut app, &ctx, vec![]);
        let labels = frame(&mut app, &ctx, vec![]);
        let chats = labels.get("Chats").copied().expect("a Chats heading");
        assert!(
            !labels.contains_key("New chat"),
            "no separate New chat item"
        );
        // The + sits at the right end of the Chats row.
        let plus = Pos2::new(app_sidebar_right(&ctx) - 22., chats.y);
        frame(&mut app, &ctx, vec![Event::PointerMoved(plus)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                &ctx,
                vec![Event::PointerButton {
                    pos: plus,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent, vec!["/new".to_owned()]);
    }

    fn app_sidebar_right(ctx: &egui::Context) -> f32 {
        egui::containers::panel::PanelState::load(ctx, egui::Id::new("workspace"))
            .map(|s| s.rect.right())
            .unwrap_or(220.)
    }

    #[test]
    fn sidebar_width_is_dragged_and_saved_once() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(
            commands.try_recv().is_err(),
            "an untouched sidebar saves nothing"
        );
        let at = |x| Pos2::new(x, 450.);
        let press = |x, pressed| Event::PointerButton {
            pos: at(x),
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, &ctx, vec![Event::PointerMoved(at(220.))]);
        frame(&mut app, &ctx, vec![press(220., true)]);
        for x in [240., 270., 300.] {
            frame(&mut app, &ctx, vec![Event::PointerMoved(at(x))]);
        }
        assert!(commands.try_recv().is_err(), "nothing is saved mid-drag");
        frame(&mut app, &ctx, vec![press(300., false)]);
        frame(&mut app, &ctx, vec![]);
        let sent: Vec<String> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        let prefs: crate::state::Preferences =
            serde_json::from_str(sent[0].strip_prefix("/preferences ").unwrap()).unwrap();
        let width = prefs.sidebar_width.unwrap();
        assert!((290..=310).contains(&width), "{width}");
    }

    #[test]
    fn project_rows_have_a_context_menu() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (_, rx) = std::sync::mpsc::channel();
        let (_, preview) = tokio::sync::watch::channel(String::new());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Blue);
        ctx.style_mut(|style| style.animation_time = 0.);
        app.projects.recent = vec![crate::projects::Project {
            name: "Plans".into(),
            path: dir.path().join("plans"),
            description: String::new(),
            instructions: String::new(),
        }];
        frame(&mut app, &ctx, vec![]);
        let labels = frame(&mut app, &ctx, vec![]);
        let menu = click(&mut app, &ctx, labels["Plans"], PointerButton::Secondary);
        for item in [
            "New chat in project",
            "Open project page",
            "Hide chats",
            "Rename or edit goal…",
            "Instructions…",
            "Memories…",
            "Copy folder path",
            "Delete project…",
        ] {
            assert!(
                menu.contains_key(item),
                "Missing {item} among {:?}",
                menu.keys()
            );
        }
        click(&mut app, &ctx, menu["Memories…"], PointerButton::Primary);
        assert!(app.page == Page::Settings && app.memory_tab);
        assert_eq!(app.memory.filter, Some(dir.path().join("plans")));
    }
}
