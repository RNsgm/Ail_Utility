use crate::viewer::{EncoderView, GalleryView, ReaderView};

/// Основное состояние приложения
pub struct ViewerApp {
    gallery: GalleryView,
    reader: ReaderView,
    encoder: EncoderView,
    mode: AppMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AppMode {
    Gallery,
    Reader,
    Encoder,
}

impl ViewerApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            gallery: GalleryView::new(),
            reader: ReaderView::new(),
            encoder: EncoderView::new(),
            mode: AppMode::Gallery,
        }
    }

    pub fn run() -> eframe::Result {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 720.0])
                .with_min_inner_size([800.0, 600.0]),
            ..Default::default()
        };

        eframe::run_native(
            "AIL Viewer",
            options,
            Box::new(|cc| Ok(Box::new(ViewerApp::new(cc)))),
        )
    }
}

impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::Panel::left("nav_panel")
            .resizable(false)
            .default_width(160.0)
            .show(ctx, |ui| {
                ui.heading("AIL Tool");
                ui.add_space(10.0);

                if ui
                    .selectable_label(self.mode == AppMode::Gallery, "🖼 Галерея")
                    .clicked()
                {
                    self.mode = AppMode::Gallery;
                }
                if ui
                    .selectable_label(self.mode == AppMode::Encoder, "⚙ Кодировщик")
                    .clicked()
                {
                    self.mode = AppMode::Encoder;
                }
            });

        match self.mode {
            AppMode::Gallery => {
                egui::CentralPanel::default().show_inside(ctx, |ui| {
                    let response = self.gallery.ui(ctx, ui);

                    if let Some(path) = response.clicked_file {
                        self.reader.open_file(ctx, path);
                        self.mode = AppMode::Reader;
                    }
                });
            }
            AppMode::Reader => {
                egui::CentralPanel::default().show_inside(ctx, |ui| {
                    let response = self.reader.ui(ctx, ui);

                    if response.close_requested {
                        self.reader.close();
                        self.gallery.clear_selection();
                        self.mode = AppMode::Gallery;
                    }
                });
            }
            AppMode::Encoder => {
                egui::CentralPanel::default().show_inside(ctx, |ui| {
                    self.encoder.ui(ctx, ui);
                });
            }
        }
    }
}

/// Запускает приложение просмотрщика
pub fn run_viewer() -> eframe::Result {
    ViewerApp::run()
}
