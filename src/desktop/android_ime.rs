use eframe::egui;
#[cfg(target_os = "android")]
use egui::text::{CCursor, CCursorRange};
#[cfg(any(target_os = "android", test))]
use std::time::{Duration, Instant};

// Android can report a temporary loss of egui focus while the keyboard changes
// the available window size. Keep the native editor associated with that field
// during the transition. A different focused widget or a closed panel ends it.
#[cfg(any(target_os = "android", test))]
#[derive(Default)]
struct FocusGrace(Option<Instant>);

#[cfg(any(target_os = "android", test))]
impl FocusGrace {
    fn keep_editor(
        &mut self,
        now: Instant,
        rendered: Option<egui::Id>,
        focused: Option<egui::Id>,
    ) -> bool {
        let Some(id) = rendered else {
            self.0 = None;
            return false;
        };
        if focused == Some(id) {
            self.0 = None;
            return true;
        }
        if focused.is_some() {
            self.0 = None;
            return false;
        }
        now.duration_since(*self.0.get_or_insert(now)) < Duration::from_millis(500)
    }
}

#[derive(Default)]
pub(super) struct ComposerIme {
    #[cfg(target_os = "android")]
    revision: i32,
    #[cfg(target_os = "android")]
    active: Option<Editor>,
    #[cfg(target_os = "android")]
    seen: Option<egui::Id>,
    #[cfg(target_os = "android")]
    focus_grace: FocusGrace,
}

#[cfg(target_os = "android")]
struct Editor {
    key: egui::Id,
    text: String,
    selection: (usize, usize),
    clip_bottom: f32,
}

#[cfg(target_os = "android")]
fn char_index(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    text.chars()
        .take_while(|ch| {
            units += ch.len_utf16();
            units <= utf16
        })
        .count()
}

#[cfg(target_os = "android")]
fn utf16_index(text: &str, index: usize) -> usize {
    text.chars().take(index).map(char::len_utf16).sum()
}

