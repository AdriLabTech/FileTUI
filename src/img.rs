//! Image preview for the right panel, using the Kitty graphics protocol.
//!
//! Images are pushed *after* each frame is drawn (see [`sync`]), placing a
//! "transmit to screen" graphic at the top-left of the preview content area.
//! Support is detected with a cheap env check; the terminal is never probed so
//! the raw-mode event loop can never block on a query.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

use crate::state::App;

/// Extensions treated as previewable images.
pub const IMAGE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico",
];

/// Extensions the terminal can decode natively (kitty `f=100` autodetect).
fn kitty_native(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif")
}

/// Whether `path` has a previewable image extension.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Whether this terminal speaks the Kitty graphics protocol. Cheap env check
/// only: kitty sets `TERM=xterm-kitty`.
pub fn kitty_supported() -> bool {
    std::env::var("TERM")
        .map(|t| t.to_ascii_lowercase().contains("kitty"))
        .unwrap_or(false)
}

/// Bookkeeping for the image currently shown (or attempted).
#[derive(Debug, Default, Clone)]
pub struct ImgState {
    /// Path of the entry that produced the current placement.
    pub placed_path: Option<PathBuf>,
    /// Content-area size the current placement was fitted to.
    pub placed_wh: Option<(u16, u16)>,
    /// Last placement error, shown in the panel when set.
    pub error: Option<String>,
}

impl ImgState {
    fn clear(&mut self) {
        *self = ImgState::default();
    }
}

/// Reconcile the on-screen image with the current selection. Called from the
/// main loop right after the frame is drawn; `content` is the right panel's
/// content area. Failures are stored in `app.img.error` and never crash the
/// app.
pub fn sync(app: &mut App, content: Rect) -> io::Result<()> {
    // Overlays cover the panel, so drop any image while they are open.
    if app.dialog.is_some() || app.show_help {
        if app.img.placed_path.is_some() && kitty_supported() {
            delete_all()?;
        }
        app.img.clear();
        return Ok(());
    }

    let supported = kitty_supported();
    let target = if supported {
        app.selected_entry()
            .map(|e| e.path.clone())
            .filter(|p| is_image_path(p))
    } else {
        None
    };

    let same_target = match (&app.img.placed_path, &target) {
        (Some(prev), Some(next)) => {
            prev == next && app.img.placed_wh == Some((content.width, content.height))
        }
        (None, None) => true,
        _ => false,
    };
    if same_target {
        return Ok(());
    }

    let had_placement = app.img.placed_path.is_some();
    app.img.clear();
    if had_placement && supported {
        delete_all()?;
    }

    let Some(path) = target else {
        return Ok(());
    };
    if content.width < 2 || content.height < 2 {
        return Ok(());
    }

    match place_image(&path, content) {
        Ok(()) => {
            app.img.placed_path = Some(path);
            app.img.placed_wh = Some((content.width, content.height));
        }
        Err(e) => {
            app.img.placed_path = Some(path);
            app.img.placed_wh = Some((content.width, content.height));
            app.img.error = Some(e.to_string());
        }
    }
    Ok(())
}

/// Remove every visible placement and free the transmitted image data.
fn delete_all() -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\")?;
    out.flush()
}

/// Decode the picture's dimensions, fit it into the area (cells, half-block
/// model) and transmit it to the terminal.
fn place_image(path: &Path, area: Rect) -> io::Result<()> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
        .with_guessed_format()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let (w_px, h_px) = reader
        .into_dimensions()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let (cols, rows) = fit_cells(w_px, h_px, area.width as u32, area.height as u32);
    if cols == 0 || rows == 0 {
        return Err(io::Error::other("preview area too small"));
    }

    // Let kitty autodetect the format (`f=100`) for natively supported files
    // (streams the original bytes). Everything else is decoded and re-encoded
    // to PNG (`f=24`) so the terminal always receives something it can show.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let (data, format) = if kitty_native(&ext) {
        (std::fs::read(path)?, 100)
    } else {
        let img = image::open(path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        (png, 24)
    };

    transmit(
        &data,
        Placed {
            format,
            w_px,
            h_px,
            cols,
            rows,
        },
        area.x,
        area.y,
    )
}

/// What to tell the terminal about the placed picture.
struct Placed {
    format: u8,
    w_px: u32,
    h_px: u32,
    cols: u32,
    rows: u32,
}

/// Place the graphic cursor at the top-left cell of the area and stream the
/// base64-encoded image in ≤4096-char chunks (kitty's per-command cap).
fn transmit(data: &[u8], placed: Placed, x: u16, y: u16) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b[{};{}H", y + 1, x + 1)?;

    const CHUNK: usize = 4096;
    let payload = base64(data);
    let bytes = payload.as_bytes();
    let mut i = 0;
    let mut first = true;
    while i < bytes.len() {
        let end = (i + CHUNK).min(bytes.len());
        let last = end == bytes.len();
        let chunk = std::str::from_utf8(&bytes[i..end]).expect("base64 is ASCII");
        if first {
            write!(
                out,
                "\x1b_Gf={},a=T,t=d,s={},v={},c={},r={},m={};{}\x1b\\",
                placed.format,
                placed.w_px,
                placed.h_px,
                placed.cols,
                placed.rows,
                if last { 0 } else { 1 },
                chunk
            )?;
            first = false;
        } else {
            write!(out, "\x1b_Gm={};{}\x1b\\", if last { 0 } else { 1 }, chunk)?;
        }
        i = end;
    }
    out.flush()
}

/// Fit a picture into a cell area, assuming ~1:2 cell aspect (half-block
/// model) so on-screen proportions are preserved when kitty scales.
fn fit_cells(w_px: u32, h_px: u32, area_w: u32, area_h: u32) -> (u32, u32) {
    if w_px == 0 || h_px == 0 || area_w == 0 || area_h == 0 {
        return (0, 0);
    }
    let native_rows = h_px.div_ceil(2);
    let scale = (w_px as f32 / area_w as f32).max(native_rows as f32 / area_h as f32);
    if scale <= 1.0 {
        (w_px.max(1), native_rows.max(1))
    } else {
        (
            (w_px as f32 / scale).round().max(1.0) as u32,
            (native_rows as f32 / scale).round().max(1.0) as u32,
        )
    }
}

/// Standard RFC 4648 base64.
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_well_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_length_property() {
        for len in [0usize, 1, 2, 3, 4, 7, 255, 4096, 4097, 8192] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            assert_eq!(base64(&data).len(), len.div_ceil(3) * 4);
        }
    }

    #[test]
    fn fit_respects_area() {
        // Larger than the area -> scaled to fit both axes.
        assert_eq!(fit_cells(800, 600, 40, 20), (40, 15));
        // Smaller than the area -> native size in half-block cells.
        assert_eq!(fit_cells(40, 25, 60, 20), (40, 13));
        // Degenerate inputs produce no placement.
        assert_eq!(fit_cells(0, 10, 40, 20), (0, 0));
        assert_eq!(fit_cells(10, 10, 0, 20), (0, 0));
    }
}
