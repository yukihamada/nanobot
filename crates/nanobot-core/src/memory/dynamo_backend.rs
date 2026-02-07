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
