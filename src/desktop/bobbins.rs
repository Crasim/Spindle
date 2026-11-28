use super::*;
use crate::bobbins::{AssignmentDraft, Draft, Picture, Profile, home};
mod sigil;
use sigil::Activity;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

const PALETTE: [(Color, &str); 6] = [
    (Color::from_rgb(86, 186, 146), "Jade"),
    (Color::from_rgb(104, 151, 226), "Blue"),
    (Color::from_rgb(171, 135, 222), "Violet"),
    (Color::from_rgb(233, 171, 112), "Amber"),
    (Color::from_rgb(213, 123, 151), "Rose"),
    (Color::from_rgb(118, 173, 179), "Teal"),
];

#[derive(Clone, Copy, Default, PartialEq)]
enum Tab {
    #[default]
    Overview,
    Assignments,
    Runs,
    Settings,
    Appearance,
}

#[derive(Default)]
pub(super) struct Screen {
    selected: Option<String>,
    tab: Tab,
    search: String,
    filter: String,
    project_filter: Option<PathBuf>,
    projects: Option<Vec<crate::projects::Project>>,
    editor: Option<Editor>,
    message: String,
    project_message: Option<String>,
    sent_chat: Option<u64>,
    assignment: Option<(Option<String>, AssignmentDraft)>,
    schedule: Option<ScheduleForm>,
    steering: Option<(u64, String)>,
    deleting: Option<String>,
    art: Artwork,
}

#[derive(Default)]
struct Artwork {
    photos: BTreeMap<String, Option<egui::TextureHandle>>,
    shells: sigil::Textures,
}

#[derive(Clone, Copy, Default)]
struct Look<'a> {
    activity: Activity,
    model: Option<&'a str>,
    demo_rate: Option<f32>,
}

struct Editor {
    key: Option<String>,
    revision: Option<u64>,
    draft: Draft,
    appearance: bool,
    file: String,
    prompt: String,
    artwork: Vec<String>,
    job: Option<AssetJob>,
    feedback: String,
    saving: Option<std::time::Instant>,
    demo_rate: f32,
}

