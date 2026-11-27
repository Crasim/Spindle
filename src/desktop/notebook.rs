use super::{Page, android_ime::ComposerIme, reader};
use crate::notebook::{self, Library};
use eframe::egui::{
    self, RichText,
    text::{CCursor, CCursorRange},
};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::mpsc,
};

#[derive(Default)]
pub(super) struct Notebook {
    library: Library,
    root: Option<PathBuf>,
    listing: Option<mpsc::Receiver<anyhow::Result<Vec<PathBuf>>>>,
    files: Vec<PathBuf>,
    note: Option<Note>,
    filter: String,
    new_title: Option<String>,
    vault_input: String,
    vault_status: String,
    status: String,
}

struct Note {
    path: PathBuf,
    saved: String,
    text: String,
    preview: bool,
    images: HashMap<String, Option<egui::TextureHandle>>,
}

impl Note {
    fn open(root: &Path, path: PathBuf) -> anyhow::Result<Self> {
        let text = notebook::read(root, &path)?;
        Ok(Self {
            path,
            saved: text.clone(),
            text,
            preview: false,
            images: HashMap::new(),
        })
    }

    fn modified(&self) -> bool {
        self.text != self.saved
    }

    fn save(&mut self, root: &Path) -> anyhow::Result<()> {
        if self.modified() {
            notebook::save(root, &self.path, &self.saved, &self.text)?;
            self.saved.clone_from(&self.text);
        }
        Ok(())
    }
}

