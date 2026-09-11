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

/// What is in a directory, or why it could not be read.
#[derive(Debug, Default)]
struct Listing {
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    /// `Some` when the directory could not be read at all.
    ///
    /// An unreadable directory and an empty one are not the same thing, and a
    /// browser that draws both as "Nothing here" tells someone whose disk has
    /// gone away that their files are gone.
    error: Option<String>,
}

/// Directories first, then files with the right extension.
///
/// Symlinks are followed by `is_dir`, which answers false for a loop rather
/// than recursing, so one is simply not offered as somewhere to go.
fn listing(directory: &Path, extension: &str) -> Listing {
    let read = match std::fs::read_dir(directory) {
        Ok(read) => read,
        Err(e) => {
            return Listing {
                error: Some(format!("Cannot open this folder: {e}")),
                ..Listing::default()
            }
        }
    };
    let mut listing = Listing::default();
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
            listing.dirs.push(path);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        {
            listing.files.push(path);
        }
    }
    listing.dirs.sort();
    listing.files.sort();
    listing
}

/// The directory the browser opens on, given where the document lives.
///
/// A bare file name, which is what an unsaved document offers, has `Some("")`
/// for a parent rather than `None`. Opening on it gives a browser whose path
/// bar is blank, whose list is empty however many files are next to it, and
/// whose Up button is disabled: every route out of it closed at once. The
/// working directory is the honest answer to that and to a path with no parent.
fn starting_directory(start: &Path) -> PathBuf {
    if start.is_dir() {
        return start.to_path_buf();
    }
    match start.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Longest file name that will be accepted, in bytes.
///
/// Every filesystem worth naming stops at 255 bytes per component: ext4, APFS,
/// NTFS and every FAT variant. Catching it here says which part of the name is
/// the problem, where letting the write fail reports `File name too long`
/// against the whole path, after the document has been meshed and encoded.
const MAX_NAME: usize = 255;

/// The path a typed name means, with the extension the purpose needs.
fn resolved(name: &str, directory: &Path, extension: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a file name".to_string());
    }
    // One check for four bad names: a path rather than a name, an absolute
    // path, `.`, and `..`. Each of the last three joins to a directory, and
    // writing a document over a directory fails with an error from the
    // filesystem that says nothing about what the user typed.
    if Path::new(name).file_name().is_none_or(|f| f != name) {
        return Err("Enter a file name, not a path".to_string());
    }
    let mut file_name = name.to_string();
    if !Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
    {
        // Appended rather than substituted: the `v2` in `bracket.v2` is part of
        // the name somebody chose, not a file type. Saving it as typed would
        // leave a document this browser never lists again, which is a document
        // its author cannot find.
        file_name.push('.');
        file_name.push_str(extension);
    }
    if file_name.len() > MAX_NAME {
        return Err(format!(
            "That name is too long: {} characters, and the limit is {MAX_NAME}",
            file_name.len()
        ));
    }
    Ok(directory.join(file_name))
}

/// A file picked from the list, if it is still there.
///
/// Between a directory being read and a row in it being clicked, the file can
/// have been moved, deleted or unmounted: the listing is a picture of a moment
/// that has passed. Whatever comes back here is opened, so this is the last
/// point at which anything checks.
fn still_there(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        Ok(path.to_path_buf())
    } else {
        Err(format!("{} is no longer there", path.display()))
    }
}

