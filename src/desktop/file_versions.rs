use super::*;
use crate::file_history::{Comparison, History, label};

pub(super) struct Pane {
    history: History,
    versions: Vec<(String, String)>,
    selected: Option<String>,
    comparison: Option<Comparison>,
    confirming: bool,
    restored: bool,
    just_restored: bool,
    pub(super) status: String,
}

impl Pane {
    pub(super) fn open(path: &Path) -> anyhow::Result<Self> {
        let history = History::for_path(path)?;
        let mut pane = Self {
            history,
            versions: vec![],
            selected: None,
            comparison: None,
            confirming: false,
            restored: false,
            just_restored: false,
            status: String::new(),
        };
        if let Err(error) = pane.refresh() {
            pane.status = format!("{error:#}");
        }
        Ok(pane)
    }

    fn refresh(&mut self) -> anyhow::Result<()> {
        self.versions = self
            .history
            .versions()?
            .into_iter()
            .map(|id| {
                let title = label(&id);
                (id, title)
            })
            .collect();
        let selection = self
            .selected
            .clone()
            .filter(|id| self.versions.iter().any(|(listed, _)| listed == id))
            .or_else(|| self.versions.first().map(|(id, _)| id.clone()));
        if let Some(id) = selection {
            self.select(id)?;
        } else {
            self.selected = None;
            self.comparison = None;
        }
        Ok(())
    }

    fn select(&mut self, id: String) -> anyhow::Result<()> {
        self.confirming = false;
        self.comparison = None;
        self.selected = Some(id.clone());
        self.comparison = Some(self.history.compare(&id)?);
        Ok(())
    }

    pub(super) fn restored_path(self) -> Option<PathBuf> {
        self.restored.then_some(self.history.path)
    }

    pub(super) fn take_restored(&mut self) -> Option<&Path> {
        std::mem::take(&mut self.just_restored).then_some(self.history.path.as_path())
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, can_restore: bool) -> bool {
        let mut close = false;
        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            close = crate::widgets::close_button(ui, "Close file history").clicked();
            ui.label(Text::new("File history").strong());
            refresh = ui.small_button("Refresh").clicked();
        });
        let name = self
            .history
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        ui.add(egui::Label::new(&name).truncate())
            .on_hover_text(crate::projects::display_path(&self.history.path));
        ui.separator();
        if refresh {
            self.status = self
                .refresh()
                .err()
                .map_or_else(String::new, |e| format!("{e:#}"));
        }
        if self.versions.is_empty() {
            ui.weak("Previous versions appear here after Spindle changes this file.");
        }
        let mut select = None;
        egui::ScrollArea::vertical()
            .id_salt("saved-file-versions")
            .max_height(130.)
            .show(ui, |ui| {
                for (id, title) in &self.versions {
                    if ui
                        .selectable_label(self.selected.as_ref() == Some(id), title)
                        .clicked()
                    {
                        select = Some(id.clone());
                    }
                }
            });
        if let Some(id) = select {
            self.status = self
                .select(id)
                .err()
                .map_or_else(String::new, |e| format!("{e:#}"));
        }
        if let (Some(id), Some(comparison)) = (&self.selected, &self.comparison) {
            ui.label(Text::new("Saved version → current file").strong());
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        can_restore && !comparison.matches_current,
                        egui::Button::new("Restore this version"),
                    )
                    .on_disabled_hover_text(if can_restore {
                        "This version is already current"
                    } else {
                        "Wait for the current task to finish"
                    })
                    .clicked()
                {
                    self.confirming = true;
                }
                if let Some(diff) = comparison.diff.as_ref().filter(|diff| !diff.is_empty()) {
                    if ui.small_button("Copy diff").clicked() {
                        ui.ctx().copy_text(diff.clone());
                    }
                    if ui.small_button("Save .diff…").clicked() {
                        self.status = match super_download(&format!("{name}-{id}.diff"), None, diff)
                        {
                            Ok(Some(path)) => {
                                format!("Saved {}", crate::projects::display_path(&path))
                            }
                            Ok(None) => String::new(),
                            Err(e) => format!("Could not save the diff: {e:#}"),
                        };
                    }
                }
            });
            let mut restore = false;
            if self.confirming {
                ui.label("Restore the selected version? Spindle saves the current contents first.");
                ui.horizontal_wrapped(|ui| {
                    restore = ui
                        .add_enabled(can_restore, egui::Button::new("Restore"))
                        .clicked();
                    if ui.button("Cancel").clicked() {
                        self.confirming = false;
                    }
                });
            }
            if restore {
                self.confirming = false;
                self.status = match self.history.restore(id, comparison) {
                    Ok(()) => {
                        self.restored = true;
                        self.just_restored = true;
                        self.refresh().map_or_else(
                            |e| format!("Restored. Could not refresh: {e:#}"),
                            |()| "Version restored".into(),
                        )
                    }
                    Err(e) => format!("{e:#}"),
                };
            }
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        if let Some(comparison) = &self.comparison {
            let description = comparison.description();
            let end = description.floor_char_boundary(description.len().min(24_000));
            egui::ScrollArea::both()
                .id_salt("file-version-diff")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(egui::Label::new(Text::new(&description[..end]).monospace()).extend());
                    if end < description.len() {
                        ui.weak("Copy or save the complete diff.");
                    }
                });
        }
        close
    }
}
