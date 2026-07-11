use std::collections::HashMap;
use egui::TextureHandle;
use image::DynamicImage;

/// Кэш текстур для отображения изображений
pub struct TextureCache {
    pub textures: HashMap<String, TextureHandle>,
}

impl TextureCache {
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }

    /// Загружает изображение из байтов и создает текстуру
    pub fn load_texture(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        data: &[u8],
    ) -> Option<TextureHandle> {
        if let Some(handle) = self.textures.get(key) {
            return Some(handle.clone());
        }

        let color_images = load_color_images_from_memory(data);
        if color_images.is_empty() {
            return None;
        }

        let color_image = color_images.into_iter().next()?;

        let texture = ctx.load_texture(
            key,
            color_image,
            egui::TextureOptions::LINEAR,
        );

        self.textures.insert(key.to_string(), texture.clone());
        Some(texture)
    }

    /// Создает текстуру из уже готового ColorImage
    pub fn load_from_color_image(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        color_image: egui::ColorImage,
    ) -> TextureHandle {
        let texture = ctx.load_texture(
            key,
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.textures.insert(key.to_string(), texture.clone());
        texture
    }

    /// Получает текстуру по ключу
    pub fn get(&self, key: &str) -> Option<TextureHandle> {
        self.textures.get(key).cloned()
    }

    /// Очищает кэш
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
    let Some(image) = load_image_from_memory(data) else { return vec![]; };

    let width = image.width();
    let height = image.height();

    if width <= MAX_TEXTURE_SIDE && height <= MAX_TEXTURE_SIDE {
        let image_buffer = image.to_rgba8();
        let size = [width as usize, height as usize];
        return vec![egui::ColorImage::from_rgba_unmultiplied(size, image_buffer.as_raw())];
    }

    let mut parts = Vec::new();
    if height > MAX_TEXTURE_SIDE {
        let mut current_y = 0;
        while current_y < height {
            let part_height = (height - current_y).min(MAX_TEXTURE_SIDE);
            let sub_image = image.clone().crop_imm(0, current_y, width, part_height);
            let size = [width as usize, part_height as usize];
            parts.push(egui::ColorImage::from_rgba_unmultiplied(size, sub_image.to_rgba8().as_raw()));
            current_y += part_height;
        }
    } else if width > MAX_TEXTURE_SIDE {
        let mut current_x = 0;
        while current_x < width {
            let part_width = (width - current_x).min(MAX_TEXTURE_SIDE);
            let sub_image = image.clone().crop_imm(current_x, 0, part_width, height);
            let size = [part_width as usize, height as usize];
            parts.push(egui::ColorImage::from_rgba_unmultiplied(size, sub_image.to_rgba8().as_raw()));
            current_x += part_width;
        }
    }

    parts
}

/// Загружает изображение из памяти
fn load_image_from_memory(data: &[u8]) -> Option<DynamicImage> {
    image::load_from_memory(data).ok()
}

/// Кадр GIF-анимации
pub struct GifFrame {
    pub texture: egui::TextureHandle,
    pub delay_ms: u64,
}

/// Загружает кадры GIF как ColorImage
pub fn load_gif_color_images(data: &[u8]) -> Option<Vec<(egui::ColorImage, u64)>> {
    use std::io::Cursor;
    use image::AnimationDecoder;

    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(data)).ok()?;
    let frames = decoder.into_frames();

    let mut result = Vec::new();
    for frame_result in frames {
        let frame = frame_result.ok()?;
        let delay = frame.delay();
        let (num, den) = delay.numer_denom_ms();
        let delay_ms = (num as u64 * 1000) / den as u64;

        let buffer = frame.into_buffer();
        let (width, height) = (buffer.width(), buffer.height());
        let size = [width as usize, height as usize];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, buffer.as_raw());

        result.push((color_image, delay_ms));
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Загружает все кадры GIF из памяти с созданием текстур
pub fn load_gif_frames(ctx: &egui::Context, data: &[u8]) -> Option<Vec<GifFrame>> {
    let color_images = load_gif_color_images(data)?;

    let mut gif_frames = Vec::new();
    for (color_image, delay_ms) in color_images {
        let texture = ctx.load_texture(
            format!("gif_frame_{}", gif_frames.len()),
            color_image,
            egui::TextureOptions::LINEAR,
        );

        gif_frames.push(GifFrame {
            texture,
            delay_ms,
        });
    }

    if gif_frames.is_empty() {
        None
    } else {
        Some(gif_frames)
    }
}


/// Масштабирует изображение для превью
pub fn scale_for_thumbnail(image: &DynamicImage, max_size: u32) -> DynamicImage {
    let (width, height) = (image.width(), image.height());

    if width <= max_size && height <= max_size {
        return image.clone();
    }

    let ratio = width as f32 / height as f32;
    let (new_width, new_height) = if width > height {
        (max_size, (max_size as f32 / ratio) as u32)
    } else {
        ((max_size as f32 * ratio) as u32, max_size)
    };

    image.resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
}

/// Загружает текстуру с масштабированием
pub fn load_thumbnail(
    ctx: &egui::Context,
    key: &str,
    data: &[u8],
    max_size: u32,
) -> Option<TextureHandle> {
    let image = load_image_from_memory(data)?;
    let thumbnail = scale_for_thumbnail(&image, max_size);

    let size = [thumbnail.width() as usize, thumbnail.height() as usize];
    let image_buffer = thumbnail.to_rgba8();
    let pixels = image_buffer.as_raw();

    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels);

    Some(ctx.load_texture(
        key,
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}
