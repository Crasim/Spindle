use super::chat_selection::ChatText;
use super::*;
use crate::documents::{self, Body, Document, EXTENSIONS};
use anyhow::Result;
use std::sync::mpsc::{self, TryRecvError};

#[derive(Default)]
pub(super) struct ReaderPane {
    pub open: bool,
    pub(super) preview: html_preview::HtmlPreview,
    width: Option<f32>,
    resizing: bool,
    pub(super) file: PathBuf,
    pub(super) preview_requested: bool,
    document: Option<Document>,
    loading: Option<Receiver<Result<Document>>>,
    pub(super) index: usize,
    error: String,
    edit_excerpt: Option<String>,
    page_image: Option<egui::TextureHandle>,
    rendering: Option<Receiver<Result<egui::ColorImage>>>,
    pdf_text: bool,
    snapshot: bool,
    snapshot_text: Option<String>,
    editor: Option<crate::editing::Editable>,
    dirty: bool,
    sheet: usize,
    confirm_overwrite: bool,
    pub(super) status: String,
    /// Open the editor as soon as the requested file finishes loading.
    pub(super) edit_when_loaded: bool,
    text_preview: bool,
    selection: Option<(usize, usize)>,
    pub(super) tab: side_tabs::Tab,
    pub(super) browser: side_tabs::BrowserPane,
    library: side_tabs::Library,
    /// This chat's files and the library folder, set by the desktop before each frame.
    pub(super) chat_files: Vec<side_tabs::ChatFile>,
    pub(super) library_root: Option<(PathBuf, String)>,
    /// What the user picked in the Files or Library tab, for the desktop to open.
    pub(super) picked: Option<side_tabs::Pick>,
}

