//! The right panel's other tabs: this chat's files, the project library, and a browser.
//!
//! The Viewer tab (documents and HTML previews) lives in `reader.rs`; this file holds the
//! rest. Tabs hand back what the user picked as a [`Pick`], and the desktop opens it in the
//! Viewer the same way a file card in the chat would.

use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Viewer,
    Files,
    Library,
    Browser,
    Plan,
}

impl Tab {
    pub const ALL: [(Tab, &'static str, &'static str); 5] = [
        (Tab::Viewer, "Viewer", "Documents and previews"),
        (
            Tab::Files,
            "Files",
            "Files attached to and made in this chat",
        ),
        (
            Tab::Library,
            "Library",
            "Every file in this chat's project or workspace",
        ),
        (Tab::Browser, "Browser", "Browse the web beside the chat"),
        (Tab::Plan, "Plan", "View and edit this conversation’s plan"),
    ];
}

/// A file attached to or made in the current chat.
pub(super) struct ChatFile {
    pub name: String,
    /// Attached by the user (rather than made by a model or tool).
    pub input: bool,
    pub disk: Option<PathBuf>,
    pub text: String,
    pub size: usize,
}

/// What the user picked in a tab.
pub(super) enum Pick {
    PlanSteps(Vec<crate::plan::Step>),
    RevisePlan { change: String, replace_goal: bool },
    DisconnectBrowser(u64),
    #[cfg(windows)]
    ShareBrowserPage { id: u64, snapshot: String },
    #[cfg(windows)]
    PanelResult { id: u64, snapshot: Option<serde_json::Value>, error: String },
    /// Open a chat file: from disk if it's there, or else its snapshot.
    ChatFile {
        name: String,
        disk: Option<PathBuf>,
        text: String,
    },
    /// Open a file from the library.
    Path(PathBuf),
    /// Show a file's folder.
    Reveal(PathBuf),
}

pub(super) fn files_tab(ui: &mut egui::Ui, files: &[ChatFile]) -> Option<Pick> {
    let mut pick = None;
    if files.is_empty() {
        ui.add_space(12.);
        ui.label(Text::new("No files in this chat yet").strong());
        ui.weak("Files you attach and files the model makes show up here.");
        return None;
    }
    egui::ScrollArea::vertical()
        .id_salt("chat-files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (input, heading) in [(true, "Inputs"), (false, "Outputs")] {
                let group: Vec<&ChatFile> = files.iter().filter(|f| f.input == input).collect();
                if group.is_empty() {
                    continue;
                }
                ui.add_space(6.);
                ui.label(
                    Text::new(format!("{heading} · {}", group.len()))
                        .strong()
                        .color(muted()),
                );
                for f in group {
                    ui.push_id(("chat-file", input, &f.name), |ui| {
                        if let Some(p) = file_row(ui, &f.name, f.size as u64, f.disk.as_deref()) {
                            pick = Some(match p {
                                RowAction::Open => Pick::ChatFile {
                                    name: f.name.clone(),
                                    disk: f.disk.clone(),
                                    text: f.text.clone(),
                                },
                                RowAction::Reveal(p) => Pick::Reveal(p),
                            });
                        }
                    });
                }
            }
        });
    pick
}

enum RowAction {
    Open,
    Reveal(PathBuf),
}

fn file_row(
    ui: &mut egui::Ui,
    name: &str,
    size: u64,
    disk: Option<&std::path::Path>,
) -> Option<RowAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        let shown = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let open = ui
            .add(
                egui::Button::new(Text::new(shown).size(13.))
                    .frame(false)
                    .truncate(),
            )
            .on_hover_text(format!("{name}\nOpen in the viewer"));
        if open.clicked() {
            action = Some(RowAction::Open);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(path) = disk
                && ui
                    .small_button("Folder")
                    .on_hover_text("Show in its folder")
                    .clicked()
            {
                action = Some(RowAction::Reveal(path.to_owned()));
            }
            if size > 0 {
                ui.weak(size_text(size));
            }
        });
    });
    action
}

