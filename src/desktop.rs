use crate::{
    engine::{HELP, View},
    state::Mode,
};
use eframe::egui::{self, Color32 as Color, RichText as Text};
use std::path::Path;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
mod audio;
mod chat_selection;
mod audio_settings;
mod html_preview;
mod images;
mod message_edit;
mod notes;
mod paste;
mod submission;
mod android_ime;
mod project_view;
mod reader;
mod settings;
mod side_tabs;
mod sync_view;
mod tasks;
mod thread_view;

pub fn spindle_icon(accent: Accent) -> egui::IconData {
    let png: &[u8] = match accent {
        Accent::Red => include_bytes!("../assets/accents/red/spindle-256.png"),
        Accent::Orange => include_bytes!("../assets/accents/orange/spindle-256.png"),
        Accent::Yellow => include_bytes!("../assets/accents/yellow/spindle-256.png"),
        Accent::Green => include_bytes!("../assets/accents/green/spindle-256.png"),
        Accent::Blue => include_bytes!("../assets/spindle-256.png"),
        Accent::Indigo => include_bytes!("../assets/accents/indigo/spindle-256.png"),
        Accent::Violet => include_bytes!("../assets/accents/violet/spindle-256.png"),
        Accent::Magenta => include_bytes!("../assets/accents/magenta/spindle-256.png"),
    };
    eframe::icon_data::from_png_bytes(png).expect("embedded Spindle accent icon")
}

/// A tray-hidden window needs a native wake-up before egui can process its close.
#[derive(Default)]
pub(crate) struct DesktopShutdown {
    finished: AtomicBool,
    #[cfg(any(windows, target_os = "linux"))]
    restarting: AtomicBool,
    #[cfg(any(windows, target_os = "linux"))]
    restart_error: OnceLock<String>,
    wake_window: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl DesktopShutdown {
    #[cfg(any(windows, target_os = "linux"))]
    pub(crate) fn restarting(&self) -> bool {
        self.restarting.load(Ordering::Acquire)
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn request_update_restart(&self, tx: &UnboundedSender<String>) -> anyhow::Result<()> {
        crate::updates::verify_restart()?;
        self.restarting.store(true, Ordering::Release);
        if tx.send("/exit".into()).is_err() {
            self.restarting.store(false, Ordering::Release);
            anyhow::bail!("The workspace engine has already stopped");
        }
        Ok(())
    }

    #[cfg(any(windows, target_os = "linux"))]
    pub(crate) fn cancel_update_restart(&self, error: &anyhow::Error) {
        if self.restarting() {
            let _ = self.restart_error.set(format!("{error:#}"));
            self.restarting.store(false, Ordering::Release);
        }
    }

    pub(crate) fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        if let Some(wake) = self.wake_window.get() {
            wake();
        }
    }
}

#[cfg(windows)]
fn window_handle(window: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    use raw_window_handle::RawWindowHandle;
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

struct SpindleLogo {
    accent: Accent,
    window: std::sync::Arc<egui::IconData>,
    texture: egui::TextureHandle,
}

impl SpindleLogo {
    fn new(ctx: &egui::Context, accent: Accent) -> Self {
        let icon = spindle_icon(accent);
        let texture = ctx.load_texture(
            "Spindle",
            egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            ),
            egui::TextureOptions::LINEAR,
        );
        Self {
            accent,
            window: icon.into(),
            texture,
        }
    }
}

/// Egui visuals for the current palette: no outline on selected items, rounded corners.
fn apply_style(ctx: &egui::Context, light: bool) {
    crate::widgets::set_light(light);
    ctx.set_theme(if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    });
    let mut style = (*ctx.style()).clone();
    style.visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    style.visuals.panel_fill = surface();
    style.visuals.extreme_bg_color = panel();
    style.visuals.window_fill = panel();
    style.visuals.faint_bg_color = raised();
    style.visuals.override_text_color = Some(ink());
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(15.));
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(16.));
    style.spacing.item_spacing = egui::vec2(9., 8.);
    style.spacing.button_padding = egui::vec2(12., 7.);
    style.spacing.interact_size.y = 30.;
    let corners = egui::CornerRadius::same(8);
    let widgets = &mut style.visuals.widgets;
    for w in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        w.corner_radius = corners;
        w.expansion = 0.;
    }
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, line());
    widgets.inactive.bg_fill = raised();
    widgets.inactive.weak_bg_fill = raised();
    // A thin outline, so fields (and buttons) stand out from the card behind them; text
    // fields draw it from these same settings.
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, line());
    for w in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        w.bg_fill = hover();
        w.weak_bg_fill = hover();
        w.bg_stroke = egui::Stroke::new(1.0_f32, muted().gamma_multiply(0.6));
        w.fg_stroke.color = ink();
    }
    // Selected items get a quiet fill and no outline; the stroke only colors their text.
    style.visuals.selection.bg_fill = highlight();
    style.visuals.selection.stroke = egui::Stroke::new(0.0_f32, ink());
    style.visuals.window_corner_radius = egui::CornerRadius::same(12);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(10);
    style.visuals.window_stroke = egui::Stroke::new(1.0_f32, line());
    ctx.set_style(style);
}

#[cfg(windows)]
fn caption(window: &impl raw_window_handle::HasWindowHandle, light: bool) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Graphics::Dwm::*;
    let Ok(window) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = window.as_raw() else {
        return;
    };
    let rgb = |c: Color| u32::from(c.r()) | (u32::from(c.g()) << 8) | (u32::from(c.b()) << 16);
    for (attr, value) in [
        (DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(!light)),
        (DWMWA_CAPTION_COLOR, rgb(surface())),
        (DWMWA_TEXT_COLOR, rgb(ink())),
        (DWMWA_BORDER_COLOR, rgb(line())),
    ] {
        // DWM copies this four-byte value during the call. The native window is owned by eframe.
        unsafe {
            DwmSetWindowAttribute(
                handle.hwnd.get() as _,
                attr as u32,
                (&value as *const u32).cast(),
                4,
            );
        }
    }
}
use crate::providers::{Connection, Protocol, route};
use crate::widgets::*;
use std::{path::PathBuf, sync::mpsc::Receiver, time::Duration};
use tokio::sync::mpsc::UnboundedSender;

#[derive(PartialEq)]
enum Page {
    Projects,
    Project,
    Chat,
    Queue,
    Automations,
    Skills,
    Settings,
}

fn panel_header(ui: &mut egui::Ui, title: &str, button: &str) -> bool {
    let closed = ui
        .horizontal_wrapped(|ui| {
            let closed = ui.button(button).clicked();
            ui.heading(title);
            closed
        })
        .inner
        || ui.input(|input| input.key_pressed(egui::Key::Escape));
    ui.separator();
    closed
}

fn detail_panel(
    ui: &mut egui::Ui,
    title: &str,
    close_label: &str,
    open: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    if *open {
        ui.set_max_width(ui.available_width().min(780.));
        if panel_header(ui, title, close_label) {
            *open = false;
        } else {
            egui::ScrollArea::vertical().id_salt(title).show(ui, body);
        }
    }
}

#[cfg(windows)]
fn export_skill(path: &std::path::Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("Choose a destination directory"))?,
    )?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(path)
        .map_err(|e| anyhow::anyhow!("Choose a new file name: {}", e.error))?;
    Ok(())
}

pub struct Desktop {
    /// On a phone-sized screen: the sidebar is shown full screen, as a drawer.
    drawer: bool,
    /// The Ollama address being typed in Settings (None: show the saved one).
    ollama_address: Option<String>,
    ollama_key: String,
    /// The page and chat when the drawer opened: moving to another closes it.
    drawer_at: Option<(std::mem::Discriminant<Page>, u64)>,
    reported_view: Option<u64>,
    message_editors: message_edit::MessageEditors,
    paste_drafts: paste::PasteDrafts,
    submissions: submission::Submissions,
    android_ime: android_ime::ComposerIme,
    threads: thread_view::Threads,
    drafts_by_thread: std::collections::BTreeMap<u64, String>,
    generating: Option<(u64, std::time::Instant, usize)>,
    tasks: tasks::TaskScreen,
    reader: reader::ReaderPane,
    images: images::ImagesForm,
    voice: audio::Voice,
    sync_form: sync_view::SyncForm,
    sync_tab: bool,
    tray: Option<crate::tray::Tray>,
    shutdown: Arc<DesktopShutdown>,
    voice_form: audio_settings::VoiceForm,
    projects: project_view::ProjectScreen,
    rx: Receiver<View>,
    preview: tokio::sync::watch::Receiver<String>,
    tx: UnboundedSender<String>,
    root: PathBuf,
    view: View,
    page: Page,
    draft: String,
    notice: String,
    focus: bool,
    details: bool,
    help: bool,
    /// Highlighted row in the slash-command list, and a draft the user dismissed it for.
    command_index: usize,
    commands_dismissed: String,
    swarm: bool,
    worker_models: [String; 2],
    worker_roles: [String; 2],
    assignment: String,
    heading_font: egui::FontFamily,
    logo: SpindleLogo,
    /// The palette last applied, so the style is rebuilt only when the theme changes.
    applied_light: Option<bool>,
    search: String,
    favorites_only: bool,
    select_focus: bool,
    /// Open the model picker on the next frame (from /model).
    controls_panel: Option<settings::ControlsPanel>,
    /// Images waiting to go with the next message: name, bytes and a thumbnail.
    image_drafts: Vec<submission::ImageDraft>,
    images_by_thread: std::collections::BTreeMap<u64, Vec<submission::ImageDraft>>,
    /// Copy the open conversation on the next frame (from /share).
    share_pending: bool,
    preferences_tab: bool,
    models_tab: bool,
    memory_tab: bool,
    sandbox_tab: bool,
    integration_tab: Option<settings::IntegrationTab>,
    browser_setup: settings::BrowserSetup,
    #[cfg(any(windows, target_os = "linux"))]
    updates: settings::UpdatePanel,
    connector_setup: settings::ConnectorSetup,
    local_servers: settings::LocalServers,
    browser_path_draft: Option<String>,
    chat_selection: chat_selection::Selection,
    /// Text to put on the clipboard on the next frame.
    pending_copy: Option<String>,
    /// The task being worked on and when it started, for the elapsed time.
    work_started: Option<(u64, std::time::Instant)>,
    /// Tool-step groups the user opened, by (chat, first message).
    open_steps: std::collections::HashSet<(u64, usize)>,
    /// The notice shown as a toast, and when it appeared.
    toast: Option<(String, std::time::Instant)>,
    /// When the zoom last changed, to save it once the user stops scrolling.
    zoom_changed: Option<std::time::Instant>,
    /// Asking whether to throw away the sandbox copy.
    discarding: bool,
    /// The sidebar width last sent to be saved, so a drag is saved once.
    sidebar_sent: Option<u16>,
    plan: notes::PlanPane,
    memory: notes::MemoryPage,
    git: notes::GitPane,
    components: notes::ComponentsPane,
    model_query: String,
    model_provider: Option<String>,
    tuning_editor: Option<(String, crate::state::Tuning)>,
    provider_setup: settings::ProviderSetup,
    connection: Connection,
    remove_provider: Option<String>,
    settings_feedback: String,

    builder: bool,
    skill_name: String,
    skill_brief: String,
}

/// Routine notices that would only be noise as a toast.
fn quiet_notice(n: &str) -> bool {
    n.is_empty()
        || matches!(n, "Response saved" | "Preferences saved" | "Conversation opened" | "Ollama connected" | "Connecting to Ollama…")
        || n.starts_with("Queued #")
        || n.starts_with("ext:")
        || n.starts_with("Pasted files queued")
        || n.ends_with("image(s) queued")
        || n.starts_with("Ready")
}

