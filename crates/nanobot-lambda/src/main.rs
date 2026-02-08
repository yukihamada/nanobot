use std::sync::Arc;

use lambda_http::{run, Error};
use tracing::info;

use nanobot_core::config;
use nanobot_core::service::http::{create_router, AppState};
use nanobot_core::session::dynamo_store::DynamoSessionStore;

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

    let cfg = config::load_config_from_env();

    // DynamoDB session store
    let table_name = std::env::var("DYNAMODB_SESSIONS_TABLE")
        .unwrap_or_else(|_| "nanobot-sessions".to_string());
    let tenant_id = std::env::var("NANOBOT_TENANT_ID")
        .unwrap_or_else(|_| "default".to_string());

    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let config_table = std::env::var("DYNAMODB_CONFIG_TABLE")
        .unwrap_or_else(|_| "nanobot-config".to_string());

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