fn column(mut n: usize) -> String {
    let mut bytes = Vec::new();
    loop {
        bytes.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    bytes.reverse();
    String::from_utf8(bytes).unwrap()
}

impl ReaderPane {
    pub fn open_output(&mut self, name: String, source: String) {
        self.open = true;
        self.tab = side_tabs::Tab::Viewer;
        self.preview.load(name, source);
    }

    pub fn load(&mut self, file: PathBuf) {
        if crate::html_preview::is_html(&file) {
            self.preview.hide();
            *self = Self {
                open: true,
                width: self.width,
                file: file.clone(),
                browser: std::mem::take(&mut self.browser),
                library: std::mem::take(&mut self.library),
                ..Default::default()
            };
            match crate::artifacts::read_html(&file) {
                Ok(source) => {
                    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    self.preview.load_with_path(name, source, Some(file));
                }
                Err(error) => self.error = format!("Cannot open HTML: {error:#}"),
            }
            return;
        }
        self.load_document(file);
    }

    pub fn load_document(&mut self, file: PathBuf) {
        self.preview.hide();
        *self = Self {
            open: true,
            width: self.width,
            file: file.clone(),
            browser: std::mem::take(&mut self.browser),
            library: std::mem::take(&mut self.library),
            ..Default::default()
        };
        self.tab = side_tabs::Tab::Viewer;
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(documents::read(&file));
        });
    }

    /// Open a generated file from its thread snapshot when the original path is unavailable.
    pub fn load_snapshot(&mut self, name: String, text: String) {
        self.preview.hide();
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_owned())
            .unwrap_or_default();
        *self = Self {
            open: true,
            width: self.width,
            file: PathBuf::from(&name),
            snapshot: true,
            snapshot_text: Some(text.clone()),
            browser: std::mem::take(&mut self.browser),
            library: std::mem::take(&mut self.library),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(documents::parse(&ext, text.into_bytes()));
        });
    }

    fn start_edit(&mut self) {
        self.status.clear();
        let opened = match &self.snapshot_text {
            Some(text) => crate::editing::Editable::snapshot(&self.file.to_string_lossy(), text),
            None => crate::editing::Editable::open(&self.file),
        };
        match opened {
            Ok(editor) => {
                self.editor = Some(editor);
                self.dirty = false;
                self.sheet = 0;
                self.confirm_overwrite = false;
            }
            Err(e) => self.error = format!("Cannot edit this file: {e:#}"),
        }
    }

    /// Ask where to save an edited copy. Off Windows, use a numbered name beside the original.
    fn choose_copy(&self, root: &std::path::Path) -> Option<PathBuf> {
        let editor = self.editor.as_ref()?;
        let base = if self.snapshot {
            root.join(&self.file)
        } else {
            self.file.clone()
        };
        let default = crate::editing::copy_name(&base);
        #[cfg(windows)]
        {
            let own = crate::documents::extension(&self.file);
            let mut kinds = crate::editing::save_as_extensions(&editor.content).to_vec();
            kinds.sort_by_key(|e| *e != own.as_str());
            let mut dialog = rfd::FileDialog::new()
                .set_title("Save an edited copy")
                .set_file_name(default.file_name().unwrap_or_default().to_string_lossy());
            if let Some(dir) = default.parent() {
                dialog = dialog.set_directory(dir);
            }
            for kind in kinds {
                dialog = dialog.add_filter(kind, &[kind]);
            }
            let _ = editor;
            dialog.save_file()
        }
        #[cfg(not(windows))]
        {
            let _ = editor;
            Some(default)
        }
    }

    fn finish_save(&mut self, result: Result<PathBuf>) {
        match result {
            Ok(path) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.load(path);
                self.status = format!("Saved {name}");
            }
            Err(e) => self.error = format!("Save failed: {e:#}"),
        }
    }

    fn edit_ui(&mut self, ui: &mut egui::Ui, root: &std::path::Path) {
        let Some(mut editor) = self.editor.take() else {
            return;
        };
        let mut save = false;
        let mut save_as = false;
        let mut done = false;
        if !self.confirm_overwrite {
            ui.horizontal_wrapped(|ui| {
                let writable = crate::editing::writable(&self.file);
                save = ui
                    .add_enabled(
                        self.dirty && !self.snapshot && writable,
                        egui::Button::new("Save"),
                    )
                    .on_disabled_hover_text(if self.snapshot {
                        "This is a thread snapshot. Use Save as."
                    } else if !writable {
                        "Spindle can't write this format. Use Save as."
                    } else {
                        "No changes to save"
                    })
                    .clicked();
                save_as = ui.button("Save as…").clicked();
                done = ui
                    .button(if self.dirty {
                        "Discard changes"
                    } else {
                        "Done editing"
                    })
                    .clicked();
                if self.dirty {
                    ui.weak("Unsaved changes");
                }
            });
            if editor.lossy {
                ui.label(
                    Text::new(
                        "Saving keeps text and cell values only. Fonts, styles, formulas, \
                         charts and images are not kept.",
                    )
                    .color(muted())
                    .size(12.),
                );
            }
            ui.separator();
            match &mut editor.content {
                crate::editing::Content::Text(text) => {
                    use crate::editing::Format;
                    let ext = crate::documents::extension(&self.file);
                    let md = matches!(ext.as_str(), "md" | "markdown");
                    let mut format = None;
                    if md {
                        ui.horizontal_wrapped(|ui| {
                            ui.selectable_value(&mut self.text_preview, false, "Edit");
                            ui.selectable_value(&mut self.text_preview, true, "Preview");
                            if self.text_preview {
                                return;
                            }
                            ui.separator();
                            for (label, hover, f) in [
                                ("B", "Bold", Format::Bold),
                                ("I", "Italic", Format::Italic),
                                ("H", "Heading", Format::Heading),
                                ("• List", "Bulleted list", Format::Bullet),
                                ("1. List", "Numbered list", Format::Numbered),
                                ("Code", "Inline code", Format::Code),
                                ("Link", "Link", Format::Link),
                                ("Table", "Insert a table", Format::Table),
                            ] {
                                let t = Text::new(label);
                                let t = match f {
                                    Format::Bold => t.strong(),
                                    Format::Italic => t.italics(),
                                    _ => t,
                                };
                                if ui.small_button(t).on_hover_text(hover).clicked() {
                                    format = Some(f);
                                }
                            }
                        });
                    }
                    let mut place_cursor = None;
                    if let Some(f) = format {
                        let end = text.chars().count();
                        let selection = self
                            .selection
                            .map(|(a, b)| (a.min(end), b.min(end)))
                            .unwrap_or((end, end));
                        let next = crate::editing::apply(text, selection, f);
                        self.selection = Some(next);
                        place_cursor = Some(next);
                        self.dirty = true;
                    }
                    let words = text.split_whitespace().count();
                    let lines = text.lines().count();
                    if md && self.text_preview {
                        egui::ScrollArea::vertical()
                            .id_salt(("edit-preview", &self.file))
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - 24.)
                            .show(ui, |ui| {
                                markdown(ui, text, false);
                            });
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt(("edit-text", &self.file))
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - 24.)
                            .show(ui, |ui| {
                                let id = egui::Id::new(("edit-text-field", &self.file));
                                let mut output = egui::TextEdit::multiline(text)
                                    .id(id)
                                    .font(if md || ext == "txt" {
                                        egui::TextStyle::Monospace
                                    } else {
                                        egui::TextStyle::Body
                                    })
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(30)
                                    .show(ui);
                                if output.response.changed() {
                                    self.dirty = true;
                                }
                                if let Some(range) = output.cursor_range {
                                    self.selection = Some((
                                        range.primary.ccursor.index,
                                        range.secondary.ccursor.index,
                                    ));
                                }
                                if let Some((a, b)) = place_cursor {
                                    output.state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(a),
                                            egui::text::CCursor::new(b),
                                        ),
                                    ));
                                    output.state.store(ui.ctx(), id);
                                    output.response.request_focus();
                                }
                            });
                    }
                    ui.weak(format!("{words} words · {lines} lines"));
                }
                crate::editing::Content::Sheets(sheets) => {
                    self.sheet = self.sheet.min(sheets.len().saturating_sub(1));
                    if sheets.len() > 1 {
                        crate::widgets::InlineChoice::from_id_salt("edit-sheet")
                            .selected_text(&sheets[self.sheet].name)
                            .show_ui(ui, |ui| {
                                for (i, s) in sheets.iter().enumerate() {
                                    ui.selectable_value(&mut self.sheet, i, &s.name);
                                }
                            });
                    }
                    let index = self.sheet;
                    let rows = &mut sheets[index].rows;
                    let mut cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                    ui.horizontal(|ui| {
                        if ui.small_button("Add row").clicked() {
                            rows.push(vec![String::new(); cols.max(1)]);
                            self.dirty = true;
                        }
                        if ui.small_button("Add column").clicked() {
                            if rows.is_empty() {
                                rows.push(vec![]);
                            }
                            for row in rows.iter_mut() {
                                row.resize(cols + 1, String::new());
                            }
                            cols += 1;
                            self.dirty = true;
                        }
                    });
                    let height = ui.spacing().interact_size.y + 4.;
                    let width = 120.;
                    use crate::editing::Shape;
                    let options_id = ui.make_persistent_id(("sheet-options", &self.file, index));
                    let mut options = ui.data(|data| data.get_temp::<(bool, usize)>(options_id));
                    let mut reshape = None;
                    if let Some((column, target)) = options {
                        if (column && target >= cols) || (!column && target >= rows.len()) {
                            options = None;
                        } else {
                            ui.group(|ui| {
                                let title = if column {
                                    format!("Column {}", crate::editing::column(target))
                                } else {
                                    format!("Row {}", target + 1)
                                };
                                if super::panel_header(ui, &title, "Close") {
                                    options = None;
                                    return;
                                }
                                let actions = if column {
                                    [
                                        ("Insert column left", Shape::InsertColumn(target)),
                                        ("Insert column right", Shape::InsertColumn(target + 1)),
                                        ("Delete column", Shape::DeleteColumn(target)),
                                    ]
                                } else {
                                    [
                                        ("Insert row above", Shape::InsertRow(target)),
                                        ("Insert row below", Shape::InsertRow(target + 1)),
                                        ("Delete row", Shape::DeleteRow(target)),
                                    ]
                                };
                                ui.horizontal_wrapped(|ui| {
                                    for (name, action) in actions {
                                        if ui.button(name).clicked() {
                                            reshape = Some(action);
                                            options = None;
                                        }
                                    }
                                });
                            });
                        }
                    }
                    egui::ScrollArea::both()
                        .id_salt(("edit-sheet-body", &self.file, index))
                        .auto_shrink([false, false])
                        .show_rows(ui, height, rows.len() + 1, |ui, range| {
                            for i in range {
                                ui.horizontal(|ui| {
                                    if i == 0 {
                                        ui.add_sized([40., height], egui::Label::new(""));
                                        for c in 0..cols {
                                            let cell = ui
                                                .add_sized(
                                                    [width, height],
                                                    egui::Label::new(
                                                        Text::new(crate::editing::column(c))
                                                            .color(accent())
                                                            .monospace(),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                                .on_hover_text("Column options");
                                            if cell.clicked() || cell.secondary_clicked() {
                                                options = Some((true, c));
                                            }
                                        }
                                        return;
                                    }
                                    let r = i - 1;
                                    let cell = ui
                                        .add_sized(
                                            [40., height],
                                            egui::Label::new(
                                                Text::new((r + 1).to_string()).color(muted()),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text("Row options");
                                    if cell.clicked() || cell.secondary_clicked() {
                                        options = Some((false, r));
                                    }
                                    let row = &mut rows[r];
                                    for c in 0..cols {
                                        let id = egui::Id::new(("edit-cell", index, r, c));
                                        if c < row.len() {
                                            if ui
                                                .add_sized(
                                                    [width, height - 4.],
                                                    egui::TextEdit::singleline(&mut row[c]).id(id),
                                                )
                                                .changed()
                                            {
                                                self.dirty = true;
                                            }
                                        } else {
                                            let mut value = String::new();
                                            if ui
                                                .add_sized(
                                                    [width, height - 4.],
                                                    egui::TextEdit::singleline(&mut value).id(id),
                                                )
                                                .changed()
                                            {
                                                row.resize(c + 1, String::new());
                                                row[c] = value;
                                                self.dirty = true;
                                            }
                                        }
                                    }
                                });
                            }
                        });
                    ui.data_mut(|data| {
                        if let Some(options) = options {
                            data.insert_temp(options_id, options);
                        } else {
                            data.remove::<(bool, usize)>(options_id);
                        }
                    });
                    if let Some(change) = reshape {
                        crate::editing::reshape(rows, change);
                        self.dirty = true;
                    }
                }
            }
            if save {
                if editor.lossy {
                    self.confirm_overwrite = true;
                } else {
                    let path = editor.path.clone();
                    let result = editor.save().map(|()| path);
                    self.editor = Some(editor);
                    if result.is_ok() {
                        self.editor = None;
                    }
                    return self.finish_save(result);
                }
            }
        }
        if self.confirm_overwrite {
            let mut over = false;
            let mut keep_open = true;
            let mut panel_open = true;
            super::detail_panel(
                ui,
                "Save over the original?",
                "Back",
                &mut panel_open,
                |ui| {
                    ui.label(format!(
                        "Spindle's basic writer will replace {} with text and cell values only. \
                         Fonts, styles, formulas, charts, images and comments will be lost.",
                        self.file.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    ui.horizontal(|ui| {
                        over = ui.button("Save over").clicked();
                        if ui.button("Save as a copy…").clicked() {
                            save_as = true;
                            keep_open = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep_open = false;
                        }
                    });
                },
            );
            if over {
                self.confirm_overwrite = false;
                let path = editor.path.clone();
                let result = editor.save().map(|()| path);
                if result.is_err() {
                    self.editor = Some(editor);
                }
                return self.finish_save(result);
            }
            self.confirm_overwrite = keep_open && panel_open;
        }
        self.editor = Some(editor);
        if save_as {
            self.confirm_overwrite = false;
            if let Some(path) = self.choose_copy(root) {
                let editor = self.editor.as_ref().unwrap();
                let result = crate::editing::write(&path, &editor.content).map(|()| path);
                if result.is_ok() {
                    self.editor = None;
                }
                return self.finish_save(result);
            }
        }
        if done {
            self.editor = None;
            self.dirty = false;
        }
    }

    fn section_changed(&mut self) {
        self.page_image = None;
        self.rendering = None;
        self.error.clear();
        self.edit_excerpt = None;
        #[cfg(windows)]
        if !self.pdf_text {
            if let Some(pdf) = self.document.as_ref().and_then(|d| d.pdf.clone()) {
                let index = self.index;
                let (tx, rx) = mpsc::channel();
                self.rendering = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send(documents::render_pdf(&pdf, index));
                });
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(Ok(doc)) => {
                    if self.index >= doc.sections.len() {
                        self.error = "Section does not exist".into();
                        self.loading = None;
                        return;
                    }
                    self.document = Some(doc);
                    self.loading = None;
                    self.section_changed();
                    if std::mem::take(&mut self.edit_when_loaded) {
                        self.start_edit();
                    }
                }
                Ok(Err(e)) => {
                    self.error = format!("{e:#}");
                    self.loading = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = "Document reader stopped".into();
                    self.loading = None;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        if let Some(rx) = &self.rendering {
            match rx.try_recv() {
                Ok(Ok(image)) => {
                    self.page_image =
                        Some(ctx.load_texture("PDF page", image, egui::TextureOptions::LINEAR));
                    self.rendering = None;
                }
                Ok(Err(e)) => {
                    self.error = format!("Page rendering failed: {e:#}");
                    self.rendering = None;
                    self.pdf_text = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = "Page renderer stopped".into();
                    self.rendering = None;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
    }

    pub fn pick(&mut self, root: &std::path::Path) {
        #[cfg(windows)]
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(root)
            .add_filter("Readable documents", EXTENSIONS)
            .pick_file()
        {
            self.load(path);
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        root: &std::path::Path,
        attach: &mut Option<String>,
        plan: Option<(&mut notes::PlanPane, &crate::state::ThreadInfo, &str, &mut android_ime::ComposerIme)>,
        browser_path: &str,
        browser_chat: (u64, &[(u64, String)], bool),
    ) {
        if !self.open {
            self.preview.hide();
            self.browser.hide();
            return;
        }
        self.poll(ctx);
        let bounds = ctx.available_rect();
        let max = (bounds.width() - 405.).clamp(250., 900.);
        let mut width = self.width.unwrap_or(400.).clamp(250., max);
        let edge = bounds.right() - width;
        // A fast drag may send press, movement and release before the next rendered frame.
        // Read those events in order and keep the release position as the final width.
        let hovering = ctx.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    } => {
                        self.resizing =
                            (pos.x - edge).abs() <= 6. && bounds.y_range().contains(pos.y);
                    }
                    egui::Event::PointerMoved(pos) if self.resizing => {
                        width = (bounds.right() - pos.x).clamp(250., max);
                    }
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    } if self.resizing => {
                        width = (bounds.right() - pos.x).clamp(250., max);
                        self.resizing = false;
                    }
                    egui::Event::PointerGone => self.resizing = false,
                    _ => (),
                }
            }
            input
                .pointer
                .hover_pos()
                .is_some_and(|pos| (pos.x - edge).abs() <= 6. && bounds.y_range().contains(pos.y))
        });
        self.width = Some(width);
        if self.resizing || hovering {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        // On a phone the panel covers the conversation; Close brings it back.
        let width = if super::narrow(ctx) {
            ctx.screen_rect().width()
        } else {
            width
        };
        let panel = egui::SidePanel::right("document-reader")
            .exact_width(width)
            .resizable(false)
            .frame(egui::Frame::new().fill(panel()).inner_margin(14))
            .show(ctx, |ui| {
                if self.edit_excerpt.is_some() {
                    if let Some(text) = &mut self.edit_excerpt {
                        let mut open = true;
                        let mut submit = false;
                        super::detail_panel(
                            ui,
                            "Attach a document excerpt",
                            "Back",
                            &mut open,
                            |ui| {
                                ui.label(
                                    "Keep the passage you want the next model request to receive.",
                                );
                                egui::ScrollArea::vertical()
                                    .max_height(260.)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(text)
                                                .desired_rows(8)
                                                .desired_width(f32::INFINITY),
                                        );
                                    });
                                ui.label(format!("{} / 6000 UTF-8 bytes", text.len()));
                                submit = ui
                                    .add_enabled(
                                        !text.trim().is_empty() && text.len() <= 6000,
                                        egui::Button::new("Review attachment"),
                                    )
                                    .clicked();
                            },
                        );
                        if submit {
                            let title = &self.document.as_ref().unwrap().sections[self.index].title;
                            *attach = Some(format!(
                                "/excerpt {}",
                                serde_json::to_string(&(&self.file, title, &text)).unwrap()
                            ));
                            open = false;
                        }
                        if !open {
                            self.edit_excerpt = None;
                        }
                    }
                    return;
                }
                if self.tab == side_tabs::Tab::Plan && plan.is_none() {
                    self.tab = side_tabs::Tab::Viewer;
                }
                ui.horizontal_top(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        let close = ui.add(
                            egui::Button::new(Text::new("×").size(20.))
                                .frame(false)
                                .min_size(egui::vec2(30., 30.)),
                        );
                        close.widget_info(|| egui::WidgetInfo::labeled(
                            egui::WidgetType::Button, ui.is_enabled(), "Close side panel",
                        ));
                        if close.on_hover_text("Close side panel").clicked() {
                            self.open = false;
                            self.preview.hide();
                            self.browser.hide();
                        }
                        let tabs = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
                        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 30.), tabs, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.;
                            for (tab, name, hint) in side_tabs::Tab::ALL {
                                let hover = if tab == side_tabs::Tab::Plan {
                                    let Some((_, thread, _, _)) = plan.as_ref() else { continue };
                                    format!("{hint} · {}/{} complete", thread.steps.iter().filter(|s| s.done).count(), thread.steps.len())
                                } else {
                                    hint.to_owned()
                                };
                                let label = if tab == side_tabs::Tab::Files && !self.chat_files.is_empty() {
                                    format!("{name} {}", self.chat_files.len())
                                } else {
                                    name.to_owned()
                                };
                                ui.selectable_value(&mut self.tab, tab, label).on_hover_text(hover);
                            }
                        });
                    });
                });
                ui.separator();
                if self.tab != side_tabs::Tab::Viewer {
                    self.preview.hide();
                }
                if self.tab != side_tabs::Tab::Browser {
                    self.browser.hide();
                }
                if !self.open {
                    return;
                }
                match self.tab {
                    side_tabs::Tab::Plan => {
                        if let Some((pane, thread, goal, ime)) = plan {
                            self.picked = pane.show(ui, thread, goal, ime);
                        }
                        return;
                    }
                    side_tabs::Tab::Viewer => {}
                    side_tabs::Tab::Files => {
                        self.picked = side_tabs::files_tab(ui, &self.chat_files);
                        return;
                    }
                    side_tabs::Tab::Library => {
                        let (folder, label) = self
                            .library_root
                            .clone()
                            .unwrap_or_else(|| (root.to_owned(), "Workspace".into()));
                        self.picked = self.library.show(ui, &folder, &label);
                        return;
                    }
                    side_tabs::Tab::Browser => {
                        let (chat, fallbacks, ready) = browser_chat;
                        self.picked = self.browser.show(ui, frame, chat, fallbacks, ready, browser_path);
                        return;
                    }
                }
                if self.preview.active() {
                    if !self.open {
                        self.preview.hide();
                        self.preview = Default::default();
                        return;
                    }
                    if !self.file.as_os_str().is_empty()
                        && ui.small_button("Document tools").clicked()
                    {
                        self.load_document(self.file.clone());
                    } else {
                        self.preview.show(ui, frame, browser_path);
                        return;
                    }
                }
                let ready = self.loading.is_none() && self.document.is_some();
                let html = !self.snapshot
                    && matches!(
                        crate::documents::extension(&self.file).as_str(),
                        "htm" | "html"
                    );
                let outside = !self.file.canonicalize().is_ok_and(|p| p.starts_with(root));
                let (mut reload, mut start_edit, mut excerpt) = (false, false, false);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button("Open…")
                        .on_hover_text("Open a document (Ctrl+O)")
                        .clicked()
                    {
                        self.pick(root);
                    }
                    if ready && !self.snapshot && ui.small_button("Reload").clicked() {
                        reload = true;
                    }
                    if ready
                        && self.editor.is_none()
                        && crate::editing::editable(&self.file)
                        && ui
                            .small_button("Edit")
                            .on_hover_text("Edit this file and save it from the side panel")
                            .clicked()
                    {
                        start_edit = true;
                    }
                    if ready
                        && self.editor.is_none()
                        && !outside
                        && ui
                            .small_button("Excerpt to chat")
                            .on_hover_text("Attach part of this document to your next message")
                            .clicked()
                    {
                        excerpt = true;
                    }
                    if ready && html && ui.small_button("Run preview").clicked() {
                        self.preview_requested = true;
                    }
                });
                if reload {
                    self.load(self.file.clone());
                    return;
                }
                if start_edit {
                    self.start_edit();
                }
                if !self.error.is_empty() {
                    ui.colored_label(accent(), &self.error);
                }
                if self.loading.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading document…");
                    });
                    return;
                }
                if self.document.is_none() {
                    if self.error.is_empty() {
                        ui.add_space(16.);
                        ui.heading("Open a document");
                        ui.label("Markdown, Word, RTF, CSV, Excel, PDF, HTML or text");
                    }
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        Text::new(self.file.file_name().unwrap_or_default().to_string_lossy())
                            .strong(),
                    )
                    .on_hover_text(self.file.display().to_string());
                    let doc = self.document.as_ref().unwrap();
                    ui.weak(if self.snapshot {
                        format!("{} · thread snapshot", doc.kind)
                    } else {
                        doc.kind.to_owned()
                    });
                });
                if !self.status.is_empty() {
                    ui.label(Text::new(&self.status).color(accent()));
                }
                if self.editor.is_some() {
                    ui.separator();
                    self.edit_ui(ui, root);
                    return;
                }
                let doc = self.document.as_ref().unwrap();
                if !doc.note.is_empty() {
                    ui.weak(&doc.note);
                }
                let mut index = self.index;
                let mut mode_changed = false;
                if doc.sections.len() > 1 {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(index > 0, egui::Button::new("‹").small())
                            .on_hover_text("Previous section")
                            .clicked()
                        {
                            index -= 1;
                        }
                        crate::widgets::InlineChoice::from_id_salt("doc-section")
                            .width((ui.available_width() - 45.).clamp(100., 300.))
                            .selected_text(&doc.sections[index].title)
                            .show_ui(ui, |ui| {
                                for (i, s) in doc.sections.iter().enumerate() {
                                    ui.selectable_value(&mut index, i, &s.title);
                                }
                            });
                        if ui
                            .add_enabled(
                                index + 1 < doc.sections.len(),
                                egui::Button::new("›").small(),
                            )
                            .on_hover_text("Next section")
                            .clicked()
                        {
                            index += 1;
                        }
                    });
                }
                #[cfg(windows)]
                if doc.pdf.is_some() {
                    ui.horizontal(|ui| {
                        mode_changed |= ui
                            .selectable_value(&mut self.pdf_text, false, "Page image")
                            .changed();
                        mode_changed |= ui
                            .selectable_value(&mut self.pdf_text, true, "Text")
                            .changed();
                    });
                }
                if excerpt {
                    self.edit_excerpt = Some(doc.sections[index].text());
                }
                if index != self.index || mode_changed {
                    self.index = index;
                    self.section_changed();
                }
                ui.separator();
                let doc = self.document.as_ref().unwrap();
                let section = &doc.sections[self.index];
                egui::ScrollArea::both()
                    .id_salt(("reader-body", &self.file, self.index, self.pdf_text))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        if self.rendering.is_some() {
                            ui.spinner();
                            return;
                        }
                        if let Some(image) = &self.page_image {
                            let scale = width / image.size_vec2().x;
                            ui.image((image.id(), image.size_vec2() * scale));
                            return;
                        }
                        match &section.body {
                            Body::Text(text) => {
                                ui.set_width(width);
                                if text.is_empty() {
                                    ui.label("No extractable text in this section.");
                                } else if doc.kind == "Markdown" || doc.kind == "Word" {
                                    markdown(ui, text, false);
                                } else {
                                    ui.add(egui::Label::new(text).wrap().selectable(true));
                                }
                            }
                            Body::Table {
                                rows,
                                first_row,
                                first_col,
                                styles,
                            } => {
                                if rows.is_empty() {
                                    ui.label("Empty sheet");
                                    return;
                                }
                                let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                                egui::Grid::new(("reader-cells", &self.file, self.index))
                                    .striped(true)
                                    .spacing([12., 5.])
                                    .max_col_width(260.)
                                    .show(ui, |ui| {
                                        ui.label("");
                                        for c in 0..cols {
                                            ui.label(
                                                Text::new(column(first_col + c))
                                                    .color(accent())
                                                    .monospace(),
                                            );
                                        }
                                        ui.end_row();
                                        for (i, row) in rows.iter().enumerate() {
                                            ui.label(
                                                Text::new((first_row + i + 1).to_string())
                                                    .color(muted()),
                                            );
                                            for (c, cell) in row.iter().enumerate() {
                                                let style = styles.get(i).and_then(|r| r.get(c));
                                                styled_cell(ui, cell, style);
                                            }
                                            ui.end_row();
                                        }
                                    });
                            }
                        }
                    });
            });
        let border = panel.response.rect.left();
        let painter = ctx.layer_painter(panel.response.layer_id);
        painter.vline(
            border,
            panel.response.rect.y_range(),
            egui::Stroke::new(
                1_f32,
                if self.resizing || hovering {
                    accent()
                } else {
                    line()
                },
            ),
        );
    }
}