impl Desktop {
    pub(crate) fn attach_shutdown(
        &mut self,
        cc: &eframe::CreationContext<'_>,
        shutdown: Arc<DesktopShutdown>,
    ) {
        let ctx = cc.egui_ctx.clone();
        #[cfg(windows)]
        let hwnd = window_handle(cc);
        let _ = shutdown.wake_window.set(Box::new(move || {
            #[cfg(windows)]
            if let Some(hwnd) = hwnd {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetWindowThreadProcessId, IsWindowVisible, SW_SHOWMINNOACTIVE, ShowWindowAsync,
                };
                let hwnd = hwnd as _;
                unsafe {
                    let mut owner = 0;
                    GetWindowThreadProcessId(hwnd, &mut owner);
                    if owner == std::process::id() && IsWindowVisible(hwnd) == 0 {
                        ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE);
                    }
                }
            }
            ctx.request_repaint();
        }));
        self.shutdown = shutdown;
    }

    fn request_exit(&mut self, ctx: &egui::Context) {
        if self.tx.send("/exit".into()).is_err() {
            self.tray = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Show a new notice briefly in the status banner.
    fn show_notice(&mut self, ctx: &egui::Context) {
        if !quiet_notice(&self.notice) && self.toast.as_ref().is_none_or(|(n, _)| *n != self.notice)
        {
            self.toast = Some((self.notice.clone(), std::time::Instant::now()));
        }
        let Some((text, at)) = &self.toast else {
            return;
        };
        let error = text.starts_with("Error") || text.starts_with("Couldn't");
        let life = std::time::Duration::from_secs(if error { 8 } else { 4 });
        if at.elapsed() > life {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let text = text.clone();
        egui::TopBottomPanel::top("notice-banner")
            .frame(egui::Frame::new().fill(raised()).inner_margin(8))
            .show(ctx, |ui| {
                let width = (ui.available_width() - 90.).max(40.);
                ui.horizontal_top(|ui| {
                    if ui.small_button("Dismiss").clicked() {
                        self.toast = Some((text.clone(), std::time::Instant::now() - life));
                    }
                    let color = if error {
                        Color::from_rgb(230, 160, 90)
                    } else {
                        ink()
                    };
                    let mut message = egui::text::LayoutJob::simple(
                        text,
                        egui::FontId::proportional(13.),
                        color,
                        width,
                    );
                    message.wrap.max_rows = 3;
                    message.wrap.break_anywhere = true;
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| ui.add(egui::Label::new(message).wrap()),
                    );
                });
            });
    }
}

/// For each message, the first message of the run of tool steps it belongs to: replies that
/// called a tool and the results sent back. The final answer after a run stays outside it.
fn step_groups(state: &crate::state::State) -> Vec<Option<usize>> {
    let step = |i: usize| {
        if state.history[i].role == "user" {
            state.thread.tool_results.contains(&i)
        } else {
            state.thread.turns.get(&i).is_some_and(|t| t.tool.is_some())
        }
    };
    let n = state.history.len();
    let mut groups = vec![None; n];
    let mut i = 0;
    while i < n {
        if step(i) {
            let start = i;
            while i < n && step(i) {
                i += 1;
            }
            // Only runs with at least one tool call fold; a lone Spindle note shows as itself.
            let calls = (start..i).any(|k| {
                state.history[k].role != "user"
                    && state.thread.turns.get(&k).is_some_and(|t| t.tool.is_some())
            });
            if calls {
                for g in &mut groups[start..i] {
                    *g = Some(start);
                }
            }
        } else {
            i += 1;
        }
    }
    groups
}

/// Draw the box behind a group of tool steps, from `top` down to here.
fn close_group_box(ui: &mut egui::Ui, slot: egui::layers::ShapeIdx, top: f32) {
    ui.add_space(6.);
    let x = ui.max_rect().x_range();
    let rect = egui::Rect::from_x_y_ranges(x, top..=ui.cursor().top());
    ui.painter().set(
        slot,
        egui::epaint::RectShape::new(
            rect,
            10.,
            raised(),
            egui::Stroke::new(1_f32, line()),
            egui::StrokeKind::Inside,
        ),
    );
}

/// The folded line for a run of tool steps. Returns true when clicked.
fn steps_header(
    ui: &mut egui::Ui,
    state: &crate::state::State,
    groups: &[Option<usize>],
    start: usize,
    open: bool,
    working: bool,
) -> bool {
    let notes: Vec<&crate::state::ToolNote> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| **g == Some(start))
        .filter_map(|(i, _)| state.thread.turns.get(&i).and_then(|t| t.tool.as_ref()))
        .collect();
    let failed = notes
        .iter()
        .filter(|n| {
            let s = n.status.to_lowercase();
            s.contains("fail")
                || s.contains("not ")
                || s.contains("couldn't")
                || s.contains("denied")
                || s.contains("stopped")
        })
        .count();
    let mut names: Vec<&str> = vec![];
    for n in &notes {
        if !names.contains(&n.summary.as_str()) {
            names.push(&n.summary);
        }
    }
    let shown: Vec<String> = names
        .iter()
        .take(3)
        .map(|s| s.chars().take(40).collect())
        .collect();
    if working {
        // While the run is going, show what it's doing now.
        let latest = notes
            .last()
            .map(|n| format!("{} · {}", n.summary, n.status))
            .unwrap_or_default();
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(13.).color(accent()));
            disclosure(ui, open, accent());
            clicked = ui
                .add(
                    egui::Label::new(
                        Text::new(format!("Working · step {} · {}", notes.len(), latest))
                            .size(13.)
                            .color(accent()),
                    )
                    .sense(egui::Sense::click())
                    .truncate(),
                )
                .on_hover_text(if open {
                    "Hide the steps"
                } else {
                    "Show each step and its result"
                })
                .clicked();
        });
        return clicked;
    }
    let mut text = format!(
        "{} {} step{} · {}{}",
        "Worked ·",
        notes.len(),
        if notes.len() == 1 { "" } else { "s" },
        shown.join(", "),
        if names.len() > 3 {
            format!(", +{} more", names.len() - 3)
        } else {
            String::new()
        }
    );
    if failed > 0 {
        text.push_str(&format!(" · {failed} didn't work"));
    }
    let mut clicked = false;
    ui.horizontal(|ui| {
        symbol(ui, Icon::Tool, muted());
        disclosure(ui, open, muted());
        clicked = ui
            .add(
                egui::Label::new(Text::new(text).size(13.).color(muted()))
                    .sense(egui::Sense::click())
                    .truncate(),
            )
            .on_hover_text(if open {
                "Hide the steps"
            } else {
                "Show each step and its result"
            })
            .clicked();
    });
    clicked
}

/// A small open/closed triangle, drawn (some fonts lack ▸ and ▾).
fn disclosure(ui: &mut egui::Ui, open: bool, color: Color) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10., 10.), egui::Sense::hover());
    let c = rect.center();
    let points = if open {
        vec![
            c + egui::vec2(-4., -2.),
            c + egui::vec2(4., -2.),
            c + egui::vec2(0., 3.),
        ]
    } else {
        vec![
            c + egui::vec2(-2., -4.),
            c + egui::vec2(3., 0.),
            c + egui::vec2(-2., 4.),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// A line in the conversation where earlier messages were compacted for the model.
fn compaction_divider(ui: &mut egui::Ui, exchanges: usize) {
    ui.add_space(10.);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 360.).max(40.) / 2.;
        let (r, _) = ui.allocate_exact_size(egui::vec2(w, 1.), egui::Sense::hover());
        ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1_f32, muted()));
        ui.label(
            Text::new(format!(
                "Compacted · the model now sees a summary of the first {exchanges} exchange{}",
                if exchanges == 1 { "" } else { "s" }
            ))
            .size(12.)
            .color(muted()),
        )
        .on_hover_text("To fit the model's context window, earlier messages are sent as a short summary. Everything is still here for you, and the model can be asked about it.");
        let (r, _) = ui.allocate_exact_size(egui::vec2(w, 1.), egui::Sense::hover());
        ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1_f32, muted()));
    });
}

fn message_row(ui: &mut egui::Ui, user: bool, contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    let inset = (width * 0.10).clamp(12., 84.);
    let row_width = if user {
        (width - inset).min(680.)
    } else {
        width - inset * 0.55
    };
    let align = if user {
        egui::Align::RIGHT
    } else {
        egui::Align::LEFT
    };
    ui.with_layout(egui::Layout::top_down(align), |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(row_width.max(1.), 0.),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let frame = if user {
                    egui::Frame::new()
                        .fill(raised())
                        .corner_radius(12)
                        .inner_margin(12)
                } else {
                    egui::Frame::new()
                };
                frame.show(ui, |ui| {
                    ui.set_min_width((row_width - if user { 24. } else { 0. }).max(1.));
                    contents(ui);
                });
            },
        );
    });
    ui.add_space(8.);
}

fn typing_into_chat(ui: &egui::Ui, composer: egui::Id) -> bool {
    let typing = ui.input(|i| {
        i.focused
            && i.events.iter().any(|event| match event {
                egui::Event::Text(text) => !text.trim().is_empty(),
                egui::Event::Paste(text) => !text.is_empty(),
                _ => false,
            })
    });
    if !typing {
        return false;
    }
    let focused = ui.memory(|m| m.focused());
    if focused
        .is_some_and(|id| id == composer || egui::TextEdit::load_state(ui.ctx(), id).is_some())
    {
        return false;
    }
    // Typing after using a message action or clicking the transcript starts a follow-up.
    // Let popups, settings, and other editors keep their own keyboard input.
    ui.memory(|m| {
        !m.any_popup_open()
            && !m
                .areas()
                .visible_layer_ids()
                .iter()
                .any(|l| matches!(l.order, egui::Order::Middle | egui::Order::Foreground))
    })
}

