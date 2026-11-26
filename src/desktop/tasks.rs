use super::*;
use crate::automation::{FIVE_HOURS, Rule, WEEK, local_time, now, parse_local};

pub(super) struct TaskScreen {
    pub(super) tab: usize,
    title: String,
    prompt: String,
    when: String,
    model: String,
    repeat: i64,
    custom: i64,
    fallback: String,
    scope: String,
    window: i64,
    cap: u32,
    percent: u32,
    used: u32,
    reset: String,
    pause: bool,
}
impl Default for TaskScreen {
    fn default() -> Self {
        Self {
            tab: 0,
            title: String::new(),
            prompt: String::new(),
            when: local_time(now() + 3600),
            model: String::new(),
            repeat: 0,
            custom: 60,
            fallback: String::new(),
            scope: "ollama-cloud".into(),
            window: FIVE_HOURS,
            cap: 100,
            percent: 90,
            used: 0,
            reset: local_time(now() + FIVE_HOURS),
            pause: false,
        }
    }
}
impl TaskScreen {
    pub(super) fn clear_saved_form(&mut self) {
        self.title.clear();
        self.prompt.clear();
    }
}
fn select(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[(String, String)]) {
    let current = options
        .iter()
        .find(|(id, _)| id == value)
        .map(|(_, v)| v.as_str())
        .unwrap_or("Choose…");
    crate::widgets::InlineChoice::from_label(label)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (id, name) in options {
                ui.selectable_value(value, id.clone(), name);
            }
        });
}
fn subtitle(ui: &mut egui::Ui, s: &str) {
    ui.label(Text::new(s).color(muted()).size(14.));
}
fn tile(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(panel())
        .inner_margin(14)
        .corner_radius(9)
        .stroke(egui::Stroke::new(1_f32, line()))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}