fn size_text(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} KB", n as f64 / 1024.),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.),
    }
}

/// Files in a folder, for the Library tab.
#[derive(Default)]
pub(super) struct Library {
    root: PathBuf,
    files: Vec<(String, u64)>,
    filter: String,
    truncated: bool,
}

const LIBRARY_MAX: usize = 2000;
const SKIP: [&str; 7] = [
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    ".spindle",
    "_attachments",
];

impl Library {
    fn scan(&mut self, root: &std::path::Path) {
        self.root = root.to_owned();
        self.files.clear();
        self.truncated = false;
        let mut stack = vec![(root.to_owned(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if depth < 8 && !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                        stack.push((entry.path(), depth + 1));
                    }
                } else if kind.is_file() {
                    if self.files.len() >= LIBRARY_MAX {
                        self.truncated = true;
                        return;
                    }
                    let rel = entry
                        .path()
                        .strip_prefix(root)
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or(name);
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    self.files.push((rel, size));
                }
            }
        }
        self.files.sort_by_key(|(n, _)| n.to_lowercase());
    }

    pub fn show(&mut self, ui: &mut egui::Ui, root: &std::path::Path, label: &str) -> Option<Pick> {
        if self.root != root {
            self.scan(root);
        }
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.label(Text::new(label).strong())
                .on_hover_text(root.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Refresh").clicked() {
                    self.scan(root);
                }
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter files")
                .desired_width(f32::INFINITY),
        );
        let filter = self.filter.to_lowercase();
        let shown: Vec<&(String, u64)> = self
            .files
            .iter()
            .filter(|(n, _)| filter.is_empty() || n.to_lowercase().contains(&filter))
            .collect();
        ui.weak(format!(
            "{} file{}{}",
            shown.len(),
            if shown.len() == 1 { "" } else { "s" },
            if self.truncated {
                format!(" · first {LIBRARY_MAX} only")
            } else {
                String::new()
            }
        ));
        egui::ScrollArea::vertical()
            .id_salt("library-files")
            .auto_shrink([false, false])
            .show_rows(ui, 22., shown.len(), |ui, rows| {
                for (name, size) in &shown[rows] {
                    let path = root.join(name);
                    ui.push_id(("library", name), |ui| {
                        match file_row(ui, name, *size, Some(&path)) {
                            Some(RowAction::Open) => pick = Some(Pick::Path(path)),
                            Some(RowAction::Reveal(p)) => pick = Some(Pick::Reveal(p)),
                            None => {}
                        }
                    });
                }
            });
        pick
    }
}

pub(super) struct BrowserPane {
    tabs: Vec<BrowserTab>,
    selected: usize,
}

impl Default for BrowserPane {
    fn default() -> Self {
        Self { tabs: vec![BrowserTab::default()], selected: 0 }
    }
}

#[derive(PartialEq)]
enum BrowserSource {
    Public { chat: u64, address: String },
    Linked { chat: u64 },
}

#[derive(Default)]
struct BrowserTab {
    #[cfg(not(windows))]
    navigating: Option<std::sync::mpsc::Receiver<anyhow::Result<String>>>,
    source: Option<BrowserSource>,
    address: String,
    error: String,
    model_page: Option<(String, String)>,
    activity: Option<crate::execution::BrowserActivity>,
    #[cfg(windows)]
    web: Option<wry::WebView>,
    #[cfg(windows)]
    profile: Option<tempfile::TempDir>,
    #[cfg(windows)]
    network: Option<std::sync::Arc<crate::public_web::PanelProxy>>,
    #[cfg(windows)]
    visible: bool,
    #[cfg(windows)]
    bounds: Option<wry::Rect>,
    #[cfg(windows)]
    current: std::sync::Arc<std::sync::Mutex<String>>,
    #[cfg(windows)]
    pending_read: Option<PendingRead>,
}

