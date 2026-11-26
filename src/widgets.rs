use eframe::egui::{
    self, Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2,
};

/// A choice list that expands in the current panel.
pub struct InlineChoice {
    id: egui::Id,
    label: Option<egui::WidgetText>,
    selected: egui::WidgetText,
    width: Option<f32>,
}

impl InlineChoice {
    pub fn from_id_salt(salt: impl std::hash::Hash) -> Self {
        Self {
            id: egui::Id::new(salt),
            label: None,
            selected: "".into(),
            width: None,
        }
    }

    pub fn from_label(label: &str) -> Self {
        let mut choice = Self::from_id_salt(label);
        choice.label = Some(label.to_owned().into());
        choice
    }

    pub fn selected_text(mut self, text: impl Into<egui::WidgetText>) -> Self {
        self.selected = text.into();
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn show_ui<R>(
        self,
        ui: &mut Ui,
        contents: impl FnOnce(&mut Ui) -> R,
    ) -> egui::InnerResponse<Option<R>> {
        let id = ui.make_persistent_id(self.id);
        let width = self.width.unwrap_or(ui.spacing().combo_width)
            .min(ui.available_width()).max(24.);
        ui.vertical(|ui| {
            if let Some(label) = self.label {
                ui.label(label);
            }
            let mut open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
            let icon = if open { "-" } else { "+" };
            let response = ui.add_sized([width, 25.], egui::Button::new(
                format!("{icon}  {}", self.selected.text()),
            ).truncate());
            if response.clicked() {
                open = !open;
            }
            let inner = if open {
                ui.group(|ui| {
                    ui.set_max_width(width);
                    if ui.small_button("Close choices").clicked() {
                        open = false;
                        return None;
                    }
                    let choices = egui::ScrollArea::vertical()
                        .id_salt(id)
                        .max_height(220.)
                        .show(ui, contents);
                    if ui.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary))
                        && ui.rect_contains_pointer(choices.inner_rect)
                    {
                        open = false;
                    }
                    Some(choices.inner)
                }).inner
            } else {
                None
            };
            ui.data_mut(|data| data.insert_temp(id, open));
            egui::InnerResponse { inner, response }
        }).inner
    }
}

/// The accent palette offered in Settings → Preferences.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Accent {
    Red,
    Orange,
    Yellow,
    Green,
    #[default]
    Blue,
    Indigo,
    Violet,
    Magenta,
}
impl Accent {
    pub const ALL: [Accent; 8] = [
        Accent::Red,
        Accent::Orange,
        Accent::Yellow,
        Accent::Green,
        Accent::Blue,
        Accent::Indigo,
        Accent::Violet,
        Accent::Magenta,
    ];
    pub fn color(self) -> Color32 {
        match self {
            Accent::Red => Color32::from_rgb(240, 110, 110),
            Accent::Orange => Color32::from_rgb(245, 160, 95),
            Accent::Yellow => Color32::from_rgb(232, 200, 90),
            Accent::Green => Color32::from_rgb(105, 200, 135),
            Accent::Blue => Color32::from_rgb(100, 160, 245),
            Accent::Indigo => Color32::from_rgb(125, 130, 240),
            Accent::Violet => Color32::from_rgb(175, 130, 240),
            Accent::Magenta => Color32::from_rgb(230, 110, 200),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Accent::Red => "Red",
            Accent::Orange => "Orange",
            Accent::Yellow => "Yellow",
            Accent::Green => "Green",
            Accent::Blue => "Blue",
            Accent::Indigo => "Indigo",
            Accent::Violet => "Violet",
            Accent::Magenta => "Magenta",
        }
    }
}

static ACCENT_RGB: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x64A0F5);

