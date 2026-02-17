// Tag parsing utilities for LLM response markers
//
// Supports:
// - Reply tags: [[reply_to_current]] for threaded replies
// - Reasoning tags: <think>...</think> for transparent reasoning

use serde::{Deserialize, Serialize};

/// Parse reply tag from LLM response
///
/// Looks for `[[reply_to_current]]` marker and extracts it.
/// Returns cleaned response text and optional reply target.
///
/// # Examples
///
/// ```
/// let (clean, reply) = parse_reply_tag("[[reply_to_current]] Here's my answer");
/// assert_eq!(clean, "Here's my answer");
/// assert_eq!(reply, Some("current".to_string()));
/// ```
pub fn parse_reply_tag(response: &str) -> (String, Option<String>) {
    if response.contains("[[reply_to_current]]") {
        let clean = response.replace("[[reply_to_current]]", "").trim().to_string();
        return (clean, Some("current".to_string()));
    }
    (response.to_string(), None)
}

/// Reasoning block extracted from <think> tags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningBlock {
    pub content: String,
    pub index: usize,
}

/// Extract reasoning blocks from LLM response
///
/// Looks for `<think>...</think>` blocks and extracts them.
/// Returns cleaned response text and reasoning blocks.
///
/// # Examples
///
/// ```
/// let response = "Let me think... <think>First I need to...</think> Here's the answer.";
/// let (clean, blocks) = extract_reasoning(response);
/// assert_eq!(blocks.len(), 1);
/// assert!(clean.starts_with("Let me think..."));
/// assert!(!clean.contains("<think>"));
/// ```
pub fn extract_reasoning(response: &str) -> (String, Vec<ReasoningBlock>) {
    let mut reasoning_blocks = Vec::new();
    let mut clean = String::new();
    let mut last_end = 0;
    let mut index = 0;

    // Manual parsing to handle nested tags gracefully
    let chars: Vec<char> = response.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Look for <think> opening tag
        if i + 6 < chars.len()
            && chars[i..i + 6] == ['<', 't', 'h', 'i', 'n', 'k']
            && (i + 6 >= chars.len() || chars[i + 6] == '>')
        {
            // Found opening tag
            clean.push_str(&response[last_end..i]);
            i += 7; // Skip "<think>"
            let think_start = i;

            // Find closing tag
            let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if i + 7 < chars.len()
                    && chars[i..i + 7] == ['<', '/', 't', 'h', 'i', 'n', 'k']
                    && (i + 7 >= chars.len() || chars[i + 7] == '>')
                {
                    depth -= 1;
                    if depth == 0 {
                        // Extract reasoning content
                        let content = response[think_start..i].trim().to_string();
                        reasoning_blocks.push(ReasoningBlock {
                            content,
                            index,
                        });
                        index += 1;
                        i += 8; // Skip "</think>"
                        last_end = i;
                        break;
                    }
                } else if i + 6 < chars.len()
                    && chars[i..i + 6] == ['<', 't', 'h', 'i', 'n', 'k']
                    && (i + 6 >= chars.len() || chars[i + 6] == '>')
                {
                    // Nested opening tag
                    depth += 1;
                    i += 7;
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }

    // Add remaining text
    clean.push_str(&response[last_end..]);

    (clean.trim().to_string(), reasoning_blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_reply_tag_simple() {
        let (clean, reply) = parse_reply_tag("[[reply_to_current]] This is my answer");
        assert_eq!(clean, "This is my answer");
        assert_eq!(reply, Some("current".to_string()));
    }

    #[test]
    fn test_parse_reply_tag_no_tag() {
        let (clean, reply) = parse_reply_tag("This is my answer");
        assert_eq!(clean, "This is my answer");
        assert_eq!(reply, None);
    }

    #[test]
    fn test_parse_reply_tag_with_whitespace() {
        let (clean, reply) = parse_reply_tag("  [[reply_to_current]]  \n  Answer here  ");
        assert_eq!(clean, "Answer here");
        assert_eq!(reply, Some("current".to_string()));
    }

    #[test]
    fn test_extract_reasoning_simple() {
        let response = "Let me think. <think>I need to analyze this carefully.</think> Here's my answer.";
        let (clean, blocks) = extract_reasoning(response);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "I need to analyze this carefully.");
        assert_eq!(blocks[0].index, 0);
        assert_eq!(clean, "Let me think.  Here's my answer.");
    }

    #[test]
    fn test_extract_reasoning_multiple() {
        let response = "<think>First thought</think> Some text <think>Second thought</think> Final answer.";
        let (clean, blocks) = extract_reasoning(response);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].content, "First thought");
        assert_eq!(blocks[0].index, 0);
        assert_eq!(blocks[1].content, "Second thought");
        assert_eq!(blocks[1].index, 1);
        assert_eq!(clean, "Some text  Final answer.");
    }

    #[test]
    fn test_extract_reasoning_no_tags() {
        let response = "Just a normal response.";
        let (clean, blocks) = extract_reasoning(response);

        assert_eq!(blocks.len(), 0);
        assert_eq!(clean, "Just a normal response.");
    }

    #[test]
    fn test_extract_reasoning_nested() {
        // Should handle nested tags gracefully (or at least not crash)
        let response = "<think>Outer <think>inner</think> still outer</think> Done.";
        let (clean, blocks) = extract_reasoning(response);

        // At minimum, should not panic
        assert!(!clean.is_empty());
    }

    #[test]
    fn test_extract_reasoning_multiline() {
        let response = r#"Let me solve this step by step.

<think>
Step 1: Understand the problem
Step 2: Break it down
Step 3: Solve each part
</think>

Here's my solution."#;
        let (clean, blocks) = extract_reasoning(response);

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.contains("Step 1"));
        assert!(blocks[0].content.contains("Step 2"));
        assert!(!clean.contains("<think>"));
    }
}