impl Desktop {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        root: PathBuf,
        tx: UnboundedSender<String>,
        rx: Receiver<View>,
        preview: tokio::sync::watch::Receiver<String>,
        initial_accent: Accent,
    ) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        #[cfg(windows)]
        caption(cc, false);
        #[allow(unused_mut)]
        let mut heading_font = egui::FontFamily::Proportional;
        #[cfg(windows)]
        if let Some(data) = std::env::var_os("WINDIR")
            .and_then(|base| std::fs::read(PathBuf::from(base).join("Fonts/segoeui.ttf")).ok())
        {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("Segoe UI".into(), egui::FontData::from_owned(data).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "Segoe UI".into());
            if let Some(bytes) = std::env::var_os("WINDIR")
                .and_then(|dir| std::fs::read(PathBuf::from(dir).join("Fonts/georgia.ttf")).ok())
            {
                heading_font = egui::FontFamily::Name("Spindle display".into());
                fonts
                    .font_data
                    .insert("Georgia".into(), egui::FontData::from_owned(bytes).into());
                fonts
                    .families
                    .insert(heading_font.clone(), vec!["Georgia".into()]);
            }
            cc.egui_ctx.set_fonts(fonts);
        }
        apply_style(&cc.egui_ctx, false);
        let mut view = View::default();
        view.state.preferences.accent = initial_accent;
        Self {
            message_editors: Default::default(),
            paste_drafts: Default::default(),
            submissions: Default::default(),
            android_ime: Default::default(),
            threads: Default::default(),
            drafts_by_thread: Default::default(),
            generating: None,
            tasks: Default::default(),
            reader: Default::default(),
            images: Default::default(),
            voice: Default::default(),
            sync_form: Default::default(),
            drawer: false,
            drawer_at: None,
            reported_view: None,
            ollama_address: None,
            ollama_key: String::new(),
            sync_tab: false,
            tray: None,
            shutdown: Default::default(),
            voice_form: Default::default(),
            projects: Default::default(),
            rx,
            preview,
            tx,
            root,
            view,
            page: Page::Chat,
            draft: String::new(),
            notice: "Connecting to Ollama…".into(),
            focus: !cfg!(target_os = "android"),
            details: false,
            help: false,
            command_index: 0,
            commands_dismissed: String::new(),
            swarm: false,
            worker_models: Default::default(),
            worker_roles: ["analyst".into(), "reviewer".into()],
            assignment: String::new(),
            heading_font,
            logo: SpindleLogo::new(&cc.egui_ctx, initial_accent),
            applied_light: None,
            search: String::new(),
            favorites_only: false,
            select_focus: false,
            controls_panel: None,
            image_drafts: vec![],
            images_by_thread: Default::default(),
            share_pending: false,
            preferences_tab: false,
            models_tab: false,
            memory_tab: false,
            sandbox_tab: false,
            integration_tab: None,
            browser_setup: Default::default(),
            #[cfg(any(windows, target_os = "linux"))]
            updates: Default::default(),
            connector_setup: Default::default(),
            local_servers: Default::default(),
            browser_path_draft: None,
            pending_copy: None,
            work_started: None,
            open_steps: Default::default(),
            toast: None,
            chat_selection: Default::default(),
            zoom_changed: None,
            discarding: false,
            sidebar_sent: None,
            plan: Default::default(),
            memory: Default::default(),
            git: Default::default(),
            components: Default::default(),
            model_query: String::new(),
            model_provider: None,
            tuning_editor: None,
            provider_setup: Default::default(),
            connection: Connection::default(),
            remove_provider: None,
            settings_feedback: String::new(),
            builder: false,
            skill_name: String::new(),
            skill_brief: String::new(),
        }
    }

    /// Ctrl+Scroll (and egui's Ctrl+Plus/Minus) zooms the whole window; Ctrl+0 resets. The
    /// level is saved once it settles.
    fn zoom(&mut self, ctx: &egui::Context) {
        let (delta, reset) = ctx.input(|i| {
            (
                i.zoom_delta(),
                i.modifiers.command && i.key_pressed(egui::Key::Num0),
            )
        });
        let before = ctx.zoom_factor();
        let after = if reset {
            1.0
        } else {
            (before * delta).clamp(0.6, 2.5)
        };
        if (after - before).abs() > 0.001 {
            ctx.set_zoom_factor(after);
            self.zoom_changed = Some(std::time::Instant::now());
        } else if (ctx.zoom_factor() * 100.).round() as u16 != self.view.state.preferences.zoom {
            self.zoom_changed
                .get_or_insert_with(std::time::Instant::now);
        }
        if let Some(at) = self.zoom_changed {
            if at.elapsed() >= std::time::Duration::from_millis(600) {
                self.zoom_changed = None;
                let percent = (ctx.zoom_factor() * 100.).round() as u16;
                if percent != self.view.state.preferences.zoom {
                    let mut prefs = self.view.state.preferences.clone();
                    prefs.zoom = percent;
                    self.view.state.preferences.zoom = percent;
                    self.send(format!(
                        "/preferences {}",
                        serde_json::to_string(&prefs).expect("Serializable preferences")
                    ));
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(650));
            }
        }
    }

    fn send(&mut self, text: impl Into<String>) {
        self.dispatch(text.into(), None);
    }

    fn report_view(&mut self, chat: Option<u64>) {
        if self.reported_view != chat {
            self.reported_view = chat;
            let _ = self.tx.send(format!("/view-thread {}", serde_json::json!(chat)));
        }
    }

    fn dispatch(&mut self, mut text: String, receipt: Option<u64>) -> submission::Dispatch {
        use submission::Dispatch;
        // Commands are case-insensitive: lower-case the command word, keep its arguments.
        if text.trim_start().starts_with('/') {
            let start = text.len() - text.trim_start().len();
            let end = text[start..]
                .find(char::is_whitespace)
                .map_or(text.len(), |i| start + i);
            let word = text[start..end].to_lowercase();
            text.replace_range(start..end, &word);
        }
        if text.trim() == "/copy" {
            let state = &self.view.state;
            let title = state.thread.label(&state.history);
            self.pending_copy = Some(crate::threads::transcript(&title, &state.history));
            self.notice = "Conversation copied as Markdown".into();
            return Dispatch::Handled;
        }
        if text.trim() == "/new"
            || text.trim().starts_with("/new-in ")
            || (!text.trim_start().starts_with('/') && self.view.state.history.is_empty())
        {
            self.threads.archived = false;
        }
        if let Some(arg) = text.trim().strip_prefix("/preview ") {
            let chat = self.view.state.chat_id;
            let root = self.view.state.files_root(&self.root, chat);
            let result = crate::state::project_path(&root, arg.trim()).or_else(|_| {
                crate::artifacts::folder(&root, chat).and_then(|folder| crate::state::project_path(&folder, arg.trim()))
            });
            match result {
                Ok(path) if path.is_file() && crate::documents::readable(&path.to_string_lossy()) => {
                    self.reader.load(path);
                    self.page = Page::Chat;
                }
                Ok(_) => {
                    self.notice = "Choose a document or HTML file".into();
                    return Dispatch::Rejected;
                }
                Err(e) => {
                    self.notice = e.to_string();
                    return Dispatch::Rejected;
                }
            }
            return Dispatch::Handled;
        }
        match text.trim() {
            "/model" => {
                self.show_models();
                self.page = Page::Chat;
                return Dispatch::Handled;
            }
            "/open" => {
                self.reader.pick(&self.root);
                return Dispatch::Handled;
            }
            "/share" => {
                self.share_pending = true;
                return Dispatch::Handled;
            }
            "/rename" | "/settings" => {
                self.edit_thread(self.view.state.chat_id);
                return Dispatch::Handled;
            }
            "/fork" => {
                let id = self.view.state.chat_id;
                self.page = Page::Chat;
                return self.dispatch_engine(format!(
                    "/thread {id} {}",
                    serde_json::to_string(&crate::threads::Action::Branch)
                        .expect("Serializable thread action")
                ), receipt);
            }
            "/init" => {
                match self.view.state.thread.project.clone() {
                    Some(path) => self.project_menu(&path, project_view::ProjectMenu::Instructions),
                    None => self.projects.start_create(),
                }
                return Dispatch::Handled;
            }
            "/undo" => {
                if let Some(last) = self
                    .view
                    .state
                    .history
                    .iter()
                    .rev()
                    .find(|m| m.role == "user")
                {
                    if !self.view.busy {
                        self.draft = last.content.clone();
                        self.focus = true;
                    }
                }
            }
            _ => {}
        }
        if text.trim() == "/usage" {
            self.page = Page::Automations;
            self.tasks.tab = 2;
        }
        if text.trim() == "/automation" {
            self.page = Page::Automations;
            return Dispatch::Handled;
        }
        if text.trim() == "/read" {
            self.reader.open = true;
            return Dispatch::Handled;
        }
        if let Some(args) = text.trim().strip_prefix("/read ") {
            match crate::documents::read_request(args).and_then(|(index, path)| {
                Ok((index, crate::state::project_path(&self.root, path)?))
            }) {
                Ok((index, path)) => {
                    self.reader.load(path);
                    self.reader.index = index;
                }
                Err(e) => {
                    self.notice = format!("Reader: {e:#}");
                    return Dispatch::Rejected;
                }
            }
            return Dispatch::Handled;
        }
        self.dispatch_engine(text, receipt)
    }

    fn dispatch_engine(&mut self, text: String, receipt: Option<u64>) -> submission::Dispatch {
        use submission::Dispatch;
        let text = match receipt {
            Some(id) => format!("/ui-submit {id} {} {text}", self.view.state.chat_id),
            None => text,
        };
        if self.tx.send(text).is_err() {
            self.notice = "Session engine stopped. Reopen the workspace.".into();
            Dispatch::Rejected
        } else {
            Dispatch::Sent
        }
    }

    fn compose(&mut self, prefix: &str) {
        self.page = Page::Chat;
        self.draft = prefix.into();
        self.focus = true;
    }

    fn toggle_sidebar(&mut self) {
        let mut prefs = self.view.state.preferences.clone();
        prefs.compact_sidebar = !prefs.compact_sidebar;
        self.send(format!(
            "/preferences {}",
            serde_json::to_string(&prefs).unwrap()
        ));
    }
    fn sidebar_mode(&mut self, ui: &mut egui::Ui, compact: bool) {
        let cowork = self.view.state.thread.cowork;
        let chosen = if compact {
            let (icon, hint) = if cowork {
                (Icon::Tool, "Cowork mode · switch to Chat")
            } else {
                (Icon::Chat, "Chat mode · switch to Cowork")
            };
            small(ui, icon, hint).then_some(!cowork)
        } else {
            let mut chosen = None;
            egui::Frame::new().fill(raised()).stroke(egui::Stroke::new(1_f32, line()))
                .corner_radius(9).inner_margin(3).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.;
                        for (label, value, hint) in [
                            ("Chat", false, "Talk, ask questions, or make an image"),
                            ("Cowork", true, "Work on files and tasks together"),
                        ] {
                            if ui.selectable_label(cowork == value, label).on_hover_text(hint).clicked() {
                                chosen = Some(value);
                            }
                        }
                    });
                });
            chosen
        };
        if let Some(mode) = chosen {
            if mode != cowork {
                self.send(if mode { "/mode cowork" } else { "/mode chat" });
            }
            self.page = Page::Chat;
            self.drawer = false;
            self.focus = true;
        }
    }

    pub(super) fn sidebar(&mut self, ctx: &egui::Context) {
        let drawer = narrow(ctx);
        let compact = self.view.state.preferences.compact_sidebar && !drawer;
        let screen = ctx.screen_rect().width();
        let (min, max) = sidebar_range(screen);
        let saved = self
            .view
            .state
            .preferences
            .sidebar_width
            .map(f32::from)
            .unwrap_or(if screen < 850. { 184. } else { 220. })
            .clamp(min, max);
        let side = if drawer {
            egui::SidePanel::left("workspace-drawer")
                .exact_width(screen)
                .resizable(false)
                .show_separator_line(false)
        } else if compact {
            egui::SidePanel::left("workspace-compact")
                .exact_width(60.)
                .resizable(false)
        } else {
            // Drag the right edge to resize. The width is saved when the drag ends.
            egui::SidePanel::left("workspace")
                .default_width(saved)
                .width_range(min..=max)
                .resizable(true)
                .show_separator_line(false)
        };
        let shown = side
            .frame(
                egui::Frame::new()
                    .fill(panel())
                    .inner_margin(if compact { 8 } else { 14 }),
            )
            .show(ctx, |ui| {
                ui.add_space(7.);
                ui.horizontal(|ui| {
                    ui.image((self.logo.texture.id(), egui::vec2(38., 38.)));
                    if !compact {
                        ui.label(Text::new("Spindle").size(25.).strong().color(ink()));
                    }
                    if drawer {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(Text::new("Close").size(16.)).clicked() {
                                self.drawer = false;
                            }
                        });
                    }
                });
                if !drawer
                    && control(
                        ui,
                        Icon::Sidebar,
                        if compact { "" } else { "Collapse sidebar" },
                        egui::vec2(ui.available_width(), 30.),
                        false,
                    )
                    .on_hover_text(if compact {
                        "Expand sidebar (Ctrl+B)"
                    } else {
                        "Collapse sidebar (Ctrl+B)"
                    })
                    .clicked()
                {
                    self.toggle_sidebar();
                }
                ui.add_space(4.);
                self.sidebar_mode(ui, compact);
                // The full sidebar has + beside Chats; the narrow one keeps an icon here.
                if compact
                    && control(
                        ui,
                        Icon::New,
                        "",
                        egui::vec2(ui.available_width(), 37.),
                        false,
                    )
                    .on_hover_text("New chat outside any project (Ctrl+N)")
                    .clicked()
                {
                    self.send("/new");
                    self.page = Page::Chat;
                }
                ui.add_space(10.);
                for (page, icon, label) in [
                    (
                        Page::Queue,
                        Icon::Queue,
                        format!("Work queue    {}", self.view.state.queue.len()),
                    ),
                    (Page::Skills, Icon::Skill, "Skill library".into()),
                    (Page::Automations, Icon::Clock, "Automations".into()),
                ] {
                    if control(
                        ui,
                        icon,
                        if compact { "" } else { &label },
                        egui::vec2(ui.available_width(), 37.),
                        self.page == page,
                    )
                    .on_hover_text(label)
                    .clicked()
                    {
                        if page == Page::Projects {
                            self.projects.refresh(self.root.clone());
                        }
                        self.page = page;
                    }
                }
                if !compact {
                    self.thread_list(ui);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    let icon_only = compact || ui.available_width() < 175.;
                    if compact {
                        self.sync_indicator(ui, true);
                    }
                    ui.horizontal(|ui| {
                        if control(
                            ui,
                            Icon::Settings,
                            if compact { "" } else { "Settings" },
                            egui::vec2(if compact { 38. } else { 89. }, 32.),
                            self.page == Page::Settings,
                        )
                        .on_hover_text("Settings")
                        .clicked()
                        {
                            self.page = Page::Settings;
                        }
                        if !compact {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                self.sync_indicator(ui, icon_only);
                            });
                        }
                    });
                    ui.separator();
                });
            });
        if !compact && !drawer {
            let width = shown.response.rect.width().round();
            // egui gives the divider its own drag response. A click in the sidebar
            // can also change its content width, but should not save a preference.
            let divider = egui::Id::new("workspace").with("__resize");
            let settled = ctx.read_response(divider).is_some_and(|r| r.drag_stopped())
                && self.zoom_changed.is_none();
            let stored = self.view.state.preferences.sidebar_width.map(f32::from);
            if settled
                && (width - saved).abs() >= 1.
                && stored != Some(width)
                && self.sidebar_sent != Some(width as u16)
            {
                self.sidebar_sent = Some(width as u16);
                let mut prefs = self.view.state.preferences.clone();
                prefs.sidebar_width = Some(width as u16);
                self.send(format!(
                    "/preferences {}",
                    serde_json::to_string(&prefs).expect("Serializable preferences")
                ));
            }
        }
    }

    fn header(&mut self, ctx: &egui::Context) {
        let phone = narrow(ctx);
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(surface())
                    .inner_margin(if phone { 8 } else { 16 }),
            )
            .show(ctx, |ui| {
                ui.set_max_height(34.);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let compact = phone || ui.available_width() < 760.;
                    let title = match self.page {
                        Page::Chat => self.view.state.thread.label(&self.view.state.history),
                        Page::Queue => "Work queue",
                        Page::Automations => "Automations",
                        Page::Skills => "Skill library",
                        Page::Settings => "Settings",
                        Page::Projects => "Projects",
                        Page::Project => self.projects.home_title(),
                    }.to_owned();
                    let label = |s: &'static str| if compact { "" } else { s };
                        if control(
                            ui,
                            Icon::Reader,
                            label("Panel"),
                            egui::vec2(if compact { 34. } else { 84. }, 30.),
                            self.reader.open,
                        )
                        .on_hover_text("Show the side panel: viewer, files, library, browser and plan")
                        .clicked()
                        {
                            self.reader.open = !self.reader.open;
                        }
                        if self.page == Page::Chat && small(ui, Icon::More, "Conversation settings")
                        {
                            self.edit_thread(self.view.state.chat_id);
                        }
                        if small(ui, Icon::Clock, "Session details") {
                            self.details = true;
                        }
                    // Give the title exactly the space left over after the actions.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if phone {
                            if control(ui, Icon::Sidebar, "", egui::vec2(40., 34.), false)
                                .on_hover_text("Chats, projects and settings")
                                .clicked()
                            {
                                self.drawer = true;
                            }
                        } else if ui.available_width() >= 220. {
                            symbol(ui, Icon::Folder, muted());
                            let folder = self.root.file_name().unwrap_or_default().to_string_lossy();
                            ui.add_sized(
                                [ui.available_width().min(120.) * 0.9, 30.],
                                egui::Label::new(Text::new(folder).size(14.).color(muted()))
                                    .truncate(),
                            )
                            .on_hover_text(self.root.display().to_string());
                            ui.label(Text::new("/").color(line()));
                        }
                        ui.add_sized(
                            [ui.available_width().max(0.), 30.],
                            egui::Label::new(Text::new(&title).size(14.))
                                .halign(egui::Align::LEFT)
                                .truncate(),
                        )
                        .on_hover_text(&title);
                    });
                });
            });
    }

    fn composer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("composer")
            .min_height(
                (if self.paste_drafts.files.contains_key(&self.view.state.chat_id) {46.} else {0.}) +
                // Image chips (one row; they wrap only in a very narrow window) and the note
                // shown when the model can't see images.
                (if self.image_drafts.is_empty() {0.} else {
                    (if self.view.vision.contains(&self.view.state.model) {0.} else {20.}) +
                    60. * (1. + ((self.image_drafts.len() as f32 * 210.) / (ctx.screen_rect().width() - 320.).max(400.)).floor()).min(2.)
                }) +
                if self.view.state.pending.is_some() || self.view.state.attachment.is_some() {
                    248.
                } else {
                    202.
                } - if self.view.state.thread.cowork { 42. } else { 16. },
            )
            .frame(egui::Frame::new().fill(surface()).inner_margin(16))
            .show(ctx, |ui| {
                if let Some(file) = &self.view.state.pending {
                    let label = format!(
                        "Attach {} · {} bytes to {}?",
                        file.path,
                        file.text.len(),
                        self.provider_name(route(&self.view.state.model).0)
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.label(label);
                        if ui.button("Preview").clicked() {
                            self.send("/permissions");
                            self.details = true;
                        }
                        if ui.button("Approve").clicked() {
                            self.send("/approve");
                        }
                        if ui.button("Deny").clicked() {
                            self.send("/deny");
                        }
                    });
                } else if let Some(file) = &self.view.state.attachment {
                    ui.label(format!("Attached for next task: {}", file.path));
                }
                let id = ui.make_persistent_id("prompt");
                if std::mem::take(&mut self.focus) || typing_into_chat(ui, id) {
                    ui.memory_mut(|m| m.request_focus(id));
                }
                self.paste_chips(ui,id);
                self.image_chips(ui);
                let has_pastes=self.paste_drafts.files.contains_key(&self.view.state.chat_id);
                let commands = if ui.memory(|m| m.has_focus(id)) && self.draft != self.commands_dismissed {
                    command_matches(&self.draft, &self.view.state.skills)
                } else {
                    vec![]
                };
                let mut chosen_command = None;
                if !commands.is_empty() {
                    self.command_index = self.command_index.min(commands.len() - 1);
                    ui.input_mut(|i| {
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                            self.command_index = (self.command_index + 1) % commands.len();
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                            self.command_index = (self.command_index + commands.len() - 1) % commands.len();
                        }
                        let pick = &commands[self.command_index].0;
                        // Enter on a complete command sends it; otherwise it fills in the selection.
                        let complete = self.draft.trim() == pick.trim();
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                            || (!complete && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                        {
                            chosen_command = Some(pick.clone());
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                            self.commands_dismissed = self.draft.clone();
                        }
                    });
                } else {
                    self.command_index = 0;
                }
                let enter = ui.memory(|m| m.has_focus(id))
                    && ui.input_mut(|i| {
                        i.consume_key(
                            if self.view.state.preferences.enter_sends {
                                egui::Modifiers::NONE
                            } else {
                                egui::Modifiers::COMMAND
                            },
                            egui::Key::Enter,
                        )
                    });
                let mut submit = enter;
                let mut steer = false;
                let cowork = self.view.state.thread.cowork;
                if !has_pastes && self.image_drafts.len() == 1
                    && ui.small_button("Edit image").on_hover_text("Compose an image edit").clicked()
                {
                    if !self.draft.trim_start().to_ascii_lowercase().starts_with("/image ") {
                        self.draft = format!("/image edit {}", self.draft.trim());
                    }
                    self.focus = true;
                }
                let hint = if cowork { "Give Spindle a task…   / for commands" } else { "Message Spindle…   / for commands" };
                let composer = egui::Frame::new()
                    .fill(raised())
                    .stroke(egui::Stroke::new(1.0_f32, line()))
                    .corner_radius(14)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        #[cfg(not(target_os = "android"))]
                        let response = ui.add(
                            egui::TextEdit::multiline(&mut self.draft)
                                .id(id)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .frame(false)
                                .hint_text(hint),
                        );
                        #[cfg(target_os = "android")]
                        let response = self.android_ime.edit(
                            ui, id, id.with(self.view.state.chat_id), &mut self.draft,
                            hint,
                        );
                        if response.clicked() {
                            self.reader.preview.focus_chat();
                        }
                        ui.horizontal(|ui| {
                            let compact = narrow(ctx) || ui.available_width() < 520.;
                            self.model_picker(ui, compact);
                            if !compact {
                                let current = self.view.state.model.clone();
                                self.capability_icons(ui, &current, 16., false);
                            }
                            self.thinking_toggle(ui, compact);
                            if !compact || (!self.view.busy && !self.view.state.held
                                && self.voice.recording().is_none() && ui.available_width() >= 202.)
                            {
                                self.usage_rings(ui);
                            }
                            if !compact {
                                ui.separator();
                            }
                            if small(ui, Icon::Attach, "Attach files to your next message") {
                                #[cfg(windows)]
                                if let Some(paths) =
                                    rfd::FileDialog::new().set_directory(&self.root).pick_files()
                                {
                                    for path in paths {
                                        if image_file(&path) {
                                            self.attach_image_file(ui.ctx(), &path);
                                        } else {
                                            self.attach_file(&path);
                                        }
                                    }
                                }
                                #[cfg(not(windows))]
                                self.compose("/add ");
                            }
                            if let Some(elapsed) = self.voice.recording() {
                                let secs = elapsed.as_secs();
                                if small(ui, Icon::Stop, "Stop and write down what you said") {
                                    let settings = self.speech_settings();
                                    self.voice.stop_recording(ui.ctx(), settings);
                                }
                                let cancel = if compact {
                                    ui.small_button("Cancel").clicked()
                                } else {
                                    ui.label(
                                        Text::new(format!("● {}:{:02}", secs / 60, secs % 60))
                                            .color(Color::from_rgb(220, 60, 60)),
                                    );
                                    ui.small_button("Cancel").clicked()
                                };
                                if cancel {
                                    self.voice.cancel_recording();
                                }
                            } else if self.voice.transcribing() {
                                ui.add(egui::Spinner::new().size(14.));
                                if !compact {
                                    ui.weak("Writing it down…");
                                }
                            } else if small(ui, Icon::Mic, "Dictate: record from your microphone") {
                                if let Err(e) = self.voice.start_recording() {
                                    self.notice = format!("{e:#}");
                                }
                            }
                            if !compact && small(ui, Icon::Swarm, "Choose swarm workers") {
                                self.show_swarm();
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let label = if self.view.busy || self.view.state.held {
                                        "Queue"
                                    } else {
                                        "Send"
                                    };
                                    let (r, res) = ui.allocate_exact_size(
                                        egui::vec2(39., 33.),
                                        egui::Sense::click(),
                                    );
                                    let ready = !self.awaiting_submission()
                                        && (!self.draft.trim().is_empty() || has_pastes || !self.image_drafts.is_empty());
                                    ui.painter().rect_filled(
                                        r,
                                        10,
                                        if ready {
                                            accent()
                                        } else {
                                            accent().gamma_multiply(0.45)
                                        },
                                    );
                                    draw(
                                        ui,
                                        Icon::Arrow,
                                        egui::Rect::from_center_size(
                                            r.center(),
                                            egui::vec2(19., 19.),
                                        ),
                                        surface(),
                                    );
                                    res.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            ready,
                                            label,
                                        )
                                    });
                                    submit |= ready && res.on_hover_text(label).clicked();
                                    if self.view.busy && self.view.state.queue.front().is_some_and(|t|t.origin.unwrap_or(self.view.state.chat_id)==self.view.state.chat_id) {
                                        steer = ui.add_enabled_ui(!self.awaiting_submission() && !self.draft.trim().is_empty() && !has_pastes && self.image_drafts.is_empty(),
                                            |ui| small(ui, Icon::Steer, "Steer the running task with this message")).inner;
                                        if small(ui, Icon::Stop, "Stop the running task") {
                                            self.send("/cancel");
                                        }
                                    } else if self.view.state.held && ui.button("Resume").clicked()
                                    {
                                        self.send("/resume");
                                    }
                                },
                            );
                        });
                    });
                if !commands.is_empty() {
                    let rows = commands.len().min(8);
                    ui.horizontal(|ui| {
                        ui.strong("Commands");
                        if ui.small_button("Close").clicked() {
                            self.commands_dismissed = self.draft.clone();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .id_salt(id.with("slash-commands"))
                        .max_height((ctx.screen_rect().height() * 0.25).clamp(50., 200.))
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(panel())
                                .stroke(egui::Stroke::new(1.0_f32, line()))
                                .corner_radius(10)
                                .inner_margin(6)
                                .show(ui, |ui| {
                                    ui.set_width(composer.response.rect.width().min(520.));
                                    ui.with_layout(
                                        egui::Layout::top_down_justified(egui::Align::LEFT),
                                        |ui| {
                                            for (i, (cmd, about)) in commands.iter().take(rows).enumerate() {
                                                let selected = i == self.command_index;
                                                let mut job = egui::text::LayoutJob::default();
                                                job.append(
                                                    cmd.trim_end(),
                                                    0.,
                                                    egui::TextFormat {
                                                        font_id: egui::FontId::monospace(14.),
                                                        color: if selected { accent() } else { ink() },
                                                        ..Default::default()
                                                    },
                                                );
                                                job.append(
                                                    about,
                                                    14.,
                                                    egui::TextFormat {
                                                        font_id: egui::FontId::proportional(13.),
                                                        color: muted(),
                                                        ..Default::default()
                                                    },
                                                );
                                                let row = ui.add(egui::SelectableLabel::new(selected, job));
                                                let keys = shortcut(cmd.trim_end());
                                                if !keys.is_empty() {
                                                    ui.painter().text(
                                                        row.rect.right_center() - egui::vec2(8., 0.),
                                                        egui::Align2::RIGHT_CENTER,
                                                        keys,
                                                        egui::FontId::proportional(12.),
                                                        muted(),
                                                    );
                                                }
                                                if row.hovered() {
                                                    self.command_index = i;
                                                }
                                                // On press: by the release the message box may have
                                                // lost focus and this list closed.
                                                if row.clicked() || (row.hovered() && ui.input(|i| i.pointer.primary_pressed())) {
                                                    chosen_command = Some(cmd.clone());
                                                }
                                            }
                                        },
                                    );
                                });
                        });
                }
                if let Some(cmd) = chosen_command {
                    self.draft = cmd;
                    let end = egui::text::CCursor::new(self.draft.chars().count());
                    if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                        st.store(ui.ctx(), id);
                    }
                    ui.memory_mut(|m| m.request_focus(id));
                    self.command_index = 0;
                }
                if (submit || steer) && (!self.draft.trim().is_empty() || has_pastes || !self.image_drafts.is_empty()) {
                    self.submit_chat(steer);
                }
                ui.add_space(5.);
                ui.horizontal(|ui| {
                    let first = self.notice.lines().next().unwrap_or("Ready");
                    if self.view.busy {
                        ui.spinner();
                    }
                    let state=&self.view.state;
                    if state.preferences.nerd_stats {
                    let last=state.thread.turns.values().rev().find(|t|t.model==state.model);
                    let cap=self.view.context.or_else(||last.and_then(|t|t.window));
                    let usage=last.filter(|t|t.input>0).map(|t|(t.input+t.output).to_string()).unwrap_or_else(||"—".into());
                    let window=cap.map(|n|n.to_string()).unwrap_or_else(||if route(&state.model).0=="ollama" {"16384 requested".into()}else{"unreported".into()});
                    let compaction=state.thread.compaction.unwrap_or(state.default_compaction(state.chat_id));
                    let threshold=compaction.threshold(cap.unwrap_or(crate::compaction::UNKNOWN_WINDOW));
                    let used=last.filter(|t|t.input>0).map(|t|t.input+t.output);
                    // Green with room, amber past 60% of the compaction point, red past 85%.
                    let tone=used.map(|u|u as f64/threshold.max(1) as f64).map(|r|if r>=0.85 {Color::from_rgb(230,92,92)} else if r>=0.6 {Color::from_rgb(232,178,64)} else {Color::from_rgb(98,190,128)});
                    ui.label(Text::new(format!("Context {usage} / {window}")).color(tone.unwrap_or(muted()))).on_hover_text(format!("Last completed request’s token usage. Compaction: {}. Next prompt threshold: about {} input tokens{}.",compaction.label(),compaction.threshold(cap.unwrap_or(crate::compaction::UNKNOWN_WINDOW)),if cap.is_none(){" using the fallback context estimate"}else{""}));
                    if let Some((_,start,base))=self.generating {
                        let count=self.preview.borrow().chars().count().saturating_sub(base);
                        if count>0 {ui.weak(format!("≈{:.1} tok/s",count as f64/4./start.elapsed().as_secs_f64().max(0.1))).on_hover_text("Streaming estimate from visible characters");}
                    } else if let Some(rate)=last.and_then(|t|t.tokens_per_second) {ui.weak(format!("{rate:.1} tok/s")).on_hover_text("Provider generation timing when available, otherwise output tokens per elapsed request second");}
                    else {ui.weak("— tok/s");}
                    ui.separator();
                    }
                    ui.add(egui::Label::new(Text::new(first).color(muted()).size(12.)).truncate());
                });
                ui.add_space(4.);
            });
    }

    fn conversation(&mut self, ui: &mut egui::Ui) {
        // Branches from other devices, above the conversation.
        self.branch_bar(ui);
        self.build_outputs(ui);
        let copy_selection = self.chat_selection.touch_copy_visible().then(|| ui.small_button("Copy selection"));
        let mut chat_text = chat_selection::ChatText::collecting();
        egui::ScrollArea::vertical()
            .id_salt(("conversation-history", self.view.state.chat_id))
            .stick_to_bottom(!self.chat_selection.dragging())
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.view.state.history.is_empty() && self.view.state.queue.is_empty() {
                    if self.browser_access_card(ui) {
                        return;
                    }
                    ui.add_space((ui.available_height() * 0.15).clamp(12., 85.));
                    ui.vertical_centered(|ui| {
                        ui.image((self.logo.texture.id(), egui::vec2(86., 86.)));
                        ui.add_space(10.);
                        ui.add(egui::Label::new(
                            Text::new("What shall we work on?")
                                .family(self.heading_font.clone())
                                .size(if ui.available_width() < 560. {
                                    28.
                                } else {
                                    38.
                                })
                                .color(ink()),
                        ).wrap());
                    });
                    ui.add_space(26.);
                    if ui.available_width() < 475. {
                        ui.vertical_centered(|ui| {
                            if ui.button("Plan a task").clicked() {
                                self.compose("/plan ");
                            }
                            if ui.button("Ask a swarm").clicked() {
                                self.show_swarm();
                            }
                            if ui.button("Build a skill").clicked() {
                                self.builder = true;
                            }
                        });
                        return;
                    }
                    let width = ((ui.available_width() - 22.) / 3.).min(205.);
                    let offset = ((ui.available_width() - 3. * width - 18.) / 2.).max(0.);
                    ui.horizontal(|ui| {
                        ui.add_space(offset);
                        if starter(
                            ui,
                            Icon::Plan,
                            "Plan a task",
                            "Draft steps and checks",
                            width,
                        )
                        .clicked()
                        {
                            self.compose("/plan ");
                        }
                        if starter(
                            ui,
                            Icon::Swarm,
                            "Ask a swarm",
                            "Compare model findings",
                            width,
                        )
                        .clicked()
                        {
                            self.show_swarm();
                        }
                        if starter(
                            ui,
                            Icon::Skill,
                            "Build a skill",
                            "Make a reusable command",
                            width,
                        )
                        .clicked()
                        {
                            self.builder = true;
                        }
                    });
                }
                let mut action=None;
                let files_root=self.view.state.files_root(&self.root,self.view.state.chat_id);
                let mut open_file=None;
                let mut open_document=None;
                let state=&self.view.state;
                if let Some(work)=state.sync.as_ref().and_then(|p|p.remote_work.get(&state.chat_id)) {
                    ui.weak(format!("Last activity on another device · {work}"));
                    if let Some(text)=work.strip_prefix("Draft · ") {
                        if ui.add_enabled(self.draft.is_empty(),egui::Button::new("Use draft in composer")).clicked(){self.draft=text.to_owned();self.focus=true;}
                    }
                }
                let mut note=None;
                let mut undo_at=None;
                let mut listen:Option<((u64,usize),String)>=None;
                // Runs of tool calls and their results fold into one line, closed by default.
                let groups=step_groups(state);
                let open_steps=self.open_steps.clone();
                let mut toggle_steps=None;
                // The box drawn behind the current group of steps: its shape slot, top and start.
                let mut group_box: Option<(egui::layers::ShapeIdx,f32,usize)>=None;
                for (index,message) in state.history.iter().enumerate() {
                    chat_text.message((state.chat_id,index));
                    if let Some((slot,top,s))=group_box && groups[index]!=Some(s) {
                        close_group_box(ui,slot,top);
                        group_box=None;
                    }
                    if let Some(n)=state.thread.compactions.get(&index) { compaction_divider(ui,*n); }
                    if let Some(start)=groups[index] {
                        let open=open_steps.contains(&(state.chat_id,start));
                        if start==index {
                            let live=self.view.busy || self.view.job.is_some() || state.queue.front().is_some_and(|t|t.origin.unwrap_or(state.chat_id)==state.chat_id);
                            let working=live && groups.last().copied().flatten()==Some(start);
                            ui.add_space(12.);
                            let slot=ui.painter().add(egui::Shape::Noop);
                            group_box=Some((slot,ui.cursor().top(),start));
                            ui.add_space(6.);
                            ui.horizontal(|ui| {
                                ui.add_space(10.);
                                if steps_header(ui,state,&groups,start,open,working) {toggle_steps=Some((state.chat_id,start));}
                            });
                        }
                        if !open {continue;}
                    }
                    let user=message.role=="user";
                    let turn=state.thread.turns.get(&index);
                    if user && let Some(update)=state.thread.plan_notes.get(&index) {
                        ui.push_id(("plan-note",index),|ui| if let Some(a)=notes::user_plan_note(ui,update) {note=Some(a);});
                    }
                    if user && state.thread.tool_results.contains(&index) {
                        ui.add_space(6.);
                        ui.push_id(("tool-result",index),|ui| notes::tool_result(ui,&message.content));
                        continue;
                    }
                    ui.add_space(12.);
                    message_row(ui,user,|ui|{
                    let failed=turn.is_some_and(|turn|turn.failed);
                    let label=if failed {"Spindle"} else {turn.map(|t|route(&t.model).1).unwrap_or(if user {"You"}else{"Spindle"})};
                    ui.horizontal_wrapped(|ui|{
                        ui.label(Text::new(label).strong().color(if user || failed {muted()} else {accent()}));
                        if failed && index + 1 == state.history.len() && state.can_continue_stopped_goal()
                            && ui.small_button("Continue goal").on_hover_text("Use the selected model and the saved plan").clicked() {
                            let _ = self.tx.send("/goal continue".into());
                        }
                        if let Some(turn)=turn.filter(|_| state.preferences.nerd_stats) {
                            if let Some(rate)=turn.tokens_per_second {ui.weak(format!("{rate:.1} tok/s"));}
                            if turn.output>0 || turn.earlier_output>0 {
                                let count=ui.weak(format!("{} output tokens",turn.output.saturating_add(turn.earlier_output)));
                                if let Some(n)=turn.output_budget {count.on_hover_text(format!("Output allowance for the final request: {n} tokens"));}
                            }
                        }
                        if state.thread.edited_messages.contains(&index) { ui.weak("Edited"); }
                        if self.message_editors.toolbar(ui, state.chat_id, index, message) {
                            self.notice = "Message text copied".into();
                        }
                        if !user && !message.content.trim().is_empty() {
                            let at=(state.chat_id,index);
                            if audio::listen_button(ui,self.voice.playing==Some(at),self.voice.busy_with(at)) {
                                listen=Some((at,message.content.clone()));
                            }
                        }
                    });
                    let snapshots: &[crate::state::Attachment] = if user {
                        state.thread.turns.get(&(index+1)).map(|t|t.pastes.as_slice())
                    } else { turn.map(|t|t.artifacts.as_slice()) }.unwrap_or(&[]);
                    let previews = turn.map(|t| t.html_previews.as_slice()).unwrap_or(&[]);
                    // Files made by tools or scripts (a .docx, say) have no text snapshot; they are
                    // shown from the chat's folder.
                    let extra: Vec<crate::state::Attachment> = if user { vec![] } else {
                        turn.map(|t|t.files.iter().enumerate().filter(|(i,n)|!snapshots.iter().chain(previews).any(|f|&f.path==*n) && !t.files[..*i].contains(n)).map(|(_,n)|n).map(|n|crate::state::Attachment{path:n.clone(),text:String::new()}).collect()).unwrap_or_default()
                    };
                    let files = || snapshots.iter().chain(previews).chain(extra.iter());
                    let size_of=|f:&crate::state::Attachment| if f.text.is_empty() {
                        crate::artifacts::existing_file(&files_root,state.chat_id,&f.path).ok().and_then(|p|std::fs::metadata(p).ok()).map(|m|m.len() as usize).unwrap_or(0)
                    } else {f.text.len()};
                    let names: Vec<String> = files().map(|f|f.path.clone()).collect();
                    let mut shown: Vec<String> = vec![];
                    let mut chosen: Vec<(String, reader::FileAction)> = vec![];
                    let clicked=ui.push_id((state.chat_id,index),|ui| {
                        if self.message_editors.field(ui, state.chat_id, index, &message.content, (message.role=="user").then(|| state.history.len()-index-1), &self.tx) { None }
                        // A reply word for word the same as an earlier one: one line, not the text again.
                        else if !user && !failed && message.content.trim().len()>60 && state.history[..index].iter().any(|m|m.role=="assistant" && m.content.trim()==message.content.trim()) {
                            ui.label(Text::new("Repeated an earlier reply word for word").size(13.).italics().color(muted()));
                            None
                        }
                        else {
                            reader::markdown_with_files(ui,&message.content,!user,&names,&mut chat_text,&mut |ui,name|{
                                if shown.iter().any(|s|s==name) {return;}
                                let bytes=files().find(|f|f.path==name).map(&size_of).unwrap_or(0);
                                if let Some(a)=reader::file_card(ui,name,bytes) {chosen.push((name.to_owned(),a));}
                                shown.push(name.to_owned());
                            })
                        }
                    }).inner;
                    if let Some(turn)=turn.filter(|_|!user) {
                        ui.push_id(("reply-notes",index),|ui| if let Some(a)=notes::reply_notes(ui,turn,&state.memories) {note=Some(a);});
                        if let Some(applied)=&turn.applied {
                            ui.push_id(("applied",index),|ui| if notes::applied_note(ui,applied) {undo_at=Some(index);});
                        }
                    }
                    if !message.images.is_empty() {
                        ui.horizontal(|ui| {
                            symbol(ui, Icon::Eye, muted());
                            ui.weak(format!("{} image{} attached", message.images.len(), if message.images.len() == 1 { "" } else { "s" }));
                        });
                    }
                    match clicked {
                        Some(reader::CodeAction::OpenFile(name))=>chosen.push((name,reader::FileAction::Open)),
                        Some(cmd)=>action=Some((cmd,state.history.get(index.saturating_sub(1)).map(|m|m.content.clone()).unwrap_or_default())),
                        None=>{}
                    }
                    for file in files() {
                        if shown.contains(&file.path) {continue;}
                        shown.push(file.path.clone());
                        ui.push_id(("file-card",&file.path),|ui|{
                            if let Some(a)=reader::file_card(ui,&file.path,size_of(file)) {chosen.push((file.path.clone(),a));}
                        });
                    }
                    for (name,choice) in chosen {
                        let Some(file)=files().find(|f|f.path==name) else {continue};
                        let local=if user {true} else {turn.is_some_and(|t|t.files.contains(&file.path))};
                        let disk=if previews.iter().any(|a| a.path == file.path) {
                            preview_disk(state, &self.root, &file.path)
                        } else {
                            local.then(||crate::artifacts::existing_file(&files_root,state.chat_id,&file.path).ok()).flatten()
                        };
                        let html=crate::html_preview::is_html(Path::new(&name));
                        let source=||disk.as_ref().and_then(|p|std::fs::metadata(p).ok().filter(|m|m.len()<=2*1024*1024).and_then(|_|std::fs::read_to_string(p).ok())).unwrap_or_else(||file.text.clone());
                        match choice {
                            reader::FileAction::Open if html => open_file=Some((name.clone(),source(),false,disk.clone())),
                            reader::FileAction::Open if crate::documents::readable(&name) => open_document=Some((disk.clone(),name.clone(),file.text.clone(),false)),
                            reader::FileAction::Open | reader::FileAction::Source => open_file=Some((name.clone(),source(),true,disk.clone())),
                            reader::FileAction::Edit => open_document=Some((disk.clone(),name.clone(),file.text.clone(),true)),
                            reader::FileAction::Download => {
                                self.notice=match super_download(&name,disk.as_deref(),&file.text) {
                                    Ok(Some(path))=>format!("Downloaded to {}",crate::projects::display_path(&path)),
                                    Ok(None)=>String::new(),
                                    Err(e)=>format!("Download failed: {e:#}"),
                                };
                            }
                            reader::FileAction::Reveal => {
                                self.notice=match &disk {
                                    Some(path)=>reveal(path).map(|()|String::new()).unwrap_or_else(|e|format!("Could not open the folder: {e}")),
                                    None=>"This file is available as a thread snapshot. Use Download or Save a copy first.".into(),
                                };
                            }
                            reader::FileAction::CopyPath => {
                                if let Some(path)=&disk { ui.ctx().copy_text(crate::projects::display_path(path)); self.notice="Path copied".into(); }
                                else {self.notice="This file is available as a thread snapshot. Use Save a copy to put it in this workspace.".into();}
                            }
                            reader::FileAction::SaveCopy => match crate::artifacts::save(&files_root,state.chat_id,&file.path,&file.text) {
                                Ok(path)=>{ui.ctx().copy_text(crate::projects::display_path(&path));self.notice=format!("Saved {} · path copied",path.display());}
                                Err(err)=>self.notice=err.to_string(),
                            },
                        }
                    }
                    });
                }
                if let Some((slot,top,_))=group_box { close_group_box(ui,slot,top); }
                if let Some(update)=state.thread.plan_notes.get(&state.history.len()) {
                    if let Some(a)=notes::user_plan_note(ui,update) {note=Some(a);}
                }
                if let Some(n)=state.thread.compactions.get(&state.history.len()) { compaction_divider(ui,*n); }
                let mut decision=None;
                for req in state.requests.iter().filter(|r|r.chat==state.chat_id && !r.approved) {
                    ui.push_id(("approval",req.id),|ui| if let Some(c)=notes::approval_card(ui,req) {decision=Some(c);});
                }
                let applying=state.requests.iter().any(|r|r.chat==state.chat_id && matches!(r.call.call,crate::tools::Call::Apply{}));
                // With automatic apply on, changes made during a run are applied when it finishes: no bar.
                let live=self.view.busy || state.queue.iter().any(|t|t.origin.unwrap_or(state.chat_id)==state.chat_id);
                let changes: &[String]=if state.preferences.auto_apply && live {&[]} else {&state.thread.sandbox_changes};
                if let Some(c)=notes::sandbox_bar(ui,changes,self.view.job.as_deref(),applying,&mut self.discarding) {decision=Some(c);}
                self.browser_access_card(ui);
                if let Some(a)=note {self.note_action(a);}
                if let Some(c)=decision {self.send(c);}
                if let Some(i)=undo_at {self.send(format!("/undo-apply {} {i}",self.view.state.chat_id));}
                if let Some((at,text))=listen {
                    if self.voice.playing==Some(at) { self.voice.stop_playing(); }
                    else { let settings=self.speech_settings(); self.voice.speak(ui.ctx(),settings,text,at); }
                }
                if let Some(key)=toggle_steps { if !self.open_steps.remove(&key) {self.open_steps.insert(key);} }
                if let Some((name,source,raw,disk))=open_file {
                    self.reader.open_output(name,source);
                    self.reader.preview.set_disk(disk);
                    if raw {self.reader.preview.show_source();}
                }
                if let Some((disk,name,text,edit))=open_document {
                    match disk {
                        Some(path) if edit => self.reader.load_document(path),
                        Some(path) => self.reader.load(path),
                        None => self.reader.load_snapshot(name,text),
                    }
                    self.reader.edit_when_loaded=edit;
                }
                if let Some((cmd,prompt))=action {
                    match cmd {
                        reader::CodeAction::OpenFile(_)=>{}
                        reader::CodeAction::Preview(info,body)=>{
                            let name=crate::artifacts::name(&info,&prompt).unwrap_or_else(||"HTML preview".into());
                            self.reader.open_output(name,body);self.page=Page::Chat;
                        }
                        reader::CodeAction::Save(info,body)=>{
                            if let Some(name)=crate::artifacts::name(&info,&prompt) {
                                self.notice=match crate::artifacts::save(&self.view.state.files_root(&self.root,self.view.state.chat_id),self.view.state.chat_id,&name,&body){Ok(p)=>format!("Saved {}",crate::projects::display_path(&p)),Err(e)=>e.to_string()};
                            }
                        }
                    }
                }
                if let Some(t) = self.view.state.queue.front().filter(|t|t.origin.unwrap_or(self.view.state.chat_id)==self.view.state.chat_id) {
                    ui.add_space(12.);
                    if t.tool_result {
                        // A tool result or Spindle note on its way to the model, not the user's words.
                        ui.push_id(("pending-result",t.id),|ui| notes::tool_result(ui,&t.text));
                    } else {
                        message_row(ui,true,|ui|{
                            ui.label(Text::new(format!("You · #{}",t.id)).strong().color(muted()));
                            chat_text.message((self.view.state.chat_id,self.view.state.history.len()));
                            chat_text.label(ui, t.text.as_str(), false);
                        });
                    }
                    chat_text.message((self.view.state.chat_id,self.view.state.history.len()+1));
                    message_row(ui,false,|ui|{
                    if self.view.busy {
                        let partial = self.preview.borrow();
                        if !partial.is_empty() {
                            ui.label(Text::new(route(t.actual.as_deref().unwrap_or(&t.model)).1).color(accent()));
                            ui.push_id((self.view.state.chat_id,self.view.state.history.len()+1),|ui|reader::markdown_with_files(ui,&crate::artifacts::streaming_text(&t.text,&crate::tools::streaming_view(partial.split("\n<!-- spindle-title:").next().unwrap_or(""))),false,&[],&mut chat_text,&mut |_,_|{}));
                        }
                    }
                    if !self.view.busy && !t.continuation.text.is_empty() {
                        ui.weak("Saved reply so far");
                        ui.push_id(("saved-partial",t.id), |ui| reader::markdown_with_files(ui,&crate::artifacts::streaming_text(&t.text,&crate::tools::streaming_view(&t.continuation.text)),false,&[],&mut chat_text,&mut |_,_|{}));
                    }
                    if let Some(s) = t.steering.last() {
                        ui.label(format!("Steering: {s}"));
                    }
                    ui.horizontal(|ui| {
                        if !self.view.state.held {
                            ui.spinner();
                        }
                        let started = match self.work_started {
                            Some((id, at)) if id == t.id => at,
                            _ => {
                                let now = std::time::Instant::now();
                                self.work_started = Some((t.id, now));
                                now
                            }
                        };
                        let secs = started.elapsed().as_secs();
                        let model = route(t.actual.as_deref().unwrap_or(&t.model)).1.to_owned();
                        let streaming = self.view.busy && !self.preview.borrow().is_empty();
                        ui.label(
                            Text::new(if self.view.state.held {
                                t.hold_reason.clone().unwrap_or("On hold · Resume when ready".into())
                            } else if t.continuation.pending() {
                                format!("{model} is continuing its reply · {secs}s")
                            } else if t.tool_result && !streaming {
                                format!("{model} is reading the result · {secs}s")
                            } else if streaming {
                                format!("{model} is writing · {secs}s")
                            } else if self.view.busy {
                                format!("{model} is thinking · {secs}s")
                            } else {
                                format!("Queued for {model}")
                            })
                            .color(accent()),
                        );
                        if !self.view.state.held {
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                        }
                    });
                    });
                }
                self.chat_selection.update(ui, chat_text, copy_selection);
            });
    }

    fn build_outputs(&mut self, ui: &mut egui::Ui) {
        if !crate::native::supported() {
            return;
        }
        let chat = self.view.state.chat_id;
        let files = crate::native::outputs(&self.root, chat);
        if files.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(format!("Build outputs · {}", files.len()))
            .id_salt(("native-outputs", chat))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(150.).show(ui, |ui| {
                    let label = if cfg!(windows) { "Export…" } else { "Export to Downloads" };
                    for name in &files {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(name);
                            if !ui.small_button(label).clicked() {
                                return;
                            }
                            #[cfg(windows)]
                            let target = rfd::FileDialog::new().set_file_name(name).save_file();
                            #[cfg(not(windows))]
                            let target = std::env::var_os("HOME").map(|home| {
                                PathBuf::from(home).join("Downloads").join(name)
                            });
                            let Some(target) = target else { return; };
                            self.notice = match crate::native::export(&self.root, chat, name, &target) {
                                Ok(()) => format!("Exported {}", crate::projects::display_path(&target)),
                                Err(err) => format!("Export failed: {err:#}"),
                            };
                        });
                    }
                });
            });
    }

    fn queue(&mut self, ui: &mut egui::Ui) {
        if self.view.state.queue.is_empty() {
            ui.label("No outstanding tasks.");
        }
        for (i, t) in self.view.state.queue.clone().into_iter().enumerate() {
            ui.group(|ui| {
                ui.label(
                    Text::new(format!("#{}  ·  {}", t.id, t.model))
                        .strong()
                        .color(accent()),
                );
                ui.label(t.text);
                if let Mode::Swarm(workers) | Mode::Council(workers) = t.mode {
                    for w in workers {
                        ui.label(format!(
                            "{} · {} · {}",
                            w.role,
                            w.model,
                            if w.answer.is_some() {
                                "finished"
                            } else {
                                "pending"
                            }
                        ));
                    }
                }
                ui.horizontal(|ui| {
                    if i == 0 && self.view.busy {
                        if small(ui, Icon::Stop, "Stop the running task") {
                            self.send("/cancel");
                        }
                    } else {
                        if ui.button("Remove").clicked() {
                            self.send(format!("/queue remove {}", t.id));
                        }
                        if self.view.busy
                            && small(
                                ui,
                                Icon::Steer,
                                "Use this queued message to steer the running task",
                            )
                        {
                            self.send(format!("/queue steer {}", t.id));
                        }
                    }
                });
            });
        }
    }

    fn skills(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Create skill").clicked() {
                self.builder = true;
            }
            if ui.button("Install from workspace").clicked() {
                #[cfg(windows)]
                if let Some(path) = rfd::FileDialog::new()
                    .set_directory(&self.root)
                    .add_filter("Skill definition", &["md"])
                    .pick_file()
                {
                    self.send(format!("/skills install {}", path.display()));
                }
                #[cfg(not(windows))]
                self.compose("/skills install ");
            }
        });
        ui.add_space(12.);
        ui.heading("Built-in");
        ui.group(|ui| {
            ui.label(Text::new("/council").strong().color(accent()));
            ui.label("Independent, read-only feedback from your saved review panel.");
            ui.horizontal(|ui| {
                if ui.button("Ask council").clicked() {
                    self.compose("/council ");
                }
                if ui.button("Configure roster").clicked() {
                    self.preferences_tab = true;
                    self.models_tab = false;
                    self.memory_tab = false;
                    self.sandbox_tab = false;
                    self.integration_tab = None;
                    self.page = Page::Settings;
                }
            });
        });
        for (name, definition) in crate::state::BUILTIN_SKILLS {
            if self.view.state.skills.contains_key(name) {
                continue;
            }
            egui::CollapsingHeader::new(format!("/{name}"))
                .id_salt(("built-in-skill", name))
                .show(ui, |ui| {
                    ui.label(definition);
                    if ui.button("Use skill").clicked() {
                        self.compose(&format!("/{name} "));
                    }
                });
        }
        ui.add_space(12.);
        for (title, skills) in [
            ("Drafts", self.view.state.drafts.clone()),
            ("Installed", self.view.state.skills.clone()),
        ] {
            ui.heading(title);
            if skills.is_empty() {
                ui.label(Text::new("None yet").color(muted()));
            }
            for (name, body) in skills {
                egui::CollapsingHeader::new(&name)
                    .id_salt((title, &name))
                    .show(ui, |ui| {
                        ui.label(&body);
                        ui.horizontal(|ui| {
                            if title == "Drafts" && ui.button("Install").clicked() {
                                self.send(format!("/skills install {name}"));
                            }
                            if ui.button("Try example").clicked() {
                                self.compose(&format!("/skills test {name} "));
                            }
                            if title == "Installed" && ui.button("Use skill").clicked() {
                                self.compose(&format!("/{name} "));
                            }
                            if ui.button("Copy Markdown").clicked() {
                                ui.ctx().copy_text(body.clone());
                            }
                            #[cfg(windows)]
                            if ui.button("Export…").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_directory(&self.root)
                                    .set_file_name("SKILL.md")
                                    .save_file()
                                {
                                    self.notice = match export_skill(&path, &body) {
                                        Ok(()) => format!("Saved {}", path.display()),
                                        Err(err) => format!("Export failed: {err:#}"),
                                    };
                                }
                            }
                        });
                    });
            }
            ui.add_space(14.);
        }
    }

    fn show_swarm(&mut self) {
        for m in &mut self.worker_models {
            if !self.view.models.contains(m) {
                m.clone_from(&self.view.state.model);
            }
        }
        self.swarm = true;
    }

    fn builder_panel(&mut self, ui: &mut egui::Ui) {
        let mut create = false;
        detail_panel(ui, "Build a skill", "Back", &mut self.builder, |ui| {
            ui.label("Command name");
            self.android_ime.field(
                ui,
                "skill-name",
                &mut self.skill_name,
                Default::default(),
                |field| field.hint_text("release-notes"),
            );
            ui.label("What should the skill do?");
            self.android_ime
                .multiline(ui, "skill-brief", &mut self.skill_brief, |field| {
                    field
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("Describe its inputs, steps and expected result.")
                });
            create = ui
                .add_enabled(
                    crate::state::skill_name(self.skill_name.trim()).is_ok()
                        && !self.skill_brief.trim().is_empty(),
                    egui::Button::new("Generate draft"),
                )
                .clicked();
        });
        if create {
            self.send(format!(
                "/skills create {} {}",
                self.skill_name.trim(),
                self.skill_brief.trim()
            ));
            self.builder = false;
            self.page = Page::Chat;
        }
    }

    fn swarm_panel(&mut self, ui: &mut egui::Ui) {
        let mut run = false;
        detail_panel(ui, "Ask two models", "Back", &mut self.swarm, |ui| {
            ui.label("Assignment");
            ui.add(
                egui::TextEdit::multiline(&mut self.assignment)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.weak(format!("Coordinator · {}", route(&self.view.state.model).1));
            for i in 0..2 {
                ui.group(|ui| {
                    ui.label(format!("Worker {}", i + 1));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.worker_roles[i])
                                .desired_width(120.)
                                .hint_text("Role"),
                        );
                        crate::widgets::InlineChoice::from_id_salt(("worker-model", i))
                            .selected_text(&self.worker_models[i])
                            .show_ui(ui, |ui| {
                                for m in &self.view.models {
                                    ui.selectable_value(&mut self.worker_models[i], m.clone(), m);
                                }
                            });
                    });
                });
            }
            run = ui
                .add_enabled(
                    !self.assignment.trim().is_empty()
                        && self.worker_roles[0] != self.worker_roles[1]
                        && self
                            .worker_roles
                            .iter()
                            .all(|s| !s.is_empty() && s.chars().all(|c| !c.is_whitespace() && c != '='))
                        && self.worker_models.iter().all(|model| self.view.models.contains(model)),
                    egui::Button::new("Run swarm"),
                )
                .clicked();
        });
        if run {
            self.send(format!(
                "/swarm {} --worker {}={} --worker {}={}",
                self.assignment,
                self.worker_roles[0],
                self.worker_models[0],
                self.worker_roles[1],
                self.worker_models[1]
            ));
            self.swarm = false;
            self.page = Page::Chat;
        }
    }

    fn has_detail_panel(&self) -> bool {
        #[cfg(target_os = "android")]
        if crate::scan::state() != crate::scan::State::Off {
            return true;
        }
        self.help
            || self.details
            || self.builder
            || self.swarm
            || self.controls_panel.is_some()
            || self.tuning_editor.is_some()
            || self.threads.has_editor()
            || self.projects.has_dialog()
            || self.sync_form.browse.is_some()
            || self.needs_projects_folder()
    }

    fn render_detail_panel(&mut self, ui: &mut egui::Ui) {
        #[cfg(target_os = "android")]
        if crate::scan::state() != crate::scan::State::Off {
            self.scanner_panel(ui);
            return;
        }
        if self.sync_form.browse.is_some() {
            self.folder_browser_panel(ui);
        } else if self.projects.has_dialog() {
            self.project_dialogs(ui);
        } else if self.threads.has_editor() {
            self.thread_windows(ui);
        } else if let Some(panel) = self.controls_panel {
            self.controls_panel(ui, panel);
        } else if self.tuning_editor.is_some() {
            self.tuning_panel(ui);
        } else if self.builder {
            self.builder_panel(ui);
        } else if self.swarm {
            self.swarm_panel(ui);
        } else if self.details {
            detail_panel(ui, "Session details", "Close", &mut self.details, |ui| {
                ui.label(&self.notice);
            });
        } else if self.help {
            detail_panel(ui, "Commands", "Close", &mut self.help, |ui| {
                ui.monospace(HELP);
            });
        } else {
            self.projects_folder_panel(ui);
        }
    }
}

