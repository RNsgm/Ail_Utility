use crate::encoder::{
    collect_encoding_jobs, encode_media_to_ail, CompressionType, EncodeOptions, EncodeStatus,
};
use crate::log_info;
use crate::viewer::theme::{self, primary_button};
use crate::viewer::thread_util::spawn_worker;
use egui::{Context, Ui};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

enum EncodeMessage {
    Progress {
        current: usize,
        total: usize,
        name: String,
    },
    Success(String),
    Skipped(String),
    Error(String, String),
    Done,
}

pub struct EncoderView {
    input_paths: Vec<PathBuf>,
    output_dir: Option<PathBuf>,
    options: EncodeOptions,
    is_encoding: bool,
    progress: (usize, usize),
    current_file: String,
    logs: Vec<(String, bool)>,
    receiver: Option<Receiver<EncodeMessage>>,
}

impl EncoderView {
    pub fn new() -> Self {
        Self {
            input_paths: Vec::new(),
            output_dir: None,
            options: EncodeOptions::default(),
            is_encoding: false,
            progress: (0, 0),
            current_file: String::new(),
            logs: Vec::new(),
            receiver: None,
        }
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) {
        self.process_messages();

        ui.add_space(8.0);
        ui.vertical(|ui| {
            theme::page_header(
                ui,
                "Кодировщик в .ail",
                Some(
                    "Папка с картинками → галерея; каждое видео → свой .ail; архивы → отдельно",
                ),
            );

            theme::section_card(ui, "1. Источники", |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("➕ Файлы").clicked() {
                        if let Some(files) = rfd::FileDialog::new().pick_files() {
                            self.add_paths(files);
                        }
                    }
                    if ui.button("📁 Папка").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.add_paths([folder]);
                        }
                    }
                    if ui.button("🗑 Очистить").clicked() {
                        self.input_paths.clear();
                    }
                });

                let (files, folders) = self.count_inputs();
                let jobs = collect_encoding_jobs(&self.input_paths, self.output_dir.as_deref());
                ui.label(
                    egui::RichText::new(format!(
                        "Источников: {files} файлов, {folders} папок → {} заданий",
                        jobs.len()
                    ))
                    .color(theme::text_muted()),
                );

                if !self.input_paths.is_empty() {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for path in &self.input_paths {
                                let icon = if path.is_dir() { "📁" } else { "📄" };
                                ui.label(format!("{icon} {}", path.display()));
                            }
                        });
                }
            });

            ui.add_space(8.0);

            theme::section_card(ui, "2. Папка сохранения", |ui| {
                ui.horizontal(|ui| {
                    if ui.button("📁 Выбрать").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.output_dir = Some(folder);
                        }
                    }
                    if let Some(dir) = &self.output_dir {
                        ui.label(dir.to_string_lossy());
                    } else {
                        ui.label(
                            egui::RichText::new("Не выбрана — рядом с исходником")
                                .color(theme::text_hint()),
                        );
                    }
                });
            });

            ui.add_space(8.0);

            theme::section_card(ui, "3. Настройки", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Сжатие:");
                    ui.selectable_value(
                        &mut self.options.compression,
                        CompressionType::Zstd,
                        "Zstd",
                    );
                    ui.selectable_value(&mut self.options.compression, CompressionType::Lz4, "LZ4");
                    ui.selectable_value(
                        &mut self.options.compression,
                        CompressionType::None,
                        "Нет",
                    );
                });
                ui.add(
                    egui::Slider::new(&mut self.options.thumbnail_size, 100..=600)
                        .text("Размер превью"),
                );
                ui.checkbox(
                    &mut self.options.skip_existing,
                    "Пропускать уже существующие .ail",
                );
            });

            ui.add_space(12.0);

            if self.is_encoding {
                ui.add(
                    egui::ProgressBar::new(self.progress.0 as f32 / self.progress.1.max(1) as f32)
                        .text(format!("Обработка {}/{}", self.progress.0, self.progress.1))
                        .fill(theme::accent()),
                );
                ui.label(
                    egui::RichText::new(format!("Текущий: {}", self.current_file))
                        .color(theme::text_muted()),
                );
            } else {
                let can_start = !self.input_paths.is_empty();
                if primary_button(ui, "🚀 Начать кодирование", can_start).clicked() {
                    self.start_encoding(ctx);
                }
            }

            ui.add_space(12.0);
            theme::log_panel(ui, 200.0, &self.logs);
        });
    }

    fn add_paths<I: IntoIterator<Item = PathBuf>>(&mut self, paths: I) {
        for path in paths {
            if self.input_paths.iter().any(|p| p == &path) {
                continue;
            }
            self.input_paths.push(path);
        }
    }

    fn count_inputs(&self) -> (usize, usize) {
        let folders = self.input_paths.iter().filter(|p| p.is_dir()).count();
        let files = self.input_paths.len() - folders;
        (files, folders)
    }

    fn start_encoding(&mut self, ctx: &Context) {
        let jobs = collect_encoding_jobs(&self.input_paths, self.output_dir.as_deref());
        if jobs.is_empty() {
            self.logs
                .push(("⚠ Нет заданий для кодирования".to_string(), true));
            return;
        }

        self.is_encoding = true;
        self.progress = (0, jobs.len());
        self.logs.clear();

        let (tx, rx) = channel();
        self.receiver = Some(rx);

        let options = self.options.clone();
        let ctx = ctx.clone();

        spawn_worker("encoder", move || {
            let total = jobs.len();
            log_info!("Encoder: starting {total} jobs");
            for (i, job) in jobs.into_iter().enumerate() {
                let _ = tx.send(EncodeMessage::Progress {
                    current: i + 1,
                    total,
                    name: job.label.clone(),
                });
                ctx.request_repaint();

                match encode_media_to_ail(&job.input, &job.output, Some(options.clone())) {
                    Ok(EncodeStatus::Created) => {
                        let _ = tx.send(EncodeMessage::Success(format!(
                            "{} → {}",
                            job.label,
                            job.output.display()
                        )));
                    }
                    Ok(EncodeStatus::Skipped) => {
                        let _ = tx.send(EncodeMessage::Skipped(format!(
                            "{} (уже есть: {})",
                            job.label,
                            job.output.display()
                        )));
                    }
                    Err(e) => {
                        let _ = tx.send(EncodeMessage::Error(job.label, e.to_string()));
                    }
                }
                ctx.request_repaint();
            }
            let _ = tx.send(EncodeMessage::Done);
            ctx.request_repaint();
        });
    }

    fn process_messages(&mut self) {
        let receiver = self.receiver.take();
        if let Some(rx) = receiver {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    EncodeMessage::Progress {
                        current,
                        total,
                        name,
                    } => {
                        self.progress = (current, total);
                        self.current_file = name;
                    }
                    EncodeMessage::Success(name) => {
                        self.logs.push((format!("✅ Готово: {name}"), false));
                    }
                    EncodeMessage::Skipped(name) => {
                        self.logs.push((format!("⏭ Пропущен: {name}"), false));
                    }
                    EncodeMessage::Error(name, err) => {
                        self.logs
                            .push((format!("❌ Ошибка {name}: {err}"), true));
                    }
                    EncodeMessage::Done => {
                        self.is_encoding = false;
                        self.receiver = None;
                        return;
                    }
                }
            }
            self.receiver = Some(rx);
        }
    }
}
