//! Integrated tabbed editor backend: safe workspace file I/O, binary/encoding
//! detection, atomic saves, conflict hashing, and per-workspace layout/draft persistence.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

const MAX_EDITOR_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditorTabState {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub relative_path: String,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub draft_content: Option<String>,
    #[serde(default)]
    pub base_hash: String,
    #[serde(default)]
    pub line_ending: String,
    #[serde(default)]
    pub scroll_top: f64,
    #[serde(default)]
    pub cursor_start: usize,
    #[serde(default)]
    pub cursor_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditorWorkspaceState {
    #[serde(default)]
    pub workspace_key: String,
    #[serde(default)]
    pub is_open: bool,
    #[serde(default)]
    pub layout_mode: String,
    #[serde(default)]
    pub width_px: u32,
    #[serde(default)]
    pub active_path: String,
    #[serde(default)]
    pub tabs: Vec<EditorTabState>,
    #[serde(default)]
    pub updated_at: u64,
}

pub struct EditorStore {
    dir: PathBuf,
    cache: RwLock<HashMap<String, EditorWorkspaceState>>,
}

impl EditorStore {
    pub fn new(data_dir: &Path) -> Self {
        let dir = data_dir.join("editor");
        let _ = fs::create_dir_all(&dir);
        Self {
            dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    fn state_file(&self, workspace_key: &str) -> PathBuf {
        let norm = if workspace_key.trim().is_empty() {
            "default"
        } else {
            workspace_key.trim()
        };
        let digest = format!("{:x}", Sha256::digest(norm.as_bytes()));
        self.dir.join(format!("{digest}.json"))
    }

    pub fn get_workspace_state(&self, workspace_key: &str) -> Value {
        let key = if workspace_key.trim().is_empty() {
            "default"
        } else {
            workspace_key.trim()
        };
        if let Some(st) = self.cache.read().get(key) {
            return serde_json::to_value(st).unwrap_or_else(|_| json!({}));
        }
        let file = self.state_file(key);
        let loaded = fs::read_to_string(&file)
            .ok()
            .and_then(|raw| serde_json::from_str::<EditorWorkspaceState>(&raw).ok())
            .unwrap_or_else(|| EditorWorkspaceState {
                workspace_key: key.to_string(),
                layout_mode: "split".to_string(),
                width_px: 520,
                ..Default::default()
            });
        self.cache.write().insert(key.to_string(), loaded.clone());
        serde_json::to_value(&loaded).unwrap_or_else(|_| json!({}))
    }

    pub fn save_workspace_state(&self, workspace_key: &str, body: &Value) -> Value {
        let key = if workspace_key.trim().is_empty() {
            "default"
        } else {
            workspace_key.trim()
        };
        let state_val = body.get("state").unwrap_or(body);
        let mut parsed: EditorWorkspaceState = match serde_json::from_value(state_val.clone()) {
            Ok(v) => v,
            Err(err) => return json!({ "error": format!("Invalid editor state: {err}") }),
        };
        parsed.workspace_key = key.to_string();
        if parsed.layout_mode.trim().is_empty() {
            parsed.layout_mode = "split".to_string();
        }
        if parsed.width_px == 0 {
            parsed.width_px = 520;
        }
        parsed.updated_at = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let _ = fs::create_dir_all(&self.dir);
        let file = self.state_file(key);
        let bytes = match serde_json::to_vec_pretty(&parsed) {
            Ok(b) => b,
            Err(err) => return json!({ "error": format!("Failed to serialize editor state: {err}") }),
        };
        if let Err(err) = crate::storage::atomic_write(&file, &bytes) {
            return json!({ "error": format!("Failed to persist editor state: {err}") });
        }
        self.cache.write().insert(key.to_string(), parsed.clone());
        json!({ "ok": true, "updatedAt": parsed.updated_at })
    }
}

pub(crate) fn clean_display_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
}

pub(crate) fn detect_language(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "dockerfile" {
        return "dockerfile";
    }
    if name == "makefile" {
        return "makefile";
    }
    if name == "cargo.toml" || name == "cargo.lock" {
        return "toml";
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "py" | "pyw" => "python",
        "md" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "shell",
        "ps1" | "psm1" | "psd1" => "powershell",
        "sql" => "sql",
        "xml" | "svg" => "xml",
        "php" => "php",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "txt" | "log" | "env" | "gitignore" => "plaintext",
        _ => "plaintext",
    }
}

fn detect_language_with_content(path: &Path, content: &str) -> &'static str {
    let by_ext = detect_language(path);
    if by_ext != "plaintext" {
        return by_ext;
    }
    let trimmed = content.trim_start();
    if trimmed.starts_with("#!/") || trimmed.starts_with("#! /") {
        let first_line = trimmed.lines().next().unwrap_or("");
        if first_line.contains("python") {
            return "python";
        }
        if first_line.contains("sh") || first_line.contains("bash") || first_line.contains("zsh") {
            return "shell";
        }
        if first_line.contains("node") {
            return "javascript";
        }
    }
    if trimmed.starts_with("<?php") {
        return "php";
    }
    if trimmed.starts_with("<?xml") {
        return "xml";
    }
    "plaintext"
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn mtime_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn has_nul_bytes(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

fn decode_text_strict(bytes: &[u8]) -> Result<(String, &'static str, bool), &'static str> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let body = std::str::from_utf8(&bytes[3..]).map_err(|_| "invalid-utf8")?;
        return Ok((body.to_string(), "UTF-8-BOM", true));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if bytes[2..].len() % 2 != 0 {
            return Err("invalid-utf16");
        }
        let s = String::from_utf16(&units).map_err(|_| "invalid-utf16")?;
        return Ok((s, "UTF-16LE", true));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        if bytes[2..].len() % 2 != 0 {
            return Err("invalid-utf16");
        }
        let s = String::from_utf16(&units).map_err(|_| "invalid-utf16")?;
        return Ok((s, "UTF-16BE", true));
    }
    if has_nul_bytes(bytes) {
        return Err("binary");
    }
    let s = std::str::from_utf8(bytes).map_err(|_| "invalid-utf8")?;
    Ok((s.to_string(), "UTF-8", false))
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    matches!(decode_text_strict(bytes), Err("binary" | "invalid-utf16"))
}

fn detect_line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn canonical_or_parent(path: &Path) -> Option<PathBuf> {
    if let Ok(p) = path.canonicalize() {
        return Some(p);
    }
    let name = path.file_name()?;
    let parent = canonical_or_parent(path.parent()?)?;
    Some(parent.join(name))
}

pub(crate) fn is_path_authorized(target: &Path, allowed_roots: &[String]) -> bool {
    let Some(resolved) = canonical_or_parent(target) else {
        return false;
    };
    allowed_roots
        .iter()
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| Path::new(r).canonicalize().ok())
        .any(|root| resolved.starts_with(root))
}

pub fn open_file(target: &Path, allowed_roots: &[String], relative_hint: Option<&str>) -> Value {
    if !is_path_authorized(target, allowed_roots) {
        return json!({
            "error": "Access denied: this file is outside the authorized workspace folders."
        });
    }
    let meta = match fs::metadata(target) {
        Ok(m) => m,
        Err(err) => return json!({ "error": format!("Cannot access file: {err}") }),
    };
    if !meta.is_file() {
        return json!({ "error": "Selected path is not a regular file." });
    }
    if meta.len() > MAX_EDITOR_BYTES {
        return json!({
            "error": format!("File is too large for the editor ({} bytes; limit is 5 MB).", meta.len())
        });
    }
    let bytes = match fs::read(target) {
        Ok(b) => b,
        Err(err) => return json!({ "error": format!("Could not read file: {err}") }),
    };
    let display_path = clean_display_path(target);
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    match decode_text_strict(&bytes) {
        Ok((raw_text, encoding, has_bom)) => {
            let line_ending = detect_line_ending(&raw_text);
            let normalized_text = raw_text.replace("\r\n", "\n");
            let lang = detect_language_with_content(target, &normalized_text);
            json!({
                "path": display_path,
                "name": name,
                "relativePath": relative_hint.unwrap_or(""),
                "language": lang,
                "content": normalized_text,
                "lineEnding": if line_ending == "\r\n" { "CRLF" } else { "LF" },
                "encoding": encoding,
                "hasBom": has_bom,
                "binary": false,
                "invalidEncoding": false,
                "size": meta.len(),
                "mtimeMs": mtime_ms(&meta),
                "hash": hash_bytes(&bytes),
                "readOnly": meta.permissions().readonly(),
                "error": Value::Null,
            })
        }
        Err("invalid-utf8") => {
            let latin1_preview: String = bytes.iter().map(|&b| b as char).collect();
            json!({
                "path": display_path,
                "name": name,
                "relativePath": relative_hint.unwrap_or(""),
                "language": detect_language(target),
                "content": latin1_preview.replace("\r\n", "\n"),
                "lineEnding": if latin1_preview.contains("\r\n") { "CRLF" } else { "LF" },
                "encoding": "ISO-8859-1 / non-UTF-8",
                "hasBom": false,
                "binary": false,
                "invalidEncoding": true,
                "size": meta.len(),
                "mtimeMs": mtime_ms(&meta),
                "hash": hash_bytes(&bytes),
                "readOnly": true,
                "error": "File contains non-UTF-8 byte sequences. Opened read-only to prevent silent corruption.",
            })
        }
        Err(_) => {
            json!({
                "path": display_path,
                "name": name,
                "relativePath": relative_hint.unwrap_or(""),
                "binary": true,
                "invalidEncoding": false,
                "size": meta.len(),
                "mtimeMs": mtime_ms(&meta),
                "error": "Binary file cannot be edited as text."
            })
        }
    }
}

pub fn save_file(
    target: &Path,
    allowed_roots: &[String],
    content: &str,
    line_ending: Option<&str>,
    expected_hash: Option<&str>,
    force: bool,
) -> Value {
    if !is_path_authorized(target, allowed_roots) {
        return json!({
            "error": "Access denied: cannot save outside the authorized workspace folders."
        });
    }
    if !force {
        if let Some(expected) = expected_hash.filter(|s| !s.trim().is_empty()) {
            if target.exists() {
                if let Ok(disk_bytes) = fs::read(target) {
                    let disk_hash = hash_bytes(&disk_bytes);
                    if disk_hash != expected {
                        let disk_text = String::from_utf8_lossy(&disk_bytes).replace("\r\n", "\n");
                        let mtime = fs::metadata(target).map(|m| mtime_ms(&m)).unwrap_or(0);
                        return json!({
                            "conflict": true,
                            "diskContent": disk_text,
                            "diskHash": disk_hash,
                            "diskMtimeMs": mtime,
                            "error": "File was modified on disk since it was opened."
                        });
                    }
                }
            }
        }
    }
    let use_crlf = matches!(line_ending, Some("CRLF" | "\r\n"));
    let normalized = content.replace("\r\n", "\n");
    let final_text = if use_crlf {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    };
    let bytes = final_text.as_bytes();
    if let Err(err) = crate::storage::atomic_write(target, bytes) {
        return json!({ "error": format!("Failed to save file: {err}") });
    }
    let meta = fs::metadata(target).ok();
    json!({
        "ok": true,
        "path": clean_display_path(target),
        "bytesWritten": bytes.len(),
        "size": meta.as_ref().map(|m| m.len()).unwrap_or(bytes.len() as u64),
        "mtimeMs": meta.as_ref().map(mtime_ms).unwrap_or(0),
        "hash": hash_bytes(bytes),
    })
}

pub fn stat_file(target: &Path, allowed_roots: &[String], include_content: bool) -> Value {
    if !is_path_authorized(target, allowed_roots) {
        return json!({ "error": "Access denied." });
    }
    let Ok(meta) = fs::metadata(target) else {
        return json!({
            "exists": false,
            "path": clean_display_path(target),
        });
    };
    if !meta.is_file() {
        return json!({
            "exists": false,
            "path": clean_display_path(target),
        });
    }
    let bytes = fs::read(target).ok();
    let hash = bytes.as_deref().map(hash_bytes).unwrap_or_default();
    let mut out = json!({
        "exists": true,
        "path": clean_display_path(target),
        "size": meta.len(),
        "mtimeMs": mtime_ms(&meta),
        "hash": hash,
    });
    if include_content {
        if let Some(raw_bytes) = bytes {
            if !is_binary_bytes(&raw_bytes) {
                let text = String::from_utf8_lossy(&raw_bytes);
                out["lineEnding"] = json!(if detect_line_ending(&text) == "\r\n" { "CRLF" } else { "LF" });
                out["content"] = json!(text.replace("\r\n", "\n"));
                out["binary"] = json!(false);
            } else {
                out["binary"] = json!(true);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_save_conflict_detection_and_persistence_work() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let file = ws.join("sample.rs");
        fs::write(&file, "fn main() {\r\n    println!(\"hi\");\r\n}\r\n").unwrap();
        let roots = vec![ws.to_string_lossy().to_string()];

        let opened = open_file(&file, &roots, Some("sample.rs"));
        assert_eq!(opened["binary"], false);
        assert_eq!(opened["language"], "rust");
        assert_eq!(opened["lineEnding"], "CRLF");
        assert_eq!(opened["content"], "fn main() {\n    println!(\"hi\");\n}\n");
        let h1 = opened["hash"].as_str().unwrap().to_string();

        // Simulate external modification
        fs::write(&file, "fn main() {\r\n    println!(\"external\");\r\n}\r\n").unwrap();
        let conflict = save_file(&file, &roots, "fn main() {}\n", Some("CRLF"), Some(&h1), false);
        assert_eq!(conflict["conflict"], true);

        // Force save preserves CRLF
        let saved = save_file(&file, &roots, "fn main() {\n    println!(\"manual\");\n}\n", Some("CRLF"), Some(&h1), true);
        assert_eq!(saved["ok"], true);
        let disk_raw = fs::read_to_string(&file).unwrap();
        assert!(disk_raw.contains("\r\n"));
        assert!(disk_raw.contains("manual"));

        // Binary detection
        let bin = ws.join("image.bin");
        fs::write(&bin, [0u8, 159, 146, 150]).unwrap();
        let bin_res = open_file(&bin, &roots, Some("image.bin"));
        assert_eq!(bin_res["binary"], true);

        // Workspace state persistence
        let store = EditorStore::new(dir.path());
        let saved_st = store.save_workspace_state(
            &ws.to_string_lossy(),
            &json!({
                "isOpen": true,
                "layoutMode": "split",
                "widthPx": 560,
                "activePath": clean_display_path(&file),
                "tabs": [{
                    "path": clean_display_path(&file),
                    "name": "sample.rs",
                    "dirty": true,
                    "draftContent": "unsaved edit",
                    "scrollTop": 42.0,
                    "cursorStart": 5,
                    "cursorEnd": 5
                }]
            }),
        );
        assert_eq!(saved_st["ok"], true);
        let loaded = store.get_workspace_state(&ws.to_string_lossy());
        assert_eq!(loaded["isOpen"], true);
        assert_eq!(loaded["widthPx"], 560);
        assert_eq!(loaded["tabs"][0]["draftContent"], "unsaved edit");
    }
}
