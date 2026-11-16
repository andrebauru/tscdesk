//! Safe directory listing and recursive walks for the Explorer.
//!
//! Every path is resolved component by component below an already canonical
//! source root. Absolute paths, `..`, Windows drive/ADS syntax and symlinks
//! that escape the root are rejected or reported, never followed.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".cache",
    "__pycache__",
    ".venv",
    ".next",
    ".turbo",
    ".gradle",
    ".idea",
    ".DS_Store",
];

pub const MAX_DIRECTORY_ENTRIES: usize = 500;
pub const MAX_WALK_FILES: usize = 4000;
pub const MAX_WALK_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    pub relative_path: String,
    pub is_dir: bool,
    pub size: u64,
    pub hidden: bool,
    pub ignored: bool,
    pub excluded: bool,
    pub symlink_escape: bool,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub show_hidden: bool,
    pub show_ignored: bool,
    pub extra_excludes: Vec<String>,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_ignored: false,
            extra_excludes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WalkedFile {
    pub relative_path: String,
    pub size: u64,
}

/// Normalises a UI-supplied relative path to forward slashes and rejects
/// traversal, absolute paths and platform-specific escapes.
pub fn normalize_rel(raw: &str) -> Result<String, String> {
    if raw.contains('\0') {
        return Err("Path contains a NUL byte.".into());
    }
    let replaced = raw.replace('\\', "/");
    let mut parts = Vec::new();
    for part in replaced.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err("Path traversal is not allowed.".into()),
            other => {
                let drive_prefixed = other.len() >= 2
                    && other.as_bytes()[0].is_ascii_alphabetic()
                    && other.as_bytes()[1] == b':';
                if drive_prefixed || (cfg!(windows) && other.contains(':')) {
                    return Err("Drive or alternate stream syntax is not allowed.".into());
                }
                parts.push(other);
            }
        }
    }
    Ok(parts.join("/"))
}

/// Joins `rel` below `root` and verifies that an existing target still lives
/// inside the canonical root (this also blocks symlink escapes).
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = normalize_rel(rel)?;
    let mut out = root.to_path_buf();
    if !rel.is_empty() {
        for part in rel.split('/') {
            out.push(part);
        }
    }
    if out.exists() {
        let root_canon = root
            .canonicalize()
            .map_err(|error| format!("The selected source is unavailable: {error}"))?;
        let target = out
            .canonicalize()
            .map_err(|error| format!("The path is unavailable: {error}"))?;
        if !target.starts_with(&root_canon) {
            return Err("The path escapes the selected source.".into());
        }
        return Ok(target);
    }
    Ok(out)
}

pub fn canonical_root(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Choose a folder or file first.".into());
    }
    if trimmed.contains('\0') {
        return Err("The path contains invalid characters.".into());
    }
    let path = PathBuf::from(trimmed);
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("The path is unavailable: {error}"))?;
    Ok(canonical)
}

pub fn is_hidden(path: &Path, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(metadata) = path.metadata() {
            return metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
        }
    }
    false
}

pub fn is_excluded(relative_path: &str, options: &ListOptions) -> bool {
    let rel = relative_path.replace('\\', "/");
    let lower = rel.to_ascii_lowercase();
    let mut names = lower.split('/');
    let base = names.next_back().unwrap_or("");
    let hit = |pattern: &str| -> bool {
        let pattern = pattern.trim().trim_matches('/').replace('\\', "/");
        if pattern.is_empty() {
            return false;
        }
        let pattern = pattern.to_ascii_lowercase();
        if pattern.contains('/') {
            lower == pattern || lower.starts_with(&(pattern.clone() + "/"))
        } else {
            base == pattern || lower.split('/').any(|part| part == pattern)
        }
    };
    DEFAULT_EXCLUDES.iter().any(|pattern| hit(pattern))
        || options.extra_excludes.iter().any(|pattern| hit(pattern))
}

pub struct IgnoreMatcher {
    gitignore: Option<Gitignore>,
}

impl IgnoreMatcher {
    pub fn new(root: &Path) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        let _ = builder.add(root.join(".gitignore"));
        Self {
            gitignore: builder.build().ok(),
        }
    }

    pub fn matches(&self, relative_path: &str, is_dir: bool) -> bool {
        let Some(gitignore) = &self.gitignore else {
            return false;
        };
        gitignore
            .matched_path_or_any_parents(Path::new(relative_path), is_dir)
            .is_ignore()
    }
}

