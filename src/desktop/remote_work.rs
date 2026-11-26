use super::*;
use crate::sync::placement::{Kind, Scope, Target, target_label};

impl Desktop {
    pub(super) fn run_on_button(&mut self, ui: &mut egui::Ui, compact: bool) {
        let state = &self.view.state;
        let scope = Scope::Chat(state.chat_id);
        let targets = Kind::OPERATIONS.map(|kind| state.placement.effective(state, &scope, kind));
        let first = targets[0];
        let label = if targets.iter().all(|target| {
            *target == first
                || matches!(
                    (target, first),
                    (None, Some(Target::Local)) | (Some(Target::Local), None)
                )
        }) {
            target_label(state, first)
        } else {
            "Multiple devices".into()
        };
        let title = format!("Run on · {label}");
        let clicked = if compact {
            small(ui, Icon::Tool, &title)
        } else {
            ui.small_button(&title).clicked()
        };
        if clicked {
            self.execution_panel = Some(scope);
        }
    }

    pub(super) fn render_execution_panel(&mut self, ui: &mut egui::Ui, scope: Scope) {
        let project = matches!(scope, Scope::Project(_));
        if super::panel_header(
            ui,
            if project {
                "Project execution"
            } else {
                "Run on"
            },
            "Back",
        ) {
            self.execution_panel = None;
            return;
        }
        let state = &self.view.state;
        let has_project = matches!(&scope, Scope::Chat(id) if state.thread_data(*id).is_some_and(|(thread, _)| thread.project.is_some()));
        let mut change = None;
        let mut inventory = None;
        let mut open_sync = false;
        egui::ScrollArea::vertical().id_salt("execution-routes").show(ui, |ui| {
        ui.weak(if project {
            "Defaults for this project’s conversations on this device. A conversation can override them."
        } else {
            "Choose where new tool requests run. The selected model and chat files stay in this workspace."
        });
        ui.add_space(8.);
        for kind in std::iter::once(Kind::Default).chain(Kind::OPERATIONS) {
            let choice = state.placement.configured(&scope, kind);
            let effective = state.placement.effective(state, &scope, kind);
            let label = target_label(state, effective);
            let inherited = if kind == Kind::Default && has_project { "Use project default" } else { "Inherit" };
            egui::Frame::new().fill(panel()).inner_margin(10).corner_radius(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(kind.label());
                crate::widgets::InlineChoice::from_id_salt(("run-on", kind as u8))
                    .selected_text(if choice.is_some() { label } else { format!("{inherited} · {label}") })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(choice.is_none(), inherited).clicked() { change = Some((kind, None)); }
                        if ui.selectable_label(choice == Some(&Target::Local), "This device").clicked() {
                            change = Some((kind, Some(Target::Local)));
                        }
                        if let Some(sync) = &state.space {
                            for (id, os) in &sync.remote.platforms {
                                if id == &sync.identity.device.id || !kind.supports(os) { continue; }
                                let candidate = Target::Device(id.clone());
                                let label = sync.remote.host_label(id);
                                let label = if sync.source_paused(id) { format!("{label} · Sync paused") } else { label };
                                if ui.selectable_label(choice == Some(&candidate), label).clicked() {
                                    change = Some((kind, Some(candidate)));
                                }
                            }
                        }
                    });
                if let Some(sync) = &state.space
                    && let Some(Target::Device(id)) = effective
                    && sync.remote.platforms.get(id).is_some_and(|os| !kind.supports(os)) {
                    ui.weak("Choose a different device for this operation.");
                }
                if let Some(sync) = &state.space
                    && let Some(Target::Device(id)) = effective
                    && sync.remote.platforms.contains_key(id)
                    && !sync.paused && !sync.source_paused(id)
                    && ui.small_button("Check capabilities").clicked() {
                        inventory = Some(id.clone());
                }
            });
            ui.add_space(6.);
        }
        ui.weak("Remote jobs ask before sending files. On the destination, enable sandboxed jobs for this device in Sync settings.");
        if ui.button("Manage paired devices").clicked() { open_sync = true; }
        });
        if let Some((kind, target)) = change {
            self.send(format!(
                "/run-on {}",
                serde_json::json!([scope, kind, target])
            ));
        }
        if let Some(id) = inventory {
            self.send(format!("/remote inventory {id}"));
            self.execution_panel = None;
            self.page = Page::Queue;
        } else if open_sync {
            self.execution_panel = None;
            self.open_settings();
            self.sync_tab = true;
            self.integration_tab = None;
        }
    }

    pub(super) fn remote_jobs(&mut self, ui: &mut egui::Ui) {
        let Some(sync) = &self.view.state.space else {
            return;
        };
        let jobs = sync.remote.jobs();
        if jobs.is_empty() {
            return;
        }
        let mut export = None;
        ui.add_space(12.);
        ui.heading("Remote jobs");
        for job in jobs.iter().take(24) {
            let header = format!(
                "{} · {} · {}",
                job.label,
                sync.remote.host_label(job.host),
                job.status
            );
            egui::CollapsingHeader::new(header)
                .id_salt(("remote-job", job.id))
                .show(ui, |ui| {
                    ui.weak(format!("Job {}", &job.id[..12]));
                    if let Some(detail) = job.detail {
                        ui.add(egui::Label::new(detail).wrap().selectable(true));
                    }
                    if job.output && ui.button("Save output").clicked() {
                        export = Some(job.id.to_owned());
                    }
                });
        }
        if let Some(id) = export {
            self.send(format!("/remote save {id}"));
        }
    }
}
