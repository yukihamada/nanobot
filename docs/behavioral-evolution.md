# Behavioral Evolution

## Overview

nanobot can learn and adapt its behavior based on user feedback, personalizing the experience over time.

## Architecture

### Personality Dimensions

Five key dimensions tracked per user:

| Dimension | Values | Default |
|-----------|--------|---------|
| **Tone** | formal, friendly, casual, technical, humorous | friendly |
| **Verbosity** | concise, moderate, detailed | moderate |
| **EmojiUsage** | none, minimal, moderate, heavy | minimal |
| **CodeStyle** | minimal_comments, detailed_comments | detailed_comments |
| **Proactivity** | reactive, proactive | proactive |

### Confidence Scoring

Each dimension has a confidence score (0.0-1.0):
- **0.0-0.4**: Low confidence, not shown in prompt
- **0.5-0.7**: Medium confidence, shown in prompt
- **0.8-1.0**: High confidence, strongly influences behavior

### Feedback Learning

User feedback (thumbs up/down) adjusts confidence:
- **Positive feedback**: Reinforces current traits (+0.1 to +0.3)
- **Negative feedback**: Weakens current traits (-0.1 to -0.3)

## Data Flow

```
User Feedback (👍/👎)
         ↓
Feedback Analysis
    (analyze_feedback_context)
         ↓
Dimension Adjustments
    (e.g., Verbosity -0.2)
         ↓
Update DynamoDB
    (PersonalityBackend)
         ↓
Next Response
    (ContextBuilder injects learned preferences)
```

## DynamoDB Schema

```
PK: PERSONALITY#{user_id}
SK: {DIMENSION}  (e.g., TONE, VERBOSITY)
Attributes:
  - value: string (e.g., "concise")
  - confidence: number (0.0-1.0)
  - feedback_count: number
  - updated_at: timestamp
```

## Implementation

### PersonalityBackend Trait

```rust
#[async_trait]
pub trait PersonalityBackend {
    async fn get_personality(&self, user_id: &str) -> Result<Vec<PersonalitySection>>;
    async fn update_personality(&self, user_id: &str, section: PersonalitySection) -> Result<()>;
    async fn learn_from_feedback(&self, user_id: &str, rating: &str, context: &str) -> Result<()>;
}
```

### Feedback Analysis

```rust
pub fn analyze_feedback_context(context: &str, rating: &str) -> HashMap<PersonalityDimension, f32> {
    // Detects keywords like "too long", "too many emojis", etc.
    // Returns dimension → adjustment map
}
```

Example:
- Feedback: "Response was too long with too many emojis"
- Analysis: `{Verbosity: -0.2, EmojiUsage: -0.2}`

### System Prompt Injection

Learned preferences appear in system prompt when confidence ≥ 0.5:

```
# Learned Preferences

- **TONE**: friendly (confidence: 80%)
- **VERBOSITY**: concise (confidence: 90%)
- **EMOJI_USAGE**: minimal (confidence: 75%)

*These preferences were learned from your feedback. Adjust your behavior accordingly.*
```

## Usage

### Automatic Learning

1. User receives response
2. User clicks 👎 (negative feedback)
3. System analyzes last response context
4. Adjusts relevant personality dimensions
5. Next response reflects updated preferences

### Self-Reflection (Future)

After each response, nanobot waits 30 seconds:
- If feedback received → learn from it
- If no feedback → assume neutral/positive

## Examples

### Example 1: Learning Verbosity

**Initial State**:
- Verbosity: moderate (confidence: 0.5)

**Response 1**:
```
Here's a detailed explanation of how to use the API:

First, you need to install the SDK...
(300 words)
```

**User Feedback**: 👎 "Too long, just give me the code"

**Adjustment**: Verbosity → concise (confidence: 0.7)

**Response 2**:
```
pip install api-sdk
api-sdk --auth token run
```

**User Feedback**: 👍

**Adjustment**: Verbosity → concise (confidence: 0.9)

### Example 2: Learning Tone

**Initial State**:
- Tone: friendly (confidence: 0.5)

**Response 1**:
```
Hey there! 😊 Let me help you with that! This is super easy...
```

**User Feedback**: 👎 "Too casual, I prefer professional"

**Adjustment**: Tone → formal (confidence: 0.7)

**Response 2**:
```
I can assist you with that request. The following steps are required:
1. Configure the environment
2. Execute the command
```

## Testing

### Unit Tests

```rust
#[test]
fn test_personality_learns_from_negative_feedback() {
    let feedback = "Response was too long";
    let adjustments = analyze_feedback_context(feedback, "down");
    assert!(adjustments.contains_key(&PersonalityDimension::Verbosity));
    assert!(adjustments[&PersonalityDimension::Verbosity] < 0.0);
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_personality_confidence_increases() {
    let backend = DynamoMemoryBackend::new(client, table, user_id);
    let mut section = PersonalitySection::new("tone", "technical");

    // Simulate 5 positive feedbacks
    for _ in 0..5 {
        section.reinforce(0.1);
        backend.update_personality(user_id, section.clone()).await.unwrap();
    }

    let personality = backend.get_personality(user_id).await.unwrap();
    let tone = personality.iter().find(|s| s.key == "TONE").unwrap();
    assert!(tone.confidence > 0.8);
}
```

## Future Enhancements

### Planned Features

1. **Multi-Modal Learning**
   - Learn from response time (too slow → prefer faster models)
   - Learn from retry patterns (user re-asks → wasn't clear enough)

2. **Context-Aware Preferences**
   - Work context → formal tone
   - Personal context → casual tone
   - Coding context → minimal comments

3. **Preference Sharing**
   - Export/import personality profiles
   - Team-wide personality presets
   - "Save as template" feature

4. **Advanced Feedback Analysis**
   - Use LLM to analyze feedback text
   - Detect sarcasm ("Great job... not")
   - Multi-language feedback support

## Privacy

### Data Storage

- Personality data stored per-user in DynamoDB
- Encrypted at rest (DynamoDB encryption)
- No cross-user data sharing

### Data Retention

- Personality data persists indefinitely
- User can reset via API: `DELETE /api/v1/personality`

### GDPR Compliance

- Right to be forgotten: Delete DynamoDB items with `PK=PERSONALITY#{user_id}`
- Data export: `GET /api/v1/personality` returns JSON

## References

- Implementation: `src/agent/personality.rs`
- DynamoDB Backend: `src/memory/dynamo_backend.rs`
- Context Injection: `src/agent/context.rs`
- Tests: `tests/personality.rs`
