use super::*;

const TEMPLATES: &[(&str, &str, Protocol)] = &[
    (
        "Claude · Anthropic API",
        "https://api.anthropic.com/v1",
        Protocol::Messages,
    ),
    #[cfg(any(windows, target_os = "linux"))]
    ("Claude · Console OAuth", "", Protocol::ClaudeConsole),
    ("Custom compatible", "", Protocol::ChatCompletions),
    (
        "LM Studio",
        "http://localhost:1234/v1",
        Protocol::ChatCompletions,
    ),
    (
        "Maya",
        "http://127.0.0.1:18081/v1",
        Protocol::ChatCompletions,
    ),
    // Ollama uses its own API; adding it sets the server address.
    ("Ollama", "", Protocol::ChatCompletions),
    (
        "OpenAI API",
        "https://api.openai.com/v1",
        Protocol::Responses,
    ),
    #[cfg(any(windows, target_os = "android"))]
    (
        "OpenAI · Sign in with ChatGPT",
        "https://api.openai.com/v1",
        Protocol::ChatGpt,
    ),
    (
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        Protocol::ChatCompletions,
    ),
];

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ProviderSetup {
    #[default]
    Connections,
    Choose,
    Configure(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlsPanel {
    Models,
    Thinking,
    PlanUsage,
    Budget,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IntegrationTab {
    Plugins,
    ComputerUse,
    Connectors,
}

#[derive(Default)]
pub(super) struct BrowserSetup {
    browser: crate::browser::LinkBrowser,
    install: Option<std::sync::mpsc::Receiver<anyhow::Result<std::path::PathBuf>>>,
    folder: Option<std::path::PathBuf>,
    existing_clients: Vec<String>,
    updating: bool,
    manage: Option<crate::browser::LinkBrowser>,
    status: String,
    url: String,
    page_address: String,
    page_request: Option<u64>,
}

#[cfg(any(windows, target_os = "linux"))]
#[derive(Default)]
pub(super) struct UpdatePanel {
    update: Option<crate::updates::Update>,
    job: Option<std::sync::mpsc::Receiver<anyhow::Result<UpdateResult>>>,
    status: String,
    confirm_restart: bool,
}

#[cfg(any(windows, target_os = "linux"))]
enum UpdateResult {
    Checked(Option<crate::updates::Update>),
    Downloaded,
}

#[cfg(any(windows, target_os = "linux"))]
impl UpdatePanel {
    fn run(&mut self, update: Option<crate::updates::Update>) {
        self.status = if update.is_some() { "Downloading and verifying…".into() } else { "Checking GitHub Releases…".into() };
        let (send, receive) = std::sync::mpsc::channel();
        self.job = Some(receive);
        std::thread::spawn(move || {
            let result = tokio::runtime::Runtime::new().map_err(anyhow::Error::from)
                .and_then(|runtime| runtime.block_on(async {
                    if let Some(update) = update {
                        crate::updates::download(&update).await?;
                        Ok(UpdateResult::Downloaded)
                    } else {
                        Ok(UpdateResult::Checked(crate::updates::check().await?))
                    }
                }));
            let _ = send.send(result);
        });
    }

    fn show(&mut self, ui: &mut egui::Ui, has_drafts: bool, submitting: bool) -> bool {
        let mut restart = false;
        if let Some(job) = &self.job {
            match job.try_recv() {
                Ok(result) => {
                    self.job = None;
                    match result {
                        Ok(UpdateResult::Checked(update)) => {
                            self.status = if update.is_some() { String::new() } else { "No newer compatible release on GitHub.".into() };
                            self.update = update;
                        }
                        Ok(UpdateResult::Downloaded) => {
                            self.status.clear();
                            self.update = None;
                        }
                        Err(error) => self.status = format!("Update check or download failed. {error:#}"),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(Duration::from_millis(150)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.status = "The update check stopped. Try again.".into();
                }
            }
        }
        ui.separator();
        ui.heading("Updates");
        ui.weak(format!("Spindle · {}", crate::version()));
        let staged = crate::updates::staged_version();
        let can_restart = self.job.is_none() && !submitting;
        self.confirm_restart &= staged.is_some() && has_drafts;
        if let Some(version) = &staged {
            ui.horizontal_wrapped(|ui| {
                if self.confirm_restart {
                    ui.label("Restarting will clear unsent drafts.");
                    if ui
                        .add_enabled(can_restart, egui::Button::new("Restart and discard drafts"))
                        .clicked()
                    {
                        restart = true;
                        self.confirm_restart = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_restart = false;
                    }
                } else {
                    ui.label(format!("Version {version} is ready to install"));
                    if ui
                        .add_enabled(can_restart, egui::Button::new("Restart to update"))
                        .on_disabled_hover_text("Wait for the current download, check, or message submission to finish.")
                        .clicked()
                    {
                        self.confirm_restart = has_drafts;
                        restart = !has_drafts;
                    }
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.job.is_none(), egui::Button::new("Check for updates")).clicked() {
                self.run(None);
            }
            if crate::updates::releases_configured() {
                ui.hyperlink_to("GitHub Releases", crate::updates::RELEASES);
            }
        });
        if let Some(update) = self.update.clone()
            && staged.as_deref() != Some(update.version.to_string().as_str())
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Version {} available", update.version));
                if ui.add_enabled(self.job.is_none(), egui::Button::new("Download update")).clicked() {
                    self.run(Some(update.clone()));
                }
                ui.hyperlink_to("Release notes", update.notes_url);
            });
        }
        if self.job.is_some() {
            ui.spinner();
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        restart
    }
}

#[derive(Default)]
pub(super) struct ConnectorSetup {
    service: crate::connectors::Service,
    credential: String,
    connecting: Option<std::sync::mpsc::Receiver<anyhow::Result<()>>>,
    status: String,
}

#[derive(Default)]
pub(super) struct LocalServers {
    host: String,
    search: Option<std::sync::mpsc::Receiver<anyhow::Result<Vec<crate::local_servers::Server>>>>,
    results: Vec<crate::local_servers::Server>,
    status: String,
}

impl Desktop {
    pub(super) fn provider_name(&self, id: &str) -> &str {
        if id == "ollama" {
            "Ollama"
        } else if id == "ollama-cloud" {
            "Ollama Cloud"
        } else {
            self.view
                .state
                .providers
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.as_str())
                .unwrap_or("Disconnected")
        }
    }

    pub(super) fn model_picker(&mut self, ui: &mut egui::Ui, compact: bool) {
        let current = self.view.state.model.clone();
        let (id, model) = route(&current);
        let full_label = format!("{}  /  {}", self.provider_name(id), model);
        let button = if compact {
            control(ui, Icon::Chip, "", egui::vec2(39., 33.), false)
        } else {
            let label = if model.is_empty() {
                "Select model".to_string()
            } else {
                format!(
                    "{}   /   {}",
                    self.provider_name(id).chars().take(12).collect::<String>(),
                    model.chars().take(24).collect::<String>()
                )
            };
            let width = ui.fonts(|f| {
                f.layout_no_wrap(label.clone(), egui::FontId::proportional(14.), ink())
                    .size()
                    .x
            }) + 58.;
            let (rect, button) =
                ui.allocate_exact_size(egui::vec2(width, 32.), egui::Sense::click());
            ui.painter()
                .rect_filled(rect, 7, if button.hovered() { line() } else { panel() });
            draw(
                ui,
                Icon::Chip,
                egui::Rect::from_center_size(
                    rect.left_center() + egui::vec2(14., 0.),
                    egui::vec2(16., 16.),
                ),
                accent(),
            );
            let mut label = egui::text::LayoutJob::simple(
                label,
                egui::FontId::proportional(14.),
                ink(),
                (width - 52.).max(1.),
            );
            label.wrap.max_rows = 1;
            label.wrap.break_anywhere = true;
            let label = ui.fonts(|fonts| fonts.layout_job(label));
            ui.painter().galley(
                rect.left_center() + egui::vec2(28., -label.size().y / 2.),
                label,
                ink(),
            );
            draw(
                ui,
                Icon::Chevron,
                egui::Rect::from_center_size(
                    rect.right_center() - egui::vec2(14., 0.),
                    egui::vec2(14., 14.),
                ),
                muted(),
            );
            button
        };
        button.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &full_label)
        });
        let button = button.on_hover_text(format!("{full_label}  ·  Ctrl+K"));
        if button.clicked() {
            self.show_models();
        }
    }

    pub(super) fn show_models(&mut self) {
        self.controls_panel = Some(ControlsPanel::Models);
        self.search.clear();
        self.select_focus = !cfg!(target_os = "android");
    }

    fn model_panel(&mut self, ui: &mut egui::Ui) {
        let current = self.view.state.model.clone();
        let (provider, model) = route(&current);
        let label = format!("{} / {model}", self.provider_name(provider));
        let mut open = true;
        super::detail_panel(ui, "Choose a model", "Back", &mut open, |ui| {
            let available = ui.available_width();
            if !model.is_empty() {
                ui.label(Text::new(label).size(13.).color(muted()));
            }
            ui.horizontal(|ui| {
                symbol(ui, Icon::Search, muted());
                let width = (ui.available_width() - 39.).clamp(40., 212.);
                let search = self.android_ime.field(
                    ui,
                    "model-search",
                    &mut self.search,
                    Default::default(),
                    |field| {
                        field
                            .desired_width(width)
                            .hint_text("Search model or provider")
                    },
                );
                if self.select_focus {
                    search.request_focus();
                    self.select_focus = false;
                }
                if control(
                    ui,
                    Icon::Star,
                    "",
                    egui::vec2(29., 27.),
                    self.favorites_only,
                )
                .on_hover_text("Show favorites only")
                .clicked()
                {
                    self.favorites_only = !self.favorites_only;
                }
            });
            ui.separator();
            let query = self.search.trim().to_lowercase();
            let mut matches = self
                .view
                .models
                .iter()
                .filter(|m| {
                    let (p, name) = route(m);
                    (name.to_lowercase().contains(&query)
                        || self.provider_name(p).to_lowercase().contains(&query))
                        && (!self.favorites_only || self.view.state.favorites.contains(m))
                        && (!self.view.state.hidden_models.contains(*m)
                            || **m == self.view.state.model)
                })
                .cloned()
                .collect::<Vec<_>>();
            matches.sort_by_cached_key(|m| {
                let (p, name) = route(m);
                (
                    self.provider_name(p).to_lowercase(),
                    p.to_owned(),
                    !self.view.state.favorites.contains(m),
                    name.to_lowercase(),
                )
            });
            let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let mut chosen = if enter {
                matches.first().cloned()
            } else {
                None
            };
            egui::ScrollArea::vertical()
                .id_salt("model-results")
                .max_height((ui.available_height() - 52.).max(100.))
                .show(ui, |ui| {
                    let mut last = String::new();
                    for key in &matches {
                        let (p, tag) = route(key);
                        if last != p {
                            ui.add_space(4.);
                            ui.label(
                                Text::new(self.provider_name(p))
                                    .size(12.)
                                    .strong()
                                    .color(accent()),
                            );
                            last = p.into();
                        }
                        ui.horizontal(|ui| {
                            if control(
                                ui,
                                Icon::Star,
                                "",
                                egui::vec2(25., 27.),
                                self.view.state.favorites.contains(key),
                            )
                            .on_hover_text("Toggle favorite")
                            .clicked()
                            {
                                self.send(format!("/favorite {key}"));
                            }
                            let short: String = tag.chars().take(33).collect();
                            let label = if short != tag {
                                format!("{short}…")
                            } else {
                                short
                            };
                            if ui
                                .add_sized(
                                    [(available - 105.).max(85.), 29.],
                                    egui::Button::new(label).frame(false).truncate(),
                                )
                                .on_hover_text(tag)
                                .clicked()
                            {
                                chosen = Some(key.clone());
                            }
                            self.capability_icons(ui, key, 14., true);
                            if key == &current {
                                symbol(ui, Icon::Check, accent());
                            }
                        });
                    }
                    if matches.is_empty() {
                        ui.label("No matching models.");
                    }
                });
            if let Some(m) = chosen {
                self.send(format!("/model {m}"));
                self.controls_panel = None;
                self.focus = !cfg!(target_os = "android");
            }
            ui.separator();
            if control(
                ui,
                Icon::Plug,
                "Manage providers",
                egui::vec2(240., 30.),
                false,
            )
            .clicked()
            {
                self.page = Page::Settings;
                self.preferences_tab = false;
                self.models_tab = false;
                self.memory_tab = false;
                self.sandbox_tab = false;
                self.sync_tab = false;
                self.integration_tab = None;
                self.controls_panel = None;
            }
        });
        if !open {
            self.controls_panel = None;
        }
    }

    pub(super) fn settings(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(ui.available_width().min(780.));
        ui.add_space(8.);
        if self.provider_setup != ProviderSetup::Connections {
            self.add_provider_panel(ui);
            return;
        }
        ui.label(Text::new("Settings").size(31.).color(ink()));
        // Wraps onto a second row on a narrow screen.
        ui.horizontal_wrapped(|ui| {
            if control(
                ui,
                Icon::Plug,
                "Model providers",
                egui::vec2(162., 35.),
                !self.preferences_tab
                    && !self.models_tab
                    && !self.memory_tab
                    && !self.sandbox_tab
                    && !self.sync_tab
                    && self.integration_tab.is_none(),
            )
            .clicked()
            {
                self.preferences_tab = false;
                self.models_tab = false;
                self.memory_tab = false;
                self.sandbox_tab = false;
                self.sync_tab = false;
                self.integration_tab = None;
            }
            if control(
                ui,
                Icon::Chat,
                "Models",
                egui::vec2(110., 35.),
                self.models_tab,
            )
            .clicked()
            {
                self.preferences_tab = false;
                self.models_tab = true;
                self.memory_tab = false;
                self.sandbox_tab = false;
                self.sync_tab = false;
                self.integration_tab = None;
            }
            if control(
                ui,
                Icon::Memory,
                "Memory",
                egui::vec2(112., 35.),
                self.memory_tab,
            )
            .clicked()
            {
                self.preferences_tab = false;
                self.models_tab = false;
                self.memory_tab = true;
                self.sandbox_tab = false;
                self.sync_tab = false;
                self.integration_tab = None;
            }
            if control(
                ui,
                Icon::Shield,
                "Sandbox",
                egui::vec2(118., 35.),
                self.sandbox_tab,
            )
            .clicked()
            {
                self.preferences_tab = false;
                self.models_tab = false;
                self.memory_tab = false;
                self.sandbox_tab = true;
                self.sync_tab = false;
                self.integration_tab = None;
            }
            if control(ui, Icon::Cycle, "Sync", egui::vec2(96., 35.), self.sync_tab).clicked() {
                self.preferences_tab = false;
                self.models_tab = false;
                self.memory_tab = false;
                self.sandbox_tab = false;
                self.sync_tab = true;
                self.integration_tab = None;
            }
            if control(
                ui,
                Icon::Settings,
                "Preferences",
                egui::vec2(136., 35.),
                self.preferences_tab,
            )
            .clicked()
            {
                self.preferences_tab = true;
                self.models_tab = false;
                self.memory_tab = false;
                self.sandbox_tab = false;
                self.sync_tab = false;
                self.integration_tab = None;
            }
            for (tab, icon, label, width) in [
                (IntegrationTab::Plugins, Icon::Plug, "Plugins", 108.),
                (IntegrationTab::ComputerUse, Icon::Shield, "Computer use", 155.),
                (IntegrationTab::Connectors, Icon::Plug, "Connectors", 128.),
            ] {
                if control(ui, icon, label, egui::vec2(width, 35.), self.integration_tab == Some(tab)).clicked() {
                    self.preferences_tab = false;
                    self.models_tab = false;
                    self.memory_tab = false;
                    self.sandbox_tab = false;
                    self.sync_tab = false;
                    self.integration_tab = Some(tab);
                self.browser_setup.manage = None;
                }
            }
        });
        ui.add_space(12.);
        if let Some(tab) = self.integration_tab {
            self.integration_settings(ui, tab);
            return;
        }
        if self.sync_tab {
            self.sync_settings(ui);
            return;
        }
        if self.models_tab {
            self.models_settings(ui);
            ui.add_space(12.);
            let prefs = self.view.state.preferences.clone();
            if let Some(c) = self.images.show(ui, &prefs) {
                self.send(c);
            }
            ui.add_space(12.);
            let speech = self.speech_settings();
            if let Some(c) =
                self.voice_form
                    .show(ui, &prefs, &speech, &self.view.speech, &self.view.audio)
            {
                self.send(c);
            }
            return;
        }
        if self.sandbox_tab {
            self.sandbox_settings(ui);
            return;
        }
        if self.memory_tab {
            self.memory_settings(ui);
            return;
        }
        if self.preferences_tab {
            let mut prefs = self.view.state.preferences.clone();
            let mut changed = false;
            egui::Frame::new()
                .fill(panel())
                .inner_margin(16)
                .corner_radius(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(Text::new("Conversation").size(19.).strong());
                    ui.horizontal(|ui| {
                        ui.label("Message text size");
                        changed |= ui
                            .add(egui::Slider::new(&mut prefs.text_size, 14..=22).suffix(" px"))
                            .changed();
                    });
                    changed |= ui
                        .checkbox(&mut prefs.enter_sends, "Send with Enter")
                        .changed();
                    ui.label(
                        Text::new(if prefs.enter_sends {
                            "Shift+Enter adds a new line."
                        } else {
                            "Ctrl+Enter sends a message. Enter adds a new line."
                        })
                        .size(13.)
                        .color(muted()),
                    );
                });
            ui.add_space(14.);
            ui.heading("Appearance");
            ui.add_space(5.);
            changed |= ui.checkbox(&mut prefs.nerd_stats, "Show nerd stats").changed();
            ui.weak("Show context usage, generation speed, and output token counts in chats.");
            ui.add_space(8.);
            ui.horizontal(|ui| {
                for (theme, label, tip) in [
                    (
                        crate::widgets::Theme::System,
                        "System",
                        "Follow Windows' light or dark setting",
                    ),
                    (crate::widgets::Theme::Light, "Light", "Light backgrounds"),
                    (crate::widgets::Theme::Dark, "Dark", "Charcoal backgrounds"),
                ] {
                    if ui
                        .selectable_label(prefs.theme == theme, label)
                        .on_hover_text(tip)
                        .clicked()
                        && prefs.theme != theme
                    {
                        prefs.theme = theme;
                        changed = true;
                    }
                }
            });
            ui.add_space(10.);
            ui.heading("Accent color");
            ui.horizontal_wrapped(|ui| {
                for a in crate::widgets::Accent::ALL {
                    let selected = prefs.accent == a;
                    let (rect, r) =
                        ui.allocate_exact_size(egui::vec2(30., 30.), egui::Sense::click());
                    let painter = ui.painter();
                    painter.circle_filled(rect.center(), 11., a.color());
                    if selected {
                        painter.circle_stroke(rect.center(), 14., egui::Stroke::new(2_f32, ink()));
                    } else if r.hovered() {
                        painter.circle_stroke(
                            rect.center(),
                            14.,
                            egui::Stroke::new(1_f32, muted()),
                        );
                    }
                    if r.on_hover_text(a.name()).clicked() && !selected {
                        prefs.accent = a;
                        changed = true;
                    }
                }
            });
            ui.add_space(14.);
            if cfg!(any(windows, target_os = "linux")) {
                ui.heading("Startup and background");
                let mut login = prefs.start_on_sign_in.unwrap_or(prefs.background);
                if ui.checkbox(&mut prefs.background, "Keep Spindle running in the tray").changed() {
                    prefs.start_on_sign_in = Some(login);
                    changed = true;
                }
                ui.weak("Closing the window keeps sync and schedules running.");
                if ui.checkbox(&mut login, "Start Spindle when I sign in").changed() {
                    self.send(if login { "/service install" } else { "/service uninstall" });
                }
                ui.add_space(14.);
            }
            ui.heading("Model guidance");
            changed |= ui.checkbox(&mut prefs.over_18, "I’m over 18").changed();
            ui.weak("When unchecked, Spindle asks models to respond in a way suitable for minors. Models may ignore this guidance. Parental supervision is still needed.");
            ui.add_space(14.);
            ui.heading("Context compaction");
            ui.weak("Summarize older exchanges before the next request when the estimated prompt reaches this threshold.");
            changed |= compaction_controls(ui, &mut prefs.compaction);
            ui.separator();
            ui.label(Text::new("Review council").size(20.).strong());
            ui.label(Text::new("Choose up to four models for /council. Each receives the review material independently and returns feedback.").color(muted()));
            let mut roster = self.view.state.council.clone();
            for member in self.view.state.council.clone() {
                let (p, tag) = route(&member);
                ui.horizontal(|ui| {
                    symbol(ui, Icon::Chip, accent());
                    ui.label(format!("{} / {tag}", self.provider_name(p)));
                    if !self.view.models.contains(&member) {
                        ui.label("Unavailable");
                    }
                    if ui
                        .add_enabled(roster.len() > 1, egui::Button::new("Remove"))
                        .clicked()
                    {
                        roster.retain(|m| m != &member);
                    }
                });
            }
            if roster.len() < 4 {
                crate::widgets::InlineChoice::from_id_salt("council-add")
                    .selected_text("Add a model…")
                    .show_ui(ui, |ui| {
                        for m in &self.view.models {
                            let (p, tag) = route(m);
                            if !roster.contains(m)
                                && ui
                                    .button(format!("{} / {tag}", self.provider_name(p)))
                                    .clicked()
                            {
                                roster.push(m.clone());
                            }
                        }
                    });
            }
            if roster != self.view.state.council {
                self.send(format!("/council set {}", roster.join(" ")));
            }
            if changed {
                self.send(format!(
                    "/preferences {}",
                    serde_json::to_string(&prefs).unwrap()
                ));
            }
            ui.add_space(14.);
            #[cfg(any(windows, target_os = "linux"))]
            {
                let has_drafts = !self.draft.is_empty()
                    || self.drafts_by_thread.values().any(|draft| !draft.is_empty())
                    || self.projects.composers.values().any(|draft| !draft.is_empty())
                    || !self.paste_drafts.files.is_empty()
                    || !self.image_drafts.is_empty()
                    || self.images_by_thread.values().any(|images| !images.is_empty());
                let submitting = self.awaiting_submission();
                if self.updates.show(ui, has_drafts, submitting)
                    && let Err(error) = self.shutdown.request_update_restart(&self.tx)
                {
                    self.updates.status = format!("Could not restart. {error:#}");
                }
                if let Some(error) = self.shutdown.restart_error.get() {
                    ui.label(format!("Could not save the workspace for restart. {error}"));
                }
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            ui.label(
                Text::new(format!("Spindle   ·   {}", crate::version()))
                    .color(muted())
                    .size(13.),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(Text::new("Connections").size(20.).strong());
            if ui.button("+  Add provider").clicked() {
                self.provider_setup = ProviderSetup::Choose;
                self.settings_feedback.clear();
            }
        });
        ui.label(
            Text::new("Connect a model server, then choose its models in the composer.")
                .size(14.)
                .color(muted()),
        );
        ui.add_space(8.);
        self.local_server_settings(ui);
        ui.add_space(8.);
        let profiles = self.view.state.providers.clone();
        let endpoint = crate::ollama::endpoint();
        let mut connections: Vec<_> = profiles
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.id.as_str(),
                    p.base.as_str(),
                    p.models.len(),
                    Some(p),
                )
            })
            .collect();
        if self.view.state.preferences.ollama_on {
            let count = |id| self.view.models.iter().filter(|m| route(m).0 == id).count();
            connections.push(("Ollama", "ollama", endpoint.as_str(), count("ollama"), None));
            let cloud = count("ollama-cloud");
            if cloud > 0 || route(&self.view.state.model).0 == "ollama-cloud" {
                connections.push((
                    "Ollama Cloud",
                    "ollama-cloud",
                    "Via your signed-in local Ollama server",
                    cloud,
                    None,
                ));
            }
        }
        if connections.is_empty() {
            ui.weak("No providers yet. Add one: Ollama, LM Studio, OpenAI, Anthropic, OpenRouter or any OpenAI-compatible server.");
        }
        connections.sort_by_cached_key(|(name, id, ..)| (name.to_lowercase(), *id));
        for (name, id, base, models, profile) in connections {
            self.provider_row(ui, id, name, base, models);
            if id == "ollama" {
                // Where Ollama runs: this computer by default, or another one on the network.
                let saved = self.view.state.preferences.ollama.clone();
                let mut save = None;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Ollama address");
                    let field = self.ollama_address.get_or_insert_with(|| saved.clone());
                    let width = (ui.available_width() - 52.).clamp(80., 320.);
                    let edit = self.android_ime.field(
                        ui,
                        "ollama-address",
                        field,
                        Default::default(),
                        |field| {
                            field
                                .hint_text("http://localhost:11434")
                                .desired_width(width)
                        },
                    );
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let changed = field.trim() != saved;
                    if (ui.add_enabled(changed, egui::Button::new("Save")).clicked()
                        || (enter && changed))
                        && !self.view.connecting
                    {
                        save = Some(field.trim().to_owned());
                    }
                });
                if let Some(address) = save {
                    self.set_ollama(true, address);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("API key");
                    self.android_ime.field(ui, "local-ollama-key", &mut self.ollama_key,
                        super::android_ime::Field { password: true, ..Default::default() },
                        |field| field.hint_text("Optional").desired_width(170.));
                    let save = ui.add_enabled(!self.ollama_key.trim().is_empty(), egui::Button::new("Save key")).clicked();
                    let clear = ui.button("Clear key").clicked();
                    if save || clear {
                        let key = std::mem::take(&mut self.ollama_key);
                        self.send(format!("/ollama-key {}", serde_json::json!([saved, if clear { "" } else { key.trim() }])));
                    }
                });
            }
            let Some(p) = profile else { continue };
            #[cfg(any(windows, target_os = "android"))]
            if p.protocol == Protocol::ChatGpt {
                ui.horizontal_wrapped(|ui| {
                    if let Some(r) = &p.registration {
                        ui.label(&r.label);
                    }
                    let label = if p.registration.is_some() {
                        "Sign in again"
                    } else {
                        "Sign in on this device"
                    };
                    if ui
                        .add_enabled(!self.view.connecting, egui::Button::new(label).small())
                        .clicked()
                    {
                        self.send(format!("/reauth {}", p.id));
                    }
                });
            }
            #[cfg(any(windows, target_os = "linux"))]
            if p.protocol == Protocol::ClaudeConsole && p.claude_login.is_some() {
                if ui
                    .add_enabled(
                        !self.view.connecting,
                        egui::Button::new("Sign in again").small(),
                    )
                    .clicked()
                {
                    self.send(format!("/reauth {}", p.id));
                }
            }
        }
        if self.view.connecting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking provider catalog…");
            });
        }
        if !self.settings_feedback.is_empty() {
            ui.label(Text::new(&self.settings_feedback).size(14.).color(accent()));
        }
    }

    fn integration_settings(&mut self, ui: &mut egui::Ui, tab: IntegrationTab) {
        ui.heading(match tab {
            IntegrationTab::Plugins => "Plugins",
            IntegrationTab::ComputerUse => "Computer use",
            IntegrationTab::Connectors => "Connectors",
        });
        ui.add_space(10.);
        match tab {
            IntegrationTab::Plugins => {
                ui.label("External tools extend what Spindle can do from Cowork.");
                if ui.button("Find an external tool").clicked() {
                    self.send("/mode cowork");
                    self.compose("Find an external tool for ");
                }
                ui.add_space(10.);
                ui.strong("Installed tools");
                let tools = crate::tool_catalog::installed_plugins();
                if tools.is_empty() {
                    ui.weak("No external tools installed on this device.");
                }
                for tool in tools { ui.label(tool); }
                return;
            }
            IntegrationTab::Connectors => {
                self.connector_settings(ui);
                return;
            }
            IntegrationTab::ComputerUse => {}
        }
        if !crate::browser::SUPPORTED {
            ui.label("Spindle Browser Link is available on Windows and Linux.");
            return;
        }
        self.poll_browser_setup(ui);
        if let Some(browser) = self.browser_setup.manage {
            self.browser_management(ui, browser);
            return;
        }
        ui.label("Use Browser Link with the tabs in your regular browser.");
        ui.add_space(10.);
        for browser in crate::browser::LinkBrowser::ALL {
            self.browser_settings_card(ui, browser);
            ui.add_space(8.);
        }
        ui.add_space(12.);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Sandbox and native builds").clicked() {
                self.integration_tab = None;
                self.sandbox_tab = true;
            }
            if ui.button("Connected devices").clicked() {
                self.integration_tab = None;
                self.sync_tab = true;
            }
        });
    }

    fn browser_settings_card(&mut self, ui: &mut egui::Ui, browser: crate::browser::LinkBrowser) {
        let mut live = self.view.browser_clients.iter().filter(|client|
            client.browser == browser && client.regular_profile()).peekable();
        let has_client = live.peek().is_some();
        let installed = has_client || browser.installed();
        let outdated = live.clone().any(|client| !client.up_to_date());
        let ready = live.any(|client| client.ready && client.up_to_date());
        let known = has_client || crate::browser_native::connected_before(browser);
        let prepared = crate::browser_native::setup_started(browser);
        let disabled = self.view.state.preferences.disabled_browsers.contains(&browser);
        let status = if !installed { "Browser is not installed" }
            else if disabled { "Access is off" }
            else if ready { "Browser Link connected" }
            else if outdated { "Browser Link update required" }
            else if has_client { "Browser Link needs attention" }
            else if known { "Open this browser to connect" }
            else if prepared { "Finish Browser Link setup" }
            else { "Browser Link is not installed" };
        egui::Frame::new()
            .fill(panel())
            .inner_margin(14)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    symbol(ui, Icon::Plug, if ready && !disabled { accent() } else { ui.visuals().weak_text_color() });
                    ui.vertical(|ui| {
                        ui.strong(browser.label());
                        ui.weak(status);
                    });
                    if installed && known {
                        if ui.button("Manage").clicked() {
                            self.browser_setup.manage = Some(browser);
                        }
                        self.browser_access_toggle(ui, browser);
                    }
                    if installed && !ready && !disabled {
                        if ui.add_enabled(self.browser_setup.install.is_none(),
                            egui::Button::new(if outdated { "Update" } else if prepared || known { "Set up again" } else { "Install" })).clicked()
                        {
                            self.browser_setup.manage = Some(browser);
                            self.launch_browser_link(browser, outdated);
                        }
                    }
                    if !installed {
                        let url = match browser {
                            crate::browser::LinkBrowser::Edge => "https://www.microsoft.com/edge/download",
                            crate::browser::LinkBrowser::Chrome => "https://www.google.com/chrome/",
                        };
                        ui.hyperlink_to("Get browser", url);
                    }
                });
            });
    }

    fn browser_access_toggle(&mut self, ui: &mut egui::Ui, browser: crate::browser::LinkBrowser) {
        let mut enabled = !self.view.state.preferences.disabled_browsers.contains(&browser);
        let label = if enabled { "On" } else { "Off" };
        if crate::widgets::switch(ui, &mut enabled, browser.label())
            .on_hover_text(format!("{label} · Allow Spindle to use this browser in this workspace")).changed() {
            self.send(format!("/browser-control {} {}", browser.key(), if enabled { "on" } else { "off" }));
        }
    }

    fn browser_management(&mut self, ui: &mut egui::Ui, browser: crate::browser::LinkBrowser) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Back").clicked() { self.browser_setup.manage = None; }
            ui.strong(browser.label());
        });
        ui.add_space(8.);
        ui.horizontal_wrapped(|ui| {
            ui.label("Allow Spindle access in this workspace");
            self.browser_access_toggle(ui, browser);
        });
        if self.view.browser_link_kind == Some(browser)
            && let Some((chat, connected)) = self.view.browser_pairing
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} chat #{chat}", if connected { "Connected to" } else { "Connecting to" }));
                if ui.button("Disconnect").clicked() { self.send("/browser-link stop"); }
            });
        }
        if self.browser_setup.browser == browser {
            self.browser_setup_details(ui);
        }
        let available = crate::browser_link::extension_version();
        let versions: std::collections::BTreeSet<_> = self.view.browser_clients.iter()
            .filter(|client| client.browser == browser && client.regular_profile())
            .map(|client| client.version.as_deref().unwrap_or("Unknown")).collect();
        let needs_update = self.view.browser_clients.iter().any(|client|
            client.browser == browser && client.regular_profile() && !client.up_to_date());
        if !versions.is_empty() {
            ui.label(format!("Installed · {}", versions.into_iter().collect::<Vec<_>>().join(", ")));
        }
        ui.weak(format!("Available with this build · {available}"));
        if ui.add_enabled(self.browser_setup.install.is_none(),
            egui::Button::new(if needs_update { "Update Browser Link" } else { "Set up again" })).clicked() {
            self.launch_browser_link(browser, needs_update);
        }
        ui.separator();
        self.browser_policy(ui);
        if let Some(blocker) = crate::engine::browser_access_blocker(&self.view.state) {
            ui.weak(blocker);
        }
        ui.add_space(12.);
        ui.strong("Saved model permissions");
        for grant in self.view.state.grants.iter().filter(|grant| grant.risk == crate::policy::Risk::Browser) {
            ui.horizontal_wrapped(|ui| {
                ui.label(grant.describe());
                if ui.small_button("Revoke").clicked() {
                    let _ = self.tx.send(format!("/revoke {}", grant.id));
                }
            });
        }
    }

    pub(super) fn browser_access_card(&mut self, ui: &mut egui::Ui) -> bool {
        let chat = self.view.state.chat_id;
        let pairing = self.view.browser_pairing.filter(|(owner, _)| *owner == chat);
        let selected = pairing.and(self.view.browser_tab_origin.clone());
        let read = self.view.state.browser_reads.get(&chat).cloned();
        let requested = selected.is_some() && self.view.browser_site_request == selected;
        if read.is_none() && !requested {
            if let Some((id, address)) = self.view.browser_fallback.clone()
                && let Ok(page) = crate::browser::public_page(&address) {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.strong(format!("Spindle couldn’t read {}", page.host_str().unwrap_or("this site")));
                    if let Some(reason) = crate::engine::browser_access_blocker(&self.view.state) {
                        ui.label(reason);
                    } else {
                        ui.label("Continue in your regular browser. Spindle will request access to that tab.");
                        let ready = !self.view.busy && self.view.job.is_none();
                        if ui.add_enabled(ready, egui::Button::new("Use regular browser")).clicked() {
                            self.send(format!("/browser-link fallback {id}"));
                        }
                    }
                });
                return true;
            }
            if crate::engine::interrupted_browser_read(&self.view.state).is_none() { return false; }
            if ui.button("Continue browser request").clicked() { self.send("/browser-link retry"); }
            return true;
        }
        self.poll_browser_setup(ui);
        let destination = read.as_ref().and_then(|r| r.destination.as_ref());
        let policy = crate::browser_link::Policy::from_preferences(&self.view.state.preferences);
        let disabled = &self.view.state.preferences.disabled_browsers;
        let clients: Vec<_> = self.view.browser_clients.iter()
            .filter(|client| client.can_connect(destination) && !disabled.contains(&client.browser))
            .cloned().collect();
        let update_browsers: Vec<_> = crate::browser::LinkBrowser::ALL.into_iter()
            .filter(|browser| !disabled.contains(browser)
                && self.view.browser_clients.iter().any(|client| client.browser == *browser
                    && client.regular_profile() && !client.up_to_date()))
            .collect();
        let updating = self.browser_setup.updating
            && (self.browser_setup.install.is_some() || self.browser_setup.folder.is_some());
        let needs_setup = read.is_some() && clients.is_empty()
            && !self.view.browser_clients.iter().any(|c| c.ready && c.regular_profile()
                && c.up_to_date() && !disabled.contains(&c.browser))
            && !(selected.is_some() && destination.is_none());
        let site = if let Some(page) = destination {
            policy.rule_for(page).is_none().then(|| page.origin().ascii_serialization())
        } else {
            selected.filter(|_| requested || pairing == Some((chat, false)))
        };
        ui.add_space(8.);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            if read.is_some() && clients.is_empty() && (updating || !update_browsers.is_empty()) {
                ui.strong("Update Browser Link to continue");
                ui.label(format!("This request waits for Browser Link {} or later.",
                    crate::browser_link::extension_version()));
                self.browser_setup_details(ui);
                if self.browser_setup.install.is_none() {
                    ui.horizontal_wrapped(|ui| {
                        for browser in update_browsers {
                            if ui.button(format!("Update Browser Link for {}", browser.label())).clicked() {
                                self.launch_browser_link(browser, true);
                            }
                        }
                    });
                }
            } else if needs_setup {
                ui.strong("Connect your regular browser");
                ui.label("Set up Browser Link in the profile with your tabs and sign-ins. This chat will continue when the extension connects.");
                self.browser_setup_controls(ui);
            } else if let Some(site) = site {
                ui.strong("Allow access to this website?");
                ui.label(&site);
                if let Some(read) = &read {
                    if let Some(page) = destination { ui.label(format!("Page: {page}")); }
                    ui.label(format!("Let {} read this page once. “Allow all” makes every website eligible for future requests.", read.model));
                }
                ui.horizontal_wrapped(|ui| {
                    for (label, choice) in [("Allow this site", "site"), ("Allow all", "all"), ("Deny", "deny")] {
                        if ui.button(label).clicked() {
                            self.send(if let Some(read) = &read {
                                format!("/browser-site read-{choice} {chat} {} {site}", read.id)
                            } else { format!("/browser-site {choice} {chat} {site}") });
                        }
                    }
                });
                return;
            } else if let Some(read) = &read {
                if clients.is_empty() && destination.is_none() {
                    ui.strong("Which page should Spindle open?");
                    ui.label("Enter the website address, or switch to the page in your connected browser.");
                    if self.browser_setup.page_request != Some(read.id) {
                        self.browser_setup.page_address.clear();
                        self.browser_setup.page_request = Some(read.id);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.browser_setup.page_address).hint_text("https://…"));
                        if ui.add_enabled(crate::browser_link::page_url(&self.browser_setup.page_address).is_ok(), egui::Button::new("Continue")).clicked() {
                            self.send(format!("/browser-link page {} {}", read.id, self.browser_setup.page_address.trim()));
                        }
                    });
                } else if clients.len() > 1 && destination.is_none() {
                    ui.strong("Which browser has your page?");
                    ui.horizontal_wrapped(|ui| {
                        for (index, client) in clients.iter().enumerate() {
                            let mut label = client.browser.label().to_string();
                            if clients.iter().filter(|c| c.browser == client.browser).count() > 1 { label.push_str(&format!(" · {}", index + 1)); }
                            if ui.button(label).clicked() { self.send(format!("/browser-link connect {}", client.id)); }
                        }
                    });
                } else {
                    ui.horizontal(|ui| { ui.spinner(); ui.label("Connecting to your page…"); });
                }
            }
            if let Some(read) = &read && ui.button("Cancel").clicked() {
                self.send(format!("/browser-site read-deny {chat} {}", read.id));
            }
        });
        true
    }

    fn poll_browser_setup(&mut self, ui: &egui::Ui) {
        let setup = &mut self.browser_setup;
        if let Some(install) = &setup.install {
            match install.try_recv() {
                Ok(result) => {
                    setup.install = None;
                    match result {
                        Ok(folder) => {
                            ui.ctx().copy_text(folder.display().to_string());
                            if let Err(error) = setup.browser.open_install_guide(&folder) {
                                setup.status = format!("The extension is ready. Could not open the install guide. {error:#}");
                            }
                            setup.folder = Some(folder);
                        }
                        Err(error) => setup.status = format!("{error:#}"),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(150)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    setup.install = None;
                    setup.status = "The extension installer stopped.".into();
                }
            }
        }
    }

    fn launch_browser_link(&mut self, browser: crate::browser::LinkBrowser, updating: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(browser.prepare_existing_profile());
        });
        self.browser_setup.browser = browser;
        self.browser_setup.updating = updating;
        self.browser_setup.existing_clients = self.view.browser_clients.iter()
            .filter(|client| client.browser == browser && client.regular_profile())
            .map(|client| client.id.clone()).collect();
        self.browser_setup.folder = None;
        self.browser_setup.install = Some(rx);
        self.browser_setup.status.clear();
    }

    fn browser_setup_details(&mut self, ui: &mut egui::Ui) {
        let setup = &mut self.browser_setup;
        if setup.install.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Preparing Browser Link for {}…", setup.browser.label()));
            });
            return;
        }
        if let Some(folder) = &setup.folder {
            let connected = self.view.browser_clients.iter().any(|client|
                client.regular_profile() && client.browser == setup.browser
                    && !setup.existing_clients.contains(&client.id)
                    && client.up_to_date());
            if connected {
                ui.strong(format!("Browser Link connected · {}", setup.browser.label()));
            } else {
                ui.strong(format!("Ready to add to {}", setup.browser.label()));
                if setup.updating {
                    ui.label("In Manage extensions, remove the older Spindle Browser Link before loading this folder. Spindle will show the new version when it connects.");
                }
                if cfg!(windows) {
                    ui.label("Browser Link is unpacked in Downloads. In Manage extensions, turn on Developer mode, choose Load unpacked, and select the Browser Link folder.");
                } else {
                    ui.label("Follow the guide in your browser. Drag the prepared folder onto the Extensions page or choose Load unpacked and paste its path.");
                }
                ui.monospace(folder.display().to_string());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Show extension folder").clicked() {
                        setup.status = reveal_folder(folder).err().map_or_else(String::new, |error| error.to_string());
                    }
                    if ui.button("Copy folder path").clicked() {
                        ui.ctx().copy_text(folder.display().to_string());
                    }
                    if ui.button("Reopen install guide").clicked() {
                        setup.status = setup.browser.open_install_guide(folder).err().map_or_else(String::new, |error| format!("{error:#}"));
                    }
                });
            }
        }
        if !setup.status.is_empty() {
            ui.label(&setup.status);
            if setup.folder.is_none() && let Some(url) = crate::browser_link::extension_download_url() {
                ui.hyperlink_to("Download Browser Link ZIP", url);
            }
        }
    }

    fn browser_setup_controls(&mut self, ui: &mut egui::Ui) {
        use crate::browser::LinkBrowser;
        self.browser_setup_details(ui);
        if self.browser_setup.install.is_some() { return; }
        let mut available = LinkBrowser::available().peekable();
        if available.peek().is_none() {
            ui.label("Browser Link needs Microsoft Edge or Google Chrome.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for browser in available {
                if self.view.state.preferences.disabled_browsers.contains(&browser) {
                    if ui.button(format!("Enable {}", browser.label())).clicked() {
                        self.send(format!("/browser-control {} on", browser.key()));
                    }
                    continue;
                }
                let linked = self.view.browser_clients.iter().any(|c| c.regular_profile() && c.browser == browser);
                let label = if linked {
                    format!("Set up another {} profile", browser.label())
                } else if self.browser_setup.folder.is_some() && self.browser_setup.browser == browser {
                    "Restart setup".to_owned()
                } else {
                    format!("Set up {}", browser.label())
                };
                if ui.button(label).clicked() {
                    self.launch_browser_link(browser, false);
                }
            }
        });
    }

    fn browser_policy(&mut self, ui: &mut egui::Ui) {
        use crate::browser_link::Mode;
        let policy = crate::browser_link::Policy::from_preferences(&self.view.state.preferences);
        ui.strong("Website access");
        ui.weak("Website rules apply to both Edge and Chrome in this workspace.");
        ui.horizontal_wrapped(|ui| {
            for mode in [Mode::AllowAll, Mode::Allowlist, Mode::Blocklist, Mode::BlockAll] {
                if ui.selectable_label(mode == policy.mode, mode.label()).clicked() {
                    self.send(format!("/browser-mode {}", serde_json::json!(mode)));
                }
            }
        });
        if matches!(policy.mode, Mode::Allowlist | Mode::Blocklist) {
            let adding = if policy.mode == Mode::Allowlist { "/browser-allow" } else { "/browser-block" };
            let removing = if policy.mode == Mode::Allowlist { "/browser-disallow" } else { "/browser-unblock" };
            ui.weak("Use an exact domain or a path. Subdomains require their own entries. Lists follow Settings sync.");
            ui.horizontal_wrapped(|ui| {
                let width = ui.available_width().clamp(100., 300.);
                self.android_ime.field(ui, "browser-rule", &mut self.browser_setup.url, Default::default(), |edit| {
                    edit.hint_text("https://example.com or https://example.com/docs").desired_width(width)
                });
                if ui.add_enabled(!self.browser_setup.url.trim().is_empty(), egui::Button::new("Add URL")).clicked() {
                    let value = std::mem::take(&mut self.browser_setup.url);
                    self.send(format!("{adding} {}", value.trim()));
                }
            });
            for rule in policy.rules {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&rule);
                    if ui.small_button("Remove").clicked() { self.send(format!("{removing} {rule}")); }
                });
            }
        }
        if self.view.browser_tab_origin.is_some() {
            self.browser_access_card(ui);
        }
    }

    fn connector_settings(&mut self, ui: &mut egui::Ui) {
        let form = &mut self.connector_setup;
        if let Some(job) = &form.connecting {
            match job.try_recv() {
                Ok(result) => {
                    form.connecting = None;
                    form.status = result.map_or_else(|e| format!("{e:#}"), |()| "Connected and verified".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(150)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    form.connecting = None;
                    form.status = "The connection check stopped.".into();
                }
            }
        }
        ui.label("Connect accounts for approved, read-only lookups in Cowork. Credentials stay on this device.");
        ui.horizontal_wrapped(|ui| {
            for service in crate::connectors::Service::ALL {
                let name = if service.connected() { format!("{} · connected", service.label()) } else { service.label().into() };
                if ui.add_enabled(form.connecting.is_none(), egui::Button::new(name).selected(form.service == service)).clicked() {
                    form.service = service;
                    form.credential.clear();
                    form.status.clear();
                }
            }
        });
        ui.add_space(12.);
        let service = form.service;
        ui.strong(service.label());
        ui.label(service.scope());
        ui.horizontal_wrapped(|ui| {
            let width = ui.available_width().clamp(100., 300.);
            self.android_ime.field(ui, "connector-token", &mut form.credential,
                super::android_ime::Field { password: true, ..Default::default() },
                |edit| edit.hint_text("Access token").desired_width(width));
            if ui.add_enabled(form.connecting.is_none() && !form.credential.trim().is_empty(), egui::Button::new("Test and connect")).clicked() {
                let secret = std::mem::take(&mut form.credential);
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread().enable_all().build()
                        .map_err(anyhow::Error::from)
                        .and_then(|runtime| runtime.block_on(service.connect(&secret)));
                    let _ = tx.send(result);
                });
                form.connecting = Some(rx);
                form.status = format!("Connecting to {}…", service.label());
            }
            if ui.add_enabled(form.connecting.is_none() && service.connected(), egui::Button::new("Disconnect")).clicked() {
                form.status = service.disconnect().map_or_else(|e| format!("{e:#}"), |()| "Disconnected on this device".into());
            }
        });
        if !form.status.is_empty() { ui.label(&form.status); }
    }

    fn local_server_settings(&mut self, ui: &mut egui::Ui) {
        use crate::local_servers::{Kind, Server};
        let mut connect: Option<Server> = None;
        let local = &mut self.local_servers;
        if let Some(search) = &local.search {
            match search.try_recv() {
                Ok(result) => {
                    local.search = None;
                    match result {
                        Ok(servers) => {
                            local.status = if servers.is_empty() {
                                "No servers answered on the standard ports. Use Add provider for a custom address or credentials.".into()
                            } else { String::new() };
                            local.results = servers;
                        }
                        Err(error) => local.status = error.to_string(),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(150)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    local.search = None;
                    local.status = "Discovery ended before getting a result.".into();
                }
            }
        }
        egui::CollapsingHeader::new("Find local AI servers").id_salt("local-ai-servers").show(ui, |ui| {
            ui.weak("Check standard service ports on this computer or on a host you specify.");
            ui.horizontal_wrapped(|ui| {
                let width = ui.available_width().clamp(120., 240.);
                self.android_ime.field(ui, "local-server-host", &mut local.host, Default::default(), |field| {
                    field.hint_text("localhost or LAN hostname").desired_width(width)
                });
                if ui.add_enabled(local.search.is_none(), egui::Button::new("Find servers")).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let host = local.host.clone();
                    std::thread::spawn(move || {
                        let result = tokio::runtime::Builder::new_current_thread().enable_all().build()
                            .map_err(anyhow::Error::from)
                            .and_then(|runtime| runtime.block_on(crate::local_servers::discover(&host)));
                        let _ = tx.send(result);
                    });
                    local.search = Some(rx);
                    local.status = "Looking for local servers…".into();
                    local.results.clear();
                }
            });
            if !local.status.is_empty() {
                ui.weak(&local.status);
            }
            let prefs = &self.view.state.preferences;
            for server in &local.results {
                let active = match server.kind {
                    Kind::Ollama => prefs.ollama_on && crate::ollama::endpoint().trim_end_matches('/') == server.url,
                    Kind::Compatible => self.view.state.providers.iter().any(|p| p.base.trim_end_matches('/') == server.url),
                    Kind::ComfyUi => prefs.comfyui == server.url && prefs.image_backend == crate::comfy::Backend::ComfyUi,
                    Kind::StableDiffusion => prefs.sd_webui == server.url && prefs.image_backend == crate::comfy::Backend::StableDiffusion,
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(Text::new(server.name).strong());
                    ui.weak(format!("{} · {} models", server.url, server.models));
                    if active {
                        ui.weak("Connected");
                    } else if ui.add_enabled(!self.view.connecting, egui::Button::new(match server.kind {
                        Kind::ComfyUi | Kind::StableDiffusion => "Use for images",
                        _ => "Connect",
                    })).clicked() {
                        connect = Some(server.clone());
                    }
                });
            }
        });
        if let Some(server) = connect {
            match server.kind {
                Kind::Ollama => self.set_ollama(true, server.url),
                Kind::Compatible => {
                    let form = Connection { name: server.name.into(), base: server.url, ..Default::default() };
                    self.send(format!("/connect {}", serde_json::json!(form)));
                }
                Kind::ComfyUi | Kind::StableDiffusion => {
                    let command = if server.kind == Kind::ComfyUi { "server" } else { "sd-webui" };
                    self.send(format!("/image {command} {}", server.url));
                    self.images = Default::default();
                }
            }
        }
    }

    fn provider_row(&mut self, ui: &mut egui::Ui, id: &str, name: &str, base: &str, count: usize) {
        egui::Frame::new()
            .fill(panel())
            .stroke(egui::Stroke::new(1.0_f32, line()))
            .corner_radius(11)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    symbol(ui, Icon::Chip, accent());
                    ui.label(Text::new(name).size(17.).strong());
                    ui.weak(format!("{count} models"));
                });
                ui.add(egui::Label::new(Text::new(base).monospace().size(12.).color(muted())).wrap());
                ui.horizontal_wrapped(|ui| {
                        if id != "ollama-cloud" && ui.small_button("Remove").clicked() {
                            self.remove_provider = Some(id.into());
                        }
                        if ui
                            .add_enabled(!self.view.connecting, egui::Button::new("Refresh"))
                            .clicked()
                        {
                            self.send(format!(
                                "/refresh {}",
                                if id == "ollama-cloud" { "ollama" } else { id }
                            ));
                        }
                });
                if self.remove_provider.as_deref() == Some(id) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Remove this provider connection?");
                        if ui.button("Remove connection").clicked() {
                            if id == "ollama" {
                                let address = self.view.state.preferences.ollama.clone();
                                self.set_ollama(false, address);
                            } else {
                                self.send(format!("/disconnect {id}"));
                            }
                            self.remove_provider = None;
                        }
                        if ui.button("Keep").clicked() {
                            self.remove_provider = None;
                        }
                    });
                }
            });
    }

    /// Add or remove Ollama as a provider, at this address.
    fn set_ollama(&mut self, on: bool, address: String) {
        let mut prefs = self.view.state.preferences.clone();
        prefs.ollama_on = on;
        prefs.ollama = address;
        self.send(format!(
            "/preferences {}",
            serde_json::to_string(&prefs).unwrap()
        ));
        self.send("/refresh ollama");
        self.ollama_address = None;
    }

    fn add_provider_panel(&mut self, ui: &mut egui::Ui) {
        let step = self.provider_setup;
        let title = if let ProviderSetup::Configure(name) = step {
            name
        } else {
            "Add provider"
        };
        if super::panel_header(ui, title, "Back") {
            self.provider_setup = if matches!(step, ProviderSetup::Configure(_)) {
                ProviderSetup::Choose
            } else {
                ProviderSetup::Connections
            };
            self.connection = Connection::default();
            self.settings_feedback.clear();
            return;
        }
        if let ProviderSetup::Configure(name) = step {
            self.provider_form(ui, name);
        } else {
            let size = egui::vec2(ui.available_width(), 42.);
            for &(name, base, protocol) in TEMPLATES {
                let selected = ui.allocate_ui_with_layout(
                    size,
                    egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Min),
                    |ui| ui.add(egui::Button::new(Text::new(name).size(15.)).wrap().min_size(size)),
                ).inner.clicked();
                if selected {
                    self.connection = Connection {
                        name: name.into(),
                        base: base.into(),
                        protocol,
                        key: String::new(),
                    };
                    self.provider_setup = ProviderSetup::Configure(name);
                }
            }
        }
    }

    fn provider_form(&mut self, ui: &mut egui::Ui, name: &str) {
        let width = ui.available_width();
        if name == "Ollama" {
            ui.label("Address of the computer running Ollama");
            self.android_ime.field(
                ui,
                "ollama-provider-base",
                &mut self.connection.base,
                Default::default(),
                |field| {
                    field
                        .desired_width(width)
                        .hint_text("http://localhost:11434")
                },
            );
            if cfg!(target_os = "android") {
                ui.weak("Enter the network address of the Ollama server.");
            } else {
                ui.weak("Leave it empty for this computer.");
            }
            ui.label("API key (optional)");
            self.android_ime.field(ui, "ollama-provider-key", &mut self.connection.key,
                super::android_ime::Field { password: true, ..Default::default() },
                |field| field.desired_width(width).hint_text("Leave blank for local access"));
            if ui
                .add_enabled(!self.view.connecting, egui::Button::new("Add Ollama"))
                .clicked()
            {
                let address = self.connection.base.trim().to_owned();
                let key = std::mem::take(&mut self.connection.key);
                self.send(format!("/ollama-connect {}", serde_json::json!([address, key.trim()])));
                self.provider_setup = ProviderSetup::Connections;
            }
            return;
        }
        ui.label("Display name");
        self.android_ime.field(
            ui,
            "provider-name",
            &mut self.connection.name,
            Default::default(),
            |field| field.desired_width(width),
        );
        let action = match self.connection.protocol {
            Protocol::ChatGpt => {
                ui.label("Use your ChatGPT plan for eligible models. Your browser opens for account selection and consent.");
                "Continue with ChatGPT"
            }
            Protocol::ClaudeConsole => {
                ui.label("Sign in to a Claude Console workspace through the official Anthropic CLI. Usage is billed to that API workspace.");
                ui.hyperlink_to(
                    "Get the Anthropic CLI",
                    "https://github.com/anthropics/anthropic-cli/releases/latest",
                );
                ui.label("Anthropic CLI executable");
                self.android_ime.field(
                    ui,
                    "provider-ant",
                    &mut self.connection.base,
                    Default::default(),
                    |field| {
                        field.desired_width(width).hint_text(if cfg!(windows) {
                            "Full path to ant.exe"
                        } else {
                            "Full path to ant"
                        })
                    },
                );
                #[cfg(windows)]
                if ui.small_button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Windows executable", &["exe"])
                        .pick_file()
                {
                    self.connection.base = crate::projects::display_path(&path);
                }
                "Continue with Claude Console"
            }
            protocol => {
                ui.label("API base URL");
                self.android_ime.field(
                    ui,
                    "provider-api-base",
                    &mut self.connection.base,
                    Default::default(),
                    |field| {
                        field
                            .desired_width(width)
                            .hint_text("https://server.example/v1")
                    },
                );
                ui.label("API key");
                self.android_ime.field(
                    ui,
                    "provider-api-key",
                    &mut self.connection.key,
                    super::android_ime::Field {
                        password: true,
                        ..Default::default()
                    },
                    |field| {
                        field
                            .desired_width(width)
                            .hint_text("Optional for local servers")
                    },
                );
                if protocol == Protocol::Messages {
                    ui.hyperlink_to(
                        "Manage Claude Console API keys",
                        "https://platform.claude.com/settings/keys",
                    );
                }
                "Test & connect"
            }
        };
        if !self.settings_feedback.is_empty() {
            ui.label(Text::new(&self.settings_feedback).color(accent()));
        }
        let enabled = !self.view.connecting
            && !self.connection.name.trim().is_empty()
            && !self.connection.base.trim().is_empty();
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(if self.view.connecting {
                    "Connecting…"
                } else {
                    action
                }),
            )
            .clicked()
        {
            self.settings_feedback.clear();
            self.send(format!(
                "/connect {}",
                serde_json::to_string(&self.connection).unwrap()
            ));
        }
    }
}

