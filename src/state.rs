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
}