impl eframe::App for Desktop {
    /// Android draws under the status bar and the gesture bar (or keyboard). Tell egui its
    /// screen is only the part in between, so pages, menus and dialogs all stay usable.
    #[cfg(target_os = "android")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(screen) = raw_input.screen_rect else {
            return;
        };
        let ppp = ctx.pixels_per_point();
        let (top, bottom) = crate::android::insets(screen.height() * ppp);
        let (top, bottom) = (top / ppp, bottom / ppp);
        if top + bottom > 0. && top + bottom < screen.height() - 100. {
            raw_input.screen_rect = Some(egui::Rect::from_min_max(
                egui::pos2(screen.min.x, screen.min.y + top),
                egui::pos2(screen.max.x, screen.max.y - bottom),
            ));
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.shutdown.finished.load(Ordering::Acquire) {
            self.tray = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        #[cfg(any(windows, target_os = "linux"))]
        if self.shutdown.restarting() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Saving the workspace and restarting Spindle…");
                });
            });
            return;
        }
        self.background(ctx, frame);
        if !cfg!(target_os = "android")
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Q))
        {
            self.request_exit(ctx);
        }
        if self.page == Page::Chat
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K))
        {
            self.show_models();
        }
        let light = match self.view.state.preferences.theme {
            Theme::Light => true,
            Theme::Dark => false,
            Theme::System => ctx.input(|i| i.raw.system_theme) == Some(egui::Theme::Light),
        };
        if self.applied_light != Some(light) {
            apply_style(ctx, light);
            #[cfg(windows)]
            caption(&*frame, light);
            self.applied_light = Some(light);
        }
        while let Ok(v) = self.rx.try_recv() {
            self.observe_submission(&v);
            if self.plan.submitted && v.notice == "Plan revision queued" && v.state.chat_id == self.view.state.chat_id {
                self.plan.revision_accepted();
            }
            if let (Some(before), Some(after)) = (&self.view.state.space, &v.state.space)
                && (!before.projects.keys().eq(after.projects.keys())
                    || before.project_aliases != after.project_aliases)
            {
                if self.projects.home.as_ref().is_some_and(|p| {
                    before.projects.contains_key(&p.path) && !after.projects.contains_key(&p.path)
                        && crate::projects::existing(&p.path).ok().flatten().is_none()
                }) {
                    self.projects.home = None;
                    self.page = Page::Chat;
                }
                self.projects.refresh(self.root.clone());
            }
            if !cfg!(target_os = "android")
                && self.page == Page::Chat
                && self.view.busy
                && !v.busy
                && self.view.state.chat_id == v.state.chat_id
                && v.state.history.len() > self.view.state.history.len()
                && ctx.memory(|m| m.focused().is_none() && !m.any_popup_open())
            {
                self.focus = true;
            }
            if v.notice.starts_with("Scheduled #") && v.notice != self.view.notice {
                self.tasks.clear_saved_form();
            }
            self.notice = if v.notice.starts_with("ext:") {
                // A reply to another program (CLI, MCP), not for this window.
                self.notice.clone()
            } else if v
                .state
                .history
                .last()
                .is_some_and(|m| m.content == v.notice)
            {
                "Response saved".into()
            } else {
                v.notice.clone()
            };
            if v.notice.starts_with("Connection failed")
                || v.notice.contains(" connected · ")
                || v.notice.contains("disconnected")
                || v.notice.contains("refreshed")
                || v.notice.starts_with("Error:")
            {
                self.settings_feedback = v.notice.clone();
            }
            if v.notice.contains(" connected · ") {
                self.provider_setup = Default::default();
                self.connection.key.clear();
            }
            if v.state.preferences.zoom != self.view.state.preferences.zoom
                && self.zoom_changed.is_none()
            {
                ctx.set_zoom_factor(f32::from(v.state.preferences.zoom.clamp(60, 250)) / 100.);
            }
            if v.state.preferences.text_size != self.view.state.preferences.text_size {
                ctx.style_mut(|style| {
                    style.text_styles.insert(
                        egui::TextStyle::Body,
                        egui::FontId::proportional(v.state.preferences.text_size as f32),
                    );
                });
            }
            if v.state.chat_id != self.view.state.chat_id {
                self.chat_selection = Default::default();
                self.plan = Default::default();
                if self.reader.preview.active() {
                    self.reader.preview.hide();
                    self.reader.preview = Default::default();
                    self.reader.open = false;
                }

                self.drafts_by_thread
                    .insert(self.view.state.chat_id, std::mem::take(&mut self.draft));
                self.draft = self
                    .drafts_by_thread
                    .remove(&v.state.chat_id)
                    .unwrap_or_default();
                self.images_by_thread.insert(
                    self.view.state.chat_id,
                    std::mem::take(&mut self.image_drafts),
                );
                self.image_drafts = self
                    .images_by_thread
                    .remove(&v.state.chat_id)
                    .unwrap_or_default();
            }
            self.paste_drafts
                .files
                .retain(|id, _| v.state.thread_data(*id).is_some());
            self.images_by_thread
                .retain(|id, _| v.state.thread_data(*id).is_some());
            self.message_editors.observe(&v);
            self.threads.observe(&v);
            self.drafts_by_thread
                .retain(|id, _| v.state.thread_data(*id).is_some());
            self.view = v;
        }
        let selected = self.view.state.preferences.accent;
        if self.awaiting_submission() && self.tx.is_closed() {
            self.reject_submission();
            self.notice = "Session engine stopped. Your draft was kept.".into();
        }
        crate::widgets::set_accent(selected);
        if self.logo.accent != selected {
            self.logo = SpindleLogo::new(ctx, selected);
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(self.logo.window.clone())));
        }
        ctx.request_repaint_after(Duration::from_millis(120));
        if self.view.busy && !self.preview.borrow().is_empty() {
            if let Some(task) =
                self.view.state.queue.front().filter(|t| {
                    t.origin.unwrap_or(self.view.state.chat_id) == self.view.state.chat_id
                })
            {
                if self.generating.is_none_or(|(id, _, _)| id != task.id) {
                    self.generating = Some((
                        task.id,
                        std::time::Instant::now(),
                        self.preview.borrow().chars().count(),
                    ));
                }
            } else {
                self.generating = None;
            }
        } else {
            self.generating = None;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
            self.send("/new");
            self.page = Page::Chat;
            self.focus = true;
        }
        if std::mem::take(&mut self.share_pending) {
            let state = &self.view.state;
            ctx.copy_text(crate::threads::transcript(
                state.thread.label(&state.history),
                &state.history,
            ));
            self.notice = "Conversation copied as Markdown".into();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
            self.toggle_sidebar();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
            self.reader.pick(&self.root);
        }
        for path in ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect::<Vec<_>>()
        }) {
            if image_file(&path) && self.page == Page::Chat {
                self.attach_image_file(ctx, &path);
            } else if self.page == Page::Chat {
                // Files dropped on the chat go with the next message.
                self.attach_file(&path);
            } else {
                self.reader.load(path);
                break;
            }
        }
        // Ctrl+V with only an image on the clipboard produces no paste event; the key release
        // still arrives, so check the clipboard for an image then.
        if self.page == Page::Chat
            && ctx.input(|i| {
                i.events.iter().any(|e| {
                    matches!(e, egui::Event::Key { key: egui::Key::V, pressed: false, modifiers, .. } if modifiers.command)
                })
            })
        {
            if let Some(png) = clipboard_image() {
                self.add_image(ctx, "Pasted image.png".into(), png);
            }
        }
        if let Some(text) = self.pending_copy.take() {
            ctx.copy_text(text);
        }
        self.zoom(ctx);
        self.poll_projects();
        self.poll_voice(ctx);
        // Android: the screen Spindle draws in already leaves out the status bar and the
        // gesture bar or keyboard (see raw_input_hook).
        #[cfg(target_os = "android")]
        {
            // Opened by scanning the pairing QR code: join straight away.
            if let Some(link) = crate::android::take_link() {
                self.page = Page::Settings;
                self.sync_tab = true;
                self.integration_tab = None;
                self.drawer = false;
                if self.view.state.space.is_some() {
                    self.notice = "This phone already syncs. To pair it again, leave sync first (Settings › Sync).".into();
                } else {
                    self.send(format!(
                        "/sync join {link} {}",
                        crate::sync::live::hostname()
                    ));
                }
            }
        }
        if narrow(ctx) {
            if self.drawer {
                // The drawer covers the screen; choosing anything in it closes it.
                let now = (std::mem::discriminant(&self.page), self.view.state.chat_id);
                let at = *self.drawer_at.get_or_insert(now);
                self.show_notice(ctx);
                self.sidebar(ctx);
                egui::CentralPanel::default().show(ctx, |_| {});
                let after = (std::mem::discriminant(&self.page), self.view.state.chat_id);
                if after != at || self.projects.has_dialog() || self.threads.has_editor() {
                    self.drawer = false;
                }
                if !self.drawer {
                    self.drawer_at = None;
                }
                self.report_view(None);
                #[cfg(target_os = "android")]
                self.android_ime.finish_frame(ctx);
                return;
            }
        } else {
            self.drawer = false;
            self.sidebar(ctx);
        }
        self.header(ctx);
        self.show_notice(ctx);
        let detail = self.has_detail_panel();
        if detail {
            self.reader.preview.hide();
            self.reader.browser.hide();
        }
        let mut attachment = None;
        if self.reader.open && !detail {
            self.reader.chat_files = self.chat_files();
            let state = &self.view.state;
            self.reader.library_root = state.thread.project.clone().map(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Project".into());
                (p, name)
            });
        }
        if self.reader.preview.active() && (self.page != Page::Chat || detail) {
            self.reader.preview.hide();
            self.reader.browser.hide();
        } else if !detail {
            let plan = (self.page == Page::Chat).then_some((&mut self.plan, &self.view.state.thread, self.view.state.goal.as_str(), &mut self.android_ime));
            self.reader.show(ctx, frame, &self.root, &mut attachment, plan, &self.view.state.preferences.browser_path);
        }
        match self.reader.picked.take() {
            Some(side_tabs::Pick::PlanSteps(steps)) => self.set_steps(steps),
            Some(side_tabs::Pick::RevisePlan { change, replace_goal }) => {
                let command = if replace_goal { "/goal replace" } else { "/plan revise" };
                self.send(format!("{command} {change}"));
            }
            Some(side_tabs::Pick::ChatFile { name, disk, text }) => {
                let html = crate::html_preview::is_html(Path::new(&name));
                let source = || {
                    disk.as_ref()
                        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= 2 * 1024 * 1024))
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_else(|| text.clone())
                };
                if html {
                    self.reader.open_output(name.clone(), source());
                    self.reader.preview.set_disk(disk);
                    self.page = Page::Chat;
                } else if crate::documents::readable(&name) {
                    match disk {
                        Some(path) => self.reader.load(path),
                        None => self.reader.load_snapshot(name, text),
                    }
                } else {
                    let body = source();
                    self.reader.open_output(name, body);
                    self.reader.preview.show_source();
                }
            }
            Some(side_tabs::Pick::Path(path)) => {
                let name = path.to_string_lossy().into_owned();
                if crate::documents::readable(&name) {
                    self.reader.load(path);
                    if self.reader.preview.active() {
                        self.page = Page::Chat;
                    }
                } else {
                    match std::fs::metadata(&path) {
                        Ok(m) if m.len() <= 2 * 1024 * 1024 => match std::fs::read_to_string(&path)
                        {
                            Ok(body) => {
                                let title = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or(name);
                                self.reader.open_output(title, body);
                                self.reader.preview.show_source();
                            }
                            Err(_) => self.notice = "That file isn't text Spindle can show".into(),
                        },
                        _ => self.notice = "That file is too large to show here".into(),
                    }
                }
            }
            Some(side_tabs::Pick::Reveal(path)) => {
                if let Err(e) = reveal(&path) {
                    self.notice = format!("Could not open the folder: {e}");
                }
            }
            None => {}
        }
        if std::mem::take(&mut self.reader.preview_requested) {
            self.reader.load(self.reader.file.clone());
            self.page = Page::Chat;
        }
        if let Some(cmd) = attachment {
            self.page = Page::Chat;
            self.send(cmd);
        }
        if self.page == Page::Chat && !detail {
            self.composer(ctx);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(surface())
                    .inner_margin(if narrow(ctx) { 10 } else { 24 }),
            )
            .show(ctx, |ui| {
                if detail {
                    self.render_detail_panel(ui);
                    return;
                }
                match self.page {
                    Page::Chat => self.conversation(ui),
                    Page::Queue => {
                        egui::ScrollArea::vertical().show(ui, |ui| self.queue(ui));
                    }
                    Page::Automations => {
                        egui::ScrollArea::vertical()
                            .id_salt("automations")
                            .show(ui, |ui| self.automations_page(ui));
                    }
                    Page::Settings => {
                        egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui));
                    }
                    Page::Projects => {
                        egui::ScrollArea::vertical().show(ui, |ui| self.projects_page(ui));
                    }
                    Page::Project => {
                        egui::ScrollArea::vertical().show(ui, |ui| self.project_home(ui));
                    }
                    Page::Skills => {
                        egui::ScrollArea::vertical().show(ui, |ui| self.skills(ui));
                    }
                }
            });
        let viewing = self.page == Page::Chat
            && self.view.state.thread.started
            && !self.has_detail_panel()
            && (!narrow(ctx) || !self.reader.open)
            && ctx.input(|i| i.focused);
        self.report_view(viewing.then_some(self.view.state.chat_id));
        #[cfg(target_os = "android")]
        self.android_ime.finish_frame(ctx);
    }
}

