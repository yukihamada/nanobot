//! Personality Learning System
//!
//! Enables nanobot to learn user preferences and adapt its behavior over time.
//! Tracks 5 key personality dimensions based on user feedback.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single personality trait with confidence score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalitySection {
    /// Dimension key (e.g., "tone", "verbosity")
    pub key: String,

    /// Current value (e.g., "friendly", "concise")
    pub value: String,

    /// Confidence level (0.0-1.0), increases with positive feedback
    pub confidence: f32,

    /// Last update timestamp
    pub last_updated: chrono::DateTime<chrono::Utc>,

    /// Number of feedback samples contributing to this trait
    pub feedback_count: i64,
}

impl PersonalitySection {
    /// Create a new personality section with default confidence
    pub fn new(key: String, value: String) -> Self {
        Self {
            key,
            value,
            confidence: 0.5,
            last_updated: chrono::Utc::now(),
            feedback_count: 0,
        }
    }

    /// Increase confidence (positive feedback)
    pub fn reinforce(&mut self, amount: f32) {
        self.confidence = (self.confidence + amount).min(1.0);
        self.feedback_count += 1;
        self.last_updated = chrono::Utc::now();
    }

    /// Decrease confidence (negative feedback)
    pub fn weaken(&mut self, amount: f32) {
        self.confidence = (self.confidence - amount).max(0.0);
        self.feedback_count += 1;
        self.last_updated = chrono::Utc::now();
    }
}

/// Five key personality dimensions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonalityDimension {
    /// Communication tone
    Tone,

    /// Response length preference
    Verbosity,

    /// Emoji usage level
    EmojiUsage,

    /// Code comment style
    CodeStyle,

    /// Proactivity level
    Proactivity,
}

impl PersonalityDimension {
    /// Get all dimensions
    pub fn all() -> Vec<Self> {
        vec![
            Self::Tone,
            Self::Verbosity,
            Self::EmojiUsage,
            Self::CodeStyle,
            Self::Proactivity,
        ]
    }

    /// Get DynamoDB sort key for this dimension
    pub fn to_sk(&self) -> String {
        match self {
            Self::Tone => "TONE".to_string(),
            Self::Verbosity => "VERBOSITY".to_string(),
            Self::EmojiUsage => "EMOJI_USAGE".to_string(),
            Self::CodeStyle => "CODE_STYLE".to_string(),
            Self::Proactivity => "PROACTIVITY".to_string(),
        }
    }

    /// Parse from sort key
    pub fn from_sk(sk: &str) -> Option<Self> {
        match sk {
            "TONE" => Some(Self::Tone),
            "VERBOSITY" => Some(Self::Verbosity),
            "EMOJI_USAGE" => Some(Self::EmojiUsage),
            "CODE_STYLE" => Some(Self::CodeStyle),
            "PROACTIVITY" => Some(Self::Proactivity),
            _ => None,
        }
    }

    /// Get possible values for this dimension
    pub fn possible_values(&self) -> Vec<&'static str> {
        match self {
            Self::Tone => vec!["formal", "friendly", "casual", "technical", "humorous"],
            Self::Verbosity => vec!["concise", "moderate", "detailed"],
            Self::EmojiUsage => vec!["none", "minimal", "moderate", "heavy"],
            Self::CodeStyle => vec!["minimal_comments", "detailed_comments"],
            Self::Proactivity => vec!["reactive", "proactive"],
        }
    }

    /// Get default value for this dimension
    pub fn default_value(&self) -> &'static str {
        match self {
            Self::Tone => "friendly",
            Self::Verbosity => "moderate",
            Self::EmojiUsage => "minimal",
            Self::CodeStyle => "detailed_comments",
            Self::Proactivity => "proactive",
        }
    }
}

/// Backend for storing and learning personality
#[async_trait::async_trait]
pub trait PersonalityBackend: Send + Sync {
    /// Get all personality traits for a user
    async fn get_personality(&self, user_id: &str) -> Result<Vec<PersonalitySection>, Box<dyn std::error::Error + Send + Sync>>;

