use crate::encoder::{collect_encoding_jobs, encode_media_to_ail, CompressionType, EncodeOptions};
use egui::{Color32, Context, RichText, Ui};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

enum EncodeMessage {
    Progress {
        current: usize,
        total: usize,
        name: String,
    },
    Success(String),
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

        ui.vertical(|ui| {
            ui.heading("Кодировщик в .ail");
            ui.label(
                RichText::new("Каждый архив (zip/rar/7z) кодируется в отдельный .ail")
                    .color(Color32::GRAY),
            );
            ui.add_space(10.0);

            ui.group(|ui| {
                ui.label("1. Выбор файлов и папок");
                ui.horizontal(|ui| {
                    if ui.button("➕ Добавить файлы").clicked() {
                        if let Some(files) = rfd::FileDialog::new().pick_files() {
                            self.add_paths(files);
                        }
                    }
                    if ui.button("📁 Добавить папку").clicked() {
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
                ui.label(format!(
                    "Источников: {} файлов, {} папок → {} заданий .ail",
                    files,
                    folders,
                    jobs.len()
                ));

                if !self.input_paths.is_empty() {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for path in &self.input_paths {
                                let label = if path.is_dir() {
                                    format!("📁 {}", path.display())
                                } else {
                                    format!("📄 {}", path.display())
                                };
                                ui.label(label);
                            }
                        });
                }
            });

            ui.add_space(5.0);

            ui.group(|ui| {
                ui.label("2. Папка сохранения");
                ui.horizontal(|ui| {
                    if ui.button("📁 Выбрать папку").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.output_dir = Some(folder);
                        }
                    }
                    if let Some(dir) = &self.output_dir {
                        ui.label(dir.to_string_lossy());
                    } else {
                        ui.label(
                            RichText::new("Не выбрана (будет рядом с исходником)")
                                .color(Color32::GOLD),
                        );
                    }
                });
            });

            ui.add_space(5.0);

            ui.group(|ui| {
                ui.label("3. Настройки");
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
            });

            ui.add_space(10.0);

            if self.is_encoding {
                ui.add(
                    egui::ProgressBar::new(self.progress.0 as f32 / self.progress.1.max(1) as f32)
                        .text(format!("Обработка {}/{}", self.progress.0, self.progress.1)),
                );
                ui.label(format!("Текущий: {}", self.current_file));
            } else {
                let can_start = !self.input_paths.is_empty();
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new("🚀 Начать кодирование").min_size([200.0, 40.0].into()),
                    )
                    .clicked()
                {
                    self.start_encoding(ctx);
                }
            }

            ui.add_space(10.0);
            ui.label("Логи:");
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for (msg, is_err) in self.logs.iter().rev() {
                        let color = if *is_err {
                            Color32::LIGHT_RED
                        } else {
                            Color32::LIGHT_GREEN
                        };
                        ui.label(RichText::new(msg).color(color));
                    }
                });
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

        std::thread::spawn(move || {
            let total = jobs.len();
            for (i, job) in jobs.into_iter().enumerate() {
                let _ = tx.send(EncodeMessage::Progress {
                    current: i + 1,
                    total,
                    name: job.label.clone(),
                });
                ctx.request_repaint();

                match encode_media_to_ail(&job.input, &job.output, Some(options.clone())) {
                    Ok(_) => {
                        let _ = tx.send(EncodeMessage::Success(format!(
                            "{} → {}",
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
                        self.logs.push((format!("✅ Готово: {}", name), false));
                    }
                    EncodeMessage::Error(name, err) => {
                        self.logs
                            .push((format!("❌ Ошибка {}: {}", name, err), true));
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
