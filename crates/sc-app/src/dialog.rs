//! An in-application file browser.
//!
//! Deliberately not a native dialog. Those go through the XDG desktop portal on
//! Linux, which is frequently absent (under WSL, for instance, there is no
//! portal and no GTK), and a file dialog that silently fails to appear is worse
//! than a plain one that always works. This also keeps the chrome consistent.

use crate::theme;
use egui::{Align, Layout, RichText, Vec2};
use std::path::{Path, PathBuf};

/// What the browser is being used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Open,
    SaveAs,
    ExportStl,
}

impl Purpose {
    fn title(self) -> &'static str {
        match self {
            Purpose::Open => "Open document",
            Purpose::SaveAs => "Save document as",
            Purpose::ExportStl => "Export STL",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Purpose::Open | Purpose::SaveAs => sc_doc::file::EXTENSION,
            Purpose::ExportStl => "stl",
        }
    }

    fn is_save(self) -> bool {
        !matches!(self, Purpose::Open)
    }

    fn confirm(self) -> &'static str {
        match self {
            Purpose::Open => "Open",
            Purpose::SaveAs => "Save",
            Purpose::ExportStl => "Export",
        }
    }
}

/// What the browser decided this frame.
pub(crate) enum Outcome {
    /// Still open.
    Pending,
    Cancelled,
    Chosen(PathBuf),
}

pub(crate) struct FileBrowser {
    pub(crate) purpose: Purpose,
    directory: PathBuf,
    filename: String,
    message: Option<String>,
}

impl FileBrowser {
    pub(crate) fn new(purpose: Purpose, start: &Path, filename: String) -> Self {
        let directory = if start.is_dir() {
            start.to_path_buf()
        } else {
            start.parent().map_or_else(
                || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Path::to_path_buf,
            )
        };
        Self {
            purpose,
            directory,
            filename,
            message: None,
        }
    }

    /// Directories first, then files with the right extension.
    fn entries(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let Ok(read) = std::fs::read_dir(&self.directory) else {
            return (Vec::new(), Vec::new());
        };
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        for entry in read.flatten() {
            let path = entry.path();
            // Hidden entries are noise in a file picker.
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(self.purpose.extension()))
            {
                files.push(path);
            }
        }
        dirs.sort();
        files.sort();
        (dirs, files)
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Pending;

        egui::Modal::new(egui::Id::new("file-browser")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new(self.purpose.title()).heading());
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.directory.parent().is_some(), egui::Button::new("Up"))
                    .clicked()
                {
                    if let Some(parent) = self.directory.parent() {
                        self.directory = parent.to_path_buf();
                    }
                }
                ui.label(
                    RichText::new(self.directory.display().to_string())
                        .size(12.0)
                        .color(theme::TEXT_DIM),
                );
            });
            ui.add_space(8.0);

            let (dirs, files) = self.entries();
            egui::Frame::new()
                .fill(theme::SURFACE_ALT)
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if dirs.is_empty() && files.is_empty() {
                                ui.label(
                                    RichText::new("Nothing here")
                                        .size(12.0)
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for dir in dirs {
                                let name = dir.file_name().unwrap_or_default().to_string_lossy();
                                if ui.selectable_label(false, format!("{name}/")).clicked() {
                                    self.directory = dir;
                                }
                            }
                            for file in files {
                                let name = file.file_name().unwrap_or_default().to_string_lossy();
                                let selected = self.filename == name;
                                if ui.selectable_label(selected, name.to_string()).clicked() {
                                    self.filename = name.to_string();
                                    if !self.purpose.is_save() {
                                        outcome = Outcome::Chosen(file.clone());
                                    }
                                }
                            }
                        });
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Name").size(12.0).color(theme::TEXT_DIM));
                ui.add_sized(
                    Vec2::new(ui.available_width() - 180.0, 28.0),
                    egui::TextEdit::singleline(&mut self.filename),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, self.purpose.confirm()).clicked() {
                        match self.resolve() {
                            Ok(path) => outcome = Outcome::Chosen(path),
                            Err(e) => self.message = Some(e),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Outcome::Cancelled;
                    }
                });
            });

            if let Some(message) = &self.message {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(message)
                        .size(11.5)
                        .color(egui::Color32::from_rgb(0xC0, 0x3A, 0x3A)),
                );
            }
        });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }

    /// Validates the typed name and appends the extension if it is missing.
    fn resolve(&self) -> Result<PathBuf, String> {
        let name = self.filename.trim();
        if name.is_empty() {
            return Err("Enter a file name".to_string());
        }
        let mut path = self.directory.join(name);
        if path.extension().is_none() {
            path.set_extension(self.purpose.extension());
        }
        if !self.purpose.is_save() && !path.is_file() {
            return Err(format!("{} does not exist", path.display()));
        }
        Ok(path)
    }
}
