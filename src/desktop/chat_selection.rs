use eframe::egui::{self, Galley, Label, Pos2, Rect, Response, Sense, Ui, WidgetText};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Endpoint {
    label: (usize, usize, usize),
    character: usize,
}

#[derive(Default)]
pub(super) struct Selection {
    range: Option<(Endpoint, Endpoint)>,
    dragging: bool,
    touch: bool,
}

struct Span {
    key: (usize, usize, usize),
    response: Response,
    pos: Pos2,
    galley: Arc<Galley>,
    clip: Rect,
    painter: egui::Painter,
    shape: Option<egui::layers::ShapeIdx>,
    color: egui::Color32,
}

impl Span {
    fn endpoint(&self, pos: Pos2) -> Endpoint {
        Endpoint {
            label: self.key,
            character: self.galley.cursor_from_pos(pos - self.pos).ccursor.index,
        }
    }
}

/// The existing Markdown layout supplies both character positions and reading order.
/// Keep just the endpoints between frames so scrolling and wrapping preserve the range.
#[derive(Default)]
pub(super) struct ChatText {
    spans: Option<Vec<Span>>,
    message: usize,
    block: usize,
    first_span: usize,
}

impl ChatText {
    pub(super) fn collecting() -> Self {
        Self {
            spans: Some(Vec::new()),
            ..Self::default()
        }
    }

    pub(super) fn message(&mut self, index: usize) {
        self.message = index;
        self.block(0);
    }

    pub(super) fn block(&mut self, index: usize) {
        self.block = index;
        self.first_span = self.spans.as_ref().map_or(0, Vec::len);
    }

    pub(super) fn label(
        &mut self,
        ui: &mut Ui,
        text: impl Into<WidgetText>,
        link: bool,
    ) -> Response {
        let text = text.into();
        let Some(spans) = &mut self.spans else {
            return if link {
                ui.link(text)
            } else {
                ui.add(Label::new(text).selectable(true).wrap())
            };
        };
        let touch = ui.input(|i| i.any_touches());
        let sense = if touch {
            Sense::click()
        } else {
            Sense::click_and_drag()
        } - Sense::FOCUSABLE;
        let (pos, galley, response) = Label::new(text)
            .wrap()
            .selectable(false)
            .sense(sense)
            .layout_in_ui(ui);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                if link {
                    egui::WidgetType::Link
                } else {
                    egui::WidgetType::Label
                },
                ui.is_enabled(),
                galley.text(),
            )
        });
        let visible = ui.is_rect_visible(response.rect);
        let color = ui.visuals().text_color();
        let shape = visible.then(|| {
            ui.painter()
                .add(egui::Shape::galley(pos, galley.clone(), color))
        });
        let response = response.on_hover_cursor(if link {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::Text
        });
        spans.push(Span {
            key: (self.message, self.block, spans.len() - self.first_span),
            response: response.clone(),
            pos,
            galley,
            clip: ui.clip_rect(),
            painter: ui.painter().clone(),
            shape,
            color,
        });
        response
    }

    pub(super) fn url(&mut self, ui: &mut Ui, text: WidgetText, url: String) {
        if self.spans.is_none() {
            ui.hyperlink_to(text, url);
        } else {
            let link = self.label(ui, text, true).on_hover_text(&url);
            if link.clicked() || link.middle_clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
    }
}

impl Selection {
    pub(super) fn dragging(&self) -> bool {
        self.dragging
    }

    pub(super) fn copy_rows(&self, ui: &Ui, copy_button: Option<&Response>) -> Option<std::ops::RangeInclusive<usize>> {
        let (a, b) = self.range?;
        let copy = copy_button.is_some_and(|r| r.clicked())
            || (!ui.ctx().wants_keyboard_input() && ui.input(|i| i.events.contains(&egui::Event::Copy)));
        copy.then(|| a.label.0.min(b.label.0)..=a.label.0.max(b.label.0))
    }

    pub(super) fn touch_copy_visible(&self) -> bool {
        self.touch && !self.dragging && self.range.is_some_and(|(a, b)| a != b)
    }

