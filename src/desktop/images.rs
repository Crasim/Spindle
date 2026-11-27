//! Settings › Models › Image generation

use super::*;
use crate::comfy::{ASPECT_RATIOS, Backend, Catalog, Workflow};
use anyhow::{Result, ensure};
use std::sync::mpsc;

#[derive(Default)]
pub(super) struct ImagesForm {
    initialized: bool,
    url: String,
    workflow: String,
    edit_workflow: String,
    backend: Backend,
    stable_diffusion: String,
    checkpoint: String,
    aspect_ratio: String,
    transparent: bool,
    profiles: Vec<WorkflowForm>,
    default_profile: String,
    catalog: Option<Catalog>,
    checking: Option<mpsc::Receiver<Result<Catalog>>>,
    status: String,
}

#[derive(Default)]
struct WorkflowForm {
    value: Workflow,
    options: String,
}

impl From<&Workflow> for WorkflowForm {
    fn from(value: &Workflow) -> Self {
        Self {
            value: value.clone(),
            options: serde_json::to_string_pretty(&value.parameters).unwrap_or_default(),
        }
    }
}

impl ImagesForm {
    pub fn show(&mut self, ui: &mut egui::Ui, prefs: &crate::state::Preferences) -> Option<String> {
        if !self.initialized {
            self.initialized = true;
            self.url.clone_from(&prefs.comfyui);
            self.workflow.clone_from(&prefs.comfy_workflow);
            self.edit_workflow.clone_from(&prefs.comfy_edit_workflow);
            self.backend = prefs.image_backend;
            self.stable_diffusion.clone_from(&prefs.sd_webui);
            self.checkpoint.clone_from(&prefs.comfy_checkpoint);
            self.aspect_ratio.clone_from(&prefs.image_aspect_ratio);
            self.transparent = prefs.image_transparent;
            self.profiles = prefs
                .comfy_profiles
                .iter()
                .map(WorkflowForm::from)
                .collect();
            self.default_profile.clone_from(&prefs.comfy_profile);
            if self.backend == Backend::ComfyUi && !self.url.is_empty() {
                self.scan();
            }
        }
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(Ok(catalog)) => {
                    self.status = format!(
                        "Connected · {} checkpoints · {} diffusion models{}",
                        catalog.checkpoints.len(),
                        catalog.diffusion.len(),
                        if catalog.alpha() {
                            " · Alpha available"
                        } else {
                            ""
                        }
                    );
                    self.catalog = Some(catalog);
                    self.checking = None;
                }
                Ok(Err(e)) => {
                    self.status = e.to_string();
                    self.checking = None;
                }
                Err(mpsc::TryRecvError::Empty) => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => self.checking = None,
            }
        }
        egui::Frame::new().fill(panel()).inner_margin(16).corner_radius(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(Text::new("Image generation").strong().size(18.));
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.backend, Backend::ComfyUi, "ComfyUI");
                ui.selectable_value(&mut self.backend, Backend::StableDiffusion, "Stable Diffusion WebUI");
            });
            if self.backend == Backend::ComfyUi {
                ui.label("ComfyUI address");
                if ui.add(egui::TextEdit::singleline(&mut self.url).hint_text("http://localhost:8188")).changed() {
                    self.catalog = None;
                }
                if ui.add_enabled(self.checking.is_none() && !self.url.trim().is_empty(), egui::Button::new("Refresh models")).clicked() {
                    self.scan();
                }
                if self.checking.is_some() { ui.spinner(); }
                self.model_picker(ui);
            } else {
                ui.label("Stable Diffusion WebUI address");
                ui.add(egui::TextEdit::singleline(&mut self.stable_diffusion).hint_text("http://localhost:7860"));
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Default aspect ratio");
                let label = if self.aspect_ratio.is_empty() { "1:1" } else { &self.aspect_ratio };
                crate::widgets::InlineChoice::from_id_salt("image-aspect-ratio").selected_text(label).width(126.).show_ui(ui, |ui| {
                    for ratio in ASPECT_RATIOS {
                        ui.selectable_value(&mut self.aspect_ratio, ratio.to_owned(), ratio);
                    }
                });
                if self.backend == Backend::ComfyUi {
                    ui.checkbox(&mut self.transparent, "Transparent PNG by default");
                }
            });
            if self.backend == Backend::ComfyUi {
                self.workflows(ui);
            }
            ui.weak("Use /image with options before the prompt, for example /image --aspect 16:9 --count 2 clouds above a mountain.");
            if !self.status.is_empty() {
                ui.weak(&self.status);
            }
            if ui.button("Save image settings").clicked() {
                match self.preferences(prefs) {
                    Ok(updated) => return Some(format!("/preferences {}", serde_json::json!(updated))),
                    Err(error) => self.status = error.to_string(),
                }
            }
            None
        }).inner
    }

    fn scan(&mut self) {
        let (tx, rx) = mpsc::channel();
        let address = super::side_tabs::to_url(&self.url);
        self.url.clone_from(&address);
        self.checking = Some(rx);
        self.status = "Connecting…".into();
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(crate::comfy::catalog(&address)));
            let _ = tx.send(result);
        });
    }

    fn model_picker(&mut self, ui: &mut egui::Ui) {
        ui.label("Default model");
        let label = if self.checkpoint.is_empty() {
            "Automatic"
        } else {
            &self.checkpoint
        };
        crate::widgets::InlineChoice::from_id_salt("comfy-checkpoint")
            .selected_text(label)
            .width(330.)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.checkpoint, String::new(), "Automatic");
                if let Some(models) = &self.catalog {
                    if !models.checkpoints.is_empty() {
                        ui.weak("Checkpoints");
                    }
                    for model in &models.checkpoints {
                        ui.selectable_value(&mut self.checkpoint, model.clone(), model);
                    }
                    if !models.diffusion.is_empty() {
                        ui.weak("Diffusion models");
                    }
                    for model in &models.diffusion {
                        ui.selectable_value(&mut self.checkpoint, model.clone(), model)
                            .on_hover_text(if Catalog::builtin_diffusion(model) {
                                "Built-in workflow available"
                            } else {
                                "Select a workflow that supports this model"
                            });
                    }
                }
            });
    }

    fn workflows(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.);
        ui.label(Text::new("Workflows").strong());
        let label = match self.default_profile.as_str() {
            "" => "Automatic",
            "builtin" => "Built-in",
            name => name,
        };
        crate::widgets::InlineChoice::from_id_salt("default-comfy-workflow")
            .selected_text(label)
            .width(330.)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.default_profile, String::new(), "Automatic");
                ui.selectable_value(&mut self.default_profile, "builtin".to_owned(), "Built-in");
                for form in &self.profiles {
                    let name = form.value.name.trim();
                    if !name.is_empty() {
                        ui.selectable_value(&mut self.default_profile, name.to_owned(), name);
                    }
                }
            });
        ui.weak("Set node inputs to {{model}}, {{prompt}}, {{width}}, {{height}}, {{seed}}, {{count}}, {{transparent}}, or a named option such as {{steps}}. Edit workflows use {{image}} for the source.");
        let mut remove = None;
        for (index, form) in self.profiles.iter_mut().enumerate() {
            let heading = if form.value.name.is_empty() {
                "New workflow"
            } else {
                &form.value.name
            };
            egui::CollapsingHeader::new(heading)
                .id_salt(("image-workflow", index))
                .default_open(form.value.name.is_empty())
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut form.value.name);
                    ui.label("API workflow JSON");
                    ui.text_edit_singleline(&mut form.value.path);
                    #[cfg(windows)]
                    if ui.small_button("Choose JSON file…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ComfyUI workflow", &["json"])
                            .pick_file()
                        {
                            form.value.path = path.to_string_lossy().into_owned();
                        }
                    }
                    ui.checkbox(&mut form.value.edit, "Image edit workflow");
                    ui.label("Custom option defaults (JSON)");
                    ui.add(
                        egui::TextEdit::multiline(&mut form.options)
                            .hint_text("{\"steps\": 16}")
                            .desired_rows(3),
                    );
                    if ui.small_button("Remove workflow").clicked() {
                        remove = Some(index);
                    }
                });
        }
        if let Some(index) = remove {
            if self.profiles[index].value.name == self.default_profile {
                self.default_profile.clear();
            }
            self.profiles.remove(index);
        }
        if ui.button("Add workflow").clicked() {
            self.profiles.push(WorkflowForm::default());
        }
        if !self.workflow.is_empty() || !self.edit_workflow.is_empty() {
            egui::CollapsingHeader::new("Existing workflow paths").show(ui, |ui| {
                ui.label("Generation");
                ui.text_edit_singleline(&mut self.workflow);
                ui.label("Editing");
                ui.text_edit_singleline(&mut self.edit_workflow);
            });
        }
    }

    fn preferences(&self, prefs: &crate::state::Preferences) -> Result<crate::state::Preferences> {
        let mut updated = prefs.clone();
        updated.image_backend = self.backend;
        updated.image_aspect_ratio.clone_from(&self.aspect_ratio);
        updated.image_transparent = self.backend == Backend::ComfyUi && self.transparent;
        let address = |s: &str| {
            if s.trim().is_empty() {
                Ok(String::new())
            } else {
                crate::local_servers::address(&super::side_tabs::to_url(s))
            }
        };
        if self.backend == Backend::StableDiffusion {
            updated.sd_webui = address(&self.stable_diffusion)?;
            return Ok(updated);
        }
        updated.comfyui = address(&self.url)?;
        updated.comfy_checkpoint.clone_from(&self.checkpoint);
        updated.comfy_workflow = self.workflow.trim().to_owned();
        updated.comfy_edit_workflow = self.edit_workflow.trim().to_owned();
        let mut profiles = Vec::new();
        for form in &self.profiles {
            let mut profile = form.value.clone();
            profile.name = profile.name.trim().to_owned();
            profile.path = profile.path.trim().to_owned();
            profile.parameters = if form.options.trim().is_empty() {
                Default::default()
            } else {
                serde_json::from_str(&form.options)?
            };
            profile.validate()?;
            ensure!(
                !profiles
                    .iter()
                    .any(|p: &Workflow| p.name.eq_ignore_ascii_case(&profile.name)),
                "Give each workflow a different name"
            );
            profiles.push(profile);
        }
        updated.comfy_profile = if self.default_profile == "builtin"
            || profiles.iter().any(|p| p.name == self.default_profile)
        {
            self.default_profile.clone()
        } else {
            String::new()
        };
        updated.comfy_profiles = profiles;
        Ok(updated)
    }
}