pub(super) enum CodeAction {
    Preview(String, String),
    Save(String, String),
    /// A filename mentioned inline in a reply was clicked.
    OpenFile(String),
}

/// What the user chose on a generated-file card.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FileAction {
    Open,
    Source,
    Edit,
    CopyPath,
    SaveCopy,
    Download,
    Reveal,
}

/// One compact card per generated file: icon, name, type and size, a primary action and a
/// menu for the rest.
pub(super) fn file_card(ui: &mut egui::Ui, name: &str, bytes: usize) -> Option<FileAction> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let html = matches!(ext.as_str(), "html" | "htm");
    let apk = ext == "apk";
    let readable = crate::documents::readable(name);
    let editable = crate::editing::editable(std::path::Path::new(name));
    let mut chosen = None;
    ui.add_space(4.);
    egui::Frame::new()
        .fill(raised())
        .stroke(egui::Stroke::new(1_f32, line()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(460.));
            if apk {
                ui.horizontal(|ui| {
                    if ui.button("Download APK").clicked() {
                        chosen = Some(FileAction::Download);
                    }
                    ui.add(egui::Label::new(Text::new(name).strong()).truncate())
                        .on_hover_text(name);
                });
                return;
            }
            let options_id = ui.make_persistent_id(("file-actions", name));
            let mut options = ui
                .data(|data| data.get_temp::<bool>(options_id))
                .unwrap_or(false);
            ui.horizontal(|ui| {
                symbol(ui, Icon::Reader, accent());
                let response = ui
                    .add(
                        egui::Label::new(Text::new(name).strong())
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Open in the side panel");
                if response.clicked() {
                    chosen = Some(FileAction::Open);
                }
                ui.weak(format!(
                    "{} · {:.1} KB",
                    ext.to_ascii_uppercase(),
                    bytes as f64 / 1024.
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("More").clicked() {
                        options = !options;
                    }
                    let primary = if html {
                        "Preview"
                    } else if readable {
                        "Open"
                    } else {
                        "View"
                    };
                    if ui.button(primary).clicked() {
                        chosen = Some(FileAction::Open);
                    }
                });
            });
            if options {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Close").clicked() {
                        options = false;
                    }
                    if (readable || html) && ui.button("View source").clicked() {
                        chosen = Some(FileAction::Source);
                    }
                    if editable && ui.button("Edit").clicked() {
                        chosen = Some(FileAction::Edit);
                    }
                    if ui.button("Download…").clicked() {
                        chosen = Some(FileAction::Download);
                    }
                    if ui.button("Show in folder").clicked() {
                        chosen = Some(FileAction::Reveal);
                    }
                    ui.separator();
                    if ui.button("Copy path").clicked() {
                        chosen = Some(FileAction::CopyPath);
                    }
                    if ui.button("Save a copy").clicked() {
                        chosen = Some(FileAction::SaveCopy);
                    }
                });
            }
            ui.data_mut(|data| data.insert_temp(options_id, options && chosen.is_none()));
        });
    chosen
}

