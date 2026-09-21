//! Preview panel: renders a small text-file preview or basic metadata for
//! binary files. All filesystem reads happen here, never in render code.

use std::time::SystemTime;

use chrono::Local;

use crate::fsops::Entry;

/// Maximum number of bytes read for a text preview.
const MAX_PREVIEW_BYTES: usize = 4096;
/// How many bytes to sniff to decide "is this text?".
const SNIFF_BYTES: usize = 2048;

/// Extensions we treat as binary without reading.
const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tiff", "mp3", "mp4", "avi", "mkv", "mov",
    "wav", "flac", "ogg", "m4a", "zip", "gz", "bz2", "xz", "7z", "rar", "tar", "pdf", "doc",
    "docx", "xls", "xlsx", "ppt", "pptx", "odt", "so", "o", "a", "bin", "exe", "dll", "class",
    "jar", "wasm", "deb", "rpm", "woff", "ttf", "otf", "eot",
];

fn fmt_size(size: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if size < 1024 {
        return format!("{} B", size);
    }
    let mut v = size as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", size)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

fn fmt_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => {
            let dt: chrono::DateTime<Local> = t.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }
        None => "unknown".to_string(),
    }
}

fn file_type_name(entry: &Entry) -> String {
    if entry.is_symlink {
        return "symbolic link".to_string();
    }
    entry
        .path
        .extension()
        .map(|e| format!("'{}' file", e.to_string_lossy().to_lowercase()))
        .unwrap_or_else(|| "file".to_string())
}

/// Decide whether `bytes` (first chunk of a file) look like binary data.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0x00)
}

/// Metadata lines for an entry, shared by the preview panel and the
/// image (non-kitty) view.
pub fn metadata_lines(entry: &Entry) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(entry.name.clone());
    lines.push(String::new());
    if entry.is_dir {
        lines.push("Type:        directory".into());
    } else {
        lines.push(format!("Size:        {}", fmt_size(entry.size)));
        lines.push(format!("Type:        {}", file_type_name(entry)));
    }
    lines.push(format!("Path:        {}", entry.path.display()));
    lines.push(format!("Permissions: {}", entry.permissions.symbolic));
    lines.push(format!("Modified:    {}", fmt_time(entry.modified)));
    lines.push(format!(
        "Symlink:     {}",
        if entry.is_symlink { "yes" } else { "no" }
    ));
    lines.push(String::new());
    lines
}

/// Build the lines shown in the preview panel. Lines may exceed the panel
/// width; the renderer wraps them.
pub fn build_preview(entry: &Entry) -> Vec<String> {
    let mut lines = metadata_lines(entry);

    if entry.is_dir {
        return lines;
    }

    // Decide whether to attempt a text/code preview.
    let ext = entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if BINARY_EXT.contains(&ext.as_str()) {
        lines.push("Binary file — no preview.".into());
        lines.push("Open externally to inspect.".into());
        return lines;
    }

    match read_prefix(entry) {
        Ok(bytes) if !looks_binary(&bytes) => {
            lines.push("── Preview ──────────────".into());
            let content = String::from_utf8_lossy(&bytes).into_owned();
            for (i, line) in content.lines().enumerate() {
                if i >= 40 {
                    lines.push("… (truncated)".into());
                    break;
                }
                lines.push(line.to_string());
            }
            if content.len() >= MAX_PREVIEW_BYTES {
                lines.push("… (truncated)".into());
            }
        }
        Ok(_) => {
            lines.push("Binary file — no preview.".into());
            lines.push("Open externally to inspect.".into());
        }
        Err(e) => {
            lines.push("Could not read file.".into());
            lines.push(format!("<{}>", e));
        }
    }

    lines
}

/// Read up to `MAX_PREVIEW_BYTES` bytes from the entry's path.
fn read_prefix(entry: &Entry) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(&entry.path)?;
    let cap = (MAX_PREVIEW_BYTES as u64).min(entry.size.max(1)) as usize;
    let mut buf = vec![0u8; cap];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}