impl ComposerIme {
    pub(super) fn field(
        &mut self,
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash,
        text: &mut String,
        field: Field,
        style: impl for<'a> FnOnce(egui::TextEdit<'a>) -> egui::TextEdit<'a>,
    ) -> egui::Response {
        let id = ui.make_persistent_id(salt);
        self.show(ui, id, id, text, Some(field), |text| {
            style(
                egui::TextEdit::singleline(text)
                    .password(field.password)
                    .char_limit(field.limit),
            )
        })
    }

    pub(super) fn multiline(
        &mut self,
        ui: &mut egui::Ui,
        salt: impl std::hash::Hash,
        text: &mut String,
        style: impl for<'a> FnOnce(egui::TextEdit<'a>) -> egui::TextEdit<'a>,
    ) -> egui::Response {
        let id = ui.make_persistent_id(salt);
        self.show(ui, id, id, text, None, |text| {
            style(egui::TextEdit::multiline(text))
        })
    }

    #[cfg(target_os = "android")]
    pub(super) fn edit(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        key: egui::Id,
        text: &mut String,
        hint: &str,
    ) -> egui::Response {
        self.show(ui, id, key, text, None, |text| {
            egui::TextEdit::multiline(text)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .frame(false)
                .hint_text(hint)
        })
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        _key: egui::Id,
        text: &mut String,
        _field: Option<Field>,
        build: impl for<'a> FnOnce(&'a mut String) -> egui::TextEdit<'a>,
    ) -> egui::Response {
        #[cfg(not(target_os = "android"))]
        {
            ui.add(build(text).id(id))
        }
        #[cfg(target_os = "android")]
        {
            let mode = _field.map_or(0, |field| if field.password { 2 } else { 1 });
            let limit = _field.map_or(usize::MAX, |field| field.limit);
            self.edit_native(ui, id, _key, text, limit, mode, build)
        }
    }

    #[cfg(target_os = "android")]
    fn edit_native(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        key: egui::Id,
        text: &mut String,
        limit: usize,
        mode: i32,
        build: impl for<'a> FnOnce(&'a mut String) -> egui::TextEdit<'a>,
    ) -> egui::Response {
        let mut changed_by_ime = false;
        if let Some(active) = &mut self.active
            && active.key == key
            && active.text == *text
            && let Some((revision, next, start, end)) = crate::android::take_composer_ime()
            && revision == self.revision
        {
            active.text.clone_from(&next);
            let next = if limit == usize::MAX {
                next
            } else {
                next.chars().take(limit).collect()
            };
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange {
                primary: CCursor::new(char_index(&next, end)),
                secondary: CCursor::new(char_index(&next, start)),
            }));
            state.store(ui.ctx(), id);
            changed_by_ime = *text != next;
            active.selection = (start, end);
            *text = next;
        }
        let mut out = build(text).id(id).show(ui);
        if changed_by_ime {
            out.response.mark_changed();
        }
        if out.response.has_focus() {
            let clip_bottom = ui.clip_rect().bottom();
            if let Some(active) = self.active.as_mut().filter(|editor| editor.key == key) {
                if clip_bottom < active.clip_bottom && out.response.rect.bottom() > clip_bottom {
                    out.response.scroll_to_me(Some(egui::Align::Center));
                }
                active.clip_bottom = clip_bottom;
            }
            let selection = out
                .cursor_range
                .map(|range| {
                    let range = range.as_ccursor_range();
                    (
                        utf16_index(text, range.secondary.index),
                        utf16_index(text, range.primary.index),
                    )
                })
                .unwrap_or_else(|| {
                    let end = text.encode_utf16().count();
                    (end, end)
                });
            let changed = self
                .active
                .as_ref()
                .is_none_or(|a| a.key != key || a.text != *text || a.selection != selection);
            let show = self.active.as_ref().is_none_or(|a| a.key != key) || out.response.clicked();
            if changed {
                self.revision = self.revision.wrapping_add(1).max(1);
            }
            if (changed || show)
                && crate::android::set_composer_ime(
                    self.revision,
                    text,
                    selection.0 as i32,
                    selection.1 as i32,
                    show,
                    mode,
                )
            {
                self.active = Some(Editor {
                    key,
                    text: text.clone(),
                    selection,
                    clip_bottom,
                });
            }
        }
        if self.active.as_ref().is_some_and(|a| a.key == key) {
            self.seen = Some(id);
        }
        out.response
    }

    #[cfg(target_os = "android")]
    pub(super) fn finish_frame(&mut self, ctx: &egui::Context) {
        // Otherwise egui-winit independently calls Android's show/hide on the
        // NativeActivity window, competing with our Java EditText for the IME.
        // Forward keyboard demand through Spindle's bridge only.
        ctx.output_mut(|output| output.ime = None);
        let rendered = self.seen.take();
        let focused = ctx.memory(|m| m.focused());
        if self.focus_grace.keep_editor(Instant::now(), rendered, focused) {
            crate::android::keyboard(false, true);
            ctx.request_repaint_after(Duration::from_millis(33));
        } else if self.active.take().is_some() {
            crate::android::close_composer_ime();
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        } else {
            crate::android::keyboard(ctx.wants_keyboard_input(), false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_keyboard_survives_transient_focus_loss_and_closes_on_navigation() {
        let mut grace = FocusGrace::default();
        let field = egui::Id::new("delete-confirmation");
        let other = egui::Id::new("unrelated-field");
        let now = Instant::now();
        assert!(grace.keep_editor(now, Some(field), Some(field)));
        assert!(grace.keep_editor(now, Some(field), None));
        assert!(grace.keep_editor(now + Duration::from_millis(300), Some(field), None));
        assert!(grace.keep_editor(now + Duration::from_millis(350), Some(field), Some(field)));
        assert!(!grace.keep_editor(now, Some(field), Some(other)));
        assert!(!grace.keep_editor(now, None, None));
        assert!(grace.keep_editor(now, Some(field), None));
        assert!(!grace.keep_editor(now + Duration::from_millis(500), Some(field), None));
    }
}

#[derive(Clone, Copy)]
pub(super) struct Field {
    pub(super) password: bool,
    pub(super) limit: usize,
}

impl Default for Field {
    fn default() -> Self {
        Self {
            password: false,
            limit: usize::MAX,
        }
    }
}
