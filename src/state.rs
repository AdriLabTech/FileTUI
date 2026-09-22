//! Application state: the current directory, the visible entries, the selected
//! index, whether hidden files are shown, and the current dialog (if any).

use std::path::PathBuf;

use crate::fsops::{self, Entry};

/// Kinds of modal dialogs the app can show.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogKind {
    /// Create a directory (text input).
    CreateDir,
    /// Create a file (text input).
    CreateFile,
    /// Rename the selected entry (text input).
    Rename,
    /// Confirm a destructive action (yes/no).
    ConfirmDelete,
    /// Alert with a single OK button (e.g. an error).
    Alert,
}

/// The active dialog and whatever message/context it needs.
#[derive(Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    /// User-editable text buffer for input dialogs.
    pub input: String,
    pub cursor: usize,
    /// Main message shown in the dialog.
    pub message: String,
    /// Optional secondary detail (e.g. target name for confirmation/alert).
    pub detail: String,
}

impl Dialog {
    pub fn title(&self) -> &'static str {
        match self.kind {
            DialogKind::CreateDir => "Create directory",
            DialogKind::CreateFile => "Create file",
            DialogKind::Rename => "Rename",
            DialogKind::ConfirmDelete => "Confirm delete",
            DialogKind::Alert => "Message",
        }
    }

    /// Insert a character into the input at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.input.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }
}

/// The mutation we want to perform when a text-input dialog is submitted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MutationOp {
    CreateDir,
    CreateFile,
    Rename,
}

/// A pending copy (is_move=false) or move (is_move=true) clipboard entry.
#[derive(Debug, Clone)]
pub struct Clipboard {
    pub path: PathBuf,
    pub is_move: bool,
}

/// What the fullscreen viewer is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerKind {
    /// A readable text document: the viewer scrolls its lines.
    Text,
    /// An image on a kitty-capable terminal: the viewer zooms and pans it.
    Image,
    /// Anything else (binary, or an image without kitty support): the viewer
    /// shows metadata plus an explanatory hint.
    Meta,
}

/// A file opened fullscreen. Opened with `→`/`Enter` on a file and closed with
/// `Esc`/`q`/`←` (image zoom-out closes it when back at fit). The viewer never
/// quits the app.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub kind: ViewerKind,
    pub path: PathBuf,
    pub name: String,
    /// Lines to render: the document for `Text`, metadata for `Meta`, or empty
    /// for an `Image` (the terminal draws the graphic itself).
    pub lines: Vec<String>,
    /// First visible line for text/meta viewers.
    pub scroll: usize,
    /// Viewport height in rows, kept in sync by the renderer each frame.
    pub page: usize,
    /// Magnification level for images: 1 (fit), 2, 4 or 8.
    pub zoom: u32,
    /// Pan offset for images, in eighths of the visible window per axis.
    pub pan: (i32, i32),
}

impl Viewer {
    pub fn new(kind: ViewerKind, path: PathBuf, name: String, lines: Vec<String>) -> Self {
        Viewer {
            kind,
            path,
            name,
            lines,
            scroll: 0,
            page: 1,
            zoom: 1,
            pan: (0, 0),
        }
    }

    /// Maximum `scroll` with the current viewport height (last full page).
    pub fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.page.max(1))
    }

    /// Scroll by `delta` lines (positive = down), clamped to the document.
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll() as isize;
        let next = self.scroll as isize + delta;
        self.scroll = next.clamp(0, max) as usize;
    }

    pub fn scroll_top(&mut self) {
        self.scroll = 0;
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    /// Cycle magnification one step up: 1 → 2 → 4 → 8 → 1.
    pub fn zoom_in(&mut self) {
        self.zoom = match self.zoom {
            1 => 2,
            2 => 4,
            4 => 8,
            _ => 1,
        };
        self.pan = self.clamp_pan();
    }

    /// Cycle magnification one step down: 8 → 4 → 2 → 1 → 1.
    pub fn zoom_out(&mut self) {
        self.zoom = match self.zoom {
            8 => 4,
            4 => 2,
            2 => 1,
            _ => 1,
        };
        self.pan = self.clamp_pan();
    }

    /// Shift the image pan by `(dx, dy)` eighths of the visible window.
    pub fn pan_by(&mut self, dx: i32, dy: i32) {
        self.pan = (self.pan.0.saturating_add(dx), self.pan.1.saturating_add(dy));
        self.pan = self.clamp_pan();
    }

    /// Keep pan within ±8 eighths (a full window of travel in each direction).
    fn clamp_pan(&self) -> (i32, i32) {
        (self.pan.0.clamp(-8, 8), self.pan.1.clamp(-8, 8))
    }
}

#[derive(Debug)]
pub struct App {
    /// Directory currently open in the file list.
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub show_hidden: bool,
    pub show_help: bool,
    pub dialog: Option<Dialog>,
    /// Transient status message shown at the bottom.
    pub status: Option<String>,
    /// Clipboard for copy/move.
    pub clip: Option<Clipboard>,
    /// Cache of the last preview, keyed by path, to avoid re-reading files
    /// on every redraw.
    pub preview_path: Option<PathBuf>,
    pub preview_lines: Vec<String>,
    /// Parent directory and its listing (left panel).
    pub parent: PathBuf,
    pub parent_entries: Vec<Entry>,
    /// Tracked kitty image placement for the preview panel.
    pub img: crate::img::ImgState,
    /// Fullscreen viewer (file opened with `→`/`Enter`), if any.
    pub viewer: Option<Viewer>,
}