/// Save a generated file to a location the user picks. Off Windows, copy it to Downloads.
fn super_download(
    name: &str,
    disk: Option<&std::path::Path>,
    snapshot: &str,
) -> anyhow::Result<Option<PathBuf>> {
    #[cfg(target_os = "android")]
    if name.ends_with(".apk") {
        let path = disk.ok_or_else(|| anyhow::anyhow!("This APK is unavailable on this device"))?;
        let bytes = std::fs::read(path)?;
        return crate::android::download_apk(name, &bytes).map(|path| Some(path.into()));
    }
    let bytes = match disk {
        Some(path) => std::fs::read(path)?,
        None => snapshot.as_bytes().to_vec(),
    };
    #[cfg(windows)]
    let target = {
        let mut dialog = rfd::FileDialog::new().set_file_name(name);
        if let Some(dir) = std::env::var_os("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Downloads"))
            .filter(|d| d.is_dir())
        {
            dialog = dialog.set_directory(dir);
        }
        dialog.save_file()
    };
    #[cfg(not(windows))]
    let target =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads").join(name));
    let Some(target) = target else {
        return Ok(None);
    };
    std::fs::write(&target, bytes)?;
    Ok(Some(target))
}

/// Open a folder in the system file manager.
impl Desktop {
    /// Speech settings, with models picked from what Ollama reports.
    fn speech_settings(&self) -> crate::speech::Settings {
        let local = |set: &std::collections::BTreeSet<String>| -> Vec<String> {
            set.iter()
                .filter(|m| route(m).0 == "ollama")
                .cloned()
                .collect()
        };
        crate::speech::Settings::from(
            &self.view.state.preferences,
            &local(&self.view.speech),
            &local(&self.view.audio),
        )
    }

