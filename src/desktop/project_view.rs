use super::*;
use crate::projects::{self, Project, Snapshot, display_path};
use anyhow::{Context, Result};
use std::{path::Path, sync::mpsc};

#[derive(Default)]
pub(super) struct ProjectScreen {
    current: Option<Project>,
    pub(super) recent: Vec<Project>,
    loaded: bool,
    /// The project whose page is open.
    pub(super) home: Option<Project>,
    pub(super) composers: std::collections::BTreeMap<PathBuf, String>,
    details: Option<(String, String)>,
    instructions: Option<String>,
    creating: Option<NewProject>,
    deleting: Option<DeleteProject>,
    shares: Vec<String>,
    pending: Option<mpsc::Receiver<Result<Event>>>,
    notice: String,
    create_at: Option<PathBuf>,
    name: String,
    preview: Option<Snapshot>,
    passphrase: String,
}

struct NewProject {
    name: String,
    goal: String,
    instructions: Option<String>,
    folder: Option<PathBuf>,
}

struct DeleteProject {
    project: Project,
    typed: String,
    everywhere: bool,
    files: bool,
}

impl DeleteProject {
    fn new(project: Project) -> Self {
        Self { project, typed: String::new(), everywhere: false, files: false }
    }
}

/// Default home for new projects: the projects folder chosen in Settings › Sync, else
/// Documents\Spindle Projects (on a phone, its Documents\Spindle), then \<name>.
fn default_folder(base: &str, name: &str) -> PathBuf {
    let base = if base.trim().is_empty() {
        PathBuf::from(crate::sync::live::suggested_projects_folder())
    } else {
        PathBuf::from(base.trim())
    };
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    base.join(clean.trim_end_matches(['.', ' ']))
}

impl ProjectScreen {
    pub(super) fn has_dialog(&self) -> bool {
        self.creating.is_some() || self.deleting.is_some()
    }

    pub(super) fn home_title(&self) -> &str {
        self.home.as_ref().map_or("Project", |p| p.name.as_str())
    }
    pub(super) fn start_create(&mut self) {
        self.creating = Some(NewProject {
            name: String::new(),
            goal: String::new(),
            instructions: None,
            folder: None,
        });
    }
}

enum Event {
    Loaded(Option<Project>, Vec<Project>, Vec<String>),
    Open(Project),
    Preview(Snapshot),
}

fn load(root: &Path) -> Result<Event> {
    let current = projects::existing(root)?;
    let recent = projects::library(current.as_ref())?;
    let shares = if current.is_some() {
        projects::list_snapshots(root)?
    } else {
        vec![]
    };
    Ok(Event::Loaded(current, recent, shares))
}

impl ProjectScreen {
    fn start(&mut self, work: impl FnOnce() -> Result<Event> + Send + 'static) {
        if self.pending.is_some() {
            return;
        }
        self.notice.clear();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
    }
    pub(super) fn refresh(&mut self, root: PathBuf) {
        self.start(move || load(&root));
    }
    fn open(&mut self, path: PathBuf) {
        self.start(move || {
            let p = projects::existing(&path)?.context(
                "This folder has no Spindle project. Use Create project to initialize it.",
            )?;
            projects::library(Some(&p))?;
            Ok(Event::Open(p))
        });
    }
}

