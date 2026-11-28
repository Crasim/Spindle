//! The tray icon shown while Spindle runs in the background: Open Spindle, status lines,
//! Pause/Resume, and Exit Spindle (a full exit, holding any running work).
//!
//! Linux uses a StatusNotifierItem over D-Bus (`ksni`, no GTK); Windows uses `tray-icon` on a
//! thread of its own with a message loop. Either way the tray runs beside whatever owns the
//! workspace: the headless service or the window.

use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    Pause,
    Resume,
    /// Exit the workspace, holding queued work for later.
    Exit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mood {
    #[default]
    Idle,
    Syncing,
    Working,
    Problem,
}

/// What the tray shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub mood: Mood,
    /// "Synced 2 min ago", "Couldn't sync: …", "Working: <chat>".
    pub lines: Vec<String>,
    pub paused: bool,
}

type OnAction = Arc<dyn Fn(Action) + Send + Sync>;

/// A running tray; drop it to remove the icon.
pub struct Tray {
    status: Arc<Mutex<Status>>,
    #[cfg(target_os = "linux")]
    handle: Option<ksni::blocking::Handle<Linux>>,
    #[cfg(windows)]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(windows)]
    stopped: std::sync::mpsc::Receiver<()>,
}

impl Tray {
    /// Update what the tray shows.
    pub fn set(&self, s: Status) {
        if *self.status.lock().unwrap() == s {
            return;
        }
        *self.status.lock().unwrap() = s;
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.handle {
            let _ = h.update(|_| {});
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(h) = self.handle.take() {
            let _ = h.shutdown();
        }
        #[cfg(windows)]
        {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            // Let the tray thread unregister its icon before the process ends. An open
            // Windows menu can hold that thread, so keep the wait bounded.
            let _ = self.stopped.recv_timeout(std::time::Duration::from_millis(250));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    Black,
    White,
    Outlined,
}

/// Render the 64 px artwork and its status glyph at 32 px for the tray.
fn pixels(ink: Ink, mood: Mood) -> (u32, u32, Vec<u8>) {
    let (png, color): (&[u8], u8) = match ink {
        Ink::Black => (include_bytes!("../assets/spindle-tray-black.png"), 0),
        Ink::White | Ink::Outlined => (include_bytes!("../assets/spindle-tray-white.png"), 255),
    };
    let mut img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("embedded Spindle tray icon")
        .into_rgba8();
    if mood != Mood::Idle {
        for y in 46..64 {
            for x in 46..64 {
                let (dx, dy) = (x as i32 - 55, y as i32 - 55);
                let radius_sq = dx * dx + dy * dy;
                let visible = match mood {
                    Mood::Syncing => (16..=49).contains(&radius_sq),
                    Mood::Working => radius_sq <= 36,
                    Mood::Problem => dx.abs() <= 2 && ((-7..=1).contains(&dy) || (4..=7).contains(&dy)),
                    Mood::Idle => false,
                };
                if radius_sq <= 81 {
                    img.put_pixel(x, y, image::Rgba([color, color, color, if visible { 255 } else { 0 }]));
                }
            }
        }
    }
    if ink == Ink::Outlined {
        // Some tray hosts provide no panel color. A one-pixel keyline works on either background.
        let mut outline = image::RgbaImage::new(img.width(), img.height());
        for (x, y, pixel) in outline.enumerate_pixels_mut() {
            let mut alpha = 0;
            for row in y.saturating_sub(2)..=(y + 2).min(img.height() - 1) {
                for col in x.saturating_sub(2)..=(x + 2).min(img.width() - 1) {
                    alpha = alpha.max(img.get_pixel(col, row)[3]);
                }
            }
            *pixel = image::Rgba([0, 0, 0, alpha]);
        }
        image::imageops::overlay(&mut outline, &img, 0, 0);
        img = outline;
    }
    let icon = image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Triangle);
    (icon.width(), icon.height(), icon.into_raw())
}

fn sync_action(paused: bool) -> (&'static str, Action) {
    if paused {
        ("Resume sync and remote work", Action::Resume)
    } else {
        ("Pause sync and remote work", Action::Pause)
    }
}

const EXIT_LABEL: &str = "Exit Spindle";

/// The Linux menu's lines and optional actions.
#[cfg(any(target_os = "linux", test))]
fn menu_lines(s: &Status) -> Vec<(String, Option<Action>)> {
    let mut out = vec![("Open Spindle".into(), Some(Action::Open))];
    out.extend(s.lines.iter().cloned().map(|line| (line, None)));
    let (label, action) = sync_action(s.paused);
    out.push((label.into(), Some(action)));
    out.push((EXIT_LABEL.into(), Some(Action::Exit)));
    out
}

// ------------------------------------------------------------------ Linux

#[cfg(target_os = "linux")]
pub struct Linux {
    status: Arc<Mutex<Status>>,
    on: OnAction,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for Linux {
    fn id(&self) -> String {
        "spindle".into()
    }
    fn title(&self) -> String {
        "Spindle".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let (w, h, rgba) = pixels(Ink::Outlined, self.status.lock().unwrap().mood);
        // ARGB in network byte order.
        let data = rgba
            .chunks(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect();
        vec![ksni::Icon {
            width: w as i32,
            height: h as i32,
            data,
        }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let s = self.status.lock().unwrap();
        ksni::ToolTip {
            title: "Spindle".into(),
            description: s.lines.join("\n"),
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on)(Action::Open);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let s = self.status.lock().unwrap().clone();
        let mut items = vec![];
        for (label, action) in menu_lines(&s) {
            if action == Some(Action::Exit) || (action.is_none() && items.len() == 1) {
                items.push(ksni::MenuItem::Separator);
            }
            items.push(
                StandardItem {
                    label,
                    enabled: action.is_some(),
                    activate: Box::new(move |t: &mut Self| {
                        let Some(a) = action else { return };
                        (t.on)(a);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }
}

/// Show the tray icon. Returns `None` where there is no tray (no desktop session).
pub fn start(on: impl Fn(Action) + Send + Sync + 'static) -> Option<Tray> {
    let status = Arc::new(Mutex::new(Status::default()));
    #[cfg(target_os = "linux")]
    {
        use ksni::blocking::TrayMethods;
        let t = Linux {
            status: status.clone(),
            on: Arc::new(on),
        };
        // Without a D-Bus session (a server with no desktop) there's simply no tray.
        let handle = t.spawn().ok();
        let ok = handle.is_some();
        let tray = Tray { status, handle };
        return ok.then_some(tray);
    }
    #[cfg(windows)]
    {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s2, st2) = (status.clone(), stop.clone());
        let on: OnAction = Arc::new(on);
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let (done, stopped) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("spindle-tray".into())
            .spawn(move || {
                windows_tray(s2, on, st2, ready);
                let _ = done.send(());
            })
            .ok()?;
        if started.recv_timeout(std::time::Duration::from_secs(2)).is_err() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        return Some(Tray { status, stop, stopped });
    }
    #[allow(unreachable_code)]
    {
        let _ = (status, on);
        None
    }
}

// ------------------------------------------------------------------ Windows

#[cfg(windows)]
fn taskbar_ink() -> Ink {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        .encode_utf16()
        .chain([0])
        .collect();
    let name: Vec<u16> = "SystemUsesLightTheme".encode_utf16().chain([0]).collect();
    let mut light = 0u32;
    let mut bytes = std::mem::size_of_val(&light) as u32;
    // The pointers refer to terminated UTF-16 strings and a four-byte DWORD buffer.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut light as *mut u32).cast(),
            &mut bytes,
        )
    };
    match (result, light) {
        (0, 0) => Ink::White,
        (0, 1) => Ink::Black,
        _ => Ink::Outlined,
    }
}

#[cfg(windows)]
fn windows_tray(
    status: Arc<Mutex<Status>>,
    on: OnAction,
    stop: Arc<std::sync::atomic::AtomicBool>,
    ready: std::sync::mpsc::SyncSender<()>,
) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
    use std::time::{Duration, Instant};
    let icon = |ink, mood| {
        let (w, h, rgba) = pixels(ink, mood);
        Icon::from_rgba(rgba, w, h).ok()
    };
    let mut shown = Status::default();
    let mut ink = taskbar_ink();
    let mut theme_checked = Instant::now();
    // Keep action IDs stable while the tray refreshes its displayed status.
    let menu = Menu::new();
    let open = MenuItem::new("Open Spindle", true, None);
    let sync = MenuItem::new(sync_action(false).0, true, None);
    let exit = MenuItem::new(EXIT_LABEL, true, None);
    if menu.append_items(&[&open, &sync, &exit]).is_err() {
        return;
    }
    let mut status_items = Vec::new();
    let mut builder = TrayIconBuilder::new()
        .with_tooltip("Spindle")
        .with_menu(Box::new(menu.clone()));
    if let Some(i) = icon(ink, Mood::Idle) {
        builder = builder.with_icon(i);
    }
    let Ok(tray) = builder.build() else { return };
    if ready.send(()).is_err() {
        return;
    }
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        // A message loop for the tray's hidden window.
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
            };
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if &e.id == open.id() {
                on(Action::Open);
            } else if &e.id == sync.id() {
                on(sync_action(shown.paused).1);
            } else if &e.id == exit.id() {
                on(Action::Exit);
            }
        }
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if matches!(e, TrayIconEvent::DoubleClick { .. }) {
                on(Action::Open);
            }
        }
        let now = status.lock().unwrap().clone();
        let previous_ink = ink;
        if theme_checked.elapsed() >= Duration::from_secs(1) {
            ink = taskbar_ink();
            theme_checked = Instant::now();
        }
        if ink != previous_ink || now.mood != shown.mood {
            let _ = tray.set_icon(icon(ink, now.mood));
        }
        if now != shown {
            let _ = tray.set_tooltip(Some(format!("Spindle\n{}", now.lines.join("\n"))));
            if now.lines != shown.lines {
                for item in status_items.drain(..) {
                    let _ = menu.remove(&item);
                }
                for (index, line) in now.lines.iter().enumerate() {
                    let item = MenuItem::new(line, false, None);
                    if menu.insert(&item, index + 1).is_ok() {
                        status_items.push(item);
                    }
                }
            }
            sync.set_text(sync_action(now.paused).0);
            shown = now;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

/// What the tray shows for the latest view.
pub fn status_of(v: &crate::engine::View) -> Status {
    let mut lines = vec![];
    let mut mood = if v.busy { Mood::Working } else { Mood::Idle };
    let mut paused = false;
    if let Some(s) = &v.state.space {
        paused = s.paused;
        if s.status.syncing {
            mood = Mood::Syncing;
        }
        match (&s.status.error, s.status.last_ok) {
            (Some(e), _) => {
                mood = Mood::Problem;
                lines.push(format!(
                    "Couldn't sync: {}",
                    e.chars().take(80).collect::<String>()
                ));
            }
            (None, Some(t)) => lines.push(format!("Synced {}", crate::automation::local_time(t))),
            (None, None) => {}
        }
        if paused {
            lines.push("Sync is paused".into());
        }
    }
    if let Some((thread, history)) = v.state.queue.front().and_then(|task| {
        v.state.thread_data(task.origin.unwrap_or(v.state.chat_id))
    }) {
        let label: String = thread.label(history).chars().take(40).collect();
        let status = if v.state.held { "On hold" } else if v.busy { "Working" } else { "Queued" };
        lines.push(format!("{status}: {label}"));
    }
    let waiting = crate::attention::waiting(v).len();
    if waiting > 0 {
        mood = Mood::Problem;
        lines.insert(0, format!("Needs attention · {waiting} conversation{}", if waiting == 1 { "" } else { "s" }));
    }
    Status {
        mood,
        lines,
        paused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_offers_exit_with_queued_work() {
        let s = Status {
            paused: true,
            lines: vec!["Synced 1 min ago".into(), "On hold: Snake".into()],
            ..Default::default()
        };
        let lines = menu_lines(&s);
        assert_eq!(lines[0].1, Some(Action::Open));
        assert!(
            lines
                .iter()
                .any(|(l, a)| l.starts_with("Resume") && *a == Some(Action::Resume))
        );
        assert_eq!(lines.last(), Some(&("Exit Spindle".into(), Some(Action::Exit))));
    }

    #[test]
    fn tray_variants_keep_the_same_silhouette_and_monochrome_status_marks() {
        let mut statuses = std::collections::HashSet::new();
        for mood in [Mood::Idle, Mood::Syncing, Mood::Working, Mood::Problem] {
            let (w, h, white) = pixels(Ink::White, mood);
            let (_, _, black) = pixels(Ink::Black, mood);
            assert_eq!((w, h, white.len()), (32, 32, 32 * 32 * 4));
            for (white, black) in white.chunks_exact(4).zip(black.chunks_exact(4)) {
                assert_eq!(white[3], black[3]);
                if white[3] != 0 {
                    assert_eq!(&white[..3], &[255; 3]);
                    assert_eq!(&black[..3], &[0; 3]);
                }
            }
            assert_eq!(white[3], 0);
            assert!(statuses.insert(white));
        }
    }
}