pub(super) fn compaction_controls(
    ui: &mut egui::Ui,
    policy: &mut crate::compaction::Policy,
) -> bool {
    let before = *policy;
    ui.horizontal_wrapped(|ui| {
        for (value, label, hover) in [
            (25, "Aggressive", "Summarize early to keep token use and cost down"),
            (50, "Normal", "A balance of cost and remembered detail"),
            (75, "Generous", "Keep more of the conversation word for word"),
        ] {
            if ui
                .selectable_label(!policy.dynamic && policy.percent == value, label)
                .on_hover_text(hover)
                .clicked()
            {
                policy.percent = value;
                policy.dynamic = false;
            }
        }
        if ui
            .selectable_label(policy.dynamic, "Dynamic")
            .on_hover_text("Generous while a model has plenty of allowance, more aggressive as it nears a usage limit set in Automations → Usage limits")
            .clicked()
        {
            policy.dynamic = true;
        }
    });
    if !policy.dynamic {
        egui::CollapsingHeader::new(Text::new("Fine-tune").color(muted()).size(13.))
            .id_salt(ui.next_auto_id())
            .default_open(false)
            .show(ui, |ui| {
                ui.add(
                    egui::Slider::new(&mut policy.percent, 1..=100)
                        .suffix("%")
                        .text("Compact when context reaches"),
                );
                let mut capped = policy.token_cap.is_some();
                if ui
                    .checkbox(&mut capped, "Also set a token ceiling")
                    .changed()
                {
                    policy.token_cap = capped.then_some(8192);
                }
                if let Some(cap) = &mut policy.token_cap {
                    ui.add(
                        egui::DragValue::new(cap)
                            .range(1..=u32::MAX)
                            .speed(64)
                            .suffix(" tokens"),
                    );
                }
            });
    }
    *policy != before
}

