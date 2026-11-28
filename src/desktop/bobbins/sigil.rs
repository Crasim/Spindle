use super::*;
use crate::bobbins::sigil::{
    Architecture, Direction, Kernel, Line, ModelMark, Outline, Shell, Sigil, Terms, Ticks,
    TierBasis,
};
use egui::{Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Activity {
    #[default]
    Ready,
    Complete,
    Failed,
    Unavailable,
    Paused,
    Queued,
    Working,
    UsingTools,
    NeedsApproval,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Complete => "Completed",
            Self::Failed => "Run failed",
            Self::Unavailable => "Model unavailable",
            Self::Paused => "Paused",
            Self::Queued => "Queued",
            Self::Working => "Working",
            Self::UsingTools => "Using tools",
            Self::NeedsApproval => "Needs approval",
        }
    }

    fn color(self, base: Color) -> Color {
        match self {
            Self::Ready => base,
            Self::Complete => Color::from_rgb(94, 211, 160),
            Self::Working => Color::from_rgb(102, 197, 237),
            Self::UsingTools => Color::from_rgb(153, 151, 241),
            Self::NeedsApproval => Color::from_rgb(250, 185, 100),
            Self::Queued => Color::from_rgb(146, 173, 225),
            Self::Failed => Color::from_rgb(238, 120, 133),
            Self::Paused | Self::Unavailable => Color::from_rgb(149, 160, 178),
        }
    }
}

fn random(seed: u64, index: usize) -> f32 {
    let mut mixed = seed.wrapping_add((index as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15));
    mixed = ((mixed >> 30) ^ mixed).wrapping_mul(0xbf58476d1ce4e5b9);
    mixed = ((mixed >> 27) ^ mixed).wrapping_mul(0x94d049bb133111eb);
    ((mixed ^ (mixed >> 31)) >> 40) as f32 / 16_777_216.
}

fn radial(angle: f32) -> Vec2 {
    vec2(angle.cos(), angle.sin())
}

fn polygon(geometry: Outline, center: Pos2, inradius: f32, angle: f32) -> Vec<Pos2> {
    let (sides, offset) = match geometry {
        Outline::Circle => (64, 0.),
        Outline::Square => (4, PI / 4.),
        Outline::Diamond => (4, 0.),
        Outline::Hexagon => (6, PI / 6.),
        Outline::Octagon => (8, PI / 8.),
    };
    let half = PI / sides as f32;
    (0..=sides)
        .map(|i| {
            center
                + radial(angle + offset + i as f32 * half * 2. - FRAC_PI_2)
                    * (inradius / half.cos())
        })
        .collect()
}

fn mix(a: Color, b: Color, amount: f32) -> Color {
    let channel = |left: u8, right: u8| (left as f32 + (right as f32 - left as f32) * amount) as u8;
    Color::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

struct Ink<'a> {
    paint: &'a Painter,
    rect: Rect,
    color: Color,
    axis: Vec2,
}