    fn poll_voice(&mut self, ctx: &egui::Context) {
        let polled = self.voice.poll(ctx);
        if let Some(text) = polled.dictated {
            if text.is_empty() {
                self.notice = "No speech was heard in the recording".into();
            } else {
                if !self.draft.is_empty() && !self.draft.ends_with([' ', '\n']) {
                    self.draft.push(' ');
                }
                self.draft.push_str(&text);
                self.focus = true;
            }
        }
        for (chat, name, text) in polled.attached {
            let files = self.paste_drafts.files.entry(chat).or_default();
            let title = format!("{name} (transcript)");
            files.retain(|f| f.name != title);
            files.push(paste::Draft {
                name: title,
                body: if text.is_empty() {
                    "(No speech was heard in this recording.)".into()
                } else {
                    text
                },
            });
            self.notice =
                format!("Transcribed {name}. The transcript goes with your next message.");
        }
        if let Some(e) = polled.error {
            self.notice = e;
        }
        let state = &self.view.state;
        if state.preferences.read_aloud
            && let Some(i) = self
                .voice
                .should_read(state.chat_id, &state.history, self.view.busy)
        {
            let text = state.history[i].content.clone();
            let at = (state.chat_id, i);
            let settings = self.speech_settings();
            if !settings.tts_model.is_empty() {
                self.voice.speak(ctx, settings, text, at);
            }
        }
    }