impl Desktop {
    /// Settings → Models: choose which connected models appear in the picker. Advanced
    /// per-model settings live behind each row's "Advanced…" button.
    fn models_settings(&mut self, ui: &mut egui::Ui) {
        let total = self.view.models.len();
        let hidden = self
            .view
            .models
            .iter()
            .filter(|m| self.view.state.hidden_models.contains(*m))
            .count();
        egui::Frame::new()
            .fill(panel())
            .inner_margin(16)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(Text::new("Models").strong().size(18.));
                    ui.weak(format!("{} shown · {hidden} hidden", total - hidden));
                });
                ui.weak("Hidden models leave the model picker. They stay connected, and work already using them continues.");
                ui.add_space(6.);
                let mut providers: Vec<String> = self
                    .view
                    .models
                    .iter()
                    .map(|m| route(m).0.to_owned())
                    .collect();
                providers.sort_by_cached_key(|p| (self.provider_name(p).to_lowercase(), p.clone()));
                providers.dedup();
                let width = ui.available_width().min(340.);
                ui.add(egui::TextEdit::singleline(&mut self.model_query)
                    .id_salt("settings-field-6")
                    .hint_text("Search models")
                    .desired_width(width));
                let label = self.model_provider.as_deref()
                    .map_or("All providers", |p| self.provider_name(p));
                let mut choice = self.model_provider.clone();
                crate::widgets::InlineChoice::from_label("Provider")
                    .width(width)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut choice, None, "All providers");
                        for p in &providers {
                            ui.selectable_value(&mut choice, Some(p.clone()), self.provider_name(p));
                        }
                    });
                self.model_provider = choice;
                let query = self.model_query.to_lowercase();
                let shown: Vec<String> = self
                    .view
                    .models
                    .iter()
                    .filter(|m| {
                        let (p, name) = route(m);
                        self.model_provider.as_deref().is_none_or(|f| f == p)
                            && (query.is_empty() || name.to_lowercase().contains(&query))
                    })
                    .cloned()
                    .collect();
                let mut change: Option<(bool, Vec<String>)> = None;
                ui.horizontal(|ui| {
                    ui.weak(format!("{} listed", shown.len()));
                    if ui.small_button("Show all listed").clicked() {
                        change = Some((true, shown.clone()));
                    }
                    if ui.small_button("Hide all listed").clicked() {
                        change = Some((false, shown.clone()));
                    }
                });
                ui.separator();
                let row_height = 30.;
                egui::ScrollArea::vertical()
                    .id_salt("models-list")
                    .max_height(520.)
                    .auto_shrink([false, true])
                    .show_rows(ui, row_height, shown.len(), |ui, range| {
                        for m in &shown[range] {
                            let (p, name) = route(m);
                            ui.horizontal(|ui| {
                                let mut visible = !self.view.state.hidden_models.contains(m);
                                if ui
                                    .checkbox(&mut visible, "")
                                    .on_hover_text(if visible {
                                        "Shown in the model picker"
                                    } else {
                                        "Hidden from the model picker"
                                    })
                                    .changed()
                                {
                                    change = Some((visible, vec![m.clone()]));
                                }
                                let text = Text::new(name);
                                ui.label(if visible { text } else { text.color(muted()) });
                                ui.weak(self.provider_name(p));
                                self.capability_icons(ui, m, 14., true);
                                if *m == self.view.state.model {
                                    ui.weak("· Selected");
                                }
                                if self.view.state.tuning.contains_key(m) {
                                    ui.label(Text::new("· Custom settings").color(accent()).size(12.));
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button("Advanced…")
                                            .on_hover_text("Compaction, thinking, temperature and context window for this model")
                                            .clicked()
                                        {
                                            self.tuning_editor = Some((
                                                m.clone(),
                                                self.view
                                                    .state
                                                    .tuning
                                                    .get(m)
                                                    .cloned()
                                                    .unwrap_or_default(),
                                            ));
                                        }
                                    },
                                );
                            });
                        }
                    });
                if let Some(change) = change {
                    let _ = self.tx.send(format!(
                        "/models-visible {}",
                        serde_json::to_string(&change).expect("Serializable model list")
                    ));
                }
            });
    }

    /// The advanced settings window for one model, opened only from Settings → Models.
    pub(super) fn tuning_panel(&mut self, ui: &mut egui::Ui) {
        use crate::state::Thinking;
        let Some((model, tuning)) = &mut self.tuning_editor else {
            return;
        };
        let (provider, name) = route(model);
        let local_ollama = provider == "ollama";
        let mut open = true;
        let (mut save, mut reset, mut cancel) = (false, false, false);
        super::detail_panel(
            ui,
            &format!("Advanced settings · {name}"),
            "Back",
            &mut open,
            |ui| {
                egui::Frame::new()
                    .fill(Color::from_rgb(60, 48, 30))
                    .corner_radius(8)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.label(Text::new("Advanced").strong().color(Color::WHITE));
                        ui.label("These change how this model behaves in every chat. The defaults suit most models. A wrong value can make replies fail, ramble or get cut off. Reset to defaults any time.");
                    });
                ui.add_space(8.);
                ui.label(Text::new("Context compaction").strong());
                ui.weak("How early older messages are summarized for this model. Compact sooner on costly models to keep token use down.");
                let mut own = tuning.compaction.is_some();
                if ui
                    .checkbox(
                        &mut own,
                        "Use a setting for this model instead of the workspace default",
                    )
                    .changed()
                {
                    tuning.compaction = own.then(|| crate::compaction::Policy {
                        percent: 25,
                        ..Default::default()
                    });
                }
                if let Some(p) = &mut tuning.compaction {
                    compaction_controls(ui, p);
                }
                ui.add_space(8.);
                ui.label(Text::new("Thinking").strong());
                ui.weak(
                    "For reasoning models. Others ignore it, and a few may reject the request.",
                );
                let shown = match &tuning.think_value {
                    Some(v) => crate::ollama::ThinkInfo::label(v),
                    None => tuning.thinking.label().to_owned(),
                };
                let reported = self.view.think_levels.get(model).filter(|info| info.supported());
                crate::widgets::InlineChoice::from_id_salt("tuning-thinking")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        let standard = if reported.is_some() { &Thinking::ALL[..1] } else { &Thinking::ALL[..] };
                        for &t in standard {
                            if ui
                                .selectable_label(
                                    tuning.think_value.is_none() && tuning.thinking == t,
                                    t.label(),
                                )
                                .clicked()
                            {
                                tuning.thinking = t;
                                tuning.think_value = None;
                            }
                        }
                        if let Some(info) = reported {
                            for value in &info.values {
                                if ui.selectable_label(tuning.think_value.as_ref() == Some(value),
                                    crate::ollama::ThinkInfo::label(value)).clicked()
                                {
                                    tuning.thinking = Thinking::Default;
                                    tuning.think_value = Some(value.clone());
                                }
                            }
                        }
                    });
                ui.add_space(8.);
                ui.label(Text::new("Trust for actions on your computer").strong());
                let default = crate::policy::default_trust(model, None, crate::automation::now());
                let shown = tuning.trust.unwrap_or(default);
                crate::widgets::InlineChoice::from_id_salt("model-trust")
                    .selected_text(if tuning.trust.is_none() {
                        format!("Automatic · {}", default.label())
                    } else {
                        shown.label().to_owned()
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(tuning.trust.is_none(), format!("Automatic · {}", default.label()))
                            .on_hover_text("Models of 16B parameters or fewer start Restricted, very large ones Trusted, others Standard")
                            .clicked()
                        {
                            tuning.trust = None;
                        }
                        for t in crate::policy::Trust::ALL {
                            if ui
                                .selectable_label(tuning.trust == Some(t), t.label())
                                .on_hover_text(t.describe())
                                .clicked()
                            {
                                tuning.trust = Some(t);
                            }
                        }
                    });
                ui.weak(shown.describe());
                ui.add_space(8.);
                let mut priced = tuning.price_in.is_some();
                if ui
                    .checkbox(&mut priced, Text::new("Price for the API budget").strong())
                    .on_hover_text("Used when the provider doesn't publish prices")
                    .changed()
                {
                    tuning.price_in = priced.then_some(1.);
                    tuning.price_out = priced.then_some(4.);
                }
                if let (Some(i), Some(o)) = (&mut tuning.price_in, &mut tuning.price_out) {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(i)
                                .range(0.0..=1000.)
                                .speed(0.05)
                                .prefix("$")
                                .suffix(" in"),
                        );
                        ui.add(
                            egui::DragValue::new(o)
                                .range(0.0..=1000.)
                                .speed(0.05)
                                .prefix("$")
                                .suffix(" out"),
                        );
                        ui.weak("per million tokens");
                    });
                }
                ui.add_space(8.);
                let mut custom_temperature = tuning.temperature.is_some();
                if ui
                    .checkbox(&mut custom_temperature, Text::new("Temperature").strong())
                    .changed()
                {
                    tuning.temperature = custom_temperature.then_some(0.7);
                }
                if let Some(t) = &mut tuning.temperature {
                    ui.add(egui::Slider::new(t, 0.0..=2.0).step_by(0.05));
                    ui.weak("Lower is more focused and repeatable; higher is more varied.");
                }
                ui.add_space(8.);
                {
                    let mut custom_context = tuning.context.is_some();
                    if ui
                        .checkbox(&mut custom_context, Text::new("Context window").strong())
                        .changed()
                    {
                        tuning.context = custom_context.then_some(16_384);
                    }
                    if let Some(c) = &mut tuning.context {
                        ui.add(
                            egui::DragValue::new(c)
                                .range(2_048..=2_097_152)
                                .speed(512)
                                .suffix(" tokens"),
                        );
                        ui.weak(if local_ollama {
                            "Larger windows hold more conversation but use more GPU memory and load slower. Spindle never asks for more than the model supports."
                        } else {
                            "The context window the server runs this model with. Spindle reads it from the server when it can; set it here when the server doesn't say. It doesn't change the server: set the server's own context there (in Project Maya, Settings › Model › Context size)."
                        });
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    save = ui.button("Save").clicked();
                    reset = ui.button("Reset to defaults").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            },
        );
        if reset {
            *tuning = Default::default();
        }
        if save || reset {
            let _ = self.tx.send(format!(
                "/model-tuning {}",
                serde_json::to_string(&(model.clone(), tuning.clone()))
                    .expect("Serializable tuning")
            ));
        }
        if !open || save || cancel {
            self.tuning_editor = None;
        }
    }
}

