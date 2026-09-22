//! Image preview for the right panel, using the Kitty graphics protocol.
//!
//! Images are pushed *after* each frame is drawn (see [`sync`]), placing a
//! "transmit to screen" graphic at the top-left of the preview content area.
//! Support is detected with a cheap env check; the terminal is never probed so
//! the raw-mode event loop can never block on a query.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::layout::Rect;

use crate::state::{App, ViewerKind};

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
/// only, so the raw-mode event loop never blocks on a query: kitty sets
/// `TERM=xterm-kitty` and `KITTY_WINDOW_ID`, wezterm sets `TERM_PROGRAM`.
pub fn kitty_supported() -> bool {
    let term = std::env::var("TERM").ok();
    let term_program = std::env::var("TERM_PROGRAM").ok();
    let kitty_window_id = std::env::var("KITTY_WINDOW_ID").ok();
    detect_kitty(
        term.as_deref(),
        term_program.as_deref(),
        kitty_window_id.as_deref(),
    )
}

/// Pure detection logic (testable without touching the environment).
fn detect_kitty(
    term: Option<&str>,
    term_program: Option<&str>,
    kitty_window_id: Option<&str>,
) -> bool {
    if kitty_window_id.is_some() {
        return true;
    }
    let has = |s: Option<&str>, needle: &str| {
        s.map(|v| v.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
    };
    has(term, "kitty") || has(term, "wezterm") || has(term_program, "wezterm")
}

/// Deduplication keys for the Kitty placement. Whenever the key is unchanged,
/// `sync` skips placing entirely — this is what prevents per-frame
/// decode/re-encode work (the original freeze).
#[derive(Debug, Clone, PartialEq)]
pub enum PlaceKey {
    /// The right-panel preview, fitted to (`w`, `h`) cells.
    Preview { path: PathBuf, w: u16, h: u16 },
    /// The fullscreen viewer, fitted to (`w`, `h`) with zoom/pan.
    Fullscreen {
        path: PathBuf,
        w: u16,
        h: u16,
        zoom: u32,
        pan: (i32, i32),
    },
}

/// A decoded RGBA image kept across frames so zoom/pan changes don't re-read
/// and re-decode the file. Invalidated by path or mtime changes.
#[derive(Clone)]
pub struct DecodedCache {
    pub path: PathBuf,
    mtime: Option<SystemTime>,
    pub img: image::RgbaImage,
}

impl fmt::Debug for DecodedCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedCache")
            .field("path", &self.path)
            .field("dims", &(self.img.width(), self.img.height()))
            .finish()
    }
}

/// Bookkeeping for the image currently shown (or attempted).
#[derive(Debug, Default, Clone)]
pub struct ImgState {
    /// Key of the current placement, if any (drives dedupe).
    pub key: Option<PlaceKey>,
    /// The single decoded image cached for the fullscreen viewer.
    pub cache: Option<DecodedCache>,
    /// Last placement error, shown in the panel when set.
    pub error: Option<String>,
}

impl ImgState {
    fn clear(&mut self) {
        *self = ImgState::default();
    }
}

/// Reconcile the on-screen image with the current state. Called from the
/// main loop right after the frame is drawn. Two placements exist:
/// - `preview` is the right panel's content area (driven by the selection);
/// - `screen` is the whole terminal, used only by the fullscreen viewer.
///
/// Failures are stored in `app.img.error` and never crash the app.
pub fn sync(app: &mut App, preview: Rect, screen: Rect) -> io::Result<()> {
    // Overlays cover the panels, so drop any image while they are open.
    if app.dialog.is_some() || app.show_help {
        if app.img.key.is_some() && kitty_supported() {
            delete_all()?;
        }
        app.img.clear();
        return Ok(());
    }

    let supported = kitty_supported();

    // A fullscreen viewer replaces everything else on screen.
    if let Some(viewer) = &app.viewer {
        let path = viewer.path.clone();
        let zoom = viewer.zoom;
        let pan = viewer.pan;
        // The viewer's block has 1-cell borders: the graphic goes inside.
        let interior = Rect::new(
            screen.x + 1,
            screen.y + 1,
            screen.width.saturating_sub(2),
            screen.height.saturating_sub(2),
        );
        let key = PlaceKey::Fullscreen {
            path: path.clone(),
            w: interior.width,
            h: interior.height,
            zoom,
            pan,
        };

        let want_image = viewer.kind == ViewerKind::Image && supported;
        let same = app.img.key.as_ref() == Some(&key);
        if !want_image || same {
            if same {
                return Ok(());
            }
            // Text/meta viewer (or a non-kitty image): nothing placed.
            if app.img.key.is_some() {
                if supported {
                    delete_all()?;
                }
                app.img.clear();
            }
            return Ok(());
        }

        // Leaving the fullscreen image: drop prior placement (keeps `cache`).
        if app.img.key.is_some() {
            delete_all()?;
            app.img.key = None;
            app.img.error = None;
        }
        if interior.width < 2 || interior.height < 2 {
            return Ok(());
        }

        match place_fullscreen(app, &path, interior, zoom, pan) {
            Ok(()) => {
                app.img.key = Some(key);
            }
            Err(e) => {
                // Record the attempted key too, so we don't retry every frame.
                app.img.key = Some(key);
                app.img.error = Some(e.to_string());
            }
        }
        return Ok(());
    }

    if matches!(app.img.key, Some(PlaceKey::Fullscreen { .. })) {
        if supported {
            delete_all()?;
        }
        app.img.clear();
    }

    sync_preview(app, preview, supported)
}