struct AssetJob {
    receive: Receiver<anyhow::Result<Vec<String>>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for AssetJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct ScheduleForm {
    key: String,
    when: String,
    interval: i64,
    skip_missed: bool,
}

fn send(app: &mut Desktop, command: Value) {
    app.send(format!("/bobbin {command}"));
}

fn label(app: &Desktop, model: &str) -> String {
    if model.is_empty() {
        return "Select a model".into();
    }
    let (provider, tag) = route(model);
    format!("{} / {tag}", app.provider_name(provider))
}

fn field(
    ime: &mut android_ime::ComposerIme,
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    rows: usize,
) {
    ui.label(label);
    ime.multiline(ui, ("bobbin", label), value, |edit| {
        edit.desired_width(f32::INFINITY)
            .desired_rows(rows)
            .char_limit(if rows == 1 { 240 } else { 6_000 })
    });
    ui.add_space(6.);
}

fn tile(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(raised())
        .stroke(egui::Stroke::new(1_f32, line()))
        .corner_radius(12.)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
    ui.add_space(8.);
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .take(3)
        .collect()
}

fn picture(
    ui: &mut egui::Ui,
    draft: &Draft,
    artwork: &mut Artwork,
    length: f32,
    look: Look<'_>,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(length, length), egui::Sense::hover());
    if !ui.is_rect_visible(rect) { return; }
    let paint = ui.painter_at(rect);
    let color = PALETTE.get(draft.color).unwrap_or(&PALETTE[0]).0;
    paint.rect_filled(rect, length * 0.24, color.gamma_multiply(0.23));
    let assets = &mut artwork.photos;
    if let Some(key) = &draft.avatar {
        if assets.len() >= 64 && !assets.contains_key(key) {
            assets.clear();
        }
        let image = assets.entry(key.clone()).or_insert_with(|| {
            let file = crate::bobbins::avatar_path(key).ok()?;
            let bytes = std::fs::read(file).ok()?;
            if bytes.len() > 512 * 1024 {
                return None;
            }
            let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
            if image.dimensions() != (256, 256) {
                return None;
            }
            Some(ui.ctx().load_texture(
                format!("Bobbin {key}"),
                egui::ColorImage::from_rgba_unmultiplied([256, 256], image.as_raw()),
                egui::TextureOptions::LINEAR,
            ))
        });
        if let Some(image) = image {
            paint.image(
                image.id(),
                rect.shrink(2.),
                egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                Color::WHITE,
            );
            return;
        }
    }
    let c = rect.center();
    let p = |x: f32, y: f32| c + egui::vec2(x, y) * length;
    let stroke = egui::Stroke::new(length * 0.05, color);
    match draft.picture {
        Picture::Sigil | Picture::Character => {
            sigil::preview(ui, rect, &mut artwork.shells, draft, color, look);
        }
        Picture::Initials => {
            paint.text(
                c,
                egui::Align2::CENTER_CENTER,
                initials(&draft.name),
                egui::FontId::proportional(length * 0.36),
                color,
            );
        }
        Picture::Spool => {
            for y in [-0.26, 0.26] {
                paint.line_segment([p(-0.25, y), p(0.25, y)], stroke);
            }
            for x in [-0.18, 0.18] {
                paint.line_segment([p(x, -0.26), p(x, 0.26)], stroke);
            }
            for y in [-0.10, 0., 0.10] {
                paint.line_segment([p(-0.18, y + 0.05), p(0.18, y - 0.05)], stroke);
            }
        }
        Picture::Shuttle => {
            paint.add(egui::Shape::closed_line(
                vec![p(-0.30, 0.), p(0., -0.19), p(0.30, 0.), p(0., 0.19)],
                stroke,
            ));
            paint.circle_filled(c, length * 0.076, color);
        }
        Picture::Loop => {
            paint.circle_stroke(p(-0.13, -0.08), length * 0.18, stroke);
            paint.circle_stroke(p(0.13, 0.08), length * 0.18, stroke);
        }
    }
}

fn run_activity(view: &View, chat: u64, run: &crate::bobbins::Run) -> Activity {
    use crate::bobbins::Status;
    if !run.status.active() {
        return match run.status {
            Status::Done => Activity::Complete,
            Status::Failed | Status::Interrupted => Activity::Failed,
            _ => Activity::Ready,
        };
    }
    if view.state.requests.iter().any(|request| request.chat == chat && !request.approved) {
        Activity::NeedsApproval
    } else if matches!(view.stop, Some(crate::engine::StopTarget::Tool { chat: owner, .. }) if owner == chat) {
        Activity::UsingTools
    } else if run.held_task.is_some() {
        Activity::Paused
    } else if run.status == Status::Running {
        Activity::Working
    } else {
        Activity::Queued
    }
}

fn profile_activity(view: &View, id: &str, profile: &Profile) -> Activity {
    view.state.bobbins.live(id).map(|(&chat, run)| run_activity(view, chat, run)).max()
        .unwrap_or_else(|| idle_activity(view, profile))
}

fn idle_activity(view: &View, profile: &Profile) -> Activity {
    if profile.paused || profile.archived { return Activity::Paused; }
    if !view.models.contains(&profile.draft.model) { return Activity::Unavailable; }
    Activity::Ready
}

fn choice<T: Copy + Eq>(ui: &mut egui::Ui, caption: &str, value: &mut T, options: &[(T, &'static str)]) {
    ui.vertical(|ui| {
        ui.label(caption);
        egui::ComboBox::from_id_salt(caption)
            .selected_text(sigil::name(options, *value))
            .show_ui(ui, |ui| {
                for &(option, name) in options {
                    ui.selectable_value(value, option, name);
                }
            });
    });
}

impl Desktop {
    pub(super) fn bobbins_page(&mut self, ui: &mut egui::Ui) {
        let mut screen = std::mem::take(&mut self.bobbins);
        screen.render(self, ui);
        self.bobbins = screen;
    }

    pub(super) fn bobbin_run_footer(&mut self, ctx: &egui::Context) {
        let chat = self.view.state.chat_id;
        let run = &self.view.state.bobbins.runs[&chat];
        let activity = run_activity(&self.view, chat, run);
        let (back, cancel) = egui::TopBottomPanel::bottom("bobbin-run")
            .frame(egui::Frame::new().fill(panel()).inner_margin(10))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let back = control(ui, Icon::Back, "Bobbin runs", egui::vec2(126., 30.), false)
                        .clicked();
                    picture(ui, &run.configuration, &mut self.bobbins.art, 30., Look {
                        activity,
                        model: run.actual_model.as_deref(),
                        ..Default::default()
                    });
                    ui.strong(&run.configuration.name);
                    ui.weak(if run.status.active() { activity.label() } else { run.status.label() });
                    (
                        back,
                        run.status.active() && ui.button("Cancel run").clicked(),
                    )
                })
                .inner
            })
            .inner;
        if back {
            self.bobbins.selected = self
                .view
                .state
                .bobbins
                .profiles
                .contains_key(&run.bobbin)
                .then(|| run.bobbin.clone());
            self.bobbins.tab = Tab::Runs;
            self.page = Page::Bobbins;
        }
        if cancel {
            send(self, json!({"action":"cancel","chat":chat}));
        }
    }
}

impl Screen {
    pub(super) fn for_project(&mut self, path: PathBuf, text: String) {
        self.selected = None;
        self.editor = None;
        self.project_filter = Some(path);
        self.project_message = Some(text);
        self.filter.clear();
        self.search.clear();
    }

    fn render(&mut self, app: &mut Desktop, ui: &mut egui::Ui) {
        ui.set_max_width(ui.available_width().min(980.));
        if self.projects.is_none() {
            self.refresh_projects(&app.root);
        }
        if let Some(mut editor) = self.editor.take() {
            if editor.render(self, app, ui) {
                self.editor = Some(editor);
            }
        } else if let Some((id, profile)) = self.selected.clone().and_then(|id| {
            app.view
                .state
                .bobbins
                .profiles
                .get(&id)
                .cloned()
                .map(|profile| (id, profile))
        }) {
            self.detail(app, ui, &id, &profile);
        } else {
            self.roster(app, ui);
        }
        if app.notice.starts_with("Error:") || app.notice.starts_with("Bobbin schedule paused:") {
            ui.add_space(6.);
            ui.colored_label(accent(), &app.notice);
        }
    }

    fn refresh_projects(&mut self, root: &std::path::Path) {
        let current = crate::projects::existing(root).ok().flatten();
        self.projects = Some(crate::projects::library(current.as_ref()).unwrap_or_default());
    }

    fn select(&mut self, id: &str, tab: Tab) {
        if self.selected.as_deref() != Some(id) {
            self.message.clear();
            self.sent_chat = None;
            self.assignment = None;
            self.schedule = None;
            self.steering = None;
            self.deleting = None;
        }
        self.selected = Some(id.into());
        self.tab = tab;
        if tab == Tab::Overview
            && let Some(message) = self.project_message.take()
        {
            self.message = message;
        }
    }