#[cfg(windows)]
struct PendingRead {
    id: u64,
    proxy: Option<u16>,
    address: String,
    started: std::time::Instant,
    result: Option<std::sync::mpsc::Receiver<String>>,
    loaded: std::sync::Arc<std::sync::atomic::AtomicBool>,
    marked: Option<std::time::Instant>,
}

/// Turn what the user typed into a URL: add https:// to a bare host, and search for anything
/// that isn't one.
pub(super) fn to_url(typed: &str) -> String {
    let t = typed.trim();
    if t.contains("://") || t.starts_with("about:") {
        return t.to_owned();
    }
    let host = t.split('/').next().unwrap_or("");
    let looks_like_host = !t.contains(' ')
        && (host.contains('.') || host.starts_with("localhost") || host.contains(':'));
    if looks_like_host {
        let name = host.split(':').next().unwrap_or(host);
        let local = name == "localhost"
            || name.ends_with(".local")
            || name.ends_with(".lan")
            || name
                .parse::<std::net::Ipv4Addr>()
                .is_ok_and(|ip| ip.is_private() || ip.is_loopback());
        format!("{}://{t}", if local { "http" } else { "https" })
    } else {
        let mut q = String::new();
        for b in t.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    q.push(b as char)
                }
                b' ' => q.push('+'),
                _ => q.push_str(&format!("%{b:02X}")),
            }
        }
        format!("https://duckduckgo.com/?q={q}")
    }
}

impl BrowserPane {
    /// Opening and linking may select a tab. Finishing a background load only updates it.
    pub fn present(&mut self, page: crate::execution::BrowserPanel, foreground: bool) -> bool {
        use crate::execution::BrowserActivity;
        let starting = matches!(page.activity, BrowserActivity::Opening | BrowserActivity::Linked);
        #[cfg(windows)]
        let starting = starting || matches!(page.activity, BrowserActivity::Reading { .. });
        let source = if matches!(page.activity, BrowserActivity::Linked) {
            BrowserSource::Linked { chat: page.chat }
        } else {
            BrowserSource::Public { chat: page.chat, address: page.address.clone() }
        };
        let index = match self.tabs.iter().position(|tab| tab.source.as_ref() == Some(&source)) {
            Some(index) => index,
            None if starting => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.source.is_none() && tab.address.is_empty()) {
                    self.tabs[index].source = Some(source);
                    index
                } else {
                    self.tabs.push(BrowserTab { source: Some(source), ..Default::default() });
                    self.tabs.len() - 1
                }
            }
            None => return false,
        };
        self.tabs[index].present(page);
        if foreground && starting {
            self.select(index);
        }
        foreground && starting
    }

    pub fn disconnected(&mut self, chat: u64) {
        for tab in &mut self.tabs {
            if tab.source == Some(BrowserSource::Linked { chat }) {
                tab.activity = Some(crate::execution::BrowserActivity::Unavailable("Browser Link disconnected.".into()));
            }
        }
    }

    #[cfg(windows)]
    pub fn reveal_public(&mut self, chat: u64, address: String) {
        use crate::execution::{BrowserActivity, BrowserPanel};
        let source = BrowserSource::Public { chat, address: address.clone() };
        if let Some(index) = self.tabs.iter().position(|tab| tab.source.as_ref() == Some(&source)) {
            self.select(index);
        } else {
            self.present(BrowserPanel { chat, address: address.clone(), activity: BrowserActivity::Opening }, true);
            self.present(BrowserPanel { chat, address,
                activity: BrowserActivity::Unavailable("The isolated browser could not read this page.".into()) }, true);
        }
    }

    pub fn hide(&mut self) {
        self.tabs[self.selected].hide();
    }

    fn select(&mut self, index: usize) {
        if index != self.selected {
            self.hide();
            self.selected = index;
        }
    }

    fn close(&mut self, index: usize) {
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.tabs.push(BrowserTab::default());
        }
        self.selected = (self.selected - usize::from(index < self.selected)).min(self.tabs.len() - 1);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame,
        chat: u64, fallbacks: &[(u64, String)], ready: bool, browser_path: &str) -> Option<Pick> {
        let mut select = None;
        let mut close = None;
        let new_tab = ui.horizontal(|ui| {
            let new_tab = ui.small_button("+").on_hover_text("New browser tab").clicked();
            egui::ScrollArea::horizontal().id_salt("browser-tabs").show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, tab) in self.tabs.iter().enumerate() {
                        let shown = tab.label();
                        let label: String = shown.chars().take(32).collect();
                        let label = if label.len() < shown.len() { format!("{label}…") } else { label };
                        if ui.selectable_label(index == self.selected, label).on_hover_text(&tab.address).clicked() {
                            select = Some(index);
                        }
                        if ui.small_button("×").on_hover_text("Close browser tab").clicked() {
                            close = Some(index);
                        }
                    }
                });
            });
            new_tab
        }).inner;
        if let Some(index) = close {
            #[cfg(windows)]
            let pending = self.tabs[index].pending_read.take().filter(|r| r.proxy.is_some());
            self.close(index);
            #[cfg(windows)]
            if let Some(read) = pending {
                return Some(Pick::PanelResult { id: read.id, snapshot: None, error: "You closed the browser tab.".into() });
            }
        } else if new_tab {
            self.tabs.push(BrowserTab::default());
            self.select(self.tabs.len() - 1);
        } else if let Some(index) = select {
            self.select(index);
        }
        ui.add_space(4.);
        let tab = &mut self.tabs[self.selected];
        let retry = if let Some(BrowserSource::Public { chat: owner, address }) = &tab.source
            && *owner == chat {
            fallbacks.iter().find(|(_, url)| url == address)
        } else { None };
        ui.push_id(self.selected, |ui| tab.show(ui, frame, retry, ready, browser_path)).inner
    }
}

