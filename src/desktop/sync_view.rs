//! Settings › Sync, the sync control in the chat header, and the branch bar in chats.

use super::*;

#[derive(Default)]
pub(super) struct SyncForm {
    name: String,
    server: String,
    admin: String,
    user: String,
    password: String,
    join: String,
    join_where: String,
    join_user: String,
    join_password: String,
    confirm_remove: Option<String>,
    show_removed: bool,
    manage_device: Option<String>,
    provider_host: Option<String>,
    peer_dns: Option<String>,
    pair_number: String,
    /// The projects folder being typed (None: show the saved one or the suggestion).
    projects_dir: Option<String>,
    /// "Later" on the projects-folder question, for this session.
    projects_later: bool,
    /// The in-app folder browser (phones): the folder being looked at.
    pub(super) browse: Option<std::path::PathBuf>,
    /// A new folder's name, in the browser.
    pub(super) new_folder: String,
    /// The pairing QR code shown large.
    qr_big: bool,
    confirm_leave: bool,
    /// Renaming this device: the new name being typed.
    rename: Option<String>,
    peer: String,
    /// Check connection: the report, once the check is done.
    check: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    /// A branch being read: (chat, branch id).
    pub reading: Option<(u64, String)>,
}

fn when(t: i64) -> String {
    let ago = crate::automation::now() - t;
    match ago {
        i64::MIN..60 => "just now".into(),
        60..3600 => format!("{} min ago", ago / 60),
        3600..86_400 => format!("{} h ago", ago / 3600),
        _ => crate::automation::local_time(t),
    }
}

/// Draw a QR code from its modules.
/// Draw a QR code; returns whether it was clicked.
fn qr(ui: &mut egui::Ui, link: &str, size: f32) -> bool {
    let Ok((w, dark)) = crate::sync::pair::qr_modules(link) else {
        return false;
    };
    let quiet = 2;
    let cells = w + quiet * 2;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let clicked = response
        .on_hover_cursor(egui::CursorIcon::ZoomIn)
        .on_hover_text("Click to make it bigger or smaller")
        .clicked();
    let p = ui.painter();
    p.rect_filled(rect, 4, Color::WHITE);
    let cell = size / cells as f32;
    for y in 0..w {
        for x in 0..w {
            if dark[y * w + x] {
                let min =
                    rect.min + egui::vec2((x + quiet) as f32 * cell, (y + quiet) as f32 * cell);
                p.rect_filled(
                    egui::Rect::from_min_size(min, egui::vec2(cell + 0.5, cell + 0.5)),
                    0,
                    Color::BLACK,
                );
            }
        }
    }
    clicked
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(panel())
        .inner_margin(16)
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(Text::new(title).strong().size(18.));
            ui.add_space(4.);
            body(ui);
        });
    ui.add_space(12.);
}

fn device_icon(ui: &mut egui::Ui, os: &str) {
    let (bounds, response) = ui.allocate_exact_size(egui::vec2(27., 27.), egui::Sense::hover());
    let paint = ui.painter();
    let at = |x: f32, y: f32| bounds.min + egui::vec2(x, y);
    let rect = |left, top, right, bottom| egui::Rect::from_min_max(at(left, top), at(right, bottom));
    let name = match os {
        "windows" => {
            for (x, y) in [(3., 3.), (14., 3.), (3., 14.), (14., 14.)] {
                paint.rect_filled(rect(x, y, x + 9., y + 9.), 1., Color::from_rgb(63, 146, 235));
            }
            "Windows"
        }
        "linux" => {
            let charcoal = Color::from_rgb(55, 61, 70);
            paint.circle_filled(at(13., 15.), 10., charcoal);
            paint.circle_filled(at(13., 8.), 7., charcoal);
            paint.circle_filled(at(13., 17.), 6., Color::WHITE);
            for x in [9., 17.] {
                paint.circle_filled(at(x, 7.), 1.4, Color::WHITE);
                paint.circle_filled(at(x, 23.), 2.5, Color::from_rgb(235, 173, 69));
            }
            paint.rect_filled(rect(10., 10., 16., 13.), 2., Color::from_rgb(235, 173, 69));
            "Linux"
        }
        "android" => {
            let green = Color::from_rgb(64, 174, 119);
            paint.rect_filled(rect(3., 8., 24., 23.), 6., green);
            for (from, to) in [(at(6., 5.), at(9., 10.)), (at(21., 5.), at(18., 10.))] {
                paint.line_segment([from, to], egui::Stroke::new(1.6_f32, green));
            }
            for x in [9., 18.] { paint.circle_filled(at(x, 14.), 1.4, Color::WHITE); }
            "Android"
        }
        _ => {
            paint.rect_stroke(rect(4., 5., 23., 19.), 3., egui::Stroke::new(1.5_f32, muted()), egui::StrokeKind::Inside);
            paint.line_segment([at(9., 23.), at(18., 23.)], egui::Stroke::new(1.5_f32, muted()));
            if os == "macos" { "macOS" } else { "Device OS unknown" }
        }
    };
    response.on_hover_text(name);
}