impl Ink<'_> {
    fn shade(&self, p: Pos2) -> Color {
        let blend =
            ((p - self.rect.center()).dot(self.axis) / self.rect.width() + 0.5).clamp(0., 1.);
        mix(
            self.color.gamma_multiply(0.72),
            mix(self.color, Color::WHITE, 0.36),
            blend,
        )
    }

    fn lines(&self, points: &[Pos2], width: f32, line: Line) {
        let mut mesh = egui::Mesh::default();
        let mut distance = 0.;
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let delta = b - a;
            let len = delta.length();
            if len <= 0.01 {
                continue;
            }
            let normal = vec2(-delta.y, delta.x) / len * width * 0.5;
            let mut start = 0.;
            while start < len {
                let (period, on) = match line {
                    Line::Dotted => (5., 1.8),
                    Line::Dashed => (11., 6.5),
                    _ => (0., 0.),
                };
                let phase = if period > 0. {
                    (distance + start) % period
                } else {
                    0.
                };
                let draw = period == 0. || phase < on;
                let end = if period == 0. {
                    len
                } else {
                    (start + (if draw { on - phase } else { period - phase }).max(0.02)).min(len)
                };
                if draw {
                    let from = a + delta * (start / len);
                    let to = a + delta * (end / len);
                    let v = mesh.vertices.len() as u32;
                    for (p, color) in [
                        (from + normal, self.shade(from)),
                        (from - normal, self.shade(from)),
                        (to + normal, self.shade(to)),
                        (to - normal, self.shade(to)),
                    ] {
                        mesh.colored_vertex(p, color);
                    }
                    mesh.add_triangle(v, v + 1, v + 2);
                    mesh.add_triangle(v + 2, v + 1, v + 3);
                }
                start = end;
            }
            distance += len;
        }
        if line == Line::Glow {
            self.paint.add(egui::Shape::line(
                points.to_vec(),
                Stroke::new(width * 3.5, self.color.gamma_multiply(0.23)),
            ));
        }
        self.paint.add(egui::Shape::mesh(mesh));
    }

    fn ring(&self, center: Pos2, radius: f32, width: f32) {
        self.paint
            .circle_stroke(center, radius, Stroke::new(width, self.shade(center)));
    }

    fn kernel(
        &self,
        c: Pos2,
        r: f32,
        family: Kernel,
        turn: f32,
        phase: f32,
        filled: bool,
        compact: bool,
    ) {
        let p = |x: f32, y: f32| {
            c + vec2(
                x * turn.cos() - y * turn.sin(),
                x * turn.sin() + y * turn.cos(),
            ) * r
        };
        let w = (r * 0.10).clamp(1.1, 2.);
        let stroke = |points: &[Pos2]| self.lines(points, w, Line::Solid);
        let sampled = |n: usize, point: &dyn Fn(f32) -> Pos2| {
            (0..=n)
                .map(|i| point(i as f32 / n as f32 * TAU))
                .collect::<Vec<_>>()
        };
        match family {
            Kernel::Iris => {
                for i in 0..6 {
                    let a = i as f32 * TAU / 6. + turn;
                    let aperture = 0.28 + 0.05 * phase.sin();
                    let vertices = vec![
                        c + radial(a) * r * 0.90,
                        c + radial(a + 1.05) * r * 0.90,
                        c + radial(a + 1.7) * r * aperture,
                        c + radial(a + 0.64) * r * aperture,
                    ];
                    let color = self.shade(vertices[0]);
                    self.paint.add(egui::Shape::convex_polygon(
                        vertices,
                        if filled || compact {
                            color
                        } else {
                            Color::TRANSPARENT
                        },
                        Stroke::new(
                            w * 0.64,
                            if filled || compact {
                                Color::from_rgb(13, 22, 35)
                            } else {
                                color
                            },
                        ),
                    ));
                }
            }
            Kernel::Wave => {
                let count = if compact { 8 } else { 14 };
                for i in 0..count {
                    let a = i as f32 * TAU / count as f32;
                    let end = 0.75 + (a * 3. - phase).sin() * 0.24;
                    stroke(&[
                        p(a.cos() * 0.32, a.sin() * 0.32),
                        p(a.cos() * end, a.sin() * end),
                    ]);
                }
            }
            Kernel::Star => {
                let nodes = (0..=8)
                    .map(|i| {
                        let a = i as f32 * PI / 4.;
                        let len = if i % 2 == 0 { 1. } else { 0.30 };
                        p(a.cos() * len, a.sin() * len)
                    })
                    .collect::<Vec<_>>();
                if filled {
                    for segment in nodes.windows(2) {
                        self.paint.add(egui::Shape::convex_polygon(
                            vec![c, segment[0], segment[1]],
                            self.shade(segment[0]),
                            Stroke::NONE,
                        ));
                    }
                }
                stroke(&nodes);
            }
            Kernel::Horizon => {
                self.ring(c, r * 0.40, w);
                let outer = (0..=24)
                    .map(|i| {
                        let a = i as f32 / 24. * TAU * 0.80;
                        p(a.cos(), a.sin() * 0.57)
                    })
                    .collect::<Vec<_>>();
                stroke(&outer);
            }
            Kernel::Ribbon => stroke(&sampled(40, &|a| p(a.sin(), a.sin() * a.cos() * 1.18))),
            Kernel::Blob => {
                let nodes = sampled(26, &|a| {
                    let len = 0.74 + 0.13 * (a * 3. + phase).sin() + 0.06 * (a * 5.).cos();
                    p(a.cos() * len, a.sin() * len)
                });
                stroke(&nodes);
            }
            Kernel::Mirror => {
                for side in [-1., 1.] {
                    let nodes = (0..=18)
                        .map(|i| {
                            let x = (i as f32 / 18.) * 1.8 - 0.9;
                            p(
                                x,
                                side * (0.3 + 0.36 * (x * 5. + phase).cos().abs())
                                    * (1. - x.abs() * 0.55),
                            )
                        })
                        .collect::<Vec<_>>();
                    stroke(&nodes);
                }
            }
            Kernel::Geometry => {
                let nodes = polygon(Outline::Hexagon, c, r * 0.7, turn);
                stroke(&nodes);
                if !compact {
                    stroke(&polygon(Outline::Diamond, c, r * 0.43, -turn));
                }
            }
        }
    }
}

