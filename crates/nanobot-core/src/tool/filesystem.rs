use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Tool;

fn resolve_path(path: &str, allowed_dir: Option<&Path>) -> Result<PathBuf, String> {
    let expanded = if path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            home.join(&path[2..])
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };

    let resolved = expanded
        .canonicalize()
        .unwrap_or_else(|_| expanded.clone());

    if let Some(allowed) = allowed_dir {
        let allowed_resolved = allowed.canonicalize().unwrap_or_else(|_| allowed.to_path_buf());
        if !resolved.starts_with(&allowed_resolved) {
            return Err(format!(
                "Path {} is outside allowed directory {}",
                path,
                allowed.display()
            ));
        }
    }

    Ok(resolved)
}

// ====== ReadFileTool ======

pub struct ReadFileTool {
    allowed_dir: Option<PathBuf>,
}

impl ReadFileTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self { allowed_dir }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the given path."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: 'path' parameter is required".to_string(),
        };

        match resolve_path(path, self.allowed_dir.as_deref()) {
            Ok(file_path) => {
                if !file_path.exists() {
                    return format!("Error: File not found: {}", path);
                }
                if !file_path.is_file() {
                    return format!("Error: Not a file: {}", path);
                }
                match std::fs::read_to_string(&file_path) {
                    Ok(content) => content,
                    Err(e) => format!("Error reading file: {}", e),
                }
            }
            Err(e) => format!("Error: {}", e),
        }
    }
}

// ====== WriteFileTool ======

pub struct WriteFileTool {
    allowed_dir: Option<PathBuf>,
}

impl WriteFileTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self { allowed_dir }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file at the given path. Creates parent directories if needed."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: 'path' parameter is required".to_string(),
        };
        let content = match params.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return "Error: 'content' parameter is required".to_string(),
        };

        // For write, resolve parent to check allowed_dir
        let file_path = if path.starts_with("~/") || path.starts_with("~\\") {
            if let Some(home) = dirs::home_dir() {
                home.join(&path[2..])
            } else {
                PathBuf::from(path)
            }
        } else {
            PathBuf::from(path)
        };

        if let Some(ref allowed) = self.allowed_dir {
            let allowed_resolved = allowed.canonicalize().unwrap_or_else(|_| allowed.clone());
            let parent = file_path.parent().unwrap_or(&file_path);
            if parent.exists() {
                let parent_resolved = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
                if !parent_resolved.starts_with(&allowed_resolved) {
                    return format!("Error: Path {} is outside allowed directory", path);
                }
            }
        }

        if let Some(parent) = file_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return format!("Error creating directories: {}", e);
            }
        }

        match std::fs::write(&file_path, content) {
            Ok(_) => format!("Successfully wrote {} bytes to {}", content.len(), path),
            Err(e) => format!("Error writing file: {}", e),
        }
    }
}

// ====== EditFileTool ======

pub struct EditFileTool {
    allowed_dir: Option<PathBuf>,
}

impl EditFileTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self { allowed_dir }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing old_text with new_text. The old_text must exist exactly in the file."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The text to replace with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: 'path' parameter is required".to_string(),
        };
        let old_text = match params.get("old_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return "Error: 'old_text' parameter is required".to_string(),
        };
        let new_text = match params.get("new_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return "Error: 'new_text' parameter is required".to_string(),
        };

        match resolve_path(path, self.allowed_dir.as_deref()) {
            Ok(file_path) => {
                if !file_path.exists() {
                    return format!("Error: File not found: {}", path);
                }
                match std::fs::read_to_string(&file_path) {
                    Ok(content) => {
                        if !content.contains(old_text) {
                            return "Error: old_text not found in file. Make sure it matches exactly.".to_string();
                        }
                        let count = content.matches(old_text).count();
                        if count > 1 {
                            return format!(
                                "Warning: old_text appears {} times. Please provide more context to make it unique.",
                                count
                            );
                        }
                        let new_content = content.replacen(old_text, new_text, 1);
                        match std::fs::write(&file_path, new_content) {
                            Ok(_) => format!("Successfully edited {}", path),
                            Err(e) => format!("Error writing file: {}", e),
                        }
                    }
                    Err(e) => format!("Error reading file: {}", e),
                }
            }
            Err(e) => format!("Error: {}", e),
        }
    }
}

// ====== ListDirTool ======

pub struct ListDirTool {
    allowed_dir: Option<PathBuf>,
}

impl ListDirTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self { allowed_dir }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the contents of a directory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: 'path' parameter is required".to_string(),
        };

        match resolve_path(path, self.allowed_dir.as_deref()) {
            Ok(dir_path) => {
                if !dir_path.exists() {
                    return format!("Error: Directory not found: {}", path);
                }
                if !dir_path.is_dir() {
                    return format!("Error: Not a directory: {}", path);
                }

                match std::fs::read_dir(&dir_path) {
                    Ok(entries) => {
                        let mut items: Vec<String> = entries
                            .flatten()
                            .map(|entry| {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if entry.path().is_dir() {
                                    format!("[DIR]  {}", name)
                                } else {
                                    format!("[FILE] {}", name)
                                }
                            })
                            .collect();

                        items.sort();

                        if items.is_empty() {
                            format!("Directory {} is empty", path)
                        } else {
                            items.join("\n")
                        }
                    }
                    Err(e) => format!("Error listing directory: {}", e),
                }
            }
            Err(e) => format!("Error: {}", e),
        }
    }
}
