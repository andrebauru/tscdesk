//! Bounded, safe text reading for the Explorer.
//!
//! Files are read through a `Take` limit so an oversized file never lands in
//! memory in full. Binary content is reported, never decoded as text.

use serde::Serialize;
use std::io::Read;
use std::path::Path;

use super::tree::safe_join;

pub const DEFAULT_MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub relative_path: String,
    pub content: Option<String>,
    pub encoding: String,
    pub binary: bool,
    pub truncated: bool,
    pub bytes_read: u64,
    pub error: Option<String>,
}

impl FilePreview {
    fn failed(relative_path: &str, error: impl Into<String>) -> Self {
        Self {
            relative_path: relative_path.to_string(),
            content: None,
            encoding: "unknown".into(),
            binary: false,
            truncated: false,
            bytes_read: 0,
            error: Some(error.into()),
        }
    }
}

/// Reads at most `max_bytes` of a text file. The limit is clamped so a caller
/// cannot request an unbounded preview.
pub fn read_file(root: &Path, relative_path: &str, max_bytes: u64) -> FilePreview {
    let max_bytes = max_bytes.clamp(1, MAX_PREVIEW_BYTES);
    let path = match safe_join(root, relative_path) {
        Ok(path) => path,
        Err(error) => return FilePreview::failed(relative_path, error),
    };
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => return FilePreview::failed(relative_path, format!("The file is unavailable: {error}")),
    };
    if metadata.is_dir() {
        return FilePreview::failed(relative_path, "The selected path is a folder, not a file.");
    }
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => return FilePreview::failed(relative_path, format!("The file cannot be read: {error}")),
    };
    let mut buffer = Vec::new();
    if let Err(error) = file.take(max_bytes + 1).read_to_end(&mut buffer) {
        return FilePreview::failed(relative_path, format!("The file cannot be read: {error}"));
    }
    let truncated = buffer.len() as u64 > max_bytes;
    buffer.truncate(max_bytes as usize);
    let bytes_read = buffer.len() as u64;
    if is_probably_binary(&buffer) {
        return FilePreview {
            relative_path: relative_path.to_string(),
            content: None,
            encoding: "binary".into(),
            binary: true,
            truncated,
            bytes_read,
            error: Some("Binary file: content is not sent to the model.".into()),
        };
    }
    let (content, encoding) = decode(&buffer);
    FilePreview {
        relative_path: relative_path.to_string(),
        content: Some(content),
        encoding,
        binary: false,
        truncated,
        bytes_read,
        error: None,
    }
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    if bytes.contains(&0) {
        // UTF-16 documents legitimately contain NUL bytes; recognise their BOM
        // before declaring the file binary.
        if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
            return false;
        }
        return true;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    let control = sample
        .iter()
        .filter(|byte| {
            let byte = **byte;
            byte < 0x09 || (byte > 0x0D && byte < 0x20) || byte == 0x7F
        })
        .count();
    control * 100 / sample.len().max(1) > 30
}

fn decode(bytes: &[u8]) -> (String, String) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).to_string(), "utf-8-bom".into());
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (decode_utf16(rest, true), "utf-16le".into());
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (decode_utf16(rest, false), "utf-16be".into());
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_string(), "utf-8".into()),
        Err(_) => (String::from_utf8_lossy(bytes).to_string(), "utf-8-lossy".into()),
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_utf8_with_accents_and_keeps_content_exact() {
        let root = tempfile::tempdir().unwrap();
        let text = "linha 1 — ação, coração, 日本語\nlinha 2\n";
        std::fs::write(root.path().join("acentos.txt"), text).unwrap();
        let preview = read_file(root.path(), "acentos.txt", DEFAULT_MAX_FILE_BYTES);
        assert!(!preview.binary);
        assert_eq!(preview.encoding, "utf-8");
        assert_eq!(preview.content.as_deref(), Some(text));
    }

    #[test]
    fn detects_utf16_bom_and_reports_binary_files() {
        let root = tempfile::tempdir().unwrap();
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "hello".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(root.path().join("utf16.txt"), utf16).unwrap();
        let preview = read_file(root.path(), "utf16.txt", DEFAULT_MAX_FILE_BYTES);
        assert_eq!(preview.encoding, "utf-16le");
        assert_eq!(preview.content.as_deref(), Some("hello"));

        std::fs::write(root.path().join("image.bin"), [0x89, 0x50, 0x4E, 0x47, 0, 0, 0]).unwrap();
        let binary = read_file(root.path(), "image.bin", DEFAULT_MAX_FILE_BYTES);
        assert!(binary.binary);
        assert!(binary.content.is_none());
    }

    #[test]
    fn truncates_large_files_without_reading_them_whole() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("big.txt"), "a".repeat(5000)).unwrap();
        let preview = read_file(root.path(), "big.txt", 100);
        assert!(preview.truncated);
        assert_eq!(preview.bytes_read, 100);
        assert_eq!(preview.content.as_deref().map(str::len), Some(100));
    }

    #[test]
    fn refuses_paths_outside_the_source_and_folders() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("dir")).unwrap();
        let traversal = read_file(root.path(), "../secret.txt", DEFAULT_MAX_FILE_BYTES);
        assert!(traversal.error.is_some());
        let folder = read_file(root.path(), "dir", DEFAULT_MAX_FILE_BYTES);
        assert!(folder.error.unwrap().contains("folder"));
    }

    #[cfg(windows)]
    #[test]
    fn reports_an_os_access_denial_instead_of_panicking() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("locked.txt"), "private").unwrap();
        let _lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.path().join("locked.txt"))
            .unwrap();
        let preview = read_file(root.path(), "locked.txt", DEFAULT_MAX_FILE_BYTES);
        assert!(preview.content.is_none());
        assert!(preview.error.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn reports_a_unix_permission_denial_instead_of_panicking() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("locked.txt");
        std::fs::write(&file, "private").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&file, permissions).unwrap();
        let preview = read_file(root.path(), "locked.txt", DEFAULT_MAX_FILE_BYTES);
        // Root in a container can still read the file; the contract is that a
        // denial is reported, not that the test process is unprivileged.
        if preview.content.is_none() {
            assert!(preview.error.is_some());
        }
    }
}
