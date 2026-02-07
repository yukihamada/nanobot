use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;
use tracing::warn;

use super::store::SessionStore;
use super::{Session, SessionMessage};

/// DynamoDB-based session store.
pub struct DynamoSessionStore {
    client: Client,
    table_name: String,
    tenant_id: String,
    cache: HashMap<String, Session>,
}

impl DynamoSessionStore {
    pub fn new(client: Client, table_name: String, tenant_id: String) -> Self {
        Self {
            client,
            table_name,
            tenant_id,
            cache: HashMap::new(),
        }
    }

    fn load_from_dynamo(&self, key: &str) -> Option<Session> {
        let rt = tokio::runtime::Handle::try_current().ok()?;
        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();
        let key = key.to_string();

        std::thread::spawn(move || {
            rt.block_on(async {
                let resp = client
                    .get_item()
                    .table_name(&table)
                    .key("tenant_id", AttributeValue::S(tenant))
                    .key("session_key", AttributeValue::S(key.clone()))
                    .send()
                    .await;

                match resp {
                    Ok(output) => {
                        if let Some(item) = output.item {
                            parse_session_from_item(&key, &item)
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        warn!("DynamoDB get_item error: {}", e);
                        None
                    }
                }
            })
        })
        .join()
        .ok()
        .flatten()
    }

    fn save_to_dynamo(&self, session: &Session) {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => return,
        };

        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();
        let session_key = session.key.clone();
        let messages_json = serde_json::to_string(&session.messages).unwrap_or_default();
        let created_at = session.created_at.to_rfc3339();
        let updated_at = session.updated_at.to_rfc3339();
        let ttl = (chrono::Utc::now().timestamp() + 30 * 24 * 3600).to_string();

        let _ = std::thread::spawn(move || {
            rt.block_on(async {
                let result = client
                    .put_item()
                    .table_name(&table)
                    .item("tenant_id", AttributeValue::S(tenant))
                    .item("session_key", AttributeValue::S(session_key))
                    .item("messages", AttributeValue::S(messages_json))
                    .item("created_at", AttributeValue::S(created_at))
                    .item("updated_at", AttributeValue::S(updated_at))
                    .item("ttl", AttributeValue::N(ttl))
                    .send()
                    .await;

                if let Err(e) = result {
                    warn!("DynamoDB put_item error: {}", e);
                }
            })
        })
        .join();
    }
}

impl SessionStore for DynamoSessionStore {
    fn get_or_create(&mut self, key: &str) -> &mut Session {
        if !self.cache.contains_key(key) {
            let session = self
                .load_from_dynamo(key)
                .unwrap_or_else(|| Session::new(key));
            self.cache.insert(key.to_string(), session);
        }
        self.cache.get_mut(key).unwrap()
    }

    fn save(&self, session: &Session) {
        self.save_to_dynamo(session);
    }

    fn save_by_key(&self, key: &str) {
        if let Some(session) = self.cache.get(key) {
            self.save_to_dynamo(session);
        }
    }

    fn delete(&mut self, key: &str) -> bool {
        self.cache.remove(key);

        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => return false,
        };

        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();
        let key = key.to_string();

        std::thread::spawn(move || {
            rt.block_on(async {
                client
                    .delete_item()
                    .table_name(&table)
                    .key("tenant_id", AttributeValue::S(tenant))
                    .key("session_key", AttributeValue::S(key))
                    .send()
                    .await
                    .is_ok()
            })
        })
        .join()
        .unwrap_or(false)
    }

    fn list_sessions(&self) -> Vec<serde_json::Value> {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(rt) => rt,
            Err(_) => return Vec::new(),
        };

        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();

        std::thread::spawn(move || {
            rt.block_on(async {
                let resp = client
                    .query()
                    .table_name(&table)
                    .key_condition_expression("tenant_id = :tid")
                    .expression_attribute_values(":tid", AttributeValue::S(tenant))
                    .send()
                    .await;

                match resp {
                    Ok(output) => {
                        let mut sessions = Vec::new();
                        if let Some(items) = output.items {
                            for item in &items {
                                let key = item
                                    .get("session_key")
                                    .and_then(|v| v.as_s().ok())
                                    .cloned()
                                    .unwrap_or_default();
                                let created_at = item
                                    .get("created_at")
                                    .and_then(|v| v.as_s().ok())
                                    .cloned();
                                let updated_at = item
                                    .get("updated_at")
                                    .and_then(|v| v.as_s().ok())
                                    .cloned();

                                sessions.push(serde_json::json!({
                                    "key": key,
                                    "created_at": created_at,
                                    "updated_at": updated_at,
                                }));
                            }
                        }
                        sessions.sort_by(|a, b| {
                            let ua = a.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
                            let ub = b.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
                            ub.cmp(ua)
                        });
                        sessions
                    }
                    Err(e) => {
                        warn!("DynamoDB query error: {}", e);
                        Vec::new()
                    }
                }
            })
        })
        .join()
        .unwrap_or_default()
    }
}

fn parse_session_from_item(
    key: &str,
    item: &HashMap<String, AttributeValue>,
) -> Option<Session> {
    let messages_str = item.get("messages").and_then(|v| v.as_s().ok())?;
    let messages: Vec<SessionMessage> = serde_json::from_str(messages_str).ok()?;

    let created_at = item
        .get("created_at")
        .and_then(|v| v.as_s().ok())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    let updated_at = item
        .get("updated_at")
        .and_then(|v| v.as_s().ok())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    Some(Session {
        key: key.to_string(),
        messages,
        created_at,
        updated_at,
        metadata: HashMap::new(),
    })
}
