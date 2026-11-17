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
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "jsx" | "tsx" => "typescript",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "py" | "pyw" => "python",
        "md" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "shell",
        "sql" => "sql",
        "xml" | "svg" => "xml",
        "php" => "php",
        "txt" | "log" | "env" | "gitignore" => "plaintext",
        _ => "plaintext",
    }
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

fn is_binary_bytes(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    if sample.contains(&0) {
        return true;
    }
    std::str::from_utf8(bytes).is_err()
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
    if is_binary_bytes(&bytes) {
        return json!({
            "path": display_path,
            "name": name,
            "relativePath": relative_hint.unwrap_or(""),
            "binary": true,
            "size": meta.len(),
            "mtimeMs": mtime_ms(&meta),
            "error": "Binary or non-UTF-8 file cannot be edited as text."
        });
    }
    let raw_text = String::from_utf8_lossy(&bytes).to_string();
    let line_ending = detect_line_ending(&raw_text);
    let normalized_text = raw_text.replace("\r\n", "\n");
    json!({
        "path": display_path,
        "name": name,
        "relativePath": relative_hint.unwrap_or(""),
        "language": detect_language(target),
        "content": normalized_text,
        "lineEnding": if line_ending == "\r\n" { "CRLF" } else { "LF" },
        "encoding": "UTF-8",
        "binary": false,
        "size": meta.len(),
        "mtimeMs": mtime_ms(&meta),
        "hash": hash_bytes(&bytes),
        "readOnly": meta.permissions().readonly(),
        "error": Value::Null,
    })
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