impl Desktop {
    /// Apply finished project work. Runs every frame so the sidebar sees the library.
    pub(super) fn poll_projects(&mut self, ctx: &egui::Context) {
        let screen = &mut self.projects;
        if !screen.loaded && screen.pending.is_none() {
            screen.loaded = true;
            screen.refresh(self.root.clone());
        }
        let Some(result) = screen.pending.as_ref().map(|r| r.try_recv()) else {
            return;
        };
        let result = match result {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("Project operation stopped")),
        };
        screen.pending = None;
        let mut start_in = None;
        match result {
            Err(e) => screen.notice = format!("{e:#}"),
            Ok(Event::Loaded(p, recent, shares)) => {
                screen.current = p;
                screen.recent = recent;
                screen.shares = shares;
            }
            Ok(Event::Preview(s)) => {
                screen.preview = Some(s);
            }
            Ok(Event::Open(p)) => {
                screen.refresh(self.root.clone());
                if p.path == self.root {
                    screen.notice = format!("{} is this workspace's project", p.name);
                } else {
                    screen.notice = format!(
                        "{} is ready. Chats in it save files to its artifacts folder.",
                        p.name
                    );
                    start_in = Some(p.path);
                }
            }
        }
        if let Some(path) = start_in {
            self.new_chat_in(&path);
        }
    }

    pub(super) fn new_chat_in(&mut self, path: &Path) {
        self.send(format!(
            "/new-in {}",
            serde_json::to_string(path).expect("Serializable project path")
        ));
        self.page = Page::Chat;
    }

    pub(super) fn projects_page(&mut self, ui: &mut egui::Ui) {
        self.poll_projects(ui.ctx());
        if self.projects.preview.is_some() {
            let screen = &mut self.projects;
            if let Some(s) = screen.preview.as_mut() {
                let mut apply = false;
                let mut open = true;
                super::detail_panel(ui, "Earlier shared conversation", "Back", &mut open, |ui| {
                    ui.heading(&s.title);
                    egui::ScrollArea::vertical()
                        .max_height(290.)
                        .show(ui, |ui| {
                            for m in &s.messages {
                                ui.label(
                                    Text::new(if m.role == "user" { "You" } else { "Assistant" })
                                        .strong(),
                                );
                                ui.label(&m.content);
                                ui.separator();
                            }
                        });
                    apply = ui.button("Import into a thread").clicked();
                });
                if apply {
                    let _ = self.tx.send(format!(
                        "/project-import {}",
                        serde_json::to_string(s).unwrap()
                    ));
                    self.page = Page::Chat;
                    open = false;
                }
                if !open {
                    screen.preview = None;
                }
            }
            return;
        }
        let screen = &mut self.projects;
        ui.set_max_width(ui.available_width().min(810.));
        ui.label(Text::new("Projects").size(31.));
        ui.horizontal(|ui| {
            if ui.button("Create project").clicked() {
                screen.start_create();
            }
            #[cfg(windows)]
            if ui.button("Open shared folder…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_directory(&self.root)
                    .set_title("Open a local or network project")
                    .pick_folder()
                {
                    screen.open(path);
                }
            }
            if ui.button("Refresh").clicked() {
                screen.refresh(self.root.clone());
            }
        });
        if let Some(path) = screen.create_at.clone() {
            ui.group(|ui| {
                ui.label(display_path(&path));
                ui.label("Project name");
                ui.text_edit_singleline(&mut screen.name);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !screen.name.trim().is_empty() && screen.pending.is_none(),
                            egui::Button::new("Create in this folder"),
                        )
                        .clicked()
                    {
                        let name = screen.name.clone();
                        screen.start(move || {
                            let p = projects::create(&path, &name)?;
                            projects::library(Some(&p))?;
                            Ok(Event::Open(p))
                        });
                        screen.create_at = None;
                    }
                    if ui.button("Cancel").clicked() {
                        screen.create_at = None;
                    }
                });
            });
        }
        if screen.pending.is_some() {
            ui.spinner();
        }
        if !screen.notice.is_empty() {
            ui.label(Text::new(&screen.notice).color(accent()));
        }
        ui.add_space(10.);
        ui.heading("Current workspace");
        egui::Frame::new().fill(panel()).inner_margin(16).corner_radius(10).show(ui,|ui|{
            ui.set_width(ui.available_width());
            if let Some(p)=screen.current.clone(){
                ui.horizontal(|ui|{symbol(ui,Icon::Folder,accent());ui.label(Text::new(&p.name).size(19.).strong());});
                ui.label(Text::new(display_path(&p.path)).color(muted()));
                ui.label("Project files use this folder’s Windows access permissions.");
                if ui.button("Copy folder path").clicked(){ui.ctx().copy_text(display_path(&p.path));}
                ui.separator();
                ui.heading("Encrypted thread sync");
                #[cfg(windows)]
                if ui.button("Export phone companion…").on_hover_text("Save the standalone mobile thread reader and draft editor").clicked() {
                    if let Some(path)=rfd::FileDialog::new().set_directory(&p.path).set_file_name("spindle-threads.html").save_file() {
                        screen.notice=match super::export_skill(&path,include_str!("../../assets/spindle-threads.html")) {
                            Ok(())=>format!("Phone companion saved to {}",display_path(&path)),
                            Err(e)=>format!("Could not export companion: {e}"),
                        };
                    }
                }
                if let Some(pair)=&self.view.state.sync {
                    ui.label(&self.view.state.sync_status);
                    if ui.button(if pair.enabled {"Pause sync on this device"} else {"Resume sync on this device"}).clicked(){
                        let _=self.tx.send("/sync-toggle".into());
                    }
                } else {
                    ui.label("Sync all threads in this project through its shared folder. Enter the same passphrase on each device. Keep a copy for recovery.");
                    ui.add(egui::TextEdit::singleline(&mut screen.passphrase).password(true).hint_text("Project passphrase · at least 12 characters"));
                    if ui.add_enabled(screen.passphrase.chars().count()>=12,egui::Button::new("Enable encrypted sync")).clicked(){
                        let pass=std::mem::take(&mut screen.passphrase);
                        let _=self.tx.send(format!("/sync-unlock {}",serde_json::to_string(&pass).unwrap()));
                    }
                    if self.view.notice.starts_with("Error:"){ui.label(Text::new(&self.view.notice).color(accent()));}
                }
            }else{
                ui.label(display_path(&self.root));
                if ui.button("Create project here").clicked(){screen.create_at=Some(self.root.clone());screen.name=self.root.file_name().unwrap_or_default().to_string_lossy().into();}
            }
        });
        if screen.current.is_some() && !screen.shares.is_empty() {
            ui.add_space(12.);
            ui.heading("Earlier plaintext shares");
            if screen.shares.is_empty() {
                ui.label(
                    Text::new("No conversations have been shared in this project.").color(muted()),
                );
            }
            for id in screen.shares.clone() {
                let title = id
                    .trim_start_matches("chat-")
                    .split_once('-')
                    .map(|(_, rest)| {
                        rest.split("-share-")
                            .next()
                            .unwrap_or(rest)
                            .replace('_', " ")
                    })
                    .unwrap_or(id.clone());
                if ui.button(title).clicked() {
                    let root = self.root.clone();
                    screen.start(move || Ok(Event::Preview(projects::snapshot(&root, &id)?)));
                }
            }
        }
        ui.add_space(12.);
        ui.heading("Project library");
        let mut new_chat = None;
        let mut project_home = None;
        for p in screen.recent.clone() {
            egui::Frame::new().fill(panel()).inner_margin(14).corner_radius(10).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    symbol(ui, Icon::Folder, accent());
                    ui.label(Text::new(&p.name).strong());
                });
                ui.weak(display_path(&p.path));
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Open project").clicked() { project_home = Some(p.path.clone()); }
                    if ui.button("New chat").clicked() { new_chat = Some(p.path.clone()); }
                });
            });
            ui.add_space(8.);
        }
        if screen.recent.is_empty() {
            ui.label(
                Text::new("Create or open a project to add it to your library.").color(muted()),
            );
        }
        if let Some(path) = new_chat {
            self.new_chat_in(&path);
        } else if let Some(path) = project_home {
            self.open_project(&path);
        }
    }
}

