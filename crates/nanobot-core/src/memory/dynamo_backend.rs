use std::path::PathBuf;

use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;
use tracing::warn;

use super::backend::MemoryBackend;

/// DynamoDB-based memory backend for multi-tenant SaaS.
pub struct DynamoMemoryBackend {
    client: Client,
    table_name: String,
    tenant_id: String,
}

impl DynamoMemoryBackend {
    pub fn new(client: Client, table_name: String, tenant_id: String) -> Self {
        Self {
            client,
            table_name,
            tenant_id,
        }
    }

    fn get_item_sync(&self, sk: &str) -> Option<String> {
        let rt = tokio::runtime::Handle::try_current().ok()?;
        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();
        let sk = sk.to_string();

        std::thread::scope(|_| {
            rt.block_on(async {
                let resp = client
                    .get_item()
                    .table_name(&table)
                    .key("tenant_id", AttributeValue::S(tenant))
                    .key("session_key", AttributeValue::S(sk))
                    .send()
                    .await;

                match resp {
                    Ok(output) => output
                        .item
                        .and_then(|item| {
                            item.get("content")
                                .and_then(|v| v.as_s().ok())
                                .cloned()
                        }),
                    Err(e) => {
                        warn!("DynamoDB get memory error: {}", e);
                        None
                    }
                }
            })
        })
    }

    fn put_item_sync(&self, sk: &str, content: &str) {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => return,
        };

        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();
        let sk = sk.to_string();
        let content = content.to_string();

        std::thread::scope(|_| {
            rt.block_on(async {
                let result = client
                    .put_item()
                    .table_name(&table)
                    .item("tenant_id", AttributeValue::S(tenant))
                    .item("session_key", AttributeValue::S(sk))
                    .item("content", AttributeValue::S(content))
                    .item(
                        "updated_at",
                        AttributeValue::S(chrono::Utc::now().to_rfc3339()),
                    )
                    .send()
                    .await;

                if let Err(e) = result {
                    warn!("DynamoDB put memory error: {}", e);
                }
            })
        });
    }

    fn today_key(&self) -> String {
        format!("memory:daily:{}", crate::util::today_date())
    }
}

impl MemoryBackend for DynamoMemoryBackend {
    fn read_today(&self) -> String {
        self.get_item_sync(&self.today_key()).unwrap_or_default()
    }

    fn append_today(&self, content: &str) {
        let key = self.today_key();
        let existing = self.get_item_sync(&key).unwrap_or_default();
        let new_content = if existing.is_empty() {
            format!("# {}\n\n{}", crate::util::today_date(), content)
        } else {
            format!("{}\n{}", existing, content)
        };
        self.put_item_sync(&key, &new_content);
    }

    fn read_long_term(&self) -> String {
        self.get_item_sync("memory:long_term")
            .unwrap_or_default()
    }

    fn write_long_term(&self, content: &str) {
        self.put_item_sync("memory:long_term", content);
    }

    fn get_recent_memories(&self, days: u32) -> String {
        let today = chrono::Local::now().date_naive();
        let mut memories = Vec::new();

        for i in 0..days {
            let date = today - chrono::Duration::days(i as i64);
            let key = format!("memory:daily:{}", date.format("%Y-%m-%d"));
            if let Some(content) = self.get_item_sync(&key) {
                memories.push(content);
            }
        }

        memories.join("\n\n---\n\n")
    }

    fn list_memory_files(&self) -> Vec<PathBuf> {
        // DynamoDB backend doesn't use file paths
        Vec::new()
    }

    fn get_memory_context(&self) -> String {
        let mut parts = Vec::new();

        let long_term = self.read_long_term();
        if !long_term.is_empty() {
            parts.push(format!("## Long-term Memory\n{}", long_term));
        }

        let today = self.read_today();
        if !today.is_empty() {
            parts.push(format!("## Today's Notes\n{}", today));
        }

        parts.join("\n\n")
    }
}

// ---------------------------------------------------------------------------
// PersonalityBackend implementation for DynamoDB
// ---------------------------------------------------------------------------

use crate::agent::personality::{PersonalityBackend, PersonalitySection, PersonalityDimension, analyze_feedback_context};

#[async_trait::async_trait]
impl PersonalityBackend for DynamoMemoryBackend {
    async fn get_personality(&self, user_id: &str) -> Result<Vec<PersonalitySection>, Box<dyn std::error::Error + Send + Sync>> {
        let pk = format!("PERSONALITY#{}", user_id);

        let result = self.client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .send()
            .await?;

        let mut sections = Vec::new();

        if let Some(items) = result.items {
            for item in items {
                let sk = item.get("sk")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("Missing sk")?;

                let value = item.get("value")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("Missing value")?
                    .to_string();

                let confidence = item.get("confidence")
                    .and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse::<f32>().ok())
                    .unwrap_or(0.5);

                let feedback_count = item.get("feedback_count")
                    .and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse::<i64>().ok())
                    .unwrap_or(0);

                let updated_at = item.get("updated_at")
                    .and_then(|v| v.as_s().ok())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(chrono::Utc::now);

                sections.push(PersonalitySection {
                    key: sk.to_string(),
                    value,
                    confidence,
                    last_updated: updated_at,
                    feedback_count,
                });
            }
        }

        // If no personality exists, initialize with defaults
        if sections.is_empty() {
            for dimension in PersonalityDimension::all() {
                sections.push(PersonalitySection::new(
                    dimension.to_sk(),
                    dimension.default_value().to_string(),
                ));
            }
        }

        Ok(sections)
    }

    async fn update_personality(&self, user_id: &str, section: PersonalitySection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pk = format!("PERSONALITY#{}", user_id);

        self.client
            .put_item()
            .table_name(&self.table_name)
            .item("pk", AttributeValue::S(pk))
            .item("sk", AttributeValue::S(section.key))
            .item("value", AttributeValue::S(section.value))
            .item("confidence", AttributeValue::N(section.confidence.to_string()))
            .item("feedback_count", AttributeValue::N(section.feedback_count.to_string()))
            .item("updated_at", AttributeValue::S(section.last_updated.to_rfc3339()))
            .send()
            .await?;

        Ok(())
    }

    async fn learn_from_feedback(
        &self,
        user_id: &str,
        rating: &str,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Analyze feedback to determine which dimensions to adjust
        let adjustments = analyze_feedback_context(context, rating);

        if adjustments.is_empty() {
            // No clear signal, nothing to learn
            return Ok(());
        }

        // Get current personality
        let mut sections = self.get_personality(user_id).await?;

        // Apply adjustments
        for (dimension, adjustment) in adjustments {
            if let Some(section) = sections.iter_mut().find(|s| s.key == dimension.to_sk()) {
                if adjustment > 0.0 {
                    section.reinforce(adjustment);
                } else {
                    section.weaken(-adjustment);
                }

                // Update in database
                self.update_personality(user_id, section.clone()).await?;
            }
        }

        Ok(())
    }
}