impl Desktop {
    /// Composer control for models that can reason: a button that opens a stepped
    /// Off/Low/Medium/High slider. Applies to this model in every chat, the same setting
    /// as Settings → Models → Advanced.
    pub(super) fn thinking_toggle(&mut self, ui: &mut egui::Ui, compact: bool) {
        self.thinking_control(ui, compact, false);
    }

    pub(super) fn controls_panel(&mut self, ui: &mut egui::Ui, panel: ControlsPanel) {
        if panel == ControlsPanel::Models {
            self.model_panel(ui);
            return;
        }
        let title = match panel {
            ControlsPanel::Thinking => "Thinking",
            ControlsPanel::PlanUsage => "Plan usage",
            ControlsPanel::Budget => "API budget",
            ControlsPanel::Models => unreachable!(),
        };
        let mut open = true;
        super::detail_panel(ui, title, "Back", &mut open, |ui| {
            if panel == ControlsPanel::Thinking {
                self.thinking_control(ui, false, true);
            } else {
                self.composer_usage(ui, Some(panel));
            }
        });
        if !open {
            self.controls_panel = None;
        }
    }

    fn thinking_control(&mut self, ui: &mut egui::Ui, compact: bool, editing: bool) {
        use crate::ollama::ThinkInfo;
        use crate::state::Thinking;
        #[derive(Clone, PartialEq)]
        enum Step {
            Value(String),
            Level(Thinking),
        }
        let model = self.view.state.model.clone();
        if !self.view.reasoning.contains(&model) {
            return;
        }
        let mut tuning = self
            .view
            .state
            .tuning
            .get(&model)
            .cloned()
            .unwrap_or_default();
        let before = (tuning.thinking, tuning.think_value.clone());
        // Use the levels the model reports about itself; otherwise a generic scale.
        let reported = self
            .view
            .think_levels
            .get(&model)
            .filter(|i| i.supported())
            .cloned();
        let steps: Vec<(String, Step)> = match &reported {
            Some(info) => info
                .values
                .iter()
                .map(|v| (ThinkInfo::label(v), Step::Value(v.clone())))
                .collect(),
            None => [
                Thinking::Off,
                Thinking::Low,
                Thinking::Medium,
                Thinking::High,
            ]
            .into_iter()
            .map(|t| (t.label().to_owned(), Step::Level(t)))
            .collect(),
        };
        let selected = |t: &crate::state::Tuning| {
            steps.iter().position(|(_, s)| match s {
                Step::Value(v) => t.think_value.as_ref() == Some(v)
                    || (t.think_value.is_none()
                        && (t.thinking.effort() == Some(v.as_str()) || (v == "none" && t.thinking == Thinking::Off))),
                Step::Level(l) => t.think_value.is_none() && t.thinking == *l,
            })
        };
        let auto = match reported.as_ref().and_then(|i| i.default.as_deref()) {
            Some(d) => format!("Auto · {}", ThinkInfo::label(d)),
            None => "Auto".into(),
        };
        let current = selected(&tuning).map_or(auto.clone(), |i| steps[i].0.clone());
        let set = selected(&tuning).is_some();
        let color = if set { accent() } else { muted() };
        // Leave room for the brain icon painted over the button's left edge.
        if !editing {
            let button = if compact {
                control(ui, Icon::Brain, "", egui::vec2(34., 33.), set)
            } else {
                let button = ui.add(egui::Button::new(
                    Text::new(format!("      {current}")).size(13.).color(color),
                ));
                draw(
                    ui,
                    Icon::Brain,
                    egui::Rect::from_center_size(
                        button.rect.left_center() + egui::vec2(17., 0.),
                        egui::vec2(17., 17.),
                    ),
                    color,
                );
                button
            };
            let button = button.on_hover_text("How much this model reasons before answering. More thinking is slower and uses more tokens. Applies to this model in every chat.");
            if button.clicked() {
                self.controls_panel = Some(ControlsPanel::Thinking);
            }
            return;
        }
        let index = selected(&tuning);
        ui.horizontal(|ui| {
            ui.label(
                Text::new(index.map_or(auto.clone(), |i| steps[i].0.clone()))
                    .strong()
                    .size(16.),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        index.is_some(),
                        egui::Button::new(Text::new("Reset").size(12.)).small(),
                    )
                    .on_hover_text("Use the model's default")
                    .clicked()
                {
                    tuning.thinking = Thinking::Default;
                    tuning.think_value = None;
                }
            });
        });
        ui.add_space(2.);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(200., 26.), egui::Sense::click_and_drag());
        let response = response.on_hover_text(
            "More thinking is slower and uses more tokens. Applies to this model in every chat.",
        );
        let track = egui::Rect::from_center_size(rect.center(), egui::vec2(200., 14.));
        let inset = 8.;
        let last = steps.len().saturating_sub(1).max(1) as f32;
        let x_of = |i: usize| track.left() + inset + (track.width() - 2. * inset) * i as f32 / last;
        let painter = ui.painter();
        painter.rect_filled(track, 7., raised());
        if let Some(i) = index {
            let fill = egui::Rect::from_min_max(
                track.left_top(),
                egui::pos2(x_of(i) + inset, track.bottom()),
            );
            painter.rect_filled(fill, 7., accent());
        }
        for i in 0..steps.len() {
            let lit = index.is_some_and(|k| i < k);
            painter.circle_filled(
                egui::pos2(x_of(i), track.center().y),
                2.5,
                if lit {
                    Color::from_white_alpha(170)
                } else {
                    muted()
                },
            );
        }
        if let Some(i) = index {
            painter.circle_filled(egui::pos2(x_of(i), track.center().y), 10., Color::WHITE);
        }
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let step = if steps.len() > 1 {
                    ((pos.x - x_of(0)) / (x_of(1) - x_of(0)))
                        .round()
                        .clamp(0., last) as usize
                } else {
                    0
                };
                match &steps[step].1 {
                    Step::Value(v) => {
                        tuning.think_value = Some(v.clone());
                        tuning.thinking = Thinking::Default;
                    }
                    Step::Level(l) => {
                        tuning.thinking = *l;
                        tuning.think_value = None;
                    }
                }
            }
        }
        if (tuning.thinking, tuning.think_value.clone()) != before {
            let _ = self.tx.send(format!(
                "/model-tuning {}",
                serde_json::to_string(&(model, tuning)).expect("Serializable tuning")
            ));
        }
    }
}