impl BrowserTab {
    fn reading_pointer(ui: &mut egui::Ui) {
        let id = egui::Id::new("spindle-browser-pointer");
        let cached = ui.ctx().data(|data| data.get_temp::<egui::TextureHandle>(id));
        let texture = cached.unwrap_or_else(|| {
            let image = image::load_from_memory(crate::browser::POINTER_PNG).expect("bundled browser pointer").to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            let texture = ui.ctx().load_texture("Spindle browser pointer", pixels, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        });
        ui.image((texture.id(), egui::vec2(34.65, 44.)));
    }

    fn label(&self) -> String {
        if matches!(self.source, Some(BrowserSource::Linked { .. })) {
            return "Browser Link".into();
        }
        if let Ok(url) = reqwest::Url::parse(&self.address)
            && let Some(host) = url.host_str()
        {
            return format!("{host}{}", url.path().trim_end_matches('/'));
        }
        if self.address.is_empty() { "New tab".into() } else { self.address.clone() }
    }

    fn present(&mut self, page: crate::execution::BrowserPanel) {
        use crate::execution::BrowserActivity;
        self.address = page.address;
        self.error.clear();
        self.model_page = None;
        self.activity = None;
        #[cfg(windows)]
        {
            self.pending_read = None;
            if let Some(web) = &self.web {
                let _ = web.evaluate_script("window.dispatchEvent(new Event('spindle-read-finished'))");
            }
            if matches!(page.activity, BrowserActivity::Opening) && self.network.is_some() {
                self.web = None;
                self.network = None;
                self.profile = None;
                self.visible = false;
                self.current = Default::default();
            }
        }
        match page.activity {
            #[cfg(windows)]
            BrowserActivity::Loaded(_) | BrowserActivity::Unavailable(_) if self.profile.is_some() && self.web.is_some() => {
                // The live document is already here, including any site-generated error.
                // Re-navigating would show a different page from the one the model read.
            }
            BrowserActivity::Loaded(text) => self.model_page = Some((self.address.clone(), text)),
            activity => {
                self.hide();
                self.activity = Some(activity);
            }
        }
    }

    fn hide(&mut self) {
        #[cfg(windows)]
        if self.visible
            && let Some(web) = &self.web
        {
            self.visible = web.set_visible(false).is_err();
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame,
        retry: Option<&(u64, String)>, ready: bool, browser_path: &str) -> Option<Pick> {
        #[cfg(windows)]
        let _ = browser_path;
        #[cfg(not(windows))]
        let _ = (retry, ready);
        #[cfg(windows)]
        if let Some(pick) = self.poll_shared_page(ui, retry.map(|(id, _)| *id), ready) {
            return Some(pick);
        }
        let mut go = None;
        #[cfg(windows)]
        let mut navigation_result = None;
        #[cfg(windows)]
        let mut share_here = false;
        #[cfg(windows)]
        let mut direct = None;
        let mut typing = false;
        ui.horizontal(|ui| {
            #[cfg(windows)]
            if self.activity.is_none() && let Some(web) = &self.web {
                if ui.small_button("‹").on_hover_text("Back").clicked() {
                    let _ = web.evaluate_script("history.back()");
                }
                if ui.small_button("›").on_hover_text("Forward").clicked() {
                    let _ = web.evaluate_script("history.forward()");
                }
                if ui.small_button("⟳").on_hover_text("Reload").clicked() {
                    let _ = web.reload();
                }
            }
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.address)
                    .hint_text("Address or search")
                    .desired_width(ui.available_width() - 40.),
            );
            typing = field.has_focus();
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.small_button("Go").clicked() || enter) && !self.address.trim().is_empty() {
                go = Some(to_url(&self.address));
            }
        });
        if go.is_some() {
            self.activity = None;
            self.source = None;
            self.model_page = None;
            #[cfg(windows)]
            {
                let pending = self.pending_read.take();
                if self.network.take().is_some() {
                    self.web = None;
                    self.profile = None;
                    self.visible = false;
                    self.current = Default::default();
                }
                if let Some(read) = pending.filter(|r| r.proxy.is_some()) {
                    navigation_result = Some(Pick::PanelResult { id: read.id, snapshot: None,
                        error: "You navigated away during the browser read.".into() });
                }
            }
        } else if let Some(activity) = &self.activity {
            use crate::execution::BrowserActivity;
            ui.add_space(8.);
            match activity {
                #[cfg(windows)]
                BrowserActivity::Reading { id, proxy } => {
                    direct = Some((*id, proxy.port));
                    self.network = Some(proxy.clone());
                    go = Some(self.address.clone());
                }
                BrowserActivity::Opening => {
                    ui.horizontal_wrapped(|ui| {
                        Self::reading_pointer(ui);
                        ui.label("Spindle is opening this page…");
                    });
                }
                BrowserActivity::Unavailable(reason) => {
                    #[cfg(windows)]
                    if let Some((_, address)) = retry {
                        ui.label("Open the page in this panel and share its visible text and links with this chat.");
                        share_here = ui.add_enabled(ready, egui::Button::new("Open and share here")).clicked();
                        if share_here { go = Some(address.clone()); }
                    }
                    if go.is_none() {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.add(egui::Label::new(reason).wrap().selectable(true));
                        });
                    }
                }
                BrowserActivity::Linked => {
                    ui.strong("Browser Link");
                    ui.label("Spindle is using the tab in your regular browser. Press Esc in that tab to revoke access.");
                    if ui.button("Disconnect").clicked()
                        && let Some(BrowserSource::Linked { chat }) = self.source
                    {
                        return Some(Pick::DisconnectBrowser(chat));
                    }
                }
                BrowserActivity::Loaded(_) => {}
            }
            if go.is_none() { return None; }
        }
        #[cfg(windows)]
        if direct.is_some() || share_here {
            let (id, proxy) = direct.map(|(id, port)| (id, Some(port)))
                .unwrap_or_else(|| (retry.expect("explicit retry").0, None));
            self.hide();
            self.web = None;
            self.profile = None;
            self.visible = false;
            self.current = Default::default();
            self.activity = None;
            self.pending_read = Some(PendingRead { id, proxy, address: self.address.clone(),
                started: std::time::Instant::now(), result: None, loaded: Default::default(), marked: None });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
        #[cfg(windows)]
        let automatic = self.model_page.take().filter(|_| go.is_none());
        #[cfg(windows)]
        let from_model = automatic.is_some();
        #[cfg(windows)]
        if let Some((url, _)) = automatic {
            go = Some(url);
        }
        #[cfg(windows)]
        // Follow the page's address while the user isn't typing in the field.
        if !typing
            && let Ok(now) = self.current.lock()
            && !now.is_empty()
            && *now != self.address
        {
            self.address = now.clone();
        }
        #[cfg(not(windows))]
        let _ = typing;
        if !self.error.is_empty() {
            ui.colored_label(accent(), &self.error);
        }
        #[cfg(windows)]
        if let Some(read) = &self.pending_read {
            let stop = ui.horizontal_wrapped(|ui| {
                if read.marked.is_none() { Self::reading_pointer(ui); }
                ui.label("Waiting for the requested page…");
                read.proxy.is_some() && ui.button("Stop reading").clicked()
            }).inner;
            if stop {
                return Some(Pick::PanelResult { id: self.pending_read.take()?.id, snapshot: None,
                    error: "You stopped this browser read.".into() });
            }
        }
        #[cfg(windows)]
        {
            let rect = ui.available_rect_before_wrap();
            let scale = ui.ctx().pixels_per_point();
            let bounds = wry::Rect {
                position: wry::dpi::PhysicalPosition::new(
                    (rect.min.x * scale).round() as i32,
                    (rect.min.y * scale).round() as i32,
                )
                .into(),
                size: wry::dpi::PhysicalSize::new(
                    (rect.width() * scale).max(1.) as u32,
                    (rect.height() * scale).max(1.) as u32,
                )
                .into(),
            };
            if let Some(url) = &go {
                self.address = url.clone();
                if let Some(web) = &self.web {
                    if let Err(e) = web.load_url(url) {
                        self.error = format!("Could not open the page: {e}");
                    } else {
                        self.error.clear();
                    }
                }
            }
            if let Some(web) = &self.web {
                if self.bounds != Some(bounds) && web.set_bounds(bounds).is_ok() {
                    self.bounds = Some(bounds);
                }
                if !self.visible {
                    self.visible = web.set_visible(true).is_ok();
                }
            } else if let Some(url) = go {
                let current = self.current.clone();
                let ctx = ui.ctx().clone();
                let loaded = self.pending_read.as_ref().map(|request| request.loaded.clone());
                // A browser for the user, not the model: ordinary web pages, no access to
                // Spindle or the workspace, and no downloads.
                let port = self.pending_read.as_ref().and_then(|read| read.proxy);
                let mut context;
                let builder = if let Some(port) = port {
                    use wry::WebViewBuilderExtWindows;
                    let profile = match tempfile::Builder::new().prefix("spindle-browser-").tempdir() {
                        Ok(profile) => profile,
                        Err(error) => return Some(Pick::PanelResult { id: self.pending_read.take()?.id,
                            snapshot: None, error: error.to_string() }),
                    };
                    context = wry::WebContext::new(Some(profile.path().to_owned()));
                    self.profile = Some(profile);
                    wry::WebViewBuilder::new_with_web_context(&mut context)
                        .with_additional_browser_args(format!("--proxy-server=http://127.0.0.1:{port} --proxy-bypass-list=<-loopback> --disable-quic --force-webrtc-ip-handling-policy=disable_non_proxied_udp --disable-background-networking"))
                } else { wry::WebViewBuilder::new() };
                let web = builder
                    .with_url(&url)
                    .with_incognito(share_here || port.is_some())
                    .with_focused(!from_model && port.is_none())
                    .with_devtools(port.is_none())
                    .with_bounds(bounds)
                    .with_download_started_handler(|_, _| false)
                    .with_on_page_load_handler(move |event, _| {
                        if let Some(loaded) = &loaded {
                            loaded.store(matches!(event, wry::PageLoadEvent::Finished), std::sync::atomic::Ordering::Relaxed);
                        }
                    })
                    .with_navigation_handler(move |url| {
                        let ok = if port.is_some() { crate::browser::public_page(&url).is_ok_and(|u| u.scheme() == "https") } else { url.starts_with("http://")
                            || url.starts_with("https://")
                            || url.starts_with("about:") };
                        if ok && let Ok(mut c) = current.lock() {
                            *c = url;
                            ctx.request_repaint();
                        }
                        ok
                    })
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .build_as_child(frame);
                match web {
                    Ok(w) => {
                        self.web = Some(w);
                        self.bounds = Some(bounds);
                        self.visible = true;
                        self.error.clear();
                    }
                    Err(e) => {
                        self.error = format!("WebView2 could not start the browser: {e}");
                        if port.is_some() {
                            return Some(Pick::PanelResult { id: self.pending_read.take()?.id, snapshot: None, error: self.error.clone() });
                        }
                    }
                }
            } else {
                ui.add_space(12.);
                ui.weak("Type an address or a search and press Enter.");
            }
        }
        #[cfg(not(windows))]
        {
            let _ = frame;
            if let Some(url) = go {
                self.model_page = None;
                self.address = url.clone();
                self.error.clear();
                let (send, receive) = std::sync::mpsc::channel();
                self.navigating = Some(receive);
                let browser_path = browser_path.to_owned();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let result = tokio::runtime::Runtime::new().map_err(anyhow::Error::from)
                        .and_then(|rt| rt.block_on(crate::execution::open_public(url, 0, false, false, &browser_path, Default::default())));
                    let _ = send.send(result);
                    ctx.request_repaint();
                });
            }
            if let Some(job) = &self.navigating {
                match job.try_recv() {
                    Ok(result) => {
                        self.navigating = None;
                        match result {
                            Ok(text) => self.model_page = Some((self.address.clone(), text)),
                            Err(error) => self.error = format!("Could not read this page. {error:#}"),
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => { ui.spinner(); }
                    Err(_) => { self.navigating = None; self.error = "Page read interrupted.".into(); }
                }
            }
            ui.add_space(12.);
            if let Some((_, text)) = &self.model_page {
                ui.weak("Page snapshot");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::Label::new(text).wrap().selectable(true));
                });
            } else {
                ui.weak("Enter a public address to read the page here.");
            }
        }
        #[cfg(windows)]
        { navigation_result }
        #[cfg(not(windows))]
        { None }
    }

    #[cfg(windows)]
    fn poll_shared_page(&mut self, ui: &mut egui::Ui, retry: Option<u64>, ready: bool) -> Option<Pick> {
        let mut pending = self.pending_read.take()?;
        let direct = pending.proxy.is_some();
        if !direct && retry != Some(pending.id) { return None; }
        // A site can briefly navigate to a verification or consent screen.
        // Wait for its normal return to the requested address before reading any text.
        let waiting_for_return = direct && self.current.lock().ok().is_some_and(|current| {
            !current.is_empty() && crate::browser::public_page(&current).is_ok_and(|mut url| {
                url.set_fragment(None);
                crate::browser::public_page(&pending.address).is_ok_and(|mut expected| {
                    expected.set_fragment(None); url != expected
                })
            })
        });
        let loaded = pending.loaded.load(std::sync::atomic::Ordering::Relaxed);
        if direct && loaded && pending.marked.is_none() && let Some(web) = &self.web {
            let _ = web.evaluate_script(&crate::browser::panel_cursor_script());
            pending.marked = Some(std::time::Instant::now());
        } else if !loaded {
            pending.marked = None;
        }
        let result = if waiting_for_return && pending.started.elapsed().as_secs() >= 30 {
            return Some(Pick::PanelResult { id: pending.id, snapshot: None,
                error: "The website redirected away from the requested page.".into() });
        } else if pending.marked.is_some_and(|mark| mark.elapsed().as_millis() < 500) {
            None
        } else if pending.started.elapsed().as_secs() >= 30 {
            Some(Err(anyhow::anyhow!("The page did not finish loading in time.")))
        } else if !ready && !direct {
            None
        } else if let Some(receiver) = &pending.result {
            match receiver.try_recv() {
                Ok(snapshot) if snapshot != "null" && snapshot.len() <= 300_000 => {
                    if direct {
                        let page = serde_json::from_str::<serde_json::Value>(&snapshot).ok();
                        let stopped = page.as_ref().is_some_and(|v| v["stopped"] == true);
                        return Some(Pick::PanelResult { id: pending.id, snapshot: page.filter(|v| v["address"].is_string()),
                            error: if stopped { "You stopped this browser read." } else { "The page did not return a readable document." }.into() });
                    }
                    Some(serde_json::from_str::<crate::browser::VisiblePage>(&snapshot)
                        .map_err(anyhow::Error::from)
                        .and_then(|page| page.read(&pending.address))
                        .map(|_| snapshot))
                }
                Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => { pending.result = None; None }
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            }
        } else if pending.loaded.load(std::sync::atomic::Ordering::Relaxed) && let Some(web) = &self.web {
            let (sender, receiver) = std::sync::mpsc::channel();
            let ctx = ui.ctx().clone();
            let script = if waiting_for_return {
                "document.documentElement.dataset.spindleReadStopped === '1' ? {stopped:true} : null"
            } else {
                crate::browser::VISIBLE_PAGE_SCRIPT
            };
            match web.evaluate_script_with_callback(script, move |snapshot| {
                let _ = sender.send(snapshot);
                ctx.request_repaint();
            }) {
                Ok(()) => { pending.result = Some(receiver); None }
                Err(error) => Some(Err(error.into())),
            }
        } else { None };
        match result {
            Some(Err(error)) if direct => Some(Pick::PanelResult { id: pending.id, snapshot: None, error: error.to_string() }),
            Some(Ok(snapshot)) => Some(Pick::ShareBrowserPage { id: pending.id, snapshot }),
            Some(Err(error)) => {
                self.hide();
                self.activity = Some(crate::execution::BrowserActivity::Unavailable(format!("Could not share this page. {error}")));
                None
            }
            None => {
                self.pending_read = Some(pending);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserPane, to_url};
    use crate::execution::{BrowserActivity, BrowserPanel};

    #[test]
    fn addresses_become_urls() {
        assert_eq!(to_url("example.com"), "https://example.com");
        assert_eq!(
            to_url("model-host.local:8188"),
            "http://model-host.local:8188"
        );
        assert_eq!(to_url("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(to_url("https://a.b/c"), "https://a.b/c");
        assert_eq!(
            to_url("rust egui panels"),
            "https://duckduckgo.com/?q=rust+egui+panels"
        );
    }

    #[test]
    fn browser_results_update_their_own_tab_without_changing_selection() {
        let mut pane = BrowserPane::default();
        let page = |address: &str, activity| BrowserPanel { chat: 7, address: address.into(), activity };
        let first = "https://example.com/one";
        let second = "https://example.com/two";
        assert!(pane.present(page(first, BrowserActivity::Opening), true));
        assert!(pane.present(page(second, BrowserActivity::Opening), true));
        assert!(!pane.present(page(first, BrowserActivity::Loaded("First page".into())), true));
        assert_eq!(pane.tabs.len(), 2);
        assert_eq!(pane.selected, 1);
        assert_eq!(pane.tabs[0].model_page, Some((first.into(), "First page".into())));
        pane.close(1);
        assert!(!pane.present(page(second, BrowserActivity::Loaded("Second page".into())), true));
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.tabs[pane.selected].address, first);
    }
}
