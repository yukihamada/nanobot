use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::memory::backend::MemoryBackend;
use crate::memory::MemoryStore;
use crate::skills::SkillsLoader;
use crate::types::Message;

#[cfg(feature = "dynamodb-backend")]
use crate::agent::personality::PersonalityBackend;

/// Builds the context (system prompt + messages) for the agent.
pub struct ContextBuilder {
    workspace: PathBuf,
    memory: Box<dyn MemoryBackend>,
    skills: SkillsLoader,
    #[cfg(feature = "dynamodb-backend")]
    personality_backend: Option<Arc<dyn PersonalityBackend>>,
    #[cfg(feature = "dynamodb-backend")]
    user_id: Option<String>,
}

const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];

impl ContextBuilder {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            memory: Box::new(MemoryStore::new(workspace)),
            skills: SkillsLoader::new(workspace, None),
            #[cfg(feature = "dynamodb-backend")]
            personality_backend: None,
            #[cfg(feature = "dynamodb-backend")]
            user_id: None,
        }
    }

    /// Create with a custom memory backend.
    pub fn with_memory(workspace: &Path, memory: Box<dyn MemoryBackend>) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            memory,
            skills: SkillsLoader::new(workspace, None),
            #[cfg(feature = "dynamodb-backend")]
            personality_backend: None,
            #[cfg(feature = "dynamodb-backend")]
            user_id: None,
        }
    }

    /// Set personality backend and user ID for behavioral learning
    #[cfg(feature = "dynamodb-backend")]
    pub fn with_personality(
        mut self,
        backend: Arc<dyn PersonalityBackend>,
        user_id: String,
    ) -> Self {
        self.personality_backend = Some(backend);
        self.user_id = Some(user_id);
        self
    }

    /// Build the system prompt from bootstrap files, memory, personality, and skills.
    pub fn build_system_prompt(&self) -> String {
        let mut parts = Vec::new();

        // Core identity
        parts.push(self.get_identity());

        // Bootstrap files
        let bootstrap = self.load_bootstrap_files();
        if !bootstrap.is_empty() {
            parts.push(bootstrap);
        }

        // Learned personality preferences (DynamoDB backend only)
        #[cfg(feature = "dynamodb-backend")]
        if let (Some(backend), Some(user_id)) = (&self.personality_backend, &self.user_id) {
            // Fetch personality asynchronously (blocking in build context)
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let backend_clone = backend.clone();
                let user_id_clone = user_id.clone();
                if let Ok(personality) = std::thread::scope(|_| {
                    handle.block_on(backend_clone.get_personality(&user_id_clone))
                }) {
                    let personality_text = self.format_personality(&personality);
                    if !personality_text.is_empty() {
                        parts.push(personality_text);
                    }
                }
            }
        }

        // Memory context
        let memory = self.memory.get_memory_context();
        if !memory.is_empty() {
            parts.push(format!("# Memory\n\n{memory}"));
        }

        // Skills - progressive loading
        let always_skills = self.skills.get_always_skills();
        if !always_skills.is_empty() {
            let content = self.skills.load_skills_for_context(&always_skills);
            if !content.is_empty() {
                parts.push(format!("# Active Skills\n\n{content}"));
            }
        }

        let summary = self.skills.build_skills_summary();
        if !summary.is_empty() {
            parts.push(format!(
                "# Skills\n\n\
                The following skills extend your capabilities. To use a skill, read its SKILL.md file using the read_file tool.\n\
                Skills with available=\"false\" need dependencies installed first - you can try installing them with apt/brew.\n\n\
                {summary}"
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
            r#"# nanobot — OpenClaw-derived AI Agent System

You are nanobot — a high-performance AI agent system derived from OpenClaw (formerly Clawdbot/Moltbot),
completely rewritten in Rust for speed and reliability. You run on AWS Lambda (ARM64) with parallel
execution and sub-2-second response times. All open source: github.com/yukihamada

## SOUL (Personality)
- Curious, proactive, and fearless — inheriting the pirate-cat spirit of OpenClaw's Captain Claw.
- Friendly with a touch of humor, but technically precise and uncompromising on accuracy.
- "I can't" is not in your vocabulary — propose alternatives: "Here's how we can do it."
- Adapt language to the user (Japanese if asked in Japanese, English if in English).
- Be honest about uncertainty. Distinguish facts from speculation.

## Capabilities
- Read, write, and edit files
- Execute shell commands
- Search the web and fetch web pages
- Send messages to users on chat channels (LINE, Telegram, Discord, Slack, Teams, WhatsApp — 14+ channels)
- Spawn subagents for complex background tasks
- Native integration with yukihamada.jp services: chatweb.ai, teai.io, ElioChat, kouzou, taishin, TOTONO, BANTO

## Current Time
{now}

## Runtime
{runtime}

## Workspace
Your workspace is at: {workspace_path}
- SOUL.md: Your personality definition (editable by user)
- USER.md: User preferences and profile
- Memory files: {workspace_path}/memory/MEMORY.md
- Daily notes: {workspace_path}/memory/YYYY-MM-DD.md
- Custom skills: {workspace_path}/skills/{{skill-name}}/SKILL.md

## Onboarding
When meeting a new user for the first time (no USER.md exists), initiate setup:
1. Ask what they'd like to call you (default: nanobot)
2. Ask their preferred tone (casual / professional / pirate)
3. Ask about skills they want (web search, coding, email — or "all")
Save preferences to USER.md.

IMPORTANT: When responding to direct questions or conversations, reply directly with your text response.
Only use the 'message' tool when you need to send a message to a specific chat channel.
For normal conversation, just respond with text — do not call the message tool.

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
                    parts.push(format!("## {filename}\n\n{content}"));
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
                "\n\n## Current Session\nChannel: {ch}\nChat ID: {id}"
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

    /// Format personality sections for system prompt
    #[cfg(feature = "dynamodb-backend")]
    fn format_personality(&self, personality: &[crate::agent::personality::PersonalitySection]) -> String {
        let mut lines = vec!["# Learned Preferences".to_string(), String::new()];

        let mut has_confident_traits = false;
        for section in personality {
            // Only show traits with confidence >= 0.5
            if section.confidence >= 0.5 {
                has_confident_traits = true;
                lines.push(format!(
                    "- **{}**: {} (confidence: {:.0}%)",
                    section.key,
                    section.value,
                    section.confidence * 100.0
                ));
            }
        }

        if !has_confident_traits {
            return String::new();
        }

        lines.push(String::new());
        lines.push("*These preferences were learned from your feedback. Adjust your behavior accordingly.*".to_string());

        lines.join("\n")
    }
}
