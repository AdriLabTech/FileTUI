//! Preview panel: renders a small text-file preview or basic metadata for
//! binary files. All filesystem reads happen here, never in render code.

use std::time::SystemTime;

use chrono::Local;

use crate::fsops::Entry;

/// Maximum number of bytes read for a text preview.
const MAX_PREVIEW_BYTES: usize = 4096;
/// How many bytes to sniff to decide "is this text?".
const SNIFF_BYTES: usize = 2048;
/// Caps for the fullscreen document viewer (read once when it opens).
const MAX_DOC_BYTES: usize = 1_048_576;
const MAX_DOC_LINES: usize = 50_000;

/// What [`build_document`] found in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// Text content: the viewer shows scrollable lines.
    Text,
    /// Anything else: the viewer shows metadata only.
    Binary,
}

/// Hint shown when an image cannot be rendered (no Kitty graphics support).
pub const IMAGE_HINT: [&str; 2] = [
    "Image preview needs the Kitty graphics",
    "protocol (kitty, WezTerm).",
];

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
    read_prefix_cap(entry, MAX_PREVIEW_BYTES)
}

/// Read at most `cap` bytes from the entry's path.
fn read_prefix_cap(entry: &Entry, cap: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(&entry.path)?;
    let cap = (cap as u64).min(entry.size.max(1)) as usize;
    let mut buf = vec![0u8; cap];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// Metadata plus the no-Kitty hint, used by the preview panel and by the
/// fullscreen viewer on terminals that cannot draw images.
pub fn image_fallback_lines(entry: &Entry) -> Vec<String> {
    let mut lines = metadata_lines(entry);
    lines.push(String::new());
    for hint in IMAGE_HINT {
        lines.push(hint.to_string());
    }
    lines
}

/// Full content for the fullscreen viewer, read once when it opens (never
/// per frame). Text files yield their lines; everything else yields
/// metadata so there is always something to show.
pub fn build_document(entry: &Entry) -> (DocKind, Vec<String>) {
    if entry.is_dir {
        return (DocKind::Binary, metadata_lines(entry));
    }

    let ext = entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if BINARY_EXT.contains(&ext.as_str()) {
        let mut lines = metadata_lines(entry);
        lines.push("Binary file — no preview.".into());
        lines.push("Open externally to inspect.".into());
        return (DocKind::Binary, lines);
    }

    match read_prefix_cap(entry, MAX_DOC_BYTES) {
        Ok(bytes) if !looks_binary(&bytes) => {
            let content = String::from_utf8_lossy(&bytes).into_owned();
            let mut out: Vec<String> = content
                .lines()
                .take(MAX_DOC_LINES)
                .map(str::to_string)
                .collect();
            if out.is_empty() {
                out.push("(empty file)".into());
            } else if out.len() >= MAX_DOC_LINES || content.len() >= MAX_DOC_BYTES {
                out.push("… (truncated)".into());
            }
            (DocKind::Text, out)
        }
        Ok(_) => {
            let mut lines = metadata_lines(entry);
            lines.push("Binary file — no preview.".into());
            lines.push("Open externally to inspect.".into());
            (DocKind::Binary, lines)
        }
        Err(e) => {
            let mut lines = metadata_lines(entry);
            lines.push("Could not read file.".into());
            lines.push(format!("<{}>", e));
            (DocKind::Binary, lines)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;

    use crate::fsops::list_dir;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("filetui-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn only_entry(dir: &Path) -> Entry {
        let listed = list_dir(dir, false).unwrap();
        assert_eq!(listed.len(), 1, "expected one entry in the test dir");
        listed[0].clone()
    }

    #[test]
    fn text_document_read_once_with_lines() {
        let dir = tmpdir("doctext");
        std::fs::write(dir.join("a.txt"), "first line\nsecond line\nlonger one\n").unwrap();
        let entry = only_entry(&dir);

        let (kind, lines) = build_document(&entry);
        assert_eq!(kind, DocKind::Text);
        assert_eq!(lines[0], "first line");
        assert!(lines.iter().any(|l| l.contains("second line")));
        assert!(lines.iter().any(|l| l.contains("longer one")));
        assert!(!lines.iter().any(|l| l.contains("(truncated)")));
    }

    #[test]
    fn binary_by_extension_is_metadata() {
        let dir = tmpdir("docbinaryext");
        std::fs::write(dir.join("a.png"), [0u8, 1, 2, 3]).unwrap();
        let entry = only_entry(&dir);

        let (kind, lines) = build_document(&entry);
        assert_eq!(kind, DocKind::Binary);
        assert!(lines.iter().any(|l| l.contains("Binary file")));
    }

    #[test]
    fn binary_sniffed_despite_text_extension() {
        let dir = tmpdir("docbinarysniff");
        let payload = vec![0u8; 64];
        std::fs::write(dir.join("odd.txt"), &payload).unwrap();
        let entry = only_entry(&dir);

        let (kind, lines) = build_document(&entry);
        assert_eq!(kind, DocKind::Binary);
        assert!(lines.iter().any(|l| l.contains("Binary file")));
    }

    #[test]
    fn text_caps_out_at_max_lines_with_marker() {
        let dir = tmpdir("doclinescap");
        let mut body = String::new();
        for _ in 0..(MAX_DOC_LINES + 5) {
            body.push_str("x\n");
        }
        std::fs::write(dir.join("big.txt"), body).unwrap();
        let entry = only_entry(&dir);

        let (kind, lines) = build_document(&entry);
        assert_eq!(kind, DocKind::Text);
        assert!(lines.iter().any(|l| l.contains("(truncated)")));
        assert!(
            lines.len() <= MAX_DOC_LINES + 5,
            "metadata + content + marker"
        );
    }

    #[test]
    fn image_fallback_lines_has_metadata_and_hint() {
        let dir = tmpdir("docimgfallback");
        std::fs::write(dir.join("a.png"), [0u8, 1, 2, 3]).unwrap();
        let entry = only_entry(&dir);

        let lines = image_fallback_lines(&entry);
        assert!(lines.iter().any(|l| l.contains("a.png")));
        for hint in IMAGE_HINT {
            assert!(
                lines.iter().any(|l| l.contains(hint)),
                "missing hint line: {hint}"
            );
        }
    }

    #[test]
    fn empty_text_file_yields_placeholder() {
        let dir = tmpdir("docempty");
        std::fs::write(dir.join("e.txt"), "").unwrap();
        let entry = only_entry(&dir);

        let (kind, lines) = build_document(&entry);
        assert_eq!(kind, DocKind::Text);
        assert!(lines.iter().any(|l| l.contains("(empty file)")));
    }
}