/// A path short enough for the bar it is drawn in.
///
/// Elided from the left, because the end of a path says where you are and the
/// beginning says where everybody's home directory is. Without this a working
/// directory a few levels down runs past the edge of the modal.
fn shorten(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    if count <= max_chars {
        return path.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let tail: String = path.chars().skip(count - keep).collect();
    // Start at a separator where there is one, so the first name shown is a
    // whole name rather than the end of one.
    match tail.find(std::path::MAIN_SEPARATOR) {
        Some(at) => format!("\u{2026}{}", &tail[at..]),
        None => format!("\u{2026}{tail}"),
    }
}

/// How much of the directory path fits across the modal, in characters.
const PATH_CHARS: usize = 64;

impl FileBrowser {
    pub(crate) fn new(purpose: Purpose, start: &Path, filename: String) -> Self {
        Self {
            purpose,
            directory: starting_directory(start),
            filename,
            message: None,
        }
    }

    /// Moves to another directory, dropping a message about the last one.
    fn go_to(&mut self, directory: PathBuf) {
        self.directory = directory;
        self.message = None;
    }

    /// Where the browser is, and the way back up.
    fn location_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.directory.parent().is_some(), egui::Button::new("Up"))
                .clicked()
            {
                if let Some(parent) = self.directory.parent() {
                    self.go_to(parent.to_path_buf());
                }
            }
            // The whole path on hover, since what is elided is the part that
            // says which machine and which user, which is worth being able to
            // check before writing a file.
            let full = self.directory.display().to_string();
            ui.label(
                RichText::new(shorten(&full, PATH_CHARS))
                    .size(12.0)
                    .color(theme::palette().text_dim),
            )
            .on_hover_text(full);
        });
    }

    /// What is in this directory. Reports a file chosen by clicking its row.
    fn list(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let Listing { dirs, files, error } = listing(&self.directory, self.purpose.extension());
        let mut chosen = None;
        egui::Frame::new()
            .fill(theme::palette().surface_alt)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(error) = &error {
                            ui.label(
                                RichText::new(error)
                                    .size(12.0)
                                    .color(theme::palette().danger),
                            );
                        } else if dirs.is_empty() && files.is_empty() {
                            ui.label(
                                RichText::new("Nothing here")
                                    .size(12.0)
                                    .color(theme::palette().text_dim),
                            );
                        }
                        for dir in dirs {
                            let name = dir.file_name().unwrap_or_default().to_string_lossy();
                            if ui.selectable_label(false, format!("{name}/")).clicked() {
                                self.go_to(dir);
                            }
                        }
                        for file in files {
                            let name = file.file_name().unwrap_or_default().to_string_lossy();
                            let selected = self.filename == name;
                            if ui.selectable_label(selected, name.to_string()).clicked() {
                                self.filename = name.to_string();
                                self.message = None;
                                // A save wants the name in the field and the
                                // chance to change it; an open is done here.
                                if !self.purpose.is_save() {
                                    match still_there(&file) {
                                        Ok(path) => chosen = Some(path),
                                        Err(e) => self.message = Some(e),
                                    }
                                }
                            }
                        }
                    });
            });
        chosen
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Pending;

        egui::Modal::new(egui::Id::new("file-browser")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new(self.purpose.title()).heading());
            ui.add_space(10.0);

            self.location_bar(ui);
            ui.add_space(8.0);
            if let Some(chosen) = self.list(ui) {
                outcome = Outcome::Chosen(chosen);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Name")
                        .size(12.0)
                        .color(theme::palette().text_dim),
                );
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
                        .color(theme::palette().danger),
                );
            }
        });

        // Only when nothing else has been decided. Escape arriving in the same
        // frame as a click on Open would otherwise throw the choice away.
        if matches!(outcome, Outcome::Pending) && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }

    /// Validates the typed name and appends the extension if it is missing.
    fn resolve(&self) -> Result<PathBuf, String> {
        let path = resolved(&self.filename, &self.directory, self.purpose.extension())?;
        if !self.purpose.is_save() && !path.is_file() {
            return Err(format!("{} does not exist", path.display()));
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{listing, resolved, shorten, starting_directory, still_there, Purpose, MAX_NAME};
    use std::path::{Path, PathBuf};

    /// A scratch directory of this test's own, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("shapecad-dialog-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("scratch directory");
            Self(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, b"x").expect("scratch file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// An unsaved document suggests a bare file name, whose parent is the empty
    /// path rather than nothing at all. Opening on it gives a browser with a
    /// blank path bar, an empty list wherever it is run from, and Up disabled.
    #[test]
    fn a_name_with_no_directory_opens_somewhere_real() {
        let directory = starting_directory(Path::new("Untitled.shapecad"));
        assert!(
            directory.is_dir(),
            "the browser opened on {}, which is not a directory",
            directory.display()
        );
    }

    #[test]
    fn a_document_in_a_directory_opens_beside_it() {
        let scratch = Scratch::new("beside");
        let file = scratch.file("part.shapecad");
        assert_eq!(starting_directory(&file), scratch.0);
        // And a directory handed in directly is where it opens.
        assert_eq!(starting_directory(&scratch.0), scratch.0);
    }

    /// The root has no parent, which the Up button already knows. Opening
    /// straight onto it must not be a special case either.
    #[test]
    fn a_path_with_no_parent_is_not_a_problem() {
        let root = Path::new(std::path::MAIN_SEPARATOR_STR);
        assert!(starting_directory(root).is_dir());
    }

    /// A directory that cannot be read is not an empty directory, and drawing
    /// both as "Nothing here" tells somebody whose disk has gone away that
    /// their files are gone.
    #[test]
    fn a_directory_that_cannot_be_read_says_so() {
        let missing = std::env::temp_dir().join("shapecad-no-such-directory-9f3a1c");
        let listed = listing(&missing, "shapecad");
        assert!(
            listed.error.is_some(),
            "an unreadable directory listed as if it were empty"
        );
        assert!(listed.dirs.is_empty() && listed.files.is_empty());
    }

    #[test]
    fn a_listing_shows_matching_files_and_every_directory() {
        let scratch = Scratch::new("listing");
        scratch.file("one.shapecad");
        scratch.file("two.stl");
        scratch.file(".hidden.shapecad");
        std::fs::create_dir_all(scratch.0.join("sub")).expect("subdirectory");

        let listed = listing(&scratch.0, "shapecad");
        assert!(listed.error.is_none());
        assert_eq!(listed.files.len(), 1, "{:?}", listed.files);
        assert!(listed.files[0].ends_with("one.shapecad"));
        assert_eq!(listed.dirs.len(), 1, "{:?}", listed.dirs);
    }

    /// A symlink that points at itself cannot be described as a directory, and
    /// listing must answer rather than walk into it.
    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_is_listed_without_following_it() {
        let scratch = Scratch::new("loop");
        let link = scratch.0.join("ouroboros");
        std::os::unix::fs::symlink(&link, &link).expect("a link to itself");

        let listed = listing(&scratch.0, "shapecad");
        assert!(listed.error.is_none());
        assert!(
            listed.dirs.is_empty(),
            "a loop was offered as somewhere to go"
        );
    }

    /// A name with no extension gets the one the purpose needs, and a name with
    /// the wrong one keeps what was typed and gains the right one: `bracket.v2`
    /// is a version, not a file type, and saving it as typed leaves a document
    /// this browser will never list again.
    #[test]
    fn a_name_always_ends_up_with_the_right_extension() {
        let dir = Path::new("/tmp");
        for (typed, expected) in [
            ("bracket", "bracket.shapecad"),
            ("bracket.shapecad", "bracket.shapecad"),
            ("bracket.SHAPECAD", "bracket.SHAPECAD"),
            ("bracket.v2", "bracket.v2.shapecad"),
            ("bracket.stl", "bracket.stl.shapecad"),
            ("bracket.", "bracket..shapecad"),
            ("  bracket  ", "bracket.shapecad"),
        ] {
            let path = resolved(typed, dir, "shapecad").expect(typed);
            assert_eq!(
                path,
                dir.join(expected),
                "{typed:?} resolved to {}",
                path.display()
            );
        }
        assert_eq!(
            resolved("bracket", dir, "stl").expect("stl"),
            dir.join("bracket.stl")
        );
    }

    /// Four names that are not names. Each one joins to a directory or somewhere
    /// else entirely, and the write then fails with a message about the path
    /// that says nothing about what was typed.
    #[test]
    fn a_name_that_is_a_path_is_refused() {
        let dir = Path::new("/tmp");
        for typed in ["", "   ", ".", "..", "/etc/passwd", "sub/part", "sub/"] {
            assert!(
                resolved(typed, dir, "shapecad").is_err(),
                "{typed:?} was accepted as a file name"
            );
        }
    }

    /// Every filesystem stops at 255 bytes per component. Caught here it names
    /// the problem; left alone it fails after the document has been encoded.
    #[test]
    fn a_name_too_long_for_any_filesystem_is_refused() {
        let dir = Path::new("/tmp");
        let longest = "a".repeat(MAX_NAME - ".shapecad".len());
        assert!(resolved(&longest, dir, "shapecad").is_ok());
        assert!(
            resolved(&format!("{longest}a"), dir, "shapecad").is_err(),
            "a name over the limit was accepted"
        );
        assert!(resolved(&"b".repeat(4000), dir, "shapecad").is_err());
    }

    /// A listing is a picture of a moment that has passed. The file can be gone
    /// by the time the row for it is clicked.
    #[test]
    fn a_file_that_has_gone_is_not_opened() {
        let scratch = Scratch::new("vanishing");
        let file = scratch.file("here.shapecad");
        assert_eq!(still_there(&file).expect("it is there"), file);

        std::fs::remove_file(&file).expect("removed");
        assert!(
            still_there(&file).is_err(),
            "a file that is no longer there was handed over to be opened"
        );
        // A directory is not a document either.
        assert!(still_there(&scratch.0).is_err());
    }

    /// A path a few levels down is longer than the modal is wide.
    #[test]
    fn a_long_path_is_elided_from_the_left() {
        let short = "/home/a/parts";
        assert_eq!(shorten(short, 64), short);

        let long = "/home/someone/Workspace/personal/projects/machine/parts/brackets/mounting";
        let shortened = shorten(long, 40);
        assert!(
            shortened.chars().count() <= 40,
            "{shortened:?} is still {} characters",
            shortened.chars().count()
        );
        assert!(shortened.starts_with('\u{2026}'), "{shortened:?}");
        assert!(
            long.ends_with(shortened.trim_start_matches('\u{2026}')),
            "{shortened:?} is not the end of the path"
        );
    }

    /// Eliding must not split a character in half, which is what slicing by
    /// bytes would do.
    #[test]
    fn eliding_a_path_of_wide_characters_is_still_a_string() {
        let path = "/home/\u{7532}\u{9aa8}\u{6587}/\u{90e8}\u{54c1}/\u{56fa}\u{5b9a}\u{5177}";
        for width in 0..path.chars().count() + 2 {
            let shortened = shorten(path, width);
            assert!(shortened.chars().count() <= width.max(1));
        }
    }

    #[test]
    fn each_purpose_asks_for_the_extension_it_writes() {
        assert_eq!(Purpose::Open.extension(), sc_doc::file::EXTENSION);
        assert_eq!(Purpose::SaveAs.extension(), sc_doc::file::EXTENSION);
        assert_eq!(Purpose::ExportStl.extension(), "stl");
        assert!(!Purpose::Open.is_save());
        assert!(Purpose::SaveAs.is_save() && Purpose::ExportStl.is_save());
    }
}
