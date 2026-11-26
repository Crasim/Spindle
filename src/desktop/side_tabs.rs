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

/// A web browser beside the chat. On Windows it's a WebView2 page; elsewhere Spindle opens
/// pages in the system browser.
#[derive(Default)]
pub(super) struct BrowserPane {
    address: String,
    error: String,
    #[cfg(windows)]
    web: Option<wry::WebView>,
    #[cfg(windows)]
    visible: bool,
    #[cfg(windows)]
    bounds: Option<wry::Rect>,
    #[cfg(windows)]
    current: std::sync::Arc<std::sync::Mutex<String>>,
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
    pub fn hide(&mut self) {
        #[cfg(windows)]
        if self.visible
            && let Some(web) = &self.web
        {
            self.visible = web.set_visible(false).is_err();
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let mut go = None;
        let mut typing = false;
        ui.horizontal(|ui| {
            #[cfg(windows)]
            if let Some(web) = &self.web {
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
                // A browser for the user, not the model: ordinary web pages, no access to
                // Spindle or the workspace, and no downloads.
                let web = wry::WebViewBuilder::new()
                    .with_url(&url)
                    .with_focused(true)
                    .with_devtools(true)
                    .with_bounds(bounds)
                    .with_download_started_handler(|_, _| false)
                    .with_navigation_handler(move |url| {
                        let ok = url.starts_with("http://")
                            || url.starts_with("https://")
                            || url.starts_with("about:");
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
                    Err(e) => self.error = format!("WebView2 could not start the browser: {e}"),
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
                self.address = url.clone();
                self.error = match webbrowser::open(&url) {
                    Ok(()) => String::new(),
                    Err(e) => format!("Could not open the page: {e}"),
                };
            }
            ui.add_space(12.);
            ui.weak("On this system pages open in your own browser. The built-in browser is part of the Windows build.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::to_url;

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
}