    fn roster(&mut self, app: &mut Desktop, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Bobbins");
            if control(ui, Icon::New, "New bobbin", egui::vec2(126., 33.), false).clicked() {
                let model = app.view.state.model.clone();
                self.editor = Some(Editor::new(
                    None,
                    None,
                    Draft {
                        model,
                        project: self.project_filter.clone(),
                        ..Draft::default()
                    },
                    false,
                ));
            }
        });
        ui.weak("Give a model a purpose and repeatable work.");
        ui.add_space(10.);
        app.android_ime
            .multiline(ui, "search-bobbins", &mut self.search, |edit| {
                edit.desired_rows(1)
                    .hint_text("Search Bobbins")
                    .desired_width(300.)
            });
        ui.horizontal_wrapped(|ui| {
            if self.filter.is_empty() {
                self.filter = "Active".into();
            }
            for item in ["Active", "Working", "Paused", "Archived"] {
                ui.selectable_value(&mut self.filter, item.into(), item);
            }
            project_picker(
                ui,
                "roster-project",
                &mut self.project_filter,
                self.projects.as_deref().unwrap_or(&[]),
                "All projects",
            );
        });
        ui.add_space(10.);
        let registry = &app.view.state.bobbins;
        let query = self.search.trim().to_lowercase();
        let mut activities = BTreeMap::<&str, (usize, Activity)>::new();
        for (&chat, run) in registry.active_runs() {
            let status = activities.entry(&run.bobbin).or_default();
            status.0 += 1;
            status.1 = status.1.max(run_activity(&app.view, chat, run));
        }
        let mut matches: Vec<_> = registry
            .profiles
            .iter()
            .filter_map(|(id, p)| {
                let (active, activity) = activities.get(id.as_str()).copied()
                    .unwrap_or_else(|| (0, idle_activity(&app.view, p)));
                let status = match self.filter.as_str() {
                    "Archived" => p.archived,
                    "Paused" => p.paused && !p.archived,
                    "Working" => !p.archived && active != 0,
                    _ => !p.archived,
                };
                let visible = status
                    && self
                        .project_filter
                        .as_ref()
                        .is_none_or(|path| p.draft.project.as_ref() == Some(path))
                    && (query.is_empty()
                        || format!("{} {}", p.draft.name, p.draft.purpose)
                            .to_lowercase()
                            .contains(&query));
                visible.then_some((id, p, active, activity))
            })
            .collect();
        matches.sort_by_cached_key(|(_, p, _, _)| (!p.pinned, p.draft.name.to_lowercase()));
        if matches.is_empty() {
            tile(ui, |ui| {
                ui.strong(if registry.profiles.is_empty() {
                    "Your first Bobbin"
                } else {
                    "No matching Bobbins"
                });
                if registry.profiles.is_empty() {
                    ui.label("Choose a model and start with one assignment.");
                }
            });
        }
        let mut command = None;
        for (id, profile, count, activity) in matches {
            ui.push_id(id, |ui| tile(ui, |ui| {
                ui.horizontal(|ui| {
                    picture(ui, &profile.draft, &mut self.art, 46., Look { activity, ..Default::default() });
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(Text::new(&profile.draft.name).size(17.).strong()).wrap());
                        ui.add(egui::Label::new(&profile.draft.purpose).wrap());
                    });
                });
                ui.weak(label(app, &profile.draft.model));
                ui.horizontal_wrapped(|ui| {
                    ui.weak(home());
                    ui.strong(if profile.archived { "Archived" } else { activity.label() });
                    if count > 1 { ui.weak(format!("{count} active")); }
                    if !profile.paused && let Some(next) = upcoming(&profile) {
                        ui.weak(format!("Next {}", crate::automation::local_time(next)));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button(if profile.archived { "View" } else { "Message" }).clicked() { self.select(id, Tab::Overview); }
                    if ui.button("View runs").clicked() { self.select(id, Tab::Runs); }
                    if !profile.archived && ui.button(if profile.paused { "Resume" } else { "Pause" }).clicked() {
                        command = Some(json!({"action": if profile.paused { "resume" } else { "pause" }, "id":id, "stop":false}));
                    }
                });
            }));
        }
        if let Some(command) = command {
            send(app, command);
        }
    }

    fn detail(&mut self, app: &mut Desktop, ui: &mut egui::Ui, id: &str, profile: &Profile) {
        if ui.button("Back to Bobbins").clicked() {
            self.selected = None;
            return;
        }
        ui.add_space(5.);
        let activity = profile_activity(&app.view, id, profile);
        ui.horizontal(|ui| {
            picture(ui, &profile.draft, &mut self.art, 52., Look { activity, ..Default::default() });
            ui.vertical(|ui| {
                ui.heading(&profile.draft.name);
                ui.label(&profile.draft.purpose);
            });
        });
        ui.add_space(10.);
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (Tab::Overview, "Overview"),
                (Tab::Assignments, "Assignments"),
                (Tab::Runs, "Runs"),
                (Tab::Settings, "Settings"),
                (Tab::Appearance, "Appearance"),
            ] {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
        match self.tab {
            Tab::Overview => self.overview(app, ui, id, profile),
            Tab::Assignments => self.assignments(app, ui, id, profile),
            Tab::Runs => self.runs(app, ui, id),
            Tab::Settings | Tab::Appearance => {
                self.editor = Some(Editor::new(
                    Some(id.into()),
                    Some(profile.revision),
                    profile.draft.clone(),
                    self.tab == Tab::Appearance,
                ));
                self.tab = Tab::Overview;
                ui.ctx().request_repaint();
            }
        }
    }

    fn overview(&mut self, app: &mut Desktop, ui: &mut egui::Ui, id: &str, p: &Profile) {
        tile(ui, |ui| {
            ui.strong("Home device");
            ui.label(home());
            ui.strong("Model");
            ui.label(label(app, &p.draft.model));
            access(ui, &p.draft);
            if p.draft.picture.uses_sigil() && p.draft.avatar.is_none() {
                egui::CollapsingHeader::new("Reading the sigil").show(ui, |ui| {
                    let activity = profile_activity(&app.view, id, p);
                    let length = ui.available_width().min(320.);
                    picture(ui, &p.draft, &mut self.art, length, Look { activity, ..Default::default() });
                    sigil::details(ui, &p.draft.sigil, &p.draft.model, activity, None);
                });
            }
        });
        if !p.archived {
            tile(ui, |ui| {
                if self.sent_chat.is_some_and(|prior| {
                    app.view
                        .state
                        .bobbins
                        .runs
                        .range((prior.saturating_add(1))..)
                        .any(|(_, r)| r.bobbin == id && r.assignment.is_none())
                }) {
                    self.message.clear();
                    self.sent_chat = None;
                    self.tab = Tab::Runs;
                    ui.ctx().request_repaint();
                }
                field(&mut app.android_ime, ui, "Message", &mut self.message, 3);
                let ready = !p.paused && app.view.models.contains(&p.draft.model);
                if !ready {
                    ui.weak(if p.paused {
                        "Resume this Bobbin to start new work."
                    } else {
                        "Connect the model or choose another in Settings."
                    });
                }
                if app.notice.starts_with("Error:") {
                    self.sent_chat = None;
                }
                if ui
                    .add_enabled(
                        ready && self.sent_chat.is_none() && !self.message.trim().is_empty(),
                        egui::Button::new("Run now"),
                    )
                    .clicked()
                {
                    self.sent_chat = Some(
                        app.view
                            .state
                            .bobbins
                            .runs
                            .keys()
                            .next_back()
                            .copied()
                            .unwrap_or(0),
                    );
                    send(app, json!({"action":"run","id":id,"prompt":self.message}));
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button(if p.pinned { "Unpin" } else { "Pin" }).clicked() { send(app, json!({"action":"pin","id":id,"pinned":!p.pinned})); }
            if p.draft.picture.uses_sigil() && p.draft.avatar.is_none() {
                crate::widgets::InlineChoice::from_label("duplicate Bobbin").selected_text("Duplicate").show_ui(ui, |ui| {
                    for (mode, label) in [("keep", "Keep exact appearance"), ("lineage", "Create a lineage variant"), ("new", "Start with a new sigil")] {
                        if ui.button(label).clicked() { send(app, json!({"action":"duplicate","id":id,"appearance":mode})); }
                    }
                });
            } else if ui.button("Duplicate").clicked() {
                send(app, json!({"action":"duplicate","id":id}));
            }
            if p.archived {
                if ui.button("Restore").clicked() { send(app, json!({"action":"restore","id":id})); }
                if ui.button("Delete Bobbin…").clicked() { self.deleting = Some(String::new()); }
            } else {
                if ui.button(if p.paused { "Resume" } else { "Pause after current runs" }).clicked() {
                    send(app, json!({"action":if p.paused { "resume" } else { "pause" },"id":id,"stop":false}));
                }
                if app.view.state.bobbins.live(id).next().is_some() && ui.button("Pause and stop runs").clicked() {
                    send(app, json!({"action":"pause","id":id,"stop":true}));
                }
                if ui.button("Archive").on_hover_text("Stop active work and disable schedules").clicked() { send(app, json!({"action":"archive","id":id})); }
            }
        });
        if let Some(name) = &mut self.deleting {
            let mut close = false;
            field(
                &mut app.android_ime,
                ui,
                "Type the Bobbin name to delete it. Run chats are kept.",
                name,
                1,
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(name == &p.draft.name, egui::Button::new("Delete Bobbin"))
                    .clicked()
                {
                    send(app, json!({"action":"delete","id":id,"name":name}));
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
            if close {
                self.deleting = None;
            }
        }
    }

    fn assignments(&mut self, app: &mut Desktop, ui: &mut egui::Ui, id: &str, profile: &Profile) {
        if !profile.archived && ui.button("New assignment").clicked() {
            self.assignment = Some((None, AssignmentDraft::default()));
            app.notice.clear();
        }
        if let Some((key, draft)) = &mut self.assignment {
            let mut close = false;
            tile(ui, |ui| {
                field(
                    &mut app.android_ime,
                    ui,
                    "Assignment title",
                    &mut draft.title,
                    1,
                );
                field(
                    &mut app.android_ime,
                    ui,
                    "Instructions",
                    &mut draft.prompt,
                    4,
                );
                field(
                    &mut app.android_ime,
                    ui,
                    "What should the run deliver?",
                    &mut draft.done,
                    2,
                );
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Save assignment").clicked() {
                        send(app, json!({"action":"assign","id":id,"assignment":key,"revision":profile.revision,"draft":draft}));
                    }
                    if ui.button("Back").clicked() { close = true; }
                });
            });
            if close || app.notice == "Assignment saved" {
                self.assignment = None;
            }
        }
        for (key, assignment) in &profile.assignments {
            ui.push_id(key, |ui| {
                tile(ui, |ui| {
                    ui.strong(&assignment.draft.title);
                    ui.label(&assignment.draft.done);
                    if let Some(schedule) = &assignment.schedule {
                        ui.weak(if schedule.enabled {
                            format!("Next {}", crate::automation::local_time(schedule.next))
                        } else {
                            "Schedule inactive".into()
                        });
                        if let Some(error) = &schedule.last_error {
                            ui.weak(error);
                        }
                    }
                    if !profile.archived {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(!profile.paused, egui::Button::new("Run now"))
                                .clicked()
                            {
                                send(
                                    app,
                                    json!({"action":"run","id":id,"assignment":key,"prompt":""}),
                                );
                            }
                            if ui.button("Edit").clicked() {
                                self.assignment =
                                    Some((Some(key.clone()), assignment.draft.clone()));
                                app.notice.clear();
                            }
                            if !cfg!(target_os = "android") && ui.button("Schedule…").clicked() {
                                app.notice.clear();
                                let saved = assignment.schedule.as_ref();
                                self.schedule = Some(ScheduleForm {
                                    key: key.clone(),
                                    when: crate::automation::local_time(
                                        saved
                                            .map_or(0, |s| s.next)
                                            .max(crate::automation::now() + 3600),
                                    ),
                                    interval: saved.map_or(0, |s| s.interval),
                                    skip_missed: saved.is_some_and(|s| s.skip_missed),
                                });
                            }
                            if assignment.schedule.as_ref().is_some_and(|s| s.enabled)
                                && ui.button("Pause schedule").clicked()
                            {
                                send(
                                    app,
                                    json!({"action":"pause_schedule","id":id,"assignment":key}),
                                );
                            }
                        });
                    }
                })
            });
        }
        if let Some(form) = &mut self.schedule {
            let mut close = false;
            tile(ui, |ui| {
                ui.strong("Schedule preview");
                ui.label(label(app, &profile.draft.model));
                ui.weak(home());
                access(ui, &profile.draft);
                field(
                    &mut app.android_ime,
                    ui,
                    "First run in local time (YYYY-MM-DD HH:MM)",
                    &mut form.when,
                    1,
                );
                ui.horizontal_wrapped(|ui| {
                    for (interval, title) in [
                        (0, "Once"),
                        (3600, "Every hour"),
                        (86400, "Every 24 hours"),
                        (604800, "Every 7 days"),
                    ] {
                        ui.selectable_value(&mut form.interval, interval, title);
                    }
                });
                ui.checkbox(&mut form.skip_missed, "Skip missed runs");
                let next = crate::automation::parse_local(&form.when)
                    .ok()
                    .filter(|t| *t > crate::automation::now());
                if let Some(next) = next {
                    ui.weak(format!("First run {}", crate::automation::local_time(next)));
                    if form.interval > 0 {
                        ui.weak(format!(
                            "Following run {}",
                            crate::automation::local_time(next + form.interval)
                        ));
                    }
                } else {
                    ui.weak("Choose a valid future date and time.");
                }
                ui.horizontal_wrapped(|ui| {
                    if let Some(next) = next && ui.add_enabled(!profile.paused, egui::Button::new("Enable schedule")).clicked() {
                        send(app, json!({"action":"schedule","id":id,"assignment":form.key,"revision":profile.revision,"next":next,"interval":form.interval,"skip_missed":form.skip_missed}));
                    }
                    if ui.button("Back").clicked() { close = true; }
                });
            });
            if close || app.notice.starts_with("Schedule enabled for ") {
                self.schedule = None;
            }
        }
    }

    fn runs(&mut self, app: &mut Desktop, ui: &mut egui::Ui, id: &str) {
        let mut listed = app
            .view
            .state
            .bobbins
            .runs
            .iter()
            .rev()
            .filter(|(_, run)| run.bobbin == id)
            .peekable();
        if listed.peek().is_none() {
            ui.weak("Runs and their outputs appear here.");
        }
        let mut queued = BTreeMap::new();
        for task in &app.view.state.queue {
            if let Some(chat) = task.origin {
                queued.entry(chat).or_insert(task);
            }
        }
        let mut command = None;
        let mut open = None;
        for (&chat, run) in listed {
            let task = queued.get(&chat);
            let held = task.and_then(|task| task.hold_reason.as_deref());
            let actual = run.actual_model.as_deref().unwrap_or(&run.configuration.model);
            let model = label(app, actual);
            let activity = run_activity(&app.view, chat, run);
            let unread = app.view.state.unread.contains(&chat);
            let can_open = app.view.state.thread_data(chat).is_some();
            let ime = &mut app.android_ime;
            ui.push_id(chat, |ui| {
                tile(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        picture(ui, &run.configuration, &mut self.art, 36., Look { activity, model: Some(actual), demo_rate: None });
                        ui.strong(&run.title);
                        ui.label(if run.held_task.is_some() {
                            "Paused"
                        } else if held.is_some() {
                            "Waiting"
                        } else {
                            run.status.label()
                        });
                        if unread {
                            ui.colored_label(accent(), "Unread");
                        }
                    });
                    ui.weak(format!(
                        "{} · {}",
                        crate::automation::local_time(run.triggered),
                        if run.assignment.is_some() {
                            "Assignment"
                        } else {
                            "Message"
                        }
                    ));
                    ui.weak(format!("{model} · {}", run.home));
                    if run.configuration.picture.uses_sigil() && run.configuration.avatar.is_none() {
                        egui::CollapsingHeader::new("Reading the sigil").show(ui, |ui| {
                            let rate = app.view.state.thread_data(chat).and_then(|(thread, _)| {
                                thread.turns.values().rev().filter(|turn| turn.model == actual).find_map(|turn| turn.tokens_per_second)
                            });
                            sigil::details(ui, &run.configuration.sigil, actual, activity, rate);
                        });
                    }
                    if run.missed > 0 {
                        ui.weak(format!("{} missed occurrences combined", run.missed));
                    }
                    if !run.outcome.is_empty() {
                        ui.label(&run.outcome);
                    }
                    if let Some(held) = held {
                        ui.weak(held);
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(can_open, egui::Button::new("Open run"))
                            .clicked()
                        {
                            open = Some(chat);
                        }
                        if run.status.active() {
                            if ui
                                .add_enabled(task.is_some(), egui::Button::new("Steer"))
                                .clicked()
                            {
                                self.steering = Some((chat, String::new()));
                            }
                            if ui.button("Cancel run").clicked() {
                                command = Some(json!({"action":"cancel","chat":chat}));
                            }
                        }
                    });
                    if let Some((target, correction)) = &mut self.steering
                        && *target == chat
                    {
                        field(ime, ui, "Correction for this run", correction, 2);
                        if ui.button("Send correction").clicked() {
                            command = Some(json!({"action":"steer","chat":chat,"text":correction}));
                        }
                    }
                })
            });
        }
        if let Some(command) = command {
            send(app, command);
        }
        if let Some(chat) = open {
            app.send(format!("/open {chat}"));
            app.page = Page::Chat;
            app.drawer = false;
        }
        if app.notice == "Bobbin correction queued" {
            self.steering = None;
        }
    }
}