impl Notebook {
    fn refresh(&mut self, ctx: &egui::Context) {
        self.listing = None;
        self.files.clear();
        match self.library.open_editor() {
            Ok(root) => {
                if self.root.as_ref().is_some_and(|previous| previous != &root) {
                    if self.note.as_ref().is_some_and(Note::modified) {
                        self.status = "The notes folder changed. Save or discard this draft before switching folders.".into();
                        return;
                    }
                    self.note = None;
                }
                self.root = Some(root.clone());
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(notebook::list(&root));
                    ctx.request_repaint();
                });
                self.listing = Some(rx);
            }
            Err(e) => {
                if !self.note.as_ref().is_some_and(Note::modified) {
                    self.root = None;
                    self.note = None;
                }
                self.status = e.to_string();
            }
        }
    }

    pub(super) fn connector(&mut self, ui: &mut egui::Ui, ime: &mut ComposerIme) {
        let linked = notebook::vault();
        if let Some(root) = &linked {
            ui.label(crate::projects::display_path(root));
        }
        let dirty =
            self.library == Library::Obsidian && self.note.as_ref().is_some_and(Note::modified);
        if dirty {
            ui.weak("Save or discard the open vault note before changing this connection.");
        }
        ui.add_enabled_ui(!dirty, |ui| {
            ime.field(
                ui,
                "vault-folder",
                &mut self.vault_input,
                Default::default(),
                |field| {
                    field
                        .hint_text("Path to an Obsidian vault folder")
                        .desired_width(f32::INFINITY)
                },
            );
            ui.horizontal_wrapped(|ui| {
                #[cfg(windows)]
                if ui.button("Browse…").clicked()
                    && let Some(folder) = rfd::FileDialog::new()
                        .set_title("Choose the Obsidian vault folder")
                        .pick_folder()
                {
                    self.vault_input = crate::projects::display_path(&folder);
                }
                let path = self.vault_input.trim();
                let result = if ui
                    .add_enabled(!path.is_empty(), egui::Button::new("Connect vault"))
                    .clicked()
                {
                    Some((notebook::connect(Path::new(path)), "Vault connected"))
                } else if linked.is_some() && ui.button("Disconnect").clicked() {
                    Some((notebook::disconnect(), "Vault disconnected"))
                } else {
                    None
                };
                if let Some((result, message)) = result {
                    match result {
                        Ok(()) => {
                            self.vault_input.clear();
                            self.vault_status = message.into();
                            self.status.clear();
                            if self.library == Library::Obsidian {
                                self.note = None;
                                self.refresh(ui.ctx());
                            }
                        }
                        Err(e) => self.vault_status = format!("{e:#}"),
                    }
                }
            });
        });
        if !self.vault_status.is_empty() {
            ui.label(&self.vault_status);
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        ime: &mut ComposerIme,
    ) -> Option<(String, String)> {
        if self.root.is_none() && self.library == Library::Spindle && self.status.is_empty() {
            self.refresh(ui.ctx());
        }
        if let Some(result) = self.listing.as_ref().map(mpsc::Receiver::try_recv) {
            match result {
                Ok(result) => {
                    self.listing = None;
                    match result {
                        Ok(files) => self.files = files,
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.listing = None;
                    self.status = "The notes scan stopped. Try Refresh.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let dirty = self.note.as_ref().is_some_and(Note::modified);
        ui.horizontal_wrapped(|ui| {
            for library in [Library::Spindle, Library::Obsidian] {
                if ui
                    .add_enabled(
                        !dirty,
                        egui::Button::new(library.label()).selected(self.library == library),
                    )
                    .clicked()
                    && self.library != library
                {
                    self.library = library;
                    self.note = None;
                    self.new_title = None;
                    self.filter.clear();
                    self.status.clear();
                    self.refresh(ui.ctx());
                }
            }
            if ui.button("Refresh").clicked() {
                self.status.clear();
                if !dirty && let (Some(note), Some(root)) = (&self.note, &self.root) {
                    match Note::open(root, note.path.clone()) {
                        Ok(fresh) => self.note = Some(fresh),
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
                self.refresh(ui.ctx());
            }
        });
        ui.add_space(8.);
        let Some(root) = self.root.clone() else {
            if self.library == Library::Obsidian {
                self.connector(ui, ime);
                if notebook::vault().is_some() && !self.status.is_empty() {
                    ui.label(&self.status);
                }
            } else if !self.status.is_empty() {
                ui.label(&self.status);
            }
            return None;
        };
        let mut attachment = None;
        if let Some(note) = &mut self.note {
            let close = egui::ScrollArea::vertical()
                .id_salt("note-editor-scroll")
                .show(ui, |ui| {
                    let mut close = false;
                    ui.horizontal_wrapped(|ui| {
                        close = ui
                            .add_enabled(!dirty, egui::Button::new("All notes"))
                            .clicked();
                        ui.label(
                            RichText::new(
                                note.path.file_stem().unwrap_or_default().to_string_lossy(),
                            )
                            .strong()
                            .size(20.),
                        );
                    });
                    if close {
                        return true;
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui.selectable_label(!note.preview, "Edit").clicked() {
                            note.preview = false;
                        }
                        if ui.selectable_label(note.preview, "Preview").clicked() {
                            note.preview = true;
                        }
                        if ui.add_enabled(dirty, egui::Button::new("Save")).clicked()
                            || ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                            })
                        {
                            self.status = note
                                .save(&root)
                                .map_or_else(|e| format!("{e:#}"), |()| "Note saved".into());
                        }
                        if ui
                            .add_enabled(!note.modified(), egui::Button::new("Attach to chat"))
                            .on_hover_text(
                                "Attach this saved note to the current chat’s next message",
                            )
                            .clicked()
                        {
                            let name = note
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            attachment = Some((name, note.saved.clone()));
                        }
                        #[cfg(any(windows, target_os = "linux"))]
                        if self.library == Library::Obsidian
                            && ui.button("Open in Obsidian").clicked()
                        {
                            self.status = open_obsidian(&root, &note.path)
                                .err()
                                .map_or_else(String::new, |e| format!("{e:#}"));
                        }
                        if note.modified() && ui.small_button("Discard edits").clicked() {
                            match Note::open(&root, note.path.clone()) {
                                Ok(fresh) => {
                                    *note = fresh;
                                    self.status.clear();
                                }
                                Err(e) => self.status = format!("{e:#}"),
                            }
                        }
                    });
                    if note.modified() {
                        ui.weak("Unsaved edits");
                    }
                    if !self.status.is_empty() {
                        ui.label(&self.status);
                    }
                    ui.separator();
                    if note.preview {
                        let mut markdown = reader::Markdown::note(ui, &note.text);
                        markdown.show(
                            ui,
                            false,
                            &mut Default::default(),
                            &mut |ui, url| {
                                render_image(ui, &root, &note.path, &mut note.images, url)
                            },
                            false,
                        );
                    } else {
                        let key = ("markdown-note", self.library as u8, &note.path);
                        let id = ui.make_persistent_id(key);
                        if let Some(action) = toolbar(ui) {
                            apply(ui, id, &mut note.text, action);
                        }
                        #[cfg(windows)]
                        if ui.small_button("Add image file…").clicked()
                            && let Some(source) = rfd::FileDialog::new()
                                .add_filter("PNG or JPEG", &["png", "jpg", "jpeg"])
                                .pick_file()
                        {
                            match copy_image(&root, &note.path, &source) {
                                Ok(image) => {
                                    apply(ui, id, &mut note.text, Format::ImageFile(image))
                                }
                                Err(e) => self.status = format!("{e:#}"),
                            }
                        }
                        let response = ime.multiline(ui, key, &mut note.text, |field| {
                            field
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(17)
                                .desired_width(f32::INFINITY)
                                .margin(egui::Margin::symmetric(9, 8))
                                .hint_text("Write your note in Markdown…")
                        });
                        if response.changed() {
                            self.status.clear();
                        }
                    }
                    false
                })
                .inner;
            if close {
                self.note = None;
                self.status.clear();
            }
        } else {
            if ui.button("New note").clicked() {
                self.new_title = Some(String::new());
            }
            if let Some(title) = &mut self.new_title {
                let mut create = false;
                let mut cancel = false;
                ime.field(ui, "note-title", title, Default::default(), |field| {
                    field.hint_text("Note name").desired_width(f32::INFINITY)
                });
                ui.horizontal(|ui| {
                    create = ui.button("Create note").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                if create {
                    match notebook::create(&root, title).and_then(|path| Note::open(&root, path)) {
                        Ok(note) => {
                            self.note = Some(note);
                            self.new_title = None;
                            self.status.clear();
                            self.refresh(ui.ctx());
                        }
                        Err(e) => self.status = format!("{e:#}"),
                    }
                } else if cancel {
                    self.new_title = None;
                }
            }
            if !self.status.is_empty() {
                ui.label(&self.status);
            }
            ime.field(
                ui,
                "notes-filter",
                &mut self.filter,
                Default::default(),
                |field| {
                    field
                        .hint_text("Find a note by name")
                        .desired_width(f32::INFINITY)
                },
            );
            let filter = self.filter.to_lowercase();
            let mut open = None;
            egui::ScrollArea::vertical()
                .id_salt("note-list")
                .show(ui, |ui| {
                    if self.listing.is_some() {
                        ui.spinner();
                    }
                    if self.files.is_empty() && self.listing.is_none() {
                        ui.weak("No notes in this folder.");
                    }
                    for file in self.files.iter().filter(|file| {
                        filter.is_empty() || file.to_string_lossy().to_lowercase().contains(&filter)
                    }) {
                        ui.push_id(file, |ui| {
                            if ui
                                .add(egui::Button::new(file.to_string_lossy()).wrap())
                                .clicked()
                            {
                                open = Some(file.clone());
                            }
                        });
                    }
                });
            if let Some(path) = open {
                match Note::open(&root, path) {
                    Ok(note) => {
                        self.note = Some(note);
                        self.status.clear();
                    }
                    Err(e) => self.status = format!("{e:#}"),
                }
            }
        }
        attachment
    }
}

impl super::Desktop {
    pub(super) fn notes_page(&mut self, ui: &mut egui::Ui) {
        if let Some((name, text)) = self.notebook.show(ui, &mut self.android_ime) {
            self.attach_text(name, text);
            self.page = Page::Chat;
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn open_obsidian(root: &Path, path: &Path) -> anyhow::Result<()> {
    let path = notebook::resolve(root, path)?;
    let mut url = reqwest::Url::parse("obsidian://open")?;
    url.query_pairs_mut()
        .append_pair("path", &crate::projects::display_path(&path));
    webbrowser::open(url.as_str())?;
    Ok(())
}

fn render_image(
    ui: &mut egui::Ui,
    root: &Path,
    note: &Path,
    images: &mut HashMap<String, Option<egui::TextureHandle>>,
    url: &str,
) {
    if let Ok(link) = reqwest::Url::parse(url)
        && matches!(link.scheme(), "http" | "https")
    {
        ui.hyperlink_to("Open linked image", link.as_str());
        return;
    }
    let image = images.entry(url.to_owned()).or_insert_with(|| {
        let relative = crate::html_preview::decode(url)?;
        let path = notebook::resolve(root, &note.parent()?.join(relative)).ok()?;
        let pixels = super::artifact_thumbnail(&path, path.metadata().ok()?.len())?;
        Some(
            ui.ctx()
                .load_texture(url, pixels, egui::TextureOptions::LINEAR),
        )
    });
    if let Some(image) = image {
        let size = image.size_vec2();
        ui.add(egui::Image::new((
            image.id(),
            size * (ui.available_width() / size.x).min(1.),
        )));
    } else {
        ui.weak("Image unavailable in this note folder");
    }
}

#[cfg(windows)]
fn copy_image(root: &Path, note: &Path, source: &Path) -> anyhow::Result<String> {
    use std::{fs, io::Read};
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    anyhow::ensure!(
        matches!(ext.as_str(), "png" | "jpg" | "jpeg"),
        "Choose a PNG or JPEG image"
    );
    let mut bytes = Vec::new();
    const LIMIT: u64 = 16 * 1024 * 1024;
    fs::File::open(source)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= LIMIT as usize,
        "Choose an image up to 16 MiB"
    );
    anyhow::ensure!(
        matches!(
            image::guess_format(&bytes)?,
            image::ImageFormat::Png | image::ImageFormat::Jpeg
        ),
        "Choose a PNG or JPEG image"
    );
    let name = format!(
        "note-image-{}.{ext}",
        crate::sync::core::hex(&crate::vault::random::<8>()?)
    );
    let relative = format!("note-images/{name}");
    let path = notebook::resolve(
        root,
        &note.parent().unwrap_or(Path::new("")).join(&relative),
    )?;
    fs::create_dir_all(path.parent().unwrap())?;
    crate::tools::write_generated(&path, &bytes, false)?;
    Ok(relative)
}

enum Format {
    Heading(&'static str),
    Wrap(&'static str, &'static str, &'static str),
    Table,
    Tag,
    #[cfg(windows)]
    ImageFile(String),
}

fn toolbar(ui: &mut egui::Ui) -> Option<Format> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        for (label, hint, format) in [
            (
                RichText::new("H1").strong(),
                "Heading 1",
                Format::Heading("# "),
            ),
            (
                RichText::new("H2").strong(),
                "Heading 2",
                Format::Heading("## "),
            ),
            (
                RichText::new("B").strong(),
                "Bold",
                Format::Wrap("**", "**", "bold text"),
            ),
            (
                RichText::new("I").italics(),
                "Italic",
                Format::Wrap("*", "*", "italic text"),
            ),
            (RichText::new("List"), "Bullet list", Format::Heading("- ")),
            (
                RichText::new("Link"),
                "Insert link",
                Format::Wrap("[", "](https://)", "link text"),
            ),
            (RichText::new("Table"), "Insert table", Format::Table),
            (
                RichText::new("Image"),
                "Insert image with a URL or note-relative PNG or JPEG path",
                Format::Wrap("![", "](image.png)", "description"),
            ),
            (RichText::new("Tag"), "Insert a searchable tag", Format::Tag),
        ] {
            if ui.button(label).on_hover_text(hint).clicked() {
                action = Some(format);
            }
        }
    });
    action
}

fn apply(ui: &egui::Ui, id: egui::Id, text: &mut String, action: Format) {
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let selection = state
        .cursor
        .char_range()
        .map(|range| {
            range.primary.index.min(range.secondary.index)
                ..range.primary.index.max(range.secondary.index)
        })
        .unwrap_or_else(|| {
            let end = text.chars().count();
            end..end
        });
    let selected = format_text(text, selection, action);
    state.cursor.set_char_range(Some(CCursorRange {
        primary: CCursor::new(selected.end),
        secondary: CCursor::new(selected.start),
    }));
    state.store(ui.ctx(), id);
    ui.memory_mut(|memory| memory.request_focus(id));
}

fn format_text(text: &mut String, selection: Range<usize>, action: Format) -> Range<usize> {
    let byte = |n| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    let mut start = byte(selection.start);
    let end = byte(selection.end);
    let previous = &text[start..end];
    let (replacement, selection) = match action {
        Format::Heading(prefix) => {
            start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let before = &text[start..end];
            let existing = before.trim_start_matches('#');
            let line = if existing.len() < before.len() {
                existing.strip_prefix(' ').unwrap_or(before)
            } else {
                before
            };
            let result = format!("{prefix}{line}");
            let pos = result.chars().count();
            (result, pos..pos)
        }
        Format::Wrap(before, after, placeholder) => {
            let label = if previous.is_empty() {
                placeholder
            } else {
                previous
            };
            let offset = before.chars().count();
            (
                format!("{before}{label}{after}"),
                offset..offset + label.chars().count(),
            )
        }
        #[cfg(windows)]
        Format::ImageFile(path) => {
            let label = if previous.is_empty() {
                "description"
            } else {
                previous
            };
            (format!("![{label}]({path})"), 2..2 + label.chars().count())
        }
        Format::Table => (
            "\n\n| Column 1 | Column 2 |\n| --- | --- |\n| Value | Value |\n\n".into(),
            4..12,
        ),
        Format::Tag => {
            let tag = notebook::parse_tag(previous).unwrap_or_else(|| "tag".into());
            let space = if text[..start]
                .chars()
                .next_back()
                .is_some_and(|ch| !ch.is_whitespace())
            {
                " "
            } else {
                ""
            };
            let offset = space.len() + 1;
            (
                format!("{space}#{tag}"),
                offset..offset + tag.chars().count(),
            )
        }
    };
    let offset = text[..start].chars().count();
    text.replace_range(start..end, &replacement);
    offset + selection.start..offset + selection.end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_actions_work_on_selected_unicode() {
        let mut text = "Hello 勉 café".to_owned();
        let range = format_text(&mut text, 6..7, Format::Wrap("**", "**", "bold"));
        assert_eq!(text, "Hello **勉** café");
        assert_eq!(range, 8..9);
        format_text(&mut text, 2..2, Format::Heading("## "));
        assert_eq!(text, "## Hello **勉** café");
    }
}
