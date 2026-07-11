use crate::encoder::{
    collect_convertible_files, convert_to_mp4, mp4_output_path, needs_mp4_conversion,
};
use crate::viewer::theme::{self, primary_button};
use crate::viewer::thread_util::spawn_worker;
use egui::{Context, Ui};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

enum ConvertMessage {
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

pub struct ConverterView {
    input_paths: Vec<PathBuf>,
    output_dir: Option<PathBuf>,
    is_converting: bool,
    progress: (usize, usize),
    current_file: String,
    logs: Vec<(String, bool)>,
    receiver: Option<Receiver<ConvertMessage>>,
}

impl ConverterView {
    pub fn new() -> Self {
        Self {
            input_paths: Vec::new(),
            output_dir: None,
            is_converting: false,
            progress: (0, 0),
            current_file: String::new(),
            logs: Vec::new(),
            receiver: None,
        }
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) {
        self.process_messages();

        theme::inset_content(ui, |ui| {
            ui.vertical(|ui| {
                theme::page_header(
                    ui,
                    "Конвертер TTS → MP4",
                    Some("Файлы .tts и .mp4.tts → обычный MP4"),
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
                        if ui.button("🔍 Найти TTS").clicked() {
                            self.scan_for_convertible();
                        }
                        if ui.button("🗑 Очистить").clicked() {
                            self.input_paths.clear();
                        }
                    });

                    theme::row_gap(ui, 10.0);

                    let convertible = self.count_convertible();
                    ui.label(
                        egui::RichText::new(format!(
                            "Источников: {}, к конвертации: {convertible}",
                            self.input_paths.len()
                        ))
                        .color(theme::text_muted()),
                    );

                    if !self.input_paths.is_empty() {
                        theme::row_gap(ui, 8.0);
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .show(ui, |ui| {
                                for path in &self.input_paths {
                                    let icon = if path.is_dir() { "📁" } else { "📄" };
                                    let tag = if path.is_file() && needs_mp4_conversion(path) {
                                        " [TTS]"
                                    } else {
                                        ""
                                    };
                                    ui.label(format!("{icon} {}{tag}", path.display()));
                                }
                            });
                    }
                });

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
                                egui::RichText::new("Не выбрана — MP4 рядом с исходником")
                                    .color(theme::text_hint()),
                            );
                        }
                    });
                });

                theme::row_gap(ui, 20.0);

                if self.is_converting {
                    ui.add(
                        egui::ProgressBar::new(
                            self.progress.0 as f32 / self.progress.1.max(1) as f32,
                        )
                        .text(format!(
                            "Конвертация {}/{}",
                            self.progress.0, self.progress.1
                        ))
                        .fill(theme::accent()),
                    );
                    theme::row_gap(ui, 8.0);
                    ui.label(
                        egui::RichText::new(format!("Текущий: {}", self.current_file))
                            .color(theme::text_muted()),
                    );
                } else {
                    let can_start = self.count_convertible() > 0;
                    if primary_button(ui, "🎬 Конвертировать в MP4", can_start).clicked() {
                        self.start_conversion(ctx);
                    }
                }

                theme::log_panel(ui, 200.0, &self.logs);
                ui.add_space(20.0);
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

    fn scan_for_convertible(&mut self) {
        let roots: Vec<_> = self.input_paths.clone();
        for root in roots {
            for file in collect_convertible_files(&root) {
                if !self.input_paths.iter().any(|p| p == &file) {
                    self.input_paths.push(file);
                }
            }
        }
    }

    fn count_convertible(&self) -> usize {
        let mut files = Vec::new();
        for path in &self.input_paths {
            if path.is_file() {
                if needs_mp4_conversion(path) {
                    files.push(path.clone());
                }
            } else {
                files.extend(collect_convertible_files(path));
            }
        }
        files.sort();
        files.dedup();
        files.len()
    }

    fn files_to_convert(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for path in &self.input_paths {
            if path.is_file() {
                if needs_mp4_conversion(path) {
                    files.push(path.clone());
                }
            } else {
                files.extend(collect_convertible_files(path));
            }
        }
        files.sort();
        files.dedup();
        files
    }

    fn start_conversion(&mut self, ctx: &Context) {
        let files = self.files_to_convert();
        if files.is_empty() {
            self.logs
                .push(("⚠ Нет файлов для конвертации".to_string(), true));
            return;
        }

        self.is_converting = true;
        self.progress = (0, files.len());
        self.logs.clear();

        let (tx, rx) = channel();
        self.receiver = Some(rx);

        let output_dir = self.output_dir.clone();
        let ctx = ctx.clone();

        spawn_worker("converter", move || {
            let total = files.len();
            for (i, path) in files.into_iter().enumerate() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());

                let _ = tx.send(ConvertMessage::Progress {
                    current: i + 1,
                    total,
                    name: name.clone(),
                });
                ctx.request_repaint();

                if !needs_mp4_conversion(&path) {
                    let _ = tx.send(ConvertMessage::Skipped(name));
                    ctx.request_repaint();
                    continue;
                }

                let out_path = if let Some(ref dir) = output_dir {
                    dir.join(
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| format!("{}.mp4", i)),
                    )
                    .with_extension("mp4")
                } else {
                    mp4_output_path(&path)
                };

                if out_path.is_file() {
                    let _ = tx.send(ConvertMessage::Skipped(format!(
                        "{name} (уже есть: {})",
                        out_path.display()
                    )));
                    ctx.request_repaint();
                    continue;
                }

                match convert_to_mp4(&path, &out_path) {
                    Ok(_) => {
                        let _ = tx.send(ConvertMessage::Success(format!(
                            "{name} → {}",
                            out_path.display()
                        )));
                    }
                    Err(e) => {
                        let _ = tx.send(ConvertMessage::Error(name, e.to_string()));
                    }
                }
                ctx.request_repaint();
            }
            let _ = tx.send(ConvertMessage::Done);
            ctx.request_repaint();
        });
    }

    fn process_messages(&mut self) {
        let receiver = self.receiver.take();
        if let Some(rx) = receiver {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ConvertMessage::Progress {
                        current,
                        total,
                        name,
                    } => {
                        self.progress = (current, total);
                        self.current_file = name;
                    }
                    ConvertMessage::Success(msg) => {
                        self.logs.push((format!("✅ {msg}"), false));
                    }
                    ConvertMessage::Skipped(name) => {
                        self.logs.push((format!("⏭ Пропущен: {name}"), false));
                    }
                    ConvertMessage::Error(name, err) => {
                        self.logs.push((format!("❌ Ошибка {name}: {err}"), true));
                    }
                    ConvertMessage::Done => {
                        self.is_converting = false;
                        self.receiver = None;
                        return;
                    }
                }
            }
            self.receiver = Some(rx);
        }
    }
}