impl App {
    pub fn new(start_dir: PathBuf) -> std::io::Result<Self> {
        let mut app = App {
            cwd: start_dir,
            entries: Vec::new(),
            selected: 0,
            show_hidden: false,
            show_help: false,
            dialog: None,
            status: None,
            clip: None,
            preview_path: None,
            preview_lines: Vec::new(),
            parent: PathBuf::new(),
            parent_entries: Vec::new(),
            img: crate::img::ImgState::default(),
            viewer: None,
        };
        app.refresh()?;
        Ok(app)
    }

    /// Reload the current directory listing (and the parent panel listing).
    pub fn refresh(&mut self) -> std::io::Result<()> {
        let entries = fsops::list_dir(&self.cwd, self.show_hidden)?;
        self.entries = entries;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        self.update_parent();
        Ok(())
    }

    /// Recompute the parent directory listing shown in the left panel.
    pub fn update_parent(&mut self) {
        self.parent = fsops::parent_dir(&self.cwd);
        self.parent_entries = fsops::list_dir(&self.parent, self.show_hidden).unwrap_or_default();
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Navigate up one directory level.
    pub fn go_up(&mut self) -> std::io::Result<()> {
        let parent = fsops::parent_dir(&self.cwd);
        // Only commit the new directory if it can be listed.
        let entries = fsops::list_dir(&parent, self.show_hidden)?;
        let prev_name = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.cwd = parent;
        self.entries = entries;
        // Highlight the dir we just left if present.
        if let Some(name) = prev_name {
            if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                self.selected = i;
            } else {
                self.selected = 0;
            }
        } else {
            self.selected = 0;
        }
        self.update_parent();
        Ok(())
    }

    /// Enter the selected directory, if it is one.
    pub fn enter_selected(&mut self) -> std::io::Result<bool> {
        let Some(entry) = self.selected_entry().cloned() else {
            return Ok(false);
        };
        if entry.is_dir {
            let entries = fsops::list_dir(&entry.path, self.show_hidden)?;
            self.cwd = entry.path;
            self.entries = entries;
            self.selected = 0;
            self.update_parent();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let mut n = self.selected as isize + delta;
        if n < 0 {
            n = 0;
        }
        if n >= len {
            n = len - 1;
        }
        self.selected = n as usize;
    }

    /// Open a new fullscreen viewer for the currently selected file.
    pub fn open_viewer(&mut self, viewer: Viewer) {
        self.viewer = Some(viewer);
    }

    /// Close the fullscreen viewer, returning to the file list.
    pub fn close_viewer(&mut self) {
        self.viewer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(kind: ViewerKind, lines: Vec<String>) -> Viewer {
        Viewer::new(kind, PathBuf::from("/tmp/x"), "x".into(), lines)
    }

    fn text_lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn viewer_scroll_clamps_to_document() {
        let mut v = viewer(ViewerKind::Text, text_lines(20));
        v.page = 5;
        assert_eq!(v.max_scroll(), 15);
        v.scroll_by(-100);
        assert_eq!(v.scroll, 0);
        v.scroll_by(100);
        assert_eq!(v.scroll, 15); // last page fully visible
        v.scroll_by(1);
        assert_eq!(v.scroll, 15); // stays clamped
        v.scroll_top();
        assert_eq!(v.scroll, 0);
        v.scroll_bottom();
        assert_eq!(v.scroll, 15);
    }

    #[test]
    fn viewer_scroll_empty_document() {
        let mut v = viewer(ViewerKind::Text, Vec::new());
        v.page = 10;
        assert_eq!(v.max_scroll(), 0);
        v.scroll_by(50);
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn viewer_zoom_cycles_1_2_4_8() {
        let mut v = viewer(ViewerKind::Image, Vec::new());
        v.zoom_in();
        assert_eq!(v.zoom, 2);
        v.zoom_in();
        assert_eq!(v.zoom, 4);
        v.zoom_in();
        assert_eq!(v.zoom, 8);
        v.zoom_in();
        assert_eq!(v.zoom, 1); // wraps around
        v.zoom_out();
        assert_eq!(v.zoom, 1); // floor
        v.zoom_out();
        assert_eq!(v.zoom, 1);
    }

    #[test]
    fn viewer_pan_clamps_to_window_travel() {
        let mut v = viewer(ViewerKind::Image, Vec::new());
        v.pan_by(100, -100);
        assert_eq!(v.pan, (8, -8));
        v.pan_by(-100, 100);
        assert_eq!(v.pan, (-8, 8));
        v.pan_by(3, -2);
        assert_eq!(v.pan, (-5, 6));
        v.zoom_in();
        assert_eq!(v.zoom, 2);
        v.zoom_in();
        v.zoom_in();
        assert_eq!(v.zoom, 8);
    }

    #[test]
    fn viewer_scroll_with_smaller_page() {
        let mut v = viewer(ViewerKind::Text, text_lines(3));
        v.page = 10; // viewport larger than the document
        assert_eq!(v.max_scroll(), 0);
        v.scroll_by(5);
        assert_eq!(v.scroll, 0);
    }
}
