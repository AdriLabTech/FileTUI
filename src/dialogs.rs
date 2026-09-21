//! Dialog handling: the "business logic" that runs when a dialog is confirmed.

use crate::fsops;
use crate::state::{App, DialogKind, MutationOp};

/// The pending mutation described by the current input dialog.
pub fn pending_op(app: &App) -> MutationOp {
    match app.dialog.as_ref().map(|d| &d.kind) {
        Some(DialogKind::CreateDir) => MutationOp::CreateDir,
        Some(DialogKind::CreateFile) => MutationOp::CreateFile,
        Some(DialogKind::Rename) => MutationOp::Rename,
        _ => MutationOp::CreateDir,
    }
}

/// Run the current input mutation on submit.
pub fn submit_input(app: &mut App) {
    let op = pending_op(app);
    let value = app
        .dialog
        .as_ref()
        .map(|d| d.input.trim().to_string())
        .unwrap_or_default();

    let res = match op {
        MutationOp::CreateDir => fsops::create_dir(&app.cwd, &value),
        MutationOp::CreateFile => fsops::create_file(&app.cwd, &value),
        MutationOp::Rename => {
            let target = app.selected_entry().map(|e| e.path.clone());
            match target {
                Some(p) => fsops::rename(&p, &value),
                None => fsops::OpResult::err("Nothing selected to rename"),
            }
        }
    };

    if res.ok {
        if let Err(e) = app.refresh() {
            app.status = Some(format!("Internal error: {}", e));
        } else {
            app.status = Some(res.message);
        }
        app.dialog = None;
    } else {
        // Keep the dialog open but surface the error description.
        if let Some(d) = app.dialog.as_mut() {
            d.message = res.message;
        }
    }
}

/// Perform a confirmed delete on the selected entry.
pub fn confirm_delete(app: &mut App) {
    let target = app.selected_entry().map(|e| e.path.clone());
    match target {
        Some(p) => {
            let res = fsops::delete(&p);
            app.status = Some(res.message);
            let _ = app.refresh();
            app.dialog = None;
        }
        None => {
            if let Some(d) = app.dialog.as_mut() {
                d.message = "Nothing selected to delete".to_string();
            }
        }
    }
}

/// Open a confirmation dialog for a destructive action.
pub fn confirm(app: &mut App, message: impl Into<String>, detail: impl Into<String>) {
    app.dialog = Some(crate::state::Dialog {
        kind: DialogKind::ConfirmDelete,
        input: String::new(),
        cursor: 0,
        message: message.into(),
        detail: detail.into(),
    });
}

/// Open a generic alert dialog.
pub fn alert(app: &mut App, message: impl Into<String>) {
    app.dialog = Some(crate::state::Dialog {
        kind: DialogKind::Alert,
        input: String::new(),
        cursor: 0,
        message: message.into(),
        detail: String::new(),
    });
}

/// Put the selected entry on the clipboard.
pub fn clipboard_set(app: &mut App, is_move: bool) {
    let Some(entry) = app.selected_entry().cloned() else {
        return;
    };
    let verb = if is_move { "Cut" } else { "Copied" };
    app.clip = Some(crate::state::Clipboard {
        path: entry.path,
        is_move,
    });
    app.status = Some(format!(
        "{} '{}'. Navigate and press p to {}.",
        verb,
        entry.name,
        if is_move { "move here" } else { "paste" }
    ));
}

/// Paste the clipboard into the current directory.
pub fn paste(app: &mut App) {
    let Some(clip) = app.clip.take() else {
        return;
    };
    let dest = app.cwd.clone();
    let res = if clip.is_move {
        fsops::move_into(&clip.path, &dest)
    } else {
        fsops::copy_into(&clip.path, &dest)
    };
    let _ = app.refresh();
    app.status = Some(res.message);
}