impl Desktop {
    /// Small icons for what a model accepts beyond text: images and audio.
    pub(super) fn capability_icons(&self, ui: &mut egui::Ui, model: &str, size: f32, brain: bool) {
        for (has, icon, tip) in [
            (
                self.view.vision.contains(model),
                Icon::Eye,
                "Accepts images",
            ),
            (self.view.audio.contains(model), Icon::Wave, "Accepts audio"),
            (
                brain && self.view.reasoning.contains(model),
                Icon::Brain,
                "Can reason before answering",
            ),
        ] {
            if has {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(size + 2., size + 2.), egui::Sense::hover());
                draw(ui, icon, rect.shrink(1.), muted());
                response.on_hover_text(tip);
            }
        }
    }
}

/// A small progress ring with a glyph in the middle. Returns the clickable response.
fn usage_ring(
    ui: &mut egui::Ui,
    fill: f32,
    color: Color,
    glyph: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26., 26.), egui::Sense::click());
    let painter = ui.painter();
    let (center, radius) = (rect.center(), 10.);
    painter.circle_stroke(center, radius, egui::Stroke::new(2.5_f32, line()));
    if fill > 0. {
        let steps = ((fill * 60.).ceil() as usize).max(2);
        let points = (0..=steps)
            .map(|i| {
                let a = (-90. + 360. * fill * i as f32 / steps as f32).to_radians();
                center + egui::vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(2.5_f32, color)));
    }
    glyph(painter, rect);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Accent while there's room, amber from 75%, red from 90%.
