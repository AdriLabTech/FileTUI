//! Input handling: translates terminal key events into state transitions.
//! This is a pure logic layer; it calls filesystem helpers via `dialogs`.

use crossterm::event::{Event, KeyCode, KeyEvent, MouseEventKind};

use crate::dialogs;
use crate::state::{App, Dialog, DialogKind, Viewer, ViewerKind};
use crate::{img, preview};

/// Handle a terminal event. Returns `true` if the app should quit.
pub fn handle(app: &mut App, event: Event) -> std::io::Result<bool> {
    // The fullscreen viewer consumes every event while it is open (keys and
    // the mouse wheel alike). It can never quit the app.
    if app.viewer.is_some() {
        return handle_viewer(app, event);
    }

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

fn handle_viewer(app: &mut App, event: Event) -> std::io::Result<bool> {
    match event {
        Event::Mouse(mouse) => {
            // Wheel only reaches the app while the viewer is open (mouse
            // capture is toggled by main). Text scrolls, Images zoom.
            match mouse.kind {
                MouseEventKind::ScrollUp => viewer_wheel(app, -1),
                MouseEventKind::ScrollDown => viewer_wheel(app, 1),
                _ => {}
            }
            Ok(false)
        }
        Event::Key(key) => match key.code {
            // Close (never quit the app).
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => {
                app.close_viewer();
                Ok(false)
            }
            // `→` zooms in an image viewer; `←` zooms out, closing at fit.
            KeyCode::Right => {
                if app
                    .viewer
                    .as_ref()
                    .is_some_and(|v| v.kind == ViewerKind::Image)
                {
                    app.viewer.as_mut().unwrap().zoom_in();
                }
                Ok(false)
            }
            KeyCode::Left => {
                let close = match app.viewer.as_mut() {
                    Some(v) if v.kind == ViewerKind::Image && v.zoom > 1 => {
                        v.zoom_out();
                        false
                    }
                    Some(_) => true,
                    None => false,
                };
                if close {
                    app.close_viewer();
                }
                Ok(false)
            }
            KeyCode::Char('j') | KeyCode::Down => viewer_move(app, 1, true),
            KeyCode::Char('k') | KeyCode::Up => viewer_move(app, -1, true),
            KeyCode::PageDown => viewer_move(app, 1, false),
            KeyCode::PageUp => viewer_move(app, -1, false),
            KeyCode::Char('h') => {
                if app
                    .viewer
                    .as_ref()
                    .is_some_and(|v| v.kind == ViewerKind::Image)
                {
                    viewer_pan(app, -1, 0);
                }
                Ok(false)
            }
            KeyCode::Char('l') => {
                if app
                    .viewer
                    .as_ref()
                    .is_some_and(|v| v.kind == ViewerKind::Image)
                {
                    viewer_pan(app, 1, 0);
                }
                Ok(false)
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if let Some(v) = app.viewer.as_mut() {
                    v.scroll_top();
                }
                Ok(false)
            }
            KeyCode::End | KeyCode::Char('G') => {
                if let Some(v) = app.viewer.as_mut() {
                    v.scroll_bottom();
                }
                Ok(false)
            }
            _ => Ok(false),
        },
        _ => Ok(false),
    }
}

/// Vertical movement: scroll for text/meta lines, pan for a zoomed image.
fn viewer_move(app: &mut App, delta: isize, line_wise: bool) -> std::io::Result<bool> {
    match app.viewer.as_mut() {
        Some(v) if v.kind == ViewerKind::Image && v.zoom > 1 => {
            v.pan_by(0, delta as i32);
        }
        Some(v) => {
            if line_wise {
                v.scroll_by(delta);
            } else {
                let page = v.page.max(1) as isize;
                v.scroll_by(delta * page);
            }
        }
        None => {}
    }
    Ok(false)
}

/// Horizontal pan for a zoomed image.
fn viewer_pan(app: &mut App, dx: i32, dy: i32) {
    if let Some(v) = app.viewer.as_mut() {
        if v.kind == ViewerKind::Image && v.zoom > 1 {
            v.pan_by(dx, dy);
        }
    }
}

/// Mouse wheel: text/meta scrolls 3 lines per tick, an image zooms.
fn viewer_wheel(app: &mut App, delta_lines: isize) {
    match app.viewer.as_mut() {
        Some(v) if v.kind == ViewerKind::Image => {
            if delta_lines < 0 {
                v.zoom_in();
            } else {
                v.zoom_out();
            }
        }
        Some(v) => v.scroll_by(delta_lines * 3),
        None => {}
    }
}

/// Open the selected *file* in the fullscreen viewer. Images become an image
/// viewer on kitty terminals and metadata+ hint otherwise; other files use the
/// text/binary document split from `preview`.
fn open_viewer(app: &mut App) {
    let Some(entry) = app.selected_entry().cloned() else {
        return;
    };
    if entry.is_dir {
        return;
    }

    let (kind, lines) = if img::is_image_path(&entry.path) {
        if img::kitty_supported() {
            (ViewerKind::Image, Vec::new())
        } else {
            (ViewerKind::Meta, preview::image_fallback_lines(&entry))
        }
    } else {
        let (doc_kind, lines) = preview::build_document(&entry);
        let kind = match doc_kind {
            preview::DocKind::Text => ViewerKind::Text,
            preview::DocKind::Binary => ViewerKind::Meta,
        };
        (kind, lines)
    };
    app.open_viewer(Viewer::new(kind, entry.path, entry.name, lines));
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
            // Directories open; files open the fullscreen viewer.
            if let Some(entry) = app.selected_entry() {
                if entry.is_dir {
                    match app.enter_selected() {
                        Ok(_) => {}
                        Err(e) => {
                            dialogs::alert(app, format!("Could not enter directory: {}", e));
                        }
                    }
                } else {
                    open_viewer(app);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseEvent};
    use std::fs;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("filetui-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn wheel(kind: MouseEventKind) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn select(app: &mut App, name: &str) {
        let i = app.entries.iter().position(|e| e.name == name).unwrap();
        app.selected = i;
    }

    #[test]
    fn right_opens_viewer_for_files_and_enters_dirs() {
        let dir = tmpdir("inpfile");
        fs::write(dir.join("a.txt"), "hello world").unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        let mut app = App::new(dir).unwrap();

        select(&mut app, "a.txt");
        assert!(!handle(&mut app, key(KeyCode::Right)).unwrap());
        let v = app.viewer.as_ref().unwrap();
        assert_eq!(v.kind, ViewerKind::Text);
        assert!(v.lines.iter().any(|l| l.contains("hello world")));

        app.close_viewer();
        select(&mut app, "subdir");
        assert!(!handle(&mut app, key(KeyCode::Right)).unwrap());
        assert!(app.viewer.is_none());
        assert_eq!(app.cwd.file_name().unwrap().to_str().unwrap(), "subdir");
    }

    #[test]
    fn viewer_esc_q_backspace_close_without_quitting() {
        let dir = tmpdir("inpclose");
        fs::write(dir.join("a.txt"), "x").unwrap();
        let mut app = App::new(dir).unwrap();
        select(&mut app, "a.txt");
        handle(&mut app, key(KeyCode::Right)).unwrap();
        assert!(app.viewer.is_some());

        for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Backspace] {
            app.viewer = Some(Viewer::new(
                ViewerKind::Text,
                PathBuf::from("/a.txt"),
                "a.txt".into(),
                vec!["x".into()],
            ));
            // The viewer consumes the key and never quits the app.
            let quit = handle(&mut app, key(code)).unwrap();
            assert!(!quit);
            assert!(app.viewer.is_none(), "key {code:?} should close the viewer");
        }
    }

    #[test]
    fn arrow_keys_zoom_in_and_back_to_close() {
        let mut app = App::new(tmpdir("inpzoom")).unwrap();
        app.viewer = Some(Viewer::new(
            ViewerKind::Image,
            PathBuf::from("/a.png"),
            "a.png".into(),
            Vec::new(),
        ));

        handle(&mut app, key(KeyCode::Right)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().zoom, 2);
        handle(&mut app, key(KeyCode::Right)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().zoom, 4);

        // Left zooms out first, then closes once back at fit.
        handle(&mut app, key(KeyCode::Left)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().zoom, 2);
        assert!(app.viewer.is_some());
        handle(&mut app, key(KeyCode::Left)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().zoom, 1);
        assert!(app.viewer.is_some());
        handle(&mut app, key(KeyCode::Left)).unwrap();
        assert!(app.viewer.is_none());
    }

    #[test]
    fn keys_scroll_text_and_pan_zoomed_image() {
        let lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        let mut app = App::new(tmpdir("inpmove")).unwrap();
        app.viewer = Some(Viewer::new(
            ViewerKind::Text,
            PathBuf::from("/a.txt"),
            "a.txt".into(),
            lines,
        ));
        {
            let v = app.viewer.as_mut().unwrap();
            v.page = 10;
        }
        handle(&mut app, key(KeyCode::Down)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().scroll, 1);
        handle(&mut app, key(KeyCode::PageDown)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().scroll, 11);

        // Zoomed image: j/k/h pan rather than scroll.
        app.viewer = Some(Viewer::new(
            ViewerKind::Image,
            PathBuf::from("/a.png"),
            "a.png".into(),
            Vec::new(),
        ));
        handle(&mut app, key(KeyCode::Right)).unwrap(); // zoom 2
        handle(&mut app, key(KeyCode::Char('j'))).unwrap();
        handle(&mut app, key(KeyCode::Char('h'))).unwrap();
        {
            let v = app.viewer.as_ref().unwrap();
            assert_eq!(v.pan, (-1, 1));
            assert!(v.lines.is_empty());
        }
    }

    #[test]
    fn wheel_ignored_without_viewer_but_scrolls_and_zooms_with_it() {
        let mut app = App::new(tmpdir("inpwheel")).unwrap();
        // No viewer: the wheel event is swallowed.
        assert!(!handle(&mut app, wheel(MouseEventKind::ScrollDown)).unwrap());
        assert!(app.viewer.is_none());

        // Text viewer: 3 lines per wheel tick.
        let lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        app.viewer = Some(Viewer::new(
            ViewerKind::Text,
            PathBuf::from("/a.txt"),
            "a.txt".into(),
            lines,
        ));
        {
            let v = app.viewer.as_mut().unwrap();
            v.page = 10;
        }
        handle(&mut app, wheel(MouseEventKind::ScrollDown)).unwrap();
        handle(&mut app, wheel(MouseEventKind::ScrollDown)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().scroll, 6);
        handle(&mut app, wheel(MouseEventKind::ScrollUp)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().scroll, 3);

        // Image viewer: wheel zooms.
        app.viewer = Some(Viewer::new(
            ViewerKind::Image,
            PathBuf::from("/a.png"),
            "a.png".into(),
            Vec::new(),
        ));
        handle(&mut app, wheel(MouseEventKind::ScrollUp)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().zoom, 2);
        handle(&mut app, wheel(MouseEventKind::ScrollDown)).unwrap();
        assert_eq!(app.viewer.as_ref().unwrap().zoom, 1);
    }
}