impl Desktop {
    pub(super) fn open_project(&mut self, path: &Path) {
        match projects::existing(path) {
            Ok(Some(p)) => {
                self.projects.home = Some(p);
                self.projects.details = None;
                self.projects.instructions = None;
                self.page = Page::Project;
            }
            Ok(None) => self.notice = "This folder is no longer a Spindle project".into(),
            Err(e) => self.notice = format!("{e:#}"),
        }
    }

    fn project_chats(&self, path: &Path) -> Vec<(u64, String)> {
        let state = &self.view.state;
        let mut chats: Vec<_> = std::iter::once(state.chat_id)
            .chain(state.chats.keys().copied())
            .filter_map(|id| {
                let (t, h) = state.thread_data(id)?;
                (t.project.as_deref() == Some(path) && !t.archived)
                    .then(|| (id, t.label(h).to_owned()))
            })
            .collect();
        chats.sort_by(|a, b| b.0.cmp(&a.0));
        chats
    }

    pub(super) fn project_home(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.projects.home.clone() else {
            self.page = Page::Projects;
            return;
        };
        let chats = self.project_chats(&project.path);
        let wide = ui.available_width() > 900.;
        let main_width = if wide {
            (ui.available_width() - 300.).min(720.)
        } else {
            ui.available_width().min(720.)
        };
        let mut send = None;
        let mut open = None;
        let mut bobbin_prompt = None;
        let sending = self.awaiting_submission();
        #[cfg(target_os = "android")]
        let ime = &mut self.android_ime;
        let mut body = |ui: &mut egui::Ui, screen: &mut ProjectScreen| {
            ui.set_width(main_width);
            ui.add_space(8.);
            if let Some((name, goal)) = &mut screen.details {
                ui.add(
                    egui::TextEdit::singleline(name)
                        .font(egui::FontId::proportional(24.))
                        .desired_width(f32::INFINITY),
                );
                ui.add(
                    egui::TextEdit::multiline(goal)
                        .hint_text("What are you trying to achieve?")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
            } else {
                ui.label(Text::new(&project.name).size(30.));
                if !project.description.is_empty() {
                    ui.label(Text::new(&project.description).color(muted()));
                }
            }
            ui.add_space(14.);
            egui::Frame::new()
                .fill(raised())
                .stroke(egui::Stroke::new(1_f32, line()))
                .corner_radius(12)
                .inner_margin(12)
                .show(ui, |ui| {
                    let draft = screen.composers.entry(project.path.clone()).or_default();
                    #[cfg(not(target_os = "android"))]
                    let r = ui.add(
                        egui::TextEdit::multiline(draft)
                            .hint_text(format!("New chat in {}", project.name))
                            .desired_rows(2)
                            .frame(false)
                            .desired_width(f32::INFINITY),
                    );
                    #[cfg(target_os = "android")]
                    let r = {
                        let id = ui.make_persistent_id(("project-composer", &project.path));
                        ime.edit(ui, id, id, draft, &format!("New chat in {}", project.name))
                    };
                    let enter = r.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    ui.add(egui::Label::new(Text::new(
                        "Files from this project's chats go to its artifacts folder",
                    ).weak()).wrap());
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Use a Bobbin").clicked() {
                            bobbin_prompt = Some(draft.clone());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ready = !sending && !draft.trim().is_empty();
                            if (ui.add_enabled(ready, egui::Button::new("Start chat")).clicked() || enter)
                                && ready
                            {
                                send = Some(std::mem::take(draft));
                            }
                        });
                    });
                });
            ui.add_space(18.);
            ui.label(Text::new("Chats").strong().color(muted()));
            if chats.is_empty() {
                ui.weak("No conversations yet. Start one above.");
            }
            for (id, title) in &chats {
                ui.horizontal(|ui| {
                    super::thread_view::unread_dot(ui, self.view.state.unread.contains(id));
                    let width = ui.available_width();
                    if super::thread_view::thread_button(
                        ui, Text::new(title).size(15.), egui::vec2(width, 32.), false,
                    ).clicked() {
                        open = Some(*id);
                    }
                });
            }
        };
        let mut action = None;
        let mut revoke = None;
        let mut side = |ui: &mut egui::Ui, screen: &mut ProjectScreen| {
            ui.set_width(if wide { 260. } else { main_width });
            ui.add_space(10.);
            let row = |ui: &mut egui::Ui, icon: Icon, label: &str, button: &str| {
                ui.horizontal_wrapped(|ui| {
                    symbol(ui, icon, muted());
                    ui.label(label);
                    ui.small_button(button).clicked()
                })
                .inner
            };
            let has = !project.instructions.is_empty();
            if row(ui, Icon::Chip, "Where work runs", "Configure") {
                action = Some("execution");
            }
            if row(
                ui,
                Icon::Reader,
                "Instructions",
                if has { "Edit" } else { "Add" },
            ) {
                screen.instructions = Some(project.instructions.clone());
            }
            if let Some(text) = &mut screen.instructions {
                ui.add(
                    egui::TextEdit::multiline(text)
                        .hint_text("How should Spindle respond in this project?")
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        action = Some("instructions");
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some("cancel-instructions");
                    }
                });
            } else if has {
                ui.label(
                    Text::new(project.instructions.chars().take(160).collect::<String>())
                        .color(muted())
                        .size(12.),
                );
            }
            ui.separator();
            if screen.details.is_some() {
                ui.horizontal(|ui| {
                    if ui.button("Save details").clicked() {
                        action = Some("details");
                    }
                    if ui.button("Cancel").clicked() {
                        screen.details = None;
                    }
                });
            } else if row(ui, Icon::Rename, "Name and goal", "Edit") {
                screen.details = Some((project.name.clone(), project.description.clone()));
            }
            if row(ui, Icon::Memory, "Memory", "View") {
                action = Some("memories");
            }
            if row(ui, Icon::Folder, "Files", "Open folder") {
                action = Some("files");
            }
            if row(ui, Icon::Copy, "Folder path", "Copy") {
                ui.ctx().copy_text(projects::display_path(&project.path));
            }
            if row(ui, Icon::Settings, "Sync and sharing", "Manage") {
                action = Some("manage");
            }
            if let Some(sync) = &self.view.state.space {
                let paused = sync.projects.get(&project.path).is_some_and(|p| p.paused);
                if row(ui, Icon::Cycle, "Sync across devices", if paused { "Resume sync" } else { "Stop sync" }) {
                    action = Some(if paused { "resume-project-sync" } else { "pause-project-sync" });
                }
            }
            ui.separator();
            if let Some(cmd) = notes::permissions_section(ui, &self.view.state, &project.path) {
                revoke = Some(cmd);
            }
            ui.separator();
            if self.git.show(ui, &project.path) {
                action = Some("review");
            }
            ui.add_space(24.);
            if ui
                .button(Text::new("Delete project…").color(Color::LIGHT_RED))
                .clicked()
            {
                screen.deleting = Some(DeleteProject::new(project.clone()));
            }
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| body(ui, &mut self.projects));
                ui.add_space(24.);
                ui.vertical(|ui| side(ui, &mut self.projects));
            });
        } else {
            body(ui, &mut self.projects);
            ui.separator();
            side(ui, &mut self.projects);
        }
        if let Some(prompt) = bobbin_prompt {
            self.bobbins.for_project(project.path.clone(), prompt);
            self.page = Page::Bobbins;
        }
        if let Some(cmd) = revoke {
            self.send(cmd);
        }
        let saved = match action {
            Some("execution") => {
                self.execution_panel = Some(crate::sync::placement::Scope::Project(project.path.clone()));
                None
            }
            Some("instructions") => {
                let text = self.projects.instructions.take().unwrap_or_default();
                Some(projects::update(
                    &project.path,
                    &project.name,
                    &project.description,
                    &text,
                ))
            }
            Some("cancel-instructions") => {
                self.projects.instructions = None;
                None
            }
            Some("details") => {
                let (name, goal) = self.projects.details.take().unwrap_or_default();
                Some(projects::update(
                    &project.path,
                    &name,
                    &goal,
                    &project.instructions,
                ))
            }
            Some("files") => {
                let dir = project.path.join("artifacts");
                let target = if dir.is_dir() {
                    dir
                } else {
                    project.path.clone()
                };
                if let Err(e) = super::reveal_folder(&target) {
                    self.notice = format!("Could not open the folder: {e}");
                }
                None
            }
            Some("manage") => {
                self.projects.refresh(self.root.clone());
                self.page = Page::Projects;
                None
            }
            Some("pause-project-sync" | "resume-project-sync") => {
                let paused = action == Some("pause-project-sync");
                self.send(format!("/sync project-pause {}", serde_json::json!([project.path, paused])));
                None
            }
            Some("memories") => {
                self.show_memories(Some(project.path.clone()));
                None
            }
            Some("review") => {
                // Review in a chat of this project, so the diff and files stay with it.
                if self.view.state.thread.project.as_deref() != Some(project.path.as_path()) {
                    self.new_chat_in(&project.path);
                }
                self.page = Page::Chat;
                self.send("/review");
                None
            }
            _ => None,
        };
        match saved {
            Some(Ok(p)) => {
                let _ = projects::library(Some(&p));
                self.projects.home = Some(p);
                self.projects.refresh(self.root.clone());
                self.notice = "Project saved".into();
            }
            Some(Err(e)) => self.notice = format!("{e:#}"),
            None => {}
        }
        if let Some(id) = open {
            let _ = self.tx.send(format!("/open {id}"));
            self.page = Page::Chat;
        }
        if let Some(text) = send {
            self.submit_project_prompt(project.path, text);
        }
    }

    /// The create-project and delete-project dialogs.
    pub(super) fn project_dialogs(&mut self, ui: &mut egui::Ui) {
        let base = self.view.state.preferences.projects_dir.clone();
        let mut created = None;
        if let Some(new) = &mut self.projects.creating {
            let ime = &mut self.android_ime;
            let (mut create, mut cancel) = (false, false);
            let mut field = |text: &mut String,
                             hint: &'static str,
                             rows: usize,
                             ui: &mut egui::Ui| {
                fn style<'a>(edit: egui::TextEdit<'a>, hint: &'static str) -> egui::TextEdit<'a> {
                    edit.hint_text(hint)
                        .background_color(raised())
                        .margin(egui::Margin::symmetric(10, 8))
                        .desired_width(f32::INFINITY)
                }
                if rows == 1 {
                    ime.field(ui, hint, text, Default::default(), |edit| style(edit, hint))
                } else {
                    ime.multiline(ui, hint, text, |edit| style(edit, hint).desired_rows(rows))
                }
            };
            let mut open = true;
            super::detail_panel(ui, "Create a project", "Back", &mut open, |ui| {
                let mut enter = false;
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 64.).max(90.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_space(12.);
                        ui.label(Text::new("What are you working on?").strong().size(14.));
                        let name = field(&mut new.name, "Name your project", 1, ui);
                        enter = name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.add_space(10.);
                        ui.label(
                            Text::new("What are you trying to achieve?")
                                .strong()
                                .size(14.),
                        );
                        field(
                            &mut new.goal,
                            "Describe your project, goals, subject, etc…",
                            4,
                            ui,
                        );
                        ui.add_space(10.);
                        match &mut new.instructions {
                            Some(text) => {
                                ui.horizontal(|ui| {
                                    ui.label(Text::new("Instructions").strong().size(14.));
                                    ui.weak("optional");
                                });
                                field(text, "How should Spindle respond in this project?", 3, ui);
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    ui.label(Text::new("Instructions").strong().size(14.));
                                    ui.weak("optional");
                                });
                                if ui.button("+ Add instructions").clicked() {
                                    new.instructions = Some(String::new());
                                }
                            }
                        }
                        ui.add_space(12.);
                        let folder = new
                            .folder
                            .clone()
                            .unwrap_or_else(|| default_folder(&base, &new.name));
                        ui.horizontal_wrapped(|ui| {
                            ui.label(Text::new("Saved in").size(12.).color(muted()));
                            let shown = folder
                                .parent()
                                .and_then(|p| p.file_name())
                                .map(|p| {
                                    format!(
                                        "{}\\{}",
                                        p.to_string_lossy(),
                                        folder.file_name().unwrap_or_default().to_string_lossy()
                                    )
                                })
                                .unwrap_or_else(|| projects::display_path(&folder));
                            ui.label(Text::new(shown).size(12.))
                                .on_hover_text(projects::display_path(&folder));
                            #[cfg(windows)]
                            if ui
                                .link(Text::new("Change").size(12.).color(accent()))
                                .clicked()
                            {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_title("Choose the project folder")
                                    .pick_folder()
                                {
                                    new.folder = Some(path);
                                }
                            }
                        });
                    });
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = !new.name.trim().is_empty();
                        let primary = egui::Button::new(
                            Text::new("Create project").strong().color(if ready {
                                Color::from_gray(20)
                            } else {
                                muted()
                            }),
                        )
                        .fill(if ready { accent() } else { raised() })
                        .corner_radius(8)
                        .min_size(egui::vec2(120., 34.));
                        create = (ui.add_enabled(ready, primary).clicked() || enter) && ready;
                        cancel |= ui
                            .add(
                                egui::Button::new("Cancel")
                                    .corner_radius(8)
                                    .min_size(egui::vec2(80., 34.)),
                            )
                            .clicked();
                    });
                });
            });
            cancel |= !open;
            if create {
                let folder = new
                    .folder
                    .clone()
                    .unwrap_or_else(|| default_folder(&base, &new.name));
                created = Some(
                    projects::create_described(&folder, &new.name, &new.goal).and_then(
                        |p| match new.instructions.as_deref().map(str::trim) {
                            Some(text) if !text.is_empty() => {
                                projects::update(&p.path, &p.name, &p.description, text)
                            }
                            _ => Ok(p),
                        },
                    ),
                );
            }
            if cancel || create {
                self.projects.creating = None;
            }
        }
        match created {
            Some(Ok(p)) => {
                let _ = projects::library(Some(&p));
                self.projects.refresh(self.root.clone());
                self.open_project(&p.path);
            }
            Some(Err(e)) => self.notice = format!("Could not create the project: {e:#}"),
            None => {}
        }
        let mut confirmed = None;
        let count = self
            .projects
            .deleting
            .as_ref()
            .map_or(0, |d| self.project_chats(&d.project.path).len());
        let paired = self.view.state.space.is_some();
        if let Some(DeleteProject { project, typed, everywhere, files }) = &mut self.projects.deleting {
            let ime = &mut self.android_ime;
            let mut open = true;
            let mut cancel = false;
            super::detail_panel(ui, "Delete project?", "Back", &mut open, |ui| {
                egui::Frame::new()
                        .fill(Color::from_rgb(70, 28, 28))
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.label(
                                Text::new(format!(
                                    "This permanently deletes “{}” and its {count} conversation{} from Spindle.",
                                    project.name,
                                    if count == 1 { "" } else { "s" }
                                ))
                                .strong()
                                .size(17.)
                                .color(Color::WHITE),
                            );
                            ui.label("Queued work and schedules in those conversations are removed too. This cannot be undone.");
                        });
                ui.add_space(6.);
                if paired {
                    ui.checkbox(everywhere, "Delete project on all synced devices");
                    if *everywhere {
                        ui.label("Other devices apply the deletion when they next sync.");
                        ui.checkbox(files, "Also delete project files on all devices");
                    } else {
                        *files = false;
                        ui.weak("Synced conversations deleted here can also be removed from other devices.");
                    }
                }
                ui.label(if *files {
                    format!("Project files and generated artifacts will be permanently deleted from {}", projects::display_path(&project.path))
                } else {
                    format!("Project files stay in {}", projects::display_path(&project.path))
                });
                ui.add_space(6.);
                ui.label(format!("Type {} to confirm", project.name));
                ime.field(
                    ui,
                    "project-delete-confirmation",
                    typed,
                    Default::default(),
                    |field| field.desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                typed.trim() == project.name,
                                egui::Button::new(
                                    Text::new("Delete project").color(Color::LIGHT_RED),
                                ),
                            )
                            .clicked()
                        {
                            confirmed = Some((project.path.clone(), *everywhere, *files));
                        }
                        cancel = ui.button("Keep project").clicked();
                    });
                });
            });
            if !open || cancel || confirmed.is_some() {
                self.projects.deleting = None;
            }
        }
        if let Some((path, everywhere, files)) = confirmed {
            let command = if everywhere {
                format!("/sync project-delete {}", serde_json::json!([path, files]))
            } else {
                format!("/project-delete {}", serde_json::json!(path))
            };
            let _ = self.tx.send(command);
            self.projects.home = None;
            self.projects.loaded = false;
            self.page = Page::Chat;
        }
    }
}

