use super::*;
use base64::Engine as _;
use std::{collections::BTreeMap, io::Cursor, sync::mpsc::{self, Receiver}};

const TILE: f32 = 76.;

pub(super) fn pixels(bytes: &[u8]) -> Option<egui::ColorImage> {
    if bytes.len() > MAX_IMAGE { return None; }
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let small = reader.decode().ok()?.thumbnail(152, 152).to_rgba8();
    let size = [small.width() as usize, small.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw()))
}

pub(super) fn tile(ui: &mut egui::Ui, image: Option<&egui::TextureHandle>, label: &str) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(TILE, TILE), egui::Sense::hover());
    ui.painter().rect_filled(rect, 8., raised());
    if let Some(image) = image {
        let scale = (TILE - 4.) / image.size_vec2().max_elem();
        let bounds = egui::Rect::from_center_size(rect.center(), image.size_vec2() * scale);
        ui.painter().image(image.id(), bounds, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)), Color::WHITE);
    } else {
        draw(ui, Icon::Eye, egui::Rect::from_center_size(rect.center(), egui::vec2(18., 18.)), muted());
    }
    response.on_hover_text(label);
}

type Pixels = Vec<Option<egui::ColorImage>>;

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Key { Message(usize), Pending(u64) }

#[derive(Default)]
pub(super) struct SentImages {
    textures: BTreeMap<Key, Vec<Option<egui::TextureHandle>>>,
    pending: Option<(Key, Receiver<Pixels>)>,
}

impl SentImages {
    pub(super) fn retain_before(&mut self, index: usize, task: Option<u64>) {
        let keep = |key| match key {
            Key::Message(row) => row < index,
            Key::Pending(id) => task == Some(id),
        };
        self.textures.retain(|&key, _| keep(key));
        if self.pending.as_ref().is_some_and(|(key, _)| !keep(*key)) { self.pending = None; }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, row: Key, images: &[String]) {
        if let Some((pending_row, rx)) = &self.pending {
            match rx.try_recv() {
                Ok(decoded) => {
                    let pending_row = *pending_row;
                    if self.textures.len() >= 24
                        && let Some(far) = self.textures.keys().find(|key| **key != row).copied()
                    { self.textures.remove(&far); }
                    let textures = decoded.into_iter().enumerate().map(|(index, pixels)| pixels.map(|pixels|
                        ui.ctx().load_texture(format!("sent-image-{index}"), pixels, egui::TextureOptions::LINEAR))).collect();
                    self.textures.insert(pending_row, textures);
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.textures.insert(*pending_row, vec![]);
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if self.pending.is_none() && !self.textures.contains_key(&row)
            && ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(TILE, TILE)))
        {
            let images: Vec<_> = images.iter().take(4).map(|data| (data.len() <= MAX_IMAGE.div_ceil(3) * 4).then(|| data.clone())).collect();
            let (tx, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let images = images.into_iter().map(|data| {
                    let data = data?;
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
                    pixels(&bytes)
                }).collect();
                let _ = tx.send(images);
                ctx.request_repaint();
            });
            self.pending = Some((row, rx));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true), |ui| {
            let count = images.len().min(4);
            for i in (0..count).rev() {
                let image = self.textures.get(&row).and_then(|items| items.get(i)).and_then(Option::as_ref);
                tile(ui, image, &format!("Attached image {} of {}", i + 1, images.len()));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_use_the_attachment_bytes_and_preserve_aspect_ratio() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(300, 150, image::Rgba([211, 42, 8, 255])));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let thumb = pixels(bytes.get_ref()).unwrap();
        assert_eq!(thumb.size, [152, 76]);
        assert_eq!(thumb.pixels[0], Color::from_rgb(211, 42, 8));
        assert!(pixels(b"Not an image").is_none());
    }
}
