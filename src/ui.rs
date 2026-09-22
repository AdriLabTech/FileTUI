//! Rendering layer. This module owns all Ratatui concerns. Crucially it never
//! touches the filesystem: it only draws the state that navigation/input logic
//! has already prepared.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::fsops::Entry;
use crate::state::{App, Dialog, DialogKind, ViewerKind};
use crate::{img, preview};

const HIGHLIGHT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &mut App) {
    // The fullscreen viewer owns the whole screen while it is open.
    if app.viewer.is_some() {
        draw_viewer(f, app);
        return;
    }

    if app.show_help {
        draw_help(f);
        return;
    }

    let l = layout(f.area());

    draw_parent(f, l.parent, app);
    draw_center(f, l.center, app);
    draw_right(f, l.right, app);

    // Status bar at the bottom (full width).
    let status_text = app
        .status
        .clone()
        .unwrap_or_else(|| "h: help   .: hidden   q: quit".to_string());
    let status_span = Span::styled(
        truncate(&status_text, l.status.width as usize),
        Style::default().fg(Color::DarkGray),
    );
    f.render_widget(Paragraph::new(Line::from(status_span)), l.status);

    if app.dialog.is_some() {
        draw_dialog(f, app);
    }
}

/// Fullscreen viewer: text/document scrolling, image zoom/pan, or metadata.
/// The screen is cleared first; image viewers leave the interior empty so the
/// kitty graphic (placed by `img::sync` right after this frame) shows through.
fn draw_viewer(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Clear, area);

    let Some(viewer) = app.viewer.as_mut() else {
        return;
    };

    let hint = match viewer.kind {
        ViewerKind::Image => "→ zoom  ← unzoom/close  j/k/h/l pan  wheel zoom  Esc close",
        _ => "j/k/↑↓ scroll  PgUp/PgDn page  g/G ends  wheel scroll  Esc close",
    };

    let title = format!(
        "{} - {}",
        truncate(&viewer.name, area.width.saturating_sub(40) as usize),
        hint
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            format!(" {} ", hint),
            Style::default().fg(DIM),
        ));

    let inner = block.inner(area);
    // Keep the viewport height in sync for page-scrolling.
    viewer.page = inner.height as usize;
    viewer.scroll = viewer.scroll.min(viewer.max_scroll());

    match viewer.kind {
        // The graphic is painted by img::sync; the interior stays empty.
        // A placement error is surfaced as text instead.
        ViewerKind::Image if viewer.lines.is_empty() => {
            if let Some(err) = &app.img.error {
                let lines = vec![
                    Line::from(Span::styled(
                        "Could not render image:",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        truncate(err, inner.width.saturating_sub(2) as usize),
                        Style::default().fg(Color::Yellow),
                    )),
                ];
                f.render_widget(Paragraph::new(lines).block(block), area);
                return;
            }
            f.render_widget(block, area);
        }
        _ => {
            let scroll = viewer.scroll as u16;
            let lines: Vec<Line> = viewer
                .lines
                .iter()
                .map(|l| Line::from(Span::styled(l.clone(), Style::default())))
                .collect();
            f.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w <= max {
        s.to_string()
    } else {
        let mut out = String::new();
        let mut width = 0usize;
        for ch in s.chars() {
            let cw = UnicodeWidthStr::width(ch.to_string().as_str());
            if width + cw + 1 > max {
                out.push('…');
                break;
            }
            out.push(ch);
            width += cw;
        }
        out
    }
}

fn format_size(size: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if size < 1024 {
        return size.to_string();
    }
    let mut v = size as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{:.1}{}", v, UNITS[u])
}

/// Regions of the three-panel layout plus the status bar.
pub struct PanelLayout {
    pub parent: Rect,
    pub center: Rect,
    pub right: Rect,
    pub status: Rect,
}

/// Split the screen into the parent / current / preview columns and a status
/// row. Shared with `main` so the image placement uses exactly the same rects.
pub fn layout(area: Rect) -> PanelLayout {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(18),
            Constraint::Percentage(36),
            Constraint::Percentage(46),
        ])
        .split(outer[0]);
    PanelLayout {
        parent: cols[0],
        center: cols[1],
        right: cols[2],
        status: outer[1],
    }
}