    /// Files attached to and made in the current chat, for the side panel's Files tab.
    fn chat_files(&self) -> Vec<side_tabs::ChatFile> {
        let state = &self.view.state;
        let files_root = state.files_root(&self.root, state.chat_id);
        let disk =
            |name: &str| crate::artifacts::existing_file(&files_root, state.chat_id, name).ok();
        let mut out: Vec<side_tabs::ChatFile> = vec![];
        let mut add = |name: &str, input: bool, text: &str, preview: bool| {
            if out.iter().any(|f| f.name == name && f.input == input) {
                return;
            }
            let path = if preview { preview_disk(state, &self.root, name) } else { disk(name) };
            let size = if text.is_empty() {
                path.as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len() as usize)
                    .unwrap_or(0)
            } else {
                text.len()
            };
            out.push(side_tabs::ChatFile {
                name: name.to_owned(),
                input,
                disk: path,
                text: text.to_owned(),
                size,
            });
        };
        for (index, message) in state.history.iter().enumerate() {
            if message.role == "user" {
                if let Some(t) = state.thread.turns.get(&(index + 1)) {
                    for p in &t.pastes {
                        add(&p.path, true, &p.text, false);
                    }
                }
            } else if let Some(t) = state.thread.turns.get(&index) {
                for a in &t.artifacts {
                    add(&a.path, false, &a.text, false);
                }
                for a in &t.html_previews {
                    add(&a.path, false, &a.text, true);
                }
                for name in &t.files {
                    if name != "_spindle_run.py" {
                        add(name, false, "", false);
                    }
                }
            }
        }
        out
    }
}

fn preview_disk(state: &crate::state::State, workspace: &Path, name: &str) -> Option<PathBuf> {
    let (root, project) = crate::engine::tool_folder(state, state.chat_id, Some(workspace))?;
    let resolve = |root: &Path| {
        let (path, inside) = crate::tools::resolve(root, name).ok()?;
        (inside && path.is_file() && crate::html_preview::is_html(&path)).then_some(path)
    };
    if project && crate::execution::ready(state) {
        let copy = crate::workcopy::folder(&crate::runtime::data_dir(), state.chat_id, &root);
        if let Some(path) = resolve(&copy) { return Some(path); }
    }
    resolve(&root)
}

fn reveal_folder(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()?;
    #[cfg(not(windows))]
    std::process::Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

/// Open the system file manager with the file selected.
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        std::process::Command::new("explorer.exe")
            .arg(arg)
            .spawn()?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    #[test]
    fn workspace_accent_reaches_the_window_and_chat_logo() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _commands) = tokio::sync::mpsc::unbounded_channel();
        let (views, rx) = std::sync::mpsc::channel();
        let (_stream, preview) = tokio::sync::watch::channel(String::new());
        let mut app = Desktop::new(&cc, dir.path().into(), tx, rx, preview, Accent::Green);
        assert_eq!(app.logo.accent, Accent::Green);
        assert_eq!(app.view.state.preferences.accent, Accent::Green);
        let mut frame = eframe::Frame::_new_kittest();
        for selected in Accent::ALL {
            let previous = app.logo.texture.id();
            let mut view = View::default();
            view.state.preferences.accent = selected;
            views.send(view).unwrap();
            let output = ctx.run(Default::default(), |ctx| {
                eframe::App::update(&mut app, ctx, &mut frame);
            });
            assert_eq!(app.logo.accent, selected);
            assert_ne!(app.logo.texture.id(), previous);
            assert!(output.viewport_output[&egui::ViewportId::ROOT].commands.iter().any(|cmd| {
                matches!(cmd, egui::ViewportCommand::Icon(Some(icon)) if std::sync::Arc::ptr_eq(icon, &app.logo.window))
            }));
        }
    }
}

