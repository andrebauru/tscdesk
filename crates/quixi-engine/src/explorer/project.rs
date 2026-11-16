//! Project marker detection for the Explorer.
//!
//! Detection is only a *suggestion* shown in the UI. A source is always an
//! independent context; markers never merge folders or import another project.

use std::path::Path;

pub const MARKERS: &[(&str, &str)] = &[
    (".git", "git"),
    ("Cargo.toml", "rust"),
    ("package.json", "node"),
    ("composer.json", "php"),
    ("pyproject.toml", "python"),
    ("go.mod", "go"),
    ("pom.xml", "maven"),
    ("CMakeLists.txt", "cmake"),
];

/// Returns the first recognised project marker kind for a folder, if any.
/// A `.sln` file is also accepted for .NET solutions.
pub fn detect(folder: &Path) -> Option<String> {
    if !folder.is_dir() {
        return None;
    }
    for (marker, kind) in MARKERS {
        if folder.join(marker).exists() {
            return Some((*kind).to_string());
        }
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.to_ascii_lowercase().ends_with(".sln") && entry.path().is_file() {
                return Some("dotnet".to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_project_markers_without_merging_anything() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect(root.path()), None);
        std::fs::write(root.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect(root.path()).as_deref(), Some("rust"));

        let php = tempfile::tempdir().unwrap();
        std::fs::write(php.path().join("composer.json"), "{}").unwrap();
        assert_eq!(detect(php.path()).as_deref(), Some("php"));

        let plain = tempfile::tempdir().unwrap();
        std::fs::create_dir(plain.path().join("src")).unwrap();
        assert_eq!(detect(plain.path()), None);
    }
}
