//! filetui — a minimal, fast, three-panel file explorer for the terminal.
//!
//! Left panel: the parent directory. Center panel: the current directory
//! listing. Right panel: info + preview (text, or an image via the Kitty
//! graphics protocol). Modal dialogs handle input and destructive confirmations.

mod dialogs;
mod fsops;
mod img;
mod input;
mod preview;
mod state;
mod ui;

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event, execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use state::App;

fn main() -> io::Result<()> {
    // Allow an optional starting directory as a CLI argument.
    let start_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Enter the alternate screen, raw mode, and hide the cursor.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    execute!(stdout, crossterm::cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Restore the terminal no matter how we exit.
    let result = run(&mut terminal, start_dir);

    // Always restore the terminal, even on error.
    let restore = restore_terminal(&mut terminal);
    match result {
        Ok(()) => restore,
        Err(e) => {
            let _ = restore;
            Err(e)
        }
    }
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, crossterm::cursor::Show)?;
    execute!(out, LeaveAlternateScreen)?;
    out.flush()?;
    let _ = terminal;
    Ok(())
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    start_dir: PathBuf,
) -> io::Result<()> {
    let mut app = match App::new(start_dir) {
        Ok(a) => a,
        Err(e) => {
            let mut a = App::new(PathBuf::from(".")).expect("current dir must be readable");
            a.status = Some(format!("Could not open directory: {}", e));
            a
        }
    };

    let tick_rate = Duration::from_millis(100);
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        // Place a kitty image preview for the selection right after the frame
        // is drawn, so the graphic fills the otherwise-empty panel.
        if let Ok(size) = terminal.size() {
            let l = ui::layout(Rect::new(0, 0, size.width, size.height));
            let content = ui::preview_content_area(l.right);
            let _ = img::sync(&mut app, content);
        }

        // Exit if event polling fails (e.g. terminal closed).
        if event::poll(tick_rate)? {
            let event = event::read()?;
            if input::handle(&mut app, event)? {
                break;
            }
        }
    }
    Ok(())
}
