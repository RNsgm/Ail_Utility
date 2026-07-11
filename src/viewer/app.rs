use crate::viewer::{ConverterView, EncoderView, GalleryView, ReaderView};
use crate::viewer::theme::{self, NAV_WIDTH};

/// Основное состояние приложения
pub struct ViewerApp {
    gallery: GalleryView,
    reader: ReaderView,
    encoder: EncoderView,
    converter: ConverterView,
    mode: AppMode,
    fullscreen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AppMode {
    Gallery,
    Reader,
    Encoder,
    Converter,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        Self {
            gallery: GalleryView::new(),
            reader: ReaderView::new(),
            encoder: EncoderView::new(),
            converter: ConverterView::new(),
            mode: AppMode::Gallery,
            fullscreen: false,
        }
    }

    pub fn run() -> eframe::Result {
        #[cfg(feature = "ffmpeg")]
        if let Err(e) = crate::ffmpeg_util::ensure_initialized() {
            crate::log_error!("FFmpeg init at startup failed: {e}");
        } else {
            crate::log_info!("FFmpeg init at startup OK");
        }

        crate::log_info!("ViewerApp::run starting");

        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 720.0])
                .with_min_inner_size([900.0, 620.0])
                .with_title("ArchiveUtils"),
            ..Default::default()
        };

        eframe::run_native(
            "ArchiveUtils",
            options,
            Box::new(|cc| Ok(Box::new(ViewerApp::new(cc)))),
        )
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::left("nav_panel")
            .resizable(false)
            .default_width(NAV_WIDTH)
            .frame(
                egui::Frame::new()
                    .fill(theme::panel())
                    .inner_margin(egui::Margin::symmetric(16, 20))
                    .stroke(egui::Stroke::new(1.0, theme::border())),
            )
            .show_inside(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("ArchiveUtils")
                            .size(18.0)
                            .strong()
                            .color(theme::accent()),
                    );
                    ui.label(
                        egui::RichText::new("AIL")
                            .size(11.0)
                            .color(theme::text_muted()),
                    );
                });
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                if theme::nav_button(ui, self.mode == AppMode::Gallery, "🖼", "Галерея") {
                    self.mode = AppMode::Gallery;
                }
                if theme::nav_button(ui, self.mode == AppMode::Encoder, "⚙", "Кодировщик") {
                    self.mode = AppMode::Encoder;
                }
                if theme::nav_button(ui, self.mode == AppMode::Converter, "🎬", "TTS → MP4") {
                    self.mode = AppMode::Converter;
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("F — полный экран · H — скрыть UI")
                            .size(10.0)
                            .color(theme::text_muted()),
                    );
                });
            });

        match self.mode {
            AppMode::Gallery => {
                let response = self.gallery.ui(&ctx, ui);

                if let Some(path) = response.clicked_file {
                    self.reader.open_file(&ctx, path);
                    self.mode = AppMode::Reader;
                }
            }
            AppMode::Reader => {
                let response = self.reader.ui(&ctx, ui);

                if response.close_requested {
                    self.reader.close();
                    self.gallery.clear_selection();
                    self.mode = AppMode::Gallery;
                    if self.fullscreen {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                        self.fullscreen = false;
                    }
                }
                if response.toggle_fullscreen {
                    self.fullscreen = !self.fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                }
            }
            AppMode::Encoder => {
                self.encoder.ui(&ctx, ui);
            }
            AppMode::Converter => {
                self.converter.ui(&ctx, ui);
            }
        }
    }
}

/// Запускает приложение просмотрщика
pub fn run_viewer() -> eframe::Result {
    ViewerApp::run()
}
