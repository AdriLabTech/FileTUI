//! File system operations, decoupled from rendering and UI concerns.
//!
//! All filesystem access happens through this module. The UI/state layers
//! call here and translate results into user-facing messages.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A single entry shown in the file list.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
    pub permissions: Permissions,
}

/// Parse-like, human-friendly permissions summary.
#[derive(Debug, Clone, Default)]
pub struct Permissions {
    /// e.g. "drwxr-xr-x". Empty if unknown.
    pub symbolic: String,
}

/// The result of an operation, carrying an optional user-facing message.
#[derive(Debug)]
pub struct OpResult {
    pub ok: bool,
    pub message: String,
}

impl OpResult {
    fn ok(msg: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: msg.into(),
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: msg.into(),
        }
    }
}

fn perm_symbolic(md: &fs::Metadata) -> String {
    let mode = md.permissions().mode();
    let mut s = String::with_capacity(10);
    s.push(if md.is_dir() {
        'd'
    } else if md.file_type().is_symlink() {
        'l'
    } else {
        '-'
    });
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o100 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o010 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o001 != 0 { 'x' } else { '-' });
    s
}

/// Build the `Entry` for a given path. Returns `None` if the path can't be read
/// (e.g. dangling symlink or permission error) so callers can skip it.
pub fn entry_for(path: &Path) -> Option<Entry> {
    // Follow the symlink to describe the target when possible, but remember we
    // are a symlink so the UI can annotate it.
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return None,
    };
    let symlink_meta = fs::symlink_metadata(path);
    let is_symlink = symlink_meta
        .as_ref()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);

    let size = if meta.is_dir() { 0 } else { meta.len() };

    let permissions = if let Ok(sym_meta) = symlink_meta {
        Permissions {
            symbolic: perm_symbolic(&sym_meta),
        }
    } else {
        Permissions::default()
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    Some(Entry {
        name,
        path: path.to_path_buf(),
        is_dir: meta.is_dir(),
        is_symlink,
        size,
        modified: meta.modified().ok(),
        permissions,
    })
}