/// Slash commands offered while typing the first word of a message.
const COMMANDS: &[(&str, &str)] = &[
    ("/new", "Start a new chat"),
    ("/model", "Choose a different model"),
    ("/compact", "Summarize older messages now to free context"),
    ("/undo", "Remove your last message and edit it"),
    ("/fork", "Branch this conversation into a new chat"),
    ("/rename", "Rename this conversation"),
    ("/settings", "Conversation settings"),
    ("/open", "Open a document in the reader"),
    ("/init", "Set up this project's instructions"),
    ("/share", "Copy this conversation as Markdown"),
    ("/plan ", "Draft a plan for a task"),
    ("/remember ", "Save something to memory"),
    ("/forget ", "Forget a memory by its number"),
    ("/git", "Show this project's git status"),
    ("/review", "Review uncommitted changes"),
    (
        "/permissions",
        "What's waiting for approval and what's allowed",
    ),
    ("/copy", "Copy this conversation as Markdown"),
    #[cfg(not(target_os = "android"))]
    ("/apply", "Apply the sandbox copy's changes to the project"),
    #[cfg(not(target_os = "android"))]
    ("/discard", "Throw away the sandbox copy"),
    #[cfg(not(target_os = "android"))]
    (
        "/sandbox-check",
        "Check that the sandbox keeps its promises",
    ),
    #[cfg(any(all(windows, target_arch = "x86_64"), all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))))]
    ("/compiler-install", "Install the compiler for sandboxed native builds"),
    #[cfg(not(target_os = "android"))]
    ("/stop-run", "Stop code running in the sandbox"),
    ("/goal ", "Set a continuing goal for this chat"),
    ("/plan revise ", "Revise the current plan while working"),
    ("/remote ", "Build or run a script on a paired device"),
    ("/image ", "Generate an image with ComfyUI"),
    ("/image edit ", "Edit an attached image or a workspace file"),
    ("/image server ", "Set the local ComfyUI address"),
    ("/council", "Independent review of the last answer"),
    ("/android-apk ", "Choose an Android APK build workflow"),
    ("/browser-use ", "Use Spindle’s browser or Browser Link"),
    ("/swarm ", "Ask worker models, then combine their findings"),
    ("/steer ", "Correct the answer in progress"),
    ("/queue", "Show or manage queued messages"),
    ("/cancel", "Stop the current task"),
    ("/resume", "Release held work"),
    ("/compact status", "Show the compaction setting"),
    ("/read ", "Read a workspace file into the reader"),
    ("/add ", "Attach a workspace file to the next message"),
    ("/preview ", "Open a workspace file in the side panel"),
    ("/skills", "List, build and install skills"),
    ("/defer ", "Pause this task until a date and time"),
    ("/usage", "Usage and limits"),
    ("/automation", "Schedules and automations"),
    ("/threads", "List conversations"),
    ("/status", "Model, queue and work status"),
    ("/help", "Every command and its options"),
];

/// How narrow and wide the sidebar may be dragged in a window of this width.
#[cfg(target_os = "android")]
impl Desktop {
    /// The camera scanner for the pairing QR code: a preview while it looks, then joins.
    fn scanner_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        use crate::scan::State;
        let state = crate::scan::state();
        if state == State::Off {
            return;
        }
        if let State::Found(text) = &state {
            crate::scan::end();
            if text.starts_with("spindle://pair") {
                self.page = Page::Settings;
                self.sync_tab = true;
                self.integration_tab = None;
                self.send(format!(
                    "/sync join {text} {}",
                    crate::sync::live::hostname()
                ));
            } else {
                self.notice = "That QR code isn't a Spindle pairing code".into();
            }
            return;
        }
        let preview = crate::scan::tick();
        ctx.request_repaint();
        let mut open = true;
        // One texture kept between frames and updated in place; a new one each frame was
        // freed before it reached the screen, which flickered.
        static TEXTURE: std::sync::Mutex<Option<egui::TextureHandle>> = std::sync::Mutex::new(None);
        detail_panel(ui, "Scan the pairing QR code", "Close", &mut open, |ui| {
            match &state {
                State::Asking => {
                    ui.label("Allow Spindle to use the camera.");
                }
                State::Failed(e) => {
                    ui.label(Text::new(e).color(egui::Color32::from_rgb(232, 90, 90)));
                }
                _ => {
                    ui.label("On the other device: Settings › Sync › Add a device. Point the camera at its QR code.");
                }
            }
            let mut tex = TEXTURE.lock().unwrap();
            if let Some(p) = preview {
                let image = egui::ColorImage::from_gray([p.width, p.height], &p.grey);
                match tex.as_mut() {
                    Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        *tex = Some(ctx.load_texture(
                            "pairing-scan",
                            image,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                }
            }
            if let Some(t) = tex.as_ref() {
                // As wide as the window, no taller than half the screen.
                let [tw, th] = t.size();
                let mut w = ui.available_width();
                let mut h = w * th as f32 / tw as f32;
                let max_h = ctx.screen_rect().height() * 0.5;
                if h > max_h {
                    w *= max_h / h;
                    h = max_h;
                }
                ui.vertical_centered(|ui| {
                    ui.image((t.id(), egui::vec2(w, h)));
                });
            } else {
                ui.weak("Starting the camera…");
            }
            if ui.button("Cancel").clicked() {
                crate::scan::end();
            }
        });
        if !open || crate::scan::state() == State::Off {
            crate::scan::end();
            *TEXTURE.lock().unwrap() = None;
        }
    }
}

/// Narrower than this (a phone), the window uses one column: the sidebar becomes a drawer
/// and side panels cover the conversation.
pub(super) const NARROW: f32 = 640.;

pub(super) fn narrow(ctx: &egui::Context) -> bool {
    ctx.screen_rect().width() < NARROW
}

fn sidebar_range(screen: f32) -> (f32, f32) {
    let max = (screen * 0.4).clamp(200., 440.);
    (160_f32.min(max), max)
}

/// Keyboard shortcut shown beside a command in the list.
fn shortcut(command: &str) -> &'static str {
    match command {
        "/new" => "Ctrl+N",
        "/model" => "Ctrl+K",
        "/open" => "Ctrl+O",
        _ => "",
    }
}

/// Commands and installed skills that start with the word being typed. Empty once the
/// draft has a space or newline, or doesn't start with "/".
fn command_matches(
    draft: &str,
    skills: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let Some(word) = draft.strip_prefix('/') else {
        return vec![];
    };
    if word.contains(char::is_whitespace) {
        return vec![];
    }
    let word = word.to_lowercase();
    let mut out: Vec<(String, String)> = COMMANDS
        .iter()
        .filter(|(c, _)| {
            c[1..].starts_with(&word) && (!c.contains(' ') || c.ends_with(' ') || !word.is_empty())
        })
        .map(|(c, d)| ((*c).to_owned(), (*d).to_owned()))
        .collect();
    out.extend(
        skills
            .keys()
            .filter(|name| {
                !crate::state::BUILTIN_SKILLS.iter().any(|(key, _)| *key == name.as_str())
                    && name.to_lowercase().starts_with(&word)
            })
            .map(|name| (format!("/{name} "), "Installed skill".to_owned())),
    );
    out
}

#[cfg(test)]
mod command_tests {
    #[test]
    fn slash_lists_commands_and_skills_by_prefix() {
        let mut skills = std::collections::BTreeMap::new();
        skills.insert("release-notes".to_owned(), String::new());
        let all = super::command_matches("/", &skills);
        assert!(all.iter().any(|(c, _)| c == "/new"));
        assert!(all.iter().any(|(c, _)| c == "/release-notes "));
        assert_eq!(all[0].0, "/new");
        assert!(all.iter().any(|(c, _)| c == "/model") && super::shortcut("/model") == "Ctrl+K");
        let co = super::command_matches("/co", &skills);
        assert!(co.iter().all(|(c, _)| c.starts_with("/co")));
        assert!(co.iter().any(|(c, _)| c == "/council"));
        assert!(super::command_matches("/plan fix the bug", &skills).is_empty());
        assert!(super::command_matches("hello", &skills).is_empty());
    }
}

const MAX_IMAGE: usize = 6 * 1024 * 1024;

fn image_file(path: &std::path::Path) -> bool {
    matches!(
        crate::documents::extension(path).as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp"
    )
}

/// True for PNG, JPEG, GIF or WebP bytes.
fn image_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF8")
        || (bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

/// An image on the system clipboard, encoded as PNG. (Android: not available.)
#[cfg(target_os = "android")]
fn clipboard_image() -> Option<Vec<u8>> {
    None
}

/// An image on the system clipboard, encoded as PNG.
#[cfg(not(target_os = "android"))]
fn clipboard_image() -> Option<Vec<u8>> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if clipboard.get_text().is_ok_and(|t| !t.is_empty()) {
        return None;
    }
    let data = clipboard.get_image().ok()?;
    let img = image::RgbaImage::from_raw(
        data.width as u32,
        data.height as u32,
        data.bytes.into_owned(),
    )?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

impl Desktop {
    fn attach_image_file(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(bytes) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.add_image(ctx, name, bytes);
            }
            Err(e) => self.notice = format!("Could not read the image: {e}"),
        }
    }

    fn add_image(&mut self, ctx: &egui::Context, name: String, bytes: Vec<u8>) {
        if !image_bytes(&bytes) {
            self.notice = "Attach a PNG, JPEG, GIF or WebP image".into();
            return;
        }
        if bytes.len() > MAX_IMAGE {
            self.notice = "Images must be 6 MB or smaller".into();
            return;
        }
        // The same picture pasted again (or one paste seen twice) isn't added twice.
        if self.image_drafts.iter().any(|(_, b, _)| *b == bytes) {
            self.notice = "That image is already attached".into();
            return;
        }
        if self.image_drafts.len() >= 4 {
            self.notice = "A message can carry up to four images".into();
            return;
        }
        let thumb = image::load_from_memory(&bytes).ok().map(|img| {
            let small = img.thumbnail(96, 96).to_rgba8();
            let size = [small.width() as usize, small.height() as usize];
            ctx.load_texture(
                format!("attachment-{}", self.image_drafts.len()),
                egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw()),
                egui::TextureOptions::LINEAR,
            )
        });
        self.image_drafts.push((name, bytes, thumb));
        self.focus = true;
    }

    /// Thumbnails of images waiting to be sent, with a remove button and a warning when the
    /// selected model can't see images.
    fn image_chips(&mut self, ui: &mut egui::Ui) {
        if self.image_drafts.is_empty() {
            return;
        }
        let mut remove = None;
        // Chips of one fixed size, top-aligned, so a row of them lines up.
        ui.with_layout(
            egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
            |ui| {
                for (i, (name, bytes, thumb)) in self.image_drafts.iter().enumerate() {
                    egui::Frame::new()
                        .fill(raised())
                        .stroke(egui::Stroke::new(1.0_f32, line()))
                        .corner_radius(8)
                        .inner_margin(4)
                        .show(ui, |ui| {
                            ui.set_height(44.);
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    match thumb {
                                        Some(t) => {
                                            let scale = 40. / t.size_vec2().max_elem();
                                            ui.image((t.id(), t.size_vec2() * scale));
                                        }
                                        None => symbol(ui, Icon::Eye, muted()),
                                    }
                                    ui.vertical(|ui| {
                                        ui.label(Text::new(name).size(12.));
                                        ui.weak(format!("{:.0} KB", bytes.len() as f64 / 1024.));
                                    });
                                    if small(ui, Icon::Trash, "Remove image") {
                                        remove = Some(i);
                                    }
                                },
                            );
                        });
                }
            },
        );
        if let Some(i) = remove {
            self.image_drafts.remove(i);
        }
        if !self.view.vision.contains(&self.view.state.model) {
            ui.label(
                Text::new("The selected model can't see images. Pick one with the eye icon.")
                    .size(12.)
                    .color(Color::from_rgb(232, 178, 64)),
            );
        }
    }
}

#[cfg(test)]
mod image_tests {
    #[test]
    fn only_real_image_bytes_attach() {
        assert!(super::image_bytes(b"\x89PNG\r\n\x1a\n...."));
        assert!(super::image_bytes(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(super::image_bytes(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!super::image_bytes(b"%PDF-1.7"));
        assert!(super::image_file(std::path::Path::new("shot.PNG")));
        assert!(!super::image_file(std::path::Path::new("notes.md")));
    }
}

impl Desktop {
    /// Running in the background: closing the window hides it to the tray, which opens it
    /// again or exits for real (holding running work).
    fn background(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let on = self.view.state.preferences.background;
        if on && self.tray.is_none() {
            let (ctx2, tx) = (ctx.clone(), self.tx.clone());
            let shutdown = self.shutdown.clone();
            #[cfg(windows)]
            let hwnd = window_handle(frame);
            #[cfg(not(windows))]
            let _ = frame;
            self.tray = crate::tray::start(move |a| match a {
                crate::tray::Action::Open => {
                    #[cfg(windows)]
                    if let Some(hwnd) = hwnd {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_RESTORE, ShowWindowAsync};
                        unsafe { ShowWindowAsync(hwnd as _, SW_RESTORE) };
                    }
                    ctx2.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx2.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx2.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx2.request_repaint();
                }
                crate::tray::Action::Pause => {
                    let _ = tx.send("/sync pause".into());
                }
                crate::tray::Action::Resume => {
                    let _ = tx.send("/sync resume".into());
                }
                crate::tray::Action::Exit => {
                    if tx.send("/exit".into()).is_err() {
                        shutdown.finish();
                    }
                }
            });
        } else if !on {
            self.tray = None;
        }
        if let Some(t) = &self.tray {
            t.set(crate::tray::status_of(&self.view));
        }
        if on
            && self.tray.is_some()
            && ctx.input(|i| i.viewport().close_requested())
        {
            self.report_view(None);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.notice =
                "Spindle keeps running in the background. Open it from the tray icon.".into();
        }
        if self.tray.is_some() {
            // Keep hearing the tray while hidden.
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}