impl Desktop {
    pub(super) fn automations_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (i, label) in ["Schedules", "Model fallback", "Usage limits", "Activity"]
                .iter()
                .enumerate()
            {
                ui.selectable_value(&mut self.tasks.tab, i, *label);
            }
        });
        ui.separator();
        let options = self
            .view
            .models
            .iter()
            .map(|m| {
                let (p, tag) = route(m);
                (m.clone(), format!("{}  /  {tag}", self.provider_name(p)))
            })
            .collect::<Vec<_>>();
        if self.tasks.model.is_empty() {
            self.tasks.model = self.view.state.model.clone();
        }
        match self.tasks.tab {
            0 => self.schedule_controls(ui, &options),
            1 => self.fallback_controls(ui, &options),
            2 => self.usage_controls(ui, &options),
            _ => {
                ui.heading("Recent activity");
                if self.view.state.automation.events.is_empty() {
                    subtitle(
                        ui,
                        "Scheduled runs, quota holds and model changes appear here.",
                    );
                }
                for item in self.view.state.automation.events.iter().rev() {
                    ui.label(item);
                    ui.separator();
                }
            }
        }
        if self.notice.starts_with("Error:") {
            ui.colored_label(accent(), &self.notice);
        }
    }
    fn schedule_controls(&mut self, ui: &mut egui::Ui, options: &[(String, String)]) {
        let auto = self.view.state.automation.clone();
        if let Some(w) = &auto.wake {
            tile(ui, |ui| {
                ui.strong(format!("Task #{} resumes {}", w.task, local_time(w.at)));
                subtitle(ui, &w.reason);
            });
        }
        for s in auto.schedules.iter() {
            tile(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&s.title);
                    ui.label(if s.enabled { "Scheduled" } else { "Inactive" });
                });
                let repeat = if s.interval == 0 {
                    "Once".into()
                } else {
                    format!("Every {} minutes", s.interval / 60)
                };
                subtitle(
                    ui,
                    &format!("{repeat} · {} · Chat #{}", local_time(s.next), s.chat),
                );
                egui::CollapsingHeader::new("Task details")
                    .id_salt(s.id)
                    .show(ui, |ui| {
                        if let Some((name, _)) = &s.skill {
                            ui.monospace(format!("/{name}"));
                        }
                        ui.label(&s.prompt);
                        ui.monospace(
                            std::iter::once(&s.model)
                                .chain(&s.chain)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" → "),
                        );
                    });
                ui.horizontal_wrapped(|ui| {
                    if (s.enabled || s.interval > 0 || s.next > now())
                        && ui
                            .small_button(if s.enabled { "Pause" } else { "Enable" })
                            .clicked()
                    {
                        self.send(format!(
                            "/automation {} {}",
                            if s.enabled { "pause" } else { "enable" },
                            s.id
                        ));
                    }
                    if ui.small_button("Remove schedule").clicked() {
                        self.send(format!("/automation remove {}", s.id));
                    }
                    if self.view.state.chat_id != s.chat && ui.small_button("Open chat").clicked() {
                        self.send(format!("/open {}", s.chat));
                        self.page = Page::Chat;
                    }
                });
            });
        }
        ui.heading("Schedule a task");
        subtitle(
            ui,
            "Each run reads the latest history of this chat and saves its answer here.",
        );
        self.android_ime.field(ui, "schedule-name", &mut self.tasks.title, Default::default(), |edit| {
            edit.desired_width(f32::INFINITY).hint_text("Task name")
        });
        self.android_ime.multiline(ui, "schedule-prompt", &mut self.tasks.prompt, |edit| {
            edit
                .desired_width(f32::INFINITY)
                .desired_rows(3)
                .hint_text("What should the model do when this task runs?")
        });
        select(ui, "Model", &mut self.tasks.model, options);
        ui.horizontal_wrapped(|ui| {
            ui.label("First run · local time");
            self.android_ime.field(ui, "schedule-start", &mut self.tasks.when, Default::default(), |edit| {
                edit.desired_width(178.)
            });
        });
        ui.horizontal_wrapped(|ui| {
            let choices = [
                (0, "Once"),
                (3600, "Hourly"),
                (86400, "Daily"),
                (WEEK, "Weekly"),
                (-1, "Custom"),
            ];
            for (n, label) in choices {
                ui.selectable_value(&mut self.tasks.repeat, n, label);
            }
            if self.tasks.repeat < 0 {
                ui.add(
                    egui::DragValue::new(&mut self.tasks.custom)
                        .range(1..=525600)
                        .suffix(" min"),
                );
            }
        });
        if ui.button("Schedule in this chat").clicked() {
            match parse_local(&self.tasks.when) {
                Ok(at) => {
                    let interval = if self.tasks.repeat < 0 {
                        self.tasks.custom * 60
                    } else {
                        self.tasks.repeat
                    };
                    self.send(format!(
                        "/automation add {}",
                        serde_json::json!([
                            self.tasks.title,
                            self.tasks.prompt,
                            self.tasks.model,
                            at,
                            interval
                        ])
                    ));
                }
                Err(e) => self.notice = format!("Error: {e}"),
            };
        }
        if !self.view.state.queue.is_empty()
            && ui.button("Defer current task until this time").clicked()
        {
            self.send(format!("/defer {}", self.tasks.when));
        }
        ui.separator();
        if cfg!(not(target_os = "android")) {
            let mut background = auto.background;
            if ui.checkbox(&mut background, "Run due tasks while Spindle is closed").changed() {
                self.send(format!("/automation background {}", if background { "on" } else { "off" }));
            }
            subtitle(ui, "Due tasks are checked while you’re signed in. When the computer wakes, missed repeats run once.");
        } else {
            subtitle(ui, "Scheduled tasks run while Spindle is open.");
        }
    }
    fn fallback_controls(&mut self, ui: &mut egui::Ui, options: &[(String, String)]) {
        ui.heading("Fallback order");
        subtitle(
            ui,
            "When a model reaches a usage limit, Spindle checks this list in order. Each listed model may receive the task and its chat history.",
        );
        let mut enabled = self.view.state.automation.fallback_on_disconnect;
        if ui.checkbox(&mut enabled, "Also use fallbacks when the provider disconnects or times out").changed() {
            self.send(format!("/fallback disconnects {}", if enabled { "on" } else { "off" }));
        }
        let chain = self.view.state.automation.chain.clone();
        let mut changed = None;
        for (i, m) in chain.iter().enumerate() {
            tile(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!(
                        "{}. {}",
                        i + 1,
                        options
                            .iter()
                            .find(|(id, _)| id == m)
                            .map(|(_, name)| name)
                            .unwrap_or(m)
                    ));
                    for (label, allowed, offset) in [
                        ("Earlier", i > 0, -1isize),
                        ("Later", i + 1 < chain.len(), 1),
                    ] {
                        if ui.add_enabled(allowed, egui::Button::new(label)).clicked() {
                            let mut c = chain.clone();
                            c.swap(i, (i as isize + offset) as usize);
                            changed = Some(c);
                        }
                    }
                    if ui.small_button("Remove").clicked() {
                        let mut c = chain.clone();
                        c.remove(i);
                        changed = Some(c);
                    }
                });
            });
        }
        let available = options
            .iter()
            .filter(|(id, _)| !chain.contains(id))
            .cloned()
            .collect::<Vec<_>>();
        if !available.iter().any(|(id, _)| id == &self.tasks.fallback) {
            self.tasks.fallback = available.first().map(|v| v.0.clone()).unwrap_or_default();
        }
        select(ui, "Fallback model", &mut self.tasks.fallback, &available);
        if ui
            .add_enabled(
                !self.tasks.fallback.is_empty(),
                egui::Button::new("Add to order"),
            )
            .clicked()
        {
            let mut c = chain;
            c.push(self.tasks.fallback.clone());
            changed = Some(c);
        }
        if let Some(list) = changed {
            self.send(format!("/fallback set {}", list.join(" ")));
        }
        subtitle(
            ui,
            "Changes apply to new tasks and schedules. Council and swarm members keep their assigned models.",
        );
    }
    fn usage_controls(&mut self, ui: &mut egui::Ui, models: &[(String, String)]) {
        let auto = self.view.state.automation.clone();
        ui.horizontal(|ui| {
            ui.heading("Ollama Cloud usage");
            if ui.small_button("Refresh").clicked() {
                self.send("/usage");
            }
        });
        let cloud = &auto.cloud_usage;
        if cloud.checked > 0 {
            ui.weak(format!(
                "Account totals via local Ollama · {}",
                local_time(cloud.checked)
            ));
        }
        if let Some(n) = cloud.requests {
            ui.label(format!("{n} requests over the last 7 days"));
        }
        if let Some(n) = cloud.input {
            ui.label(format!("{n} reported account input tokens"));
        }
        if let Some(n) = cloud.output {
            ui.label(format!("{n} reported account output tokens"));
        }
        for window in &cloud.windows {
            ui.label(format!("{} · {:.2}% used", window.name, window.used));
            ui.add(egui::ProgressBar::new((window.used / 100.) as f32).desired_width(230.));
            if let Some(reset) = window.reset {
                ui.weak(format!("Resets {}", local_time(reset)));
            }
        }
        if let Some(n) = cloud.included {
            ui.label(format!("Included credit remaining  {n:.2} USD"));
        }
        if let Some(n) = cloud.purchased {
            ui.label(format!("Purchased credit balance  {n:.2} USD"));
        }
        for w in &cloud.warnings {
            ui.weak(w);
        }
        if cloud.checked == 0 && cloud.warnings.is_empty() {
            ui.weak("Fetching account totals…");
        }
        for (scope, (at, values)) in &auto.provider_stats {
            ui.strong(self.provider_name(scope));
            ui.weak(format!("Last response · {}", local_time(*at)));
            for value in values {
                ui.label(value);
            }
        }
        ui.separator();
        ui.heading("Provider limits");
        ui.horizontal_wrapped(|ui| {
            let mut percentage = auto.reported_percent;
            ui.label("Hold a provider at");
            if ui
                .add(
                    egui::DragValue::new(&mut percentage)
                        .range(1..=100)
                        .suffix("%"),
                )
                .changed()
            {
                self.send(format!("/quota threshold {percentage}"));
            }
            ui.label("of its reported quota");
        });
        ui.hyperlink_to(
            "ChatGPT account usage",
            "https://chatgpt.com/settings/usage",
        );
        for (p, l) in &auto.limits {
            tile(ui, |ui| {
                ui.strong(self.provider_name(p));
                ui.label(&l.reason);
                ui.label(
                    l.reset
                        .map(local_time)
                        .unwrap_or_else(|| "Reset time unavailable".into()),
                );
                if ui.small_button("Clear provider hold").clicked() {
                    self.send(format!("/quota clear {p}"));
                }
            });
        }
        ui.separator();
        ui.heading("Workspace request budgets");
        subtitle(
            ui,
            "Set a five-hour or weekly allowance for a model or provider. Each dispatch counts once. Other apps’ requests are outside these counters.",
        );
        for r in &auto.rules {
            tile(ui, |ui| {
                ui.strong(format!(
                    "{} · {}",
                    r.scope,
                    if r.period == WEEK {
                        "Weekly"
                    } else {
                        "Five hours"
                    }
                ));
                ui.label(format!(
                    "{} / {} attempts · {}% threshold · resets {}",
                    r.used,
                    r.cap,
                    r.percent,
                    local_time(r.reset)
                ));
                ui.horizontal_wrapped(|ui| {
                    ui.label(if r.pause {
                        "Pause the task"
                    } else {
                        "Try fallback"
                    });
                    if ui.small_button("Edit").clicked() {
                        self.tasks.scope = r.scope.clone();
                        self.tasks.window = r.period;
                        self.tasks.reset = local_time(r.reset);
                        self.tasks.cap = r.cap;
                        self.tasks.used = r.used;
                        self.tasks.percent = r.percent;
                        self.tasks.pause = r.pause;
                    }
                    if ui.small_button("Remove").clicked() {
                        self.send(format!("/quota remove {} {}", r.scope, r.period));
                    }
                });
            });
        }
        let mut scopes = vec![];
        for (id, _) in models {
            let p = route(id).0;
            if !scopes.iter().any(|(v, _)| v == p) {
                scopes.push((
                    p.to_string(),
                    format!("Provider · {}", self.provider_name(p)),
                ));
            }
        }
        scopes.extend_from_slice(models);
        select(ui, "Budget scope", &mut self.tasks.scope, &scopes);
        ui.horizontal_wrapped(|ui| {
            for (n, s) in [(FIVE_HOURS, "Five hours"), (WEEK, "Weekly")] {
                if ui.selectable_value(&mut self.tasks.window, n, s).changed() {
                    self.tasks.reset = local_time(now() + n);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Allowance");
            ui.add(egui::DragValue::new(&mut self.tasks.cap).range(1..=1_000_000));
            ui.label("Already used");
            ui.add(egui::DragValue::new(&mut self.tasks.used).range(0..=1_000_000));
            ui.label("Threshold");
            ui.add(
                egui::DragValue::new(&mut self.tasks.percent)
                    .range(1..=100)
                    .suffix("%"),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Next reset · local time");
            self.android_ime.field(ui, "usage-reset", &mut self.tasks.reset, Default::default(), |edit| {
                edit.desired_width(178.)
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tasks.pause, false, "Try fallback");
            ui.selectable_value(&mut self.tasks.pause, true, "Pause until reset");
        });
        if ui.button("Save budget").clicked() {
            match parse_local(&self.tasks.reset) {
                Ok(reset) => {
                    let rule = Rule {
                        scope: self.tasks.scope.clone(),
                        period: self.tasks.window,
                        cap: self.tasks.cap,
                        used: self.tasks.used,
                        percent: self.tasks.percent,
                        pause: self.tasks.pause,
                        reset,
                    };
                    self.send(format!(
                        "/quota set {}",
                        serde_json::to_string(&rule).unwrap()
                    ));
                }
                Err(e) => self.notice = format!("Error: {e}"),
            }
        }
        if !auto.meters.is_empty() {
            ui.separator();
            ui.heading("Recorded in this workspace");
            for (m, v) in &auto.meters {
                ui.label(format!(
                    "{m} · {} attempts · {} input / {} output tokens reported",
                    v.attempts, v.input, v.output
                ));
            }
        }
    }
}