fn upcoming(profile: &Profile) -> Option<i64> {
    profile
        .assignments
        .values()
        .filter_map(|assignment| assignment.enabled_schedule().map(|schedule| schedule.next))
        .min()
}

fn project_picker(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<PathBuf>,
    projects: &[crate::projects::Project],
    none: &str,
) {
    let current = value.as_ref().map(|path| {
        projects
            .iter()
            .find(|p| &p.path == path)
            .map_or_else(|| crate::projects::display_path(path), |p| p.name.clone())
    });
    crate::widgets::InlineChoice::from_label(id)
        .selected_text(current.as_deref().unwrap_or(none))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, none);
            for p in projects {
                ui.selectable_value(value, Some(p.path.clone()), &p.name);
            }
        });
}

fn access(ui: &mut egui::Ui, draft: &Draft) {
    ui.strong("Access");
    if draft.read_project
        && let Some(path) = &draft.project
    {
        ui.label(format!(
            "Read project · {}",
            crate::projects::display_path(path)
        ));
    }
    if draft.output_files {
        ui.label("Save files in the run chat");
    }
    if draft.web {
        ui.label("Search and read public pages");
    }
    if !draft.read_project && !draft.output_files && !draft.web {
        ui.label("Prompt and reply");
    }
}

impl Editor {
    fn new(key: Option<String>, revision: Option<u64>, draft: Draft, appearance: bool) -> Self {
        Self {
            key,
            revision,
            draft,
            appearance,
            file: String::new(),
            prompt: String::new(),
            artwork: vec![],
            job: None,
            feedback: String::new(),
            saving: None,
            demo_rate: 0.,
        }
    }