pub(super) fn markdown(ui: &mut egui::Ui, text: &str, actions: bool) -> Option<CodeAction> {
    markdown_with_files(ui, text, actions, &[], &mut ChatText::default(), &mut |_, _| {})
}

enum Piece {
    Job(egui::text::LayoutJob),
    File(String),
    Url(String, String),
}

struct Block {
    job: egui::text::LayoutJob,
    pieces: Vec<Piece>,
}

impl Block {
    fn split(&mut self) {
        if !self.job.text.is_empty() {
            self.pieces.push(Piece::Job(std::mem::take(&mut self.job)));
        }
    }
    fn discard(&mut self) {
        self.job = Default::default();
        self.pieces.clear();
    }
    fn flush(&mut self, ui: &mut egui::Ui, action: &mut Option<CodeAction>, selection: &mut ChatText) {
        if self.pieces.is_empty() {
            if !self.job.text.is_empty() {
                selection.label(ui, std::mem::take(&mut self.job), false);
            }
            return;
        }
        self.split();
        let pieces = std::mem::take(&mut self.pieces);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.;
            for piece in pieces {
                match piece {
                    Piece::Job(job) => {
                        selection.label(ui, job, false);
                    }
                    Piece::File(name) => {
                        if selection.label(ui, Text::new(&name).monospace().color(accent()), true)
                            .on_hover_text("Open in the side panel")
                            .clicked()
                        {
                            *action = Some(CodeAction::OpenFile(name));
                        }
                    }
                    Piece::Url(label, url) => {
                        selection.url(ui, Text::new(label).color(accent()).into(), url);
                    }
                }
            }
        });
    }
}