fn ring_color(fill: f32, warn: bool) -> Color {
    if fill >= 0.9 {
        Color::from_rgb(230, 92, 92)
    } else if fill >= 0.75 || warn {
        Color::from_rgb(232, 178, 64)
    } else {
        accent()
    }
}

fn usage_bar(ui: &mut egui::Ui, fill: f32, color: Color) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3, raised());
    let mut done = rect;
    done.set_width(rect.width() * fill.clamp(0., 1.));
    ui.painter().rect_filled(done, 3, color);
}

impl Desktop {
    /// Rings beside the composer for the current model: plan windows and the API budget.
    pub(super) fn usage_rings(&mut self, ui: &mut egui::Ui) {
        self.composer_usage(ui, None);
    }

    fn composer_usage(&mut self, ui: &mut egui::Ui, panel: Option<ControlsPanel>) {
        let model = self.view.state.model.clone();
        let (scope, tag) = route(&model);
        let scope = scope.to_owned();
        let windows = crate::meter::windows(&self.view.state, &model);
        if panel == Some(ControlsPanel::PlanUsage) {
            for w in &windows {
                ui.label(format!("{}  {:.0}%", w.name, w.used * 100.));
                usage_bar(ui, w.used, ring_color(w.used, false));
                if let Some(t) = w.reset {
                    ui.weak(format!("Resets {}", crate::automation::local_time(t)));
                }
                ui.add_space(6.);
            }
            if ui.link("Usage limits…").clicked() {
                self.tasks.tab = 2;
                self.page = Page::Automations;
                self.controls_panel = None;
            }
            return;
        }
        if panel.is_none()
            && let Some(top) = windows.first()
        {
            let fill = top.used;
            let ring = usage_ring(ui, fill, ring_color(fill, false), |p, r| {
                draw_with(p, Icon::Cycle, r.shrink(7.5), muted());
            });
            if ring.on_hover_text("Show plan usage").clicked() {
                self.controls_panel = Some(ControlsPanel::PlanUsage);
            }
        }
        let Some(provider) = self
            .view
            .state
            .providers
            .iter()
            .find(|p| p.id == scope && p.metered())
            .cloned()
        else {
            return;
        };
        let spend = crate::meter::spend(&self.view.state, &scope, crate::automation::now());
        let fill = spend.fill();
        let warn = spend.credit_limited();
        let color = ring_color(fill, warn);
        let priced = self
            .view
            .state
            .tuning
            .get(&model)
            .is_some_and(|t| t.price_in.is_some() && t.price_out.is_some())
            || provider.price(tag).is_some();
        if panel.is_none() {
            let ring = usage_ring(ui, fill, color, |p, r| {
                p.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    "$",
                    egui::FontId::proportional(11.),
                    muted(),
                );
            });
            if ring
                .on_hover_text(format!(
                    "{}: ${:.2} of ${:.2} · ${:.2} left",
                    spend.budget.period.this(),
                    spend.spent,
                    spend.budget.dollars,
                    spend.available()
                ))
                .clicked()
            {
                self.controls_panel = Some(ControlsPanel::Budget);
            }
            return;
        }
        let mut changed = None;
        ui.label(
            Text::new(format!("API budget · {}", provider.name))
                .strong()
                .size(15.),
        );
        ui.add_space(4.);
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: ${:.2} of ${:.2}",
                spend.budget.period.this(),
                spend.spent,
                spend.budget.dollars
            ));
        });
        usage_bar(ui, fill, color);
        ui.weak(format!(
            "Resets {}",
            crate::automation::local_time(spend.reset)
        ));
        if let Some(b) = spend.balance {
            ui.label(format!("Credits left: ${b:.2}"));
            if warn {
                ui.label(
                    Text::new(format!(
                        "Credits cover only ${:.2} of this budget.",
                        spend.available()
                    ))
                    .color(Color::from_rgb(232, 178, 64)),
                );
            }
        }
        if !priced {
            ui.label(
                        Text::new("This model's price is unknown, so its use isn't counted. Set it in Settings → Models → Advanced.")
                            .size(12.)
                            .color(muted()),
                    );
        }
        ui.separator();
        let mut budget = spend.budget;
        ui.horizontal(|ui| {
            ui.label("Budget");
            let id = ui.make_persistent_id(("budget-edit", &scope));
            let mut dollars = ui.data_mut(|d| *d.get_temp_mut_or(id, budget.dollars));
            let r = ui.add(
                egui::DragValue::new(&mut dollars)
                    .range(0.0..=1_000_000.)
                    .speed(1.)
                    .prefix("$"),
            );
            ui.data_mut(|d| d.insert_temp(id, dollars));
            if (r.drag_stopped() || r.lost_focus()) && dollars != budget.dollars {
                budget.dollars = dollars;
                changed = Some(budget);
            }
            crate::widgets::InlineChoice::from_id_salt(("budget-period", &scope))
                .selected_text(budget.period.label())
                .show_ui(ui, |ui| {
                    for p in crate::meter::Period::ALL {
                        if ui.selectable_label(budget.period == p, p.label()).clicked() {
                            budget.period = p;
                            changed = Some(budget);
                        }
                    }
                });
        });
        if let Some(budget) = changed {
            let _ = self.tx.send(format!(
                "/budget {}",
                serde_json::to_string(&(scope, budget)).expect("Serializable budget")
            ));
        }
    }
}