    /// Update a single personality trait
    async fn update_personality(&self, user_id: &str, section: PersonalitySection) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Learn from user feedback (positive or negative)
    async fn learn_from_feedback(
        &self,
        user_id: &str,
        rating: &str,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// Analyze feedback and determine which personality dimensions to adjust
pub fn analyze_feedback_context(context: &str, rating: &str) -> HashMap<PersonalityDimension, f32> {
    let mut adjustments = HashMap::new();
    let context_lower = context.to_lowercase();

    // Verbosity analysis
    if context_lower.contains("too long") || context_lower.contains("verbose") || context_lower.contains("冗長") {
        adjustments.insert(PersonalityDimension::Verbosity, if rating == "down" { -0.2 } else { 0.1 });
    } else if context_lower.contains("too short") || context_lower.contains("brief") || context_lower.contains("短すぎ") {
        adjustments.insert(PersonalityDimension::Verbosity, if rating == "down" { 0.2 } else { -0.1 });
    }

    // Tone analysis
    if context_lower.contains("formal") || context_lower.contains("professional") || context_lower.contains("丁寧") {
        adjustments.insert(PersonalityDimension::Tone, if rating == "down" { -0.15 } else { 0.15 });
    } else if context_lower.contains("casual") || context_lower.contains("friendly") || context_lower.contains("フレンドリー") {
        adjustments.insert(PersonalityDimension::Tone, if rating == "down" { 0.15 } else { -0.15 });
    }

    // Emoji usage analysis
    if context_lower.contains("emoji") || context_lower.contains("絵文字") {
        adjustments.insert(PersonalityDimension::EmojiUsage, if rating == "down" { -0.2 } else { 0.2 });
    }

    // Code style analysis
    if context_lower.contains("comment") || context_lower.contains("documentation") || context_lower.contains("コメント") {
        adjustments.insert(PersonalityDimension::CodeStyle, if rating == "down" { -0.15 } else { 0.15 });
    }

    // Proactivity analysis
    if context_lower.contains("suggest") || context_lower.contains("proactive") || context_lower.contains("提案") {
        adjustments.insert(PersonalityDimension::Proactivity, if rating == "down" { -0.15 } else { 0.15 });
    }

    adjustments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_personality_section_reinforce() {
        let mut section = PersonalitySection::new("tone".to_string(), "friendly".to_string());
        assert_eq!(section.confidence, 0.5);

        section.reinforce(0.3);
        assert_eq!(section.confidence, 0.8);
        assert_eq!(section.feedback_count, 1);

        section.reinforce(0.5);
        assert_eq!(section.confidence, 1.0); // Capped at 1.0
    }

    #[test]
    fn test_personality_section_weaken() {
        let mut section = PersonalitySection::new("tone".to_string(), "friendly".to_string());
        section.weaken(0.3);
        assert!((section.confidence - 0.2).abs() < 1e-9, "Expected 0.2, got {}", section.confidence);

        section.weaken(0.5);
        assert!((section.confidence - 0.0).abs() < 1e-9, "Expected 0.0 (floored), got {}", section.confidence);
    }

    #[test]
    fn test_analyze_feedback_verbosity() {
        let adjustments = analyze_feedback_context("Response was too long and verbose", "down");
        assert!(adjustments.contains_key(&PersonalityDimension::Verbosity));
        assert!(adjustments[&PersonalityDimension::Verbosity] < 0.0);
    }

    #[test]
    fn test_analyze_feedback_tone() {
        let adjustments = analyze_feedback_context("I prefer a more formal tone", "down");
        assert!(adjustments.contains_key(&PersonalityDimension::Tone));
    }

    #[test]
    fn test_personality_dimension_sk() {
        assert_eq!(PersonalityDimension::Tone.to_sk(), "TONE");
        assert_eq!(PersonalityDimension::from_sk("TONE"), Some(PersonalityDimension::Tone));
    }
}