    fn render(&mut self, screen: &mut Screen, app: &mut Desktop, ui: &mut egui::Ui) -> bool {
        if self.saving.is_some() && app.notice == "Bobbin saved" {
            return false;
        }
        if self
            .saving
            .is_some_and(|t| t.elapsed() > Duration::from_secs(5))
            || app.notice.starts_with("Error:")
        {
            self.saving = None;
        }
        if panel_header(
            ui,
            if self.key.is_some() {
                "Bobbin settings"
            } else {
                "New bobbin"
            },
            "Back",
        ) {
            return false;
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.appearance, false, "Profile");
            ui.selectable_value(&mut self.appearance, true, "Appearance");
        });
        ui.add_space(9.);
        if self.appearance {
            self.appearance(screen, app, ui);
        } else {
            if self.key.is_none() && self.draft.name.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Research brief").clicked() {
                        self.draft.name = "Research brief".into(); self.draft.purpose = "Compare options and cite the sources".into();
                        self.draft.instructions = "Separate verified details from assumptions. List the sources and open questions.".into();
                        self.draft.picture = Picture::Loop; self.draft.color = 1;
                    }
                    if ui.button("Project digest").clicked() {
                        self.draft.name = "Project digest".into(); self.draft.purpose = "Summarize decisions, changes and open work".into();
                        self.draft.instructions = "Keep the digest brief. Include filenames and identify anything that still needs a decision.".into();
                        self.draft.picture = Picture::Spool; self.draft.color = 0;
                    }
                });
            }
            field(&mut app.android_ime, ui, "Name", &mut self.draft.name, 1);
            field(
                &mut app.android_ime,
                ui,
                "Purpose",
                &mut self.draft.purpose,
                1,
            );
            field(
                &mut app.android_ime,
                ui,
                "Standing instructions",
                &mut self.draft.instructions,
                4,
            );
            ui.strong("Home device");
            ui.label(home());
            ui.strong("Model");
            crate::widgets::InlineChoice::from_label("bobbin model")
                .selected_text(label(app, &self.draft.model))
                .show_ui(ui, |ui| {
                    for model in &app.view.models {
                        ui.selectable_value(
                            &mut self.draft.model,
                            model.clone(),
                            label(app, model),
                        );
                    }
                });
            ui.add_space(7.);
            ui.strong("Project and access");
            ui.horizontal_wrapped(|ui| {
                project_picker(
                    ui,
                    "bobbin project",
                    &mut self.draft.project,
                    screen.projects.as_deref().unwrap_or(&[]),
                    "No project",
                );
                if ui.button("Refresh projects").clicked() {
                    screen.refresh_projects(&app.root);
                }
            });
            if self.draft.project.is_none() {
                self.draft.read_project = false;
            }
            ui.add_enabled(
                self.draft.project.is_some(),
                egui::Checkbox::new(&mut self.draft.read_project, "Read files in this project"),
            );
            ui.checkbox(
                &mut self.draft.output_files,
                "Create output files in run chats",
            );
            ui.add_enabled(
                !app.view.state.preferences.block_web,
                egui::Checkbox::new(&mut self.draft.web, "Search and read public web pages"),
            );
            if self.key.is_some() {
                ui.weak(
                    "Changes to model, instructions or access pause existing schedules for review.",
                );
            }
        }
        ui.add_space(12.);
        let ready = !self.draft.name.trim().is_empty()
            && !self.draft.purpose.trim().is_empty()
            && !self.draft.model.is_empty();
        if ui
            .add_enabled(
                ready && self.saving.is_none(),
                egui::Button::new(if self.key.is_none() {
                    "Save draft"
                } else {
                    "Save changes"
                }),
            )
            .clicked()
        {
            app.notice = "Saving Bobbin…".into();
            send(
                app,
                json!({"action":"save","id":self.key,"revision":self.revision,"draft":self.draft}),
            );
            self.saving = Some(std::time::Instant::now());
        }
        true
    }

    fn appearance(&mut self, screen: &mut Screen, app: &mut Desktop, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (preset, text) in [
                (Picture::Initials, "Initials"),
                (Picture::Spool, "Spool"),
                (Picture::Shuttle, "Shuttle"),
                (Picture::Loop, "Loop"),
                (Picture::Sigil, "Sigil only"),
                (Picture::Character, "Character with display"),
            ] {
                if ui
                    .selectable_label(
                        self.draft.avatar.is_none() && self.draft.picture == preset,
                        text,
                    )
                    .clicked()
                {
                    self.draft.picture = preset;
                    self.draft.avatar = None;
                    if preset.uses_sigil() && self.draft.sigil.seed == 0 {
                        self.reroll(false);
                    }
                }
            }
        });
        let sigil_selected = self.draft.picture.uses_sigil() && self.draft.avatar.is_none();
        let length = if sigil_selected { (ui.available_width() - 49.).clamp(72., 320.) } else { 72. };
        ui.horizontal(|ui| {
            ui.push_id("large-preview", |ui| picture(ui, &self.draft, &mut screen.art, length, Look { demo_rate: Some(self.demo_rate), ..Default::default() }));
            ui.push_id("compact-preview", |ui| picture(ui, &self.draft, &mut screen.art, 32., Look::default()));
        });
        if sigil_selected {
            self.sigil_controls(ui);
        }
        ui.horizontal_wrapped(|ui| {
            for (index, (color, name)) in PALETTE.iter().enumerate() {
                if ui
                    .selectable_label(self.draft.color == index, Text::new(*name).color(*color))
                    .clicked()
                {
                    self.draft.color = index;
                }
            }
        });
        ui.separator();
        egui::CollapsingHeader::new("Artwork").default_open(!sigil_selected || self.job.is_some()).show(ui, |ui| {
            self.artwork(screen, app, ui);
        });
        if !self.feedback.is_empty() {
            ui.colored_label(accent(), &self.feedback);
        }
    }

    fn artwork(&mut self, screen: &mut Screen, app: &mut Desktop, ui: &mut egui::Ui) {
        field(
            &mut app.android_ime,
            ui,
            "Artwork file path (PNG or JPEG)",
            &mut self.file,
            1,
        );
        #[cfg(windows)]
        if ui.button("Choose artwork…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Avatar image", &["png", "jpg", "jpeg"])
                .pick_file()
        {
            self.file = path.to_string_lossy().into_owned();
        }
        if ui
            .add_enabled(
                !self.file.trim().is_empty(),
                egui::Button::new("Use artwork"),
            )
            .clicked()
        {
            match crate::bobbins::import_avatar(&PathBuf::from(self.file.trim())) {
                Ok(key) => {
                    self.draft.avatar = Some(key);
                    self.feedback.clear();
                }
                Err(err) => self.feedback = format!("Artwork: {err:#}"),
            }
        }
        ui.separator();
        field(
            &mut app.android_ime,
            ui,
            "Describe an avatar",
            &mut self.prompt,
            3,
        );
        let settings = crate::comfy::Settings::from(&app.view.state.preferences);
        if let Some(job) = &self.job {
            match job.receive.try_recv() {
                Ok(Ok(assets)) => {
                    self.artwork = assets;
                    self.job = None;
                    self.feedback.clear();
                }
                Ok(Err(e)) => {
                    self.job = None;
                    self.feedback = format!("Generation: {e:#}");
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.feedback = "Avatar generation stopped.".into();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.weak("Generating avatars…");
                    if ui.button("Stop generation").clicked() {
                        self.job = None;
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                }
            }
        } else if settings.ready() {
            if ui
                .add_enabled(
                    !self.prompt.trim().is_empty(),
                    egui::Button::new(format!("Generate with {}", settings.label())),
                )
                .clicked()
            {
                let prompt = self.prompt.trim().to_owned();
                let (send, receive) = std::sync::mpsc::channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let stop = cancel.clone();
                std::thread::spawn(move || {
                    let result = (|| -> anyhow::Result<Vec<String>> {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()?;
                        let request = crate::comfy::Request::configured(
                            &prompt,
                            &crate::comfy::Options {
                                aspect_ratio: Some("1:1".into()),
                                count: Some(2),
                                ..Default::default()
                            },
                            &settings,
                        )?;
                        let images = runtime.block_on(crate::comfy::run(
                            &settings,
                            &request,
                            None,
                            stop.clone(),
                        ))?;
                        anyhow::ensure!(!stop.load(Ordering::Relaxed), "Generation stopped.");
                        images
                            .images
                            .iter()
                            .map(|image| crate::bobbins::store_avatar(image))
                            .collect()
                    })();
                    let _ = send.send(result);
                });
                self.job = Some(AssetJob { receive, cancel });
                ui.ctx().request_repaint();
            }
            ui.weak("Your description is sent to the configured image server.");
        } else {
            ui.weak("Configure an image server in Settings to generate artwork.");
        }
        ui.horizontal_wrapped(|ui| {
            for (index, asset) in self.artwork.iter().enumerate() {
                ui.vertical(|ui| {
                    picture(
                        ui,
                        &Draft {
                            avatar: Some(asset.clone()),
                            ..self.draft.clone()
                        },
                        &mut screen.art,
                        72.,
                        Look::default(),
                    );
                    if ui
                        .selectable_label(
                            self.draft.avatar.as_ref() == Some(asset),
                            format!("Choose {}", index + 1),
                        )
                        .clicked()
                    {
                        self.draft.avatar = Some(asset.clone());
                    }
                });
            }
        });
    }

    fn reroll(&mut self, motion_only: bool) {
        match crate::bobbins::sigil::Sigil::new_seed() {
            Ok(seed) if motion_only => self.draft.sigil.motion_seed = seed,
            Ok(seed) => self.draft.sigil.seed = seed,
            Err(error) => self.feedback = format!("Could not choose a new seed: {error}"),
        }
    }

    fn sigil_controls(&mut self, ui: &mut egui::Ui) {
        use crate::bobbins::sigil::{Architecture, Direction, Kernel, Line, Outline, Rank, Role, Shell, Terms, Ticks};
        let config = &mut self.draft.sigil;
        ui.horizontal_wrapped(|ui| {
            choice(ui, "Work role", &mut config.role, Role::CHOICES);
            choice(ui, "Responsibility", &mut config.rank, Rank::CHOICES);
            if self.draft.picture == Picture::Character {
                choice(ui, "Shell", &mut config.shell, Shell::CHOICES);
            } else {
                choice(ui, "Border shape", &mut config.border, Outline::CHOICES);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let shapes = &Outline::CHOICES[..Outline::CHOICES.len() - 1];
            choice(ui, "Layer shape", &mut config.geometry, shapes);
            choice(ui, "Tick marks", &mut config.ticks, Ticks::CHOICES);
        });
        ui.checkbox(&mut config.motion, "Animate this avatar");
        if config.motion {
            ui.add(egui::Slider::new(&mut self.demo_rate, 0. ..=120.).text("Preview only · tok/s"));
        }
        egui::CollapsingHeader::new("Layer lines and directions").show(ui, |ui| {
            for i in 0..6 {
                ui.push_id(i, |ui| ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Layer {}", i + 1));
                    choice(ui, "Line", &mut config.lines[i], Line::CHOICES);
                    choice(ui, "Direction", &mut config.directions[i], Direction::CHOICES);
                }));
            }
        });
        egui::CollapsingHeader::new("Model display details").show(ui, |ui| {
            if self.draft.model.is_empty() {
                ui.weak("Choose a model in Profile to set its display details.");
                return;
            }
            let model = &self.draft.model;
            let mut mark = config.mark(model);
            ui.weak(format!("Overrides for {}", route(model).1));
            ui.horizontal_wrapped(|ui| {
                choice(ui, "Kernel", &mut mark.kernel, Kernel::CHOICES);
                choice(ui, "Architecture", &mut mark.architecture, Architecture::CHOICES);
                choice(ui, "Weights terms", &mut mark.terms, Terms::CHOICES);
            });
            ui.horizontal_wrapped(|ui| {
                let tiers = [(None, "Automatic"),(Some(0), "Tier 0"),(Some(1), "Tier 1"),(Some(2), "Tier 2"),(Some(3), "Tier 3"),(Some(4), "Tier 4"),(Some(5), "Tier 5"),(Some(6), "Tier 6")];
                choice(ui, "Display tier", &mut mark.tier, &tiers);
                ui.vertical(|ui| {
                    ui.label("Physical layers");
                    let mut layers = mark.physical_layers.unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut layers).range(0..=4096).speed(0.25).custom_formatter(|n, _| {
                        if n == 0. { "Unknown".into() } else { format!("{n:.0}") }
                    })).changed() { mark.physical_layers = (layers > 0).then_some(layers); }
                });
            });
            if mark == Default::default() {
                config.models.remove(model);
            } else if config.models.contains_key(model) || config.models.len() < 128 {
                config.models.insert(model.clone(), mark);
            }
        });
        egui::CollapsingHeader::new("Reading the sigil").show(ui, |ui| {
            sigil::details(ui, config, &self.draft.model, Activity::Ready, None);
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Reroll identity details").clicked() { self.reroll(false); }
            if ui.button("Reroll motion").clicked() { self.reroll(true); }
        });
    }
}
