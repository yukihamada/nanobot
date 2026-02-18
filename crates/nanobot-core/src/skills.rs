use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::util::markdown;

/// Loader for agent skills.
///
/// Skills are markdown files (SKILL.md) that teach the agent how to use
/// specific tools or perform certain tasks.
pub struct SkillsLoader {
    workspace_skills: PathBuf,
    builtin_skills: Option<PathBuf>,
}

/// Information about a skill.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub path: PathBuf,
    pub source: String,
}

impl SkillsLoader {
    pub fn new(workspace: &Path, builtin_skills: Option<PathBuf>) -> Self {
        Self {
            workspace_skills: workspace.join("skills"),
            builtin_skills,
        }
    }

    /// List all available skills.
    pub fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo> {
        let mut skills = Vec::new();

        // Workspace skills (highest priority)
        if self.workspace_skills.exists() {
            if let Ok(entries) = std::fs::read_dir(&self.workspace_skills) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if dir.is_dir() {
                        let skill_file = dir.join("SKILL.md");
                        if skill_file.exists() {
                            let name = dir
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            skills.push(SkillInfo {
                                name,
                                path: skill_file,
                                source: "workspace".to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Built-in skills
        if let Some(ref builtin_dir) = self.builtin_skills {
            if builtin_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(builtin_dir) {
                    for entry in entries.flatten() {
                        let dir = entry.path();
                        if dir.is_dir() {
                            let name = dir
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            // Skip if workspace already has this skill
                            if skills.iter().any(|s| s.name == name) {
                                continue;
                            }
                            let skill_file = dir.join("SKILL.md");
                            if skill_file.exists() {
                                skills.push(SkillInfo {
                                    name,
                                    path: skill_file,
                                    source: "builtin".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        if filter_unavailable {
            skills.retain(|s| {
                let meta = self.get_skill_nanobot_meta(&s.name);
                check_requirements(&meta)
            });
        }

        skills
    }

    /// Load a skill by name.
    pub fn load_skill(&self, name: &str) -> Option<String> {
        // Check workspace first
        let ws_skill = self.workspace_skills.join(name).join("SKILL.md");
        if ws_skill.exists() {
            return std::fs::read_to_string(&ws_skill).ok();
        }

        // Check built-in
        if let Some(ref builtin_dir) = self.builtin_skills {
            let builtin_skill = builtin_dir.join(name).join("SKILL.md");
            if builtin_skill.exists() {
                return std::fs::read_to_string(&builtin_skill).ok();
            }
        }

        None
    }

    /// Load specific skills for inclusion in agent context.
    pub fn load_skills_for_context(&self, skill_names: &[String]) -> String {
        let mut parts = Vec::new();
        for name in skill_names {
            if let Some(content) = self.load_skill(name) {
                let body = markdown::strip_frontmatter(&content);
                parts.push(format!("### Skill: {name}\n\n{body}"));
            }
        }
        parts.join("\n\n---\n\n")
    }

    /// Build a summary of all skills.
    pub fn build_skills_summary(&self) -> String {
        let all_skills = self.list_skills(false);
        if all_skills.is_empty() {
            return String::new();
        }

        let mut lines = vec!["<skills>".to_string()];
        for s in &all_skills {
            let name = escape_xml(&s.name);
            let desc = escape_xml(&self.get_skill_description(&s.name));
            let meta = self.get_skill_nanobot_meta(&s.name);
            let available = check_requirements(&meta);

            lines.push(format!(
                "  <skill available=\"{}\">",
                if available { "true" } else { "false" }
            ));
            lines.push(format!("    <name>{name}</name>"));
            lines.push(format!("    <description>{desc}</description>"));
            lines.push(format!("    <location>{}</location>", s.path.display()));

            if !available {
                let missing = get_missing_requirements(&meta);
                if !missing.is_empty() {
                    lines.push(format!("    <requires>{}</requires>", escape_xml(&missing)));
                }
            }

            lines.push("  </skill>".to_string());
        }
        lines.push("</skills>".to_string());
        lines.join("\n")
    }

    /// Get skills marked as always=true that meet requirements.
    pub fn get_always_skills(&self) -> Vec<String> {
        let mut result = Vec::new();
        for s in self.list_skills(true) {
            let frontmatter = self.get_skill_metadata(&s.name);
            let nanobot_meta = self.get_skill_nanobot_meta(&s.name);
            if nanobot_meta.contains_key("always")
                || frontmatter
                    .as_ref()
                    .and_then(|m| m.get("always"))
                    .map(|v| v == "true")
                    .unwrap_or(false)
            {
                result.push(s.name);
            }
        }
        result
    }

    /// Get metadata from a skill's frontmatter.
    pub fn get_skill_metadata(&self, name: &str) -> Option<HashMap<String, String>> {
        let content = self.load_skill(name)?;
        let (meta, _) = markdown::parse_frontmatter(&content);
        if meta.is_empty() {
            None
        } else {
            Some(meta)
        }
    }

    fn get_skill_description(&self, name: &str) -> String {
        self.get_skill_metadata(name)
            .and_then(|m| m.get("description").cloned())
            .unwrap_or_else(|| name.to_string())
    }

    fn get_skill_nanobot_meta(&self, name: &str) -> HashMap<String, serde_json::Value> {
        if let Some(meta) = self.get_skill_metadata(name) {
            if let Some(raw) = meta.get("metadata") {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(nanobot) = data.get("nanobot") {
                        if let Ok(m) = serde_json::from_value(nanobot.clone()) {
                            return m;
                        }
                    }
                }
            }
        }
        HashMap::new()
    }
}

fn check_requirements(meta: &HashMap<String, serde_json::Value>) -> bool {
    if let Some(requires) = meta.get("requires") {
        if let Some(bins) = requires.get("bins").and_then(|v| v.as_array()) {
            for bin in bins {
                if let Some(name) = bin.as_str() {
                    if which::which(name).is_err() {
                        return false;
                    }
                }
            }
        }
        if let Some(envs) = requires.get("env").and_then(|v| v.as_array()) {
            for env in envs {
                if let Some(name) = env.as_str() {
                    if std::env::var(name).is_err() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn get_missing_requirements(meta: &HashMap<String, serde_json::Value>) -> String {
    let mut missing = Vec::new();
    if let Some(requires) = meta.get("requires") {
        if let Some(bins) = requires.get("bins").and_then(|v| v.as_array()) {
            for bin in bins {
                if let Some(name) = bin.as_str() {
                    if which::which(name).is_err() {
                        missing.push(format!("CLI: {name}"));
                    }
                }
            }
        }
        if let Some(envs) = requires.get("env").and_then(|v| v.as_array()) {
            for env in envs {
                if let Some(name) = env.as_str() {
                    if std::env::var(name).is_err() {
                        missing.push(format!("ENV: {name}"));
                    }
                }
            }
        }
    }
    missing.join(", ")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
