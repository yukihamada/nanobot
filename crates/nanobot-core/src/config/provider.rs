use std::path::Path;

use super::Config;

/// Trait for configuration providers.
///
/// Allows loading configuration from different sources (file, DynamoDB, etc.).
pub trait ConfigProvider: Send + Sync {
    /// Load config for a tenant (or the default local config).
    fn load_config(&self) -> Config;

    /// Load a workspace file by name (e.g., "AGENTS.md", "SOUL.md").
    /// Returns None if the file doesn't exist.
    fn load_workspace_file(&self, filename: &str) -> Option<String>;
}

/// File-based config provider (the default for CLI usage).
pub struct FileConfigProvider {
    config_path: Option<std::path::PathBuf>,
    workspace: std::path::PathBuf,
}

impl FileConfigProvider {
    pub fn new(config_path: Option<&Path>, workspace: &Path) -> Self {
        Self {
            config_path: config_path.map(|p| p.to_path_buf()),
            workspace: workspace.to_path_buf(),
        }
    }
}

impl ConfigProvider for FileConfigProvider {
    fn load_config(&self) -> Config {
        super::load_config(self.config_path.as_deref())
    }

    fn load_workspace_file(&self, filename: &str) -> Option<String> {
        let file_path = self.workspace.join(filename);
        if file_path.exists() {
            std::fs::read_to_string(&file_path).ok()
        } else {
            None
        }
    }
}