    pub(super) fn update(&mut self, ui: &mut Ui, frame: ChatText, copy_button: Option<Response>) {
        let spans = frame.spans.unwrap_or_default();
        let (pos, pressed, down, released, touch) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.any_touches(),
            )
        });
        let on_copy = copy_button.as_ref().is_some_and(|r| r.contains_pointer());
        if (pressed && !on_copy) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *self = Self::default();
        }
        if let Some(pos) = pos {
            if let Some(span) = spans.iter().find(|s| s.response.contains_pointer()) {
                let word = span.response.double_clicked() || span.response.long_touched();
                if word || (pressed && !touch) {
                    let at = span.endpoint(pos);
                    let (start, end) = if word {
                        let chars: Vec<_> = span.galley.text().chars().collect();
                        let mut start = at.character.min(chars.len().saturating_sub(1));
                        let mut end = start;
                        while start > 0 && !chars[start - 1].is_whitespace() {
                            start -= 1;
                        }
                        while end < chars.len() && !chars[end].is_whitespace() {
                            end += 1;
                        }
                        (
                            Endpoint {
                                character: start,
                                ..at
                            },
                            Endpoint {
                                character: end,
                                ..at
                            },
                        )
                    } else {
                        (at, at)
                    };
                    self.range = Some((start, end));
                    self.touch = touch;
                    self.dragging = down;
                    if touch && down {
                        ui.ctx().set_dragged_id(span.response.id);
                    }
                    ui.memory_mut(|m| {
                        if let Some(id) = m.focused() {
                            m.surrender_focus(id);
                        }
                    });
                }
            }
            let clip = ui.clip_rect();
            let scroll = (clip.top() + 10. - pos.y).max(0.) - (pos.y - clip.bottom() + 10.).max(0.);
            if self.dragging
                && (down || released)
                && (scroll != 0. || ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO))
            {
                // The finger or mouse can be in the gutter between staggered messages.
                let point = clip.clamp(pos);
                if let Some(nearest) = spans
                    .iter()
                    .filter(|s| s.clip.intersects(s.response.rect))
                    .min_by(|a, b| {
                        a.response
                            .rect
                            .distance_sq_to_pos(point)
                            .total_cmp(&b.response.rect.distance_sq_to_pos(point))
                    })
                    && let Some((_, end)) = &mut self.range
                {
                    *end = nearest.endpoint(point);
                }
            }
            if self.dragging && down && scroll != 0. {
                ui.scroll_with_delta_animation(
                    egui::vec2(0., scroll.clamp(-36., 36.)),
                    egui::style::ScrollAnimation::none(),
                );
                ui.ctx().request_repaint();
            }
        }
        if !down {
            self.dragging = false;
        }
        let Some((mut a, mut b)) = self.range else {
            return;
        };
        if !ui.ctx().wants_keyboard_input()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A))
        {
            a = Endpoint { label: (0, 0, 0), character: 0 };
            // Offscreen messages need no layout until the user actually copies them.
            b = Endpoint { label: (usize::MAX, 0, 0), character: 0 };
            self.range = Some((a, b));
        }
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        let copy = copy_button.is_some_and(|r| r.clicked())
            || (!ui.ctx().wants_keyboard_input()
                && ui.input(|i| i.events.contains(&egui::Event::Copy)));
        let mut text = String::new();
        let mut previous: Option<Rect> = None;
        for span in spans.iter().filter(|span| a.label <= span.key && span.key <= b.label) {
            let start = if span.key == a.label { a.character } else { 0 };
            let end = if span.key == b.label {
                b.character
            } else {
                span.galley.end().ccursor.index
            };
            if end <= start {
                continue;
            }
            if copy {
                let rect = span.response.rect;
                if let Some(prev) = previous
                    && rect.top() >= prev.bottom() - 1.
                {
                    text.push('\n');
                } else if let Some(prev) = previous
                    && rect.left() > prev.right() + 1.
                {
                    text.push(' ');
                }
                text.extend(span.galley.text().chars().skip(start).take(end - start));
                previous = Some(rect);
            }
            if let Some(shape) = span.shape {
                let mut galley = span.galley.clone();
                let range = egui::text::CursorRange::two(
                    galley.from_ccursor(egui::text::CCursor::new(start)),
                    galley.from_ccursor(egui::text::CCursor::new(end)),
                );
                egui::text_selection::visuals::paint_text_selection(
                    &mut galley,
                    ui.visuals(),
                    &range,
                    None,
                );
                span.painter
                    .set(shape, egui::Shape::galley(span.pos, galley, span.color));
            }
        }
        if copy && !text.is_empty() {
            #[cfg(target_os = "android")]
            crate::android::copy_text(&text);
            ui.ctx().copy_text(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, TouchPhase};

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn touch(pos: Pos2, phase: TouchPhase) -> Event {
        Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase,
            pos,
            force: None,
        }
    }

    #[test]
    fn touching_selects_after_a_hold_and_copies_across_messages() {
        let ctx = egui::Context::default();
        let mut selection = Selection::default();
        let mut locations = [Pos2::ZERO; 3];
        let mut frame = |time, events| {
            let raw = egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            };
            let output = ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| {
                        let copy = selection
                            .touch_copy_visible()
                            .then(|| ui.small_button("Copy selection"));
                        if let Some(r) = &copy {
                            locations[2] = r.rect.center();
                        }
                        ui.push_id("history", |ui| {
                            let mut text = ChatText::collecting();
                            locations[0] = text.label(ui, "begin résumé", false).rect.center();
                            locations[1] = text.label(ui, "終わり finish", false).rect.center();
                            selection.update(ui, text, copy);
                        });
                    })
                    .inner
            });
            let copied = output
                .platform_output
                .commands
                .into_iter()
                .find_map(|c| match c {
                    egui::OutputCommand::CopyText(text) => Some(text),
                    _ => None,
                });
            (locations, copied, selection.touch_copy_visible())
        };
        let ([a, b, _], _, _) = frame(0., vec![]);
        let _ = frame(
            0.1,
            vec![
                touch(a, TouchPhase::Start),
                Event::PointerMoved(a),
                button(a, true),
            ],
        );
        let _ = frame(1.1, vec![]);
        frame(
            1.2,
            vec![touch(b, TouchPhase::Move), Event::PointerMoved(b)],
        );
        let (_, _, ready) = frame(1.3, vec![touch(b, TouchPhase::End), button(b, false)]);
        assert!(ready, "a held touch must leave a copyable range");
        let ([_, _, copy], _, _) = frame(1.4, vec![]);
        frame(
            1.5,
            vec![
                touch(copy, TouchPhase::Start),
                Event::PointerMoved(copy),
                button(copy, true),
            ],
        );
        let (_, copied, _) = frame(1.6, vec![touch(copy, TouchPhase::End), button(copy, false)]);
        assert!(
            copied
                .as_deref()
                .is_some_and(|s| s.contains("résumé\n終わり")),
            "{copied:?}"
        );
    }

    #[test]
    fn reverse_drag_keeps_offscreen_messages_when_the_chat_scrolls() {
        let ctx = egui::Context::default();
        let mut selection = Selection::default();
        let mut frame = |events| {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400., 160.))),
                events,
                ..Default::default()
            };
            let mut bottom = Pos2::ZERO;
            let out = ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(!selection.dragging())
                            .show(ui, |ui| {
                                let mut text = ChatText::collecting();
                                for i in 0..24 {
                                    bottom = text
                                        .label(ui, format!("turn {i:02} café"), false)
                                        .rect
                                        .center();
                                }
                                selection.update(ui, text, None);
                            });
                    })
                    .inner
            });
            let copied = out
                .platform_output
                .commands
                .into_iter()
                .find_map(|c| match c {
                    egui::OutputCommand::CopyText(s) => Some(s),
                    _ => None,
                });
            (bottom, copied)
        };
        frame(vec![]);
        let (start, _) = frame(vec![]);
        frame(vec![Event::PointerMoved(start), button(start, true)]);
        let edge = egui::pos2(start.x, -18.);
        frame(vec![Event::PointerMoved(edge)]);
        for _ in 0..100 {
            frame(vec![]);
        }
        frame(vec![button(edge, false)]);
        let (_, copied) = frame(vec![Event::Copy]);
        let text = copied.expect("copied the reverse selection");
        assert!(
            text.contains("turn 01 café\nturn 02") && text.contains("turn 22 café\nturn"),
            "{text}"
        );
    }
}
