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

/// Strip provider-specific XML tags that leak into responses
///
/// Some LLM providers (e.g., minimax) emit internal XML tags like
/// `<minimax:tool_call>...</minimax:tool_call>` or similar provider-specific markup.
/// This function removes them before sending to the user.
pub fn strip_provider_tags(response: &str) -> String {
    // Strip <provider:anything>...</provider:anything> tags (e.g., minimax:tool_call, minimax:search_result)
    let re = regex::Regex::new(r"<\w+:\w+[^>]*>[\s\S]*?</\w+:\w+>").unwrap_or_else(|_| {
        regex::Regex::new(r"$^").unwrap()
    });
    let cleaned = re.replace_all(response, "");

    // Also strip self-closing provider tags like <minimax:something />
    let re_self = regex::Regex::new(r"<\w+:\w+[^/]*/\s*>").unwrap_or_else(|_| {
        regex::Regex::new(r"$^").unwrap()
    });
    let cleaned = re_self.replace_all(&cleaned, "");

    // Strip common Chinese/Korean trailing phrases that minimax sometimes appends.
    // Only strip if the response also contains Japanese (hiragana/katakana), to avoid
    // false positives on intentionally Chinese/Korean responses.
    let cleaned = strip_foreign_phrases(&cleaned);

    // Collapse multiple consecutive blank lines left by tag removal
    let re_blanks = regex::Regex::new(r"\n{3,}").unwrap_or_else(|_| {
        regex::Regex::new(r"$^").unwrap()
    });
    re_blanks.replace_all(cleaned.trim(), "\n\n").to_string()
}

/// Strip Chinese/Korean trailing phrases from Japanese responses.
/// Only applies when the response contains Japanese characters (hiragana/katakana).
fn strip_foreign_phrases(text: &str) -> String {
    // Only strip if response has Japanese (hiragana or katakana)
    let has_japanese = text.chars().any(|c|
        ('\u{3040}'..='\u{309F}').contains(&c) || // Hiragana
        ('\u{30A0}'..='\u{30FF}').contains(&c)     // Katakana
    );
    if !has_japanese {
        return text.to_string();
    }

    // Common Chinese phrases that minimax appends
    let chinese_patterns = [
        "还有什么我可以帮你的吗？",
        "还有什么可以帮你的吗？",
        "还有什么想知道的吗？",
        "还有什么需要帮助的吗？",
        "如果你有其他问题",
        "希望这对你有帮助",
        "有什么问题随时问我",
        "请随时告诉我",
        "如果需要更多帮助",
    ];

    let mut result = text.to_string();
    for pattern in &chinese_patterns {
        result = result.replace(pattern, "");
    }

    // Also strip any remaining pure-Chinese sentences (lines with only Chinese chars + punctuation)
    // A "Chinese-only line" = contains Chinese chars but no hiragana/katakana
    let lines: Vec<&str> = result.lines().collect();
    let filtered: Vec<&str> = lines.into_iter().filter(|line| {
        let has_chinese = line.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
        let has_jp_kana = line.chars().any(|c|
            ('\u{3040}'..='\u{309F}').contains(&c) ||
            ('\u{30A0}'..='\u{30FF}').contains(&c)
        );
        // Keep lines that: have no Chinese, or have Japanese kana alongside Chinese (kanji)
        !has_chinese || has_jp_kana || line.trim().is_empty()
    }).collect();

    filtered.join("\n")
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

    #[test]
    fn test_strip_provider_tags_minimax() {
        let response = "Here's my answer. <minimax:tool_call>some internal stuff</minimax:tool_call> And more text.";
        let clean = strip_provider_tags(response);
        assert_eq!(clean, "Here's my answer.  And more text.");
        assert!(!clean.contains("minimax"));
    }

    #[test]
    fn test_strip_provider_tags_no_tags() {
        let response = "Just a normal response with no provider tags.";
        let clean = strip_provider_tags(response);
        assert_eq!(clean, response);
    }

    #[test]
    fn test_strip_provider_tags_multiline() {
        let response = "Answer:\n\n<minimax:search_result>\nresult data here\n</minimax:search_result>\n\n\n\nDone.";
        let clean = strip_provider_tags(response);
        assert_eq!(clean, "Answer:\n\nDone.");
    }

    #[test]
    fn test_strip_chinese_from_japanese_response() {
        let response = "こんにちは！何かお手伝いできることはありますか？还有什么我可以帮你的吗？";
        let clean = strip_provider_tags(response);
        assert_eq!(clean, "こんにちは！何かお手伝いできることはありますか？");
        assert!(!clean.contains("还有"));
    }

    #[test]
    fn test_strip_chinese_line_from_japanese_response() {
        let response = "東京の天気は晴れです。\n如果需要更多帮助\nまた何かあれば聞いてね！";
        let clean = strip_provider_tags(response);
        assert!(clean.contains("東京の天気は晴れです"));
        assert!(clean.contains("また何かあれば聞いてね"));
        assert!(!clean.contains("如果需要"));
    }

    #[test]
    fn test_no_strip_chinese_from_non_japanese_response() {
        // Pure Chinese response should not be stripped
        let response = "你好！有什么我可以帮你的吗？";
        let clean = strip_provider_tags(response);
        assert_eq!(clean, response);
    }

    #[test]
    fn test_keep_kanji_in_japanese() {
        // Japanese text with kanji should not be stripped
        let response = "東京都は日本の首都です。人口は約1400万人。";
        let clean = strip_provider_tags(response);
        assert_eq!(clean, response);
    }
}