fn entry_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Lists one directory level. Missing directories and unreadable paths return
/// an error instead of pretending the folder is empty.
pub fn list_directory(
    root: &Path,
    relative_dir: &str,
    options: &ListOptions,
) -> Result<Vec<TreeEntry>, String> {
    let rel_dir = normalize_rel(relative_dir)?;
    let dir = safe_join(root, &rel_dir)?;
    let metadata = std::fs::metadata(&dir).map_err(|error| format!("The folder is unavailable: {error}"))?;
    if !metadata.is_dir() {
        return Err("The selected path is not a folder.".into());
    }
    let matcher = IgnoreMatcher::new(root);
    let root_canon = root
        .canonicalize()
        .map_err(|error| format!("The selected source is unavailable: {error}"))?;
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|error| format!("The folder cannot be read: {error}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() {
            continue;
        }
        let path = entry.path();
        let rel = entry_relative(&rel_dir, &name);
        let file_type = match entry.file_type() {
            Ok(kind) => kind,
            Err(_) => continue,
        };
        let symlink = file_type.is_symlink();
        let mut symlink_escape = false;
        let mut is_dir = file_type.is_dir();
        let mut size = 0u64;
        if symlink {
            match path.canonicalize() {
                Ok(target) if target.starts_with(&root_canon) => {
                    if let Ok(meta) = std::fs::metadata(&target) {
                        is_dir = meta.is_dir();
                        size = meta.len();
                    }
                }
                _ => symlink_escape = true,
            }
        } else if let Ok(meta) = entry.metadata() {
            is_dir = meta.is_dir();
            size = meta.len();
        }
        if symlink_escape {
            continue;
        }
        let hidden = is_hidden(&path, &name);
        let excluded = is_excluded(&rel, options);
        let ignored = matcher.matches(&rel, is_dir);
        if !options.show_hidden && hidden {
            continue;
        }
        if !options.show_ignored && (excluded || ignored) {
            continue;
        }
        out.push(TreeEntry {
            name,
            relative_path: rel,
            is_dir,
            size,
            hidden,
            ignored,
            excluded,
            symlink_escape,
        });
        if out.len() >= MAX_DIRECTORY_ENTRIES {
            break;
        }
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// Walks a subtree and returns every allowed file. Symlinks are never
/// traversed; the file count is bounded so a huge tree cannot stall the UI.
pub fn walk_files(
    root: &Path,
    relative_dir: &str,
    options: &ListOptions,
    max_files: usize,
) -> Result<(Vec<WalkedFile>, bool), String> {
    let rel_dir = normalize_rel(relative_dir)?;
    let start = safe_join(root, &rel_dir)?;
    if !start.is_dir() {
        return Err("The selected path is not a folder.".into());
    }
    let matcher = IgnoreMatcher::new(root);
    let root_canon = root
        .canonicalize()
        .map_err(|error| format!("The selected source is unavailable: {error}"))?;
    let max_files = max_files.clamp(1, MAX_WALK_FILES);
    let mut files = Vec::new();
    let mut capped = false;
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(start, rel_dir, 0)];
    while let Some((dir, rel, depth)) = stack.pop() {
        if depth > MAX_WALK_DEPTH {
            capped = true;
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<_> = entries.flatten().collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.is_empty() {
                continue;
            }
            let path = entry.path();
            let child_rel = entry_relative(&rel, &name);
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let hidden = is_hidden(&path, &name);
            if !options.show_hidden && hidden {
                continue;
            }
            let is_dir = file_type.is_dir();
            let excluded = is_excluded(&child_rel, options);
            let ignored = matcher.matches(&child_rel, is_dir);
            if !options.show_ignored && (excluded || ignored) {
                continue;
            }
            if is_dir {
                stack.push((path, child_rel, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if !path.starts_with(&root_canon) {
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            files.push(WalkedFile {
                relative_path: child_rel,
                size,
            });
            if files.len() >= max_files {
                capped = true;
                return Ok((files, capped));
            }
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok((files, capped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_absolute_and_alt_stream_paths() {
        assert!(normalize_rel("../secret").is_err());
        assert!(normalize_rel("a/../../b").is_err());
        assert!(normalize_rel("C:/Windows").is_err());
        assert!(normalize_rel("a\\..\\b").is_err());
        assert_eq!(normalize_rel("./src//main.rs").unwrap(), "src/main.rs");
    }

    #[test]
    fn lists_special_names_and_marks_binary_sizes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("espaço & ção.txt"), "ok").unwrap();
        std::fs::write(root.path().join("a.bin"), [0u8, 1, 2]).unwrap();
        std::fs::create_dir(root.path().join("pasta")).unwrap();
        let entries = list_directory(root.path(), "", &ListOptions::default()).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|entry| entry.name == "espaço & ção.txt"));
        assert!(entries[0].is_dir);
    }

    #[test]
    fn hides_generated_directories_by_default_and_shows_them_on_request() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("node_modules")).unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join(".gitignore"), "secret.txt\n").unwrap();
        std::fs::write(root.path().join("secret.txt"), "x").unwrap();
        let hidden = list_directory(root.path(), "", &ListOptions::default()).unwrap();
        assert!(hidden.iter().all(|entry| entry.name != "node_modules"));
        assert!(hidden.iter().all(|entry| entry.name != "secret.txt"));
        let shown = list_directory(
            root.path(),
            "",
            &ListOptions { show_hidden: true, show_ignored: true, extra_excludes: vec![] },
        )
        .unwrap();
        assert!(shown.iter().any(|entry| entry.name == "node_modules" && entry.excluded));
        assert!(shown.iter().any(|entry| entry.name == "secret.txt" && entry.ignored));
    }

    #[test]
    fn walk_is_bounded_and_skips_generated_directories() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src/nested")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("src/nested/lib.rs"), "pub fn f() {}").unwrap();
        std::fs::create_dir(root.path().join("target")).unwrap();
        std::fs::write(root.path().join("target/big.bin"), "x").unwrap();
        let (files, capped) = walk_files(root.path(), "", &ListOptions::default(), 100).unwrap();
        assert!(!capped);
        let names: Vec<_> = files.iter().map(|file| file.relative_path.as_str()).collect();
        assert_eq!(names, vec!["src/main.rs", "src/nested/lib.rs"]);
        let (limited, capped) = walk_files(root.path(), "", &ListOptions::default(), 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert!(capped);
    }
}