/// The inner region of the preview panel (inside its borders).
pub fn preview_content_area(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// Left panel: the parent directory with the current directory highlighted.
fn draw_parent(f: &mut Frame, area: Rect, app: &App) {
    let title = app
        .parent
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.parent.display().to_string());
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" {} ", truncate(&title, area.width as usize)),
        Style::default().fg(HIGHLIGHT),
    ));

    if app.parent_entries.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "(nothing to show)",
                Style::default().fg(DIM),
            )))
            .block(block),
            area,
        );
        return;
    }

    let cwd_name: Option<String> = app
        .cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let current = cwd_name
        .as_deref()
        .and_then(|name| app.parent_entries.iter().position(|e| e.name == name));

    let header_row = Row::new([
        Cell::from(" "),
        Cell::from(Span::styled("Name", Style::default().fg(DIM))),
    ])
    .height(1);

    let rows: Vec<Row> = app
        .parent_entries
        .iter()
        .map(|entry| {
            let style = entry_style(entry);
            let name = if entry.is_dir {
                format!("▸ {}", entry.name)
            } else {
                entry.name.clone()
            };
            Row::new([Cell::from(" "), Cell::from(Span::styled(name, style))]).height(1)
        })
        .collect();

    let table = Table::new(rows, [Constraint::Length(1), Constraint::Min(1)])
        .header(header_row)
        .block(block)
        .row_highlight_style(
            Style::default()
                .bg(HIGHLIGHT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("›");

    let mut state = ratatui::widgets::TableState::default();
    state.select(current);
    f.render_stateful_widget(table, area, &mut state);
}

/// Center panel: the current directory listing.
fn draw_center(f: &mut Frame, area: Rect, app: &App) {
    let header_text = format!(
        " {} {}",
        app.cwd.display(),
        if app.show_hidden { " [hidden on]" } else { "" }
    );
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        truncate(&header_text, area.width as usize),
        Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD),
    ));

    if app.entries.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "(empty directory)",
                Style::default().fg(DIM),
            )))
            .block(block),
            area,
        );
        return;
    }

    let header_cells = [
        Cell::from(" "),
        Cell::from(Span::styled("Name", Style::default().fg(DIM))),
        Cell::from(Span::styled("Size", Style::default().fg(DIM))),
        Cell::from(Span::styled("Type", Style::default().fg(DIM))),
    ];
    let header_row = Row::new(header_cells).style(Style::default()).height(1);

    let rows: Vec<Row> = app
        .entries
        .iter()
        .map(|entry| {
            let style = entry_style(entry);
            let size = if entry.is_dir {
                "-".to_string()
            } else {
                format_size(entry.size)
            };
            let dtype = if entry.is_dir {
                "dir".to_string()
            } else if entry.is_symlink {
                "link".to_string()
            } else {
                entry
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_else(|| "file".to_string())
            };
            let name_prefix = if entry.is_dir { "▸ " } else { "  " };
            // The name cell is preceded by a spacer so text isn't glued to border.
            let mut cells = vec![
                Cell::from(Span::styled(
                    format!("{}{}", name_prefix, entry.name),
                    style,
                )),
                Cell::from(Span::styled(size, style)),
                Cell::from(Span::styled(dtype, Style::default().fg(DIM))),
            ];
            cells.insert(0, Cell::from(" "));
            Row::new(cells).height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(8),
            Constraint::Length(6),
        ],
    )
    .header(header_row)
    .block(block)
    .row_highlight_style(
        Style::default()
            .bg(HIGHLIGHT)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("›");

    let mut state = ratatui::widgets::TableState::default();
    state.select(Some(app.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn entry_style(entry: &Entry) -> Style {
    if entry.is_dir {
        Style::default()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD)
    } else if entry.is_symlink {
        Style::default().fg(Color::Magenta)
    } else {
        Style::default().fg(Color::White)
    }
}

fn draw_right(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(entry) = app.selected_entry().cloned() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Info ", Style::default().fg(HIGHLIGHT)));
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No selection",
                Style::default().fg(DIM),
            )))
            .block(block),
            area,
        );
        return;
    };

    // Images get a dedicated panel: the graphic is placed right after the
    // frame is drawn (see `img::sync`), so text leaves the picture visible.
    if img::is_image_path(&entry.path) {
        draw_image_panel(f, area, app, &entry);
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Info ", Style::default().fg(HIGHLIGHT)));

    // Refresh the preview cache only when the selection changed (path diff).
    let needs = app
        .preview_path
        .as_ref()
        .map(|p| *p != entry.path)
        .unwrap_or(true);
    if needs {
        app.preview_lines = preview::build_preview(&entry);
        app.preview_path = Some(entry.path.clone());
    }
    let lines = app.preview_lines.clone();
    let styled: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                Line::from(Span::styled(
                    l.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(l.clone(), Style::default()))
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(styled)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Right panel for an image selection.
fn draw_image_panel(f: &mut Frame, area: Rect, app: &App, entry: &Entry) {
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" {} ", truncate(&entry.name, area.width as usize)),
        Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD),
    ));

    // Non-kitty terminals can't show the image; fall back to metadata plus a
    // hint (shared with the fullscreen viewer) so the user knows why.
    if !img::kitty_supported() {
        let lines: Vec<Line> = preview::image_fallback_lines(entry)
            .into_iter()
            .map(Line::from)
            .collect();
        f.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    // A previous placement may have failed; surface the reason.
    if let Some(err) = &app.img.error {
        let lines = vec![
            Line::from(Span::styled(
                "Could not preview image:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                truncate(err, area.width.saturating_sub(4) as usize),
                Style::default().fg(Color::Yellow),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    // The image is placed by `img::sync` right after this frame, so the panel
    // interior is intentionally left empty.
    f.render_widget(block, area);
}

fn draw_help(f: &mut Frame) {
    let help = [
        "",
        "  Navigation",
        "    ↑ / ↓ or j / k        Move selection",
        "    Enter                 Enter directory",
        "    Backspace              Go to parent",
        "    Home / End             First / last entry",
        "",
        "  View",
        "    h                      Toggle this help",
        "    .                      Toggle hidden files",
        "    Enter or → on a file    Open the fullscreen viewer",
        "    (image selected)        Rendered via the Kitty graphics protocol",
        "    q                      Quit",
        "",
        "  Fullscreen viewer",
        "    Esc / q / Backspace     Close (text viewers: ← too)",
        "    →                       Zoom in (image)",
        "    ←                       Zoom out / close at fit (image)",
        "    j/k/↑↓                  Scroll (text) or pan (image)",
        "    h/l                     Pan left/right (image)",
        "    PgUp/PgDn, g/G          Page / jump to top/bottom (text)",
        "    mouse wheel             Scroll (text) or zoom (image)",
        "",
        "  Operations",
        "    n                      Create directory",
        "    N (Shift+n)            Create file",
        "    r                   Rename selected",
        "    d                   Delete (asks confirmation)",
        "    c                   Copy selected to clipboard",
        "    m                   Move selected (cut to clipboard)",
        "    p                   Paste clipboard into current directory",
        "",
        "  In dialogs",
        "    Enter                  Confirm",
        "    Esc                    Cancel / close",
        "    Backspace / Delete     Edit input",
        "",
        "  Press h to close help.",
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Help ", Style::default().fg(HIGHLIGHT)));
    let lines: Vec<Line> = help.iter().map(|s| Line::from(s.to_string())).collect();
    f.render_widget(Paragraph::new(lines).block(block), f.area());
}

fn draw_dialog(f: &mut Frame, app: &App) {
    let Some(dialog) = &app.dialog else { return };
    let area = centered_rect(60, 42, f.area());
    f.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        &dialog.message,
        Style::default().fg(Color::Yellow),
    )));
    if !dialog.detail.is_empty() {
        lines.push(Line::from(Span::styled(
            &dialog.detail,
            Style::default().fg(DIM),
        )));
    }
    lines.push(Line::from(""));

    match dialog.kind {
        DialogKind::Alert => {
            lines.push(Line::from(Span::styled(
                "  [Enter / Esc] OK",
                Style::default().fg(HIGHLIGHT),
            )));
        }
        DialogKind::ConfirmDelete => {
            lines.push(Line::from(Span::styled(
                "  This cannot be undone.",
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  [y] yes   [n / Esc] no",
                Style::default().fg(HIGHLIGHT),
            )));
        }
        DialogKind::CreateDir | DialogKind::CreateFile | DialogKind::Rename => {
            let label = match dialog.kind {
                DialogKind::CreateDir => "Name: ",
                DialogKind::CreateFile => "Name: ",
                DialogKind::Rename => "New name: ",
                _ => "",
            };
            lines.push(Line::from(Span::styled(
                format!("{}{}", label, dialog_display(dialog, area.width as usize)),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  [Enter] confirm   [Esc] cancel",
                Style::default().fg(DIM),
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {} ", dialog.title()),
            Style::default().fg(HIGHLIGHT),
        ))
        .style(Style::default().bg(Color::Black));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Display the input text with a block-cursor marker for the current position.
fn dialog_display(dialog: &Dialog, width: usize) -> String {
    let budget = width.saturating_sub(12);
    let budget = if budget < 4 { 12 } else { budget };
    let n = dialog.input.chars().count();
    let mut out = String::new();
    let mut shown = 0usize;
    for (i, ch) in dialog.input.chars().enumerate() {
        if i == dialog.cursor {
            out.push('█');
        }
        out.push(ch);
        shown += 1;
        if shown >= budget {
            out.push('…');
            break;
        }
    }
    // trailing cursor
    if dialog.cursor >= n && shown < budget {
        out.push('█');
    }
    out
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}
