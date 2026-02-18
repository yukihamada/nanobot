use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;
use tracing::warn;

use super::provider::ConfigProvider;
use super::Config;

/// DynamoDB-based config provider for multi-tenant SaaS.
pub struct DynamoConfigProvider {
    client: Client,
    table_name: String,
    tenant_id: String,
    /// Cached config loaded at initialization.
    cached_config: Config,
}

impl DynamoConfigProvider {
    pub async fn new(client: Client, table_name: String, tenant_id: String) -> Self {
        let cached_config = load_tenant_config(&client, &table_name, &tenant_id)
            .await
            .unwrap_or_default();

        Self {
            client,
            table_name,
            tenant_id,
            cached_config,
        }
    }
}

impl ConfigProvider for DynamoConfigProvider {
    fn load_config(&self) -> Config {
        self.cached_config.clone()
    }

    fn load_workspace_file(&self, filename: &str) -> Option<String> {
        let rt = tokio::runtime::Handle::try_current().ok()?;
        let client = self.client.clone();
        let table = self.table_name.clone();
        let tenant = self.tenant_id.clone();
        let filename = filename.to_string();

        std::thread::scope(|_| {
            rt.block_on(async {
                let resp = client
                    .get_item()
                    .table_name(&table)
                    .key("tenant_id", AttributeValue::S(tenant))
                    .key("sk", AttributeValue::S("CONFIG".to_string()))
                    .projection_expression("workspace_files")
                    .send()
                    .await;

                match resp {
                    Ok(output) => {
                        output.item.and_then(|item| {
                            item.get("workspace_files")
                                .and_then(|v| v.as_m().ok())
                                .and_then(|m| {
                                    m.get(&filename)
                                        .and_then(|v| v.as_s().ok())
                                        .cloned()
                                })
                        })
                    }
                    Err(e) => {
                        warn!("DynamoDB get workspace file error: {}", e);
                        None
                    }
                }
            })
        })
    }
}

async fn load_tenant_config(
    client: &Client,
    table_name: &str,
    tenant_id: &str,
) -> Option<Config> {
    let resp = client
        .get_item()
        .table_name(table_name)
        .key("tenant_id", AttributeValue::S(tenant_id.to_string()))
        .key("sk", AttributeValue::S("CONFIG".to_string()))
        .send()
        .await;

    match resp {
        Ok(output) => {
            let item = output.item?;

            // Try to parse provider_keys as a map
            let mut config = Config::default();

            if let Some(provider_keys) = item.get("provider_keys").and_then(|v| v.as_m().ok()) {
                // Map provider keys from DynamoDB to config
                if let Some(key) = provider_keys.get("anthropic").and_then(|v| v.as_s().ok()) {
                    config.providers.anthropic.api_key = key.clone();
                }
                if let Some(key) = provider_keys.get("openai").and_then(|v| v.as_s().ok()) {
                    config.providers.openai.api_key = key.clone();
                }
                if let Some(key) = provider_keys.get("openrouter").and_then(|v| v.as_s().ok()) {
                    config.providers.openrouter.api_key = key.clone();
                }
                if let Some(key) = provider_keys.get("gemini").and_then(|v| v.as_s().ok()) {
                    config.providers.gemini.api_key = key.clone();
                }
            }

            // Parse model from plan tier
            if let Some(plan) = item.get("plan").and_then(|v| v.as_s().ok()) {
                match plan.as_str() {
                    "free" => {
                        config.agents.defaults.model = "openai/gpt-4o-mini".to_string();
                    }
                    "starter" => {
                        config.agents.defaults.model = "openai/gpt-4o".to_string();
                    }
                    // pro and enterprise keep the default (all models available)
                    _ => {}
                }
            }

            Some(config)
        }
        Err(e) => {
            warn!("DynamoDB load tenant config error: {}", e);
            None
        }
    }
}
