//! Settings › Models › Image generation: where ComfyUI is and what it uses.

use super::*;
use crate::comfy::Backend;
use anyhow::Result;
use std::sync::mpsc;

#[derive(Default)]
pub(super) struct ImagesForm {
    /// The fields as typed, before saving; `None` until first shown.
    url: Option<String>,
    workflow: String,
    edit_workflow: String,
    backend: Backend,
    stable_diffusion: String,
    checkpoint: String,
    models: Vec<String>,
    checking: Option<mpsc::Receiver<Result<Vec<String>>>>,
    status: String,
}

impl ImagesForm {
    /// Draw the card. Returns a /preferences command when the user saves.
    pub fn show(&mut self, ui: &mut egui::Ui, prefs: &crate::state::Preferences) -> Option<String> {
        if self.url.is_none() {
            self.url = Some(prefs.comfyui.clone());
            self.workflow = prefs.comfy_workflow.clone();
            self.edit_workflow = prefs.comfy_edit_workflow.clone();
            self.backend = prefs.image_backend;
            self.stable_diffusion = prefs.sd_webui.clone();
            self.checkpoint = prefs.comfy_checkpoint.clone();
        }
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(Ok(models)) => {
                    self.status = format!(
                        "Connected · {} checkpoint{}",
                        models.len(),
                        if models.len() == 1 { "" } else { "s" }
                    );
                    self.models = models;
                    self.checking = None;
                }
                Ok(Err(e)) => {
                    self.status = format!("{e:#}");
                    self.checking = None;
                }
                Err(mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => self.checking = None,
            }
        }
        let mut command = None;
        egui::Frame::new()
            .fill(panel())
            .inner_margin(16)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(Text::new("Image generation").strong().size(18.));
                ui.weak("Use /image to generate a picture. Attach a PNG or JPEG and use /image edit to describe a change. Prompts and edit sources go to the selected server.");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.backend, Backend::ComfyUi, "ComfyUI");
                    ui.selectable_value(&mut self.backend, Backend::StableDiffusion, "Stable Diffusion WebUI");
                });
                ui.add_space(6.);
                if self.backend == Backend::StableDiffusion {
                    ui.label("Stable Diffusion WebUI address");
                    ui.add(egui::TextEdit::singleline(&mut self.stable_diffusion)
                        .id_salt("stable-diffusion-address")
                        .hint_text("http://localhost:7860")
                        .desired_width(ui.available_width().clamp(120., 320.)));
                    ui.weak("Start the server with its API enabled. Spindle uses the checkpoint loaded by the server.");
                    let changed = self.stable_diffusion.trim() != prefs.sd_webui || self.backend != prefs.image_backend;
                    if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
                        match crate::local_servers::address(&super::side_tabs::to_url(&self.stable_diffusion)) {
                            Ok(url) => command = Some(format!("/image sd-webui {url}")),
                            Err(e) => self.status = e.to_string(),
                        }
                    }
                    if !self.status.is_empty() {
                        ui.weak(&self.status);
                    }
                    return;
                }
                let url = self.url.as_mut().unwrap();
                ui.horizontal_wrapped(|ui| {
                    ui.label("ComfyUI address");
                    ui.add(
                        egui::TextEdit::singleline(url)
                        .id_salt("images-field-1")
                            .hint_text("http://localhost:8188")
                            .desired_width((ui.available_width().max(230.)).min(260.)),
                    );
                    let busy = self.checking.is_some();
                    if ui
                        .add_enabled(!busy && !url.trim().is_empty(), egui::Button::new("Test"))
                        .on_hover_text("Connect and list the checkpoints")
                        .clicked()
                    {
                        let (tx, rx) = mpsc::channel();
                        let address = super::side_tabs::to_url(url);
                        *url = address.clone();
                        std::thread::spawn(move || {
                            let result = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(anyhow::Error::from)
                                .and_then(|rt| rt.block_on(crate::comfy::checkpoints(&address)));
                            let _ = tx.send(result);
                        });
                        self.checking = Some(rx);
                        self.status = "Connecting…".into();
                    }
                    if busy {
                        ui.spinner();
                    }
                });
                if !self.status.is_empty() {
                    ui.weak(&self.status);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("Checkpoint");
                    let shown = if self.checkpoint.is_empty() {
                        "Pick automatically (SDXL)".to_owned()
                    } else {
                        self.checkpoint.clone()
                    };
                    crate::widgets::InlineChoice::from_id_salt("comfy-checkpoint")
                        .selected_text(shown)
                        .width(300.)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.checkpoint,
                                String::new(),
                                "Pick automatically (SDXL)",
                            );
                            for m in &self.models {
                                ui.selectable_value(&mut self.checkpoint, m.clone(), m);
                            }
                        });
                });
                if self.models.is_empty() {
                    ui.weak("Press Test to list the checkpoints ComfyUI has.");
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("Generation workflow");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.workflow)
                        .id_salt("images-field-2")
                            .hint_text("Workflow from Export (API)")
                            .desired_width((ui.available_width().max(230.)).min(300.)),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Edit workflow");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.edit_workflow)
                            .id_salt("images-edit-workflow")
                            .hint_text("Workflow from Export (API)")
                            .desired_width(ui.available_width().clamp(230., 300.)),
                    );
                });
                ui.weak("Blank paths use the built-in SDXL workflows. Custom API workflows can use {{prompt}}, {{negative}}, {{seed}}, {{width}} and {{height}}. Put {{image}} in the custom edit workflow's source field.");
                ui.add_space(4.);
                let url = self.url.as_deref().unwrap_or("").trim().to_owned();
                let changed = url != prefs.comfyui
                    || self.backend != prefs.image_backend
                    || self.workflow.trim() != prefs.comfy_workflow
                    || self.edit_workflow.trim() != prefs.comfy_edit_workflow
                    || self.checkpoint != prefs.comfy_checkpoint;
                if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
                    let mut p = prefs.clone();
                    p.comfyui = match url.is_empty() {
                        true => String::new(),
                        false => match crate::local_servers::address(&super::side_tabs::to_url(&url)) {
                            Ok(url) => url,
                            Err(e) => {
                                self.status = e.to_string();
                                return;
                            }
                        },
                    };
                    p.image_backend = self.backend;
                    p.comfy_workflow = self.workflow.trim().to_owned();
                    p.comfy_edit_workflow = self.edit_workflow.trim().to_owned();
                    p.comfy_checkpoint = self.checkpoint.clone();
                    self.url = Some(p.comfyui.clone());
                    command = Some(format!(
                        "/preferences {}",
                        serde_json::to_string(&p).unwrap()
                    ));
                }
            });
        command
    }
}