/// A spreadsheet cell with the formatting the file gave it. Numbers line up on the right, as in
/// Excel, unless the file says otherwise. Black or white font colours are left to the theme so
/// text stays readable in light and dark mode.
fn styled_cell(ui: &mut egui::Ui, cell: &str, style: Option<&crate::documents::CellStyle>) {
    let mut text = Text::new(cell);
    let mut fill = None;
    let mut align = None;
    if let Some(s) = style {
        if s.bold {
            text = text.strong();
        }
        if s.italic {
            text = text.italics();
        }
        if let Some([r, g, b]) = s.color {
            let light = u16::from(r) + u16::from(g) + u16::from(b);
            if (60..700).contains(&light) {
                text = text.color(Color::from_rgb(r, g, b));
            }
        }
        if let Some([r, g, b]) = s.fill {
            if !(r > 245 && g > 245 && b > 245) {
                fill = Some(Color::from_rgb(r, g, b).gamma_multiply(0.45));
            }
        }
        align = s.align;
    }
    let numeric = {
        let t = cell
            .trim()
            .trim_start_matches(['$', '€', '£', '-'])
            .trim_end_matches('%')
            .replace(',', "");
        !t.is_empty() && t.parse::<f64>().is_ok()
    };
    let layout = match align.unwrap_or(if numeric { 'r' } else { 'l' }) {
        'r' => egui::Layout::right_to_left(egui::Align::Center),
        'c' => egui::Layout::top_down(egui::Align::Center),
        _ => egui::Layout::left_to_right(egui::Align::Center),
    };
    egui::Frame::new()
        .fill(fill.unwrap_or(egui::Color32::TRANSPARENT))
        .inner_margin(egui::Margin::symmetric(3, 1))
        .show(ui, |ui| {
            ui.with_layout(layout, |ui| {
                ui.add(egui::Label::new(text).wrap().selectable(true));
            });
        });
}

