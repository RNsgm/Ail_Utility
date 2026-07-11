use crate::encoder::{
    encode_media_to_ail, is_media_file, ArchiveType, CompressionType, EncodeOptions,
};
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
    input_files: Vec<PathBuf>,
    output_dir: Option<PathBuf>,
    options: EncodeOptions,
    is_encoding: bool,
    progress: (usize, usize),
    current_file: String,
    logs: Vec<(String, bool)>, // (message, is_error)
    receiver: Option<Receiver<EncodeMessage>>,
}

impl EncoderView {
    pub fn new() -> Self {
        Self {
            input_files: Vec::new(),
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
            ui.heading("Кодировщик архивов в .ail");
            ui.add_space(10.0);

            ui.group(|ui| {
                ui.label("1. Выбор файлов");
                ui.horizontal(|ui| {
                    if ui.button("➕ Добавить архивы").clicked() {
                        if let Some(files) = rfd::FileDialog::new()
                            .add_filter(
                                "Архивы и медиа",
                                &[
                                    "zip", "rar", "7z", "gif", "mp4", "webm", "avi", "mov", "mkv",
                                ],
                            )
                            .pick_files()
                        {
                            self.input_files.extend(files);
                        }
                    }
                    if ui.button("📁 Добавить папку").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.add_from_folder(folder);
                        }
                    }
                    if ui.button("🗑 Очистить").clicked() {
                        self.input_files.clear();
                    }
                });
                ui.label(format!("Выбрано файлов: {}", self.input_files.len()));
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
                            RichText::new("Не выбрана (будет рядом с архивом)")
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
                    egui::ProgressBar::new(self.progress.0 as f32 / self.progress.1 as f32)
                        .text(format!("Обработка {}/{}", self.progress.0, self.progress.1)),
                );
                ui.label(format!("Текущий: {}", self.current_file));
            } else {
                let can_start = !self.input_files.is_empty();
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

    fn add_from_folder(&mut self, folder: PathBuf) {
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && (ArchiveType::from_path(&path).is_some() || is_media_file(&path))
                {
                    self.input_files.push(path);
                }
            }
        }
    }
    fn start_encoding(&mut self, ctx: &Context) {
        self.is_encoding = true;
        self.progress = (0, self.input_files.len());
        self.logs.clear();

        let (tx, rx) = channel();
        self.receiver = Some(rx);

        let files = self.input_files.clone();
        let options = self.options.clone();
        let output_dir = self.output_dir.clone();
        let ctx = ctx.clone();

        std::thread::spawn(move || {
            let total = files.len();
            for (i, path) in files.into_iter().enumerate() {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let _ = tx.send(EncodeMessage::Progress {
                    current: i + 1,
                    total,
                    name: name.clone(),
                });
                ctx.request_repaint();

                let out_path = if let Some(ref dir) = output_dir {
                    dir.join(format!(
                        "{}.ail",
                        path.file_stem().unwrap_or_default().to_string_lossy()
                    ))
                } else {
                    path.with_extension("ail")
                };

                match encode_media_to_ail(&path, &out_path, Some(options.clone())) {
                    Ok(_) => {
                        let _ = tx.send(EncodeMessage::Success(name));
                    }
                    Err(e) => {
                        let _ = tx.send(EncodeMessage::Error(name, e.to_string()));
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