impl Desktop {
    pub(super) fn sync_settings(&mut self, ui: &mut egui::Ui) {
        let mut send: Vec<String> = vec![];
        let mut pick_projects_folder = false;
        let state = self.view.state.clone();
        let f = &mut self.sync_form;
        if f.name.is_empty() {
            f.name = crate::sync::live::hostname();
        }
        let Some(sync) = state.space.as_ref() else {
            // Not set up: start a space, or join one.
            card(ui, "Sync this Spindle across your devices", |ui| {
                ui.weak("Conversations, projects, settings, skills, memories and API keys, end-to-end encrypted. Servers and shared folders only ever hold encrypted data.");
                ui.add_space(6.);
                ui.horizontal_wrapped(|ui| {
                    ui.label("This device's name");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.name)
                            .id_salt("sync_view-field-1")
                            .hint_text("This computer's name")
                            .desired_width((ui.available_width().max(230.)).min(200.)),
                    );
                    if ui
                        .add_enabled(
                            !f.name.trim().is_empty(),
                            egui::Button::new("Start syncing"),
                        )
                        .clicked()
                    {
                        send.push(format!("/sync create {}", f.name.trim()));
                    }
                });
            });
            card(ui, "Join from another device", |ui| {
                ui.weak("On the other device: Settings › Sync › Add a device. Paste its link here, or type its code and the address it shows.");
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut f.join)
                            .id_salt("sync_view-field-2")
                            .hint_text("spindle://pair… or ABCD-EFGH-JKLM")
                            .desired_width((ui.available_width().max(230.)).min(260.)),
                    );
                    // A phone: read the code off the other device's screen instead.
                    #[cfg(target_os = "android")]
                    if ui.button("Scan QR code").clicked() {
                        crate::scan::begin();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut f.join_where)
                            .id_salt("sync_view-field-3")
                            .hint_text("server or address (for a code)")
                            .desired_width((ui.available_width().max(230.)).min(200.)),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut f.name)
                            .id_salt("sync_view-field-4")
                            .hint_text("This device's name")
                            .desired_width((ui.available_width().max(230.)).min(200.)),
                    );
                    if ui
                        .add_enabled(!f.join.trim().is_empty(), egui::Button::new("Join"))
                        .clicked()
                    {
                        let name = if f.name.trim().is_empty() {
                            "New device".to_owned()
                        } else {
                            f.name.trim().replace(' ', "-")
                        };
                        send.push(
                            format!(
                                "/sync join {} {} {name}",
                                f.join.trim(),
                                f.join_where.trim()
                            )
                            .replace("  ", " "),
                        );
                    }
                });
                if let Some(n) = crate::sync::live::join_number() {
                    ui.label("Type this number on the other device:");
                    ui.label(Text::new(n.to_string()).size(40.).strong().color(accent()));
                }
                ui.add_space(6.);
                ui.label(Text::new("Or sign in to your sync server").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut f.server)
                            .id_salt("sync_view-field-5")
                            .hint_text("https://sync.example")
                            .desired_width((ui.available_width().max(230.)).min(200.)),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut f.join_user)
                            .id_salt("sync_view-field-6")
                            .hint_text("user")
                            .desired_width(100.),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut f.join_password)
                            .id_salt("sync_view-field-7")
                            .password(true)
                            .hint_text("password")
                            .desired_width(140.),
                    );
                    let ready = !f.server.trim().is_empty()
                        && !f.join_user.trim().is_empty()
                        && !f.join_password.is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Sign in"))
                        .clicked()
                    {
                        let name = if f.name.trim().is_empty() {
                            "New-device".to_owned()
                        } else {
                            f.name.trim().replace(' ', "-")
                        };
                        send.push(format!(
                            "/sync join-account {} {} {name} {}",
                            f.server.trim(),
                            f.join_user.trim(),
                            f.join_password
                        ));
                        f.join_password.clear();
                    }
                });
                let last = crate::sync::live::last_error();
                if !last.is_empty() {
                    ui.colored_label(Color::from_rgb(210, 80, 60), last);
                }
            });
            for c in send {
                self.send(c);
            }
            return;
        };
        // Status and Sync now.
        card(ui, "Sync", |ui| {
            ui.horizontal_wrapped(|ui| {
                match (&sync.status.error, sync.status.last_ok) {
                    (Some(e), last) => {
                        ui.colored_label(
                            Color::from_rgb(210, 80, 60),
                            format!("Couldn't sync: {e}"),
                        );
                        if let Some(t) = last {
                            ui.weak(format!("· last synced {}", when(t)));
                        }
                    }
                    (None, _) if !sync.status.waiting.is_empty() => {
                        ui.weak(format!(
                            "Waiting for {}: not reachable right now (a phone syncs while Spindle is open on it).",
                            sync.status.waiting.join(", ")
                        ));
                        if let Some(t) = sync.status.last_ok {
                            ui.weak(format!("· last synced {}", when(t)));
                        }
                    }
                    (None, Some(t)) => {
                        ui.label(format!("Synced {}", when(t)));
                    }
                    (None, None) => {
                        ui.weak("Not synced yet");
                    }
                }
                if ui.button("Sync now").clicked() {
                    send.push("/sync now".into());
                }
                let label = if sync.paused { "Resume" } else { "Pause" };
                if ui
                    .button(label)
                    .on_hover_text("Pause or resume sync and remote work")
                    .clicked()
                {
                    send.push(if sync.paused {
                        "/sync resume".into()
                    } else {
                        "/sync pause".into()
                    });
                }
                if ui
                    .button("Check connection")
                    .on_hover_text("Try each of your other devices now and show what happened")
                    .clicked()
                {
                    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
                    f.check = Some(slot.clone());
                    let st = state.clone();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let report = crate::sync::live::check(&st);
                        // A copy beside the workspace, to send for help (desktop).
                        #[cfg(not(target_os = "android"))]
                        if let Ok(dir) = std::env::current_dir() {
                            let _ = std::fs::write(dir.join("sync-check.txt"), &report);
                        }
                        *slot.lock().unwrap() = Some(report);
                        ctx.request_repaint();
                    });
                }
                #[cfg(windows)]
                if ui
                    .button("Allow through firewall")
                    .on_hover_text("Let your other devices reach this PC (Windows asks for permission). Only your own devices are let in.")
                    .clicked()
                {
                    send.push("/sync firewall".into());
                }
            });
            if let Some(slot) = f.check.clone() {
                let report = slot.lock().unwrap().clone();
                ui.add_space(4.);
                match report {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak("Checking each device…");
                        });
                    }
                    Some(r) => {
                        egui::ScrollArea::both()
                            .id_salt("sync-check")
                            .max_height(320.)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(egui::RichText::new(&r).monospace().size(11.)).wrap());
                            });
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Copy").clicked() {
                                ui.ctx().copy_text(r.clone());
                            }
                            if crate::widgets::close_button(ui, "Close sync report").clicked() {
                                f.check = None;
                            }
                            #[cfg(not(target_os = "android"))]
                            ui.weak("Also saved as sync-check.txt in the workspace folder.");
                        });
                    }
                }
            }
            ui.horizontal_wrapped(|ui| match &mut f.rename {
                Some(new) => {
                    ui.label("This device's name");
                    let field = ui.add(
                        egui::TextEdit::singleline(new)
                            .id_salt("sync_view-field-8")
                            .desired_width((ui.available_width().max(230.)).min(200.)),
                    );
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ok = !new.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new("Save")).clicked() || (enter && ok) {
                        send.push(format!("/sync rename {}", new.trim()));
                        f.rename = None;
                    } else if ui.button("Cancel").clicked() {
                        f.rename = None;
                    }
                }
                None => {
                    ui.weak(format!(
                        "This device: {} · space {}",
                        sync.identity.device.name,
                        &sync.identity.keys.space[..12]
                    ));
                    if ui.small_button("Rename").clicked() {
                        f.rename = Some(sync.identity.device.name.clone());
                    }
                    if ui.small_button("Stop syncing here").clicked() {
                        f.confirm_leave = true;
                    }
                }
            });
            if f.confirm_leave {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Stop syncing on this device? Its chats and settings stay here; other devices keep theirs.");
                    if ui.button("Stop syncing").clicked() {
                        send.push("/sync leave".into());
                        f.confirm_leave = false;
                    }
                    if ui.button("Keep syncing").clicked() {
                        f.confirm_leave = false;
                    }
                });
            }
        });
        ui.heading("Devices");
        let devices = crate::sync::live::devices(&state);
        if devices.iter().any(|(_, _, _, removed)| *removed) {
            ui.checkbox(&mut f.show_removed, "Show removed devices");
        }
        for (id, name, me, removed) in devices {
            if removed && !f.show_removed { continue; }
            let os = if me { std::env::consts::OS } else { sync.remote.platforms.get(&id).map_or("", String::as_str) };
            let rules = sync.device_rules.get(&id).cloned().unwrap_or_default();
            let change = |option: &str, on| format!("/sync device-rule {}", serde_json::json!([id, option, on]));
            card(ui, &name, |ui| {
                ui.horizontal_wrapped(|ui| {
                    device_icon(ui, os);
                    if me {
                        ui.weak("This device");
                    }
                    if removed {
                        ui.weak("Removed");
                    }
                    if !me && !removed {
                        let mut enabled = !rules.paused;
                        ui.label("Receive sync");
                        if crate::widgets::switch(ui, &mut enabled, "Receive sync").changed() {
                            send.push(change("sync", enabled));
                        }
                        let open = f.manage_device.as_deref() == Some(id.as_str());
                        let toggle = if open {
                            crate::widgets::close_button(ui, "Close device settings")
                        } else {
                            ui.button("Manage")
                        };
                        if toggle.clicked() {
                            f.manage_device = if open { None } else { Some(id.clone()) };
                        }
                    }
                });
                if !removed && !me && f.manage_device.as_deref() == Some(id.as_str()) {
                    ui.separator();
                    ui.weak("These permissions apply to incoming changes and work on this device.");
                    ui.horizontal_wrapped(|ui| {
                        for (key, label) in [("projects", "Project entries"), ("conversations", "Conversations"),
                            ("settings", "Settings"), ("skills", "Skills"), ("memories", "Memories"),
                            ("providers", "Providers")] {
                            let mut enabled = !rules.blocked.contains(key);
                            if ui.checkbox(&mut enabled, label).changed() { send.push(change(key, enabled)); }
                        }
                    });
                    if !sync.projects.is_empty() {
                        ui.label("Project permissions");
                        for (path, project) in &sync.projects {
                            if project.id.is_empty() { continue; }
                            let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                            let mut enabled = !rules.blocked_projects.contains(&project.id);
                            if ui.checkbox(&mut enabled, name).changed() { send.push(change(&format!("project:{}", project.id), enabled)); }
                        }
                    }
                    let mut allowed = sync.remote.allowed.contains(&id);
                    if ui.checkbox(&mut allowed, "Allow sandboxed jobs from this device").changed() {
                        send.push(format!("/remote {} {id}", if allowed { "allow" } else { "deny" }));
                    }
                    if allowed {
                        ui.horizontal_wrapped(|ui| {
                            let host_os = std::env::consts::OS;
                            for kind in crate::sync::placement::Kind::OPERATIONS {
                                if kind.supports(host_os) {
                                    let mut enabled = !rules.blocked_jobs.contains(&kind);
                                    if ui.checkbox(&mut enabled, kind.label()).changed() {
                                        send.push(change(&format!("job:{}", kind.key()), enabled));
                                    }
                                }
                            }
                        });
                    }
                    let mut host_jobs = rules.allow_host_jobs;
                    if ui.checkbox(&mut host_jobs, "Allow host jobs from this device").changed() {
                        send.push(change("host-jobs", host_jobs));
                    }
                    ui.weak("Host jobs can inspect installed tools. Choose which computer-use resources this peer can use.");
                    ui.add_enabled_ui(host_jobs && crate::browser::SUPPORTED, |ui| {
                        let mut browser = rules.allow_browser;
                        if ui.checkbox(&mut browser, "Read public pages with the isolated browser").changed() {
                            send.push(change("host-browser", browser));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if f.confirm_remove.as_deref() == Some(id.as_str()) {
                            if ui
                                .button(
                                    Text::new("Remove from sync")
                                        .color(Color::from_rgb(210, 80, 60)),
                                )
                                .clicked()
                            {
                                send.push(format!("/sync remove {id}"));
                                f.confirm_remove = None;
                            }
                            if ui.small_button("Cancel").clicked() {
                                f.confirm_remove = None;
                            }
                        } else if ui.small_button("Remove device").clicked() {
                            f.confirm_remove = Some(id.clone());
                        }
                    });
                }
            });
        }
        // Adding a device.
        #[cfg(target_os = "android")]
        let alone = crate::sync::live::devices(&state).into_iter()
            .filter(|(_, _, _, removed)| !removed).count() <= 1;
        card(
            ui,
            "Add a device",
            |ui| match crate::sync::live::pair_view() {
                None => {
                    // Joining another device's space (this one syncs only with itself).
                    if let Some(n) = crate::sync::live::join_number() {
                        ui.label("Type this number on the other device:");
                        ui.label(Text::new(n.to_string()).size(40.).strong().color(accent()));
                        return;
                    }
                    ui.weak(
                        "Show a code for your other device to scan, or a short code to type there.",
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Add a device").clicked() {
                            send.push("/sync pair".into());
                        }
                        // A phone on its own: scan the other device's code to join its space.
                        #[cfg(target_os = "android")]
                        if alone && ui.button("Scan QR code").clicked() {
                            crate::scan::begin();
                        }
                    });
                    #[cfg(target_os = "android")]
                    if alone {
                        ui.weak("Or, to join your other device's space: on that device choose Add a device, then Scan QR code here.");
                    }
                }
                Some(v) => {
                    // Click the code to make it big (easier to scan from across a desk).
                    if f.qr_big {
                        let side = ui
                            .available_width()
                            .min(ui.ctx().screen_rect().height() * 0.7)
                            .max(190.);
                        ui.vertical_centered(|ui| {
                            if qr(ui, &v.link, side) {
                                f.qr_big = false;
                            }
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        if !f.qr_big && qr(ui, &v.link, 190.) {
                            f.qr_big = true;
                        }
                        ui.vertical(|ui| {
                            ui.label("Scan with Spindle on the new device, or type there:");
                            ui.label(Text::new(&v.typed).monospace().size(16.));
                            if ui.small_button("Copy link").clicked() {
                                ui.ctx().copy_text(v.link.clone());
                            }
                            ui.add_space(8.);
                            ui.label(&v.status);
                            if v.asking {
                                ui.horizontal_wrapped(|ui| {
                                    let field = ui.add(
                                        egui::TextEdit::singleline(&mut f.pair_number)
                                            .id_salt("sync_view-field-9")
                                            .hint_text("1–100")
                                            .desired_width(60.)
                                            .font(egui::TextStyle::Heading),
                                    );
                                    let typed = f.pair_number.trim().parse::<u32>().ok();
                                    let ok = typed.is_some_and(|n| (1..=100).contains(&n));
                                    let enter = field.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    if (ui.add_enabled(ok, egui::Button::new("Add it")).clicked()
                                        || (enter && ok))
                                        && let Some(n) = typed
                                    {
                                        send.push(format!("/sync pair {n}"));
                                        f.pair_number.clear();
                                    }
                                    if ui.button("Cancel").clicked() {
                                        send.push("/sync pair cancel".into());
                                    }
                                });
                            } else if ui.small_button("Cancel").clicked() {
                                send.push("/sync pair cancel".into());
                            }
                        });
                    });
                }
            },
        );
        // Where projects from other devices go.
        card(ui, "Projects from other devices", |ui| {
            if let Some(dir) = self.projects_folder_field(ui) {
                send.push(format!("/sync projects-folder {dir}"));
            }
        });
        let f = &mut self.sync_form;
        // Where records go.
        card(ui, "Network and servers", |ui| {
            ui.weak(if sync.switches.peers {
                "Your devices on the same network find each other and sync directly: no server or folder needed. A folder or server also reaches devices that aren't on the network."
            } else {
                "Syncing on your network is off. You can enable it in Sync defaults."
            });
            for (i, l) in sync.links.iter().enumerate() {
                if matches!(&l.place, crate::sync::adapter::Place::Peer { device: Some(_), .. }) {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(match &l.place {
                        crate::sync::adapter::Place::Folder { path } => {
                            format!("Folder {}", path.display())
                        }
                        crate::sync::adapter::Place::Hosted { url } => format!("Server {url}"),
                        crate::sync::adapter::Place::Peer { device, addr } => {
                            crate::sync::live::peer_label(&state, device.as_deref(), addr)
                        }
                    });
                    if ui.small_button("Remove").clicked() {
                        send.push(format!("/sync unlink {}", i + 1));
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                let width = ui.available_width().min(300.);
                self.android_ime.field(ui, "sync-peer-address", &mut f.peer, Default::default(), |field| {
                    field
                        .hint_text("DNS name or IP of another device")
                        .desired_width(width)
                });
                if ui
                    .add_enabled(!f.peer.trim().is_empty(), egui::Button::new("Add device"))
                    .clicked()
                {
                    send.push(format!("/sync peer {}", f.peer.trim()));
                    f.peer.clear();
                }
            });
            if let Some(p) = crate::sync::peer::port() {
                let addresses = crate::sync::live::local_peer_endpoints(p, &sync.peer_dns);
                ui.weak(format!("This device listens on port {p}"));
                if !addresses.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(addresses.replace(',', ", ")).weak()).wrap());
                }
            }
            ui.weak("A DNS name is shared alongside this device’s IPs. Leave it blank to use names Spindle discovers.");
            let name = f.peer_dns.get_or_insert_with(|| sync.peer_dns.clone());
            ui.horizontal_wrapped(|ui| {
                let width = ui.available_width().min(260.);
                self.android_ime.field(ui, "sync-peer-dns", name, Default::default(), |field| {
                    field
                        .hint_text("This device’s DNS name")
                        .desired_width(width)
                });
                if ui.add_enabled(name.trim() != sync.peer_dns, egui::Button::new("Save DNS name")).clicked() {
                    *name = name.trim().trim_end_matches('.').to_ascii_lowercase();
                    send.push(format!("/sync peer-dns {name}"));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut f.server)
                        .id_salt("sync_view-field-12")
                        .hint_text("https://your-sync-server:8443")
                        .desired_width((ui.available_width().max(230.)).min(220.)),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut f.admin)
                        .id_salt("sync_view-field-13")
                        .password(true)
                        .hint_text("admin token (first device)")
                        .desired_width(170.),
                );
                if ui
                    .add_enabled(!f.server.trim().is_empty(), egui::Button::new("Add server"))
                    .clicked()
                {
                    send.push(
                        format!("/sync server {} {}", f.server.trim(), f.admin.trim())
                            .trim_end()
                            .to_owned(),
                    );
                    f.admin.clear();
                }
            });
            ui.weak("Run a server with: spindle sync-server --data DIR --admin-token TOKEN");
        });
        // Account for joining without another device.
        card(ui, "Account on your server", |ui| {
            ui.weak("Lets a new device join by signing in when no other device is on. The password never leaves your devices; it unlocks an encrypted copy of the sync key.");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut f.user)
                        .id_salt("sync_view-field-14")
                        .hint_text("user")
                        .desired_width(120.),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut f.password)
                        .id_salt("sync_view-field-15")
                        .password(true)
                        .hint_text("password (12+ characters)")
                        .desired_width((ui.available_width().max(230.)).min(200.)),
                );
                let ok = !f.user.trim().is_empty() && f.password.chars().count() >= 12;
                if ui
                    .add_enabled(ok, egui::Button::new("Save account"))
                    .clicked()
                {
                    send.push(format!("/sync account {} {}", f.user.trim(), f.password));
                    f.password.clear();
                }
            });
        });
        // What syncs and when.
        card(ui, "Sync defaults", |ui| {
            ui.weak("These defaults apply across projects on this device. Each project can override conversation sync.");
            let s = &sync.switches;
            let mut row = |ui: &mut egui::Ui, on: bool, name: &str, label: &str, hover: &str| {
                let mut v = on;
                if ui.checkbox(&mut v, label).on_hover_text(hover).changed() {
                    send.push(format!(
                        "/sync switch {name} {}",
                        if v { "on" } else { "off" }
                    ));
                }
            };
            row(
                ui,
                s.conversations,
                "conversations",
                "Conversations",
                "The default for every project; a project can override it.",
            );
            row(ui, s.skills, "skills", "Installed skills", "Share installed skills with devices that enable this option.");
            row(
                ui, s.local_providers, "local-providers", "Providers without credentials",
                "Opt in on the sending and receiving devices. Loopback addresses use the sender's hostname.",
            );
            ui.separator();
            row(
                ui,
                s.peers,
                "peers",
                "Sync with devices on this network",
                "Find your other devices on the local network and sync with them directly, with no server or folder.",
            );
            row(
                ui,
                s.background,
                "background",
                "Sync in the background",
                "Every couple of minutes and when something changes.",
            );
            row(
                ui,
                s.on_open,
                "on-open",
                "Sync when a chat opens",
                "Fetch its latest turns first, so you see the other device's before adding yours.",
            );
            row(
                ui,
                s.before_send,
                "before-send",
                "Sync before sending",
                "A quick check before your message goes, so it doesn't start a branch.",
            );
            if s.local_providers {
                ui.separator();
                ui.label("Hostname for providers on this device");
                let name = f.provider_host.get_or_insert_with(|| sync.provider_host.clone());
                let width = ui.available_width().min(320.);
                self.android_ime.field(ui, "sync-provider-host", name, Default::default(), |field| {
                    field.hint_text(crate::sync::live::hostname()).desired_width(width)
                });
                if ui.add_enabled(name.trim() != sync.provider_host, egui::Button::new("Save hostname")).clicked() {
                    send.push(format!("/sync provider-host {}", name.trim()));
                }
            }
        });
        // Per-project choices.
        let projects: Vec<std::path::PathBuf> = state
            .chats
            .values()
            .filter_map(|c| c.thread.project.clone())
            .chain(state.thread.project.clone())
            .chain(sync.projects.keys().cloned())
            .chain(self.projects.recent.iter().map(|p| p.path.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !projects.is_empty() || !sync.remote_projects.is_empty() {
            ui.heading("Projects");
            ui.weak("Project choices override the sync defaults.");
                for p in &projects {
                    let ps = sync.projects.get(p).cloned().unwrap_or_default();
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    card(ui, &name, |ui| {
                        ui.weak(p.display().to_string());
                        ui.horizontal_wrapped(|ui| {
                        if ui.button(if ps.paused { "Resume sync" } else { "Stop sync" }).clicked() {
                            send.push(format!("/sync project-pause {}", serde_json::json!([p, !ps.paused])));
                        }
                        if ps.paused {
                            ui.weak("Paused on all devices");
                        }
                        let conversations = match ps.conversations {
                            None => "Conversations: use default",
                            Some(true) => "Conversations: sync",
                            Some(false) => "Conversations: keep here",
                        };
                        crate::widgets::InlineChoice::from_id_salt(("conversations", p))
                                .selected_text(conversations)
                                .show_ui(ui, |ui| {
                                    for (v, l) in [
                                        ("global", "Use default"),
                                        ("on", "Sync"),
                                        ("off", "Keep on this device"),
                                    ] {
                                        if ui.selectable_label(false, l).clicked() {
                                            send.push(format!(
                                                "/sync project {} conversations={v}",
                                                p.display()
                                            ));
                                        }
                                    }
                                });
                        let mut hidden = ps.hidden;
                        if ui
                            .checkbox(&mut hidden, "Don't show on other devices")
                            .changed()
                        {
                            send.push(format!(
                                "/sync project {} hidden={}",
                                p.display(),
                                if hidden { "on" } else { "off" }
                            ));
                        }
                        });
                    });
                }
                let choose = self.view.state.preferences.projects_dir.trim().is_empty();
                for v in sync.remote_projects.values() {
                    card(ui, v["name"].as_str().unwrap_or("Project"), |ui| {
                        if v["paused"] == true {
                            ui.weak("Sync paused");
                        } else if choose {
                            ui.weak("waiting: choose where projects go");
                            if ui.small_button("Choose folder…").clicked() {
                                pick_projects_folder = true;
                            }
                        } else {
                            ui.weak("on another device (its conversations don't sync)");
                        }
                    });
                }
        }
        if !sync.activity.is_empty() {
            card(ui, "Activity", |ui| {
                for a in sync.activity.iter().rev().take(12) {
                    ui.weak(a);
                }
            });
        }
        if pick_projects_folder && let Some(dir) = self.pick_projects_folder() {
            send.push(format!("/sync projects-folder {dir}"));
        }
        for c in send {
            self.send(c);
        }
    }

    /// The sync control in the chat header: status at a glance, click to sync now.
    /// Projects waiting on this device for a folder to go in: their names.
    fn waiting_projects(&self) -> Vec<String> {
        self.view
            .state
            .space
            .as_ref()
            .map(|s| {
                s.remote_projects
                    .values()
                    .filter_map(|v| v["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The projects-folder field with its button; the folder chosen, when the button is used.
    fn projects_folder_field(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let saved = self.view.state.preferences.projects_dir.clone();
        let waiting = self.waiting_projects();
        ui.weak(if saved.is_empty() {
            "Choose where projects from your other devices are kept here. Each gets a folder with its name, notes and chats. Project files stay on the original device."
        } else {
            "Each project from another device has a folder here, with its name, notes and chats. Project files stay on the original device."
        });
        if !waiting.is_empty() {
            ui.label(format!("Waiting for a folder: {}", waiting.join(", ")));
        }
        let mut chosen = None;
        let suggestion = if saved.is_empty() {
            crate::sync::live::suggested_projects_folder()
        } else {
            saved.clone()
        };
        // Pick it the usual way: Windows' folder picker, or Spindle's own folder browser
        // (phones and Linux).
        ui.horizontal_wrapped(|ui| {
            if ui.button("Choose folder…").clicked()
                && let Some(dir) = self.pick_projects_folder()
            {
                chosen = Some(dir);
            }
            ui.weak("or type a folder:");
        });
        ui.horizontal_wrapped(|ui| {
            let field = self
                .sync_form
                .projects_dir
                .get_or_insert_with(|| suggestion.clone());
            ui.add(
                egui::TextEdit::singleline(field)
                    .id_salt("sync_view-field-16")
                    .hint_text("Full path of a folder")
                    .desired_width(ui.available_width().max(230.).min(380.)),
            );
            let ready = !field.trim().is_empty() && field.trim() != saved;
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(if saved.is_empty() {
                        "Use this folder"
                    } else {
                        "Move here"
                    }),
                )
                .clicked()
            {
                chosen = Some(field.trim().to_owned());
            }
        });
        if !saved.is_empty() {
            ui.weak(format!(
                "Now: {saved}. Projects already made stay where they are."
            ));
        }
        if chosen.is_some() {
            self.sync_form.projects_dir = None;
        }
        chosen
    }

    /// Choose the projects folder: Windows' picker (the folder, at once), or open Spindle's
    /// folder browser (phones and Linux), which sends its choice when made.
    fn pick_projects_folder(&mut self) -> Option<String> {
        let saved = self.view.state.preferences.projects_dir.clone();
        let start = std::path::PathBuf::from(if saved.trim().is_empty() {
            crate::sync::live::suggested_projects_folder()
        } else {
            saved
        });
        let start = start
            .ancestors()
            .find(|p| p.is_dir())
            .map(|p| p.to_owned())
            .unwrap_or_else(|| folder_shortcuts()[0].1.clone());
        #[cfg(windows)]
        {
            rfd::FileDialog::new()
                .set_title("Where should projects from your other devices go?")
                .set_directory(start)
                .pick_folder()
                .map(|d| d.display().to_string())
        }
        #[cfg(not(windows))]
        {
            self.sync_form.browse = Some(start);
            None
        }
    }

    /// The folder browser, drawn once a frame from the top level (drawing it from both the
    /// Sync page and the question doubled every key typed in it).
    pub(super) fn folder_browser_panel(&mut self, ui: &mut egui::Ui) {
        if let Some(dir) = self.folder_browser(ui) {
            self.sync_form.projects_dir = None;
            self.send(format!("/sync projects-folder {dir}"));
        }
    }

    /// A simple folder browser drawn by Spindle (phones have no folder picker Spindle can
    /// open): shortcuts, the folders here, Up, New folder and Use this folder.
    fn folder_browser(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let here = self.sync_form.browse.clone()?;
        let mut open = true;
        let mut chosen = None;
        let mut go = None;
        super::detail_panel(ui, "Choose a folder", "Back", &mut open, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, path) in folder_shortcuts() {
                    if ui.small_button(label).clicked() {
                        go = Some(path);
                    }
                }
            });
            ui.add_space(4.);
            ui.label(Text::new(here.display().to_string()).strong());
            let mut folders: Vec<std::path::PathBuf> = std::fs::read_dir(&here)
                .map(|r| {
                    r.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default();
            folders.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() * 0.55)
                .show(ui, |ui| {
                    if let Some(up) = here.parent()
                        && ui.button("⬑  Up").clicked()
                    {
                        go = Some(up.to_owned());
                    }
                    if folders.is_empty() {
                        ui.weak("No folders here.");
                    }
                    for f in &folders {
                        let name = f.file_name().unwrap_or_default().to_string_lossy();
                        if ui.button(format!("📁  {name}")).clicked() {
                            go = Some(f.clone());
                        }
                    }
                });
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.sync_form.new_folder)
                        .id_salt("sync_view-field-17")
                        .hint_text("New folder name")
                        .desired_width(ui.available_width().max(160.).min(220.)),
                );
                let name = self.sync_form.new_folder.trim().to_owned();
                if ui
                    .add_enabled(
                        !name.is_empty() && !name.contains(['/', '\\']),
                        egui::Button::new("New folder"),
                    )
                    .clicked()
                {
                    let dir = here.join(&name);
                    if std::fs::create_dir_all(&dir).is_ok() {
                        go = Some(dir);
                        self.sync_form.new_folder.clear();
                    }
                }
            });
            ui.add_space(6.);
            if ui.button(Text::new("Use this folder").strong()).clicked() {
                chosen = Some(here.display().to_string());
            }
        });
        if let Some(p) = go {
            self.sync_form.browse = Some(p);
        }
        if !open || chosen.is_some() {
            self.sync_form.browse = None;
        }
        chosen
    }

    /// Asked once projects from another device arrive and no folder is chosen yet.
    pub(super) fn needs_projects_folder(&self) -> bool {
        // The Sync page already shows this field.
        !(self.page == super::Page::Settings && self.sync_tab)
            && self.sync_form.browse.is_none()
            && !self.sync_form.projects_later
            && self.view.state.preferences.projects_dir.trim().is_empty()
            && !self.waiting_projects().is_empty()
    }

    pub(super) fn projects_folder_panel(&mut self, ui: &mut egui::Ui) {
        let mut open = true;
        let mut chosen = None;
        super::detail_panel(ui, "Where should projects go?", "Close", &mut open, |ui| {
            chosen = self.projects_folder_field(ui);
            if ui.button("Later").clicked() {
                self.sync_form.projects_later = true;
            }
        });
        if !open {
            self.sync_form.projects_later = true;
        }
        if let Some(dir) = chosen {
            self.send(format!("/sync projects-folder {dir}"));
        }
    }

    /// Where sync stands, for the sidebar footer: a short label, its colour, the
    /// longer explanation, and whether a round is running.
    fn sync_summary(&self) -> Option<(String, Color, String, bool)> {
        let sync = self.view.state.space.as_ref()?;
        let others = crate::sync::live::devices(&self.view.state)
            .into_iter()
            .filter(|(_, _, me, removed)| !me && !removed)
            .map(|(_, name, ..)| name)
            .collect::<Vec<_>>();
        let with = if others.is_empty() {
            String::new()
        } else {
            format!(" with {}", others.join(", "))
        };
        let last = sync
            .status
            .last_ok
            .map(|t| format!("last synced {}", when(t)))
            .unwrap_or_else(|| "not synced yet".into());
        Some(if sync.status.syncing {
            ("Syncing".into(), muted(), format!("Syncing{with}"), true)
        } else if sync.paused {
            (
                "Paused".into(),
                muted(),
                "Resume in Settings › Sync".into(),
                false,
            )
        } else if let Some(e) = &sync.status.error {
            (
                "Retry".into(),
                Color::from_rgb(210, 80, 60),
                format!("{e} · {last}. Tap to try again."),
                false,
            )
        } else if !sync.status.waiting.is_empty() {
            let who = sync.status.waiting.join(", ");
            (
                "Waiting".into(),
                muted(),
                format!(
                    "{who} isn't reachable right now (a phone syncs while Spindle is open on it); it catches up when it's back · {last}. Tap to try now."
                ),
                false,
            )
        } else if others.is_empty() {
            (
                "Local".into(),
                muted(),
                "No other devices yet: Settings › Sync › Add a device".into(),
                false,
            )
        } else {
            (
                "Synced".into(),
                Color::from_rgb(90, 170, 110),
                format!("Synced{with} · {last}. Tap to sync now."),
                false,
            )
        })
    }

    /// The footer's sync mark: "⟳ Synced" (a computer) or just the coloured ⟳ (a narrow sidebar), a
    /// spinner while syncing. Tap to sync now.
    pub(super) fn sync_indicator(&mut self, ui: &mut egui::Ui, compact: bool) {
        let Some((label, color, hover, busy)) = self.sync_summary() else {
            return;
        };
        if busy {
            ui.add(egui::Spinner::new().size(14.)).on_hover_text(hover);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let text = if compact {
            "⟳".to_owned()
        } else {
            format!("⟳ {label}")
        };
        if ui
            .add(
                egui::Button::new(
                    Text::new(text)
                        .size(if compact { 18. } else { 12. })
                        .color(color),
                )
                .frame(false),
            )
            .on_hover_text(format!("{label}: {hover}"))
            .clicked()
        {
            self.send("/sync now");
        }
    }



    /// Branches from other devices: a bar above the chat with View, Make main and Delete.
    pub(super) fn branch_bar(&mut self, ui: &mut egui::Ui) {
        let state = &self.view.state;
        let Some(list) = state
            .space
            .as_ref()
            .and_then(|s| s.branches.get(&state.chat_id))
            .cloned()
        else {
            return;
        };
        let chat = state.chat_id;
        let mut send = None;
        for b in &list {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1_f32, line()))
                .corner_radius(8)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(Text::new(format!("Branch from {}", b.device_name)).strong());
                        ui.weak(format!(
                            "after message {} · {} message{}",
                            b.fork,
                            b.messages.len().saturating_sub(b.fork),
                            if b.messages.len().saturating_sub(b.fork) == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ));
                        if ui.small_button("View").clicked() {
                            self.sync_form.reading = Some((chat, b.id.clone()));
                        }
                        if ui
                            .small_button("Make main")
                            .on_hover_text(
                                "Show this branch here; the current one becomes a branch",
                            )
                            .clicked()
                        {
                            send = Some(format!("/branch main {chat} {}", b.id));
                        }
                        if ui
                            .small_button("Delete branch")
                            .on_hover_text("Remove it on every device")
                            .clicked()
                        {
                            send = Some(format!("/branch delete {chat} {}", b.id));
                        }
                    });
                });
            ui.add_space(4.);
        }
        if let Some((c, id)) = self.sync_form.reading.clone() {
            let mut open = true;
            if let Some(b) = list.iter().find(|b| b.id == id && c == chat) {
                super::detail_panel(
                    ui,
                    &format!("Branch from {}", b.device_name),
                    "Back",
                    &mut open,
                    |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(480.)
                            .show(ui, |ui| {
                                for (i, m) in
                                    b.messages.iter().enumerate().skip(b.fork.saturating_sub(1))
                                {
                                    ui.label(
                                        Text::new(if m.role == "user" { "You" } else { "Reply" })
                                            .strong()
                                            .color(if i < b.fork { muted() } else { accent() }),
                                    );
                                    reader::markdown(ui, &m.content, m.role != "user");
                                    ui.add_space(6.);
                                }
                            });
                    },
                );
            } else {
                open = false;
            }
            if !open {
                self.sync_form.reading = None;
            }
        }
        if let Some(c) = send {
            self.send(c);
        }
    }
}

/// Places to start looking for a folder.
fn folder_shortcuts() -> Vec<(&'static str, std::path::PathBuf)> {
    #[cfg(target_os = "android")]
    {
        let root = std::path::PathBuf::from("/storage/emulated/0");
        vec![
            ("Documents", root.join("Documents")),
            ("Download", root.join("Download")),
            ("Phone storage", root),
        ]
    }
    #[cfg(not(target_os = "android"))]
    {
        let home = std::path::PathBuf::from(
            std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_default(),
        );
        vec![("Documents", home.join("Documents")), ("Home", home)]
    }
}