/// List the contents of a directory. Returns entries sorted directories-first,
/// then alphabetically (case-insensitive), with hidden files included when
/// `show_hidden` is true.
pub fn list_dir(path: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let rd = fs::read_dir(path)?;
    for de in rd {
        let de = match de {
            Ok(d) => d,
            Err(_) => continue,
        };
        let name = de.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if name_str.starts_with('.') && !show_hidden {
            continue;
        }
        if let Some(e) = entry_for(&de.path()) {
            entries.push(e);
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Move up to the parent directory. Returns the new working directory.
pub fn parent_dir(path: &Path) -> PathBuf {
    path.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Create a directory under `base` with `name`.
pub fn create_dir(base: &Path, name: &str) -> OpResult {
    let name = name.trim();
    if name.is_empty() {
        return OpResult::err("Directory name is empty");
    }
    let p = base.join(name);
    if p.exists() {
        return OpResult::err(format!("'{}' already exists", name));
    }
    match fs::create_dir(&p) {
        Ok(_) => OpResult::ok(format!("Created directory '{}'", name)),
        Err(e) => OpResult::err(format!("Could not create directory: {}", e)),
    }
}

/// Create an empty file under `base` with `name`.
pub fn create_file(base: &Path, name: &str) -> OpResult {
    let name = name.trim();
    if name.is_empty() {
        return OpResult::err("File name is empty");
    }
    let p = base.join(name);
    if p.exists() {
        return OpResult::err(format!("'{}' already exists", name));
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(_) => OpResult::ok(format!("Created file '{}'", name)),
        Err(e) => OpResult::err(format!("Could not create file: {}", e)),
    }
}

/// Rename/move the entry `from` to the name `to` inside the same directory.
pub fn rename(from: &Path, to_name: &str) -> OpResult {
    let to_name = to_name.trim();
    if to_name.is_empty() {
        return OpResult::err("New name is empty");
    }
    let to = match from.parent() {
        Some(p) => p.join(to_name),
        None => PathBuf::from(to_name),
    };
    if to == from {
        return OpResult::ok("No change");
    }
    if to.exists() {
        return OpResult::err(format!("'{}' already exists", to_name));
    }
    match fs::rename(from, &to) {
        Ok(_) => OpResult::ok(format!("Renamed to '{}'", to_name)),
        Err(e) => OpResult::err(format!("Could not rename: {}", e)),
    }
}

/// Delete a file or directory. Directories are removed recursively (with care).
pub fn delete(path: &Path) -> OpResult {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return OpResult::err(format!("Could not access: {}", e)),
    };
    let result = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Ok(_) => OpResult::ok(format!(
            "Deleted '{}'",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        Err(e) => OpResult::err(format!("Could not delete: {}", e)),
    }
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = fs::metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for de in fs::read_dir(from)? {
            let de = de?;
            let dest = to.join(de.file_name());
            if de.metadata()?.is_dir() {
                copy_recursive(&de.path(), &dest)?;
            } else {
                fs::copy(de.path(), &dest)?;
            }
        }
        // Best-effort copy of mtime for the directory itself.
        let ft = filetime::FileTime::from_last_modification_time(&meta);
        let _ = filetime::set_file_mtime(to, ft);
        Ok(())
    } else {
        fs::copy(from, to)?;
        Ok(())
    }
}

fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let mut candidate = dir.join(name);
    let mut i = 1;
    while candidate.exists() {
        let stem = Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = Path::new(name)
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let new_name = if !ext.is_empty() {
            format!("{}.copy{}.{}", stem, i, ext)
        } else {
            format!("{}.copy{}", stem, i)
        };
        candidate = dir.join(new_name);
        i += 1;
    }
    candidate
}

/// Copy an entry into `dest_dir`.
pub fn copy_into(src: &Path, dest_dir: &Path) -> OpResult {
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let to = unique_dest(dest_dir, &name);
    match copy_recursive(src, &to) {
        Ok(_) => OpResult::ok(format!(
            "Copied to '{}'",
            to.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        Err(e) => OpResult::err(format!("Could not copy: {}", e)),
    }
}

/// Move an entry into `dest_dir` (falling back to copy+delete across devices).
pub fn move_into(src: &Path, dest_dir: &Path) -> OpResult {
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let to = unique_dest(dest_dir, &name);
    match fs::rename(src, &to) {
        Ok(_) => OpResult::ok(format!(
            "Moved to '{}'",
            to.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        Err(e) => {
            // Cross-device or fallback: copy then delete.
            match copy_recursive(src, &to) {
                Ok(_) => {
                    let _ = delete(src);
                    OpResult::ok(format!(
                        "Moved to '{}'",
                        to.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ))
                }
                Err(_) => OpResult::err(format!("Could not move: {}", e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("filetui-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn list_sorts_dirs_first_and_filters_hidden() {
        let dir = tmpdir("list");
        fs::write(dir.join("b.txt"), "x").unwrap();
        fs::write(dir.join("a.txt"), "x").unwrap();
        fs::create_dir(dir.join("adir")).unwrap();
        fs::write(dir.join(".hidden"), "x").unwrap();

        let listed = list_dir(&dir, false).unwrap();
        assert!(listed.first().unwrap().is_dir);
        let names: Vec<&str> = listed.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["adir", "a.txt", "b.txt"]);

        let listed_all = list_dir(&dir, true).unwrap();
        let names_all: Vec<&str> = listed_all.iter().map(|e| e.name.as_str()).collect();
        assert!(names_all.contains(&".hidden"));
    }

    #[test]
    fn symlinks_are_marked() {
        let dir = tmpdir("symlink");
        fs::write(dir.join("target.txt"), "hello").unwrap();
        std::os::unix::fs::symlink(dir.join("target.txt"), dir.join("link.txt")).unwrap();
        let listed = list_dir(&dir, true).unwrap();
        let link = listed.iter().find(|e| e.name == "link.txt").unwrap();
        assert!(link.is_symlink);
        assert!(!link.is_dir);
        assert_eq!(link.size, 5);
    }

    #[test]
    fn create_ops_require_unique_names() {
        let dir = tmpdir("createunique");
        assert!(create_dir(&dir, "newdir").ok);
        assert!(create_file(&dir, "newfile.txt").ok);
        assert!(!create_dir(&dir, "newdir").ok);
        assert!(!create_file(&dir, "newfile.txt").ok);
        assert!(!create_dir(&dir, "   ").ok);
    }

    #[test]
    fn rename_and_delete() {
        let dir = tmpdir("renamedel");
        fs::write(dir.join("a.txt"), "x").unwrap();
        assert!(rename(&dir.join("a.txt"), "b.txt").ok);
        assert!(dir.join("b.txt").exists());
        assert!(!dir.join("a.txt").exists());

        // Rename onto an existing entry must fail.
        fs::write(dir.join("c.txt"), "y").unwrap();
        assert!(!rename(&dir.join("b.txt"), "c.txt").ok);

        // Same name is a no-op and reported as ok.
        assert!(rename(&dir.join("b.txt"), "b.txt").ok);

        assert!(delete(&dir.join("b.txt")).ok);
        assert!(!dir.join("b.txt").exists());
        assert!(delete(&dir.join("c.txt")).ok);
    }

    #[test]
    fn copy_and_move_recursive() {
        let dir = tmpdir("copymove");
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/f.txt"), "data").unwrap();

        fs::create_dir(dir.join("dest")).unwrap();
        assert!(copy_into(&dir.join("sub"), &dir.join("dest")).ok);
        assert!(dir.join("dest/sub/f.txt").exists());

        // Copy again should create a .copy variant, not overwrite.
        assert!(copy_into(&dir.join("sub"), &dir.join("dest")).ok);
        let dest_entries = fs::read_dir(dir.join("dest")).unwrap().count();
        assert_eq!(dest_entries, 2);

        assert!(move_into(&dir.join("sub"), &dir.join("dest")).ok);
        assert!(!dir.join("sub").exists());
        assert!(dir.join("dest/sub").exists());
    }

    #[test]
    fn delete_nonexistent_fails_gracefully() {
        let dir = tmpdir("delmissing");
        let res = delete(&dir.join("nope"));
        assert!(!res.ok);
    }

    #[test]
    fn parent_chain_stops_at_root() {
        let p = PathBuf::from("/");
        assert_eq!(parent_dir(&p), p);
        let q = PathBuf::from("/usr/bin");
        assert_eq!(parent_dir(&q), PathBuf::from("/usr"));
    }
}
