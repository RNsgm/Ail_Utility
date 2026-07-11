use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use egui::{Context, Ui, Vec2, RichText, Color32, Rect, Sense, Key};
use crate::decoder::AilInfo;
use crate::viewer::texture_loader::{TextureCache, load_color_images_from_memory};

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

        // Запускаем загрузку в фоне с передачей Context для repaint
        let ctx = ctx.clone();
        let (tx, rx) = channel::<GalleryMessage>();
        self.load_receiver = Some(rx);

        std::thread::spawn(move || {
            let entries: Vec<_> = std::fs::read_dir(&path)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("ail"))
                        .unwrap_or(false)
                })
                .collect();

            for entry in entries {
                let path = entry.path();
                match AilInfo::from_path(path.clone()) {
                    Ok(info) => {
                        // Декодируем превью в пиксели прямо здесь, в фоне!
                        let color_images = info.preview_data.as_ref()
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
        // Берем receiver во временную переменную
        let receiver = self.load_receiver.take();
        
        if let Some(rx) = receiver {
            let mut received_count = 0;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    GalleryMessage::FileLoaded(info, color_image) => {
                        let texture_key = format!("thumb_{}", info.path.display());
                        
                        // Если есть готовые пиксели, создаем текстуру мгновенно
                        if let Some(ci) = color_image {
                            self.texture_cache.load_from_color_image(ctx, &texture_key, ci);
                        }
                        
                        self.ail_files.push(info);
                        received_count += 1;
                    }
                }
                if received_count >= 5 { break; } // Лимит на кадр для плавности
            }
            
            // Сортируем если получили новые файлы
            if received_count > 0 {
                self.ail_files.sort_by(|a, b| {
                    alphanumeric_sort::compare_str(&a.name, &b.name)
                });
            }
            
            // Возвращаем receiver обратно
            self.load_receiver = Some(rx);
        } else if self.loading {
            self.loading = false;
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selected_item.is_some()
    }

    pub fn selected_file(&self) -> Option<&AilInfo> {
        self.selected_item.and_then(|i| self.ail_files.get(i))
    }

    pub fn clear_selection(&mut self) {
        self.selected_item = None;
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> GalleryResponse {
        let mut response = GalleryResponse::default();

        // Обрабатываем сообщения от фонового потока
        self.process_load_messages(ctx);

        // Обработка скролла клавишами в библиотеке
        let scroll_delta = ctx.input(|i| {
            let mut delta = 0.0;
            if i.key_down(Key::ArrowDown) { delta -= 40.0; }
            if i.key_down(Key::ArrowUp) { delta += 40.0; }
            if i.key_pressed(Key::PageDown) { delta -= 600.0; }
            if i.key_pressed(Key::PageUp) { delta += 600.0; }
            delta
        });

        if scroll_delta != 0.0 {
            ui.scroll_with_delta(egui::vec2(0.0, scroll_delta));
        }

        ui.horizontal(|ui| {
            ui.heading("Gallery");
            ui.separator();

            if ui.button("📁 Open Folder").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.set_folder(ctx, folder);
                }
            }

            if let Some(folder) = &self.current_folder {
                ui.label(format!("📂 {}", folder.display()));
                ui.separator();
                
                if self.loading {
                    ui.label(format!("Loading... {} found", self.ail_files.len()));
                } else {
                    ui.label(format!("{} files", self.ail_files.len()));
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.thumbnail_size, 100.0..=400.0).text("Size"));
            });
        });

        ui.separator();

        if self.ail_files.is_empty() && !self.loading {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                if self.current_folder.is_none() {
                    ui.label(RichText::new("Select folder with .ail files").size(20.0));
                } else {
                    ui.label(RichText::new("No .ail files in folder").size(20.0));
                }
            });
        } else if self.loading && self.ail_files.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.spinner();
                ui.label("Scanning files...");
            });
        } else {
            self.render_grid(ctx, ui, &mut response);
        }

        response
    }

    fn render_grid(&mut self, ctx: &Context, ui: &mut Ui, response: &mut GalleryResponse) {
        let available_width = ui.available_width();
        let spacing = 16.0;
        let item_width = self.thumbnail_size;
        let columns = ((available_width + spacing) / (item_width + spacing)) as usize;
        let columns = columns.max(1);
        let rows = (self.ail_files.len() + columns - 1) / columns;

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show_rows(ui, item_width + spacing, rows, |ui, row_range| {
                egui::Grid::new("gallery_grid")
                    .num_columns(columns)
                    .spacing([spacing, spacing])
                    .show(ui, |ui| {
                        for row in row_range {
                            for col in 0..columns {
                                let index = row * columns + col;
                                if index >= self.ail_files.len() {
                                    break;
                                }

                                let (path, name, count, preview) = {
                                    let file = &self.ail_files[index];
                                    (file.path.clone(), file.name.clone(), file.image_count, file.preview_data.clone())
                                };

                                let clicked = self.render_thumbnail(
                                    ctx, 
                                    ui, 
                                    index, 
                                    &path, 
                                    &name, 
                                    count, 
                                    preview.as_deref()
                                );

                                if clicked {
                                    self.selected_item = Some(index);
                                    response.clicked_index = Some(index);
                                    response.clicked_file = Some(path);
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn render_thumbnail(
        &mut self,
        ctx: &Context,
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
        
        let bg_color = if is_selected {
            Color32::from_rgb(100, 150, 200)
        } else if response.hovered() {
            Color32::from_rgb(60, 60, 70)
        } else {
            Color32::from_rgb(40, 40, 45)
        };

        ui.painter().rect_filled(rect, 8.0, bg_color);

        if is_selected {
            ui.painter().rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(3.0, Color32::from_rgb(100, 200, 255)),
                egui::StrokeKind::Middle,
            );
        }

        let inner_rect = rect.shrink(8.0);
        if let Some(ref texture) = loaded_texture {
            let img_size = texture.size_vec2();
            let scaled_size = fit_size(img_size, inner_rect.size());
            let img_rect = Rect::from_center_size(inner_rect.center(), scaled_size);
            
            ui.painter().image(
                texture.id(),
                img_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let center = inner_rect.center();
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "",
                egui::FontId::proportional(self.thumbnail_size * 0.3),
                Color32::from_gray(100),
            );
        }

        let name_rect = Rect::from_min_max(
            rect.left_bottom() - egui::vec2(0.0, 24.0),
            rect.right_bottom(),
        );
        ui.painter().rect_filled(name_rect, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 180));
        
        // Безопасная обрезка Unicode строки
        let display_name = if name.chars().count() > 25 {
            let truncated: String = name.chars().take(22).collect();
            format!("{}...", truncated)
        } else {
            name.to_string()
        };
        
        ui.painter().text(
            name_rect.center(),
            egui::Align2::CENTER_CENTER,
            display_name,
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );

        let count_text = format!("{} ", count);
        ui.painter().text(
            rect.right_top() + egui::vec2(-8.0, 8.0),
            egui::Align2::RIGHT_TOP,
            count_text,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(200, 200, 200),
        );

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