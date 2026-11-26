use super::*;

#[derive(Default)]
pub(super) struct HtmlPreview {
    source: String,
    title: String,
    render: bool,
    html: bool,
    markdown: bool,
    error: String,
    disk: Option<PathBuf>,
    server: Option<crate::html_preview::Server>,
    #[cfg(windows)]
    web: Option<wry::WebView>,
    #[cfg(target_os = "linux")]
    window: Option<crate::html_preview::AppWindow>,
    #[cfg(target_os = "linux")]
    opened: bool,
    #[cfg(any(windows, target_os = "android"))]
    visible: bool,
    #[cfg(windows)]
    bounds: Option<wry::Rect>,
    #[cfg(target_os = "android")]
    bounds: Option<[i32; 4]>,
}

impl HtmlPreview {
    pub fn load(&mut self, title: String, source: String) {
        self.load_with_path(title, source, None);
    }
    pub fn load_with_path(&mut self, title: String, source: String, disk: Option<PathBuf>) {
        self.hide();
        let html = crate::html_preview::is_html(Path::new(&title));
        let markdown = title.rsplit_once('.').is_some_and(|(_, ext)| {
            ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
        });
        *self = Self {
            render: html || markdown,
            html,
            markdown,
            title,
            source,
            disk,
            ..Default::default()
        };
    }
    /// Show the file's text rather than its rendered view.
    pub fn show_source(&mut self) {
        self.render = false;
    }
    pub fn set_disk(&mut self, disk: Option<PathBuf>) {
        self.disk = disk;
    }
    pub fn active(&self) -> bool {
        !self.title.is_empty()
    }
    pub fn hide(&mut self) {
        #[cfg(windows)]
        if self.visible
            && let Some(web) = &self.web
        {
            self.visible = web.set_visible(false).is_err();
        }
        #[cfg(target_os = "android")]
        if self.visible {
            crate::android::hide_html_preview();
            self.visible = false;
        }
        #[cfg(target_os = "linux")]
        {
            if self.window.take().is_some() {
                self.opened = false;
            }
        }
    }
    pub fn focus_chat(&self) {
        #[cfg(windows)]
        if let Some(web) = &self.web {
            let _ = web.focus_parent();
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame, configured: &str) {
        ui.label(Text::new(&self.title).strong());
        ui.horizontal_wrapped(|ui| {
            if self.html || self.markdown {
                ui.selectable_value(&mut self.render, true, "Preview");
                ui.selectable_value(&mut self.render, false, "Source");
            }
            if !self.render && ui.small_button("Copy source").clicked() {
                ui.ctx().copy_text(self.source.clone());
            }
            if self.render && self.html && ui.small_button("Restart").clicked() {
                self.hide();
                #[cfg(windows)]
                {
                    self.web = None;
                }
                #[cfg(target_os = "linux")]
                {
                    self.opened = false;
                }
                self.server = None;
                self.error.clear();
            }
        });
        ui.separator();
        if self.render && self.markdown {
            self.hide();
            egui::ScrollArea::vertical()
                .id_salt("artifact-markdown")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    super::reader::markdown(ui, &self.source, false);
                });
            return;
        }
        if !self.render {
            self.hide();
            egui::ScrollArea::both()
                .id_salt("artifact-source")
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(Text::new(&self.source).monospace())
                            .selectable(true)
                            .extend(),
                    );
                });
            return;
        }
        if self.server.is_none() && self.error.is_empty() {
            match crate::html_preview::Server::start(
                &self.title,
                self.source.clone(),
                self.disk.as_deref(),
            ) {
                Ok(server) => self.server = Some(server),
                Err(error) => self.error = format!("Could not serve the HTML preview: {error:#}"),
            }
        }
        #[cfg(windows)]
        {
            let _ = configured;
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
            if let Some(web) = &self.web {
                if self.bounds != Some(bounds) && web.set_bounds(bounds).is_ok() {
                    self.bounds = Some(bounds);
                }
                // On Windows, showing the native child can activate it. Leave keyboard
                // focus alone while the user types beside an already visible preview.
                if !self.visible {
                    self.visible = web.set_visible(true).is_ok();
                }
            } else if self.error.is_empty()
                && let Some(server) = &self.server
            {
                let scope = server.scope.clone();
                let web = wry::WebViewBuilder::new()
                    .with_incognito(true)
                    .with_focused(false)
                    .with_clipboard(false)
                    .with_devtools(false)
                    .with_navigation_handler(move |url| {
                        url.starts_with(&scope) || url == "about:blank"
                    })
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_download_started_handler(|_, _| false)
                    .with_bounds(bounds)
                    .with_url(&server.url)
                    .build_as_child(frame);
                match web {
                    Ok(w) => {
                        self.web = Some(w);
                        self.bounds = Some(bounds);
                        self.visible = true;
                    }
                    Err(e) => {
                        self.error = format!("WebView2 could not start the HTML preview: {e}")
                    }
                }
            }
        }
        #[cfg(target_os = "android")]
        {
            let _ = (frame, configured);
            let rect = ui.available_rect_before_wrap();
            let scale = ui.ctx().pixels_per_point();
            let bounds = [
                (rect.min.x * scale).round() as i32,
                (rect.min.y * scale).round() as i32,
                (rect.width() * scale).round().max(1.) as i32,
                (rect.height() * scale).round().max(1.) as i32,
            ];
            if (!self.visible || self.bounds != Some(bounds))
                && let Some(server) = &self.server
            {
                if crate::android::show_html_preview(&server.url, &server.scope, bounds).is_ok() {
                    self.visible = true;
                    self.bounds = Some(bounds);
                } else {
                    self.error = "Android System WebView could not open the preview".into();
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            let _ = frame;
            if self
                .window
                .as_mut()
                .is_some_and(|w| w.child.try_wait().ok().flatten().is_some())
            {
                self.window = None;
            }
            if self.window.is_some() {
                ui.label("Preview is open in its own Chromium window.");
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(350));
            } else if ui.button("Open browser preview").clicked() || !self.opened {
                self.opened = true;
                if let Some(server) = &self.server {
                    match crate::html_preview::AppWindow::open(configured, server) {
                        Ok(window) => {
                            self.window = Some(window);
                            self.error.clear();
                        }
                        Err(error) => self.error = format!("Could not open preview: {error:#}"),
                    }
                }
            }
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
        {
            let _ = (frame, configured);
            ui.label("Interactive previews are available on Windows, Linux, and Android.");
        }
        if !self.error.is_empty() {
            ui.label(&self.error);
        }
    }
}