/// The right-panel preview: an image for image files, nothing otherwise.
fn sync_preview(app: &mut App, content: Rect, supported: bool) -> io::Result<()> {
    let target = if supported {
        app.selected_entry()
            .map(|e| e.path.clone())
            .filter(|p| is_image_path(p))
    } else {
        None
    };

    let key = target.as_ref().map(|path| PlaceKey::Preview {
        path: path.clone(),
        w: content.width,
        h: content.height,
    });

    if app.img.key.as_ref() == key.as_ref() {
        return Ok(());
    }

    let had_placement = app.img.key.is_some();
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
            app.img.key = Some(PlaceKey::Preview {
                path,
                w: content.width,
                h: content.height,
            });
        }
        Err(e) => {
            app.img.key = Some(PlaceKey::Preview {
                path,
                w: content.width,
                h: content.height,
            });
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

/// Cap on transmitted pixels: windows bigger than this are downscaled so a
/// single placement never stalls the frame loop for long.
const MAX_TX_PIXELS: u64 = 2_000_000;

/// Scale `(w, h)` down to fit inside `(cap_w, cap_h)`, preserving aspect, and
/// never grow. The result is also capped at [`MAX_TX_PIXELS`].
pub fn downscale_target(w: u32, h: u32, cap_w: u32, cap_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let mut nw = w as f64;
    let mut nh = h as f64;
    let scale = (cap_w as f64 / nw).min(cap_h as f64 / nh).min(1.0);
    nw = (nw * scale).round().max(1.0);
    nh = (nh * scale).round().max(1.0);
    let area = nw * nh;
    if area > MAX_TX_PIXELS as f64 {
        let s = (MAX_TX_PIXELS as f64 / area).sqrt();
        nw = (nw * s).round().max(1.0);
        nh = (nh * s).round().max(1.0);
    }
    (nw as u32, nh as u32)
}

/// Decode `path` into the RGBA cache unless an up-to-date entry already
/// exists. The cache survives across frames; it is only invalidated by a path
/// or mtime change, so zoom/pan never re-read the file.
fn ensure_cache(cache: &mut Option<DecodedCache>, path: &Path) -> io::Result<()> {
    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    if let Some(c) = cache {
        if c.path == path && c.mtime == mtime {
            return Ok(());
        }
    }
    let img = image::open(path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
        .to_rgba8();
    *cache = Some(DecodedCache {
        path: path.to_path_buf(),
        mtime,
        img,
    });
    Ok(())
}

/// The source-pixel window `(x, y, w, h)` shown at `zoom` with `pan`. The
/// window is centered by default; `pan` is in eighths of the window size.
pub fn visible_window(img_w: u32, img_h: u32, zoom: u32, pan: (i32, i32)) -> (u32, u32, u32, u32) {
    let zoom = zoom.max(1);
    let w = img_w.div_ceil(zoom);
    let h = img_h.div_ceil(zoom);
    let max_x = img_w.saturating_sub(w);
    let max_y = img_h.saturating_sub(h);
    let step_x = (w / 8).max(1) as i32;
    let step_y = (h / 8).max(1) as i32;
    let base_x = max_x as i32 / 2;
    let base_y = max_y as i32 / 2;
    let x = (base_x + pan.0.saturating_mul(step_x)).clamp(0, max_x as i32) as u32;
    let y = (base_y + pan.1.saturating_mul(step_y)).clamp(0, max_y as i32) as u32;
    (x, y, w, h)
}

/// Paint the fullscreen image: decode (cached), crop the visible window,
/// downscale to a sane pixel budget and re-encode as PNG. Always goes through
/// pixel work (never native streaming) so zoom/pan get the exact crop.
fn place_fullscreen(
    app: &mut App,
    path: &Path,
    screen: Rect,
    zoom: u32,
    pan: (i32, i32),
) -> io::Result<()> {
    ensure_cache(&mut app.img.cache, path)?;
    let Some(cache) = &app.img.cache else {
        return Err(io::Error::other("no image cache"));
    };

    let (x, y, w, h) = visible_window(cache.img.width(), cache.img.height(), zoom, pan);
    let crop = image::imageops::crop_imm(&cache.img, x, y, w, h).to_image();

    let (out_w, out_h) = downscale_target(w, h, screen.width as u32 * 4, screen.height as u32 * 4);
    let scaled = if (out_w, out_h) != (w, h) {
        image::imageops::resize(&crop, out_w, out_h, image::imageops::FilterType::Triangle)
    } else {
        crop
    };

    let mut png = Vec::new();
    scaled
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let (cols, rows) = fit_cells(out_w, out_h, screen.width as u32, screen.height as u32);
    if cols == 0 || rows == 0 {
        return Err(io::Error::other("screen too small"));
    }

    transmit(
        &png,
        Placed {
            format: 24,
            w_px: out_w,
            h_px: out_h,
            cols,
            rows,
        },
        screen.x,
        screen.y,
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

    #[test]
    fn kitty_matrix() {
        // kitty: TERM AND/OR KITTY_WINDOW_ID.
        assert!(detect_kitty(Some("xterm-kitty"), None, None));
        assert!(detect_kitty(None, None, Some("12")));
        // wezterm: TERM_PROGRAM.
        assert!(detect_kitty(Some("xterm-256color"), Some("WezTerm"), None));
        assert!(detect_kitty(None, Some("WezTerm"), None));
        // Plain terminal: no support.
        assert!(!detect_kitty(Some("xterm-256color"), None, None));
        assert!(!detect_kitty(None, None, None));
        // Overridden TERM inside kitty is safe: KITTY_WINDOW_ID still wins.
        assert!(detect_kitty(Some("xterm-256color"), None, Some("99")));
    }

    #[test]
    fn fullscreen_key_changes_with_zoom_and_pan() {
        let path = PathBuf::from("/a.png");
        let base = PlaceKey::Fullscreen {
            path: path.clone(),
            w: 80,
            h: 24,
            zoom: 1,
            pan: (0, 0),
        };
        let zoomed = PlaceKey::Fullscreen {
            path,
            w: 80,
            h: 24,
            zoom: 2,
            pan: (0, 0),
        };
        assert_ne!(base, zoomed);

        // The first line of the key stays the image path.
        match &base {
            PlaceKey::Fullscreen { path, .. } => assert_eq!(path, &PathBuf::from("/a.png")),
            _ => panic!("expected Fullscreen key"),
        }
    }

    #[test]
    fn downscale_never_grows_and_caps_pixels() {
        assert_eq!(downscale_target(400, 300, 800, 600), (400, 300));
        assert_eq!(downscale_target(1600, 1200, 800, 800), (800, 600));
        // Huge window -> capped by MAX_TX_PIXELS (~2 MPx).
        let (w, h) = downscale_target(10_000, 10_000, 320, 64);
        assert!(w as u64 * h as u64 <= MAX_TX_PIXELS);
        assert!(w <= 320 && h <= 64);
        // Degenerate inputs.
        assert_eq!(downscale_target(0, 10, 100, 100), (0, 0));
    }

    #[test]
    fn visible_window_at_zoom_one_is_the_full_image() {
        assert_eq!(visible_window(2000, 1000, 1, (0, 0)), (0, 0, 2000, 1000));
        assert_eq!(visible_window(2000, 1000, 1, (8, -8)), (0, 0, 2000, 1000));
    }

    #[test]
    fn visible_window_zooms_and_centers() {
        // Zoom 2 on 2000x1000 -> window 1000x500, centered -> x=500, y=250.
        assert_eq!(visible_window(2000, 1000, 2, (0, 0)), (500, 250, 1000, 500));
        // Pan +8 eighths moves to the far edge (max_x = 1000).
        assert_eq!(
            visible_window(2000, 1000, 2, (8, 0)),
            (1000, 250, 1000, 500)
        );
        // Pan out of range clamps back.
        assert_eq!(
            visible_window(2000, 1000, 2, (999, 0)),
            (1000, 250, 1000, 500)
        );
        assert_eq!(
            visible_window(2000, 1000, 2, (-999, 0)),
            (0, 250, 1000, 500)
        );
    }
}