pub(super) fn name<T: Copy + Eq>(choices: &[(T, &'static str)], value: T) -> &'static str {
    choices
        .iter()
        .find(|(item, _)| *item == value)
        .map(|(_, text)| *text)
        .unwrap_or("")
}

pub(super) fn details(
    ui: &mut egui::Ui,
    config: &Sigil,
    model: &str,
    activity: Activity,
    recorded: Option<f64>,
) {
    let mark = config.mark(model);
    let (tier, basis) = config.tier(model, crate::ollama::known_size(model));
    ui.label(format!(
        "{} · {}",
        name(crate::bobbins::sigil::Role::CHOICES, config.role),
        name(crate::bobbins::sigil::Rank::CHOICES, config.rank)
    ));
    ui.label(format!(
        "{} · {}",
        name(Architecture::CHOICES, mark.architecture),
        name(Terms::CHOICES, mark.terms)
    ));
    ui.label(match (tier, basis) {
        (Some(tier), TierBasis::Reported(size)) => {
            format!("Tier {tier} · {size:.3}B reported by Ollama")
        }
        (Some(tier), TierBasis::Override) => format!("Tier {tier} · configured for this model"),
        _ => "Tier unresolved".into(),
    });
    if let Some(layers) = mark.physical_layers {
        ui.label(format!(
            "{layers} physical layers · configured for this model"
        ));
    }
    if let Some(lineage) = &config.lineage {
        ui.label(format!("Lineage · generation {}", lineage.depth))
            .on_hover_text(format!("Parent Bobbin {}", lineage.parent));
    }
    ui.label(activity.label());
    if let Some(rate) = recorded.filter(|rate| rate.is_finite() && *rate > 0.) {
        ui.weak(format!("{rate:.1} tok/s · last recorded for this run"));
    } else {
        ui.weak("Throughput unavailable");
    }
}

fn draw(
    paint: &Painter,
    rect: Rect,
    config: &Sigil,
    mark: ModelMark,
    tier: Option<u8>,
    color: Color,
    time: f32,
    rotation: f32,
    embedded: bool,
) {
    let len = rect.width().min(rect.height());
    let c = rect.center();
    let compact = len < 48.;
    let detailed = len >= 160.;
    let ink = Ink {
        paint,
        rect,
        color,
        axis: radial(random(config.seed, 0) * TAU),
    };
    let border_size = len
        * if config.border == Outline::Diamond {
            0.295
        } else {
            0.34
        };
    if !embedded && config.border != Outline::Square {
        let outer = polygon(config.border, c, border_size, 0.);
        match mark.terms {
            Terms::Permissive => {
                for i in [0.08, 0.58] {
                    let start = (i * (outer.len() - 1) as f32) as usize;
                    let end = (start + outer.len() / 3).min(outer.len() - 1);
                    ink.lines(&outer[start..=end], 1.2, Line::Solid);
                }
            }
            terms => ink.lines(
                &outer,
                1.2,
                match terms {
                    Terms::Closed => Line::Solid,
                    Terms::Restricted => Line::Dashed,
                    _ => Line::Dotted,
                },
            ),
        }
    }
    if !compact {
        let rank = config.rank.circles();
        let top = if embedded || config.border == Outline::Square {
            c.y - len * 0.40
        } else {
            polygon(config.border, c, border_size, 0.)
                .into_iter()
                .map(|p| p.y)
                .fold(c.y, f32::min)
                - len * 0.035
        };
        let gap = (len * 0.048).clamp(3.8, 9.);
        for i in 0..rank {
            ink.ring(
                pos2(c.x + (i as f32 - (rank - 1) as f32 * 0.5) * gap, top),
                (gap * 0.27).max(1.3),
                1.1,
            );
        }
        if detailed && let Some(lineage) = &config.lineage {
            let dots = lineage.depth.min(6);
            for i in 0..dots {
                let p = pos2(
                    c.x + (i as f32 - (dots - 1) as f32 * 0.5) * 5.,
                    c.y + len * 0.46,
                );
                paint.circle_filled(p, 1.3, ink.shade(p));
            }
        }
        if let Some(tier) = tier.map(|tier| tier.min(6)) {
            let count = if len < 96. { tier.min(1) } else { tier };
            let motion = if config.motion_seed == 0 {
                config.seed
            } else {
                config.motion_seed
            };
            for layer in 0..count as usize {
                let direction = match config.directions[layer] {
                    Direction::Clockwise => 1.,
                    Direction::Counterclockwise => -1.,
                    Direction::Seeded => {
                        if (layer % 2 == 0) == (random(motion, 3) > 0.5) {
                            1.
                        } else {
                            -1.
                        }
                    }
                };
                let angle = random(config.seed, 20 + layer) * 0.8
                    + rotation * direction * (0.56 + random(motion, 6 + layer) * 0.75);
                let fraction = if count == 1 {
                    1.
                } else {
                    layer as f32 / (count - 1) as f32
                };
                ink.lines(
                    &polygon(config.geometry, c, len * (0.12 + 0.115 * fraction), angle),
                    if detailed { 1.25 } else { 1. },
                    config.lines[layer],
                );
            }
        } else if len >= 92. {
            // Only the unresolved mark has a small broken guide arc.
            let guide = (0..=14)
                .map(|i| c + radial(0.6 + i as f32 * 0.12) * len * 0.23)
                .collect::<Vec<_>>();
            ink.lines(&guide, 0.9, Line::Dotted);
        }
    }
    if detailed && let Some(layers) = mark.physical_layers {
        let ticks = usize::from(layers).clamp(1, 64);
        for i in 0..ticks {
            let a = (i as f32 / ticks as f32) * TAU + rotation * 0.42;
            let normal = radial(a);
            let tangent = vec2(-normal.y, normal.x);
            let p = c + normal * len * 0.30;
            let seg = |dx: f32, dy: f32| p + tangent * dx + normal * dy;
            let stroke = Stroke::new(1.1_f32, ink.shade(p));
            match config.ticks {
                Ticks::Bars => {
                    paint.line_segment([seg(0., -2.), seg(0., 2.)], stroke);
                }
                Ticks::Circles => ink.ring(p, 1.8, 1.0),
                Ticks::Dots => {
                    paint.circle_filled(p, 1.5, ink.shade(p));
                }
                Ticks::Wedges => {
                    paint.add(egui::Shape::convex_polygon(
                        vec![seg(-2., -1.), seg(2., -1.), seg(0., 2.)],
                        ink.shade(p),
                        Stroke::NONE,
                    ));
                }
                Ticks::Chevrons => {
                    paint.add(egui::Shape::line(
                        vec![seg(-2., -1.), seg(0., 2.), seg(2., -1.)],
                        stroke,
                    ));
                }
                Ticks::Pairs => {
                    for dx in [-1.2, 1.2] {
                        paint.line_segment([seg(dx, -2.), seg(dx, 2.)], stroke);
                    }
                }
                Ticks::Diamonds => {
                    ink.lines(&polygon(Outline::Diamond, p, 1.4, a), 1., Line::Solid)
                }
            }
        }
    }
    let phase = time * 0.72 + random(config.seed, 12) * TAU;
    if mark.architecture == Architecture::Moe && !compact {
        let centers = [
            (-0.18, -0.12),
            (0.09, -0.19),
            (0.19, 0.04),
            (-0.13, 0.16),
            (0.08, 0.20),
        ];
        for (i, &(x, y)) in centers
            .iter()
            .take(if len < 96. { 3 } else { 5 })
            .enumerate()
        {
            let shift = random(config.seed, i + 33);
            let path =
                radial(phase * 0.23 + shift * TAU) * (if detailed { len * 0.011 } else { 0. });
            let point = c + vec2(x, y) * len + path;
            let radius = len * (0.042 + shift * 0.014) * (0.94 + 0.10 * (phase + i as f32).sin());
            if len < 96. {
                paint.circle_filled(point, radius.max(1.5), ink.shade(point));
            } else {
                ink.kernel(
                    point,
                    radius,
                    mark.kernel,
                    shift * TAU + time * 0.09,
                    phase + shift,
                    shift > 0.35,
                    !detailed,
                );
            }
        }
    } else {
        let drift = radial(time * 0.24 + random(config.seed, 15) * TAU)
            * (if detailed { len * 0.008 } else { 0. });
        ink.kernel(
            c + drift,
            len * if compact { 0.25 } else { 0.115 },
            mark.kernel,
            time * 0.07,
            phase,
            mark.architecture != Architecture::Unknown && random(config.seed, 11) > 0.24,
            compact,
        );
    }
}

#[derive(serde::Deserialize)]
struct Sheet {
    id: String,
    seconds: f32,
    display: [f32; 4],
    frames: [[f32; 4]; 8],
}

const SHEETS: [(Shell, &str, &[u8]); 7] = [
    (
        Shell::Pebble,
        "pebble",
        include_bytes!("../../../assets/bobbins/pebble.webp"),
    ),
    (
        Shell::Cloud,
        "cloud",
        include_bytes!("../../../assets/bobbins/cloud.webp"),
    ),
    (
        Shell::Sprout,
        "sprout",
        include_bytes!("../../../assets/bobbins/sprout.webp"),
    ),
    (
        Shell::Mantle,
        "mantle",
        include_bytes!("../../../assets/bobbins/mantle.webp"),
    ),
    (
        Shell::Kite,
        "kite",
        include_bytes!("../../../assets/bobbins/kite.webp"),
    ),
    (
        Shell::Lantern,
        "lantern",
        include_bytes!("../../../assets/bobbins/lantern.webp"),
    ),
    (
        Shell::Skiff,
        "skiff",
        include_bytes!("../../../assets/bobbins/skiff.webp"),
    ),
];

static MANIFEST: std::sync::LazyLock<Vec<Sheet>> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../assets/bobbins/shells.json"))
        .expect("bundled Bobbin shell manifest")
});

#[derive(Default)]
pub(super) struct Textures([Option<egui::TextureHandle>; 7]);

fn relative(rect: Rect, [x, y, w, h]: [f32; 4]) -> Rect {
    Rect::from_min_size(
        rect.min + rect.size() * vec2(x, y),
        rect.size() * vec2(w, h),
    )
}

impl Textures {
    fn character(
        &mut self,
        ui: &egui::Ui,
        painter: &Painter,
        rect: Rect,
        shell: Shell,
        time: f32,
    ) -> Option<Rect> {
        let index = SHEETS
            .iter()
            .position(|(candidate, _, _)| *candidate == shell)?;
        let (_, id, bytes) = SHEETS[index];
        let sheet = MANIFEST.iter().find(|sheet| sheet.id == id)?;
        let texture = &mut self.0[index];
        if texture.is_none() {
            let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
                .ok()?
                .to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            *texture = Some(ui.ctx().load_texture(
                id,
                egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                egui::TextureOptions::LINEAR,
            ));
        }
        let texture = texture.as_ref()?;
        let playhead = (time / sheet.seconds).rem_euclid(1.) * 8.;
        let index = playhead.floor() as usize;
        let blend = ((playhead.fract() - 0.64) / 0.36).clamp(0., 1.);
        let blend = blend * blend * (3. - 2. * blend);
        let tile = |index| {
            Rect::from_min_size(
                pos2((index % 4) as f32 / 4., (index / 4) as f32 / 2.),
                vec2(0.25, 0.5),
            )
            .shrink2(vec2(
                0.5 / texture.size()[0] as f32,
                0.5 / texture.size()[1] as f32,
            ))
        };
        painter.image(
            texture.id(),
            relative(rect, sheet.frames[index]),
            tile(index),
            Color::WHITE,
        );
        if blend > 0. {
            let next = (index + 1) % 8;
            painter.image(
                texture.id(),
                relative(rect, sheet.frames[next]),
                tile(next),
                Color::WHITE.gamma_multiply(blend),
            );
        }
        let face = relative(rect, sheet.display);
        Some(Rect::from_center_size(
            face.center(),
            Vec2::splat(face.width().min(face.height()) * 0.9),
        ))
    }
}

pub(super) fn preview(
    ui: &egui::Ui,
    rect: Rect,
    textures: &mut Textures,
    draft: &Draft,
    color: Color,
    look: Look<'_>,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let motion = draft.sigil.motion && rect.width() >= 96.;
    let time = if motion {
        ui.ctx().request_repaint_after(Duration::from_millis(42));
        ui.input(|i| (i.time % 3600.) as f32)
    } else {
        0.
    };
    let rotation = ui.data_mut(|data| {
        let id = ui.id().with("sigil-rotation");
        let (last, angle) = data.get_temp::<(f32, f32)>(id).unwrap_or((time, 0.));
        let delta = (time - last).clamp(0., 0.10);
        let angle = angle + delta * look.demo_rate.unwrap_or(0.).clamp(0., 200.) * 0.016;
        data.insert_temp(id, (time, angle));
        if motion { angle } else { 0. }
    });
    let painter = ui.painter_at(rect);
    let face = (draft.picture == Picture::Character)
        .then(|| {
            textures.character(
                ui,
                &painter,
                rect.shrink(rect.width() * 0.06),
                draft.sigil.shell,
                time,
            )
        })
        .flatten();
    let model = look.model.unwrap_or(&draft.model);
    let (tier, _) = draft.sigil.tier(model, crate::ollama::known_size(model));
    draw(
        &painter,
        face.unwrap_or(rect),
        &draft.sigil,
        draft.sigil.mark(model),
        tier,
        look.activity.color(color),
        time,
        rotation,
        face.is_some(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn bundled_shells_have_registered_frames_and_a_display() {
        assert_eq!(MANIFEST.len(), SHEETS.len());
        for (_, id, bytes) in SHEETS {
            let sheet = MANIFEST.iter().find(|sheet| sheet.id == id).unwrap();
            assert!(sheet.seconds > 0.);
            assert_eq!(
                image::load_from_memory(bytes).unwrap().dimensions(),
                (768, 384)
            );
            let display = relative(
                Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.)),
                sheet.display,
            );
            for frame in sheet.frames {
                assert!(
                    relative(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.)), frame)
                        .contains_rect(display)
                );
            }
        }
    }

    #[test]
    fn kernels_and_rank_marks_fit_compact_and_detailed_previews() {
        let ctx = egui::Context::default();
        for length in [32., 64., 128., 192., 320.] {
            for &(kernel, _) in Kernel::CHOICES {
                for architecture in [Architecture::Dense, Architecture::Moe] {
                    let rect = Rect::from_min_size(pos2(4., 4.), Vec2::splat(length));
                    let out = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(rect.expand(4.)),
                            ..Default::default()
                        },
                        |ctx| {
                            let paint = ctx.layer_painter(egui::LayerId::background());
                            draw(
                                &paint,
                                rect,
                                &Sigil {
                                    border: Outline::Diamond,
                                    rank: crate::bobbins::sigil::Rank::Custom,
                                    ..Default::default()
                                },
                                ModelMark {
                                    kernel,
                                    architecture,
                                    physical_layers: Some(64),
                                    ..Default::default()
                                },
                                Some(6),
                                Color::LIGHT_BLUE,
                                2.4,
                                1.1,
                                false,
                            );
                        },
                    );
                    assert!(out.shapes.len() >= 2);
                    for clipped in &out.shapes {
                        let bounds = clipped.shape.visual_bounding_rect();
                        if bounds.is_positive() {
                            assert!(
                                rect.expand(1.).contains_rect(bounds),
                                "{kernel:?} at {length}: {bounds:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
