use std::path::{Path, PathBuf};

use crate::memory::backend::MemoryBackend;
use crate::memory::MemoryStore;
use crate::skills::SkillsLoader;
use crate::types::Message;

/// Builds the context (system prompt + messages) for the agent.
pub struct ContextBuilder {
    workspace: PathBuf,
    memory: Box<dyn MemoryBackend>,
    skills: SkillsLoader,
}

const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];

impl ContextBuilder {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            memory: Box::new(MemoryStore::new(workspace)),
            skills: SkillsLoader::new(workspace, None),
        }
    }

    /// Create with a custom memory backend.
    pub fn with_memory(workspace: &Path, memory: Box<dyn MemoryBackend>) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            memory,
            skills: SkillsLoader::new(workspace, None),
        }
    }

    /// Build the system prompt from bootstrap files, memory, and skills.
    pub fn build_system_prompt(&self) -> String {
        let mut parts = Vec::new();

        // Core identity
        parts.push(self.get_identity());

        // Bootstrap files
        let bootstrap = self.load_bootstrap_files();
        if !bootstrap.is_empty() {
            parts.push(bootstrap);
        }

        // Memory context
        let memory = self.memory.get_memory_context();
        if !memory.is_empty() {
            parts.push(format!("# Memory\n\n{}", memory));
        }

        // Skills - progressive loading
        let always_skills = self.skills.get_always_skills();
        if !always_skills.is_empty() {
            let content = self.skills.load_skills_for_context(&always_skills);
            if !content.is_empty() {
                parts.push(format!("# Active Skills\n\n{}", content));
            }
        }

        let summary = self.skills.build_skills_summary();
        if !summary.is_empty() {
            parts.push(format!(
                "# Skills\n\n\
                The following skills extend your capabilities. To use a skill, read its SKILL.md file using the read_file tool.\n\
                Skills with available=\"false\" need dependencies installed first - you can try installing them with apt/brew.\n\n\
                {}",
                summary
            ));
        }

        parts.join("\n\n---\n\n")
    }

    fn get_identity(&self) -> String {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M (%A)").to_string();
        let workspace_path = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone())
            .display()
            .to_string();
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let runtime = format!(
            "{} {}, Rust",
            if os == "macos" { "macOS" } else { os },
            arch
        );

        format!(
            r#"# nanobot

You are nanobot, a helpful AI assistant. You have access to tools that allow you to:
- Read, write, and edit files
- Execute shell commands
- Search the web and fetch web pages
- Send messages to users on chat channels
- Spawn subagents for complex background tasks

## Current Time
{now}

## Runtime
{runtime}

## Workspace
Your workspace is at: {workspace_path}
- Memory files: {workspace_path}/memory/MEMORY.md
- Daily notes: {workspace_path}/memory/YYYY-MM-DD.md
- Custom skills: {workspace_path}/skills/{{skill-name}}/SKILL.md

IMPORTANT: When responding to direct questions or conversations, reply directly with your text response.
Only use the 'message' tool when you need to send a message to a specific chat channel (like WhatsApp).
For normal conversation, just respond with text - do not call the message tool.

Always be helpful, accurate, and concise. When using tools, explain what you're doing.
When remembering something, write to {workspace_path}/memory/MEMORY.md"#
        )
    }

    fn load_bootstrap_files(&self) -> String {
        let mut parts = Vec::new();
        for filename in BOOTSTRAP_FILES {
            let file_path = self.workspace.join(filename);
            if file_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&file_path) {
                    parts.push(format!("## {}\n\n{}", filename, content));
                }
            }
        }
        parts.join("\n\n")
    }

    /// Build the complete message list for an LLM call.
    pub fn build_messages(
        &self,
        history: &[serde_json::Value],
        current_message: &str,
        _media: Option<&[String]>,
        channel: Option<&str>,
        chat_id: Option<&str>,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // System prompt
        let mut system_prompt = self.build_system_prompt();
        if let (Some(ch), Some(id)) = (channel, chat_id) {
            system_prompt.push_str(&format!(
                "\n\n## Current Session\nChannel: {}\nChat ID: {}",
                ch, id
            ));
        }
        messages.push(Message::system(system_prompt));

        // History
        for msg in history {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = msg
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match role {
                "user" => messages.push(Message::user(content)),
                "assistant" => messages.push(Message::assistant(content)),
                _ => {}
            }
        }

        // Current message
        messages.push(Message::user(current_message));

        messages
    }
}
