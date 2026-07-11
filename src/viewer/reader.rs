use crate::decoder::AilDecoder;
use crate::encoder::MediaType;
use crate::viewer::texture_loader::{
    load_color_images_from_memory, load_gif_color_images, GifFrame, TextureCache,
};
use crate::viewer::thread_util::spawn_worker;
use crate::viewer::video_player::VideoPlayer;
use egui::{Context, Key, RichText, Ui, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

#[derive(Debug, Clone, PartialEq)]
enum LoadMessage {
    TotalPages(usize),
    PageData(usize, Vec<egui::ColorImage>),
    GifFrames(Vec<(egui::ColorImage, u64)>),
    VideoTempPath(PathBuf),
    EntryNames(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReaderMode {
    FitWidth,
    FitHeight,
    FitWindow,
    Original,
    Zoom(f32),
}

impl Default for ReaderMode {
    fn default() -> Self {
        ReaderMode::FitWindow
    }
}

pub struct ReaderView {
    current_file: Option<PathBuf>,
    decoder: Option<AilDecoder>,
    current_page: usize,
    texture_cache: TextureCache,
    mode: ReaderMode,
    zoom_level: f32,
    show_ui: bool,
    load_receiver: Option<Receiver<LoadMessage>>,
    loaded_pages: std::collections::HashMap<usize, Vec<egui::TextureHandle>>,
    total_pages: usize,
    load_sender: Option<std::sync::mpsc::Sender<usize>>,
    requested_pages: std::collections::HashSet<usize>,
    last_queue_update_page: usize,
    gif_frames: Option<Vec<GifFrame>>,
    gif_current_frame: usize,
    gif_last_frame_time: std::time::Instant,
    gif_looping: bool,
    video_player: Option<VideoPlayer>,
    current_media_type: MediaType,
    entry_names: Vec<String>,
}

impl ReaderView {
    pub fn new() -> Self {
        Self {
            current_file: None,
            decoder: None,
            current_page: 0,
            texture_cache: TextureCache::new(),
            mode: ReaderMode::FitWindow,
            zoom_level: 1.0,
            show_ui: true,
            load_receiver: None,
            loaded_pages: std::collections::HashMap::new(),
            total_pages: 0,
            load_sender: None,
            requested_pages: std::collections::HashSet::new(),
            last_queue_update_page: 999999,
            gif_frames: None,
            gif_current_frame: 0,
            gif_last_frame_time: std::time::Instant::now(),
            gif_looping: true,
            video_player: None,
            current_media_type: MediaType::Image,
            entry_names: Vec::new(),
        }
    }

    pub fn open_file(&mut self, ctx: &Context, path: PathBuf) {
        self.current_file = Some(path.clone());
        self.current_page = 0;
        self.texture_cache.clear();
        self.loaded_pages.clear();
        self.requested_pages.clear();
        self.total_pages = 0;
        self.last_queue_update_page = 999999;
        self.clear_media_state();
        self.gif_last_frame_time = std::time::Instant::now();
        self.current_media_type = MediaType::Image;
        self.entry_names.clear();

        let (tx, rx) = channel();
        self.load_receiver = Some(rx);

        let (req_tx, req_rx) = std::sync::mpsc::channel::<usize>();
        self.load_sender = Some(req_tx);

        let ctx_clone = ctx.clone();
        let path_clone = path.clone();

        spawn_worker("reader_load", move || {
            let mut decoder = match AilDecoder::open(&path_clone) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[Background] Failed to open decoder: {}", e);
                    return;
                }
            };

            let count = decoder.len();
            let names: Vec<String> = decoder
                .entries()
                .iter()
                .map(|e| e.original_name.clone())
                .collect();
            let _ = tx.send(LoadMessage::TotalPages(count));
            let _ = tx.send(LoadMessage::EntryNames(names));
            ctx_clone.request_repaint();

            while let Ok(page_idx) = req_rx.recv() {
                if page_idx >= count {
                    continue;
                }

                if let Ok(data) = decoder.read_image(page_idx) {
                    let entry = decoder.get_entry(page_idx);
                    let name = entry.map(|e| e.original_name.as_str()).unwrap_or("");
                    let media_type = MediaType::from_path(name);

                    if entry
                        .map(|e| e.entry_type == crate::encoder::EntryType::Video)
                        .unwrap_or(false)
                    {
                        let ext = crate::encoder::video_container_extension(name);
                        let temp_name = format!(
                            "archive_utils_video_{}_{}.{ext}",
                            std::process::id(),
                            page_idx
                        );
                        let temp_path = std::env::temp_dir().join(&temp_name);
                        if std::fs::write(&temp_path, &data).is_ok() {
                            let _ = tx.send(LoadMessage::VideoTempPath(temp_path));
                            ctx_clone.request_repaint();
                        }
                    } else if media_type.is_gif() {
                        if let Some(frames) = load_gif_color_images(&data) {
                            let _ = tx.send(LoadMessage::GifFrames(frames));
                            ctx_clone.request_repaint();
                        }
                    } else {
                        let color_images = load_color_images_from_memory(&data);
                        if !color_images.is_empty() {
                            if tx
                                .send(LoadMessage::PageData(page_idx, color_images))
                                .is_err()
                            {
                                break;
                            }
                            ctx_clone.request_repaint();
                        }
                    }
                }
            }
        });

        self.update_loading_queue(true);
    }

    fn update_loading_queue(&mut self, force: bool) {
        let Some(tx) = &self.load_sender else { return };
        let current = self.current_page;
        let total = self.total_pages;
        if total == 0 {
            return;
        }

        let diff = if current > self.last_queue_update_page {
            current - self.last_queue_update_page
        } else {
            self.last_queue_update_page - current
        };
        if !force && diff < 5 {
            return;
        }
        self.last_queue_update_page = current;

        let range = 20;
        let mut pages_to_load = vec![current];
        for i in 1..=range {
            if current + i < total {
                pages_to_load.push(current + i);
            }
            if current >= i {
                pages_to_load.push(current - i);
            }
        }

        for page in pages_to_load {
            if !self.loaded_pages.contains_key(&page) && !self.requested_pages.contains(&page) {
                if tx.send(page).is_ok() {
                    self.requested_pages.insert(page);
                }
            }
        }

        let keep_range = 25;
        self.loaded_pages.retain(|&idx, _| {
            let diff = if idx > current {
                idx - current
            } else {
                current - idx
            };
            diff <= keep_range
        });
        self.requested_pages.retain(|&idx| {
            let diff = if idx > current {
                idx - current
            } else {
                current - idx
            };
            diff <= range + 5
        });
    }

    pub fn close(&mut self) {
        self.current_file = None;
        self.decoder = None;
        self.current_page = 0;
        self.texture_cache.clear();
        self.loaded_pages.clear();
        self.load_receiver = None;
        self.clear_media_state();
        self.entry_names.clear();
    }

    pub fn next_page(&mut self) {
        if self.current_page + 1 < self.total_pages {
            self.clear_media_state();
            self.current_page += 1;
            self.update_loading_queue(false);
        }
    }

    pub fn prev_page(&mut self) {
        if self.current_page > 0 {
            self.clear_media_state();
            self.current_page -= 1;
            self.update_loading_queue(false);
        }
    }

    pub fn go_to_page(&mut self, page: usize) {
        if page < self.total_pages && page != self.current_page {
            self.clear_media_state();
            self.current_page = page;
            self.update_loading_queue(false);
        }
    }

    fn clear_media_state(&mut self) {
        self.video_player = None;
        self.gif_frames = None;
        self.gif_current_frame = 0;
    }

    pub fn ui(&mut self, ctx: &Context, _ui: &mut Ui) -> ReaderResponse {
        let mut response = ReaderResponse::default();
        let mut should_update_queue = false;

        if let Some(rx) = &self.load_receiver {
            let mut processed = 0;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    LoadMessage::TotalPages(count) => {
                        self.total_pages = count;
                        should_update_queue = true;
                    }
                    LoadMessage::EntryNames(names) => {
                        self.entry_names = names;
                    }
                    LoadMessage::PageData(index, color_images) => {
                        let mut textures = Vec::new();
                        for (part_idx, color_image) in color_images.into_iter().enumerate() {
                            let texture_key = format!("page_{}_part_{}", index, part_idx);
                            let tex = ctx.load_texture(
                                texture_key,
                                color_image,
                                egui::TextureOptions::LINEAR,
                            );
                            textures.push(tex);
                        }
                        self.loaded_pages.insert(index, textures);
                        processed += 1;
                    }
                    LoadMessage::GifFrames(frames) => {
                        let mut gif_frames = Vec::new();
                        for (color_image, delay_ms) in frames {
                            let texture = ctx.load_texture(
                                format!("gif_frame_{}", gif_frames.len()),
                                color_image,
                                egui::TextureOptions::LINEAR,
                            );
                            gif_frames.push(GifFrame { texture, delay_ms });
                        }
                        self.gif_frames = Some(gif_frames);
                        self.gif_current_frame = 0;
                        self.gif_last_frame_time = std::time::Instant::now();
                    }
                    LoadMessage::VideoTempPath(path) => {
                        self.video_player = Some(VideoPlayer::new(ctx, path));
                    }
                }
                if processed >= 2 {
                    break;
                }
            }
        }

        if should_update_queue {
            self.update_loading_queue(true);
        }

        self.handle_input(ctx, &mut response);

        if self.total_pages == 0 && self.load_receiver.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Нет открытого файла").size(24.0));
                });
            });
            return response;
        }

        let total_pages = self.total_pages;
        let title = self
            .current_file
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        if self.show_ui {
            egui::Panel::top("reader_top").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("<- Назад").clicked() {
                        response.close_requested = true;
                    }

                    ui.separator();
                    ui.label(format!("{} / {}", self.current_page + 1, total_pages));
                    ui.separator();
                    ui.label(&title);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::ComboBox::from_label("")
                            .selected_text(format!("{:?}", self.mode))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.mode,
                                    ReaderMode::FitWindow,
                                    "По окну",
                                );
                                ui.selectable_value(
                                    &mut self.mode,
                                    ReaderMode::FitWidth,
                                    "По ширине",
                                );
                                ui.selectable_value(
                                    &mut self.mode,
                                    ReaderMode::FitHeight,
                                    "По высоте",
                                );
                                ui.selectable_value(
                                    &mut self.mode,
                                    ReaderMode::Original,
                                    "Оригинал",
                                );
                            });

                        if ui.button("⏭").clicked() {
                            self.next_page();
                        }
                        if ui.button("⏮").clicked() {
                            self.prev_page();
                        }
                    });
                });
            });

            egui::Panel::bottom("reader_bottom").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Страница:");
                    let mut page = self.current_page + 1;
                    if ui
                        .add(egui::DragValue::new(&mut page).range(1..=total_pages))
                        .changed()
                    {
                        self.go_to_page(page.saturating_sub(1));
                    }
                    ui.label(format!("/ {}", total_pages));

                    ui.add_space(20.0);
                    let mut progress = self.current_page as f32;
                    if ui
                        .add(
                            egui::Slider::new(
                                &mut progress,
                                0.0..=(total_pages.saturating_sub(1)) as f32,
                            )
                            .show_value(false),
                        )
                        .changed()
                    {
                        self.go_to_page(progress as usize);
                    }
                });
            });
        }

        let current_page = self.current_page;
        let mode = self.mode;
        let zoom = self.zoom_level;

        egui::CentralPanel::default().show(ctx, |ui| {
            let scroll_delta = ctx.input(|i| {
                let mut delta = 0.0;
                if i.key_down(Key::ArrowDown) {
                    delta -= 20.0;
                }
                if i.key_down(Key::ArrowUp) {
                    delta += 20.0;
                }
                if i.key_pressed(Key::PageDown) {
                    delta -= 500.0;
                }
                if i.key_pressed(Key::PageUp) {
                    delta += 500.0;
                }
                delta
            });

            if scroll_delta != 0.0 {
                ui.scroll_with_delta(egui::vec2(0.0, scroll_delta));
            }

            self.render_media(ctx, ui, current_page, mode, zoom);
        });

        response
    }

    fn render_media(
        &mut self,
        ctx: &Context,
        ui: &mut Ui,
        page: usize,
        mode: ReaderMode,
        zoom_level: f32,
    ) {
        let media_type = if page < self.entry_names.len() {
            MediaType::from_path(&self.entry_names[page])
        } else {
            MediaType::Image
        };

        match media_type {
            MediaType::Gif => {
                self.render_gif(ctx, ui, mode, zoom_level);
            }
            MediaType::Video => {
                self.render_video(ctx, ui, mode, zoom_level);
            }
            _ => {
                self.render_image(ui, page, mode, zoom_level);
            }
        }
    }

    fn render_gif(&mut self, ctx: &Context, ui: &mut Ui, mode: ReaderMode, zoom_level: f32) {
        let Some(ref frames) = self.gif_frames else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label("Загрузка GIF...");
            });
            return;
        };

        if frames.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Пустой GIF").size(18.0));
            });
            return;
        }

        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.gif_last_frame_time).as_millis() as u64;
        let current_frame = &frames[self.gif_current_frame];
        if elapsed >= current_frame.delay_ms.max(20) {
            let next = self.gif_current_frame + 1;
            if next >= frames.len() {
                if self.gif_looping {
                    self.gif_current_frame = 0;
                }
                // Если не looping, остаёмся на последнем кадре
            } else {
                self.gif_current_frame = next;
            }
            self.gif_last_frame_time = now;
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(
                (current_frame.delay_ms - elapsed).min(100),
            ));
        }

        let tex = &frames[self.gif_current_frame].texture;
        let img_size = tex.size_vec2();

        let controls_height = 36.0;
        let available_size = ui.available_size();
        let video_height = available_size.y - controls_height;

        let display_scale = match mode {
            ReaderMode::FitWindow => {
                let ratio_x = available_size.x / img_size.x;
                let ratio_y = video_height / img_size.y;
                ratio_x.min(ratio_y).min(1.0)
            }
            ReaderMode::FitWidth => (available_size.x / img_size.x).min(1.0),
            ReaderMode::FitHeight => (video_height / img_size.y).min(1.0),
            ReaderMode::Original => 1.0,
            ReaderMode::Zoom(z) => z,
        } * zoom_level;

        let final_size = img_size * display_scale;
        let x_offset = (available_size.x - final_size.x).max(0.0) / 2.0;
        let y_offset = (video_height - final_size.y).max(0.0) / 2.0;

        ui.vertical(|ui| {
            ui.add_space(y_offset);
            ui.horizontal(|ui| {
                ui.add_space(x_offset);
                ui.add(egui::Image::new(tex).max_size(final_size));
            });
        });

        // GIF controls
        ui.horizontal(|ui| {
            if ui
                .button(if self.gif_looping { "🔁" } else { "➡" })
                .on_hover_text("Loop")
                .clicked()
            {
                self.gif_looping = !self.gif_looping;
            }
            ui.label(format!(
                "Кадр {} / {}",
                self.gif_current_frame + 1,
                frames.len()
            ));
        });
    }

    fn render_video(&mut self, ctx: &Context, ui: &mut Ui, mode: ReaderMode, zoom_level: f32) {
        let Some(ref mut player) = self.video_player else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label("Загрузка видео...");
            });
            return;
        };

        let texture = player.update(ctx);
        let Some(tex) = texture else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label("Воспроизведение видео...");
            });
            return;
        };

        let img_size = tex.size_vec2();
        let available_size = ui.available_size();
        let controls_height = 40.0;
        let video_height = available_size.y - controls_height;

        let display_scale = match mode {
            ReaderMode::FitWindow => {
                let ratio_x = available_size.x / img_size.x;
                let ratio_y = video_height / img_size.y;
                ratio_x.min(ratio_y).min(1.0)
            }
            ReaderMode::FitWidth => (available_size.x / img_size.x).min(1.0),
            ReaderMode::FitHeight => (video_height / img_size.y).min(1.0),
            ReaderMode::Original => 1.0,
            ReaderMode::Zoom(z) => z,
        } * zoom_level;

        let final_size = img_size * display_scale;
        let x_offset = (available_size.x - final_size.x).max(0.0) / 2.0;
        let y_offset = (video_height - final_size.y).max(0.0) / 2.0;

        ui.vertical(|ui| {
            ui.add_space(y_offset);
            ui.horizontal(|ui| {
                ui.add_space(x_offset);
                ui.add(egui::Image::new(tex).max_size(final_size));
            });
        });

        // Controls
        let elapsed = player.elapsed().clamp(0.0, player.duration);
        let mut seek_target = None;

        ui.horizontal(|ui| {
            if ui
                .button(if player.is_paused { "▶" } else { "⏸" })
                .clicked()
            {
                player.toggle_pause();
            }

            if ui
                .button(if player.looping { "🔁" } else { "➡" })
                .on_hover_text("Loop")
                .clicked()
            {
                player.looping = !player.looping;
            }

            ui.label(format!(
                "{} / {}",
                fmt_time(elapsed),
                fmt_time(player.duration)
            ));

            let slider_width = ui.available_width() - 20.0;
            let mut slider_val = elapsed;
            let resp = ui.add_sized(
                [slider_width.max(50.0), 20.0],
                egui::Slider::new(&mut slider_val, 0.0..=player.duration)
                    .show_value(false)
                    .trailing_fill(true),
            );
            if resp.changed() {
                seek_target = Some(slider_val);
            }
        });

        if let Some(target) = seek_target {
            player.seek_to(ctx, target);
        }
    }

    fn render_image(&mut self, ui: &mut Ui, page: usize, mode: ReaderMode, zoom_level: f32) {
        let textures = self.loaded_pages.get(&page);

        let Some(textures) = textures else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label(format!("Загрузка страницы {}...", page + 1));
            });
            return;
        };

        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let available_size = ui.available_size();

                let mut total_img_size = Vec2::ZERO;
                for tex in textures {
                    let size = tex.size_vec2();
                    total_img_size.x = total_img_size.x.max(size.x);
                    total_img_size.y += size.y;
                }

                let display_scale = match mode {
                    ReaderMode::FitWindow => {
                        let ratio_x = available_size.x / total_img_size.x;
                        let ratio_y = available_size.y / total_img_size.y;
                        ratio_x.min(ratio_y).min(1.0)
                    }
                    ReaderMode::FitWidth => (available_size.x / total_img_size.x).min(1.0),
                    ReaderMode::FitHeight => (available_size.y / total_img_size.y).min(1.0),
                    ReaderMode::Original => 1.0,
                    ReaderMode::Zoom(z) => z,
                } * zoom_level;

                let final_size = total_img_size * display_scale;
                let x_offset = (available_size.x - final_size.x).max(0.0) / 2.0;
                let y_offset = (available_size.y - final_size.y).max(0.0) / 2.0;

                ui.vertical(|ui| {
                    ui.add_space(y_offset);
                    ui.horizontal(|ui| {
                        ui.add_space(x_offset);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for tex in textures {
                                let part_display_size = tex.size_vec2() * display_scale;
                                ui.add(egui::Image::new(tex).max_size(part_display_size));
                            }
                        });
                    });
                });
            });
    }

    fn handle_input(&mut self, ctx: &Context, response: &mut ReaderResponse) {
        if ctx.input(|i| i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::D)) {
            self.next_page();
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::A)) {
            self.prev_page();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            response.close_requested = true;
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            response.toggle_fullscreen = true;
        }
        if ctx.input(|i| i.key_pressed(Key::H)) {
            self.show_ui = !self.show_ui;
        }
        if ctx.input(|i| i.key_pressed(Key::Num0)) {
            self.mode = ReaderMode::Original;
        }
        if ctx.input(|i| i.key_pressed(Key::Equals)) {
            self.zoom_level = (self.zoom_level * 1.2).min(5.0);
            self.mode = ReaderMode::Zoom(self.zoom_level);
        }
        if ctx.input(|i| i.key_pressed(Key::Minus)) {
            self.zoom_level = (self.zoom_level / 1.2).max(0.1);
            self.mode = ReaderMode::Zoom(self.zoom_level);
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            self.go_to_page(0);
        }
        if ctx.input(|i| i.key_pressed(Key::End)) {
            self.go_to_page(self.total_pages.saturating_sub(1));
        }
        if ctx.input(|i| i.key_pressed(Key::OpenBracket)) {
            self.mode = ReaderMode::FitWidth;
        }
        if ctx.input(|i| i.key_pressed(Key::CloseBracket)) {
            self.mode = ReaderMode::FitWindow;
        }
    }
}

impl Default for ReaderView {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct ReaderResponse {
    pub close_requested: bool,
    pub toggle_fullscreen: bool,
}

fn fmt_time(seconds: f64) -> String {
    let secs = seconds as u64;
    let mins = secs / 60;
    let hrs = mins / 60;
    let rem = secs % 60;
    if hrs > 0 {
        format!("{:02}:{:02}:{:02}", hrs, mins % 60, rem)
    } else {
        format!("{:02}:{:02}", mins, rem)
    }
}
