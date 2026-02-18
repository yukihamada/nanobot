use std::sync::Arc;

use aws_sdk_dynamodb::types::AttributeValue;
use lambda_http::{run, Error};
use tracing::{info, warn};

use nanobot_core::config;
use nanobot_core::service::http::{create_router, AppState};
use nanobot_core::session::dynamo_store::DynamoSessionStore;

/// Known API key env var names to load from DynamoDB CONFIG#api_keys.
const API_KEY_NAMES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "ELEVENLABS_API_KEY",
    "REPLICATE_API_TOKEN",
];

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nanobot=info".parse().unwrap()),
        )
        .with_ansi(false)
        .init();

    info!("nanobot Lambda starting...");

    // DynamoDB session store
    let table_name = std::env::var("DYNAMODB_SESSIONS_TABLE")
        .unwrap_or_else(|_| "nanobot-sessions".to_string());
    let tenant_id = std::env::var("NANOBOT_TENANT_ID")
        .unwrap_or_else(|_| "default".to_string());

    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let config_table = std::env::var("DYNAMODB_CONFIG_TABLE")
        .unwrap_or_else(|_| "nanobot-config".to_string());

    // Load API keys from DynamoDB before building providers.
    // DynamoDB values take priority over env vars (allows admin panel override).
    match dynamo_client
        .get_item()
        .table_name(&config_table)
        .key("pk", AttributeValue::S("CONFIG#api_keys".to_string()))
        .key("sk", AttributeValue::S("LATEST".to_string()))
        .send()
        .await
    {
        Ok(output) => {
            if let Some(item) = output.item() {
                let mut loaded = 0u32;
                for &name in API_KEY_NAMES {
                    if let Some(val) = item.get(name).and_then(|v| v.as_s().ok()) {
                        if !val.is_empty() {
                            // SAFETY: single-threaded cold start, no concurrent readers yet.
                            unsafe { std::env::set_var(name, val); }
                            loaded += 1;
                        }
                    }
                }
                info!("Loaded {} API keys from DynamoDB CONFIG#api_keys", loaded);
            } else {
                info!("No CONFIG#api_keys in DynamoDB, using env vars only");
            }
        }
        Err(e) => {
            warn!("Failed to load API keys from DynamoDB: {}. Using env vars only.", e);
        }
    }

    let cfg = config::load_config_from_env();

    let session_store = DynamoSessionStore::new(dynamo_client.clone(), table_name, tenant_id);

    let mut app_state = AppState::with_provider(cfg, Box::new(session_store));
    app_state.dynamo_client = Some(dynamo_client);
    app_state.config_table = Some(config_table);

    // Load MCP tools from environment
    let mcp_tools = nanobot_core::mcp::client::load_mcp_tools_from_env().await;
    if !mcp_tools.is_empty() {
        info!("Loaded {} MCP tools", mcp_tools.len());
        app_state.tool_registry.register_all(mcp_tools);
    }
    info!("Total tools registered: {}", app_state.tool_registry.len());

    let state = Arc::new(app_state);

    let router = create_router(state);

    run(router).await?;

    Ok(())
}
