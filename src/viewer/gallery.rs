use crate::decoder::AilInfo;
use crate::viewer::texture_loader::{load_color_images_from_memory, TextureCache};
use crate::viewer::theme::{self, THUMB_RADIUS};
use crate::viewer::thread_util::spawn_worker;
use egui::{Context, Rect, RichText, Sense, Ui, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

pub struct GalleryView {
    current_folder: Option<PathBuf>,
    ail_files: Vec<AilInfo>,
    texture_cache: TextureCache,
    selected_item: Option<usize>,
    thumbnail_size: f32,
    loading: bool,
    load_receiver: Option<Receiver<GalleryMessage>>,
}

enum GalleryMessage {
    FileLoaded(AilInfo, Option<egui::ColorImage>),
}

impl GalleryView {
    pub fn new() -> Self {
        Self {
            current_folder: None,
            ail_files: Vec::new(),
            texture_cache: TextureCache::new(),
            selected_item: None,
            thumbnail_size: 200.0,
            loading: false,
            load_receiver: None,
        }
    }

    pub fn set_folder(&mut self, ctx: &Context, path: PathBuf) {
        self.current_folder = Some(path.clone());
        self.loading = true;
        self.ail_files.clear();
        self.texture_cache.clear();
        self.selected_item = None;

        let ctx = ctx.clone();
        let (tx, rx) = channel::<GalleryMessage>();
        self.load_receiver = Some(rx);

        spawn_worker("gallery_load", move || {
            let entries: Vec<_> = std::fs::read_dir(&path)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("ail"))
                        .unwrap_or(false)
                })
                .collect();

            for entry in entries {
                let path = entry.path();
                match AilInfo::from_path(path.clone()) {
                    Ok(info) => {
                        let color_images = info
                            .preview_data
                            .as_ref()
                            .map(|data| load_color_images_from_memory(data))
                            .unwrap_or_default();

                        let color_image = color_images.into_iter().next();

                        let _ = tx.send(GalleryMessage::FileLoaded(info, color_image));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        eprintln!("Failed to load {:?}: {}", path, e);
                    }
                }
            }
        });
    }

    fn process_load_messages(&mut self, ctx: &Context) {
        let receiver = self.load_receiver.take();

        if let Some(rx) = receiver {
            let mut received_count = 0;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    GalleryMessage::FileLoaded(info, color_image) => {
                        let texture_key = format!("thumb_{}", info.path.display());

                        if let Some(ci) = color_image {
                            let _ = self
                                .texture_cache
                                .load_from_color_image(ctx, &texture_key, ci);
                        }

                        self.ail_files.push(info);
                        received_count += 1;
                    }
                }
                if received_count >= 5 {
                    break;
                }
            }

            if received_count > 0 {
                self.ail_files
                    .sort_by(|a, b| alphanumeric_sort::compare_str(&a.name, &b.name));
            }

            self.load_receiver = Some(rx);
        } else if self.loading {
            self.loading = false;
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected_item = None;
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> GalleryResponse {
        let mut response = GalleryResponse::default();

        self.process_load_messages(ctx);

        ui.vertical(|ui| {
            self.render_toolbar(ctx, ui);
            ui.add_space(8.0);

            let scroll_height = ui.available_height();
            egui::ScrollArea::vertical()
                .id_salt("gallery_main_scroll")
                .auto_shrink([false; 2])
                .max_height(scroll_height.max(100.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(theme::CONTENT_PAD);
                        ui.vertical(|ui| {
                            let width = ui.available_width();
                            ui.set_max_width(width);

                            if self.ail_files.is_empty() && !self.loading {
                                if self.current_folder.is_none() {
                                    theme::empty_state(
                                        ui,
                                        "Выберите папку",
                                        "Откройте каталог с .ail файлами для просмотра",
                                    );
                                } else {
                                    theme::empty_state(
                                        ui,
                                        "Нет .ail файлов",
                                        "В этой папке нет архивов AIL",
                                    );
                                }
                            } else if self.loading && self.ail_files.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(100.0);
                                    ui.spinner();
                                    ui.label(
                                        RichText::new("Сканирование…").color(theme::text_muted()),
                                    );
                                });
                            } else {
                                self.render_grid(ctx, ui, &mut response);
                            }
                        });
                        ui.add_space(theme::CONTENT_PAD);
                    });
                    ui.add_space(16.0);
                });
        });

        response
    }

    fn render_toolbar(&mut self, ctx: &Context, ui: &mut Ui) {
        theme::toolbar_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(theme::CONTENT_PAD);
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Галерея")
                                .size(18.0)
                                .strong()
                                .color(theme::text()),
                        );
                        ui.add_space(8.0);

                        if ui.button("📁 Открыть папку").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.set_folder(ctx, folder);
                            }
                        }

                        if let Some(folder) = &self.current_folder {
                            ui.label(
                                RichText::new(format!("📂 {}", folder.display()))
                                    .color(theme::text_muted()),
                            );

                            let status = if self.loading {
                                format!("Загрузка… {} найдено", self.ail_files.len())
                            } else {
                                format!("{} файлов", self.ail_files.len())
                            };
                            ui.label(RichText::new(status).color(theme::text_muted()));
                        }
                    });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Размер превью").color(theme::text_muted()));
                        ui.add(
                            egui::Slider::new(&mut self.thumbnail_size, 100.0..=400.0)
                                .show_value(true),
                        );
                    });
                });
                ui.add_space(theme::CONTENT_PAD);
            });
        });
    }

    fn render_grid(&mut self, ctx: &Context, ui: &mut Ui, response: &mut GalleryResponse) {
        let spacing = 16.0;
        let item_size = self.thumbnail_size;
        let available_width = ui.available_width();
        if available_width <= 0.0 {
            return;
        }

        let columns =
            ((available_width + spacing) / (item_size + spacing)).floor() as usize;
        let columns = columns.max(1);

        let mut index = 0;
        let total = self.ail_files.len();

        while index < total {
            ui.horizontal(|ui| {
                for col in 0..columns {
                    if index >= total {
                        break;
                    }

                    let (path, name, count, preview) = {
                        let file = &self.ail_files[index];
                        (
                            file.path.clone(),
                            file.name.clone(),
                            file.image_count,
                            file.preview_data.clone(),
                        )
                    };

                    let clicked = self.render_thumbnail(
                        ctx,
                        ui,
                        index,
                        &path,
                        &name,
                        count,
                        preview.as_deref(),
                    );

                    if clicked {
                        self.selected_item = Some(index);
                        response.clicked_index = Some(index);
                        response.clicked_file = Some(path);
                    }

                    index += 1;
                    if col + 1 < columns && index < total {
                        ui.add_space(spacing);
                    }
                }
            });
            if index < total {
                ui.add_space(spacing);
            }
        }
    }

    fn render_thumbnail(
        &mut self,
        _ctx: &Context,
        ui: &mut Ui,
        index: usize,
        path: &PathBuf,
        name: &str,
        count: usize,
        _preview: Option<&[u8]>,
    ) -> bool {
        let is_selected = self.selected_item == Some(index);
        let size = Vec2::splat(self.thumbnail_size);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());

        let texture_key = format!("thumb_{}", path.display());
        let loaded_texture = self.texture_cache.get(&texture_key);

        let (bg_color, stroke) = theme::thumb_colors(is_selected, response.hovered());
        ui.painter().rect_filled(rect, THUMB_RADIUS, bg_color);

        if let Some(stroke) = stroke {
            ui.painter().rect_stroke(
                rect,
                THUMB_RADIUS,
                stroke,
                egui::StrokeKind::Inside,
            );
        }

        let inner_rect = rect.shrink(10.0);
        if let Some(ref texture) = loaded_texture {
            let img_size = texture.size_vec2();
            let scaled_size = fit_size(img_size, inner_rect.size());
            let img_rect = Rect::from_center_size(inner_rect.center(), scaled_size);

            ui.painter().image(
                texture.id(),
                img_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            ui.painter().text(
                inner_rect.center(),
                egui::Align2::CENTER_CENTER,
                "AIL",
                egui::FontId::proportional(self.thumbnail_size * 0.14),
                theme::text_muted(),
            );
        }

        let name_rect = Rect::from_min_max(
            rect.left_bottom() - egui::vec2(0.0, 26.0),
            rect.right_bottom(),
        );
        ui.painter().rect_filled(
            name_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(8, 10, 14, 200),
        );

        let display_name = if name.chars().count() > 25 {
            let truncated: String = name.chars().take(22).collect();
            format!("{truncated}…")
        } else {
            name.to_string()
        };

        ui.painter().text(
            name_rect.center(),
            egui::Align2::CENTER_CENTER,
            display_name,
            egui::FontId::proportional(12.0),
            theme::text(),
        );

        if count > 1 {
            let badge_rect = Rect::from_min_size(
                rect.right_top() + egui::vec2(-36.0, 8.0),
                Vec2::new(28.0, 18.0),
            );
            ui.painter()
                .rect_filled(badge_rect, 6.0, theme::accent_bg());
            ui.painter().text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                count.to_string(),
                egui::FontId::proportional(11.0),
                theme::text(),
            );
        }

        response.clicked()
    }
}

impl Default for GalleryView {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct GalleryResponse {
    pub clicked_index: Option<usize>,
    pub clicked_file: Option<PathBuf>,
}

fn fit_size(image_size: Vec2, container_size: Vec2) -> Vec2 {
    let ratio_x = container_size.x / image_size.x;
    let ratio_y = container_size.y / image_size.y;
    let ratio = ratio_x.min(ratio_y).min(1.0);

    Vec2::new(image_size.x * ratio, image_size.y * ratio)
}
