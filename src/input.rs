//! Input handling: translates terminal key events into state transitions.
//! This is a pure logic layer; it calls filesystem helpers via `dialogs`.

use crossterm::event::{Event, KeyCode, KeyEvent};

use crate::dialogs;
use crate::state::{App, Dialog, DialogKind};

/// Handle a terminal event. Returns `true` if the app should quit.
pub fn handle(app: &mut App, event: Event) -> std::io::Result<bool> {
    let Event::Key(key) = event else {
        return Ok(false);
    };

    // If help is up, only h/q/Esc close (q quits the app).
    if app.show_help {
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('h') | KeyCode::Esc => {
                app.show_help = false;
            }
            _ => {}
        }
        return Ok(false);
    }

    // Dialog is up: route keys to dialog handling.
    if app.dialog.is_some() {
        return handle_dialog(app, key);
    }

    handle_normal(app, key)
}

fn handle_dialog(app: &mut App, key: KeyEvent) -> std::io::Result<bool> {
    let kind = app.dialog.as_ref().unwrap().kind.clone();
    match kind {
        DialogKind::Alert => {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => {
                    app.dialog = None;
                }
                _ => {}
            }
            Ok(false)
        }
        DialogKind::ConfirmDelete => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                dialogs::confirm_delete(app);
                Ok(false)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.dialog = None;
                Ok(false)
            }
            _ => Ok(false),
        },
        DialogKind::CreateDir | DialogKind::CreateFile | DialogKind::Rename => {
            enum KeyAction {
                Confirm,
                Cancel,
                Noop,
            }
            let action = {
                let dialog = app.dialog.as_mut().unwrap();
                match key.code {
                    KeyCode::Enter => KeyAction::Confirm,
                    KeyCode::Esc => KeyAction::Cancel,
                    KeyCode::Char(c) => {
                        if !c.is_control() {
                            dialog.insert_char(c);
                        }
                        KeyAction::Noop
                    }
                    KeyCode::Backspace => {
                        dialog.backspace();
                        KeyAction::Noop
                    }
                    KeyCode::Delete => {
                        dialog.delete();
                        KeyAction::Noop
                    }
                    KeyCode::Left => {
                        if dialog.cursor > 0 {
                            dialog.cursor -= 1;
                        }
                        KeyAction::Noop
                    }
                    KeyCode::Right => {
                        if dialog.cursor < dialog.input.chars().count() {
                            dialog.cursor += 1;
                        }
                        KeyAction::Noop
                    }
                    KeyCode::Home => {
                        dialog.cursor = 0;
                        KeyAction::Noop
                    }
                    KeyCode::End => {
                        dialog.cursor = dialog.input.chars().count();
                        KeyAction::Noop
                    }
                    _ => KeyAction::Noop,
                }
            };
            match action {
                KeyAction::Confirm => dialogs::submit_input(app),
                KeyAction::Cancel => app.dialog = None,
                KeyAction::Noop => {}
            }
            Ok(false)
        }
    }
}

fn handle_normal(app: &mut App, key: KeyEvent) -> std::io::Result<bool> {
    match key.code {
        KeyCode::Char('q') => Ok(true),
        KeyCode::Char('h') => {
            app.show_help = true;
            Ok(false)
        }
        KeyCode::Char('.') => {
            app.show_hidden = !app.show_hidden;
            app.status = Some(format!(
                "Hidden files: {}",
                if app.show_hidden { "shown" } else { "hidden" }
            ));
            match app.refresh() {
                Ok(_) => {}
                Err(e) => {
                    dialogs::alert(app, format!("Could not list directory: {}", e));
                }
            }
            Ok(false)
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_selection(1);
            Ok(false)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_selection(-1);
            Ok(false)
        }
        KeyCode::Char('g') => {
            app.selected = 0;
            Ok(false)
        }
        KeyCode::Char('G') => {
            app.selected = app.entries.len().saturating_sub(1);
            Ok(false)
        }
        KeyCode::Home => {
            app.selected = 0;
            Ok(false)
        }
        KeyCode::End => {
            app.selected = app.entries.len().saturating_sub(1);
            Ok(false)
        }
        KeyCode::Enter | KeyCode::Right => {
            match app.enter_selected() {
                Ok(_) => {}
                Err(e) => {
                    dialogs::alert(app, format!("Could not enter directory: {}", e));
                }
            }
            Ok(false)
        }
        KeyCode::Backspace | KeyCode::Left => {
            match app.go_up() {
                Ok(()) => {}
                Err(e) => {
                    dialogs::alert(app, format!("Could not open parent: {}", e));
                }
            }
            Ok(false)
        }
        KeyCode::Char('n') => {
            app.dialog = Some(input_dialog(
                DialogKind::CreateDir,
                "Enter directory name:",
                String::new(),
            ));
            Ok(false)
        }
        KeyCode::Char('N') => {
            app.dialog = Some(input_dialog(
                DialogKind::CreateFile,
                "Enter file name:",
                String::new(),
            ));
            Ok(false)
        }
        KeyCode::Char('r') => {
            let selected = app.selected_entry().map(|e| e.name.clone());
            match selected {
                Some(name) => {
                    app.dialog = Some(input_dialog(DialogKind::Rename, "Rename to:", name));
                }
                None => {
                    dialogs::alert(app, "Nothing selected to rename.");
                }
            }
            Ok(false)
        }
        KeyCode::Char('d') => {
            let entry = app.selected_entry().cloned();
            match entry {
                Some(e) => dialogs::confirm(
                    app,
                    format!("Delete '{}'?", e.name),
                    if e.is_dir { "(recursively)" } else { "" }.to_string(),
                ),
                None => dialogs::alert(app, "Nothing selected to delete."),
            }
            Ok(false)
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            dialogs::clipboard_set(app, false);
            Ok(false)
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            dialogs::clipboard_set(app, true);
            Ok(false)
        }
        KeyCode::Char('p') => {
            dialogs::paste(app);
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn input_dialog(kind: DialogKind, message: &str, initial: String) -> Dialog {
    Dialog {
        kind,
        input: initial,
        cursor: 0,
        message: message.to_string(),
        detail: String::new(),
    }
}