/// Actions from a project's sidebar menu.
#[derive(Clone, Copy)]
pub(super) enum ProjectMenu {
    NewChat,
    Open,
    Rename,
    Instructions,
    Memories,
    Folder,
    Delete,
}

impl Desktop {
    pub(super) fn project_menu(&mut self, path: &Path, choice: ProjectMenu) {
        match choice {
            ProjectMenu::NewChat => self.new_chat_in(path),
            ProjectMenu::Open => self.open_project(path),
            ProjectMenu::Rename => {
                self.open_project(path);
                if let Some(p) = &self.projects.home {
                    self.projects.details = Some((p.name.clone(), p.description.clone()));
                }
            }
            ProjectMenu::Instructions => {
                self.open_project(path);
                if let Some(p) = &self.projects.home {
                    self.projects.instructions = Some(p.instructions.clone());
                }
            }
            ProjectMenu::Memories => self.show_memories(Some(path.to_owned())),
            ProjectMenu::Folder => {
                let artifacts = path.join("artifacts");
                let target = if artifacts.is_dir() {
                    artifacts
                } else {
                    path.to_path_buf()
                };
                if let Err(e) = super::reveal_folder(&target) {
                    self.notice = format!("Could not open the folder: {e}");
                }
            }
            ProjectMenu::Delete => match projects::existing(path) {
                Ok(Some(p)) => self.projects.deleting = Some(DeleteProject::new(p)),
                Ok(None) => self.notice = "This folder is no longer a Spindle project".into(),
                Err(e) => self.notice = format!("{e:#}"),
            },
        }
    }
}
