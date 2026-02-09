use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use super::Tool;

/// Shell execution tool with safety guards.
pub struct ExecTool {
    timeout: u64,
    working_dir: String,
    deny_patterns: Vec<Regex>,
    restrict_to_workspace: bool,
}

impl ExecTool {
    pub fn new(working_dir: String, timeout: u64, restrict_to_workspace: bool) -> Self {
        let deny_patterns = vec![
            r"\brm\s+-[rf]{1,2}\b",
            r"\bdel\s+/[fq]\b",
            r"\brmdir\s+/s\b",
            r"\b(format|mkfs|diskpart)\b",
            r"\bdd\s+if=",
            r">\s*/dev/sd",
            r"\b(shutdown|reboot|poweroff)\b",
            r":\(\)\s*\{.*\};\s*:",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();

        Self {
            timeout,
            working_dir,
            deny_patterns,
            restrict_to_workspace,
        }
    }

    fn guard_command(&self, command: &str, cwd: &str) -> Option<String> {
        let lower = command.to_lowercase();

        for pattern in &self.deny_patterns {
            if pattern.is_match(&lower) {
                return Some(
                    "Error: Command blocked by safety guard (dangerous pattern detected)"
                        .to_string(),
                );
            }
        }

        if self.restrict_to_workspace {
            if command.contains("../") || command.contains("..\\") {
                return Some(
                    "Error: Command blocked by safety guard (path traversal detected)".to_string(),
                );
            }

            // Check absolute paths
            let cwd_path = Path::new(cwd);
            let path_re = Regex::new(r#"/[^\s"']+"#).unwrap_or_else(|_| Regex::new(".^").unwrap());
            for mat in path_re.find_iter(command) {
                let p = Path::new(mat.as_str());
                if let Ok(resolved) = p.canonicalize() {
                    if let Ok(cwd_resolved) = cwd_path.canonicalize() {
                        if !resolved.starts_with(&cwd_resolved) {
                            return Some("Error: Command blocked by safety guard (path outside working dir)".to_string());
                        }
                    }
                }
            }
        }

        None
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output. Use with caution."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, params: HashMap<String, serde_json::Value>) -> String {
        let command = match params.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return "Error: 'command' parameter is required".to_string(),
        };
        let cwd = params
            .get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.working_dir);

        if let Some(error) = self.guard_command(command, cwd) {
            return error;
        }

        let result = tokio::time::timeout(
            Duration::from_secs(self.timeout),
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let mut parts = Vec::new();

                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.is_empty() {
                    parts.push(stdout.to_string());
                }

                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.trim().is_empty() {
                    parts.push(format!("STDERR:\n{stderr}"));
                }

                if !output.status.success() {
                    parts.push(format!(
                        "\nExit code: {}",
                        output.status.code().unwrap_or(-1)
                    ));
                }

                let result = if parts.is_empty() {
                    "(no output)".to_string()
                } else {
                    parts.join("\n")
                };

                // Truncate very long output
                const MAX_LEN: usize = 10000;
                if result.len() > MAX_LEN {
                    format!(
                        "{}\n... (truncated, {} more chars)",
                        &result[..MAX_LEN],
                        result.len() - MAX_LEN
                    )
                } else {
                    result
                }
            }
            Ok(Err(e)) => format!("Error executing command: {e}"),
            Err(_) => format!("Error: Command timed out after {} seconds", self.timeout),
        }
    }
}