/// The file a "Saved file: NAME" or "Attached file: NAME (…)" line names, if it is one.
pub(crate) fn file_marker(line: &str) -> Option<&str> {
    let line = line.trim();
    let rest = line
        .strip_prefix("Saved file: ")
        .or_else(|| line.strip_prefix("Attached file: "))?;
    Some(match rest.split_once(" (your tools") {
        Some((name, _)) => name,
        None => rest,
    })
}

/// Put each "Saved file:" marker in its own paragraph so it can become a card.
fn isolate_file_markers<'a>(text: &'a str, files: &[String]) -> std::borrow::Cow<'a, str> {
    if files.is_empty() {
        return text.into();
    }
    let mut out = String::with_capacity(text.len() + 32);
    for line in text.split_inclusive('\n') {
        let marker = file_marker(line).is_some_and(|n| files.iter().any(|f| f == n));
        if marker {
            out.push_str("\n\n");
            out.push_str(line.trim());
            out.push_str("\n\n");
        } else {
            out.push_str(line);
        }
    }
    out.into()
}

/// Render Markdown. Files named in `files` become links where mentioned in code spans or
/// links, and a "Saved file: NAME" paragraph is replaced by `card(ui, NAME)`.
pub(super) fn markdown_with_files(
    ui: &mut egui::Ui,
    text: &str,
    actions: bool,
    files: &[String],
    selection: &mut ChatText,
    card: &mut dyn FnMut(&mut egui::Ui, &str),
) -> Option<CodeAction> {
    let text = isolate_file_markers(text, files);
    let mut action = None;
    let mut ordinal = 0;
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
    let mut style = egui::TextFormat {
        font_id: egui::TextStyle::Body.resolve(ui.style()),
        color: ink(),
        ..Default::default()
    };
    let code_style = egui::TextFormat {
        font_id: egui::FontId::monospace(style.font_id.size - 1.),
        background: raised(),
        ..style.clone()
    };
    let mut stack = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut block = Block {
        job: Default::default(),
        pieces: vec![],
    };
    let mut paragraph = String::new();
    // An open link that becomes a clickable piece: (url, collected label, is a file).
    let mut link: Option<(String, String, bool)> = None;
    let mut quote = 0usize;
    let mut parser = Parser::new_ext(
        &text,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES,
    );
    while let Some(event) = parser.next() {
        if let Event::Start(Tag::Table(_)) = event {
            block.flush(ui, &mut action, selection);
            let mut rows: Vec<(bool, Vec<String>)> = vec![];
            let mut head = false;
            for inner in parser.by_ref() {
                match inner {
                    Event::End(TagEnd::Table) => break,
                    Event::Start(Tag::TableHead) => {
                        head = true;
                        rows.push((true, vec![]));
                    }
                    Event::End(TagEnd::TableHead) => head = false,
                    Event::Start(Tag::TableRow) => rows.push((head, vec![])),
                    Event::Start(Tag::TableCell) => {
                        if let Some(r) = rows.last_mut() {
                            r.1.push(String::new());
                        }
                    }
                    Event::Text(t) | Event::Code(t) => {
                        if let Some(cell) = rows.last_mut().and_then(|r| r.1.last_mut()) {
                            cell.push_str(&t);
                        }
                    }
                    Event::SoftBreak | Event::HardBreak => {
                        if let Some(cell) = rows.last_mut().and_then(|r| r.1.last_mut()) {
                            cell.push(' ');
                        }
                    }
                    _ => {}
                }
            }
            ui.push_id(("md-table", ordinal), |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1_f32, line()))
                        .corner_radius(6)
                        .inner_margin(6)
                        .show(ui, |ui| {
                            egui::Grid::new("cells")
                                .striped(true)
                                .spacing([14., 6.])
                                .max_col_width(280.)
                                .show(ui, |ui| {
                                    for (header, cells) in &rows {
                                        for cell in cells {
                                            let t = Text::new(cell);
                                            selection.label(ui, if *header { t.strong().color(strong()) } else { t }, false);
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                });
            });
            ordinal += 1;
            continue;
        }
        if let Event::Start(Tag::CodeBlock(kind)) = event {
            block.flush(ui, &mut action, selection);
            let info = match kind {
                pulldown_cmark::CodeBlockKind::Indented => String::new(),
                pulldown_cmark::CodeBlockKind::Fenced(i) => i.to_string(),
            };
            let language = info.split_whitespace().next().unwrap_or("Code");
            let mut body = String::new();
            for inner in parser.by_ref() {
                match inner {
                    Event::End(TagEnd::CodeBlock) => break,
                    Event::Text(t) => body.push_str(&t),
                    _ => {}
                }
            }
            let lines = body.lines().count();
            ui.push_id(ordinal, |ui| {
                egui::Frame::new()
                    .fill(raised())
                    .stroke(egui::Stroke::new(1_f32, line()))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        let id = ui.make_persistent_id("code");
                        let collapse =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                id,
                                lines <= 5,
                            );
                        collapse
                            .show_header(ui, |ui| {
                                ui.strong(language);
                                ui.weak(format!("{lines} lines"));
                                if ui.small_button("Copy").clicked() {
                                    ui.ctx().copy_text(body.clone());
                                }
                                if actions {
                                    if crate::artifacts::html(&info)
                                        && ui.small_button("Preview HTML").clicked()
                                    {
                                        action =
                                            Some(CodeAction::Preview(info.clone(), body.clone()));
                                    }
                                    if crate::artifacts::name(&info, "").is_some()
                                        && ui.small_button("Save file").clicked()
                                    {
                                        action = Some(CodeAction::Save(info.clone(), body.clone()));
                                    }
                                }
                            })
                            .body(|ui| {
                                egui::Frame::new()
                                    .fill(panel())
                                    .inner_margin(8)
                                    .show(ui, |ui| {
                                        selection.label(ui, Text::new(body.trim_end_matches('\n')).monospace(), false);
                                    });
                            });
                    });
            });
            ordinal += 1;
            continue;
        }
        match event {
            Event::Start(tag) => {
                stack.push(style.clone());
                match tag {
                    Tag::Paragraph => paragraph.clear(),
                    Tag::Heading { level, .. } => {
                        style.font_id.size = match level {
                            HeadingLevel::H1 => 23.,
                            HeadingLevel::H2 => 19.,
                            HeadingLevel::H3 => 17.,
                            _ => style.font_id.size,
                        };
                        style.color = strong();
                    }
                    Tag::BlockQuote(_) => {
                        block.flush(ui, &mut action, selection);
                        quote += 1;
                        style.color = muted();
                        style.italics = true;
                    }
                    Tag::Emphasis => style.italics = true,
                    Tag::Strong => style.color = strong(),
                    Tag::Strikethrough => style.strikethrough = egui::Stroke::new(1_f32, muted()),
                    Tag::CodeBlock(_) => style = code_style.clone(),
                    Tag::Link { dest_url, .. } => {
                        let url = dest_url.to_string();
                        let decoded = crate::html_preview::decode(&url).unwrap_or_else(|| url.clone());
                        let local = decoded.trim_start_matches("./");
                        let file = files.iter().find(|f| f.as_str() == local).or_else(|| {
                            let name = local.strip_prefix("artifacts/")?;
                            files.iter().find(|f| f.as_str() == name)
                        });
                        if let Some(file) = file {
                            link = Some((file.clone(), String::new(), true));
                        } else if url.starts_with("https://") || url.starts_with("http://") {
                            link = Some((url, String::new(), false));
                        } else {
                            style.color = accent();
                        }
                    }
                    Tag::List(start) => {
                        block.flush(ui, &mut action, selection);
                        lists.push(start);
                    }
                    Tag::Item => {
                        let indent = "    ".repeat(lists.len().saturating_sub(1));
                        let prefix = match lists.last_mut() {
                            Some(Some(n)) => {
                                let mark = format!("{n}. ");
                                *n = n.saturating_add(1);
                                mark
                            }
                            _ => "• ".into(),
                        };
                        block
                            .job
                            .append(&format!("{indent}{prefix}"), 0., style.clone());
                    }
                    _ => {}
                }
                if quote > 0 && matches!(block.job.text.as_str(), "") && block.pieces.is_empty() {
                    block.job.append("    ", 0., style.clone());
                }
            }
            Event::End(tag) => {
                if tag == TagEnd::Link {
                    if let Some((url, label, file)) = link.take() {
                        block.split();
                        block.pieces.push(if file {
                            Piece::File(url)
                        } else {
                            Piece::Url(if label.is_empty() { url.clone() } else { label }, url)
                        });
                    }
                }
                if tag == TagEnd::Paragraph {
                    if let Some(name) =
                        file_marker(&paragraph).filter(|n| files.iter().any(|f| f == n))
                    {
                        block.discard();
                        card(ui, name);
                    }
                }
                if matches!(
                    tag,
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item
                ) {
                    block.flush(ui, &mut action, selection);
                }
                if matches!(tag, TagEnd::BlockQuote(_)) {
                    block.flush(ui, &mut action, selection);
                    quote = quote.saturating_sub(1);
                }
                if tag == TagEnd::List(true) || tag == TagEnd::List(false) {
                    lists.pop();
                }
                if let Some(previous) = stack.pop() {
                    style = previous;
                }
            }
            Event::Text(t) => {
                if !files.is_empty() { paragraph.push_str(&t); }
                if let Some((_, label, _)) = &mut link {
                    label.push_str(&t);
                } else {
                    block.job.append(&t, 0., style.clone());
                }
            }
            Event::Code(t) => {
                if !files.is_empty() { paragraph.push_str(&t); }
                if files.iter().any(|f| **f == *t) && link.is_none() {
                    block.split();
                    block.pieces.push(Piece::File(t.to_string()));
                } else if let Some((_, label, _)) = &mut link {
                    label.push_str(&t);
                } else {
                    block.job.append(&t, 0., code_style.clone());
                }
            }
            Event::SoftBreak => block.job.append(" ", 0., style.clone()),
            Event::HardBreak => block.job.append("\n", 0., style.clone()),
            Event::TaskListMarker(done) => {
                block
                    .job
                    .append(if done { "[x] " } else { "[ ] " }, 0., code_style.clone())
            }
            Event::Rule => {
                block.flush(ui, &mut action, selection);
                ui.separator();
            }
            _ => {}
        }
    }
    block.flush(ui, &mut action, selection);
    action
}

#[cfg(test)]
mod tests {
    #[test]
    fn saved_file_markers_become_their_own_paragraphs() {
        let files = vec!["model_metrics.csv".to_owned()];
        let text = "Here it is.\nSaved file: model_metrics.csv\nIt has three columns.";
        let out = super::isolate_file_markers(text, &files);
        assert!(out.contains("\n\nSaved file: model_metrics.csv\n\n"));
        // Unknown names stay inline text.
        let other = super::isolate_file_markers("Saved file: other.csv\n", &files);
        assert_eq!(other, "Saved file: other.csv\n");
    }
}
