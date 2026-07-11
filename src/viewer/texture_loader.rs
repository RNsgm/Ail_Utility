use std::collections::HashMap;

use egui::TextureHandle;

/// Кэш текстур для отображения изображений
pub struct TextureCache {
    textures: HashMap<String, TextureHandle>,
}

impl TextureCache {
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }

    /// Создает текстуру из уже готового ColorImage
    pub fn load_from_color_image(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        color_image: egui::ColorImage,
    ) -> TextureHandle {
        let texture = ctx.load_texture(key, color_image, egui::TextureOptions::LINEAR);
        self.textures.insert(key.to_string(), texture.clone());
        texture
    }

    pub fn get(&self, key: &str) -> Option<TextureHandle> {
        self.textures.get(key).cloned()
    }

    pub fn clear(&mut self) {
        self.textures.clear();
    }
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::new()
    }
}

pub const MAX_TEXTURE_SIDE: u32 = 16000;

/// Загружает изображение из памяти и возвращает список ColorImage
pub fn load_color_images_from_memory(data: &[u8]) -> Vec<egui::ColorImage> {
    let Some(image) = image::load_from_memory(data).ok() else {
        return vec![];
    };

    let width = image.width();
    let height = image.height();

    if width <= MAX_TEXTURE_SIDE && height <= MAX_TEXTURE_SIDE {
        let image_buffer = image.to_rgba8();
        let size = [width as usize, height as usize];
        return vec![egui::ColorImage::from_rgba_unmultiplied(
            size,
            image_buffer.as_raw(),
        )];
    }

    let mut parts = Vec::new();
    if height > MAX_TEXTURE_SIDE {
        let mut current_y = 0;
        while current_y < height {
            let part_height = (height - current_y).min(MAX_TEXTURE_SIDE);
            let sub_image = image.crop_imm(0, current_y, width, part_height);
            let size = [width as usize, part_height as usize];
            parts.push(egui::ColorImage::from_rgba_unmultiplied(
                size,
                sub_image.to_rgba8().as_raw(),
            ));
            current_y += part_height;
        }
    } else if width > MAX_TEXTURE_SIDE {
        let mut current_x = 0;
        while current_x < width {
            let part_width = (width - current_x).min(MAX_TEXTURE_SIDE);
            let sub_image = image.crop_imm(current_x, 0, part_width, height);
            let size = [part_width as usize, height as usize];
            parts.push(egui::ColorImage::from_rgba_unmultiplied(
                size,
                sub_image.to_rgba8().as_raw(),
            ));
            current_x += part_width;
        }
    }

    parts
}

/// Кадр GIF-анимации
pub struct GifFrame {
    pub texture: egui::TextureHandle,
    pub delay_ms: u64,
}

/// Загружает кадры GIF как ColorImage
pub fn load_gif_color_images(data: &[u8]) -> Option<Vec<(egui::ColorImage, u64)>> {
    use image::AnimationDecoder;
    use std::io::Cursor;

    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(data)).ok()?;
    let frames = decoder.into_frames();

    let mut result = Vec::new();
    for frame_result in frames {
        let frame = frame_result.ok()?;
        let delay = frame.delay();
        let (num, den) = delay.numer_denom_ms();
        let delay_ms = (num as u64 * 1000) / den as u64;

        let buffer = frame.into_buffer();
        let size = [buffer.width() as usize, buffer.height() as usize];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, buffer.as_raw());
        result.push((color_image, delay_ms));
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}