/// The current accent color, chosen in Settings → Preferences.
pub fn accent() -> Color32 {
    let v = ACCENT_RGB.load(std::sync::atomic::Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn set_accent(a: Accent) {
    let [r, g, b, _] = a.color().to_array();
    ACCENT_RGB.store(
        u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Appearance chosen in Settings → Preferences.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Theme {
    /// Follow the operating system's light or dark setting.
    #[default]
    System,
    Dark,
    Light,
}

static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Switch the palette functions below between their dark and light values.
pub fn set_light(light: bool) {
    LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}

fn pick(dark: [u8; 3], light: [u8; 3]) -> Color32 {
    let [r, g, b] = if is_light() { light } else { dark };
    Color32::from_rgb(r, g, b)
}

/// Body text.
pub fn ink() -> Color32 {
    pick([240, 237, 230], [30, 30, 32])
}
/// Secondary text and icons.
pub fn muted() -> Color32 {
    pick([170, 167, 163], [104, 101, 98])
}
/// Sidebar, side panel and dialog background.
pub fn panel() -> Color32 {
    pick([28, 28, 30], [243, 242, 240])
}
/// Main conversation background.
pub fn surface() -> Color32 {
    pick([22, 22, 24], [252, 252, 251])
}
/// Fields, cards and buttons.
pub fn raised() -> Color32 {
    pick([39, 38, 40], [233, 232, 229])
}
/// Dividers and outlines.
pub fn line() -> Color32 {
    pick([62, 59, 60], [214, 211, 207])
}
/// Headings and bold text.
pub fn strong() -> Color32 {
    pick([255, 255, 255], [8, 8, 10])
}
/// Selected rows, tabs and text: a quiet charcoal, or a soft gray in light mode.
pub fn highlight() -> Color32 {
    pick([54, 53, 56], [222, 221, 218])
}
/// Hovered controls.
pub fn hover() -> Color32 {
    pick([47, 46, 49], [228, 227, 224])
}

#[derive(Clone, Copy)]
pub enum Icon {
    Copy,
    Stop,
    Steer,
    Rename,
    More,
    Archive,
    Trash,
    Sidebar,
    Reader,
    New,
    Chat,
    Queue,
    Skill,
    Settings,
    Folder,
    Plan,
    Swarm,
    Attach,
    Chip,
    Arrow,
    Star,
    Plug,
    Check,
    Search,
    Clock,
    Chevron,
    Eye,
    Wave,
    Brain,
    Cycle,
    Memory,
    Branch,
    Tool,
    Shield,
    Mic,
    Speaker,
}

pub fn draw(ui: &Ui, icon: Icon, rect: Rect, color: Color32) {
    draw_with(ui.painter(), icon, rect, color);
}

pub fn draw_with(painter: &egui::Painter, icon: Icon, rect: Rect, color: Color32) {
    let pt = |x: f32, y: f32| {
        pos2(
            rect.left() + x * rect.width() / 24.,
            rect.top() + y * rect.height() / 24.,
        )
    };
    let stroke = Stroke::new(1.65_f32, color);
    let path = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| pt(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x, y, r| {
        painter.circle_stroke(pt(x, y), r * rect.width() / 24., stroke);
    };
    let box_at = |x, y, a, b| {
        painter.rect_stroke(
            Rect::from_min_max(pt(x, y), pt(a, b)),
            3,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Mic => {
            painter.rect_stroke(
                Rect::from_min_max(pt(9., 3.), pt(15., 15.)),
                3. * rect.width() / 24.,
                stroke,
                egui::StrokeKind::Inside,
            );
            let arc: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let a = (180. * i as f32 / 12.).to_radians();
                    (12. - 6. * a.cos(), 11. + 6. * a.sin())
                })
                .collect();
            path(&arc);
            path(&[(12., 17.), (12., 21.)]);
            path(&[(8.5, 21.), (15.5, 21.)]);
        }
        Icon::Speaker => {
            path(&[
                (3., 9.),
                (7., 9.),
                (12., 5.),
                (12., 19.),
                (7., 15.),
                (3., 15.),
                (3., 9.),
            ]);
            for (r, n) in [(3.5_f32, 6), (7.0, 10)] {
                let arc: Vec<(f32, f32)> = (0..=n)
                    .map(|i| {
                        let a = (-50. + 100. * i as f32 / n as f32).to_radians();
                        (12. + r * a.cos(), 12. + r * a.sin())
                    })
                    .collect();
                path(&arc);
            }
        }
        Icon::Cycle => {
            // Three quarters of a circle with an arrowhead: a renewing plan.
            let points: Vec<(f32, f32)> = (0..=18)
                .map(|i| {
                    let a = (-60. + 270. * i as f32 / 18.).to_radians();
                    (12. + 7. * a.cos(), 12. + 7. * a.sin())
                })
                .collect();
            path(&points);
            let (x, y) = points[0];
            path(&[(x - 4., y - 1.), (x, y), (x + 1., y - 4.)]);
        }
        Icon::Eye => {
            path(&[
                (2., 12.),
                (5., 8.),
                (9., 6.),
                (12., 5.5),
                (15., 6.),
                (19., 8.),
                (22., 12.),
                (19., 16.),
                (15., 18.),
                (12., 18.5),
                (9., 18.),
                (5., 16.),
                (2., 12.),
            ]);
            circle(12., 12., 3.2);
            painter.circle_filled(pt(12., 12.), 1.2 * rect.width() / 24., color);
        }
        Icon::Wave => {
            for (x, half) in [(4., 2.), (8., 5.), (12., 8.), (16., 4.), (20., 1.5)] {
                path(&[(x, 12. - half), (x, 12. + half)]);
            }
        }
        Icon::Brain => {
            let left = [
                (12., 4.5),
                (9.5, 3.5),
                (7., 4.5),
                (5.8, 6.8),
                (4., 8.5),
                (3.5, 11.5),
                (4.5, 14.),
                (4., 16.5),
                (5.8, 19.),
                (8.8, 20.5),
                (12., 19.5),
            ];
            path(&left);
            path(&left.map(|(x, y)| (24. - x, y)));
            path(&[(12., 4.5), (12., 19.5)]);
            for (a, b) in [((7.5, 9.), (10., 10.5)), ((6.8, 14.5), (9.5, 13.5))] {
                path(&[a, b]);
                path(&[(24. - a.0, a.1), (24. - b.0, b.1)]);
            }
        }
        Icon::Copy => {
            path(&[(7., 16.), (4., 16.), (4., 4.), (15., 4.), (15., 7.)]);
            box_at(8., 8., 21., 21.);
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_min_max(pt(5., 5.), pt(19., 19.)), 2, color);
        }
        Icon::Steer => {
            path(&[
                (5., 20.),
                (5., 16.),
                (6., 13.),
                (9., 10.),
                (12., 9.),
                (20., 9.),
            ]);
            path(&[(15., 4.), (20., 9.), (15., 14.)]);
        }
        Icon::Rename => {
            path(&[
                (4., 20.),
                (5., 15.),
                (16., 4.),
                (20., 8.),
                (9., 19.),
                (4., 20.),
            ]);
            path(&[(14., 6.), (18., 10.)]);
        }
        Icon::More => {
            for x in [5., 12., 19.] {
                painter.circle_filled(pt(x, 12.), 1.5 * rect.width() / 24., color);
            }
        }
        Icon::Archive => {
            box_at(3., 4., 21., 9.);
            box_at(5., 9., 19., 20.);
            path(&[(10., 13.), (14., 13.)]);
        }
        Icon::Trash => {
            path(&[(3., 6.), (21., 6.)]);
            box_at(6., 6., 18., 21.);
            path(&[(9., 6.), (9., 3.), (15., 3.), (15., 6.)]);
        }
        Icon::Sidebar | Icon::Reader => {
            box_at(2., 4., 22., 20.);
            let x = if matches!(icon, Icon::Sidebar) {
                9.
            } else {
                15.
            };
            path(&[(x, 5.), (x, 19.)]);
        }
        Icon::New => {
            path(&[(5., 12.), (19., 12.)]);
            path(&[(12., 5.), (12., 19.)]);
        }
        Icon::Chat => path(&[
            (4., 4.),
            (20., 4.),
            (20., 16.),
            (11., 16.),
            (5., 20.),
            (5., 16.),
            (4., 16.),
            (4., 4.),
        ]),
        Icon::Queue => {
            for y in [6., 12., 18.] {
                circle(4., y, 0.8);
                path(&[(8., y), (20., y)]);
            }
        }
        Icon::Skill => {
            path(&[(12., 3.), (21., 8.), (12., 13.), (3., 8.), (12., 3.)]);
            for y in [12., 16.] {
                path(&[(3., y), (12., y + 5.), (21., y)]);
            }
        }
        Icon::Settings => {
            for (y, x) in [(5., 8.), (12., 16.), (19., 10.)] {
                path(&[(3., y), (x - 2., y)]);
                circle(x, y, 2.2);
                path(&[(x + 2., y), (21., y)]);
            }
        }
        Icon::Folder => path(&[
            (3., 6.),
            (9., 6.),
            (11., 9.),
            (21., 9.),
            (21., 18.),
            (3., 18.),
            (3., 6.),
        ]),
        Icon::Plan => {
            box_at(3., 3., 8., 8.);
            box_at(3., 10., 8., 15.);
            path(&[(11., 5.), (21., 5.)]);
            path(&[(11., 12.), (21., 12.)]);
            path(&[(4., 19.), (7., 21.), (11., 17.)]);
        }
        Icon::Swarm => {
            circle(6., 12., 3.);
            circle(18., 5., 3.);
            circle(18., 19., 3.);
            path(&[(9., 11.), (15., 6.)]);
            path(&[(9., 13.), (15., 18.)]);
        }
        Icon::Attach => path(&[
            (8., 14.),
            (15., 7.),
            (17., 9.),
            (9., 17.),
            (6., 17.),
            (4., 15.),
            (4., 12.),
            (14., 2.),
            (18., 2.),
            (21., 5.),
            (21., 9.),
            (10., 20.),
            (6., 21.),
            (3., 19.),
        ]),
        Icon::Chip => {
            box_at(5., 5., 19., 19.);
            box_at(9., 9., 15., 15.);
            for i in [8., 15.] {
                path(&[(i, 2.), (i, 5.)]);
                path(&[(i, 19.), (i, 22.)]);
                path(&[(2., i), (5., i)]);
                path(&[(19., i), (22., i)]);
            }
        }
        Icon::Arrow => {
            path(&[(12., 20.), (12., 4.)]);
            path(&[(5., 11.), (12., 4.), (19., 11.)]);
        }
        Icon::Star => path(&[
            (12., 2.),
            (15., 8.),
            (22., 10.),
            (17., 15.),
            (18., 22.),
            (12., 18.),
            (6., 22.),
            (7., 15.),
            (2., 10.),
            (9., 8.),
            (12., 2.),
        ]),
        Icon::Plug => {
            path(&[(8., 3.), (8., 9.)]);
            path(&[(16., 3.), (16., 9.)]);
            path(&[
                (5., 9.),
                (19., 9.),
                (18., 14.),
                (15., 17.),
                (9., 17.),
                (6., 14.),
                (5., 9.),
            ]);
            path(&[(12., 17.), (12., 22.)]);
        }
        Icon::Check => path(&[(3., 12.), (9., 18.), (21., 5.)]),
        Icon::Search => {
            circle(10., 10., 6.);
            path(&[(15., 15.), (21., 21.)]);
        }
        Icon::Clock => {
            circle(12., 12., 9.);
            path(&[(12., 6.), (12., 12.), (16., 15.)]);
        }
        Icon::Chevron => path(&[(7., 10.), (12., 15.), (17., 10.)]),
        Icon::Memory => path(&[
            (6., 3.),
            (18., 3.),
            (18., 21.),
            (12., 16.),
            (6., 21.),
            (6., 3.),
        ]),
        Icon::Tool => {
            circle(16., 8., 4.5);
            path(&[(13., 11.), (4., 20.)]);
            path(&[(6., 16.), (8., 18.)]);
        }
        Icon::Shield => path(&[
            (12., 2.),
            (20., 5.),
            (20., 11.),
            (16., 18.),
            (12., 22.),
            (8., 18.),
            (4., 11.),
            (4., 5.),
            (12., 2.),
        ]),
        Icon::Branch => {
            circle(6., 5., 2.5);
            circle(6., 19., 2.5);
            circle(18., 7., 2.5);
            path(&[(6., 7.5), (6., 16.5)]);
            path(&[(18., 9.5), (18., 11.), (13., 14.), (6., 15.)]);
        }
    }
}

pub fn control(ui: &mut Ui, icon: Icon, label: &str, size: Vec2, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if selected || response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, 8, if selected { highlight() } else { hover() });
    }
    let x = if label.is_empty() {
        rect.center().x
    } else {
        rect.left() + 17.
    };
    draw(
        ui,
        icon,
        Rect::from_center_size(pos2(x, rect.center().y), vec2(18., 18.)),
        if selected { accent() } else { muted() },
    );
    if !label.is_empty() {
        ui.painter().text(
            pos2(x + 19., rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.),
            if selected { accent() } else { ink() },
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn starter(ui: &mut Ui, icon: Icon, title: &str, detail: &str, width: f32) -> Response {
    let (r, res) = ui.allocate_exact_size(vec2(width, 101.), Sense::click());
    ui.painter().rect(
        r,
        12,
        if res.hovered() { raised() } else { panel() },
        Stroke::new(1.0_f32, if res.hovered() { accent() } else { line() }),
        egui::StrokeKind::Inside,
    );
    draw(
        ui,
        icon,
        Rect::from_min_size(r.min + vec2(15., 13.), vec2(20., 20.)),
        accent(),
    );
    ui.painter().text(
        r.min + vec2(15., 43.),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(16.),
        ink(),
    );
    ui.painter().text(
        r.min + vec2(15., 68.),
        Align2::LEFT_TOP,
        detail,
        FontId::proportional(12.),
        muted(),
    );
    res.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    res.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn small(ui: &mut Ui, icon: Icon, label: &str) -> bool {
    let response = control(ui, icon, "", vec2(32., 30.), false);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response.on_hover_text(label).clicked()
}

pub fn symbol(ui: &mut Ui, icon: Icon, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(18., 18.), Sense::hover());
    draw(ui, icon, r, color);
}
